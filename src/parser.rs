//! Recursive-descent parser: `lexer` tokens → PHP AST (`ast::Stmt`).

use crate::ast::*;
use crate::lexer::{self, Spanned, Tok};

/// Map a `(type)` cast keyword to the conversion function it desugars to.
/// `(array)`/`(object)`/`(unset)` are not supported in the scaffold → `None`.
fn cast_fn(t: &str) -> Option<&'static str> {
    match t.to_ascii_lowercase().as_str() {
        "int" | "integer" => Some("intval"),
        "float" | "double" | "real" => Some("floatval"),
        "string" => Some("strval"),
        "bool" | "boolean" => Some("boolval"),
        _ => None,
    }
}

/// Parse a PHP source string into a statement list.
pub fn parse(src: &str) -> Result<Vec<Stmt>, String> {
    let toks = lexer::lex(src)?;
    let mut p = Parser { toks, pos: 0 };
    let mut stmts = Vec::new();
    while !p.at_end() {
        stmts.push(p.statement()?);
    }
    Ok(stmts)
}

struct Parser {
    toks: Vec<Spanned>,
    pos: usize,
}

impl Parser {
    // ── cursor ─────────────────────────────────────────────────────────────

    fn at_end(&self) -> bool {
        self.pos >= self.toks.len()
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|s| &s.tok)
    }

    fn line(&self) -> u32 {
        self.toks.get(self.pos).map(|s| s.line).unwrap_or(0)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).map(|s| s.tok.clone());
        self.pos += 1;
        t
    }

    fn at_punct(&self, p: &str) -> bool {
        matches!(self.peek(), Some(Tok::Punct(x)) if *x == p)
    }

    /// Whether the token *after* the cursor is the punctuation `p` (used to spot
    /// `??`, which the lexer emits as two `?` tokens).
    fn peek2_is_punct(&self, p: &str) -> bool {
        matches!(self.toks.get(self.pos + 1).map(|s| &s.tok), Some(Tok::Punct(x)) if *x == p)
    }

    fn eat_punct(&mut self, p: &str) -> bool {
        if self.at_punct(p) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_punct(&mut self, p: &str) -> Result<(), String> {
        if self.eat_punct(p) {
            Ok(())
        } else {
            Err(format!(
                "expected '{p}' but found {:?} (line {})",
                self.peek(),
                self.line()
            ))
        }
    }

    /// True if the next token is the keyword `kw` (case-insensitive, as PHP).
    fn at_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(s)) if s.eq_ignore_ascii_case(kw))
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.at_kw(kw) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    // ── statements ─────────────────────────────────────────────────────────

    fn statement(&mut self) -> Result<Stmt, String> {
        let line = self.line();
        let kind = match self.peek() {
            Some(Tok::InlineHtml(_)) => {
                let Some(Tok::InlineHtml(s)) = self.next() else {
                    unreachable!()
                };
                StmtKind::InlineHtml(s)
            }
            Some(Tok::OpenEcho) => {
                self.pos += 1;
                let e = self.expression()?;
                self.eat_punct(";");
                StmtKind::Echo(vec![e])
            }
            Some(Tok::Punct(";")) => {
                self.pos += 1;
                StmtKind::Block(vec![])
            }
            Some(Tok::Punct("{")) => StmtKind::Block(self.block()?),
            _ if self.at_kw("echo") => {
                self.pos += 1;
                let mut args = vec![self.expression()?];
                while self.eat_punct(",") {
                    args.push(self.expression()?);
                }
                self.expect_punct(";")?;
                StmtKind::Echo(args)
            }
            _ if self.at_kw("print") => {
                self.pos += 1;
                let e = self.expression()?;
                self.expect_punct(";")?;
                StmtKind::Echo(vec![e])
            }
            _ if self.at_kw("if") => self.if_stmt()?,
            _ if self.at_kw("while") => self.while_stmt()?,
            _ if self.at_kw("do") => self.do_while_stmt()?,
            _ if self.at_kw("switch") => self.switch_stmt()?,
            _ if self.at_kw("for") => self.for_stmt()?,
            _ if self.at_kw("foreach") => self.foreach_stmt()?,
            _ if self.at_kw("function") => self.function_stmt()?,
            _ if self.at_kw("return") => {
                self.pos += 1;
                let e = if self.at_punct(";") {
                    None
                } else {
                    Some(self.expression()?)
                };
                self.expect_punct(";")?;
                StmtKind::Return(e)
            }
            _ if self.at_kw("break") => {
                self.pos += 1;
                // An optional numeric level (`break 2;`) — accepted, level ignored
                // in the scaffold (only one loop depth is unwound).
                if let Some(Tok::Int(_)) = self.peek() {
                    self.pos += 1;
                }
                self.expect_punct(";")?;
                StmtKind::Break
            }
            _ if self.at_kw("continue") => {
                self.pos += 1;
                if let Some(Tok::Int(_)) = self.peek() {
                    self.pos += 1;
                }
                self.expect_punct(";")?;
                StmtKind::Continue
            }
            _ => {
                let e = self.expression()?;
                self.expect_punct(";")?;
                StmtKind::Expr(e)
            }
        };
        Ok(Stmt { line, kind })
    }

    /// A `{ ... }` block.
    fn block(&mut self) -> Result<Vec<Stmt>, String> {
        self.expect_punct("{")?;
        let mut body = Vec::new();
        while !self.at_punct("}") && !self.at_end() {
            body.push(self.statement()?);
        }
        self.expect_punct("}")?;
        Ok(body)
    }

    /// A braced block or a single statement (for brace-less control bodies).
    fn body(&mut self) -> Result<Vec<Stmt>, String> {
        if self.at_punct("{") {
            self.block()
        } else {
            Ok(vec![self.statement()?])
        }
    }

    fn if_stmt(&mut self) -> Result<StmtKind, String> {
        self.pos += 1; // if
        self.expect_punct("(")?;
        let cond = self.expression()?;
        self.expect_punct(")")?;
        let then = self.body()?;
        let mut elifs = Vec::new();
        let mut els = None;
        loop {
            if self.eat_kw("elseif") {
                self.expect_punct("(")?;
                let c = self.expression()?;
                self.expect_punct(")")?;
                elifs.push((c, self.body()?));
            } else if self.at_kw("else") {
                self.pos += 1;
                // `else if` is two keywords; fold it into an elseif branch.
                if self.eat_kw("if") {
                    self.expect_punct("(")?;
                    let c = self.expression()?;
                    self.expect_punct(")")?;
                    elifs.push((c, self.body()?));
                } else {
                    els = Some(self.body()?);
                    break;
                }
            } else {
                break;
            }
        }
        Ok(StmtKind::If {
            cond,
            then,
            elifs,
            els,
        })
    }

    fn while_stmt(&mut self) -> Result<StmtKind, String> {
        self.pos += 1;
        self.expect_punct("(")?;
        let cond = self.expression()?;
        self.expect_punct(")")?;
        let body = self.body()?;
        Ok(StmtKind::While { cond, body })
    }

    fn do_while_stmt(&mut self) -> Result<StmtKind, String> {
        self.pos += 1; // do
        let body = self.body()?;
        if !self.eat_kw("while") {
            return Err(format!(
                "expected 'while' after 'do' body (line {})",
                self.line()
            ));
        }
        self.expect_punct("(")?;
        let cond = self.expression()?;
        self.expect_punct(")")?;
        self.expect_punct(";")?;
        Ok(StmtKind::DoWhile { cond, body })
    }

    fn switch_stmt(&mut self) -> Result<StmtKind, String> {
        self.pos += 1; // switch
        self.expect_punct("(")?;
        let subj = self.expression()?;
        self.expect_punct(")")?;
        self.expect_punct("{")?;
        let mut cases = Vec::new();
        while !self.at_punct("}") && !self.at_end() {
            // A `case EXPR:` or `default:` label (PHP also allows `;` for `:`).
            let test = if self.eat_kw("case") {
                let e = self.expression()?;
                if !self.eat_punct(":") {
                    self.expect_punct(";")?;
                }
                Some(e)
            } else if self.eat_kw("default") {
                if !self.eat_punct(":") {
                    self.expect_punct(";")?;
                }
                None
            } else {
                return Err(format!(
                    "expected 'case' or 'default' in switch (line {})",
                    self.line()
                ));
            };
            // The case body runs until the next case/default or the closing brace.
            let mut body = Vec::new();
            while !self.at_kw("case")
                && !self.at_kw("default")
                && !self.at_punct("}")
                && !self.at_end()
            {
                body.push(self.statement()?);
            }
            cases.push(SwitchCase { test, body });
        }
        self.expect_punct("}")?;
        Ok(StmtKind::Switch { subj, cases })
    }

    fn for_stmt(&mut self) -> Result<StmtKind, String> {
        self.pos += 1;
        self.expect_punct("(")?;
        let init = self.expr_list_until(";")?;
        self.expect_punct(";")?;
        let cond = if self.at_punct(";") {
            None
        } else {
            Some(self.expression()?)
        };
        self.expect_punct(";")?;
        let step = self.expr_list_until(")")?;
        self.expect_punct(")")?;
        let body = self.body()?;
        Ok(StmtKind::For {
            init,
            cond,
            step,
            body,
        })
    }

    /// A comma-separated expression list up to (but not consuming) `stop`.
    fn expr_list_until(&mut self, stop: &str) -> Result<Vec<Expr>, String> {
        let mut v = Vec::new();
        if self.at_punct(stop) {
            return Ok(v);
        }
        v.push(self.expression()?);
        while self.eat_punct(",") {
            v.push(self.expression()?);
        }
        Ok(v)
    }

    fn foreach_stmt(&mut self) -> Result<StmtKind, String> {
        self.pos += 1;
        self.expect_punct("(")?;
        let arr = self.expression()?;
        if !self.eat_kw("as") {
            return Err(format!("expected 'as' in foreach (line {})", self.line()));
        }
        let first = self.expect_var()?;
        let (key_var, val_var) = if self.eat_punct("=>") {
            (Some(first), self.expect_var()?)
        } else {
            (None, first)
        };
        self.expect_punct(")")?;
        let body = self.body()?;
        Ok(StmtKind::Foreach {
            arr,
            key_var,
            val_var,
            body,
        })
    }

    fn expect_var(&mut self) -> Result<String, String> {
        match self.next() {
            Some(Tok::Var(n)) => Ok(n),
            other => Err(format!(
                "expected a $variable but found {other:?} (line {})",
                self.line()
            )),
        }
    }

    fn function_stmt(&mut self) -> Result<StmtKind, String> {
        self.pos += 1; // function
        let name = match self.next() {
            Some(Tok::Ident(n)) => n,
            other => {
                return Err(format!(
                    "expected function name but found {other:?} (line {})",
                    self.line()
                ))
            }
        };
        self.expect_punct("(")?;
        let mut params = Vec::new();
        if !self.at_punct(")") {
            loop {
                // Skip an optional type hint (a bare identifier before the $var).
                if let Some(Tok::Ident(_)) = self.peek() {
                    self.pos += 1;
                }
                params.push(self.expect_var()?);
                // A default value (`$x = expr`) is parsed and discarded in the
                // scaffold — arity binding ignores defaults for now.
                if self.eat_punct("=") {
                    let _ = self.expression()?;
                }
                if !self.eat_punct(",") {
                    break;
                }
            }
        }
        self.expect_punct(")")?;
        // Skip an optional return-type hint (`: type`).
        if self.eat_punct(":") {
            if let Some(Tok::Ident(_)) = self.peek() {
                self.pos += 1;
            }
        }
        let body = self.block()?;
        Ok(StmtKind::Function { name, params, body })
    }

    // ── expressions (precedence climbing) ──────────────────────────────────

    fn expression(&mut self) -> Result<Expr, String> {
        self.assignment()
    }

    fn assignment(&mut self) -> Result<Expr, String> {
        let lhs = self.ternary()?;
        let op = match self.peek() {
            Some(Tok::Punct("=")) => Some(None),
            Some(Tok::Punct("+=")) => Some(Some(BinOp::Add)),
            Some(Tok::Punct("-=")) => Some(Some(BinOp::Sub)),
            Some(Tok::Punct("*=")) => Some(Some(BinOp::Mul)),
            Some(Tok::Punct("/=")) => Some(Some(BinOp::Div)),
            Some(Tok::Punct("%=")) => Some(Some(BinOp::Mod)),
            Some(Tok::Punct(".=")) => Some(Some(BinOp::Concat)),
            Some(Tok::Punct("**=")) => Some(Some(BinOp::Pow)),
            _ => None,
        };
        if let Some(compound) = op {
            self.pos += 1;
            let rhs = self.assignment()?; // right-associative
            return Ok(Expr::Assign(Box::new(lhs), compound, Box::new(rhs)));
        }
        Ok(lhs)
    }

    fn ternary(&mut self) -> Result<Expr, String> {
        let cond = self.binary(0)?;
        // Null coalesce `a ?? b` (right-associative). The lexer has no `??`
        // token, so it surfaces as two consecutive `?` tokens.
        if self.at_punct("?") && self.peek2_is_punct("?") {
            self.pos += 2;
            let rhs = self.ternary()?;
            return Ok(Expr::Coalesce(Box::new(cond), Box::new(rhs)));
        }
        if self.eat_punct("?") {
            // Short ternary / elvis `a ?: b`.
            if self.eat_punct(":") {
                let els = self.assignment()?;
                return Ok(Expr::Elvis(Box::new(cond), Box::new(els)));
            }
            let then = self.expression()?;
            self.expect_punct(":")?;
            let els = self.assignment()?;
            return Ok(Expr::Ternary(Box::new(cond), Box::new(then), Box::new(els)));
        }
        Ok(cond)
    }

    /// Precedence-climbing binary parser. Higher `min_bp` binds tighter.
    fn binary(&mut self, min_bp: u8) -> Result<Expr, String> {
        let mut lhs = self.unary()?;
        while let Some((op, lbp, rbp)) = self.peek_binop() {
            if lbp < min_bp {
                break;
            }
            self.pos += 1;
            let rhs = self.binary(rbp)?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    /// The binary operator at the cursor, plus its left/right binding powers.
    fn peek_binop(&self) -> Option<(BinOp, u8, u8)> {
        let op = match self.peek() {
            Some(Tok::Punct("||")) => BinOp::Or,
            Some(Tok::Punct("&&")) => BinOp::And,
            Some(Tok::Ident(s)) if s.eq_ignore_ascii_case("or") => BinOp::Or,
            Some(Tok::Ident(s)) if s.eq_ignore_ascii_case("and") => BinOp::And,
            Some(Tok::Punct("==")) => BinOp::LooseEq,
            Some(Tok::Punct("!=")) | Some(Tok::Punct("<>")) => BinOp::LooseNe,
            Some(Tok::Punct("===")) => BinOp::StrictEq,
            Some(Tok::Punct("!==")) => BinOp::StrictNe,
            Some(Tok::Punct("<")) => BinOp::Lt,
            Some(Tok::Punct(">")) => BinOp::Gt,
            Some(Tok::Punct("<=")) => BinOp::Le,
            Some(Tok::Punct(">=")) => BinOp::Ge,
            Some(Tok::Punct("+")) => BinOp::Add,
            Some(Tok::Punct("-")) => BinOp::Sub,
            Some(Tok::Punct(".")) => BinOp::Concat,
            Some(Tok::Punct("*")) => BinOp::Mul,
            Some(Tok::Punct("/")) => BinOp::Div,
            Some(Tok::Punct("%")) => BinOp::Mod,
            // `**` is NOT handled here — it binds tighter than unary minus, so it
            // is parsed in `power()` below the unary level, not as an infix op.
            _ => return None,
        };
        // (left bp, right bp). Right bp < left bp ⇒ right-associative (`**`).
        let (l, r) = match op {
            BinOp::Or => (1, 2),
            BinOp::And => (3, 4),
            BinOp::LooseEq | BinOp::LooseNe | BinOp::StrictEq | BinOp::StrictNe => (5, 6),
            BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => (7, 8),
            BinOp::Add | BinOp::Sub | BinOp::Concat => (9, 10),
            BinOp::Mul | BinOp::Div | BinOp::Mod => (11, 12),
            BinOp::Pow => (14, 13),
        };
        Some((op, l, r))
    }

    fn unary(&mut self) -> Result<Expr, String> {
        // Type cast: `(int)`, `(float)`, `(string)`, `(bool)`, … — three tokens
        // `( ident )` where the identifier names a cast target. Desugars to the
        // matching conversion call so no new opcode is needed.
        if self.at_punct("(") {
            if let Some(Tok::Ident(t)) = self.toks.get(self.pos + 1).map(|s| &s.tok) {
                if matches!(
                    self.toks.get(self.pos + 2).map(|s| &s.tok),
                    Some(Tok::Punct(")"))
                ) {
                    if let Some(fname) = cast_fn(t) {
                        self.pos += 3; // consume `( ident )`
                        let operand = self.unary()?;
                        return Ok(Expr::Call(fname.to_string(), vec![operand]));
                    }
                }
            }
        }
        if self.eat_punct("!") {
            return Ok(Expr::Unary(UnOp::Not, Box::new(self.unary()?)));
        }
        if self.eat_punct("-") {
            return Ok(Expr::Unary(UnOp::Neg, Box::new(self.unary()?)));
        }
        if self.eat_punct("+") {
            return Ok(Expr::Unary(UnOp::Pos, Box::new(self.unary()?)));
        }
        if self.eat_punct("++") {
            let t = self.unary()?;
            return Ok(Expr::IncDec {
                target: Box::new(t),
                inc: true,
                prefix: true,
            });
        }
        if self.eat_punct("--") {
            let t = self.unary()?;
            return Ok(Expr::IncDec {
                target: Box::new(t),
                inc: false,
                prefix: true,
            });
        }
        self.power()
    }

    /// The exponent level, sitting *below* unary so `-2 ** 2` parses as
    /// `-(2 ** 2)` (PHP binds `**` tighter than unary minus). Right-associative,
    /// and its right operand is a full unary expression so `2 ** -1` works.
    fn power(&mut self) -> Result<Expr, String> {
        let base = self.postfix()?;
        if self.eat_punct("**") {
            let exp = self.unary()?;
            return Ok(Expr::Binary(BinOp::Pow, Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }

    fn postfix(&mut self) -> Result<Expr, String> {
        let mut e = self.primary()?;
        loop {
            if self.eat_punct("[") {
                // `$a[]` (append) is only meaningful as an assignment target.
                if self.eat_punct("]") {
                    e = Expr::Append(Box::new(e));
                } else {
                    let idx = self.expression()?;
                    self.expect_punct("]")?;
                    e = Expr::Index(Box::new(e), Box::new(idx));
                }
            } else if self.at_punct("++") {
                self.pos += 1;
                e = Expr::IncDec {
                    target: Box::new(e),
                    inc: true,
                    prefix: false,
                };
            } else if self.at_punct("--") {
                self.pos += 1;
                e = Expr::IncDec {
                    target: Box::new(e),
                    inc: false,
                    prefix: false,
                };
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn primary(&mut self) -> Result<Expr, String> {
        match self.next() {
            Some(Tok::Int(n)) => Ok(Expr::Int(n)),
            Some(Tok::Float(f)) => Ok(Expr::Float(f)),
            Some(Tok::Str(s)) => Ok(Expr::Str(s)),
            Some(Tok::Interp(parts)) => Ok(Expr::Interp(parts)),
            Some(Tok::Var(n)) => Ok(Expr::Var(n)),
            Some(Tok::Punct("(")) => {
                let e = self.expression()?;
                self.expect_punct(")")?;
                Ok(e)
            }
            Some(Tok::Punct("[")) => self.array_literal("]"),
            Some(Tok::Ident(kw)) if kw.eq_ignore_ascii_case("true") => Ok(Expr::Bool(true)),
            Some(Tok::Ident(kw)) if kw.eq_ignore_ascii_case("false") => Ok(Expr::Bool(false)),
            Some(Tok::Ident(kw)) if kw.eq_ignore_ascii_case("null") => Ok(Expr::Null),
            Some(Tok::Ident(kw)) if kw.eq_ignore_ascii_case("array") => {
                self.expect_punct("(")?;
                self.array_literal(")")
            }
            // `match (subj) { ... }` — only when followed by `(`, so a plain
            // bareword `match` still parses as a name.
            Some(Tok::Ident(kw)) if kw.eq_ignore_ascii_case("match") && self.at_punct("(") => {
                self.match_expr()
            }
            Some(Tok::Ident(name)) => {
                // A bareword followed by `(` is a function call.
                if self.eat_punct("(") {
                    let mut args = Vec::new();
                    if !self.at_punct(")") {
                        loop {
                            args.push(self.expression()?);
                            if !self.eat_punct(",") {
                                break;
                            }
                        }
                    }
                    self.expect_punct(")")?;
                    // `isset()`/`empty()` are language constructs, not functions:
                    // they must not error on an undefined variable/key. phplang
                    // returns `null` for a missing var/index silently, so both
                    // desugar to plain operators over existing ops.
                    if name.eq_ignore_ascii_case("empty") && args.len() == 1 {
                        // empty($x) ≡ !$x (both are false-on-truthy, quiet on unset).
                        return Ok(Expr::Unary(
                            UnOp::Not,
                            Box::new(args.into_iter().next().unwrap()),
                        ));
                    }
                    if name.eq_ignore_ascii_case("isset") && !args.is_empty() {
                        // isset($a, $b, …) ≡ ($a !== null) && ($b !== null) && …
                        let mut it = args.into_iter();
                        let mut expr = Expr::Binary(
                            BinOp::StrictNe,
                            Box::new(it.next().unwrap()),
                            Box::new(Expr::Null),
                        );
                        for a in it {
                            let term =
                                Expr::Binary(BinOp::StrictNe, Box::new(a), Box::new(Expr::Null));
                            expr = Expr::Binary(BinOp::And, Box::new(expr), Box::new(term));
                        }
                        return Ok(expr);
                    }
                    Ok(Expr::Call(name, args))
                } else {
                    // A bare constant name; the scaffold has no user constants, so
                    // treat an unknown bareword as its string name (PHP 7 behaviour
                    // for undefined constants, minus the notice).
                    Ok(Expr::Str(name))
                }
            }
            other => Err(format!("unexpected token {other:?} (line {})", self.line())),
        }
    }

    /// Parse array elements up to `close` (already past the opener).
    fn array_literal(&mut self, close: &str) -> Result<Expr, String> {
        let mut elems = Vec::new();
        while !self.at_punct(close) && !self.at_end() {
            let first = self.expression()?;
            if self.eat_punct("=>") {
                let val = self.expression()?;
                elems.push((Some(first), val));
            } else {
                elems.push((None, first));
            }
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_punct(close)?;
        Ok(Expr::Array(elems))
    }

    /// Parse a `match (subj) { A, B => R, default => D }` expression. The `match`
    /// keyword has already been consumed by `primary`.
    fn match_expr(&mut self) -> Result<Expr, String> {
        self.expect_punct("(")?;
        let subj = self.expression()?;
        self.expect_punct(")")?;
        self.expect_punct("{")?;
        let mut arms = Vec::new();
        while !self.at_punct("}") && !self.at_end() {
            let conds = if self.eat_kw("default") {
                None
            } else {
                let mut cs = vec![self.expression()?];
                while self.eat_punct(",") {
                    // Tolerate a trailing comma before `=>`.
                    if self.at_punct("=>") {
                        break;
                    }
                    cs.push(self.expression()?);
                }
                Some(cs)
            };
            self.expect_punct("=>")?;
            let body = self.expression()?;
            arms.push(MatchArm {
                conds,
                body: Box::new(body),
            });
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_punct("}")?;
        Ok(Expr::Match {
            subj: Box::new(subj),
            arms,
        })
    }
}
