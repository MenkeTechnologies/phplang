//! PHP standard-library `ctype` functions. Part of the `stdlib` chain; see
//! `src/stdlib/mod.rs`. `dispatch` returns `None` for names it does not handle.
//!
//! Each `ctype_*` predicate classifies a value in the C locale (bytes 0-255):
//!   * A string passes iff it is non-empty and *every* byte satisfies the class
//!     (an empty string is always `false`).
//!   * An integer in `-128..=255` is a single character code (negatives get 256
//!     added, matching PHP's extended-ASCII handling).
//!   * Any other integer is classified as its decimal-string representation.
//!   * Any other value type yields `false`.
//!
//! This mirrors PHP 8's `ext/ctype`, whose macros call the C `is*` functions on
//! `unsigned char`, so only the ASCII range (< 0x80) can satisfy any class.

use crate::stdlib::common::arg;
use fusevm::Value;

// --- C-locale byte predicates (ASCII only; bytes >= 0x80 satisfy none) ---

fn c_alpha(b: u8) -> bool {
    b.is_ascii_alphabetic()
}
fn c_digit(b: u8) -> bool {
    b.is_ascii_digit()
}
fn c_alnum(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}
fn c_upper(b: u8) -> bool {
    b.is_ascii_uppercase()
}
fn c_lower(b: u8) -> bool {
    b.is_ascii_lowercase()
}
fn c_xdigit(b: u8) -> bool {
    b.is_ascii_hexdigit()
}
/// C `isspace`: space, `\t`, `\n`, `\v`, `\f`, `\r`. Note Rust's
/// `is_ascii_whitespace` omits `\v` (0x0B), so it cannot be used here.
fn c_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}
/// C `iscntrl`: 0x00-0x1F and 0x7F.
fn c_cntrl(b: u8) -> bool {
    b.is_ascii_control()
}
/// C `isgraph`: printable, non-space (0x21-0x7E).
fn c_graph(b: u8) -> bool {
    b.is_ascii_graphic()
}
/// C `isprint`: printable including space (0x20-0x7E).
fn c_print(b: u8) -> bool {
    b == b' ' || b.is_ascii_graphic()
}
/// C `ispunct`: graphic and not alphanumeric.
fn c_punct(b: u8) -> bool {
    b.is_ascii_punctuation()
}

/// Apply `pred` under PHP `ctype` argument rules and return a PHP bool.
fn classify(args: &[Value], pred: fn(u8) -> bool) -> Value {
    let bytes: Vec<u8> = match arg(args, 0) {
        Value::Int(n) => {
            if (-128..=255).contains(&n) {
                let code = if n < 0 { (n + 256) as u8 } else { n as u8 };
                return Value::bool(pred(code));
            }
            n.to_string().into_bytes()
        }
        Value::Str(s) => s.as_bytes().to_vec(),
        _ => return Value::bool(false),
    };
    if bytes.is_empty() {
        return Value::bool(false);
    }
    Value::bool(bytes.iter().all(|&b| pred(b)))
}

/// Dispatch a `ctype`-category PHP function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let pred: fn(u8) -> bool = match name {
        "ctype_alpha" => c_alpha,
        "ctype_digit" => c_digit,
        "ctype_alnum" => c_alnum,
        "ctype_space" => c_space,
        "ctype_upper" => c_upper,
        "ctype_lower" => c_lower,
        "ctype_punct" => c_punct,
        "ctype_xdigit" => c_xdigit,
        "ctype_cntrl" => c_cntrl,
        "ctype_graph" => c_graph,
        "ctype_print" => c_print,
        _ => return None,
    };
    Some(Ok(classify(args, pred)))
}
