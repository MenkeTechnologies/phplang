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
use crate::host::{ops, FuncDef};
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

#[derive(Default)]
pub struct Compiler {
    functions: Vec<(String, FuncDef)>,
    loops: Vec<LoopCtx>,
    /// Monotonic counter for compiler-generated temporary variable names
    /// (`foreach` desugaring), kept out of the PHP identifier space with a `@`.
    tmp: usize,
}

/// Compile a parsed program.
pub fn compile(stmts: &[Stmt]) -> Result<Program, String> {
    let mut c = Compiler::default();
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

    fn compile_seq(&mut self, b: &mut ChunkBuilder, body: &[Stmt]) -> Result<(), String> {
        for s in body {
            self.compile_stmt(b, s)?;
        }
        Ok(())
    }

    fn compile_stmt(&mut self, b: &mut ChunkBuilder, s: &Stmt) -> Result<(), String> {
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
                let mut fb = ChunkBuilder::new();
                // A function body has its own loop scope: a break inside it must
                // not target a loop at the call site.
                let saved = std::mem::take(&mut self.loops);
                self.compile_seq(&mut fb, body)?;
                self.loops = saved;
                self.functions.push((
                    name.to_ascii_lowercase(),
                    FuncDef {
                        params: params.clone(),
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
        let arms: Vec<(&Expr, &[Stmt])> = std::iter::once((cond, then.as_ref()))
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
                let idx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(idx), 0);
                for a in args {
                    self.compile_expr(b, a)?;
                }
                b.emit(Op::CallBuiltin(ops::CALL, (args.len() + 1) as u8), 0);
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
            b.emit(if op == BinOp::And { Op::LoadFalse } else { Op::LoadTrue }, 0);
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
            Expr::Index(recv, idx) => {
                let Expr::Var(name) = recv.as_ref() else {
                    return Err("scaffold supports only `$var[...] = ...` index assignment".into());
                };
                if op.is_some() {
                    return Err("scaffold supports only plain `=` on array elements".into());
                }
                let nidx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(nidx), 0);
                self.compile_expr(b, idx)?;
                self.compile_expr(b, rhs)?;
                b.emit(Op::CallBuiltin(ops::INDEX_SET, 3), 0);
            }
            Expr::Append(recv) => {
                let Expr::Var(name) = recv.as_ref() else {
                    return Err("scaffold supports only `$var[] = ...` append".into());
                };
                if op.is_some() {
                    return Err("`[]` append takes only plain `=`".into());
                }
                let nidx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(nidx), 0);
                self.compile_expr(b, rhs)?;
                b.emit(Op::CallBuiltin(ops::ARR_APPEND, 2), 0);
            }
            _ => return Err("invalid assignment target".into()),
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
        let Expr::Var(name) = target else {
            return Err("scaffold supports ++/-- only on plain $variables".into());
        };
        let nidx = b.add_constant(Value::str(name.clone()));
        b.emit(Op::LoadConst(nidx), 0);
        // code: bit0 = increment, bit1 = prefix.
        let code = (inc as i64) | ((prefix as i64) << 1);
        b.emit(Op::LoadInt(code), 0);
        b.emit(Op::CallBuiltin(ops::INCDEC, 2), 0);
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
            _ => unreachable!("compound assignment only uses arithmetic/concat ops"),
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
