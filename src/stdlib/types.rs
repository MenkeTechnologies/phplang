//! PHP standard-library `types` functions. Part of the `stdlib` chain; see
//! `src/stdlib/mod.rs`. `dispatch` returns `None` for names it does not handle.
//!
//! Core (`builtins::call_library`) already owns `gettype`, `is_array`,
//! `is_int`/`integer`/`long`, `is_float`/`double`, `is_string`, `is_bool`,
//! `is_null`, `is_numeric`, `is_callable`, `boolval`, `intval`, `floatval`/
//! `doubleval`, `strval`, `var_export`, `var_dump`, `print_r`; those never reach
//! here. This module adds the remaining type predicates plus `get_debug_type`
//! and the `serialize`/`unserialize` round trip.

use crate::host::with_host;
use fusevm::Value;

use super::common::arg;

/// Dispatch a `types`-category PHP function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let v = match name {
        // ── type predicates ─────────────────────────────────────────────────
        // A scalar is int/float/string/bool. null, arrays, and objects are not.
        "is_scalar" => Value::bool(matches!(
            arg(args, 0),
            Value::Int(_) | Value::Float(_) | Value::Str(_) | Value::Bool(_)
        )),
        // `is_object` is true for class instances and closures, but not arrays.
        "is_object" => {
            let a = arg(args, 0);
            with_host(|h| Value::bool(h.is_object(&a) || h.is_closure(&a)))
        }
        // Only arrays are iterable/countable here (no Traversable/Countable
        // objects exist in this runtime yet).
        "is_iterable" | "is_countable" => {
            let a = arg(args, 0);
            with_host(|h| Value::bool(h.is_array(&a)))
        }

        // ── get_debug_type ──────────────────────────────────────────────────
        // PHP 8's precise type name: scalar keywords, "array", "null", or the
        // class name for objects ("Closure" for a closure).
        "get_debug_type" => {
            let a = arg(args, 0);
            let name = with_host(|h| debug_type(h, &a));
            Value::str(name)
        }

        // ── serialization ───────────────────────────────────────────────────
        "serialize" => {
            let a = arg(args, 0);
            let out = with_host(|h| php_serialize(h, &a));
            Value::str(out)
        }
        "unserialize" => {
            let s = with_host(|h| h.to_str(&arg(args, 0)));
            // PHP returns `false` (not null) on a malformed payload.
            php_unserialize(s.as_bytes()).unwrap_or(Value::bool(false))
        }

        _ => return None,
    };
    Some(Ok(v))
}

/// `get_debug_type` name for a value.
fn debug_type(h: &crate::host::PhpHost, v: &Value) -> String {
    match v {
        Value::Undef => "null".to_string(),
        Value::Bool(_) => "bool".to_string(),
        Value::Int(_) => "int".to_string(),
        Value::Float(_) => "float".to_string(),
        Value::Str(_) => "string".to_string(),
        Value::Obj(_) => {
            if h.is_array(v) {
                "array".to_string()
            } else if h.is_closure(v) {
                "Closure".to_string()
            } else {
                h.object_class(v).unwrap_or_else(|| "object".to_string())
            }
        }
        _ => "unknown".to_string(),
    }
}

// ── serialize ────────────────────────────────────────────────────────────────

/// Render a value in PHP's serialization format. Handles null, bool, int, float,
/// string, and (recursively) arrays — the scalar/array subset PHP round-trips
/// without class support. A closure or user object has no serializable form here
/// and falls back to `N;` (null).
fn php_serialize(h: &crate::host::PhpHost, v: &Value) -> String {
    match v {
        Value::Undef => "N;".to_string(),
        Value::Bool(b) => format!("b:{};", *b as i32),
        Value::Int(n) => format!("i:{n};"),
        Value::Float(f) => format!("d:{};", serialize_float(*f)),
        // Length is the byte length, matching PHP's binary-safe strings.
        Value::Str(s) => format!("s:{}:\"{}\";", s.len(), s),
        Value::Obj(_) if h.is_array(v) => {
            let pairs = h.array_pairs(v).unwrap_or_default();
            let mut out = format!("a:{}:{{", pairs.len());
            for (k, val) in pairs {
                out.push_str(&php_serialize(h, &k));
                out.push_str(&php_serialize(h, &val));
            }
            out.push('}');
            out
        }
        _ => "N;".to_string(),
    }
}

/// Format a float the way PHP's `serialize` does (serialize_precision = -1: the
/// shortest string that round-trips). Rust's `{}` yields that; only the non-finite
/// spellings differ.
fn serialize_float(f: f64) -> String {
    if f.is_nan() {
        "NAN".to_string()
    } else if f.is_infinite() {
        if f < 0.0 { "-INF" } else { "INF" }.to_string()
    } else {
        format!("{f}")
    }
}

// ── unserialize ──────────────────────────────────────────────────────────────

/// Parse a PHP serialization payload into a `Value`. Returns `None` on any
/// malformed input (the caller maps that to PHP's `false`). Byte-oriented so
/// string lengths are binary-safe.
fn php_unserialize(b: &[u8]) -> Option<Value> {
    let mut p = Parser { b, pos: 0 };
    let v = p.value()?;
    // Trailing garbage after a complete value is a parse failure in PHP.
    if p.pos == b.len() {
        Some(v)
    } else {
        None
    }
}

struct Parser<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    /// Consume the exact byte sequence `s`, or fail.
    fn eat(&mut self, s: &[u8]) -> Option<()> {
        if self.b[self.pos..].starts_with(s) {
            self.pos += s.len();
            Some(())
        } else {
            None
        }
    }

    /// Read up to (and consuming) the delimiter byte, returning the bytes before it.
    fn read_until(&mut self, delim: u8) -> Option<&'a [u8]> {
        let start = self.pos;
        while self.pos < self.b.len() && self.b[self.pos] != delim {
            self.pos += 1;
        }
        if self.pos >= self.b.len() {
            return None;
        }
        let slice = &self.b[start..self.pos];
        self.pos += 1; // consume the delimiter
        Some(slice)
    }

    /// Parse the decimal integer that runs up to `delim`, consuming the delimiter.
    fn int_until(&mut self, delim: u8) -> Option<i64> {
        let s = std::str::from_utf8(self.read_until(delim)?).ok()?;
        s.parse().ok()
    }

    /// Parse one serialized value at the cursor.
    fn value(&mut self) -> Option<Value> {
        let tag = *self.b.get(self.pos)?;
        match tag {
            b'N' => {
                self.eat(b"N;")?;
                Some(Value::Undef)
            }
            b'b' => {
                self.eat(b"b:")?;
                let d = *self.b.get(self.pos)?;
                self.pos += 1;
                self.eat(b";")?;
                match d {
                    b'0' => Some(Value::bool(false)),
                    b'1' => Some(Value::bool(true)),
                    _ => None,
                }
            }
            b'i' => {
                self.eat(b"i:")?;
                let n = self.int_until(b';')?;
                Some(Value::int(n))
            }
            b'd' => {
                self.eat(b"d:")?;
                let s = std::str::from_utf8(self.read_until(b';')?).ok()?;
                let f = match s {
                    "NAN" => f64::NAN,
                    "INF" => f64::INFINITY,
                    "-INF" => f64::NEG_INFINITY,
                    _ => s.parse().ok()?,
                };
                Some(Value::float(f))
            }
            b's' => {
                self.eat(b"s:")?;
                let len = self.int_until(b':')? as usize;
                self.eat(b"\"")?;
                let end = self.pos.checked_add(len)?;
                if end > self.b.len() {
                    return None;
                }
                let bytes = &self.b[self.pos..end];
                self.pos = end;
                self.eat(b"\";")?;
                Some(Value::str(String::from_utf8_lossy(bytes).into_owned()))
            }
            b'a' => {
                self.eat(b"a:")?;
                let count = self.int_until(b':')?;
                self.eat(b"{")?;
                let mut pairs: Vec<(Value, Value)> = Vec::new();
                for _ in 0..count {
                    let k = self.value()?;
                    let v = self.value()?;
                    pairs.push((k, v));
                }
                self.eat(b"}")?;
                Some(super::common::make_map(pairs))
            }
            _ => None,
        }
    }
}
