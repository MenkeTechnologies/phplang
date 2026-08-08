//! PHP standard-library `json` functions. Part of the `stdlib` chain; see
//! `src/stdlib/mod.rs`. `dispatch` returns `None` for names it does not handle.
//!
//! `json_encode` lives in `builtins.rs` (core); this module supplies the decoder
//! and the error-state accessors. `json_decode` is a hand-written recursive
//! descent parser over the byte stream, matching PHP 8's `ext/json` semantics:
//!
//! * Both JSON objects and arrays decode to PHP arrays. phplang has no
//!   `stdClass`, so the `$associative` flag is effectively always-`true`; it is
//!   accepted and ignored (documented on `json_decode`).
//! * Integral numbers become `int`, everything with a `.`/`e`/`E` (or that
//!   overflows `i64`) becomes `float`, matching PHP's default (no
//!   `JSON_BIGINT_AS_STRING`).
//! * `\uXXXX` escapes and UTF-16 surrogate pairs are decoded to UTF-8.
//! * `$depth` is honored (default 512): exceeding it yields `JSON_ERROR_DEPTH`.
//! * On any failure `json_decode` returns `null` and records the error code,
//!   readable via `json_last_error` / `json_last_error_msg`.
//!
//! LIMITATION: PHP tracks the last json error in a per-request global. phplang
//! has no persistent request/constant table, so the code is kept in a
//! thread-local `Cell` for the life of the thread instead. `json_encode` (core)
//! does not reset it; only `json_decode` writes it.

use crate::host::PhpHost;
use fusevm::Value;
use std::cell::Cell;

// PHP `JSON_ERROR_*` codes (ext/json/php_json.h).
const JSON_ERROR_NONE: i64 = 0;
const JSON_ERROR_DEPTH: i64 = 1;
const JSON_ERROR_CTRL_CHAR: i64 = 3;
const JSON_ERROR_SYNTAX: i64 = 4;
const JSON_ERROR_UTF16: i64 = 10;
/// `JSON_ERROR_INF_OR_NAN` — `json_encode` refuses non-finite floats.
pub const JSON_ERROR_INF_OR_NAN: i64 = 7;

thread_local! {
    /// Last `json_decode` error for this thread (see the module LIMITATION note).
    static LAST_ERROR: Cell<i64> = const { Cell::new(JSON_ERROR_NONE) };
}

pub fn set_last_error(code: i64) {
    LAST_ERROR.with(|c| c.set(code));
}

fn get_last_error() -> i64 {
    LAST_ERROR.with(|c| c.get())
}

/// Human-readable message for a `JSON_ERROR_*` code (ext/json/json.c).
fn error_msg(code: i64) -> &'static str {
    match code {
        JSON_ERROR_NONE => "No error",
        JSON_ERROR_DEPTH => "Maximum stack depth exceeded",
        2 => "State mismatch (invalid or malformed JSON)",
        JSON_ERROR_CTRL_CHAR => "Control character error, possibly incorrectly encoded",
        JSON_ERROR_SYNTAX => "Syntax error",
        5 => "Malformed UTF-8 characters, possibly incorrectly encoded",
        JSON_ERROR_UTF16 => "Single unpaired UTF-16 surrogate in unicode escape",
        JSON_ERROR_INF_OR_NAN => "Inf and NaN cannot be JSON encoded",
        _ => "Unknown error",
    }
}

/// Dispatch a `json`-category PHP function by lowercased name. Returns `None` for
/// names this module does not implement so the stdlib chain can continue.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let v = match name {
        "json_decode" => json_decode(args),
        "json_validate" => json_validate(args),
        "json_last_error" => Value::int(get_last_error()),
        "json_last_error_msg" => Value::str(error_msg(get_last_error()).to_string()),
        _ => return None,
    };
    Some(Ok(v))
}

/// Resolve the `$depth` argument at `idx` (default 512). PHP requires depth > 0
/// (a ValueError otherwise); phplang has no stdlib exceptions, so a non-positive
/// depth is clamped to 1 — the smallest max that admits a single container level.
fn decode_depth(args: &[Value], idx: usize) -> usize {
    match args.get(idx) {
        Some(v) => {
            let d = crate::host::with_host(|h| h.to_number(v).to_int());
            if d < 1 {
                1
            } else {
                d as usize
            }
        }
        None => 512,
    }
}

/// `json_decode($json, $associative = null, $depth = 512, $flags = 0)`.
///
/// LIMITATION: `$associative` is accepted but ignored — phplang has no
/// `stdClass`, so JSON objects always decode to PHP arrays (as if
/// `$associative = true`). `$flags` beyond depth handling are ignored.
fn json_decode(args: &[Value]) -> Value {
    let json = crate::host::with_host(|h| h.to_str(&args.first().cloned().unwrap_or(Value::Undef)));
    // 3rd argument is depth; default 512 (see `decode_depth`).
    let depth = decode_depth(args, 2);

    set_last_error(JSON_ERROR_NONE);
    crate::host::with_host(|h| {
        let mut p = Parser::new(json.as_bytes(), depth, h);
        match p.parse_document() {
            Ok(v) => v,
            Err(code) => {
                set_last_error(code);
                Value::Undef
            }
        }
    })
}

/// `json_validate($json, $depth = 512, $flags = 0)` (PHP 8.3). Returns `true`
/// when `$json` is syntactically valid JSON, `false` otherwise. Like
/// `json_decode` it records the result in `json_last_error` /
/// `json_last_error_msg` (reset to `JSON_ERROR_NONE` on success).
///
/// The `$depth` argument is honored; `$flags` (only `JSON_INVALID_UTF8_IGNORE`
/// in PHP) is accepted and ignored — phplang strings are already valid UTF-8.
///
/// PHP validates without materializing the value; phplang runs the same parser
/// in a non-building mode (`Parser::new_validate`) so no PHP array is allocated,
/// matching that property while sharing the decoder's exact grammar.
fn json_validate(args: &[Value]) -> Value {
    let json = crate::host::with_host(|h| h.to_str(&args.first().cloned().unwrap_or(Value::Undef)));
    let depth = decode_depth(args, 1);

    set_last_error(JSON_ERROR_NONE);
    let ok = crate::host::with_host(|h| {
        let mut p = Parser::new_validate(json.as_bytes(), depth, h);
        match p.parse_document() {
            Ok(_) => true,
            Err(code) => {
                set_last_error(code);
                false
            }
        }
    });
    Value::bool(ok)
}

// ── recursive-descent parser ────────────────────────────────────────────────

/// A single-pass JSON reader over a byte slice. Errors carry a `JSON_ERROR_*`
/// code so `json_decode` can record it for `json_last_error`.
struct Parser<'a, 'h> {
    b: &'a [u8],
    pos: usize,
    max_depth: usize,
    depth: usize,
    /// When `false` (validate mode) the parser walks the grammar without
    /// allocating PHP arrays; container values are placeholder `Undef`s and the
    /// only observable result is `Ok`/`Err`.
    build: bool,
    h: &'h mut PhpHost,
}

impl<'a, 'h> Parser<'a, 'h> {
    fn new(b: &'a [u8], max_depth: usize, h: &'h mut PhpHost) -> Self {
        Parser {
            b,
            pos: 0,
            max_depth,
            depth: 0,
            build: true,
            h,
        }
    }

    /// Non-building parser for `json_validate`: same grammar, no array allocation.
    fn new_validate(b: &'a [u8], max_depth: usize, h: &'h mut PhpHost) -> Self {
        Parser {
            b,
            pos: 0,
            max_depth,
            depth: 0,
            build: false,
            h,
        }
    }

    /// A fresh empty PHP array, or `Undef` in validate mode.
    fn make_container(&mut self) -> Value {
        if self.build {
            self.h.new_array()
        } else {
            Value::Undef
        }
    }

    /// Parse a whole document: one value with only whitespace around it.
    fn parse_document(&mut self) -> Result<Value, i64> {
        self.skip_ws();
        if self.pos >= self.b.len() {
            return Err(JSON_ERROR_SYNTAX);
        }
        let v = self.parse_value()?;
        self.skip_ws();
        if self.pos != self.b.len() {
            return Err(JSON_ERROR_SYNTAX);
        }
        Ok(v)
    }

    fn skip_ws(&mut self) {
        while let Some(&c) = self.b.get(self.pos) {
            match c {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.pos).copied()
    }

    /// Consume a value: object, array, string, number, or a literal keyword.
    fn parse_value(&mut self) -> Result<Value, i64> {
        match self.peek() {
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => Ok(Value::str(self.parse_string()?)),
            Some(b't') => self.parse_literal(b"true", Value::bool(true)),
            Some(b'f') => self.parse_literal(b"false", Value::bool(false)),
            Some(b'n') => self.parse_literal(b"null", Value::Undef),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.parse_number(),
            _ => Err(JSON_ERROR_SYNTAX),
        }
    }

    fn parse_literal(&mut self, word: &[u8], val: Value) -> Result<Value, i64> {
        if self.b[self.pos..].starts_with(word) {
            self.pos += word.len();
            Ok(val)
        } else {
            Err(JSON_ERROR_SYNTAX)
        }
    }

    fn enter(&mut self) -> Result<(), i64> {
        // Matches ext/json PHP_JSON_DEPTH_INC: check before descending.
        if self.depth >= self.max_depth {
            return Err(JSON_ERROR_DEPTH);
        }
        self.depth += 1;
        Ok(())
    }

    fn parse_array(&mut self) -> Result<Value, i64> {
        self.enter()?;
        self.pos += 1; // '['
        let arr = self.make_container();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(arr);
        }
        loop {
            self.skip_ws();
            let v = self.parse_value()?;
            if self.build {
                self.h.arr_push_auto(&arr, v);
            }
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err(JSON_ERROR_SYNTAX),
            }
        }
        self.depth -= 1;
        Ok(arr)
    }

    fn parse_object(&mut self) -> Result<Value, i64> {
        self.enter()?;
        self.pos += 1; // '{'
        let arr = self.make_container();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(arr);
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(JSON_ERROR_SYNTAX);
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(JSON_ERROR_SYNTAX);
            }
            self.pos += 1; // ':'
            self.skip_ws();
            let v = self.parse_value()?;
            // arr_set_key normalizes canonical integer string keys to int keys,
            // matching PHP array-key coercion for objects like {"0":...}.
            if self.build {
                self.h.arr_set_key(&arr, &Value::str(key), v);
            }
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err(JSON_ERROR_SYNTAX),
            }
        }
        self.depth -= 1;
        Ok(arr)
    }

    /// Parse a `"..."` string starting at the opening quote. Handles the JSON
    /// escapes, `\uXXXX`, and high/low surrogate pairs; rejects raw control
    /// characters (`JSON_ERROR_CTRL_CHAR`) and unpaired surrogates
    /// (`JSON_ERROR_UTF16`).
    fn parse_string(&mut self) -> Result<String, i64> {
        self.pos += 1; // opening quote
        let mut out = String::new();
        loop {
            let c = self.peek().ok_or(JSON_ERROR_SYNTAX)?;
            match c {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.pos += 1;
                    let esc = self.peek().ok_or(JSON_ERROR_SYNTAX)?;
                    self.pos += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let cp = self.parse_hex4()?;
                            self.push_unicode(cp, &mut out)?;
                        }
                        _ => return Err(JSON_ERROR_SYNTAX),
                    }
                }
                c if c < 0x20 => return Err(JSON_ERROR_CTRL_CHAR),
                _ => {
                    // Copy one UTF-8 code point verbatim. The input is valid UTF-8
                    // (a Rust String), so a lead byte always has its continuation
                    // bytes present.
                    let len = utf8_len(c);
                    let end = (self.pos + len).min(self.b.len());
                    out.push_str(&String::from_utf8_lossy(&self.b[self.pos..end]));
                    self.pos = end;
                }
            }
        }
    }

    /// Read exactly four hex digits (the caller has consumed the `\u`).
    fn parse_hex4(&mut self) -> Result<u32, i64> {
        if self.pos + 4 > self.b.len() {
            return Err(JSON_ERROR_SYNTAX);
        }
        let mut val: u32 = 0;
        for _ in 0..4 {
            let d = hex_digit(self.b[self.pos]).ok_or(JSON_ERROR_SYNTAX)?;
            val = (val << 4) | d as u32;
            self.pos += 1;
        }
        Ok(val)
    }

    /// Append a decoded `\uXXXX` code unit, combining a high surrogate with a
    /// following `\uXXXX` low surrogate into one code point.
    fn push_unicode(&mut self, first: u32, out: &mut String) -> Result<(), i64> {
        if (0xD800..=0xDBFF).contains(&first) {
            // High surrogate: require a following \uDC00-\uDFFF low surrogate.
            if self.b.get(self.pos) == Some(&b'\\') && self.b.get(self.pos + 1) == Some(&b'u') {
                self.pos += 2;
                let low = self.parse_hex4()?;
                if (0xDC00..=0xDFFF).contains(&low) {
                    let cp = 0x10000 + ((first - 0xD800) << 10) + (low - 0xDC00);
                    out.push(char::from_u32(cp).ok_or(JSON_ERROR_UTF16)?);
                    return Ok(());
                }
                return Err(JSON_ERROR_UTF16);
            }
            return Err(JSON_ERROR_UTF16);
        }
        if (0xDC00..=0xDFFF).contains(&first) {
            // Unpaired low surrogate.
            return Err(JSON_ERROR_UTF16);
        }
        out.push(char::from_u32(first).ok_or(JSON_ERROR_UTF16)?);
        Ok(())
    }

    /// Parse a JSON number: `-?(0|[1-9]\d*)(\.\d+)?([eE][+-]?\d+)?`. Integral
    /// values that fit `i64` become `int`; anything with a fraction/exponent, or
    /// that overflows `i64`, becomes `float`.
    fn parse_number(&mut self) -> Result<Value, i64> {
        let start = self.pos;
        let mut is_float = false;

        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(c) if c.is_ascii_digit() => {
                while self.peek().is_some_and(|d| d.is_ascii_digit()) {
                    self.pos += 1;
                }
            }
            _ => return Err(JSON_ERROR_SYNTAX),
        }
        // Fraction.
        if self.peek() == Some(b'.') {
            is_float = true;
            self.pos += 1;
            if !self.peek().is_some_and(|d| d.is_ascii_digit()) {
                return Err(JSON_ERROR_SYNTAX);
            }
            while self.peek().is_some_and(|d| d.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        // Exponent.
        if matches!(self.peek(), Some(b'e' | b'E')) {
            is_float = true;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !self.peek().is_some_and(|d| d.is_ascii_digit()) {
                return Err(JSON_ERROR_SYNTAX);
            }
            while self.peek().is_some_and(|d| d.is_ascii_digit()) {
                self.pos += 1;
            }
        }

        let text = std::str::from_utf8(&self.b[start..self.pos]).map_err(|_| JSON_ERROR_SYNTAX)?;
        if is_float {
            text.parse::<f64>()
                .map(Value::float)
                .map_err(|_| JSON_ERROR_SYNTAX)
        } else {
            match text.parse::<i64>() {
                Ok(n) => Ok(Value::int(n)),
                // Integer literal too large for i64: PHP returns a float here.
                Err(_) => text
                    .parse::<f64>()
                    .map(Value::float)
                    .map_err(|_| JSON_ERROR_SYNTAX),
            }
        }
    }
}

/// UTF-8 sequence length from a lead byte (defaults to 1 for continuation/junk).
fn utf8_len(lead: u8) -> usize {
    match lead {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    }
}

/// Value of one ASCII hex digit, or `None`.
fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
