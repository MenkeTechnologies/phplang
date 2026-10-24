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
            with_host(|h| Value::bool(h.is_object_value(&a)))
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
            // The serialize call's own line, which a `__sleep` body overwrites;
            // its warnings are raised against it.
            let line = crate::host::warn_line();
            // `__serialize` (or, failing it, `__sleep`) is PHP code: every
            // answer is collected before the render, and a non-array
            // `__serialize` answer is the reference's `TypeError`.
            let answers = crate::host::collect_object_hooks(
                std::slice::from_ref(&a),
                &["__serialize", "__sleep"],
                &|obj, class, method, r| {
                    if method == "__sleep" {
                        crate::host::set_warn_line(line);
                        return Some(with_host(|h| sleep_props(h, obj, class, &r)));
                    }
                    if with_host(|h| h.is_array(&r)) {
                        return Some(r);
                    }
                    let msg = format!(
                        "{}::__serialize() must return an array",
                        crate::host::display_class(class)
                    );
                    let _ = crate::host::throw_from_internal(
                        "serialize",
                        std::slice::from_ref(&a),
                        "TypeError",
                        &msg,
                    );
                    None
                },
            );
            let Some(answers) = answers else {
                return Some(Ok(Value::Undef));
            };
            let out = with_host(|h| {
                let outer = std::mem::replace(&mut h.serialize_info, answers);
                let denied = unserializable_in(h, &a, &mut crate::host::Visiting::default());
                let s = php_serialize(h, &a);
                h.serialize_info = outer;
                denied.map_or(Ok(s), Err)
            });
            match out {
                Ok(s) => Value::str(s),
                Err(class) => {
                    let _ = crate::host::throw_from_internal(
                        "serialize",
                        std::slice::from_ref(&a),
                        "Exception",
                        &format!("Serialization of '{class}' is not allowed"),
                    );
                    Value::Undef
                }
            }
        }
        "unserialize" => {
            let s = with_host(|h| h.to_str(&arg(args, 0)));
            let raw = crate::bytes::to_bytes(&s);
            let bytes: &[u8] = &raw;
            let total = bytes.len();
            // An empty payload is the one malformed input PHP rejects silently.
            if total == 0 {
                return Some(Ok(Value::bool(false)));
            }
            let mut deferred = Vec::new();
            let result = php_unserialize(bytes, &mut deferred);
            // `__unserialize` and `__wakeup` run once the whole payload has
            // parsed, in the order their objects were finished, as the
            // reference's delayed calls do — and not at all when the payload
            // was malformed.
            if let Unser::Denied(class) = &result {
                let _ = crate::host::throw_from_internal(
                    "unserialize",
                    std::slice::from_ref(&arg(args, 0)),
                    "Exception",
                    &format!("Unserialization of '{class}' is not allowed"),
                );
                return Some(Ok(Value::Undef));
            }
            if !matches!(result, Unser::Failed(..)) {
                for (obj, data) in deferred {
                    let class = with_host(|h| h.object_class(&obj)).unwrap_or_default();
                    let _ = match data {
                        Some(data) => {
                            crate::host::call_method(&class, "__unserialize", Some(obj), vec![data])
                        }
                        None => crate::host::call_method(&class, "__wakeup", Some(obj), Vec::new()),
                    };
                    if crate::host::unwinding() {
                        return Some(Ok(Value::Undef));
                    }
                }
            }
            match result {
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
                Unser::Denied(_) => Value::Undef,
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
            } else if let Some((_, closed)) = h.resource_info(v) {
                if closed {
                    "resource (closed)"
                } else {
                    "resource (stream)"
                }
                .to_string()
            } else {
                // Reported the way a message reports a class name, so an
                // anonymous class shows its readable head alone.
                h.instance_class(v).map_or_else(
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
/// and (recursively) arrays, objects and `enum` cases. A resource serializes as
/// `i:0;`, as the reference writes it.
/// The classes whose `ce_flags` carry `ZEND_ACC_NOT_SERIALIZABLE` among those
/// this runtime has, besides closures, generators and anonymous classes.
const NOT_SERIALIZABLE: [&str; 2] = ["weakmap", "weakreference"];

/// The first value `serialize` would meet that `php_var_serialize_intern`
/// refuses, named as its `Serialization of '%s' is not allowed` names it —
/// walked in the order the render walks, `__serialize`/`__sleep` answers in
/// place of properties. `None` when every value can be written.
fn unserializable_in(
    h: &crate::host::PhpHost,
    v: &Value,
    seen: &mut crate::host::Visiting,
) -> Option<String> {
    if h.is_closure(v) {
        return Some("Closure".to_string());
    }
    if h.is_generator_val(v) {
        return Some("Generator".to_string());
    }
    if !(h.is_array(v) || h.is_object(v)) || h.is_stream(v) || !seen.enter(v) {
        return None;
    }
    let children: Vec<Value> = if h.is_array(v) {
        h.array_pairs(v)
            .unwrap_or_default()
            .into_iter()
            .map(|(_, val)| val)
            .collect()
    } else {
        let class = h.object_class(v).unwrap_or_default();
        if class.contains("@anonymous")
            || NOT_SERIALIZABLE.contains(&class.to_ascii_lowercase().as_str())
        {
            seen.leave();
            return Some(crate::host::display_class(&class).to_string());
        }
        let answer = match v {
            Value::Obj(id) => h.serialize_info.get(id),
            _ => None,
        };
        match answer {
            Some(data) => h
                .array_pairs(data)
                .unwrap_or_default()
                .into_iter()
                .map(|(_, val)| val)
                .collect(),
            None if h.enum_case_of(v).is_some() => Vec::new(),
            None => h.object_props(v).into_iter().map(|(_, val)| val).collect(),
        }
    };
    let found = children.iter().find_map(|c| unserializable_in(h, c, seen));
    seen.leave();
    found
}

/// The declared name of `class` when `object_common` refuses to build it
/// (`ZEND_ACC_NOT_SERIALIZABLE`), or `None`.
fn not_unserializable(class: &str) -> Option<String> {
    let lower = class.to_ascii_lowercase();
    match lower.as_str() {
        "closure" => return Some("Closure".to_string()),
        "generator" => return Some("Generator".to_string()),
        _ => {}
    }
    with_host(|h| {
        let refused = NOT_SERIALIZABLE.contains(&lower.as_str()) || lower.contains("@anonymous");
        (refused && h.class_exists(class))
            .then(|| crate::host::display_class(&h.class_display_name(&lower)).to_string())
    })
}

fn php_serialize(h: &crate::host::PhpHost, v: &Value) -> String {
    php_serialize_seen(h, v, &mut crate::host::Visiting::default())
}

/// [`php_serialize`] with the cycle guard threaded through.
///
/// DIVERGENCE: the reference encodes a repeat as a BACK-REFERENCE — `r:<n>;` for
/// an object, `R:<n>;` for a reference — where `<n>` is the position the value
/// first occupied in the serialization. phplang has no such position table, so a
/// cycle is written as `N;` instead. The output therefore no longer round-trips
/// a self-referential structure, but it is finite and syntactically valid; before
/// this guard the walk exhausted the native stack and aborted the process.
fn php_serialize_seen(
    h: &crate::host::PhpHost,
    v: &Value,
    seen: &mut crate::host::Visiting,
) -> String {
    if (h.is_array(v) || h.is_object(v)) && !seen.enter(v) {
        return "N;".to_string();
    }
    let out = php_serialize_body(h, v, seen);
    if h.is_array(v) || h.is_object(v) {
        seen.leave();
    }
    out
}

fn php_serialize_body(
    h: &crate::host::PhpHost,
    v: &Value,
    seen: &mut crate::host::Visiting,
) -> String {
    match v {
        Value::Undef => "N;".to_string(),
        Value::Bool(b) => format!("b:{};", *b as i32),
        Value::Int(n) => format!("i:{n};"),
        Value::Float(f) => format!("d:{};", serialize_float(*f)),
        // Length is the byte length, matching PHP's binary-safe strings.
        Value::Str(s) => format!("s:{}:\"{}\";", crate::bytes::byte_len(s), s),
        // `php_var_serialize_intern`'s IS_RESOURCE arm writes `i:0;`.
        Value::Obj(_) if h.is_stream(v) => "i:0;".to_string(),
        Value::Obj(_) if h.is_array(v) => {
            let pairs = h.array_pairs(v).unwrap_or_default();
            let mut out = format!("a:{}:{{", pairs.len());
            for (k, val) in pairs {
                out.push_str(&php_serialize_seen(h, &k, seen));
                out.push_str(&php_serialize_seen(h, &val, seen));
            }
            out.push('}');
            out
        }
        Value::Obj(_) if h.is_object(v) => php_serialize_object(h, v, seen),
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
fn php_serialize_object(
    h: &crate::host::PhpHost,
    v: &Value,
    seen: &mut crate::host::Visiting,
) -> String {
    let class = h.object_class(v).unwrap_or_else(|| "stdClass".to_string());
    if let Some((case, _)) = h.enum_case_of(v) {
        let tag = format!("{class}:{case}");
        return format!("E:{}:\"{tag}\";", tag.len());
    }
    // A `__serialize` answer replaces the properties, its keys as written.
    let answer = match v {
        Value::Obj(id) => h.serialize_info.get(id),
        _ => None,
    };
    if let Some(data) = answer {
        // A `__sleep` that answered a non-array: the reference has already
        // counted the element, so it writes a null in its place.
        if !h.is_array(data) {
            return "N;".to_string();
        }
        let pairs = h.array_pairs(data).unwrap_or_default();
        let mut out = format!("O:{}:\"{class}\":{}:{{", class.len(), pairs.len());
        for (k, val) in pairs {
            out.push_str(&php_serialize_seen(h, &k, seen));
            out.push_str(&php_serialize_seen(h, &val, seen));
        }
        out.push('}');
        return out;
    }
    let props = h.object_props(v);
    let mut out = format!("O:{}:\"{class}\":{}:{{", class.len(), props.len());
    for (name, val) in props {
        match crate::host::prop_key(&name) {
            Value::Int(n) => out.push_str(&format!("i:{n};")),
            _ => {
                let key = mangled_prop_key(h, &class, &name);
                out.push_str(&format!("s:{}:\"{key}\";", crate::bytes::byte_len(&key)));
            }
        }
        out.push_str(&php_serialize_seen(h, &val, seen));
    }
    out.push('}');
    out
}

/// The property table `serialize` writes for an object whose class has
/// `__sleep` and no `__serialize`: `php_var_serialize_get_sleep_props`
/// (`ext/standard/var.c`) over the names `__sleep` answered.
///
/// Each name is looked up among the instance's properties as written, then as a
/// private of the object's OWN class (`"\0Class\0name"`), then as a protected
/// (`"\0*\0name"`) — so an ancestor's private property is never found. An
/// uninitialized typed property is found and left out without a word; a name
/// found twice is kept once with a warning; a name not found warns. A non-string
/// name warns and is still looked up by its string form. A non-array answer
/// warns and comes back as null, which the render writes as `N;`.
fn sleep_props(h: &mut crate::host::PhpHost, obj: &Value, class: &str, names: &Value) -> Value {
    let class_name = crate::host::display_class(class);
    let not_names = format!(
        "serialize(): {class_name}::__sleep() should return an array only containing the names of \
         instance-variables to serialize"
    );
    if !h.is_array(names) {
        h.warn(not_names);
        return Value::Undef;
    }
    let props: Vec<(String, Value)> = h
        .object_props(obj)
        .into_iter()
        .map(|(name, val)| (mangled_prop_key(h, class, &name), val))
        .collect();
    let out = h.new_array();
    let mut added: Vec<String> = Vec::new();
    for (_, name_v) in h.array_pairs(names).unwrap_or_default() {
        if !matches!(name_v, Value::Str(_)) {
            h.warn(not_names.clone());
        }
        let name = h.to_str_diag(&name_v);
        let declared_key = mangled_prop_key(h, class, &name);
        let uninit = h.uninit_read_error(obj, &name).is_some();
        let candidates = [
            name.clone(),
            format!("\0{class_name}\0{name}"),
            format!("\0*\0{name}"),
        ];
        let found = candidates.iter().find_map(|key| {
            if let Some((_, val)) = props.iter().find(|(k, _)| k == key) {
                return Some(Some((key.clone(), val.clone())));
            }
            (uninit && *key == declared_key).then_some(None)
        });
        match found {
            Some(Some((key, val))) => {
                if added.contains(&key) {
                    h.warn(format!("serialize(): \"{name}\" is returned from __sleep() multiple times"));
                } else {
                    h.arr_set_key(&out, &Value::str(key.clone()), val);
                    added.push(key);
                }
            }
            Some(None) => {}
            None => h.warn(format!(
                "serialize(): \"{name}\" returned as member variable from __sleep() but does not exist"
            )),
        }
    }
    out
}

/// A property's key in the engine's property table: bare when public or
/// dynamic, `"\0*\0name"` when protected, `"\0Declaring\0name"` when private.
fn mangled_prop_key(h: &crate::host::PhpHost, class: &str, name: &str) -> String {
    match h.prop_visibility(class, name) {
        Some((_, crate::ast::Visibility::Protected)) => format!("\0*\0{name}"),
        Some((declaring, crate::ast::Visibility::Private)) => format!("\0{declaring}\0{name}"),
        _ => name.to_string(),
    }
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
    /// An object of a class `ZEND_ACC_NOT_SERIALIZABLE` marks, by its name.
    Denied(String),
}

/// Parse a PHP serialization payload. Byte-oriented so string lengths are
/// binary-safe.
fn php_unserialize(b: &[u8], deferred: &mut Vec<(Value, Option<Value>)>) -> Unser {
    let mut p = Parser {
        b,
        deferred,
        pos: 0,
        fail_at: None,
        unexpected_end: false,
        saturated: false,
        denied: None,
    };
    let Some(v) = p.value() else {
        if let Some(class) = p.denied {
            return Unser::Denied(class);
        }
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
    /// The delayed calls, in the order their objects were finished:
    /// `(object, Some(data))` for a class with `__unserialize`, to be handed its
    /// data, and `(object, None)` for one with only `__wakeup`.
    deferred: &'a mut Vec<(Value, Option<Value>)>,
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
    /// Set when an `O:` named a class that may not be unserialized: the
    /// reference throws and abandons the payload, with no offset warning.
    denied: Option<String>,
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
                Some(Value::str(crate::bytes::from_bytes(bytes)))
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
                if let Some(name) = not_unserializable(&class) {
                    self.denied = Some(name);
                    return None;
                }
                // An undeclared class is handed to the autoloaders when its name is
                // read, before any of its properties (`zend_lookup_class`).
                if crate::host::class_known(&class).is_err() {
                    return None;
                }
                self.eat(b":")?;
                let count = self.int_until(b':')?;
                if count < 0 {
                    return None;
                }
                self.eat(b"{")?;
                // A class with `__unserialize` is allocated first (defaults, no
                // constructor) and handed its data — keys as written — later.
                let custom = with_host(|h| {
                    h.class_exists(&class) && h.class_has_method(&class, "__unserialize")
                });
                if custom {
                    let obj = crate::host::alloc_object(&class).ok()?;
                    let mut pairs = Vec::with_capacity(count as usize);
                    for _ in 0..count {
                        if let Some(fail) = self.container_exhausted() {
                            return fail;
                        }
                        let k = self.value()?;
                        if let Some(fail) = self.container_exhausted() {
                            return fail;
                        }
                        pairs.push((k, self.value()?));
                    }
                    self.close_container()?;
                    self.deferred
                        .push((obj.clone(), Some(super::common::make_map(pairs))));
                    return Some(obj);
                }
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
                let wakes = known && with_host(|h| h.class_has_method(&target, "__wakeup"));
                // A known class starts from its declared defaults, without its
                // constructor, as `object_init_ex` does: a property the payload
                // does not carry keeps its default, and one it does takes its
                // declared slot.
                let allocated = if known {
                    crate::host::alloc_object(&target).ok()
                } else {
                    None
                };
                let obj = match allocated {
                    Some(obj) => with_host(|h| {
                        for (name, val) in props {
                            h.prop_set(&obj, &name, val);
                        }
                        obj
                    }),
                    None => with_host(|h| h.new_object_bare(&target, props)),
                };
                if wakes {
                    self.deferred.push((obj.clone(), None));
                }
                Some(obj)
            }
            // `E:<len>:"Class:CASE";` — an enum case, resolved back to the
            // singleton rather than rebuilt as a new object.
            b'E' => {
                self.eat(b"E:")?;
                let tag = self.length_prefixed_string()?;
                self.eat(b";")?;
                let (class, case) = tag.split_once(':')?;
                crate::host::class_const_silent(class, case).ok()
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
        Some(crate::bytes::from_bytes(bytes))
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
