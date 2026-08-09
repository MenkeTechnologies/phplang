//! Recursive-descent parser: `lexer` tokens → PHP AST (`ast::Stmt`).

use crate::ast::*;
use crate::lexer::{self, Spanned, Tok};

/// The words PHP's scanner turns into keyword tokens rather than identifiers, in
/// the canonical spelling a syntax error echoes back — lowercase for ordinary
/// keywords, uppercase for the magic constants, and `die` folded onto `exit`,
/// which is the token it produces. Matched case-insensitively.
///
/// `true`, `false`, `null`, `self`, `parent`, `enum` and the scalar type names are
/// deliberately absent: the scanner leaves them as identifiers, and PHP reports
/// them as `identifier "…"`.
const RESERVED_WORDS: &[&str] = &[
    "abstract",
    "and",
    "array",
    "as",
    "break",
    "callable",
    "case",
    "catch",
    "class",
    "clone",
    "const",
    "continue",
    "declare",
    "default",
    "do",
    "echo",
    "else",
    "elseif",
    "empty",
    "enddeclare",
    "endfor",
    "endforeach",
    "endif",
    "endswitch",
    "endwhile",
    "eval",
    "exit",
    "extends",
    "final",
    "finally",
    "fn",
    "for",
    "foreach",
    "function",
    "global",
    "goto",
    "if",
    "implements",
    "include",
    "include_once",
    "instanceof",
    "insteadof",
    "interface",
    "isset",
    "list",
    "match",
    "namespace",
    "new",
    "or",
    "print",
    "private",
    "protected",
    "public",
    "readonly",
    "require",
    "require_once",
    "return",
    "static",
    "switch",
    "throw",
    "trait",
    "try",
    "unset",
    "use",
    "var",
    "while",
    "xor",
    "yield",
    "__CLASS__",
    "__DIR__",
    "__FILE__",
    "__FUNCTION__",
    "__LINE__",
    "__METHOD__",
    "__NAMESPACE__",
];

/// The canonical keyword spelling for `word`, or `None` if the scanner would
/// leave it an identifier.
fn reserved_spelling(word: &str) -> Option<&'static str> {
    // `die` is an alias the scanner folds onto the `exit` token, so a syntax
    // error on `die` reports `token "exit"`.
    if word.eq_ignore_ascii_case("die") {
        return Some("exit");
    }
    RESERVED_WORDS
        .iter()
        .find(|k| k.eq_ignore_ascii_case(word))
        .copied()
}

/// Whether `e` is the `variable` PHP's grammar requires either side of a
/// `++`/`--`: a plain `$v`, an array or string element, an instance or static
/// property. Everything else — a literal, a call result, a parenthesised
/// expression — is a syntax error at the operator, not a runtime failure.
fn is_incdec_target(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Var(_) | Expr::Index(..) | Expr::PropGet(..) | Expr::StaticProp(..)
    )
}

/// Map a `(type)` cast keyword to the conversion function it desugars to.
/// `(array)` and `(object)` have no PHP-callable equivalent, so they lower to
/// the internal `__cast_array`/`__cast_object` builtins. `(unset)` was removed
/// in PHP 8 → `None`.
fn cast_fn(t: &str) -> Option<&'static str> {
    match t.to_ascii_lowercase().as_str() {
        "int" | "integer" => Some("intval"),
        "float" | "double" | "real" => Some("floatval"),
        "string" => Some("strval"),
        "bool" | "boolean" => Some("boolval"),
        "array" => Some("__cast_array"),
        "object" => Some("__cast_object"),
        _ => None,
    }
}

/// Parse a PHP source string into a statement list. Inline `rust { ... }` FFI
/// blocks are desugared to `__rust_compile(...)` calls before lexing.
pub fn parse(src: &str) -> Result<Vec<Stmt>, String> {
    let src = crate::rust_ffi::desugar(src);
    lexer::clear_diags();
    let toks = lexer::lex(&src)?;
    let mut p = Parser {
        toks,
        pos: 0,
        pending_attrs: Vec::new(),
    };
    let mut stmts = Vec::new();
    while !p.at_end() {
        stmts.push(p.statement()?);
    }
    Ok(stmts)
}

/// Turn the lexer's string segments into parsed ones: a bare `$name` becomes the
/// expression that reads it, and a [`StrPart::Raw`] — `{$expr}`, `$a->p`, `$a[k]`
/// — is lexed and parsed on its own, since each is a self-contained PHP
/// expression. Anything left over after it means the recorded source was not one.
fn resolve_interp_parts(parts: Vec<StrPart>) -> Result<Vec<InterpPart>, String> {
    parts
        .into_iter()
        .map(|p| match p {
            StrPart::Lit(s) => Ok(InterpPart::Lit(s)),
            StrPart::Var(n) => Ok(InterpPart::Expr(Box::new(Expr::Var(n)))),
            StrPart::Raw(src) => {
                let toks = lexer::lex(&format!("<?php {src};"))?;
                let mut inner = Parser {
                    toks,
                    pos: 0,
                    pending_attrs: Vec::new(),
                };
                let e = inner.expression()?;
                if !inner.at_punct(";") {
                    return Err(format!("unparsed interpolation `{{{src}}}`"));
                }
                Ok(InterpPart::Expr(Box::new(e)))
            }
        })
        .collect()
}

struct Parser {
    toks: Vec<Spanned>,
    pos: usize,
    /// Attribute names read at the head of the statement being parsed, waiting
    /// for the declaration that follows to claim them. Cleared by whoever takes
    /// them, so a `#[Attr] class A {} class B {}` cannot leak onto `B`.
    pending_attrs: Vec<String>,
}

impl Parser {
    // ── cursor ─────────────────────────────────────────────────────────────

    fn at_end(&self) -> bool {
        self.pos >= self.toks.len()
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|s| &s.tok)
    }

    /// The line of the token at the cursor. Past the end there is no such token,
    /// so the last one's line stands in — PHP reports `unexpected end of file`
    /// against the final line of the source, not line zero.
    fn line(&self) -> u32 {
        self.line_at(self.pos)
    }

    /// The line of the token at `idx`, or — past the end, where there is none —
    /// the last token's, since PHP reports `unexpected end of file` against the
    /// final line of the source rather than line zero.
    fn line_at(&self, idx: usize) -> u32 {
        self.toks
            .get(idx)
            .or_else(|| self.toks.last())
            .map(|s| s.line)
            .unwrap_or(1)
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
            Err(self.syntax_error())
        }
    }

    // ── syntax errors ──────────────────────────────────────────────────────

    /// PHP's `syntax error, …` diagnostic for the token at the cursor, in the
    /// display form the CLI prints (the `Parse error: ` severity and the leading
    /// blank line are added by [`crate::host::PhpHost::fatal`]).
    ///
    /// DIVERGENCE: the reference message often carries a `, expecting "X" or "Y"`
    /// suffix listing the tokens its LALR state would have accepted. That set is a
    /// property of PHP's generated parser tables, not of the grammar as written
    /// here, so it is omitted rather than guessed at — an invented list would be
    /// wrong more often than no list.
    fn syntax_error(&self) -> String {
        self.syntax_error_at(self.pos)
    }

    /// [`syntax_error`](Self::syntax_error) for a token the caller has already
    /// consumed — a `match self.next()` arm reports `self.pos - 1`, the token it
    /// just took, not the one after it.
    fn syntax_error_at(&self, idx: usize) -> String {
        crate::host::with_host(|h| {
            format!(
                "syntax error, unexpected {} in {} on line {}",
                self.tok_desc_at(idx),
                h.script_name(),
                self.line_at(idx)
            )
        })
    }

    /// How PHP names the token at the cursor inside a syntax error. Literals are
    /// echoed back in their source spelling and named by kind; a reserved word and
    /// every operator or delimiter are quoted as a bare `token "…"`.
    fn tok_desc_at(&self, idx: usize) -> String {
        let Some(sp) = self.toks.get(idx) else {
            return "end of file".to_string();
        };
        let raw = |fallback: String| -> String {
            sp.raw.as_ref().map(|r| r.to_string()).unwrap_or(fallback)
        };
        match &sp.tok {
            Tok::Int(n) => format!("integer \"{}\"", raw(n.to_string())),
            Tok::Float(f) => format!("floating-point number \"{}\"", raw(f.to_string())),
            Tok::Str(s) => format!("single-quoted string \"{s}\""),
            Tok::Interp(parts) => {
                // A double-quoted string with no interpolation is reported with
                // its text; anything with an embedded expression has no single
                // spelling, so only the kind is named.
                match parts.as_slice() {
                    [StrPart::Lit(s)] => format!("double-quoted string \"{s}\""),
                    _ => "double-quoted string".to_string(),
                }
            }
            Tok::Var(v) => format!("variable \"${v}\""),
            Tok::Ident(id) => match reserved_spelling(id) {
                Some(kw) => format!("token \"{kw}\""),
                None => format!("identifier \"{id}\""),
            },
            Tok::Punct(p) => format!("token \"{p}\""),
            Tok::InlineHtml(_) => "inline HTML".to_string(),
            Tok::OpenEcho => "token \"<?=\"".to_string(),
        }
    }

    /// The optional numeric level of a `break`/`continue` (`break 2;`),
    /// defaulting to 1. PHP only accepts a literal integer here.
    fn break_level(&mut self) -> u32 {
        if let Some(Tok::Int(n)) = self.peek() {
            let n = *n;
            self.pos += 1;
            return (n.max(1)) as u32;
        }
        1
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
        // A declaration statement may be preceded by attribute groups. They are
        // read here and handed to the declaration that follows through
        // `pending_attrs`, since only that declaration can interpret them.
        self.pending_attrs = self.attributes()?;
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
            _ if self.at_kw("namespace") => self.namespace_stmt()?,
            _ if self.at_kw("use") => self.use_import_stmt()?,
            _ if self.at_kw("if") => self.if_stmt()?,
            _ if self.at_kw("while") => self.while_stmt()?,
            _ if self.at_kw("do") => self.do_while_stmt()?,
            _ if self.at_kw("switch") => self.switch_stmt()?,
            _ if self.at_kw("for") => self.for_stmt()?,
            _ if self.at_kw("foreach") => self.foreach_stmt()?,
            _ if self.at_kw("function") => self.function_stmt()?,
            // `static $a = 1, $b;` — a static local declaration, distinguished
            // from a `static::` late-static-binding expression or a `static
            // function`/`static fn` closure by the `$variable` that follows.
            _ if self.at_kw("static")
                && matches!(
                    self.toks.get(self.pos + 1).map(|s| &s.tok),
                    Some(Tok::Var(_))
                ) =>
            {
                self.pos += 1; // static
                let mut decls = Vec::new();
                loop {
                    let name = self.expect_var()?;
                    let default = if self.eat_punct("=") {
                        Some(self.expression()?)
                    } else {
                        None
                    };
                    decls.push((name, default));
                    if !self.eat_punct(",") {
                        break;
                    }
                }
                self.expect_punct(";")?;
                StmtKind::StaticLocal(decls)
            }
            _ if self.at_kw("class")
                || self.at_kw("interface")
                || self.at_kw("trait")
                || ((self.at_kw("abstract") || self.at_kw("final"))
                    && matches!(self.toks.get(self.pos + 1).map(|s| &s.tok),
                        Some(Tok::Ident(s)) if s.eq_ignore_ascii_case("class"))) =>
            {
                self.class_stmt()?
            }
            // `enum Name [: type] { ... }`. `enum` is not a reserved keyword, so it
            // is only treated as an enum declaration when followed by a name and a
            // `{`, `:`, or `implements` — otherwise it stays a plain identifier.
            _ if self.at_kw("enum")
                && matches!(
                    self.toks.get(self.pos + 1).map(|s| &s.tok),
                    Some(Tok::Ident(_))
                )
                && (self.nth_is_punct(2, "{")
                    || self.nth_is_punct(2, ":")
                    || matches!(self.toks.get(self.pos + 2).map(|s| &s.tok),
                        Some(Tok::Ident(s)) if s.eq_ignore_ascii_case("implements"))) =>
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
                let level = self.break_level();
                self.expect_punct(";")?;
                StmtKind::Break(level)
            }
            _ if self.at_kw("continue") => {
                self.pos += 1;
                let level = self.break_level();
                self.expect_punct(";")?;
                StmtKind::Continue(level)
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
            return Err(self.syntax_error());
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
                return Err(self.syntax_error());
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
            return Err(self.syntax_error());
        }
        // A `&` before the value var marks by-reference iteration (writes back).
        let ref1 = self.eat_punct("&");
        let first = self.foreach_target()?;
        let (key_var, val, by_ref) = if self.eat_punct("=>") {
            // The key is always a plain variable. PHP rejects a pattern there
            // ("Cannot use list as key element"), so `[$a] => $v` is an error
            // rather than a second destructuring site.
            let ForeachVal::Var(k) = first else {
                return Err(self.syntax_error_at(self.pos - 1));
            };
            let ref2 = self.eat_punct("&");
            (Some(k), self.foreach_target()?, ref2)
        } else {
            (None, first, ref1)
        };
        // `foreach ($a as &[$x, $y])` is a parse error in PHP: a pattern is not
        // a place a reference can bind to. (An inner `[&$x, $y]` is a different
        // construct and is not accepted here either — see `array_literal`.)
        if by_ref && matches!(val, ForeachVal::Pattern(_)) {
            return Err(self.syntax_error_at(self.pos - 1));
        }
        self.expect_punct(")")?;
        let body = self.body()?;
        Ok(StmtKind::Foreach {
            arr,
            key_var,
            val,
            by_ref,
            body,
        })
    }

    /// A `foreach` value target: a plain `$v`, or a destructuring pattern in
    /// either spelling — `[$x, $y]` and `list($x, $y)` produce the same
    /// `Expr::Array`, which is also what `[$x, $y] = …` produces, so the two
    /// forms cannot drift apart.
    fn foreach_target(&mut self) -> Result<ForeachVal, String> {
        if self.eat_punct("[") {
            return Ok(ForeachVal::Pattern(self.array_literal("]")?));
        }
        if self.at_kw("list") && self.nth_is_punct(1, "(") {
            self.pos += 2; // `list` `(`
            return Ok(ForeachVal::Pattern(self.array_literal(")")?));
        }
        Ok(ForeachVal::Var(self.expect_var()?))
    }

    /// A property / method / constant name after `->` or `::` (a bare identifier).
    fn member_name(&mut self) -> Result<String, String> {
        match self.next() {
            Some(Tok::Ident(n)) => Ok(n),
            _ => Err(self.syntax_error_at(self.pos - 1)),
        }
    }

    fn expect_var(&mut self) -> Result<String, String> {
        match self.next() {
            Some(Tok::Var(n)) => Ok(n),
            _ => Err(self.syntax_error_at(self.pos - 1)),
        }
    }

    fn function_stmt(&mut self) -> Result<StmtKind, String> {
        self.pos += 1; // function
        let by_ref_return = self.eat_punct("&");
        let name = match self.next() {
            Some(Tok::Ident(n)) => n,
            _ => return Err(self.syntax_error_at(self.pos - 1)),
        };
        let params = self.param_list()?;
        self.skip_return_type();
        let body = self.block()?;
        Ok(StmtKind::Function {
            name,
            params,
            body,
            by_ref_return,
        })
    }

    /// `namespace Name;` or `namespace Name { ... }`. phplang uses a flat
    /// namespace (qualified CLASS names fold to their last segment), so the
    /// declaration is accepted and the name discarded; a block form runs its
    /// body inline.
    ///
    /// Constants do not fold — see `primary`, where `Foo\BAR` resolves as the
    /// constant of that exact name.
    fn namespace_stmt(&mut self) -> Result<StmtKind, String> {
        self.pos += 1; // namespace
        if matches!(self.peek(), Some(Tok::Ident(_))) || self.at_punct("\\") {
            let _ = self.expect_type_name()?;
        }
        if self.at_punct("{") {
            Ok(StmtKind::Block(self.block()?))
        } else {
            self.expect_punct(";")?;
            Ok(StmtKind::Block(Vec::new()))
        }
    }

    /// `use A\B\C [as D];` (also `use function …` / `use const …`) — a namespace
    /// import. In the flat-namespace model the short name already resolves, so the
    /// import is accepted and discarded (aliases via `as` are not remapped).
    fn use_import_stmt(&mut self) -> Result<StmtKind, String> {
        self.pos += 1; // use
        self.eat_kw("function");
        self.eat_kw("const");
        loop {
            let _ = self.expect_type_name()?;
            if self.eat_kw("as") {
                self.next(); // alias identifier — ignored
            }
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_punct(";")?;
        Ok(StmtKind::Block(Vec::new()))
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
            return Err(self.syntax_error());
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
            _ => return Err(self.syntax_error_at(self.pos - 1)),
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
    /// Consume any run of `#[Attr, Other(args)]` groups and return the attribute
    /// names in source order, unqualified and with any leading `\` dropped.
    ///
    /// Attributes are declarative metadata: nothing here executes, and only their
    /// NAMES are kept, because the one attribute this engine acts on
    /// (`#[AllowDynamicProperties]`) takes no arguments. The argument list is
    /// scanned for balance and discarded, so an attribute whose arguments contain
    /// brackets or a nested `#[…]` still ends at the right `]`.
    ///
    /// Every declaration form may carry attributes, so this is called at the head
    /// of statements, class members, and parameters; consuming them is what keeps
    /// an attributed declaration parsing at all.
    fn attributes(&mut self) -> Result<Vec<String>, String> {
        let mut names: Vec<String> = Vec::new();
        while self.at_punct("#[") {
            self.pos += 1;
            let mut depth = 1usize;
            // Depth 1 is the attribute list itself: a name there is an attribute,
            // a name nested inside an argument list is not.
            let mut expect_name = true;
            while depth > 0 {
                match self.next() {
                    None => return Err("unterminated attribute".to_string()),
                    Some(Tok::Punct("#[")) | Some(Tok::Punct("[")) | Some(Tok::Punct("(")) => {
                        depth += 1
                    }
                    Some(Tok::Punct("]")) | Some(Tok::Punct(")")) => depth -= 1,
                    Some(Tok::Punct(",")) if depth == 1 => expect_name = true,
                    // A namespace separator. The QUALIFICATION is kept, unlike
                    // class names elsewhere in this engine: attribute names are
                    // matched exactly, so `#[Foo\AllowDynamicProperties]` is a
                    // user attribute and must NOT be mistaken for the engine's
                    // global `#[AllowDynamicProperties]`. A leading `\` is not
                    // qualification — it only anchors the global namespace.
                    Some(Tok::Punct("\\")) if depth == 1 => {
                        if let Some(last) = names.last_mut() {
                            if !expect_name {
                                last.push('\\');
                            }
                        }
                        expect_name = true;
                    }
                    Some(Tok::Ident(n)) if depth == 1 && expect_name => {
                        match names.last_mut() {
                            Some(last) if last.ends_with('\\') => last.push_str(&n),
                            _ => names.push(n),
                        }
                        expect_name = false;
                    }
                    _ => {}
                }
            }
        }
        Ok(names)
    }

    fn param_list(&mut self) -> Result<Vec<Param>, String> {
        self.expect_punct("(")?;
        let mut params = Vec::new();
        if !self.at_punct(")") {
            loop {
                // A parameter may be attributed (`function f(#[Attr] int $x)`).
                self.attributes()?;
                // Skip a leading modifier/type-hint chain up to `...` or `$var`. A
                // visibility/readonly keyword marks a promoted constructor property.
                let mut promoted = false;
                let mut by_ref = false;
                loop {
                    if self.eat_punct("&") {
                        by_ref = true;
                        continue;
                    }
                    if self.eat_punct("?") {
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
                    by_ref,
                });
                // PHP 8.0+ allows a trailing comma in parameter lists too.
                if !self.eat_punct(",") || self.at_punct(")") {
                    break;
                }
            }
        }
        self.expect_punct(")")?;
        Ok(params)
    }

    /// Parse a call argument list up to and consuming the closing `)` (the opening
    /// `(` is already eaten). Supports `...$arr` argument unpacking and PHP 8.0
    /// named arguments (`name: value`).
    fn arg_list(&mut self) -> Result<Vec<Expr>, String> {
        let mut args = Vec::new();
        if !self.at_punct(")") {
            loop {
                if self.eat_punct("...") {
                    args.push(Expr::Spread(Box::new(self.expression()?)));
                } else if let Some(name) = self.named_arg_label() {
                    args.push(Expr::NamedArg(name, Box::new(self.expression()?)));
                } else {
                    args.push(self.expression()?);
                }
                // PHP 7.3+ allows a trailing comma before the `)`.
                if !self.eat_punct(",") || self.at_punct(")") {
                    break;
                }
            }
        }
        self.expect_punct(")")?;
        Ok(args)
    }

    /// A named-argument label `name:` — an identifier immediately followed by a
    /// single `:` (a distinct token from `::`). Consumes `name :` and returns the
    /// name; leaves the cursor untouched and returns `None` otherwise.
    fn named_arg_label(&mut self) -> Option<String> {
        if let Some(Tok::Ident(n)) = self.peek() {
            if self.nth_is_punct(1, ":") {
                let n = n.clone();
                self.pos += 2; // identifier + ':'
                return Some(n);
            }
        }
        None
    }

    /// First-class callable syntax `callee(...)` (PHP 8.1). Called just after a
    /// call's opening `(` is consumed: if the argument list is exactly `...`, this
    /// consumes `... )` and returns the desugared closure
    /// `fn(...$args) => call_user_func_array(<callable>, $args)` — a real
    /// `Closure`. Returns `None` when it is not the first-class-callable form (the
    /// caller then parses a normal argument list).
    fn try_fcc(&mut self, callable: Expr) -> Result<Option<Expr>, String> {
        if !(self.at_punct("...") && self.nth_is_punct(1, ")")) {
            return Ok(None);
        }
        self.pos += 2; // consume `...` and `)`
        let param = Param {
            name: "args".to_string(),
            default: None,
            variadic: true,
            promoted: false,
            by_ref: false,
        };
        let body = Expr::Call(
            "call_user_func_array".to_string(),
            vec![callable, Expr::Var("args".to_string())],
        );
        Ok(Some(Expr::ArrowFn {
            params: vec![param],
            body: Box::new(body),
        }))
    }

    /// `class Name [extends Parent] [implements ...] { members }`. Members are
    /// consts, properties (with visibility/static/type modifiers), and methods.
    /// A leading `abstract`/`final` class modifier is accepted but not enforced.
    fn class_stmt(&mut self) -> Result<StmtKind, String> {
        let attributes = std::mem::take(&mut self.pending_attrs);
        let is_interface = self.at_kw("interface");
        let is_trait = self.at_kw("trait");
        let is_enum = self.at_kw("enum");
        // A leading `abstract` marks the class un-instantiable; `final` is accepted
        // and ignored.
        let is_abstract = self.at_kw("abstract");
        if !self.at_kw("class") && !is_interface && !is_trait && !is_enum {
            self.pos += 1; // abstract / final
        }
        self.pos += 1; // class / interface / trait / enum
        let name = match self.next() {
            Some(Tok::Ident(n)) => n,
            _ => return Err(self.syntax_error_at(self.pos - 1)),
        };
        // A backed enum names its scalar backing type after `:` (`enum E: string`).
        let mut enum_backing = None;
        if is_enum && self.eat_punct(":") {
            enum_backing = Some(self.expect_type_name()?);
        }
        let decl = self.class_rest(name, is_interface, is_enum, is_abstract, enum_backing)?;
        Ok(StmtKind::Class(ClassDecl { attributes, ..decl }))
    }

    /// Everything of a class declaration after its name: the `extends` /
    /// `implements` clauses and the `{ members }` body.
    ///
    /// Split out of [`Parser::class_stmt`] because an anonymous class
    /// (`new class(args) extends P implements I { … }`) is exactly this tail
    /// with a generated name, so both forms must parse the same body grammar.
    /// The returned declaration carries no attributes; the caller attaches them.
    fn class_rest(
        &mut self,
        name: String,
        is_interface: bool,
        is_enum: bool,
        is_abstract: bool,
        enum_backing: Option<String>,
    ) -> Result<ClassDecl, String> {
        let mut parent = None;
        let mut implements = Vec::new();
        // `extends`: one parent for a class; an interface may extend several.
        if self.eat_kw("extends") {
            loop {
                let n = self.expect_type_name()?;
                if is_interface {
                    implements.push(n);
                    if !self.eat_punct(",") {
                        break;
                    }
                } else {
                    parent = Some(n);
                    break;
                }
            }
        }
        // `implements Iface, ...`.
        if self.eat_kw("implements") {
            loop {
                implements.push(self.expect_type_name()?);
                if !self.eat_punct(",") {
                    break;
                }
            }
        }
        self.expect_punct("{")?;
        let mut consts = Vec::new();
        let mut props = Vec::new();
        let mut methods = Vec::new();
        let mut uses = Vec::new();
        let mut trait_insteadof = Vec::new();
        let mut trait_aliases = Vec::new();
        let mut cases = Vec::new();
        while !self.at_punct("}") && !self.at_end() {
            // Any member — const, property, method, enum case — may be attributed.
            self.attributes()?;
            // `case Name [= value];` — an enum case (only meaningful inside `enum`).
            if is_enum && self.at_kw("case") {
                self.pos += 1;
                let cname = match self.next() {
                    Some(Tok::Ident(n)) => n,
                    _ => return Err(self.syntax_error_at(self.pos - 1)),
                };
                let value = if self.eat_punct("=") {
                    Some(self.expression()?)
                } else {
                    None
                };
                self.expect_punct(";")?;
                cases.push(EnumCase { name: cname, value });
                continue;
            }
            // `use Trait1, Trait2;` — pull trait members into this class.
            if self.at_kw("use") {
                self.pos += 1;
                loop {
                    uses.push(self.expect_type_name()?);
                    if !self.eat_punct(",") {
                        break;
                    }
                }
                // A `{ ... }` adaptation block resolves conflicts between the
                // used traits (`insteadof`) and binds extra names or
                // visibilities (`as`); otherwise a plain `;` terminates the use.
                if self.at_punct("{") {
                    self.trait_adaptations(&mut trait_insteadof, &mut trait_aliases)?;
                } else {
                    self.expect_punct(";")?;
                }
                continue;
            }
            // Member modifiers: `static` and the visibility keyword are captured;
            // `abstract`/`final`/`readonly`/`var` are accepted and ignored.
            let mut is_static = false;
            let mut visibility = Visibility::Public;
            loop {
                if self.eat_kw("static") {
                    is_static = true;
                } else if self.eat_kw("public") {
                    visibility = Visibility::Public;
                } else if self.eat_kw("protected") {
                    visibility = Visibility::Protected;
                } else if self.eat_kw("private") {
                    visibility = Visibility::Private;
                } else if self.at_kw("abstract")
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
                        _ => return Err(self.syntax_error_at(self.pos - 1)),
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
                let by_ref_return = self.eat_punct("&"); // return-by-ref marker
                let mname = match self.next() {
                    Some(Tok::Ident(n)) => n,
                    _ => return Err(self.syntax_error_at(self.pos - 1)),
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
                    visibility,
                    by_ref_return,
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
                    props.push(PropDecl {
                        name: pname,
                        default,
                        is_static,
                        visibility,
                    });
                    if !self.eat_punct(",") {
                        break;
                    }
                }
                self.expect_punct(";")?;
            }
        }
        self.expect_punct("}")?;
        // A backed enum implements `BackedEnum` (which extends `UnitEnum`); a pure
        // enum implements `UnitEnum`. Added so `instanceof UnitEnum`/`BackedEnum`
        // holds without the user declaring it.
        if is_enum {
            implements.push("UnitEnum".to_string());
            if enum_backing.is_some() {
                implements.push("BackedEnum".to_string());
            }
        }
        Ok(ClassDecl {
            name,
            parent,
            implements,
            uses,
            trait_insteadof,
            trait_aliases,
            is_interface,
            is_abstract,
            is_enum,
            enum_backing,
            cases,
            consts,
            props,
            methods,
            attributes: Vec::new(),
        })
    }

    /// The `{ … }` adaptation block of a `use Trait…` inside a class body:
    ///
    /// ```text
    /// A::m insteadof B, C;      // keep A's m, drop B's and C's
    /// A::m as alias;            // bind A's m under a second name
    /// A::m as protected alias;  // …with a visibility of its own
    /// m as protected;           // change the visibility of the existing m
    /// ```
    ///
    /// The left-hand side may be qualified (`A::m`) or bare (`m`); a bare name
    /// is only legal for `as`, and only when exactly one used trait declares it
    /// — a check the compiler makes, because only it knows the traits' members.
    fn trait_adaptations(
        &mut self,
        insteadof: &mut Vec<TraitInsteadof>,
        aliases: &mut Vec<TraitAlias>,
    ) -> Result<(), String> {
        self.expect_punct("{")?;
        while !self.at_punct("}") && !self.at_end() {
            let first = self.expect_type_name()?;
            // `A::m` qualifies the method with its trait; a bare `m` does not.
            let (from, method) = if self.eat_punct("::") {
                let m = match self.next() {
                    Some(Tok::Ident(n)) => n,
                    _ => return Err(self.syntax_error_at(self.pos - 1)),
                };
                (Some(first), m)
            } else {
                (None, first)
            };
            if self.eat_kw("insteadof") {
                let mut losers = Vec::new();
                loop {
                    losers.push(self.expect_type_name()?);
                    if !self.eat_punct(",") {
                        break;
                    }
                }
                // `insteadof` names which trait wins, so the left side must say
                // which trait it is talking about.
                let Some(winner) = from else {
                    return Err(self.syntax_error_at(self.pos - 1));
                };
                insteadof.push(TraitInsteadof {
                    winner,
                    method,
                    losers,
                });
            } else if self.eat_kw("as") {
                let visibility = if self.eat_kw("public") {
                    Some(Visibility::Public)
                } else if self.eat_kw("protected") {
                    Some(Visibility::Protected)
                } else if self.eat_kw("private") {
                    Some(Visibility::Private)
                } else {
                    None
                };
                // The new name is optional: `m as protected;` only re-marks the
                // visibility of the binding the class already has.
                let alias = match self.peek() {
                    Some(Tok::Ident(n)) => {
                        let n = n.clone();
                        self.pos += 1;
                        Some(n)
                    }
                    _ => None,
                };
                if visibility.is_none() && alias.is_none() {
                    return Err(self.syntax_error_at(self.pos));
                }
                aliases.push(TraitAlias {
                    from,
                    method,
                    alias,
                    visibility,
                });
            } else {
                return Err(self.syntax_error_at(self.pos));
            }
            self.expect_punct(";")?;
        }
        self.expect_punct("}")?;
        Ok(())
    }

    // ── expressions (precedence climbing) ──────────────────────────────────

    fn expression(&mut self) -> Result<Expr, String> {
        self.assignment()
    }

    fn assignment(&mut self) -> Result<Expr, String> {
        // `yield` binds looser than assignment, so it surfaces here (a statement
        // `yield $v;` and an assignment RHS `$x = yield $v` both route through
        // `assignment()`). `yield from EXPR` delegates; `yield K => V` carries a key;
        // a bare `yield` (followed by a terminator) yields null.
        if self.at_kw("yield") {
            self.pos += 1;
            if self.eat_kw("from") {
                let src = self.ternary()?;
                return Ok(Expr::YieldFrom(Box::new(src)));
            }
            // A bare `yield` with no operand (next token ends the expression).
            if matches!(
                self.peek(),
                None | Some(Tok::Punct(";"))
                    | Some(Tok::Punct(")"))
                    | Some(Tok::Punct("]"))
                    | Some(Tok::Punct(","))
            ) {
                return Ok(Expr::Yield {
                    key: None,
                    value: None,
                });
            }
            let first = self.ternary()?;
            if self.eat_punct("=>") {
                let val = self.ternary()?;
                return Ok(Expr::Yield {
                    key: Some(Box::new(first)),
                    value: Some(Box::new(val)),
                });
            }
            return Ok(Expr::Yield {
                key: None,
                value: Some(Box::new(first)),
            });
        }
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
            // `$b = &$a` — a reference binding rather than a value copy.
            if compound.is_none() && self.at_punct("&") {
                self.pos += 1;
                let rhs = self.assignment()?;
                return Ok(Expr::RefAssign(Box::new(lhs), Box::new(rhs)));
            }
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
        // `@expr` — the error-suppression operator. Dynamic: it silences every
        // diagnostic raised while the operand runs, including the ones raised
        // from inside the library functions it calls, which have no opcode of
        // their own to quieten.
        if self.eat_punct("@") {
            return Ok(Expr::Suppress(Box::new(self.unary()?)));
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
            return self.incdec_prefix(true);
        }
        if self.eat_punct("--") {
            return self.incdec_prefix(false);
        }
        let e = self.power()?;
        // `$x instanceof ClassName` (bareword class, optionally `\`-qualified).
        if self.eat_kw("instanceof") {
            let cls = self.expect_type_name()?;
            return Ok(Expr::InstanceOf(Box::new(e), cls));
        }
        Ok(e)
    }

    /// A prefix `++`/`--`, whose operand PHP's grammar requires to be a
    /// *variable* — so a non-variable operand is a syntax error at parse time,
    /// not a failure further down the pipeline.
    ///
    /// Where it is caught depends on the operand, and this follows the reference:
    /// a bare number can never begin a variable, so the number itself is the
    /// unexpected token; anything dereferencable (a string, an array literal, a
    /// parenthesised expression) parses first and only then fails, at the token
    /// standing where a `->` or `[` would have had to be.
    fn incdec_prefix(&mut self, inc: bool) -> Result<Expr, String> {
        if matches!(self.peek(), Some(Tok::Int(_) | Tok::Float(_))) {
            return Err(self.syntax_error());
        }
        let target = self.unary()?;
        if !is_incdec_target(&target) {
            return Err(self.syntax_error());
        }
        Ok(Expr::IncDec {
            target: Box::new(target),
            inc,
            prefix: true,
        })
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
                    if let Some(fcc) = self.try_fcc(Expr::Array(vec![
                        (None, e.clone()),
                        (None, Expr::Str(member.clone())),
                    ]))? {
                        e = fcc;
                    } else {
                        e = Expr::MethodCall(Box::new(e), member, self.arg_list()?);
                    }
                } else {
                    e = Expr::PropGet(Box::new(e), member);
                }
            } else if self.eat_punct("?->") {
                // Nullsafe member: `$o?->prop` or `$o?->method(args)`.
                let member = self.member_name()?;
                if self.eat_punct("(") {
                    e = Expr::NullsafeMethodCall(Box::new(e), member, self.arg_list()?);
                } else {
                    e = Expr::NullsafePropGet(Box::new(e), member);
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
                // `Class::$prop` — a static property (the `::` is followed by a
                // `$variable`, not a bareword constant/method name).
                if let Some(Tok::Var(_)) = self.peek() {
                    let prop = self.expect_var()?;
                    e = Expr::StaticProp(class, prop);
                } else {
                    let member = self.member_name()?;
                    if self.eat_punct("(") {
                        if let Some(fcc) = self.try_fcc(Expr::Str(format!("{class}::{member}")))? {
                            e = fcc;
                        } else {
                            e = Expr::StaticCall(class, member, self.arg_list()?);
                        }
                    } else {
                        e = Expr::StaticGet(class, member);
                    }
                }
            } else if self.at_punct("(") {
                // A `( args )` applied to any primary value is a dynamic call: a
                // closure held in `$f`, or an immediately-invoked `foo()(…)` /
                // `(expr)(…)`. A bareword `name(` is already consumed as
                // `Expr::Call` in `primary`, so this only fires on a value callee.
                self.pos += 1;
                if let Some(fcc) = self.try_fcc(e.clone())? {
                    e = fcc;
                } else {
                    let args = self.arg_list()?;
                    e = Expr::CallValue(Box::new(e), args);
                }
            } else if self.at_punct("++") || self.at_punct("--") {
                // Postfix `++`/`--` also takes a *variable*. Unlike the prefix
                // form the operand is already parsed, so the operator itself is
                // the token PHP reports as unexpected.
                if !is_incdec_target(&e) {
                    return Err(self.syntax_error());
                }
                let inc = self.at_punct("++");
                self.pos += 1;
                e = Expr::IncDec {
                    target: Box::new(e),
                    inc,
                    prefix: false,
                };
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn primary(&mut self) -> Result<Expr, String> {
        // A leading `\` is the global-namespace prefix. phplang has no namespaces,
        // so `\Exception` / `\strlen(…)` are the same as the bare name — skip it.
        self.eat_punct("\\");
        match self.next() {
            Some(Tok::Int(n)) => Ok(Expr::Int(n)),
            Some(Tok::Float(f)) => Ok(Expr::Float(f)),
            Some(Tok::Str(s)) => Ok(Expr::Str(s)),
            Some(Tok::Interp(parts)) => Ok(Expr::Interp(resolve_interp_parts(parts)?)),
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
            // `list($a, $b)` / `list('k' => $v)` — a destructuring language
            // construct, not a function call. It is sugar for the `[...]` short
            // form, so both lower to the same `Expr::Array` and share the
            // compiler's assignment-target destructuring path. Only intercepted
            // when followed by `(`, so a bareword `list` still parses as a name.
            Some(Tok::Ident(kw)) if kw.eq_ignore_ascii_case("list") && self.at_punct("(") => {
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
                // `new class …` declares the class inline. Its arguments come
                // BEFORE the `extends`/`implements` clauses, which is the one
                // place the grammar differs from a named declaration.
                if self.at_kw("class") {
                    let line = self.line();
                    self.pos += 1;
                    let args = if self.eat_punct("(") {
                        self.arg_list()?
                    } else {
                        Vec::new()
                    };
                    let decl =
                        self.class_rest("class@anonymous".to_string(), false, false, false, None)?;
                    return Ok(Expr::NewAnon {
                        decl: Box::new(decl),
                        args,
                        line,
                    });
                }
                self.eat_punct("\\"); // optional global-namespace prefix
                let class = match self.next() {
                    Some(Tok::Ident(n)) => n,
                    _ => return Err(self.syntax_error_at(self.pos - 1)),
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
                            // `use (&$v)` captures the enclosing variable itself
                            // rather than its value at creation time.
                            let by_ref = self.eat_punct("&");
                            let name = self.expect_var()?;
                            uses.push(Capture { name, by_ref });
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
                // A `\`-qualified name — `Foo\BAR`, `A\B\C`. Any LEADING `\` was
                // already eaten at the top of `primary`, which is what makes
                // `\NOPE` the constant `NOPE` rather than `\NOPE`.
                //
                // A CONSTANT keeps the whole name: `Foo\BAR` is a constant
                // literally called `Foo\BAR`, a different one from `BAR`, which
                // is exactly what `define('Foo\BAR', …)` creates.
                let mut qualified = name.clone();
                let mut qualified_segments = 0usize;
                while self.at_punct("\\") {
                    let Some(Tok::Ident(seg)) = self.toks.get(self.pos + 1).map(|s| &s.tok) else {
                        break;
                    };
                    let seg = seg.clone();
                    self.pos += 2; // `\` and the segment
                    qualified.push('\\');
                    qualified.push_str(&seg);
                    qualified_segments += 1;
                }
                // A qualified CALL is left as the syntax error it already was.
                // There is no answer here that is not a divergence: the
                // reference resolves a qualified name relative to the current
                // namespace, which this flat model does not track, so
                // `namespace A; A\f()` is `A\A\f` (undefined) upstream. Folding
                // to the last segment would return a value where the reference
                // fatals — trading an error for a silently wrong answer — and
                // keeping the full name would break `\A\f()`, which resolves
                // today. Constants have no such conflict, so only they are
                // resolved here. (A bare leading `\` consumes no segment and is
                // unaffected, so `\strlen(…)` still calls `strlen`.)
                if qualified_segments > 0 && self.at_punct("(") {
                    return Err(self.syntax_error_at(self.pos));
                }
                // A bareword followed by `(` is a function call.
                if self.eat_punct("(") {
                    // `name(...)` — first-class callable syntax → a `Closure`.
                    if let Some(fcc) = self.try_fcc(Expr::Str(name.clone()))? {
                        return Ok(fcc);
                    }
                    let args = self.arg_list()?;
                    // `isset()`/`empty()` are language constructs, not functions:
                    // they must not error on an undefined variable/key. phplang
                    // returns `null` for a missing var/index silently, so both
                    // desugar to plain operators over existing ops.
                    if name.eq_ignore_ascii_case("empty") && args.len() == 1 {
                        // empty($x) ≡ !$x over an isset-gated read of $x.
                        return Ok(Expr::Unary(
                            UnOp::Not,
                            Box::new(Expr::EmptyOf(Box::new(args.into_iter().next().unwrap()))),
                        ));
                    }
                    if name.eq_ignore_ascii_case("isset") && !args.is_empty() {
                        // isset($a, $b, …) ≡ isset($a) && isset($b) && …
                        let mut it = args.into_iter();
                        let mut expr = Expr::IssetOf(Box::new(it.next().unwrap()));
                        for a in it {
                            expr = Expr::Binary(
                                BinOp::And,
                                Box::new(expr),
                                Box::new(Expr::IssetOf(Box::new(a))),
                            );
                        }
                        return Ok(expr);
                    }
                    // `unset($a, $b[$k], …)` — a construct, not a function call.
                    if name.eq_ignore_ascii_case("unset") {
                        return Ok(Expr::Unset(args));
                    }
                    Ok(Expr::Call(name, args))
                } else {
                    // A constant reference, resolved against the constant table
                    // at run time. An undefined name throws
                    // `Error: Undefined constant "<name>"` — PHP 8 behaviour;
                    // the PHP 7 fallback to the bare name as a string is gone.
                    //
                    // The QUALIFIED name is what is looked up, so `Foo\BAR`
                    // reaches that same throw naming `Foo\BAR`, rather than
                    // quietly resolving some other constant.
                    Ok(Expr::ConstFetch(qualified))
                }
            }
            _ => Err(self.syntax_error_at(self.pos - 1)),
        }
    }

    /// Parse array elements up to `close` (already past the opener).
    fn array_literal(&mut self, close: &str) -> Result<Expr, String> {
        let mut elems = Vec::new();
        while !self.at_punct(close) && !self.at_end() {
            // An empty slot — `[, $b]` / `list(, $b)` — is a skipped element in a
            // destructuring target. It still consumes a positional index, so it is
            // recorded as a `Null`-valued element (a hole the compiler skips when
            // this array is used as an assignment LHS).
            if self.at_punct(",") {
                self.next();
                elems.push((None, Expr::Null));
                continue;
            }
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
