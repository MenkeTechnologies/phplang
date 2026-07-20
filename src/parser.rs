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

/// Parse a PHP source string into a statement list. Inline `rust { ... }` FFI
/// blocks are desugared to `__rust_compile(...)` calls before lexing.
pub fn parse(src: &str) -> Result<Vec<Stmt>, String> {
    let src = crate::rust_ffi::desugar(src);
    let toks = lexer::lex(&src)?;
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
        self.nth_is_punct(1, p)
    }

    /// Whether the token `n` positions ahead of the cursor is the punctuation `p`.
    fn nth_is_punct(&self, n: usize, p: &str) -> bool {
        matches!(self.toks.get(self.pos + n).map(|s| &s.tok), Some(Tok::Punct(x)) if *x == p)
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
            _ if self.at_kw("class")
                || ((self.at_kw("abstract") || self.at_kw("final"))
                    && matches!(self.toks.get(self.pos + 1).map(|s| &s.tok),
                        Some(Tok::Ident(s)) if s.eq_ignore_ascii_case("class"))) =>
            {
                self.class_stmt()?
            }
            _ if self.at_kw("try") => self.try_stmt()?,
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

    /// A property / method / constant name after `->` or `::` (a bare identifier).
    fn member_name(&mut self) -> Result<String, String> {
        match self.next() {
            Some(Tok::Ident(n)) => Ok(n),
            other => Err(format!(
                "expected a member name but found {other:?} (line {})",
                self.line()
            )),
        }
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
        let params = self.param_list()?;
        self.skip_return_type();
        let body = self.block()?;
        Ok(StmtKind::Function { name, params, body })
    }

    /// `try { body } catch (T1 | T2 [$e]) { ... } ... [finally { ... }]` — at
    /// least one `catch` or a `finally` is required (PHP rule).
    fn try_stmt(&mut self) -> Result<StmtKind, String> {
        self.pos += 1; // try
        let body = self.block()?;
        let mut catches = Vec::new();
        while self.at_kw("catch") {
            self.pos += 1;
            self.expect_punct("(")?;
            // A `|`-separated union of class names.
            let mut types = vec![self.expect_type_name()?];
            while self.eat_punct("|") {
                types.push(self.expect_type_name()?);
            }
            // The bound `$var` is optional (PHP 8 allows `catch (T) { }`).
            let var = match self.peek() {
                Some(Tok::Var(_)) => Some(self.expect_var()?),
                _ => None,
            };
            self.expect_punct(")")?;
            let cbody = self.block()?;
            catches.push(CatchArm {
                types,
                var,
                body: cbody,
            });
        }
        let finally = if self.eat_kw("finally") {
            Some(self.block()?)
        } else {
            None
        };
        if catches.is_empty() && finally.is_none() {
            return Err(format!(
                "'try' must be followed by 'catch' or 'finally' (line {})",
                self.line()
            ));
        }
        Ok(StmtKind::Try {
            body,
            catches,
            finally,
        })
    }

    /// A (possibly namespaced) class name in a `catch`. A leading `\` and any
    /// `Ns\Name` segments are folded to the trailing bare name — the scaffold has
    /// no namespaces, and the built-in exception classes are unqualified.
    fn expect_type_name(&mut self) -> Result<String, String> {
        self.eat_punct("\\");
        let mut name = match self.next() {
            Some(Tok::Ident(n)) => n,
            other => {
                return Err(format!(
                    "expected a class name but found {other:?} (line {})",
                    self.line()
                ))
            }
        };
        while self.eat_punct("\\") {
            if let Some(Tok::Ident(n)) = self.next() {
                name = n;
            }
        }
        Ok(name)
    }

    /// Skip an optional return-type hint (`: [?]type`).
    fn skip_return_type(&mut self) {
        if self.eat_punct(":") {
            self.eat_punct("?");
            if let Some(Tok::Ident(_)) = self.peek() {
                self.pos += 1;
            }
        }
    }

    /// Parse a `( ... )` formal parameter list. Before each `$var` it skips
    /// modifiers and type hints — visibility keywords, `?` nullable, `&` by-ref,
    /// and bareword type names — then reads an optional `...$rest` variadic marker,
    /// the name, and an optional `= default` value. By-ref (`&`) and typed hints
    /// are accepted but not enforced in the scaffold.
    fn param_list(&mut self) -> Result<Vec<Param>, String> {
        self.expect_punct("(")?;
        let mut params = Vec::new();
        if !self.at_punct(")") {
            loop {
                // Skip a leading modifier/type-hint chain up to `...` or `$var`. A
                // visibility/readonly keyword marks a promoted constructor property.
                let mut promoted = false;
                loop {
                    if self.eat_punct("?") || self.eat_punct("&") {
                        continue;
                    }
                    if self.at_punct("...") || matches!(self.peek(), Some(Tok::Var(_))) {
                        break;
                    }
                    if let Some(Tok::Ident(kw)) = self.peek() {
                        if matches!(
                            kw.to_ascii_lowercase().as_str(),
                            "public" | "private" | "protected" | "readonly"
                        ) {
                            promoted = true;
                        }
                        self.pos += 1;
                        continue;
                    }
                    break;
                }
                // `...$rest` collects all trailing arguments into an array.
                let variadic = self.eat_punct("...");
                let name = self.expect_var()?;
                // A default value (`$x = expr`), applied when the caller omits it.
                let default = if self.eat_punct("=") {
                    Some(self.expression()?)
                } else {
                    None
                };
                params.push(Param {
                    name,
                    default,
                    variadic,
                    promoted,
                });
                if !self.eat_punct(",") {
                    break;
                }
            }
        }
        self.expect_punct(")")?;
        Ok(params)
    }

    /// Parse a call argument list up to and consuming the closing `)` (the opening
    /// `(` is already eaten). Supports `...$arr` argument unpacking.
    fn arg_list(&mut self) -> Result<Vec<Expr>, String> {
        let mut args = Vec::new();
        if !self.at_punct(")") {
            loop {
                if self.eat_punct("...") {
                    args.push(Expr::Spread(Box::new(self.expression()?)));
                } else {
                    args.push(self.expression()?);
                }
                if !self.eat_punct(",") {
                    break;
                }
            }
        }
        self.expect_punct(")")?;
        Ok(args)
    }

    /// `class Name [extends Parent] [implements ...] { members }`. Members are
    /// consts, properties (with visibility/static/type modifiers), and methods.
    /// A leading `abstract`/`final` class modifier is accepted but not enforced.
    fn class_stmt(&mut self) -> Result<StmtKind, String> {
        if !self.at_kw("class") {
            self.pos += 1; // abstract / final
        }
        self.pos += 1; // class
        let name = match self.next() {
            Some(Tok::Ident(n)) => n,
            other => {
                return Err(format!(
                    "expected class name but found {other:?} (line {})",
                    self.line()
                ))
            }
        };
        let parent = if self.eat_kw("extends") {
            match self.next() {
                Some(Tok::Ident(n)) => Some(n),
                other => {
                    return Err(format!(
                        "expected parent class name but found {other:?} (line {})",
                        self.line()
                    ))
                }
            }
        } else {
            None
        };
        // `implements Iface, ...` — parsed and discarded (no interfaces here).
        if self.eat_kw("implements") {
            loop {
                if let Some(Tok::Ident(_)) = self.peek() {
                    self.pos += 1;
                }
                if !self.eat_punct(",") {
                    break;
                }
            }
        }
        self.expect_punct("{")?;
        let mut consts = Vec::new();
        let mut props = Vec::new();
        let mut methods = Vec::new();
        while !self.at_punct("}") && !self.at_end() {
            // Member modifiers; only `static` is retained, the rest are accepted
            // (visibility is parsed but not enforced in this wave).
            let mut is_static = false;
            loop {
                if self.eat_kw("static") {
                    is_static = true;
                } else if self.at_kw("public")
                    || self.at_kw("private")
                    || self.at_kw("protected")
                    || self.at_kw("abstract")
                    || self.at_kw("final")
                    || self.at_kw("readonly")
                    || self.at_kw("var")
                {
                    self.pos += 1;
                } else {
                    break;
                }
            }
            if self.eat_kw("const") {
                loop {
                    let cname = match self.next() {
                        Some(Tok::Ident(n)) => n,
                        other => {
                            return Err(format!(
                                "expected constant name but found {other:?} (line {})",
                                self.line()
                            ))
                        }
                    };
                    self.expect_punct("=")?;
                    consts.push((cname, self.expression()?));
                    if !self.eat_punct(",") {
                        break;
                    }
                }
                self.expect_punct(";")?;
            } else if self.at_kw("function") {
                self.pos += 1; // function
                self.eat_punct("&"); // return-by-ref marker
                let mname = match self.next() {
                    Some(Tok::Ident(n)) => n,
                    other => {
                        return Err(format!(
                            "expected method name but found {other:?} (line {})",
                            self.line()
                        ))
                    }
                };
                let params = self.param_list()?;
                self.skip_return_type();
                // An abstract/interface method has no body, just `;`.
                let body = if self.eat_punct(";") {
                    Vec::new()
                } else {
                    self.block()?
                };
                methods.push(Method {
                    name: mname,
                    params,
                    body,
                    is_static,
                });
            } else {
                // Property declaration(s): an optional type hint precedes the $var.
                if !matches!(self.peek(), Some(Tok::Var(_))) {
                    self.eat_punct("?");
                    if let Some(Tok::Ident(_)) = self.peek() {
                        self.pos += 1;
                    }
                }
                loop {
                    let pname = self.expect_var()?;
                    let default = if self.eat_punct("=") {
                        Some(self.expression()?)
                    } else {
                        None
                    };
                    props.push((pname, default));
                    if !self.eat_punct(",") {
                        break;
                    }
                }
                self.expect_punct(";")?;
            }
        }
        self.expect_punct("}")?;
        Ok(StmtKind::Class(ClassDecl {
            name,
            parent,
            consts,
            props,
            methods,
        }))
    }

    // ── expressions (precedence climbing) ──────────────────────────────────

    fn expression(&mut self) -> Result<Expr, String> {
        self.assignment()
    }

    fn assignment(&mut self) -> Result<Expr, String> {
        let lhs = self.ternary()?;
        // `??=` — the lexer emits `? ? =`; ternary() leaves the `??` unconsumed
        // when a `=` follows (see its lookahead). Desugar `$x ??= v` to
        // `$x = ($x ?? v)`.
        if self.at_punct("?") && self.nth_is_punct(1, "?") && self.nth_is_punct(2, "=") {
            self.pos += 3;
            let rhs = self.assignment()?;
            let coalesce = Expr::Coalesce(Box::new(lhs.clone()), Box::new(rhs));
            return Ok(Expr::Assign(Box::new(lhs), None, Box::new(coalesce)));
        }
        let op = match self.peek() {
            Some(Tok::Punct("=")) => Some(None),
            Some(Tok::Punct("+=")) => Some(Some(BinOp::Add)),
            Some(Tok::Punct("-=")) => Some(Some(BinOp::Sub)),
            Some(Tok::Punct("*=")) => Some(Some(BinOp::Mul)),
            Some(Tok::Punct("/=")) => Some(Some(BinOp::Div)),
            Some(Tok::Punct("%=")) => Some(Some(BinOp::Mod)),
            Some(Tok::Punct(".=")) => Some(Some(BinOp::Concat)),
            Some(Tok::Punct("**=")) => Some(Some(BinOp::Pow)),
            Some(Tok::Punct("&=")) => Some(Some(BinOp::BitAnd)),
            Some(Tok::Punct("|=")) => Some(Some(BinOp::BitOr)),
            Some(Tok::Punct("^=")) => Some(Some(BinOp::BitXor)),
            Some(Tok::Punct("<<=")) => Some(Some(BinOp::Shl)),
            Some(Tok::Punct(">>=")) => Some(Some(BinOp::Shr)),
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
        // A trailing `=` means this is `??=`, handled by `assignment()`; leave it.
        if self.at_punct("?") && self.peek2_is_punct("?") && !self.nth_is_punct(2, "=") {
            self.pos += 2;
            let rhs = self.ternary()?;
            return Ok(Expr::Coalesce(Box::new(cond), Box::new(rhs)));
        }
        // A `?` NOT followed by another `?` is the real ternary; `? ? …` here is
        // a `??=` left for `assignment()` (the coalesce case was handled above).
        if self.at_punct("?") && !self.peek2_is_punct("?") {
            self.pos += 1;
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
            Some(Tok::Punct("<=>")) => BinOp::Spaceship,
            Some(Tok::Punct("<")) => BinOp::Lt,
            Some(Tok::Punct(">")) => BinOp::Gt,
            Some(Tok::Punct("<=")) => BinOp::Le,
            Some(Tok::Punct(">=")) => BinOp::Ge,
            Some(Tok::Punct("<<")) => BinOp::Shl,
            Some(Tok::Punct(">>")) => BinOp::Shr,
            Some(Tok::Punct("&")) => BinOp::BitAnd,
            Some(Tok::Punct("|")) => BinOp::BitOr,
            Some(Tok::Punct("^")) => BinOp::BitXor,
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
        // (left bp, right bp), following PHP operator precedence (loosest first):
        // || < && < | < ^ < & < equality < relational < shift < additive <
        // multiplicative. Right bp < left bp ⇒ right-associative.
        let (l, r) = match op {
            BinOp::Or => (1, 2),
            BinOp::And => (3, 4),
            BinOp::BitOr => (5, 6),
            BinOp::BitXor => (7, 8),
            BinOp::BitAnd => (9, 10),
            BinOp::LooseEq
            | BinOp::LooseNe
            | BinOp::StrictEq
            | BinOp::StrictNe
            | BinOp::Spaceship => (11, 12),
            BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => (13, 14),
            BinOp::Shl | BinOp::Shr => (15, 16),
            BinOp::Add | BinOp::Sub | BinOp::Concat => (17, 18),
            BinOp::Mul | BinOp::Div | BinOp::Mod => (19, 20),
            BinOp::Pow => (22, 21),
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
        if self.eat_punct("~") {
            return Ok(Expr::Unary(UnOp::BitNot, Box::new(self.unary()?)));
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
            } else if self.eat_punct("->") {
                // Instance member: `$o->prop` or `$o->method(args)`.
                let member = self.member_name()?;
                if self.eat_punct("(") {
                    e = Expr::MethodCall(Box::new(e), member, self.arg_list()?);
                } else {
                    e = Expr::PropGet(Box::new(e), member);
                }
            } else if self.eat_punct("::") {
                // Static / scope-resolution access: the left must be a class name
                // — a bareword, which surfaces here as `Expr::ConstFetch` (or a
                // legacy `Expr::Str`).
                let class = match e {
                    Expr::ConstFetch(name) | Expr::Str(name) => name,
                    _ => {
                        return Err(format!(
                            "expected a class name before '::' (line {})",
                            self.line()
                        ))
                    }
                };
                let member = self.member_name()?;
                if self.eat_punct("(") {
                    e = Expr::StaticCall(class, member, self.arg_list()?);
                } else {
                    e = Expr::StaticGet(class, member);
                }
            } else if self.at_punct("(") {
                // A `( args )` applied to any primary value is a dynamic call: a
                // closure held in `$f`, or an immediately-invoked `foo()(…)` /
                // `(expr)(…)`. A bareword `name(` is already consumed as
                // `Expr::Call` in `primary`, so this only fires on a value callee.
                self.pos += 1;
                let args = self.arg_list()?;
                e = Expr::CallValue(Box::new(e), args);
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
            // `throw e` as a PHP 8 expression, so `$x ?? throw …` and
            // `cond ? throw … : …` work; a `throw e;` statement reaches here too.
            Some(Tok::Ident(kw)) if kw.eq_ignore_ascii_case("throw") => {
                let e = self.expression()?;
                Ok(Expr::Throw(Box::new(e)))
            }
            // `new Class(args)` — the class name is a bareword (or `self`/`parent`/
            // `static`); parentheses are optional when there are no arguments.
            Some(Tok::Ident(kw)) if kw.eq_ignore_ascii_case("new") => {
                let class = match self.next() {
                    Some(Tok::Ident(n)) => n,
                    other => {
                        return Err(format!(
                            "expected a class name after 'new' but found {other:?} (line {})",
                            self.line()
                        ))
                    }
                };
                let args = if self.eat_punct("(") {
                    self.arg_list()?
                } else {
                    Vec::new()
                };
                Ok(Expr::New(class, args))
            }
            // `match (subj) { ... }` — only when followed by `(`, so a plain
            // bareword `match` still parses as a name.
            Some(Tok::Ident(kw)) if kw.eq_ignore_ascii_case("match") && self.at_punct("(") => {
                self.match_expr()
            }
            // An anonymous function `function (params) [use (vars)] { body }`.
            // (A *named* function is a statement, caught in `statement()`; only
            // the expression form — `function (` — reaches here.)
            Some(Tok::Ident(kw)) if kw.eq_ignore_ascii_case("function") && self.at_punct("(") => {
                let params = self.param_list()?;
                let mut uses = Vec::new();
                if self.eat_kw("use") {
                    self.expect_punct("(")?;
                    if !self.at_punct(")") {
                        loop {
                            // By-reference capture (`use (&$v)`) is not supported —
                            // the scaffold has no reference cells, so it would
                            // silently bind by value and break recursive/mutating
                            // closures. Reject it loudly instead.
                            if self.at_punct("&") {
                                return Err(format!(
                                    "by-reference closure capture `use (&${{…}})` is not supported (line {})",
                                    self.line()
                                ));
                            }
                            uses.push(self.expect_var()?);
                            if !self.eat_punct(",") {
                                break;
                            }
                        }
                    }
                    self.expect_punct(")")?;
                }
                self.skip_return_type();
                let body = self.block()?;
                Ok(Expr::Closure { params, uses, body })
            }
            // An arrow function `fn (params) => expr` — implicit by-value capture.
            Some(Tok::Ident(kw)) if kw.eq_ignore_ascii_case("fn") && self.at_punct("(") => {
                let params = self.param_list()?;
                self.skip_return_type();
                self.expect_punct("=>")?;
                let body = self.expression()?;
                Ok(Expr::ArrowFn {
                    params,
                    body: Box::new(body),
                })
            }
            Some(Tok::Ident(name)) => {
                // A bareword followed by `(` is a function call.
                if self.eat_punct("(") {
                    let args = self.arg_list()?;
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
                    // A bare constant reference — resolved against the constant
                    // table at runtime, falling back to the bare name as a string
                    // when undefined (PHP 7 leniency for undefined constants).
                    Ok(Expr::ConstFetch(name))
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
