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
        // An array is both. Beyond that the two ask different questions: a
        // `Countable` can be counted, while anything `foreach` can drive — an
        // `Iterator`, an `IteratorAggregate`, a `Generator` — is iterable.
        "is_countable" => {
            let a = arg(args, 0);
            with_host(|h| {
                Value::bool(
                    h.is_array(&a)
                        || h.object_class(&a)
                            .is_some_and(|c| h.class_is_a_pub(&c, "Countable")),
                )
            })
        }
        "is_iterable" => {
            let a = arg(args, 0);
            with_host(|h| {
                Value::bool(
                    h.is_array(&a)
                        || h.is_generator_val(&a)
                        || h.object_class(&a).is_some_and(|c| {
                            h.class_is_a_pub(&c, "Traversable")
                                || h.class_is_a_pub(&c, "Iterator")
                                || h.class_is_a_pub(&c, "IteratorAggregate")
                        }),
                )
            })
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
            let bytes = s.as_bytes();
            let total = bytes.len();
            // An empty payload is the one malformed input PHP rejects silently.
            if total == 0 {
                return Some(Ok(Value::bool(false)));
            }
            match php_unserialize(bytes) {
                Unser::Value(v) => v,
                Unser::Saturated(v) => {
                    with_host(|h| h.warn("unserialize(): Numerical result out of range"));
                    v
                }
                // Trailing bytes do NOT fail the call: the value stands and PHP
                // warns about what it did not read.
                Unser::Extra(v, at) => {
                    with_host(|h| {
                        h.warn(format_args!(
                            "unserialize(): Extra data starting at offset {at} of {total} bytes"
                        ))
                    });
                    v
                }
                // A malformed payload is a WARNING plus `false` (not null).
                Unser::Failed(at, short) => {
                    with_host(|h| {
                        if short {
                            h.warn("unserialize(): Unexpected end of serialized data");
                        }
                        h.warn(format_args!(
                            "unserialize(): Error at offset {at} of {total} bytes"
                        ))
                    });
                    Value::bool(false)
                }
            }
        }

        _ => return None,
    };
    Some(Ok(v))
}

/// How an error message names a *value* (PHP's `zend_zval_value_name`), which
/// is not how [`debug_type`] names its type: a bool is reported as the literal
/// `true` / `false` it is, not as `bool`.
///
/// `Cannot use "::class" on true` is the wording this exists for.
pub fn value_name(h: &crate::host::PhpHost, v: &Value) -> String {
    match v {
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => "false".to_string(),
        _ => debug_type(h, v),
    }
}

/// `get_debug_type` name for a value.
pub fn debug_type(h: &crate::host::PhpHost, v: &Value) -> String {
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
                // Reported the way a message reports a class name, so an
                // anonymous class shows its readable head alone.
                h.object_class(v).map_or_else(
                    || "object".to_string(),
                    |c| crate::host::display_class(&c).to_string(),
                )
            }
        }
        _ => "unknown".to_string(),
    }
}

// ── serialize ────────────────────────────────────────────────────────────────

/// Render a value in PHP's serialization format: null, bool, int, float, string,
/// and (recursively) arrays, objects and `enum` cases. A closure or a resource
/// has no serializable form and falls back to `N;` (null).
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
        Value::Obj(_) if h.is_object(v) => php_serialize_object(h, v),
        _ => "N;".to_string(),
    }
}

/// `serialize` of an object: `O:<len>:"Class":<n>:{<key><value>...}`.
///
/// The property KEYS carry the visibility, mangled the way the engine stores
/// them: a protected `$b` is `"\0*\0b"` and a private `$c` declared in `P` is
/// `"\0P\0c"`. Those NUL bytes are counted in the string length, which is why the
/// reference prints `s:4:" * b"` — four bytes, two of them NUL.
///
/// An `enum` case is not an object here at all: it serializes as
/// `E:<len>:"Class:CASE";`, carrying only enough to look the singleton back up.
fn php_serialize_object(h: &crate::host::PhpHost, v: &Value) -> String {
    let class = h.object_class(v).unwrap_or_else(|| "stdClass".to_string());
    if let Some((case, _)) = h.enum_case_of(v) {
        let tag = format!("{class}:{case}");
        return format!("E:{}:\"{tag}\";", tag.len());
    }
    let props = h.object_props(v);
    let mut out = format!("O:{}:\"{class}\":{}:{{", class.len(), props.len());
    for (name, val) in props {
        let key = match h.prop_visibility(&class, &name) {
            Some((_, crate::ast::Visibility::Protected)) => format!("\0*\0{name}"),
            Some((declaring, crate::ast::Visibility::Private)) => format!("\0{declaring}\0{name}"),
            _ => name,
        };
        out.push_str(&format!("s:{}:\"{key}\";", key.len()));
        out.push_str(&php_serialize(h, &val));
    }
    out.push('}');
    out
}

/// Format a float at `serialize_precision = -1` — the shortest digit string that
/// round-trips. This is the representation `serialize`, `var_dump`, `var_export`
/// and `json_encode` all share; it is *not* the `precision = 14` rendering that
/// `echo` and string casts use (`echo 1/3` gives `0.33333333333333` while
/// `var_dump(1/3)` gives `float(0.3333333333333333)`).
///
/// PHP calls `zend_gcvt` on the shortest round-tripping digit string (`zend_dtoa`
/// mode 0) and switches to E-notation when the decimal point position `decpt`
/// (= exponent + 1) exceeds 17 or drops below -3 — i.e. large or small magnitudes
/// render as `1.0E+20` / `1.0E-10`, everything in between stays fixed. Rust's
/// `{:e}` already produces the shortest round-tripping mantissa (Ryu), so parse
/// that and reformat PHP-style. Verified against reference PHP 8:
/// `serialize(1e17)` = `d:1.0E+17;` but `serialize(1e16)` = `d:10000000000000000;`.
pub fn serialize_float(f: f64) -> String {
    if f.is_nan() {
        return "NAN".to_string();
    }
    if f.is_infinite() {
        return if f < 0.0 { "-INF" } else { "INF" }.to_string();
    }
    if f == 0.0 {
        // PHP renders negative zero as "-0".
        return if f.is_sign_negative() { "-0" } else { "0" }.to_string();
    }
    let neg = f < 0.0;
    let a = f.abs();
    // `{:e}` yields e.g. "1.5e0", "1e20", "1.844674407371e19".
    let sci = format!("{a:e}");
    let (mant, exp_s) = sci.split_once('e').expect("`{:e}` always contains 'e'");
    let exp: i32 = exp_s.parse().expect("`{:e}` exponent is an integer");
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    // `decpt`: position of the decimal point relative to the digit string, the
    // same quantity `zend_dtoa` reports.
    let decpt = exp + 1;
    let body = if !(-3..=17).contains(&decpt) {
        // Exponential: one leading digit, then the rest (or ".0"), signed exponent.
        let (first, rest) = digits.split_at(1);
        let frac = if rest.is_empty() { "0" } else { rest };
        let e = decpt - 1;
        format!(
            "{first}.{frac}E{}{}",
            if e < 0 { "-" } else { "+" },
            e.abs()
        )
    } else if decpt <= 0 {
        // 0.00…digits — leading zeros equal to -decpt.
        format!("0.{}{}", "0".repeat((-decpt) as usize), digits)
    } else if decpt as usize >= digits.len() {
        // Integer-valued: pad trailing zeros out to the decimal point.
        format!("{digits}{}", "0".repeat(decpt as usize - digits.len()))
    } else {
        // Decimal point falls inside the digit string.
        let (int_part, frac) = digits.split_at(decpt as usize);
        format!("{int_part}.{frac}")
    };
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

// ── unserialize ──────────────────────────────────────────────────────────────

/// The outcome of parsing a serialization payload, with everything
/// `unserialize` needs to reproduce the reference's diagnostics.
enum Unser {
    /// A complete value, consuming the whole payload.
    Value(Value),
    /// A complete value followed by bytes the parser did not read. PHP KEEPS the
    /// value here and warns — it does not fail.
    Extra(Value, usize),
    /// A complete value whose `i:` literal was clamped to the platform integer
    /// range. The value stands; only the clamp is reported.
    Saturated(Value),
    /// Malformed, at the given byte offset. `unexpected_end` marks the payload
    /// that claimed more container elements than it carried.
    Failed(usize, bool),
}

/// Parse a PHP serialization payload. Byte-oriented so string lengths are
/// binary-safe.
fn php_unserialize(b: &[u8]) -> Unser {
    let mut p = Parser {
        b,
        pos: 0,
        fail_at: None,
        unexpected_end: false,
        saturated: false,
    };
    let Some(v) = p.value() else {
        return Unser::Failed(p.fail_at.unwrap_or(0), p.unexpected_end);
    };
    if p.pos != b.len() {
        Unser::Extra(v, p.pos)
    } else if p.saturated {
        Unser::Saturated(v)
    } else {
        Unser::Value(v)
    }
}

struct Parser<'a> {
    b: &'a [u8],
    pos: usize,
    /// Where the reference would report the parse as having failed. Written by
    /// the FIRST branch to fail, which is the innermost one — a bad element deep
    /// in an array is reported at its own offset, not the array's.
    fail_at: Option<usize>,
    /// Set when a container's closing brace arrived while declared elements were
    /// still owed. The reference prints an extra note before the offset error in
    /// that case alone — a payload that merely stops mid-element does not get it.
    unexpected_end: bool,
    /// Set when an `i:` literal was clamped to `PHP_INT_MIN`/`PHP_INT_MAX`. The
    /// value still stands; the reference just warns about the clamp.
    saturated: bool,
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
    /// Strict: overflow or non-numeric input fails. Used for lengths and counts.
    fn int_until(&mut self, delim: u8) -> Option<i64> {
        let s = std::str::from_utf8(self.read_until(delim)?).ok()?;
        s.parse().ok()
    }

    /// Parse a serialized integer *value* up to `delim`. Unlike `int_until`, a
    /// magnitude beyond `i64` saturates to `PHP_INT_MAX`/`PHP_INT_MIN` rather than
    /// failing — PHP's `unserialize` clamps an out-of-range `i:` literal (emitting
    /// a warning) instead of returning `false`.
    fn int_value(&mut self, delim: u8) -> Option<i64> {
        let s = std::str::from_utf8(self.read_until(delim)?).ok()?;
        match s.parse::<i64>() {
            Ok(n) => Some(n),
            Err(_) => {
                // Saturate only genuine numeric overflow; reject anything else.
                let (neg, body) = match s.strip_prefix('-') {
                    Some(rest) => (true, rest),
                    None => (false, s),
                };
                if !body.is_empty() && body.bytes().all(|c| c.is_ascii_digit()) {
                    // The clamp is reported: PHP warns "Numerical result out of
                    // range" and still hands back the saturated value.
                    self.saturated = true;
                    Some(if neg { i64::MIN } else { i64::MAX })
                } else {
                    None
                }
            }
        }
    }

    /// Record where a parse failed, if nothing deeper already has, and fail.
    ///
    /// Which offset the reference names depends on the tag, and the split is not
    /// arbitrary: `N`/`b`/`i`/`d` and an unrecognized tag are scanned as one token
    /// and reported at the token's own start, while `s`/`O`/`E` scan a length and
    /// then check the payload separately, so they are reported at the payload —
    /// two bytes in, just past `X:`. An `a` header follows the first rule; a bad
    /// element inside any container follows whichever rule that element's tag has.
    fn fail_at(&mut self, offset: usize) -> Option<Value> {
        self.fail_at.get_or_insert(offset);
        None
    }

    /// Called before each declared element of a container. `Some(_)` means the
    /// closing brace arrived while elements were still owed — a payload that
    /// claims more than it carries, which the reference reports as "Unexpected
    /// end of serialized data" BEFORE the offset error. A payload that simply
    /// stops mid-element is a plain offset error with no such note.
    fn container_exhausted(&mut self) -> Option<Option<Value>> {
        if self.b.get(self.pos) != Some(&b'}') {
            return None;
        }
        self.unexpected_end = true;
        Some(self.fail_at(self.pos))
    }

    /// Consume a container's closing brace, blaming the cursor (not the
    /// container's own start) when something else is there.
    fn close_container(&mut self) -> Option<()> {
        if self.b.get(self.pos) != Some(&b'}') {
            self.fail_at(self.pos);
            return None;
        }
        self.pos += 1;
        Some(())
    }

    /// Parse one serialized value at the cursor.
    fn value(&mut self) -> Option<Value> {
        let start = self.pos;
        // Past the end: the innermost failure is at the cursor itself.
        let Some(&tag) = self.b.get(self.pos) else {
            return self.fail_at(start);
        };
        // `s`, `O` and `E` are reported at their payload, everything else at the
        // tag. Computed up front so every failing path below agrees.
        let blame = match tag {
            b's' | b'O' | b'E' => start + 2,
            _ => start,
        };
        match self.value_inner(tag) {
            Some(v) => Some(v),
            None => self.fail_at(blame),
        }
    }

    /// The per-tag body of [`Parser::value`]; every `None` it returns is turned
    /// into a recorded failure by the caller.
    fn value_inner(&mut self, tag: u8) -> Option<Value> {
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
                let n = self.int_value(b';')?;
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
                // A negative element count is malformed; PHP rejects it (returns
                // `false`) rather than treating it as an empty array.
                if count < 0 {
                    return None;
                }
                self.eat(b"{")?;
                let mut pairs: Vec<(Value, Value)> = Vec::new();
                for _ in 0..count {
                    if let Some(fail) = self.container_exhausted() {
                        return fail;
                    }
                    let k = self.value()?;
                    if let Some(fail) = self.container_exhausted() {
                        return fail;
                    }
                    let v = self.value()?;
                    pairs.push((k, v));
                }
                self.close_container()?;
                Some(super::common::make_map(pairs))
            }
            // `O:<len>:"Class":<n>:{...}` — an object, restored WITHOUT running
            // its constructor and with the property-name mangling undone.
            b'O' => {
                self.eat(b"O:")?;
                let class = self.length_prefixed_string()?;
                self.eat(b":")?;
                let count = self.int_until(b':')?;
                if count < 0 {
                    return None;
                }
                self.eat(b"{")?;
                let mut props: Vec<(String, Value)> = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    if let Some(fail) = self.container_exhausted() {
                        return fail;
                    }
                    let Value::Str(k) = self.value()? else {
                        return None;
                    };
                    if let Some(fail) = self.container_exhausted() {
                        return fail;
                    }
                    props.push((demangle_prop(&k), self.value()?));
                }
                self.close_container()?;
                // An unknown class becomes `__PHP_Incomplete_Class`, whose real
                // name is carried in a leading synthetic property.
                let known = with_host(|h| h.class_exists(&class));
                if !known {
                    props.insert(
                        0,
                        (
                            "__PHP_Incomplete_Class_Name".to_string(),
                            Value::str(class.clone()),
                        ),
                    );
                }
                let target = if known {
                    class
                } else {
                    "__PHP_Incomplete_Class".to_string()
                };
                Some(with_host(|h| h.new_object_bare(&target, props)))
            }
            // `E:<len>:"Class:CASE";` — an enum case, resolved back to the
            // singleton rather than rebuilt as a new object.
            b'E' => {
                self.eat(b"E:")?;
                let tag = self.length_prefixed_string()?;
                self.eat(b";")?;
                let (class, case) = tag.split_once(':')?;
                crate::host::class_const(class, case).ok()
            }
            _ => None,
        }
    }

    /// The `<len>:"<bytes>"` payload shared by the `O:` and `E:` tags.
    fn length_prefixed_string(&mut self) -> Option<String> {
        let len = self.int_until(b':')? as usize;
        self.eat(b"\"")?;
        let end = self.pos.checked_add(len)?;
        if end > self.b.len() {
            return None;
        }
        let bytes = &self.b[self.pos..end];
        self.pos = end;
        self.eat(b"\"")?;
        Some(String::from_utf8_lossy(bytes).into_owned())
    }
}

/// Undo `serialize`'s visibility mangling on a property name: `"\0*\0b"` and
/// `"\0Declaring\0c"` both name the plain property they wrap.
fn demangle_prop(k: &str) -> String {
    match k.strip_prefix('\0') {
        Some(rest) => rest
            .split_once('\0')
            .map(|(_, name)| name.to_string())
            .unwrap_or_else(|| k.to_string()),
        None => k.to_string(),
    }
}
