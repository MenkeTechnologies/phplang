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
                self.push(parse_radix(&digits, radix));
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
        let raw: String = String::from_utf8_lossy(&self.src[start..self.pos])
            .chars()
            .filter(|c| *c != '_')
            .collect();
        if is_float {
            self.push(Tok::Float(raw.parse().unwrap_or(0.0)));
            return;
        }
        // A leading-zero integer with all-octal digits is an octal literal
        // (`0755`), the classic PHP form; a `0` alone stays decimal zero.
        if raw.len() > 1 && raw.starts_with('0') && raw.bytes().all(|b| (b'0'..=b'7').contains(&b))
        {
            self.push(parse_radix(&raw[1..], 8));
            return;
        }
        match raw.parse::<i64>() {
            Ok(n) => self.push(Tok::Int(n)),
            // Integer literal that overflows i64 becomes a float, as PHP does.
            Err(_) => self.push(Tok::Float(raw.parse().unwrap_or(0.0))),
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
                    let ch = match e {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'"' => '"',
                        b'\\' => '\\',
                        b'$' => '$',
                        b'0' => '\0',
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
                    parts.push(StrPart::Var(name));
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
        });
    }

    fn starts_with(&self, s: &str) -> bool {
        self.src[self.pos..].starts_with(s.as_bytes())
    }

    fn peek(&self, n: usize) -> Option<u8> {
        self.src.get(self.pos + n).copied()
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
