//! The PHP lexer.
//!
//! Handles the two lexing modes PHP requires: *inline-HTML* mode (text outside
//! `<?php ... ?>`, emitted as a single `InlineHtml` token) and *PHP* mode (the
//! code between the tags). `<?=` opens PHP mode with an implicit `echo`.

use crate::ast::StrPart;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    InlineHtml(String),
    /// `<?=` short-echo open tag (parser emits an implicit `echo`).
    OpenEcho,
    Var(String),
    Ident(String),
    Int(i64),
    Float(f64),
    /// Single-quoted string — no interpolation.
    Str(String),
    /// Double-quoted / interpolated string.
    Interp(Vec<StrPart>),
    Punct(&'static str),
}

/// A token plus its 1-based source line.
#[derive(Debug, Clone)]
pub struct Spanned {
    pub tok: Tok,
    pub line: u32,
    /// The literal's source spelling, kept only for numbers — `0x1f`, `07`,
    /// `1_000`, `2.0` and `1e3` all lex to a value whose default rendering is not
    /// the text the author wrote, and PHP echoes that text back verbatim in a
    /// syntax error (`unexpected integer "0x1f"`). `None` for every other token,
    /// whose spelling the token itself already carries.
    pub raw: Option<Box<str>>,
}

/// The multi-character operators, longest first so `===` beats `==` beats `=`.
const OPERATORS: &[&str] = &[
    "===", "!==", "<=>", "<<=", ">>=", "**=", "...", "?->", "==", "!=", "<>", "<=", ">=", "&&",
    "||", "++", "--", "->", "=>", "::", "+=", "-=", "*=", "/=", "%=", ".=", "&=", "|=", "^=", "<<",
    ">>", "**", "(", ")", "{", "}", "[", "]", ";", ",", "=", "+", "-", "*", "/", "%", ".", "<",
    ">", "!", "?", ":", "&", "|", "^", "~", "@", "\\",
];

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    line: u32,
    out: Vec<Spanned>,
}

/// Tokenize a PHP source string.
pub fn lex(src: &str) -> Result<Vec<Spanned>, String> {
    let mut lx = Lexer {
        src: src.as_bytes(),
        pos: 0,
        line: 1,
        out: Vec::new(),
    };
    lx.run()?;
    Ok(lx.out)
}

impl<'a> Lexer<'a> {
    fn run(&mut self) -> Result<(), String> {
        loop {
            // Inline-HTML mode: consume verbatim up to the next `<?`.
            let html = self.read_inline_html();
            if !html.is_empty() {
                self.push(Tok::InlineHtml(html));
            }
            if self.pos >= self.src.len() {
                return Ok(());
            }
            // At an opening tag.
            if self.starts_with("<?php") {
                self.advance(5);
            } else if self.starts_with("<?=") {
                self.advance(3);
                self.push(Tok::OpenEcho);
            } else if self.starts_with("<?") {
                self.advance(2);
            }
            // PHP mode until the matching `?>` (or EOF).
            self.lex_php()?;
        }
    }

    /// Consume text until the next `<?` (exclusive) or EOF.
    fn read_inline_html(&mut self) -> String {
        let start = self.pos;
        while self.pos < self.src.len() {
            if self.starts_with("<?") {
                break;
            }
            if self.src[self.pos] == b'\n' {
                self.line += 1;
            }
            self.pos += 1;
        }
        String::from_utf8_lossy(&self.src[start..self.pos]).into_owned()
    }

    fn lex_php(&mut self) -> Result<(), String> {
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            match c {
                b' ' | b'\t' | b'\r' => self.pos += 1,
                b'\n' => {
                    self.line += 1;
                    self.pos += 1;
                }
                _ if self.starts_with("?>") => {
                    self.advance(2);
                    // PHP swallows one newline immediately after a close tag.
                    if self.pos < self.src.len() && self.src[self.pos] == b'\n' {
                        self.line += 1;
                        self.pos += 1;
                    }
                    // A `?>` acts as an implicit statement terminator.
                    self.push(Tok::Punct(";"));
                    return Ok(());
                }
                b'/' if self.peek(1) == Some(b'/') => self.skip_line_comment(),
                b'#' => self.skip_line_comment(),
                b'/' if self.peek(1) == Some(b'*') => self.skip_block_comment()?,
                b'$' => self.lex_variable(),
                b'\'' => self.lex_single_quote()?,
                b'"' => self.lex_double_quote()?,
                b'0'..=b'9' => self.lex_number(),
                b'.' if matches!(self.peek(1), Some(b'0'..=b'9')) => self.lex_number(),
                _ if c == b'_' || c.is_ascii_alphabetic() => self.lex_ident(),
                _ => self.lex_operator()?,
            }
        }
        Ok(())
    }

    fn skip_line_comment(&mut self) {
        while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
            // A `?>` ends a `//`/`#` comment (and PHP mode) even mid-line.
            if self.starts_with("?>") {
                return;
            }
            self.pos += 1;
        }
    }

    fn skip_block_comment(&mut self) -> Result<(), String> {
        self.advance(2);
        while self.pos < self.src.len() {
            if self.starts_with("*/") {
                self.advance(2);
                return Ok(());
            }
            if self.src[self.pos] == b'\n' {
                self.line += 1;
            }
            self.pos += 1;
        }
        Err(format!("unterminated block comment (line {})", self.line))
    }

    fn lex_variable(&mut self) {
        self.pos += 1; // `$`
        let start = self.pos;
        while self.pos < self.src.len() && is_ident(self.src[self.pos]) {
            self.pos += 1;
        }
        let name = String::from_utf8_lossy(&self.src[start..self.pos]).into_owned();
        self.push(Tok::Var(name));
    }

    fn lex_ident(&mut self) {
        let start = self.pos;
        while self.pos < self.src.len() && is_ident(self.src[self.pos]) {
            self.pos += 1;
        }
        let name = String::from_utf8_lossy(&self.src[start..self.pos]).into_owned();
        self.push(Tok::Ident(name));
    }

    fn lex_number(&mut self) {
        let tok_start = self.pos;
        // Radix-prefixed integer literals: `0x`/`0X` hex, `0b`/`0B` binary,
        // `0o`/`0O` octal (PHP 8.1). Underscores are permitted as separators.
        if self.src[self.pos] == b'0' {
            if let Some(radix) = match self.peek(1) {
                Some(b'x') | Some(b'X') => Some(16u32),
                Some(b'b') | Some(b'B') => Some(2),
                Some(b'o') | Some(b'O') => Some(8),
                _ => None,
            } {
                self.advance(2);
                let ds = self.pos;
                while self.pos < self.src.len() {
                    let c = self.src[self.pos];
                    let ok = c == b'_'
                        || match radix {
                            16 => c.is_ascii_hexdigit(),
                            8 => (b'0'..=b'7').contains(&c),
                            _ => c == b'0' || c == b'1',
                        };
                    if ok {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                let digits: String = String::from_utf8_lossy(&self.src[ds..self.pos])
                    .chars()
                    .filter(|c| *c != '_')
                    .collect();
                let raw = String::from_utf8_lossy(&self.src[tok_start..self.pos]).into_owned();
                self.push_num(parse_radix(&digits, radix), &raw);
                return;
            }
        }

        let start = self.pos;
        let mut is_float = false;
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            if c.is_ascii_digit() || c == b'_' {
                self.pos += 1;
            } else if c == b'.' && !is_float && matches!(self.peek(1), Some(b'0'..=b'9')) {
                is_float = true;
                self.pos += 1;
            } else if (c == b'e' || c == b'E')
                && matches!(self.peek(1), Some(b'0'..=b'9' | b'+' | b'-'))
            {
                is_float = true;
                self.pos += 1;
                if matches!(self.src.get(self.pos), Some(b'+' | b'-')) {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
        let src_text = String::from_utf8_lossy(&self.src[start..self.pos]).into_owned();
        let raw: String = src_text.chars().filter(|c| *c != '_').collect();
        if is_float {
            self.push_num(Tok::Float(raw.parse().unwrap_or(0.0)), &src_text);
            return;
        }
        // A leading-zero integer with all-octal digits is an octal literal
        // (`0755`), the classic PHP form; a `0` alone stays decimal zero.
        if raw.len() > 1 && raw.starts_with('0') && raw.bytes().all(|b| (b'0'..=b'7').contains(&b))
        {
            self.push_num(parse_radix(&raw[1..], 8), &src_text);
            return;
        }
        match raw.parse::<i64>() {
            Ok(n) => self.push_num(Tok::Int(n), &src_text),
            // Integer literal that overflows i64 becomes a float, as PHP does.
            Err(_) => self.push_num(Tok::Float(raw.parse().unwrap_or(0.0)), &src_text),
        }
    }

    fn lex_single_quote(&mut self) -> Result<(), String> {
        self.pos += 1; // opening quote
        let mut s = String::new();
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            match c {
                b'\'' => {
                    self.pos += 1;
                    self.push(Tok::Str(s));
                    return Ok(());
                }
                b'\\' if matches!(self.peek(1), Some(b'\'' | b'\\')) => {
                    // In single quotes only \' and \\ are escapes.
                    self.pos += 1;
                    s.push(self.src[self.pos] as char);
                    self.pos += 1;
                }
                b'\n' => {
                    self.line += 1;
                    s.push('\n');
                    self.pos += 1;
                }
                _ => {
                    // Preserve multibyte UTF-8 sequences verbatim (a raw
                    // `c as char` would decode each byte as Latin-1 and mojibake
                    // any non-ASCII text).
                    let ch_len = utf8_len(c);
                    let end = (self.pos + ch_len).min(self.src.len());
                    s.push_str(&String::from_utf8_lossy(&self.src[self.pos..end]));
                    self.pos = end;
                }
            }
        }
        Err(format!("unterminated string (line {})", self.line))
    }

    fn lex_double_quote(&mut self) -> Result<(), String> {
        self.pos += 1; // opening quote
        let mut parts: Vec<StrPart> = Vec::new();
        let mut lit = String::new();
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            match c {
                b'"' => {
                    self.pos += 1;
                    if !lit.is_empty() {
                        parts.push(StrPart::Lit(std::mem::take(&mut lit)));
                    }
                    self.push(Tok::Interp(parts));
                    return Ok(());
                }
                b'\\' => {
                    self.pos += 1;
                    let e = self.src.get(self.pos).copied().unwrap_or(b'\\');
                    // The variable-length escapes consume their own digits and
                    // push their own bytes, so they `continue` past the single
                    // trailing `self.pos += 1` below.
                    match e {
                        b'x' if self.hex_digits_at(self.pos + 1) > 0 => {
                            self.read_hex_escape(&mut lit);
                            continue;
                        }
                        b'u' if self.peek(1) == Some(b'{') => {
                            self.read_unicode_escape(&mut lit);
                            continue;
                        }
                        b'0'..=b'7' => {
                            self.read_octal_escape(&mut lit);
                            continue;
                        }
                        _ => {}
                    }
                    let ch = match e {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'v' => '\x0b',
                        b'f' => '\x0c',
                        b'"' => '"',
                        b'\\' => '\\',
                        b'$' => '$',
                        b'e' => '\x1b',
                        other => {
                            // Unknown escape: PHP keeps the backslash verbatim.
                            lit.push('\\');
                            other as char
                        }
                    };
                    lit.push(ch);
                    self.pos += 1;
                }
                // `{$expr}` — the complex form, any expression. Recorded as source
                // for the parser; a `{` not followed by `$` is ordinary text.
                b'{' if self.peek(1) == Some(b'$') => {
                    if !lit.is_empty() {
                        parts.push(StrPart::Lit(std::mem::take(&mut lit)));
                    }
                    match self.read_braced_interp() {
                        Some(src) => parts.push(StrPart::Raw(src)),
                        // Unbalanced: PHP would not have accepted it either, but
                        // treat the brace as literal rather than lose the rest.
                        None => {
                            lit.push('{');
                            self.pos += 1;
                        }
                    }
                }
                // `${name}` — the pre-8.2 form, deprecated but still substituted.
                b'$' if self.peek(1) == Some(b'{') => {
                    if !lit.is_empty() {
                        parts.push(StrPart::Lit(std::mem::take(&mut lit)));
                    }
                    self.pos += 2; // `${`
                    let start = self.pos;
                    while self.pos < self.src.len() && is_ident(self.src[self.pos]) {
                        self.pos += 1;
                    }
                    let name = String::from_utf8_lossy(&self.src[start..self.pos]).into_owned();
                    if self.src.get(self.pos) == Some(&b'}') {
                        self.pos += 1;
                    }
                    parts.push(StrPart::Var(name));
                }
                b'$' if matches!(self.peek(1), Some(b) if b == b'_' || b.is_ascii_alphabetic()) => {
                    if !lit.is_empty() {
                        parts.push(StrPart::Lit(std::mem::take(&mut lit)));
                    }
                    self.pos += 1; // `$`
                    let start = self.pos;
                    while self.pos < self.src.len() && is_ident(self.src[self.pos]) {
                        self.pos += 1;
                    }
                    let name = String::from_utf8_lossy(&self.src[start..self.pos]).into_owned();
                    // Simple syntax reaches exactly one level deep: `$a->p` and
                    // `$a[k]`, and no further (`"$a->p->q"` interpolates `$a->p`
                    // and leaves `->q` as text).
                    match self.read_simple_interp_suffix(&name) {
                        Some(src) => parts.push(StrPart::Raw(src)),
                        None => parts.push(StrPart::Var(name)),
                    }
                }
                b'\n' => {
                    self.line += 1;
                    lit.push('\n');
                    self.pos += 1;
                }
                _ => {
                    // Preserve UTF-8 multibyte sequences verbatim.
                    let ch_len = utf8_len(c);
                    let end = (self.pos + ch_len).min(self.src.len());
                    lit.push_str(&String::from_utf8_lossy(&self.src[self.pos..end]));
                    self.pos = end;
                }
            }
        }
        Err(format!("unterminated string (line {})", self.line))
    }

    /// Consume a `{$…}` interpolation and return the expression source between
    /// the braces, or `None` if the braces are unbalanced.
    ///
    /// Braces nest (`"{$a[$b['{']]}"`), and a brace inside a quoted string in the
    /// expression is not a delimiter, so both are tracked while scanning.
    fn read_braced_interp(&mut self) -> Option<String> {
        let start = self.pos + 1; // just past `{`
        let mut i = start;
        let mut depth = 1usize;
        let mut quote: Option<u8> = None;
        while i < self.src.len() {
            let c = self.src[i];
            match quote {
                Some(q) => {
                    if c == b'\\' {
                        i += 1;
                    } else if c == q {
                        quote = None;
                    }
                }
                None => match c {
                    b'\'' | b'"' => quote = Some(c),
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            let src = String::from_utf8_lossy(&self.src[start..i]).into_owned();
                            self.line +=
                                self.src[start..i].iter().filter(|&&b| b == b'\n').count() as u32;
                            self.pos = i + 1;
                            return Some(src);
                        }
                    }
                    b'\n' => {}
                    _ => {}
                },
            }
            i += 1;
        }
        None
    }

    /// After a bare `$name` inside a string, consume PHP's *simple* interpolation
    /// suffix if one follows, returning the expression source for it.
    ///
    /// `$a->prop` reads one property. `$a[k]` reads one element, and in this
    /// syntax an unquoted key is a *string* — `"$a[k]"` means `$a['k']`, unlike
    /// the same text outside a string, where `k` would be a constant. A `$var` or
    /// a signed integer key is taken as written.
    fn read_simple_interp_suffix(&mut self, name: &str) -> Option<String> {
        if self.src.get(self.pos) == Some(&b'-') && self.peek(1) == Some(b'>') {
            let after = self.pos + 2;
            if !matches!(self.src.get(after), Some(&b) if b == b'_' || b.is_ascii_alphabetic()) {
                return None;
            }
            let mut i = after;
            while i < self.src.len() && is_ident(self.src[i]) {
                i += 1;
            }
            let prop = String::from_utf8_lossy(&self.src[after..i]).into_owned();
            self.pos = i;
            return Some(format!("${name}->{prop}"));
        }
        if self.src.get(self.pos) != Some(&b'[') {
            return None;
        }
        let key_start = self.pos + 1;
        let mut i = key_start;
        while i < self.src.len() && self.src[i] != b']' {
            i += 1;
        }
        if i >= self.src.len() {
            return None;
        }
        let key = String::from_utf8_lossy(&self.src[key_start..i]).into_owned();
        if key.is_empty() {
            return None;
        }
        let key_expr = if key.starts_with('$')
            || key.parse::<i64>().is_ok()
            || (key.starts_with('\'') && key.ends_with('\''))
            || (key.starts_with('"') && key.ends_with('"'))
        {
            key
        } else {
            format!("'{}'", key.replace('\\', "\\\\").replace('\'', "\\'"))
        };
        self.pos = i + 1;
        Some(format!("${name}[{key_expr}]"))
    }

    fn lex_operator(&mut self) -> Result<(), String> {
        for op in OPERATORS {
            if self.starts_with(op) {
                self.advance(op.len());
                self.push(Tok::Punct(op));
                return Ok(());
            }
        }
        let c = self.src[self.pos] as char;
        Err(format!("unexpected character '{c}' (line {})", self.line))
    }

    // ── helpers ──────────────────────────────────────────────────────────────

    fn push(&mut self, tok: Tok) {
        self.out.push(Spanned {
            tok,
            line: self.line,
            raw: None,
        });
    }

    /// [`push`](Self::push) for a numeric literal, recording the source spelling
    /// alongside the parsed value — see [`Spanned::raw`].
    fn push_num(&mut self, tok: Tok, raw: &str) {
        self.out.push(Spanned {
            tok,
            line: self.line,
            raw: Some(raw.into()),
        });
    }

    fn starts_with(&self, s: &str) -> bool {
        self.src[self.pos..].starts_with(s.as_bytes())
    }

    fn peek(&self, n: usize) -> Option<u8> {
        self.src.get(self.pos + n).copied()
    }

    /// How many hex digits (capped at 2) start at `at` — used to tell a real
    /// `\xHH` escape from a literal `\x` that PHP leaves alone.
    fn hex_digits_at(&self, at: usize) -> usize {
        (0..2)
            .take_while(|i| self.src.get(at + i).is_some_and(|b| b.is_ascii_hexdigit()))
            .count()
    }

    /// `\xHH` — one or two hex digits. `self.pos` is on the `x`.
    ///
    /// The scaffold stores strings as Rust `String`, so a byte at or above 0x80
    /// is appended as that Unicode scalar (its UTF-8 encoding) rather than the
    /// single raw byte PHP would store.
    fn read_hex_escape(&mut self, lit: &mut String) {
        self.pos += 1; // past `x`
        let n = self.hex_digits_at(self.pos);
        let hex: String = self.src[self.pos..self.pos + n]
            .iter()
            .map(|b| *b as char)
            .collect();
        self.pos += n;
        if let Some(c) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
            lit.push(c);
        }
    }

    /// `\u{HHH…}` — a Unicode code point. `self.pos` is on the `u`.
    fn read_unicode_escape(&mut self, lit: &mut String) {
        let close = self.src[self.pos..]
            .iter()
            .position(|b| *b == b'}')
            .map(|i| self.pos + i);
        let Some(close) = close else {
            // No closing brace: not an escape, keep the backslash verbatim.
            lit.push_str("\\u");
            self.pos += 1;
            return;
        };
        let hex: String = self.src[self.pos + 2..close]
            .iter()
            .map(|b| *b as char)
            .collect();
        self.pos = close + 1;
        if let Some(c) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
            lit.push(c);
        }
    }

    /// `\NNN` — one to three octal digits. `self.pos` is on the first digit.
    fn read_octal_escape(&mut self, lit: &mut String) {
        let n = (0..3)
            .take_while(|i| {
                self.src
                    .get(self.pos + i)
                    .is_some_and(|b| (b'0'..=b'7').contains(b))
            })
            .count();
        let oct: String = self.src[self.pos..self.pos + n]
            .iter()
            .map(|b| *b as char)
            .collect();
        self.pos += n;
        // PHP truncates an out-of-range octal escape to a byte.
        if let Some(c) = u32::from_str_radix(&oct, 8)
            .ok()
            .and_then(|v| char::from_u32(v & 0xff))
        {
            lit.push(c);
        }
    }

    fn advance(&mut self, n: usize) {
        for _ in 0..n {
            if self.pos < self.src.len() && self.src[self.pos] == b'\n' {
                self.line += 1;
            }
            self.pos += 1;
        }
    }
}

/// Parse `digits` in the given radix into an integer token, falling back to a
/// float when the value overflows `i64` (as PHP does for large literals).
fn parse_radix(digits: &str, radix: u32) -> Tok {
    match i64::from_str_radix(digits, radix) {
        Ok(n) => Tok::Int(n),
        Err(_) => Tok::Float(
            u128::from_str_radix(digits, radix)
                .map(|u| u as f64)
                .unwrap_or(0.0),
        ),
    }
}

fn is_ident(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphanumeric() || c >= 0x80
}

/// Byte length of a UTF-8 code point given its leading byte.
fn utf8_len(b: u8) -> usize {
    match b {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}
