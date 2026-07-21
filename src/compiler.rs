//! Lower the PHP AST to `fusevm::Chunk`.
//!
//! Arithmetic `+ - *` lowers to native fusevm ops so the JIT can trace them; the
//! strict numeric hook (host) supplies PHP coercion for non-numeric operands.
//! `/ % **`, string concat, comparisons, and everything PHP-specific — variable
//! access, arrays, function dispatch — lower to a `CallBuiltin` that lands in
//! `builtins.rs`. Conditions are normalized through the `TRUTHY` builtin before a
//! native jump, because PHP truthiness (`0`, `""`, `"0"`, `[]`, `null` are falsy)
//! differs from fusevm's default numeric truthiness.

use crate::ast::*;
use crate::host::{self, ops, CatchClause, ClassDef, FuncDef, TryDef};
use fusevm::{Chunk, ChunkBuilder, Op, Value};
use rustc_hash::FxHashMap;

/// The full output of compiling a program.
pub struct Program {
    pub main: Chunk,
    pub functions: Vec<(String, FuncDef)>,
    pub classes: Vec<(String, ClassDef)>,
    /// `try`/`catch`/`finally` constructs, indexed by the id baked into each
    /// `RUN_TRY` call.
    pub try_defs: Vec<TryDef>,
}

/// Break/continue jump fixups for the innermost loop.
struct LoopCtx {
    breaks: Vec<usize>,
    continues: Vec<usize>,
}

/// One segment of a flattened array lvalue chain: an explicit `[key]` or an
/// append `[]`.
enum LvSeg<'a> {
    Key(&'a Expr),
    Append,
}

/// The `ops::ARR_MUT` sub-op for a by-reference array mutator name, or `None` if
/// the name isn't one. These lower specially (passing the array by variable
/// name) rather than through the normal `CALL` value path.
fn array_mutator_subop(name: &str) -> Option<i64> {
    use crate::host::arrmut;
    match name.to_ascii_lowercase().as_str() {
        "array_push" => Some(arrmut::PUSH),
        "array_pop" => Some(arrmut::POP),
        "array_shift" => Some(arrmut::SHIFT),
        "array_unshift" => Some(arrmut::UNSHIFT),
        "array_splice" => Some(arrmut::SPLICE),
        _ => None,
    }
}

#[derive(Default)]
pub struct Compiler {
    functions: Vec<(String, FuncDef)>,
    classes: Vec<(String, ClassDef)>,
    /// The class currently being compiled, and its parent — used to resolve
    /// `self::`/`parent::`/`static::` to concrete class names.
    current_class: Option<String>,
    current_parent: Option<String>,
    loops: Vec<LoopCtx>,
    /// Compiled `try`/`catch`/`finally` bodies; a `RUN_TRY` call references an
    /// entry by its index here.
    try_defs: Vec<TryDef>,
    /// Nesting depth of `try`/`catch`/`finally` bodies currently being lowered.
    /// While `> 0` with no loop in the same detached chunk, `break`/`continue`
    /// lower to control signals the orchestrator relays, not in-chunk jumps.
    in_try: usize,
    /// Monotonic counter for compiler-generated temporary variable names
    /// (`foreach` desugaring), kept out of the PHP identifier space with a `@`.
    tmp: usize,
    /// By-reference parameter positions per function (lowercased name), gathered
    /// in a pre-pass so a call can write the callee's final by-ref values back to
    /// the caller's variables even when the function is declared later.
    byref_fns: FxHashMap<String, Vec<usize>>,
    /// Emit per-statement DAP line markers (`php --dap`). Off for normal runs so
    /// the compiled chunk carries zero extra ops.
    debug: bool,
}

/// Compile a parsed program. `debug` enables per-statement DAP line markers.
pub fn compile(stmts: &[Stmt], debug: bool) -> Result<Program, String> {
    let mut c = Compiler {
        debug,
        ..Compiler::default()
    };
    // Pre-pass: record by-reference parameter positions of every function so a
    // call site can write the callee's finals back even for forward references.
    c.collect_byref(stmts);
    let mut b = ChunkBuilder::new();
    c.compile_seq(&mut b, stmts)?;
    Ok(Program {
        main: b.build(),
        functions: c.functions,
        classes: c.classes,
        try_defs: c.try_defs,
    })
}

impl Compiler {
    fn tmp_name(&mut self, tag: &str) -> String {
        self.tmp += 1;
        format!("@{tag}{}", self.tmp)
    }

    /// Compile a formal parameter list, lowering each default-value expression to
    /// its own chunk (run in the callee frame when the argument is omitted).
    /// Shared by named functions, closures, and methods.
    fn compile_params(&mut self, params: &[Param]) -> Result<Vec<host::Param>, String> {
        let mut out = Vec::with_capacity(params.len());
        for p in params {
            let default = match &p.default {
                Some(expr) => {
                    let mut db = ChunkBuilder::new();
                    self.compile_expr(&mut db, expr)?;
                    Some(db.build())
                }
                None => None,
            };
            out.push(host::Param {
                name: p.name.clone(),
                default,
                variadic: p.variadic,
                by_ref: p.by_ref,
            });
        }
        Ok(out)
    }

    /// Pre-pass: record the by-reference parameter positions of every `function`
    /// declaration (recursing into nested bodies) so call sites can write the
    /// callee's finals back to the caller — even for forward references.
    fn collect_byref(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            match &s.kind {
                StmtKind::Function { name, params, body } => {
                    let positions: Vec<usize> = params
                        .iter()
                        .enumerate()
                        .filter(|(_, p)| p.by_ref)
                        .map(|(i, _)| i)
                        .collect();
                    if !positions.is_empty() {
                        self.byref_fns.insert(name.to_ascii_lowercase(), positions);
                    }
                    self.collect_byref(body);
                }
                StmtKind::If {
                    then, elifs, els, ..
                } => {
                    self.collect_byref(then);
                    for (_, body) in elifs {
                        self.collect_byref(body);
                    }
                    if let Some(e) = els {
                        self.collect_byref(e);
                    }
                }
                StmtKind::While { body, .. }
                | StmtKind::DoWhile { body, .. }
                | StmtKind::For { body, .. }
                | StmtKind::Foreach { body, .. }
                | StmtKind::Block(body) => self.collect_byref(body),
                StmtKind::Switch { cases, .. } => {
                    for c in cases {
                        self.collect_byref(&c.body);
                    }
                }
                StmtKind::Try {
                    body,
                    catches,
                    finally,
                } => {
                    self.collect_byref(body);
                    for c in catches {
                        self.collect_byref(&c.body);
                    }
                    if let Some(f) = finally {
                        self.collect_byref(f);
                    }
                }
                _ => {}
            }
        }
    }

    fn compile_seq(&mut self, b: &mut ChunkBuilder, body: &[Stmt]) -> Result<(), String> {
        for s in body {
            self.compile_stmt(b, s)?;
        }
        Ok(())
    }

    fn compile_stmt(&mut self, b: &mut ChunkBuilder, s: &Stmt) -> Result<(), String> {
        // Under `--dap` each statement is preceded by a `DBG_LINE` marker so the
        // debugger can stop on it; the builtin returns Undef, popped immediately.
        if self.debug && s.line != 0 {
            b.emit(Op::LoadInt(s.line as i64), s.line);
            b.emit(Op::CallBuiltin(ops::DBG_LINE, 1), s.line);
            b.emit(Op::Pop, s.line);
        }
        let line = s.line;
        match &s.kind {
            StmtKind::InlineHtml(text) => {
                let idx = b.add_constant(Value::str(text.clone()));
                b.emit(Op::LoadConst(idx), line);
                b.emit(Op::CallBuiltin(ops::ECHO, 1), line);
                b.emit(Op::Pop, line);
            }
            StmtKind::Echo(args) => {
                for a in args {
                    self.compile_expr(b, a)?;
                }
                b.emit(Op::CallBuiltin(ops::ECHO, args.len() as u8), line);
                b.emit(Op::Pop, line);
            }
            StmtKind::Expr(e) => {
                self.compile_expr(b, e)?;
                b.emit(Op::Pop, line);
            }
            StmtKind::Block(body) => self.compile_seq(b, body)?,
            StmtKind::Return(e) => {
                match e {
                    Some(e) => self.compile_expr(b, e)?,
                    None => {
                        b.emit(Op::LoadUndef, line);
                    }
                }
                b.emit(Op::CallBuiltin(ops::SIG_RETURN, 1), line);
                b.emit(Op::Pop, line);
            }
            StmtKind::Break => {
                // Inside a loop in this chunk → an in-chunk jump. Inside a `try`
                // body with no such loop → a control signal the orchestrator
                // relays to the enclosing loop.
                if let Some(ctx) = self.loops.last_mut() {
                    let j = b.emit(Op::Jump(0), line);
                    ctx.breaks.push(j);
                } else if self.in_try > 0 {
                    b.emit(Op::CallBuiltin(ops::SIG_BREAK, 0), line);
                    b.emit(Op::Pop, line);
                } else {
                    return Err("'break' outside of a loop".into());
                }
            }
            StmtKind::Continue => {
                if let Some(ctx) = self.loops.last_mut() {
                    let j = b.emit(Op::Jump(0), line);
                    ctx.continues.push(j);
                } else if self.in_try > 0 {
                    b.emit(Op::CallBuiltin(ops::SIG_CONTINUE, 0), line);
                    b.emit(Op::Pop, line);
                } else {
                    return Err("'continue' outside of a loop".into());
                }
            }
            StmtKind::Try {
                body,
                catches,
                finally,
            } => self.compile_try(b, body, catches, finally.as_deref(), line)?,
            StmtKind::Function { name, params, body } => {
                // Each default-value expression is lowered to its own tiny chunk,
                // run in the callee frame when the argument is omitted (host).
                let cparams = self.compile_params(params)?;
                let mut fb = ChunkBuilder::new();
                // A function body has its own loop scope: a break inside it must
                // not target a loop at the call site.
                let saved = std::mem::take(&mut self.loops);
                self.compile_seq(&mut fb, body)?;
                self.loops = saved;
                self.functions.push((
                    name.to_ascii_lowercase(),
                    FuncDef {
                        params: cparams,
                        chunk: fb.build(),
                    },
                ));
            }
            StmtKind::Class(decl) => self.compile_class(decl)?,
            StmtKind::If {
                cond,
                then,
                elifs,
                els,
            } => self.compile_if(b, cond, then, elifs, els)?,
            StmtKind::While { cond, body } => self.compile_while(b, cond, body)?,
            StmtKind::DoWhile { cond, body } => self.compile_do_while(b, cond, body)?,
            StmtKind::Switch { subj, cases } => self.compile_switch(b, subj, cases)?,
            StmtKind::For {
                init,
                cond,
                step,
                body,
            } => self.compile_for(b, init, cond.as_ref(), step, body)?,
            StmtKind::Foreach {
                arr,
                key_var,
                val_var,
                by_ref,
                body,
            } => self.compile_foreach(b, arr, key_var.as_deref(), val_var, *by_ref, body)?,
        }
        Ok(())
    }

    fn compile_if(
        &mut self,
        b: &mut ChunkBuilder,
        cond: &Expr,
        then: &[Stmt],
        elifs: &[(Expr, Vec<Stmt>)],
        els: &Option<Vec<Stmt>>,
    ) -> Result<(), String> {
        // Flatten if/elseif/else into a chain; each arm jumps to the shared end.
        let mut ends: Vec<usize> = Vec::new();
        let arms: Vec<(&Expr, &[Stmt])> = std::iter::once((cond, then))
            .chain(elifs.iter().map(|(c, body)| (c, body.as_slice())))
            .collect();

        for (c, body) in arms {
            self.compile_truthy(b, c)?;
            let next = b.emit(Op::JumpIfFalse(0), 0);
            self.compile_seq(b, body)?;
            ends.push(b.emit(Op::Jump(0), 0));
            let here = b.current_pos();
            b.patch_jump(next, here);
        }
        if let Some(body) = els {
            self.compile_seq(b, body)?;
        }
        let end = b.current_pos();
        for j in ends {
            b.patch_jump(j, end);
        }
        Ok(())
    }

    fn compile_while(
        &mut self,
        b: &mut ChunkBuilder,
        cond: &Expr,
        body: &[Stmt],
    ) -> Result<(), String> {
        let top = b.current_pos();
        self.compile_truthy(b, cond)?;
        let exit = b.emit(Op::JumpIfFalse(0), 0);
        self.loops.push(LoopCtx {
            breaks: vec![],
            continues: vec![],
        });
        self.compile_seq(b, body)?;
        let ctx = self.loops.pop().unwrap();
        b.emit(Op::Jump(top), 0);
        let end = b.current_pos();
        b.patch_jump(exit, end);
        for j in ctx.breaks {
            b.patch_jump(j, end);
        }
        for j in ctx.continues {
            b.patch_jump(j, top);
        }
        Ok(())
    }

    fn compile_do_while(
        &mut self,
        b: &mut ChunkBuilder,
        cond: &Expr,
        body: &[Stmt],
    ) -> Result<(), String> {
        // The body runs once before the condition is ever tested.
        let top = b.current_pos();
        self.loops.push(LoopCtx {
            breaks: vec![],
            continues: vec![],
        });
        self.compile_seq(b, body)?;
        let ctx = self.loops.pop().unwrap();
        // `continue` re-tests the condition; `break` exits.
        let cond_pos = b.current_pos();
        self.compile_truthy(b, cond)?;
        b.emit(Op::JumpIfTrue(top), 0);
        let end = b.current_pos();
        for j in ctx.breaks {
            b.patch_jump(j, end);
        }
        for j in ctx.continues {
            b.patch_jump(j, cond_pos);
        }
        Ok(())
    }

    /// `switch`: evaluate the subject once, dispatch to the first `case` whose
    /// value is loosely (`==`) equal (or `default`), then run bodies in source
    /// order so fall-through is natural. `break` exits the switch.
    fn compile_switch(
        &mut self,
        b: &mut ChunkBuilder,
        subj: &Expr,
        cases: &[SwitchCase],
    ) -> Result<(), String> {
        let sw_t = self.tmp_name("sw");
        self.emit_set_var(b, &sw_t, |c, b| c.compile_expr(b, subj))?;

        // Dispatch chain: `@sw == case_value` for each non-default case.
        let mut dispatch: Vec<(usize, usize)> = Vec::new(); // (case index, JumpIfTrue pos)
        let mut default_index: Option<usize> = None;
        for (i, case) in cases.iter().enumerate() {
            match &case.test {
                Some(test) => {
                    self.emit_get_var(b, &sw_t);
                    self.compile_expr(b, test)?;
                    b.emit(Op::CallBuiltin(ops::LOOSE_EQ, 2), 0);
                    b.emit(Op::CallBuiltin(ops::TRUTHY, 1), 0);
                    let jt = b.emit(Op::JumpIfTrue(0), 0);
                    dispatch.push((i, jt));
                }
                None => default_index = Some(i),
            }
        }
        // No case matched: fall to `default` if present, else past the switch.
        let fallthrough = b.emit(Op::Jump(0), 0);

        // Bodies, emitted in source order (no jumps between them → fall-through).
        self.loops.push(LoopCtx {
            breaks: vec![],
            continues: vec![],
        });
        let mut body_starts = Vec::with_capacity(cases.len());
        for case in cases {
            body_starts.push(b.current_pos());
            self.compile_seq(b, &case.body)?;
        }
        let ctx = self.loops.pop().unwrap();
        let end = b.current_pos();

        for (i, jt) in dispatch {
            b.patch_jump(jt, body_starts[i]);
        }
        match default_index {
            Some(di) => b.patch_jump(fallthrough, body_starts[di]),
            None => b.patch_jump(fallthrough, end),
        }
        for j in ctx.breaks {
            b.patch_jump(j, end);
        }
        // `continue` inside a switch acts like `break` of the switch (PHP treats
        // the switch as a loop level; `continue 1` exits it).
        for j in ctx.continues {
            b.patch_jump(j, end);
        }
        Ok(())
    }

    fn compile_for(
        &mut self,
        b: &mut ChunkBuilder,
        init: &[Expr],
        cond: Option<&Expr>,
        step: &[Expr],
        body: &[Stmt],
    ) -> Result<(), String> {
        for e in init {
            self.compile_expr(b, e)?;
            b.emit(Op::Pop, 0);
        }
        let top = b.current_pos();
        let exit = match cond {
            Some(c) => {
                self.compile_truthy(b, c)?;
                Some(b.emit(Op::JumpIfFalse(0), 0))
            }
            None => None,
        };
        self.loops.push(LoopCtx {
            breaks: vec![],
            continues: vec![],
        });
        self.compile_seq(b, body)?;
        let ctx = self.loops.pop().unwrap();
        // `continue` in a for-loop jumps to the step, not the condition.
        let step_pos = b.current_pos();
        for e in step {
            self.compile_expr(b, e)?;
            b.emit(Op::Pop, 0);
        }
        b.emit(Op::Jump(top), 0);
        let end = b.current_pos();
        if let Some(exit) = exit {
            b.patch_jump(exit, end);
        }
        for j in ctx.breaks {
            b.patch_jump(j, end);
        }
        for j in ctx.continues {
            b.patch_jump(j, step_pos);
        }
        Ok(())
    }

    /// `foreach ($arr as [$k =>] $v) { body }` desugars to iterating the array's
    /// key list by index, binding `$v` (and `$k`) from a hidden temporary.
    fn compile_foreach(
        &mut self,
        b: &mut ChunkBuilder,
        arr: &Expr,
        key_var: Option<&str>,
        val_var: &str,
        by_ref: bool,
        body: &[Stmt],
    ) -> Result<(), String> {
        let arr_t = self.tmp_name("arr");
        let keys_t = self.tmp_name("keys");
        let i_t = self.tmp_name("i");

        // @arr = foreach_prep(<arr>);  @keys = array_keys(@arr);  @i = 0;
        // FOREACH_PREP passes arrays through and materializes an iterable object
        // (Iterator / IteratorAggregate / public properties) into an array.
        self.emit_set_var(b, &arr_t, |c, b| {
            c.compile_expr(b, arr)?;
            b.emit(Op::CallBuiltin(ops::FOREACH_PREP, 1), 0);
            Ok(())
        })?;
        self.emit_set_var(b, &keys_t, |c, b| {
            c.emit_get_var(b, &arr_t);
            b.emit(Op::CallBuiltin(ops::ARRAYKEYS, 1), 0);
            Ok(())
        })?;
        self.emit_set_var(b, &i_t, |_, b| {
            b.emit(Op::LoadInt(0), 0);
            Ok(())
        })?;

        let top = b.current_pos();
        // while (@i < count(@keys))
        self.emit_get_var(b, &i_t);
        self.emit_get_var(b, &keys_t);
        b.emit(Op::CallBuiltin(ops::ARRAYLEN, 1), 0);
        b.emit(Op::CallBuiltin(ops::LT, 2), 0);
        b.emit(Op::CallBuiltin(ops::TRUTHY, 1), 0);
        let exit = b.emit(Op::JumpIfFalse(0), 0);

        // @k = @keys[@i];  bind key var if present.
        let k_t = self.tmp_name("k");
        self.emit_set_var(b, &k_t, |c, b| {
            c.emit_get_var(b, &keys_t);
            c.emit_get_var(b, &i_t);
            b.emit(Op::CallBuiltin(ops::INDEX_GET, 2), 0);
            Ok(())
        })?;
        if let Some(kv) = key_var {
            self.emit_set_var(b, kv, |c, b| {
                c.emit_get_var(b, &k_t);
                Ok(())
            })?;
        }
        // $v = @arr[@k];
        self.emit_set_var(b, val_var, |c, b| {
            c.emit_get_var(b, &arr_t);
            c.emit_get_var(b, &k_t);
            b.emit(Op::CallBuiltin(ops::INDEX_GET, 2), 0);
            Ok(())
        })?;

        self.loops.push(LoopCtx {
            breaks: vec![],
            continues: vec![],
        });
        self.compile_seq(b, body)?;
        let ctx = self.loops.pop().unwrap();

        // `continue` lands here; for a by-reference foreach the (possibly
        // modified) value is written back into the array element first. `@arr`
        // shares the source array's handle, so the write is visible to the caller.
        let cont_target = b.current_pos();
        if by_ref {
            let nidx = b.add_constant(Value::str(arr_t.clone()));
            b.emit(Op::LoadConst(nidx), 0); // name = @arr
            self.emit_get_var(b, &k_t); // key = @k
            self.emit_get_var(b, val_var); // val = $v
            b.emit(Op::CallBuiltin(ops::INDEX_SET, 3), 0);
            b.emit(Op::Pop, 0);
        }

        // @i = @i + 1;
        self.emit_set_var(b, &i_t, |c, b| {
            c.emit_get_var(b, &i_t);
            b.emit(Op::LoadInt(1), 0);
            b.emit(Op::Add, 0);
            Ok(())
        })?;
        b.emit(Op::Jump(top), 0);
        let end = b.current_pos();
        b.patch_jump(exit, end);
        for j in ctx.breaks {
            b.patch_jump(j, end);
        }
        for j in ctx.continues {
            b.patch_jump(j, cont_target);
        }
        Ok(())
    }

    /// Lower a class declaration to a `ClassDef`: constant and property-default
    /// initializers become standalone expression chunks (each leaving its value
    /// on the stack), and each method body compiles like a free function. A
    /// constructor with promoted parameters (`public int $x`) gets a synthetic
    /// `$this->x = $x;` prepended for each promoted parameter.
    fn compile_class(&mut self, decl: &ClassDecl) -> Result<(), String> {
        let prev_class = self.current_class.take();
        let prev_parent = self.current_parent.take();
        self.current_class = Some(decl.name.clone());
        self.current_parent = decl.parent.clone();

        // Seed members from any used traits (declared earlier); the class's own
        // members below override them, matching PHP trait precedence.
        let mut consts: Vec<(String, Chunk)> = Vec::new();
        let mut prop_defaults: Vec<(String, Chunk)> = Vec::new();
        let mut methods: FxHashMap<String, FuncDef> = FxHashMap::default();
        for tname in &decl.uses {
            let tl = tname.to_ascii_lowercase();
            if let Some((_, tdef)) = self.classes.iter().find(|(n, _)| *n == tl) {
                for (n, c) in &tdef.consts {
                    consts.push((n.clone(), c.clone()));
                }
                for (n, c) in &tdef.prop_defaults {
                    prop_defaults.push((n.clone(), c.clone()));
                }
                for (n, m) in &tdef.methods {
                    methods.insert(n.clone(), m.clone());
                }
            }
        }

        for (name, expr) in &decl.consts {
            let mut cb = ChunkBuilder::new();
            self.compile_expr(&mut cb, expr)?;
            consts.retain(|(n, _)| n != name);
            consts.push((name.clone(), cb.build()));
        }

        for (name, default) in &decl.props {
            let mut pb = ChunkBuilder::new();
            match default {
                Some(e) => self.compile_expr(&mut pb, e)?,
                None => {
                    pb.emit(Op::LoadUndef, 0);
                }
            }
            prop_defaults.retain(|(n, _)| n != name);
            prop_defaults.push((name.clone(), pb.build()));
        }

        for m in &decl.methods {
            let cparams = self.compile_params(&m.params)?;
            let mut mb = ChunkBuilder::new();
            // A method body has its own loop scope (as free functions do).
            let saved = std::mem::take(&mut self.loops);
            // Constructor property promotion: `public int $x` also assigns
            // `$this->x = $x` before the body runs.
            if m.name.eq_ignore_ascii_case("__construct") {
                for p in m.params.iter().filter(|p| p.promoted) {
                    let assign = Expr::Assign(
                        Box::new(Expr::PropGet(
                            Box::new(Expr::Var("this".to_string())),
                            p.name.clone(),
                        )),
                        None,
                        Box::new(Expr::Var(p.name.clone())),
                    );
                    self.compile_expr(&mut mb, &assign)?;
                    mb.emit(Op::Pop, 0);
                }
            }
            self.compile_seq(&mut mb, &m.body)?;
            self.loops = saved;
            methods.insert(
                m.name.to_ascii_lowercase(),
                FuncDef {
                    params: cparams,
                    chunk: mb.build(),
                },
            );
        }

        self.classes.push((
            decl.name.to_ascii_lowercase(),
            ClassDef {
                parent: decl.parent.clone(),
                interfaces: decl.implements.clone(),
                consts,
                prop_defaults,
                methods,
            },
        ));

        self.current_class = prev_class;
        self.current_parent = prev_parent;
        Ok(())
    }

    /// Resolve a class reference to a concrete name, expanding the `self`,
    /// `parent`, and `static` keywords against the class being compiled.
    fn resolve_class_name(&self, name: &str) -> Result<String, String> {
        match name.to_ascii_lowercase().as_str() {
            "self" | "static" => self
                .current_class
                .clone()
                .ok_or_else(|| format!("'{name}' used outside of a class")),
            "parent" => self
                .current_parent
                .clone()
                .ok_or_else(|| "'parent' used in a class with no parent".to_string()),
            _ => Ok(name.to_string()),
        }
    }

    /// Compile a `body` into its own detached chunk (its own loop scope, and a
    /// `try`-body context so a bare `break`/`continue` becomes a control signal
    /// relayed by the orchestrator to the enclosing loop).
    fn compile_detached(&mut self, body: &[Stmt]) -> Result<Chunk, String> {
        let mut fb = ChunkBuilder::new();
        // A detached body must not see the enclosing loop's break/continue
        // fixups — those live in the parent chunk, unreachable from here.
        let saved = std::mem::take(&mut self.loops);
        self.in_try += 1;
        let r = self.compile_seq(&mut fb, body);
        self.in_try -= 1;
        self.loops = saved;
        r?;
        Ok(fb.build())
    }

    /// `try { } catch (T|U $e) { } finally { }` — each body is a detached chunk
    /// run by the `RUN_TRY` orchestrator, which returns a control status. The
    /// parent branches on that status: normal falls through; a pending
    /// return/throw re-halts this chunk to propagate it to the enclosing frame;
    /// break/continue jump to the enclosing loop's fixups.
    fn compile_try(
        &mut self,
        b: &mut ChunkBuilder,
        body: &[Stmt],
        catches: &[CatchArm],
        finally: Option<&[Stmt]>,
        line: u32,
    ) -> Result<(), String> {
        let try_chunk = self.compile_detached(body)?;
        let mut cc = Vec::with_capacity(catches.len());
        for c in catches {
            cc.push(CatchClause {
                classes: c.types.clone(),
                var: c.var.clone(),
                chunk: self.compile_detached(&c.body)?,
            });
        }
        let finally_chunk = match finally {
            Some(f) => Some(self.compile_detached(f)?),
            None => None,
        };
        let id = self.try_defs.len() as i64;
        self.try_defs.push(TryDef {
            try_chunk,
            catches: cc,
            finally_chunk,
        });

        // RUN_TRY leaves the control status int on the stack.
        b.emit(Op::LoadInt(id), line);
        b.emit(Op::CallBuiltin(ops::RUN_TRY, 1), line);

        // Dispatch on the status: 1 return, 2 throw, 3 break, 4 continue, 0 normal.
        let j_ret = self.branch_if_status(b, 1, line);
        let j_throw = self.branch_if_status(b, 2, line);
        let j_break = self.branch_if_status(b, 3, line);
        let j_cont = self.branch_if_status(b, 4, line);
        // Normal: discard the status and continue past the construct.
        b.emit(Op::Pop, line);
        let j_end = b.emit(Op::Jump(0), line);

        // Return/throw: the value is already stashed in the host signal; drop the
        // status int and halt this chunk so the enclosing frame propagates it.
        let ret_pos = b.current_pos();
        b.patch_jump(j_ret, ret_pos);
        b.emit(Op::Pop, line);
        b.emit(Op::CallBuiltin(ops::SIG_HALT, 0), line);
        b.emit(Op::Pop, line);

        let throw_pos = b.current_pos();
        b.patch_jump(j_throw, throw_pos);
        b.emit(Op::Pop, line);
        b.emit(Op::CallBuiltin(ops::SIG_HALT, 0), line);
        b.emit(Op::Pop, line);

        // Break/continue: jump to the enclosing loop's fixups (registered here so
        // a `break` inside a try-in-loop reaches the right loop). With no loop
        // present the status is simply discarded (PHP would reject it earlier).
        let break_pos = b.current_pos();
        b.patch_jump(j_break, break_pos);
        b.emit(Op::Pop, line);
        if self.loops.last().is_some() {
            let j = b.emit(Op::Jump(0), line);
            self.loops.last_mut().unwrap().breaks.push(j);
        }

        let cont_pos = b.current_pos();
        b.patch_jump(j_cont, cont_pos);
        b.emit(Op::Pop, line);
        if self.loops.last().is_some() {
            let j = b.emit(Op::Jump(0), line);
            self.loops.last_mut().unwrap().continues.push(j);
        }

        let end = b.current_pos();
        b.patch_jump(j_end, end);
        Ok(())
    }

    /// Emit `Dup; status === code; if true jump`, returning the pending jump idx.
    /// Leaves the status int on the stack on the fall-through path.
    fn branch_if_status(&mut self, b: &mut ChunkBuilder, code: i64, line: u32) -> usize {
        b.emit(Op::Dup, line);
        b.emit(Op::LoadInt(code), line);
        b.emit(Op::CallBuiltin(ops::STRICT_EQ, 2), line);
        b.emit(Op::CallBuiltin(ops::TRUTHY, 1), line);
        b.emit(Op::JumpIfTrue(0), line)
    }

    // ── expressions ────────────────────────────────────────────────────────

    fn compile_expr(&mut self, b: &mut ChunkBuilder, e: &Expr) -> Result<(), String> {
        match e {
            Expr::Null => {
                b.emit(Op::LoadUndef, 0);
            }
            Expr::Bool(v) => {
                b.emit(if *v { Op::LoadTrue } else { Op::LoadFalse }, 0);
            }
            Expr::Int(n) => {
                b.emit(Op::LoadInt(*n), 0);
            }
            Expr::Float(f) => {
                b.emit(Op::LoadFloat(*f), 0);
            }
            Expr::Str(s) => {
                let idx = b.add_constant(Value::str(s.clone()));
                b.emit(Op::LoadConst(idx), 0);
            }
            Expr::Interp(parts) => self.compile_interp(b, parts),
            Expr::Var(name) => self.emit_get_var(b, name),
            Expr::Array(elems) => {
                for (k, v) in elems {
                    match k {
                        Some(k) => self.compile_expr(b, k)?,
                        None => {
                            b.emit(Op::LoadUndef, 0);
                        }
                    }
                    self.compile_expr(b, v)?;
                }
                b.emit(Op::CallBuiltin(ops::MKARRAY, (elems.len() * 2) as u8), 0);
            }
            Expr::Index(recv, idx) => {
                self.compile_expr(b, recv)?;
                self.compile_expr(b, idx)?;
                b.emit(Op::CallBuiltin(ops::INDEX_GET, 2), 0);
            }
            Expr::Append(_) => {
                return Err("'[]' append is only valid as an assignment target".into())
            }
            Expr::Unary(op, e) => {
                self.compile_expr(b, e)?;
                match op {
                    UnOp::Neg => {
                        b.emit(Op::Negate, 0);
                    }
                    UnOp::Pos => {} // `+$x` is numeric identity in the scaffold
                    UnOp::Not => {
                        b.emit(Op::CallBuiltin(ops::TRUTHY, 1), 0);
                        b.emit(Op::LogNot, 0);
                    }
                    UnOp::BitNot => {
                        b.emit(Op::CallBuiltin(ops::BITNOT, 1), 0);
                    }
                }
            }
            Expr::Binary(op, l, r) => self.compile_binary(b, *op, l, r)?,
            Expr::Assign(lhs, op, rhs) => self.compile_assign(b, lhs, *op, rhs)?,
            Expr::IncDec {
                target,
                inc,
                prefix,
            } => self.compile_incdec(b, target, *inc, *prefix)?,
            Expr::Call(name, args) => {
                let has_spread = args.iter().any(|a| matches!(a, Expr::Spread(_)));
                // The by-reference array mutators take their array by variable
                // name so the host can rewrite (and auto-vivify) it in place. A
                // spread among the arguments falls through to the normal dispatch.
                if let (false, Some(sub), Some(Expr::Var(vname))) =
                    (has_spread, array_mutator_subop(name), args.first())
                {
                    let nidx = b.add_constant(Value::str(vname.clone()));
                    b.emit(Op::LoadConst(nidx), 0);
                    b.emit(Op::LoadInt(sub), 0);
                    for a in &args[1..] {
                        self.compile_expr(b, a)?;
                    }
                    // argc = name + subop + the remaining value arguments.
                    b.emit(Op::CallBuiltin(ops::ARR_MUT, (args.len() + 1) as u8), 0);
                } else if has_spread {
                    // Any `...$arr` argument switches to the spread dispatch: each
                    // argument is pushed as a `(is_spread, value)` pair so the host
                    // can flatten spread arrays into the positional argument list.
                    let idx = b.add_constant(Value::str(name.clone()));
                    b.emit(Op::LoadConst(idx), 0);
                    for a in args {
                        match a {
                            Expr::Spread(inner) => {
                                b.emit(Op::LoadTrue, 0);
                                self.compile_expr(b, inner)?;
                            }
                            _ => {
                                b.emit(Op::LoadFalse, 0);
                                self.compile_expr(b, a)?;
                            }
                        }
                    }
                    b.emit(
                        Op::CallBuiltin(ops::CALL_SPREAD, (args.len() * 2 + 1) as u8),
                        0,
                    );
                } else {
                    let idx = b.add_constant(Value::str(name.clone()));
                    b.emit(Op::LoadConst(idx), 0);
                    for a in args {
                        self.compile_expr(b, a)?;
                    }
                    b.emit(Op::CallBuiltin(ops::CALL, (args.len() + 1) as u8), 0);
                    // By-reference parameters: write the callee's final values back
                    // to the caller's argument variables (leaving the call result).
                    if let Some(positions) = self.byref_fns.get(&name.to_ascii_lowercase()).cloned()
                    {
                        for pos in positions {
                            if let Some(Expr::Var(vname)) = args.get(pos) {
                                let nidx = b.add_constant(Value::str(vname.clone()));
                                b.emit(Op::LoadConst(nidx), 0);
                                b.emit(Op::LoadInt(pos as i64), 0);
                                b.emit(Op::CallBuiltin(ops::BYREF_OUT, 1), 0);
                                b.emit(Op::CallBuiltin(ops::SETVAR, 2), 0);
                                b.emit(Op::Pop, 0);
                            }
                        }
                    }
                }
            }
            Expr::Spread(_) => {
                return Err("'...' argument unpacking is only valid in a function call".into())
            }
            Expr::CallValue(callee, args) => {
                self.compile_expr(b, callee)?;
                for a in args {
                    self.compile_expr(b, a)?;
                }
                b.emit(Op::CallBuiltin(ops::CALL_VALUE, (args.len() + 1) as u8), 0);
            }
            Expr::Closure { params, uses, body } => {
                self.compile_closure(b, params, uses, body)?;
            }
            Expr::ArrowFn { params, body } => {
                // An arrow fn desugars to a closure whose single-statement body
                // returns the expression; it captures every free variable of the
                // body (minus its own parameters) by value.
                let ret = vec![Stmt {
                    line: 0,
                    kind: StmtKind::Return(Some((**body).clone())),
                }];
                let mut captures = Vec::new();
                collect_free_vars(body, &mut captures);
                captures.retain(|n| !params.iter().any(|p| p.name == *n));
                self.compile_closure(b, params, &captures, &ret)?;
            }
            Expr::New(class, args) => {
                let cname = self.resolve_class_name(class)?;
                let idx = b.add_constant(Value::str(cname));
                b.emit(Op::LoadConst(idx), 0);
                for a in args {
                    self.compile_expr(b, a)?;
                }
                b.emit(Op::CallBuiltin(ops::NEW, (args.len() + 1) as u8), 0);
            }
            Expr::PropGet(recv, name) => {
                self.compile_expr(b, recv)?;
                let idx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(idx), 0);
                b.emit(Op::CallBuiltin(ops::PROP_GET, 2), 0);
            }
            Expr::MethodCall(recv, name, args) => {
                self.compile_expr(b, recv)?;
                let idx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(idx), 0);
                for a in args {
                    self.compile_expr(b, a)?;
                }
                b.emit(Op::CallBuiltin(ops::MCALL, (args.len() + 2) as u8), 0);
            }
            Expr::StaticGet(class, name) => {
                let cname = self.resolve_class_name(class)?;
                // `Class::class` / `self::class` yields the resolved class-name
                // string, not a class constant.
                if name.eq_ignore_ascii_case("class") {
                    let idx = b.add_constant(Value::str(cname));
                    b.emit(Op::LoadConst(idx), 0);
                } else {
                    let cidx = b.add_constant(Value::str(cname));
                    b.emit(Op::LoadConst(cidx), 0);
                    let nidx = b.add_constant(Value::str(name.clone()));
                    b.emit(Op::LoadConst(nidx), 0);
                    b.emit(Op::CallBuiltin(ops::SCONST, 2), 0);
                }
            }
            Expr::StaticCall(class, name, args) => {
                let cname = self.resolve_class_name(class)?;
                let cidx = b.add_constant(Value::str(cname));
                b.emit(Op::LoadConst(cidx), 0);
                let nidx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(nidx), 0);
                for a in args {
                    self.compile_expr(b, a)?;
                }
                b.emit(Op::CallBuiltin(ops::SCALL, (args.len() + 2) as u8), 0);
            }
            Expr::Ternary(c, t, f) => {
                self.compile_truthy(b, c)?;
                let jf = b.emit(Op::JumpIfFalse(0), 0);
                self.compile_expr(b, t)?;
                let jend = b.emit(Op::Jump(0), 0);
                let els = b.current_pos();
                b.patch_jump(jf, els);
                self.compile_expr(b, f)?;
                let end = b.current_pos();
                b.patch_jump(jend, end);
            }
            Expr::Elvis(a, els) => {
                // `a ?: b` — evaluate `a` once; keep it if truthy, else use `b`.
                self.compile_expr(b, a)?; // [a]
                b.emit(Op::Dup, 0); // [a, a]
                b.emit(Op::CallBuiltin(ops::TRUTHY, 1), 0); // [a, bool]
                let keep = b.emit(Op::JumpIfTrue(0), 0); // truthy → keep a, leaving [a]
                b.emit(Op::Pop, 0); // discard a
                self.compile_expr(b, els)?; // [b]
                let jend = b.emit(Op::Jump(0), 0);
                let keep_pos = b.current_pos();
                b.patch_jump(keep, keep_pos);
                let end = b.current_pos();
                b.patch_jump(jend, end);
            }
            Expr::Coalesce(a, els) => {
                // `a ?? b` — use `b` only when `a` is null (=== null).
                self.compile_expr(b, a)?; // [a]
                b.emit(Op::Dup, 0); // [a, a]
                b.emit(Op::LoadUndef, 0); // [a, a, null]
                b.emit(Op::CallBuiltin(ops::STRICT_EQ, 2), 0); // [a, a===null]
                b.emit(Op::CallBuiltin(ops::TRUTHY, 1), 0); // [a, bool]
                let use_b = b.emit(Op::JumpIfTrue(0), 0); // a is null → use b
                let jend = b.emit(Op::Jump(0), 0); // a not null → keep a, leaving [a]
                let use_b_pos = b.current_pos();
                b.patch_jump(use_b, use_b_pos);
                b.emit(Op::Pop, 0); // discard a
                self.compile_expr(b, els)?; // [b]
                let end = b.current_pos();
                b.patch_jump(jend, end);
            }
            Expr::Match { subj, arms } => self.compile_match(b, subj, arms)?,
            Expr::Throw(inner) => {
                // Evaluate the exception object, record it as pending, and unwind
                // the current chunk. As an expression it produces no value, but
                // the THROW builtin leaves an Undef the surrounding context pops.
                self.compile_expr(b, inner)?;
                b.emit(Op::CallBuiltin(ops::THROW, 1), 0);
            }
            Expr::ConstFetch(name) => {
                let idx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(idx), 0);
                b.emit(Op::CallBuiltin(ops::CONST_FETCH, 1), 0);
            }
            Expr::Unset(targets) => {
                for t in targets {
                    self.compile_unset_target(b, t)?;
                }
                // `unset(...)` is a statement construct; it evaluates to null.
                b.emit(Op::LoadUndef, 0);
            }
            Expr::InstanceOf(e, class) => {
                self.compile_expr(b, e)?;
                let cname = self.resolve_class_name(class)?;
                let idx = b.add_constant(Value::str(cname));
                b.emit(Op::LoadConst(idx), 0);
                b.emit(Op::CallBuiltin(ops::INSTANCEOF, 2), 0);
            }
            Expr::RefAssign(lhs, rhs) => {
                let (Expr::Var(t), Expr::Var(s)) = (lhs.as_ref(), rhs.as_ref()) else {
                    return Err("reference `= &` supports only `$a = &$b` between variables".into());
                };
                let ti = b.add_constant(Value::str(t.clone()));
                b.emit(Op::LoadConst(ti), 0);
                let si = b.add_constant(Value::str(s.clone()));
                b.emit(Op::LoadConst(si), 0);
                b.emit(Op::CallBuiltin(ops::REF_BIND, 2), 0);
            }
        }
        Ok(())
    }

    /// Compile one `unset()` target: a plain `$var` (remove the scope variable) or
    /// an array element `$a[k1]..[kN]` (remove the deepest key).
    fn compile_unset_target(&mut self, b: &mut ChunkBuilder, t: &Expr) -> Result<(), String> {
        match t {
            Expr::Var(name) => {
                let idx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(idx), 0);
                b.emit(Op::CallBuiltin(ops::UNSET_VAR, 1), 0);
                b.emit(Op::Pop, 0);
            }
            Expr::Index(..) => {
                let (root, segs) = Self::flatten_segments(t)?;
                let Expr::Var(name) = root else {
                    return Err("unset() supports only `$var` and `$var[...]` targets".into());
                };
                let nidx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(nidx), 0);
                for seg in &segs {
                    match seg {
                        LvSeg::Key(k) => self.compile_expr(b, k)?,
                        LvSeg::Append => return Err("cannot unset an `[]` append target".into()),
                    }
                }
                b.emit(Op::CallBuiltin(ops::UNSET_PATH, (segs.len() + 1) as u8), 0);
                b.emit(Op::Pop, 0);
            }
            _ => return Err("unset() target must be a variable or an array element".into()),
        }
        Ok(())
    }

    /// `match (subj) { A, B => R, default => D }` — a value-producing expression.
    /// The subject is compared (`===`) against each arm's conditions; the first
    /// match's body value is left on the stack.
    fn compile_match(
        &mut self,
        b: &mut ChunkBuilder,
        subj: &Expr,
        arms: &[MatchArm],
    ) -> Result<(), String> {
        let m_t = self.tmp_name("m");
        self.emit_set_var(b, &m_t, |c, b| c.compile_expr(b, subj))?;

        // Dispatch: strict compare against every condition of every non-default
        // arm; the `default` arm is the fallback regardless of its position.
        let mut dispatch: Vec<(usize, usize)> = Vec::new(); // (arm index, JumpIfTrue pos)
        let mut default_index: Option<usize> = None;
        for (i, arm) in arms.iter().enumerate() {
            match &arm.conds {
                Some(conds) => {
                    for cond in conds {
                        self.emit_get_var(b, &m_t);
                        self.compile_expr(b, cond)?;
                        b.emit(Op::CallBuiltin(ops::STRICT_EQ, 2), 0);
                        b.emit(Op::CallBuiltin(ops::TRUTHY, 1), 0);
                        let jt = b.emit(Op::JumpIfTrue(0), 0);
                        dispatch.push((i, jt));
                    }
                }
                None => default_index = Some(i),
            }
        }
        // No arm matched: jump to the default body, or throw \UnhandledMatchError
        // with the unhandled subject in the message (PHP 8 semantics). The throw
        // halts this chunk, so there is no fall-through jump to the end.
        let default_jump = if default_index.is_some() {
            Some(b.emit(Op::Jump(0), 0))
        } else {
            let cls = b.add_constant(Value::str("UnhandledMatchError".to_string()));
            b.emit(Op::LoadConst(cls), 0);
            let pfx = b.add_constant(Value::str("Unhandled match case ".to_string()));
            b.emit(Op::LoadConst(pfx), 0);
            self.emit_get_var(b, &m_t);
            b.emit(Op::CallBuiltin(ops::CONCAT, 2), 0); // message string
            b.emit(Op::CallBuiltin(ops::NEW, 2), 0); // the exception object
            b.emit(Op::CallBuiltin(ops::THROW, 1), 0); // record + unwind
            None
        };

        // Arm bodies: each leaves exactly one value, then jumps to the end.
        let mut body_starts = Vec::with_capacity(arms.len());
        let mut body_ends = Vec::with_capacity(arms.len());
        for arm in arms {
            body_starts.push(b.current_pos());
            self.compile_expr(b, &arm.body)?;
            body_ends.push(b.emit(Op::Jump(0), 0));
        }
        let end = b.current_pos();

        for (i, jt) in dispatch {
            b.patch_jump(jt, body_starts[i]);
        }
        if let Some(di) = default_index {
            b.patch_jump(default_jump.unwrap(), body_starts[di]);
        }
        for j in body_ends {
            b.patch_jump(j, end);
        }
        Ok(())
    }

    /// A double-quoted string: concatenate its parts, always yielding a string.
    fn compile_interp(&mut self, b: &mut ChunkBuilder, parts: &[StrPart]) {
        let empty = b.add_constant(Value::str(String::new()));
        b.emit(Op::LoadConst(empty), 0);
        for part in parts {
            match part {
                StrPart::Lit(s) => {
                    let idx = b.add_constant(Value::str(s.clone()));
                    b.emit(Op::LoadConst(idx), 0);
                }
                StrPart::Var(name) => self.emit_get_var(b, name),
            }
            b.emit(Op::CallBuiltin(ops::CONCAT, 2), 0);
        }
    }

    fn compile_binary(
        &mut self,
        b: &mut ChunkBuilder,
        op: BinOp,
        l: &Expr,
        r: &Expr,
    ) -> Result<(), String> {
        // Short-circuit logical operators evaluate the right side conditionally.
        if matches!(op, BinOp::And | BinOp::Or) {
            self.compile_truthy(b, l)?;
            let short = if op == BinOp::And {
                b.emit(Op::JumpIfFalse(0), 0)
            } else {
                b.emit(Op::JumpIfTrue(0), 0)
            };
            self.compile_truthy(b, r)?;
            let jend = b.emit(Op::Jump(0), 0);
            let shortcut = b.current_pos();
            b.patch_jump(short, shortcut);
            b.emit(
                if op == BinOp::And {
                    Op::LoadFalse
                } else {
                    Op::LoadTrue
                },
                0,
            );
            let end = b.current_pos();
            b.patch_jump(jend, end);
            return Ok(());
        }

        self.compile_expr(b, l)?;
        self.compile_expr(b, r)?;
        match op {
            BinOp::Add => {
                b.emit(Op::Add, 0);
            }
            BinOp::Sub => {
                b.emit(Op::Sub, 0);
            }
            BinOp::Mul => {
                b.emit(Op::Mul, 0);
            }
            BinOp::Div => {
                b.emit(Op::CallBuiltin(ops::DIV, 2), 0);
            }
            BinOp::Mod => {
                b.emit(Op::CallBuiltin(ops::MOD, 2), 0);
            }
            BinOp::Pow => {
                b.emit(Op::CallBuiltin(ops::POW, 2), 0);
            }
            BinOp::Concat => {
                b.emit(Op::CallBuiltin(ops::CONCAT, 2), 0);
            }
            BinOp::LooseEq => {
                b.emit(Op::CallBuiltin(ops::LOOSE_EQ, 2), 0);
            }
            BinOp::LooseNe => {
                b.emit(Op::CallBuiltin(ops::LOOSE_NE, 2), 0);
            }
            BinOp::StrictEq => {
                b.emit(Op::CallBuiltin(ops::STRICT_EQ, 2), 0);
            }
            BinOp::StrictNe => {
                b.emit(Op::CallBuiltin(ops::STRICT_NE, 2), 0);
            }
            BinOp::Lt => {
                b.emit(Op::CallBuiltin(ops::LT, 2), 0);
            }
            BinOp::Gt => {
                b.emit(Op::CallBuiltin(ops::GT, 2), 0);
            }
            BinOp::Le => {
                b.emit(Op::CallBuiltin(ops::LE, 2), 0);
            }
            BinOp::Ge => {
                b.emit(Op::CallBuiltin(ops::GE, 2), 0);
            }
            BinOp::Spaceship => {
                b.emit(Op::CallBuiltin(ops::SPACESHIP, 2), 0);
            }
            BinOp::BitAnd => {
                b.emit(Op::CallBuiltin(ops::BITAND, 2), 0);
            }
            BinOp::BitOr => {
                b.emit(Op::CallBuiltin(ops::BITOR, 2), 0);
            }
            BinOp::BitXor => {
                b.emit(Op::CallBuiltin(ops::BITXOR, 2), 0);
            }
            BinOp::Shl => {
                b.emit(Op::CallBuiltin(ops::SHL, 2), 0);
            }
            BinOp::Shr => {
                b.emit(Op::CallBuiltin(ops::SHR, 2), 0);
            }
            BinOp::And | BinOp::Or => unreachable!("handled above"),
        }
        Ok(())
    }

    fn compile_assign(
        &mut self,
        b: &mut ChunkBuilder,
        lhs: &Expr,
        op: Option<BinOp>,
        rhs: &Expr,
    ) -> Result<(), String> {
        match lhs {
            Expr::Var(name) => {
                let nidx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(nidx), 0);
                match op {
                    None => self.compile_expr(b, rhs)?,
                    Some(cop) => {
                        // $x <op>= rhs  ⇒  $x = $x <op> rhs
                        self.emit_get_var(b, name);
                        self.compile_expr(b, rhs)?;
                        self.emit_binop(b, cop);
                    }
                }
                b.emit(Op::CallBuiltin(ops::SETVAR, 2), 0);
            }
            Expr::PropGet(recv, name) => {
                // `$o->p = rhs` and its compound form `$o->p op= rhs`. For a
                // compound op the receiver is evaluated ONCE into a temporary
                // (shared by the read and the write).
                match op {
                    None => {
                        self.compile_expr(b, recv)?;
                        let nidx = b.add_constant(Value::str(name.clone()));
                        b.emit(Op::LoadConst(nidx), 0);
                        self.compile_expr(b, rhs)?;
                        b.emit(Op::CallBuiltin(ops::PROP_SET, 3), 0);
                    }
                    Some(cop) => {
                        let r = self.tmp_name("pr");
                        self.emit_set_var(b, &r, |c, b| c.compile_expr(b, recv))?;
                        self.emit_get_var(b, &r);
                        let nidx = b.add_constant(Value::str(name.clone()));
                        b.emit(Op::LoadConst(nidx), 0);
                        // value = @r->name op rhs
                        self.emit_get_var(b, &r);
                        let gidx = b.add_constant(Value::str(name.clone()));
                        b.emit(Op::LoadConst(gidx), 0);
                        b.emit(Op::CallBuiltin(ops::PROP_GET, 2), 0);
                        self.compile_expr(b, rhs)?;
                        self.emit_binop(b, cop);
                        b.emit(Op::CallBuiltin(ops::PROP_SET, 3), 0);
                    }
                }
            }
            Expr::Index(..) | Expr::Append(..) => {
                let (root, segs) = Self::flatten_segments(lhs)?;
                match root {
                    Expr::Var(name) => self.compile_lvalue_assign(b, name, &segs, op, rhs)?,
                    Expr::PropGet(recv, prop) => {
                        // Index/append into an array-valued property: vivify the
                        // property into an array, hold its handle in a temp, and
                        // write through it (arrays are reference handles, so the
                        // mutation lands on the object).
                        let t = self.tmp_name("po");
                        self.emit_set_var(b, &t, |c, b| {
                            c.compile_expr(b, recv)?;
                            let idx = b.add_constant(Value::str(prop.clone()));
                            b.emit(Op::LoadConst(idx), 0);
                            b.emit(Op::CallBuiltin(ops::PROP_ENSURE_ARRAY, 2), 0);
                            Ok(())
                        })?;
                        self.compile_lvalue_assign(b, &t, &segs, op, rhs)?;
                    }
                    _ => return Err("unsupported assignment target".into()),
                }
            }
            _ => return Err("invalid assignment target".into()),
        }
        Ok(())
    }

    /// Flatten a nested array lvalue (`$a[k1][k2]...`, with any `[]` appends) into
    /// its root expression (a `$var` or an `$o->prop`) and the ordered chain of
    /// segments (source order).
    fn flatten_segments(lhs: &Expr) -> Result<(&Expr, Vec<LvSeg<'_>>), String> {
        let mut cur = lhs;
        let mut segs: Vec<LvSeg> = Vec::new();
        loop {
            match cur {
                Expr::Index(recv, idx) => {
                    segs.push(LvSeg::Key(idx));
                    cur = recv;
                }
                Expr::Append(inner) => {
                    segs.push(LvSeg::Append);
                    cur = inner;
                }
                other => {
                    segs.reverse();
                    return Ok((other, segs));
                }
            }
        }
    }

    /// Assign along a flattened lvalue chain. A `[]` that is not the outermost
    /// segment (`$a[][k] = v`) is materialized by appending a fresh child array and
    /// re-rooting the remaining segments on it through a temporary, so each `[]`
    /// appends exactly one element as PHP does. Otherwise the flat name+keys(+
    /// trailing append) fast paths are used.
    fn compile_lvalue_assign(
        &mut self,
        b: &mut ChunkBuilder,
        name: &str,
        segs: &[LvSeg],
        op: Option<BinOp>,
        rhs: &Expr,
    ) -> Result<(), String> {
        // The first append that still has segments after it is a mid-path append.
        let mid = segs
            .iter()
            .position(|s| matches!(s, LvSeg::Append))
            .filter(|&i| i + 1 < segs.len());
        if let Some(i) = mid {
            // Everything before the first append is a plain key prefix.
            let prefix: Vec<&Expr> = segs[..i]
                .iter()
                .map(|s| match s {
                    LvSeg::Key(k) => *k,
                    LvSeg::Append => unreachable!("first mid-append has no earlier append"),
                })
                .collect();
            let t = self.tmp_name("ap");
            // @t = a freshly appended child array of $name[prefix...].
            self.emit_set_var(b, &t, |c, b| {
                let nidx = b.add_constant(Value::str(name.to_string()));
                b.emit(Op::LoadConst(nidx), 0);
                for k in &prefix {
                    c.compile_expr(b, k)?;
                }
                b.emit(
                    Op::CallBuiltin(ops::PATH_APPEND_CHILD, (prefix.len() + 1) as u8),
                    0,
                );
                Ok(())
            })?;
            // Keep writing through $@t along the remaining segments.
            return self.compile_lvalue_assign(b, &t, &segs[i + 1..], op, rhs);
        }

        // No mid-append: keys with an optional trailing `[]` append.
        let append = matches!(segs.last(), Some(LvSeg::Append));
        let key_segs = if append {
            &segs[..segs.len() - 1]
        } else {
            segs
        };
        let keys: Vec<&Expr> = key_segs
            .iter()
            .map(|s| match s {
                LvSeg::Key(k) => *k,
                LvSeg::Append => unreachable!("only the final segment may be an append here"),
            })
            .collect();

        if append {
            if op.is_some() {
                return Err("`[]` append takes only plain `=`".into());
            }
            let nidx = b.add_constant(Value::str(name.to_string()));
            b.emit(Op::LoadConst(nidx), 0);
            if keys.is_empty() {
                // Single-level `$a[] = rhs` keeps the compact ARR_APPEND lowering.
                self.compile_expr(b, rhs)?;
                b.emit(Op::CallBuiltin(ops::ARR_APPEND, 2), 0);
            } else {
                for k in &keys {
                    self.compile_expr(b, k)?;
                }
                self.compile_expr(b, rhs)?;
                b.emit(Op::CallBuiltin(ops::APPEND_PATH, (keys.len() + 2) as u8), 0);
            }
        } else if keys.len() == 1 && op.is_none() {
            // Fast path for the common single-level `$a[k] = rhs`.
            let nidx = b.add_constant(Value::str(name.to_string()));
            b.emit(Op::LoadConst(nidx), 0);
            self.compile_expr(b, keys[0])?;
            self.compile_expr(b, rhs)?;
            b.emit(Op::CallBuiltin(ops::INDEX_SET, 3), 0);
        } else {
            self.compile_index_assign(b, name, &keys, op, rhs)?;
        }
        Ok(())
    }

    /// `$a[k1]..[kN] = rhs` (deep set) and its compound form `$a[k1]..[kN] op=
    /// rhs`. For a compound op the key expressions are hoisted into temporaries so
    /// they are evaluated exactly once across the read and the write.
    fn compile_index_assign(
        &mut self,
        b: &mut ChunkBuilder,
        name: &str,
        keys: &[&Expr],
        op: Option<BinOp>,
        rhs: &Expr,
    ) -> Result<(), String> {
        let nidx = b.add_constant(Value::str(name.to_string()));
        match op {
            None => {
                b.emit(Op::LoadConst(nidx), 0);
                for k in keys {
                    self.compile_expr(b, k)?;
                }
                self.compile_expr(b, rhs)?;
                b.emit(Op::CallBuiltin(ops::SET_PATH, (keys.len() + 2) as u8), 0);
            }
            Some(cop) => {
                // Evaluate each key once into a temporary, then `set = get op rhs`.
                let key_tmps: Vec<String> = keys.iter().map(|_| self.tmp_name("lk")).collect();
                for (t, k) in key_tmps.iter().zip(keys) {
                    self.emit_set_var(b, t, |c, b| c.compile_expr(b, k))?;
                }
                b.emit(Op::LoadConst(nidx), 0);
                for t in &key_tmps {
                    self.emit_get_var(b, t);
                }
                // value = $a[keys] op rhs
                b.emit(Op::LoadConst(nidx), 0);
                for t in &key_tmps {
                    self.emit_get_var(b, t);
                }
                b.emit(
                    Op::CallBuiltin(ops::GET_PATH, (key_tmps.len() + 1) as u8),
                    0,
                );
                self.compile_expr(b, rhs)?;
                self.emit_binop(b, cop);
                b.emit(
                    Op::CallBuiltin(ops::SET_PATH, (key_tmps.len() + 2) as u8),
                    0,
                );
            }
        }
        Ok(())
    }

    /// Lower an anonymous function / arrow function to a closure-creating
    /// sequence: compile the body into its own chunk (registered under a synthetic
    /// name in the function table, with its parameters so defaults/variadics bind),
    /// then emit `MKCLOSURE` with the captured `(name, value)` pairs read from the
    /// current scope.
    fn compile_closure(
        &mut self,
        b: &mut ChunkBuilder,
        params: &[Param],
        captures: &[String],
        body: &[Stmt],
    ) -> Result<(), String> {
        let cparams = self.compile_params(params)?;
        let mut fb = ChunkBuilder::new();
        // Like a named function, the body gets its own loop scope so a `break`
        // inside it cannot target a loop at the creation site.
        let saved = std::mem::take(&mut self.loops);
        self.compile_seq(&mut fb, body)?;
        self.loops = saved;
        let def_name = self.tmp_name("closure");
        self.functions.push((
            def_name.clone(),
            FuncDef {
                params: cparams,
                chunk: fb.build(),
            },
        ));

        let nidx = b.add_constant(Value::str(def_name));
        b.emit(Op::LoadConst(nidx), 0);
        for cap in captures {
            let cidx = b.add_constant(Value::str(cap.clone()));
            b.emit(Op::LoadConst(cidx), 0);
            self.emit_get_var(b, cap);
        }
        b.emit(
            Op::CallBuiltin(ops::MKCLOSURE, (1 + captures.len() * 2) as u8),
            0,
        );
        Ok(())
    }

    fn compile_incdec(
        &mut self,
        b: &mut ChunkBuilder,
        target: &Expr,
        inc: bool,
        prefix: bool,
    ) -> Result<(), String> {
        // code: bit0 = increment, bit1 = prefix.
        let code = (inc as i64) | ((prefix as i64) << 1);
        match target {
            Expr::Var(name) => {
                let nidx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(nidx), 0);
                b.emit(Op::LoadInt(code), 0);
                b.emit(Op::CallBuiltin(ops::INCDEC, 2), 0);
            }
            Expr::PropGet(recv, name) => {
                // `$o->p++` — read-modify-write a scalar property.
                self.compile_expr(b, recv)?;
                let nidx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(nidx), 0);
                b.emit(Op::LoadInt(code), 0);
                b.emit(Op::CallBuiltin(ops::PROP_INCDEC, 3), 0);
            }
            Expr::Index(..) => {
                // `++$a[k1]..[kN]` — read-modify-write the deepest element. Roots
                // at a `$var` or an array-valued `$o->prop` (vivified into a temp).
                let (root, segs) = Self::flatten_segments(target)?;
                let mut keys: Vec<&Expr> = Vec::with_capacity(segs.len());
                for s in &segs {
                    match s {
                        LvSeg::Key(k) => keys.push(k),
                        LvSeg::Append => return Err("cannot ++/-- an `[]` append target".into()),
                    }
                }
                let name: String = match root {
                    Expr::Var(name) => name.clone(),
                    Expr::PropGet(recv, prop) => {
                        let t = self.tmp_name("po");
                        self.emit_set_var(b, &t, |c, b| {
                            c.compile_expr(b, recv)?;
                            let idx = b.add_constant(Value::str(prop.clone()));
                            b.emit(Op::LoadConst(idx), 0);
                            b.emit(Op::CallBuiltin(ops::PROP_ENSURE_ARRAY, 2), 0);
                            Ok(())
                        })?;
                        t
                    }
                    _ => return Err("unsupported ++/-- target".into()),
                };
                let nidx = b.add_constant(Value::str(name));
                b.emit(Op::LoadConst(nidx), 0);
                for k in &keys {
                    self.compile_expr(b, k)?;
                }
                b.emit(Op::LoadInt(code), 0);
                b.emit(Op::CallBuiltin(ops::INCDEC_PATH, (keys.len() + 2) as u8), 0);
            }
            _ => {
                return Err(
                    "scaffold supports ++/-- only on variables, array elements, and properties"
                        .into(),
                )
            }
        }
        Ok(())
    }

    /// Emit the native op for an arithmetic operator, or a builtin call for the
    /// PHP-semantic ones. Used by compound assignment.
    fn emit_binop(&mut self, b: &mut ChunkBuilder, op: BinOp) {
        match op {
            BinOp::Add => {
                b.emit(Op::Add, 0);
            }
            BinOp::Sub => {
                b.emit(Op::Sub, 0);
            }
            BinOp::Mul => {
                b.emit(Op::Mul, 0);
            }
            BinOp::Div => {
                b.emit(Op::CallBuiltin(ops::DIV, 2), 0);
            }
            BinOp::Mod => {
                b.emit(Op::CallBuiltin(ops::MOD, 2), 0);
            }
            BinOp::Pow => {
                b.emit(Op::CallBuiltin(ops::POW, 2), 0);
            }
            BinOp::Concat => {
                b.emit(Op::CallBuiltin(ops::CONCAT, 2), 0);
            }
            BinOp::BitAnd => {
                b.emit(Op::CallBuiltin(ops::BITAND, 2), 0);
            }
            BinOp::BitOr => {
                b.emit(Op::CallBuiltin(ops::BITOR, 2), 0);
            }
            BinOp::BitXor => {
                b.emit(Op::CallBuiltin(ops::BITXOR, 2), 0);
            }
            BinOp::Shl => {
                b.emit(Op::CallBuiltin(ops::SHL, 2), 0);
            }
            BinOp::Shr => {
                b.emit(Op::CallBuiltin(ops::SHR, 2), 0);
            }
            _ => unreachable!("compound assignment only uses arithmetic/bitwise/concat ops"),
        }
    }

    fn compile_truthy(&mut self, b: &mut ChunkBuilder, e: &Expr) -> Result<(), String> {
        self.compile_expr(b, e)?;
        b.emit(Op::CallBuiltin(ops::TRUTHY, 1), 0);
        Ok(())
    }

    fn emit_get_var(&mut self, b: &mut ChunkBuilder, name: &str) {
        let idx = b.add_constant(Value::str(name.to_string()));
        b.emit(Op::LoadConst(idx), 0);
        b.emit(Op::CallBuiltin(ops::GETVAR, 1), 0);
    }

    /// Emit `$name = <value produced by `f`>`, leaving the value on the stack.
    fn emit_set_var(
        &mut self,
        b: &mut ChunkBuilder,
        name: &str,
        f: impl FnOnce(&mut Self, &mut ChunkBuilder) -> Result<(), String>,
    ) -> Result<(), String> {
        let idx = b.add_constant(Value::str(name.to_string()));
        b.emit(Op::LoadConst(idx), 0);
        f(self, b)?;
        b.emit(Op::CallBuiltin(ops::SETVAR, 2), 0);
        // The desugared statements want no residual on the stack.
        b.emit(Op::Pop, 0);
        Ok(())
    }
}

/// Collect the variable names referenced anywhere in `e`, de-duplicated in
/// first-seen order — the free-variable set an arrow function captures by value.
/// Over-capturing (e.g. a name that is only ever assigned) is harmless because
/// capture is by value; a nested arrow fn contributes its body's free variables
/// minus its own parameters, and a nested `use(...)` closure contributes exactly
/// the names it captures.
fn collect_free_vars(e: &Expr, out: &mut Vec<String>) {
    fn push(name: &str, out: &mut Vec<String>) {
        if !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
    }
    match e {
        Expr::Var(n) => push(n, out),
        Expr::Interp(parts) => {
            for p in parts {
                if let StrPart::Var(n) = p {
                    push(n, out);
                }
            }
        }
        Expr::Array(elems) => {
            for (k, v) in elems {
                if let Some(k) = k {
                    collect_free_vars(k, out);
                }
                collect_free_vars(v, out);
            }
        }
        Expr::Index(a, b) | Expr::Binary(_, a, b) | Expr::Elvis(a, b) | Expr::Coalesce(a, b) => {
            collect_free_vars(a, out);
            collect_free_vars(b, out);
        }
        Expr::Append(a) | Expr::Unary(_, a) | Expr::Spread(a) => collect_free_vars(a, out),
        Expr::Assign(a, _, b) => {
            collect_free_vars(a, out);
            collect_free_vars(b, out);
        }
        Expr::IncDec { target, .. } => collect_free_vars(target, out),
        Expr::Call(_, args) => {
            for a in args {
                collect_free_vars(a, out);
            }
        }
        Expr::CallValue(callee, args) => {
            collect_free_vars(callee, out);
            for a in args {
                collect_free_vars(a, out);
            }
        }
        Expr::Ternary(a, b, c) => {
            collect_free_vars(a, out);
            collect_free_vars(b, out);
            collect_free_vars(c, out);
        }
        Expr::Match { subj, arms } => {
            collect_free_vars(subj, out);
            for arm in arms {
                if let Some(conds) = &arm.conds {
                    for c in conds {
                        collect_free_vars(c, out);
                    }
                }
                collect_free_vars(&arm.body, out);
            }
        }
        // A nested arrow fn captures its own body's free variables minus its
        // parameters; those free names must in turn be captured by the enclosing
        // arrow fn so the binding is available when the inner one runs.
        Expr::ArrowFn { params, body } => {
            let mut inner = Vec::new();
            collect_free_vars(body, &mut inner);
            for n in inner {
                if !params.iter().any(|p| p.name == n) {
                    push(&n, out);
                }
            }
        }
        // A nested `use(...)` closure names the enclosing variables it captures.
        Expr::Closure { uses, .. } => {
            for u in uses {
                push(u, out);
            }
        }
        Expr::New(_, args) | Expr::StaticCall(_, _, args) => {
            for a in args {
                collect_free_vars(a, out);
            }
        }
        Expr::PropGet(recv, _) => collect_free_vars(recv, out),
        Expr::MethodCall(recv, _, args) => {
            collect_free_vars(recv, out);
            for a in args {
                collect_free_vars(a, out);
            }
        }
        Expr::StaticGet(_, _) => {}
        Expr::Throw(inner) => collect_free_vars(inner, out),
        Expr::ConstFetch(_) => {}
        Expr::Unset(targets) => {
            for t in targets {
                collect_free_vars(t, out);
            }
        }
        Expr::InstanceOf(e, _) => collect_free_vars(e, out),
        Expr::RefAssign(a, b) => {
            collect_free_vars(a, out);
            collect_free_vars(b, out);
        }
        Expr::Null | Expr::Bool(_) | Expr::Int(_) | Expr::Float(_) | Expr::Str(_) => {}
    }
}
