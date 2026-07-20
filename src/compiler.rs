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
use crate::host::{self, ops, FuncDef};
use fusevm::{Chunk, ChunkBuilder, Op, Value};

/// The full output of compiling a program.
pub struct Program {
    pub main: Chunk,
    pub functions: Vec<(String, FuncDef)>,
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
    loops: Vec<LoopCtx>,
    /// Monotonic counter for compiler-generated temporary variable names
    /// (`foreach` desugaring), kept out of the PHP identifier space with a `@`.
    tmp: usize,
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
    let mut b = ChunkBuilder::new();
    c.compile_seq(&mut b, stmts)?;
    Ok(Program {
        main: b.build(),
        functions: c.functions,
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
            });
        }
        Ok(out)
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
                let j = b.emit(Op::Jump(0), line);
                self.loops
                    .last_mut()
                    .ok_or("'break' outside of a loop")?
                    .breaks
                    .push(j);
            }
            StmtKind::Continue => {
                let j = b.emit(Op::Jump(0), line);
                self.loops
                    .last_mut()
                    .ok_or("'continue' outside of a loop")?
                    .continues
                    .push(j);
            }
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
                body,
            } => self.compile_foreach(b, arr, key_var.as_deref(), val_var, body)?,
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
        body: &[Stmt],
    ) -> Result<(), String> {
        let arr_t = self.tmp_name("arr");
        let keys_t = self.tmp_name("keys");
        let i_t = self.tmp_name("i");

        // @arr = <arr>;  @keys = array_keys(@arr);  @i = 0;
        self.emit_set_var(b, &arr_t, |c, b| c.compile_expr(b, arr))?;
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

        // @i = @i + 1;
        let step_pos = b.current_pos();
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
            b.patch_jump(j, step_pos);
        }
        Ok(())
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
                    b.emit(Op::CallBuiltin(ops::CALL_SPREAD, (args.len() * 2 + 1) as u8), 0);
                } else {
                    let idx = b.add_constant(Value::str(name.clone()));
                    b.emit(Op::LoadConst(idx), 0);
                    for a in args {
                        self.compile_expr(b, a)?;
                    }
                    b.emit(Op::CallBuiltin(ops::CALL, (args.len() + 1) as u8), 0);
                }
            }
            Expr::Spread(_) => {
                return Err("'...' argument unpacking is only valid in a function call".into())
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
        // No arm matched: jump to the default body, or — SCAFFOLD DEVIATION —
        // yield null. Real PHP throws \UnhandledMatchError here; adding that
        // needs a host op this wave does not introduce, so null is used instead.
        let (default_jump, no_match_jump) = if default_index.is_some() {
            (Some(b.emit(Op::Jump(0), 0)), None)
        } else {
            b.emit(Op::LoadUndef, 0);
            (None, Some(b.emit(Op::Jump(0), 0)))
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
        if let Some(nm) = no_match_jump {
            b.patch_jump(nm, end);
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
            Expr::Index(..) | Expr::Append(..) => {
                let (name, segs) = Self::flatten_segments(lhs)?;
                self.compile_lvalue_assign(b, name, &segs, op, rhs)?;
            }
            _ => return Err("invalid assignment target".into()),
        }
        Ok(())
    }

    /// Flatten a nested array lvalue (`$a[k1][k2]...`, with any `[]` appends) into
    /// its root variable name and the ordered chain of segments (source order).
    fn flatten_segments(lhs: &Expr) -> Result<(&str, Vec<LvSeg<'_>>), String> {
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
                Expr::Var(name) => {
                    segs.reverse();
                    return Ok((name, segs));
                }
                _ => return Err("invalid assignment target".into()),
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
                b.emit(Op::CallBuiltin(ops::PATH_APPEND_CHILD, (prefix.len() + 1) as u8), 0);
                Ok(())
            })?;
            // Keep writing through $@t along the remaining segments.
            return self.compile_lvalue_assign(b, &t, &segs[i + 1..], op, rhs);
        }

        // No mid-append: keys with an optional trailing `[]` append.
        let append = matches!(segs.last(), Some(LvSeg::Append));
        let key_segs = if append { &segs[..segs.len() - 1] } else { segs };
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
                b.emit(Op::CallBuiltin(ops::GET_PATH, (key_tmps.len() + 1) as u8), 0);
                self.compile_expr(b, rhs)?;
                self.emit_binop(b, cop);
                b.emit(Op::CallBuiltin(ops::SET_PATH, (key_tmps.len() + 2) as u8), 0);
            }
        }
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
            Expr::Index(..) => {
                // `++$a[k1]..[kN]` — read-modify-write the deepest element.
                let (name, segs) = Self::flatten_segments(target)?;
                let mut keys: Vec<&Expr> = Vec::with_capacity(segs.len());
                for s in &segs {
                    match s {
                        LvSeg::Key(k) => keys.push(k),
                        LvSeg::Append => {
                            return Err("cannot ++/-- an `[]` append target".into())
                        }
                    }
                }
                let nidx = b.add_constant(Value::str(name.to_string()));
                b.emit(Op::LoadConst(nidx), 0);
                for k in &keys {
                    self.compile_expr(b, k)?;
                }
                b.emit(Op::LoadInt(code), 0);
                b.emit(Op::CallBuiltin(ops::INCDEC_PATH, (keys.len() + 2) as u8), 0);
            }
            _ => return Err("scaffold supports ++/-- only on variables and array elements".into()),
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
