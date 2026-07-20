//! PHP standard-library `filter` functions. Part of the `stdlib` chain; see
//! `src/stdlib/mod.rs`. `dispatch` returns `None` for names it does not handle.
//!
//! Implements `filter_var($value, $filter, $options)` and `filter_var_array`.
//! Validation filters return the (typed) value on success and `false` on
//! failure (or `null` when `FILTER_NULL_ON_FAILURE` is set); sanitization
//! filters return the cleaned string. Email/URL/domain validation uses the
//! `regex` crate; IP validation uses `std::net`.
//!
//! CONSTANTS: phplang has no constant table, so a bareword filter/flag constant
//! reaches this module as its *name* string (`Value::Str("FILTER_VALIDATE_INT")`)
//! rather than its integer. Every resolver therefore matches BOTH the canonical
//! PHP integer AND the uppercase constant-name string.
//!
//! LIMITATIONS: an array `$value` is rejected as `false` (the `FILTER_REQUIRE_ARRAY`
//! /`FILTER_FORCE_ARRAY` flags that would change this need scalar-vs-array flag
//! plumbing that is out of scope here). The rarely-used range/encoding sub-flags
//! not listed in the resolvers below are treated as absent.

use crate::host::with_host;
use fusevm::Value;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::LazyLock;

// ── filter ids (PHP canonical integers) ─────────────────────────────────────

const FILTER_DEFAULT: i64 = 516; // == FILTER_UNSAFE_RAW
const FILTER_VALIDATE_INT: i64 = 257;
const FILTER_VALIDATE_BOOLEAN: i64 = 258;
const FILTER_VALIDATE_FLOAT: i64 = 259;
const FILTER_VALIDATE_REGEXP: i64 = 272;
const FILTER_VALIDATE_URL: i64 = 273;
const FILTER_VALIDATE_EMAIL: i64 = 274;
const FILTER_VALIDATE_IP: i64 = 275;
const FILTER_VALIDATE_DOMAIN: i64 = 277;
const FILTER_SANITIZE_STRING: i64 = 513; // == FILTER_SANITIZE_STRIPPED
const FILTER_SANITIZE_SPECIAL_CHARS: i64 = 515;
const FILTER_SANITIZE_EMAIL: i64 = 517;
const FILTER_SANITIZE_URL: i64 = 518;
const FILTER_SANITIZE_NUMBER_INT: i64 = 519;
const FILTER_SANITIZE_NUMBER_FLOAT: i64 = 520;
const FILTER_SANITIZE_FULL_SPECIAL_CHARS: i64 = 522;

// ── flags ───────────────────────────────────────────────────────────────────

const FLAG_ALLOW_OCTAL: i64 = 1;
const FLAG_ALLOW_HEX: i64 = 2;
const FLAG_ALLOW_FRACTION: i64 = 4096;
const FLAG_ALLOW_THOUSAND: i64 = 8192;
const FLAG_ALLOW_SCIENTIFIC: i64 = 16384;
const FLAG_IPV4: i64 = 1048576;
const FLAG_IPV6: i64 = 2097152;
const FLAG_HOSTNAME: i64 = 1048576; // shares the numeric slot of IPV4/EMAIL_UNICODE
const FLAG_NULL_ON_FAILURE: i64 = 134217728;

/// Dispatch a `filter`-category PHP function by lowercased name. Returns `None`
/// for names this module does not implement so the stdlib chain can continue.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let v = match name {
        "filter_var" => filter_var(args),
        "filter_var_array" => filter_var_array(args),
        _ => return None,
    };
    Some(Ok(v))
}

// ── filter_var ───────────────────────────────────────────────────────────────

/// `filter_var($value, $filter = FILTER_DEFAULT, $options = 0)`.
fn filter_var(args: &[Value]) -> Value {
    let input = args.first().cloned().unwrap_or(Value::Undef);
    let filter_id = args
        .get(1)
        .map(resolve_filter_id)
        .unwrap_or(FILTER_DEFAULT);
    let cfg = args.get(2).cloned().unwrap_or(Value::Undef);
    let (flags, options) = parse_config(&cfg);
    apply_filter(&input, filter_id, flags, &options)
}

/// `filter_var_array($data, $definition = FILTER_DEFAULT)`.
///
/// `$definition` may be a single filter id (applied to every field) or an array
/// mapping each field name to its own spec (an id, or a `['filter'=>…, 'flags'=>…,
/// 'options'=>…]` config array). Fields present in `$definition` but absent from
/// `$data` are filtered as `null`.
fn filter_var_array(args: &[Value]) -> Value {
    let data = args.first().cloned().unwrap_or(Value::Undef);
    let Some(data_pairs) = with_host(|h| h.array_pairs(&data)) else {
        return Value::bool(false);
    };
    let definition = args.get(1).cloned().unwrap_or(Value::Undef);

    // Definition is an array of per-field specs.
    if let Some(def_pairs) = with_host(|h| h.array_pairs(&definition)) {
        let mut out: Vec<(Value, Value)> = Vec::with_capacity(def_pairs.len());
        for (field, spec) in &def_pairs {
            // A field named in the definition but absent from the data yields
            // `null` (PHP distinguishes a missing key from a present null, which
            // instead runs through the filter and typically fails to `false`).
            let found = data_pairs.iter().find(|(k, _)| value_key_eq(k, field));
            let result = match found {
                Some((_, value)) => {
                    let (id, flags, options) = resolve_spec(spec);
                    apply_filter(value, id, flags, &options)
                }
                None => Value::Undef,
            };
            out.push((field.clone(), result));
        }
        return make_map(out);
    }

    // Definition is a single filter id applied to every field.
    let id = resolve_filter_id(&definition);
    let mut out: Vec<(Value, Value)> = Vec::with_capacity(data_pairs.len());
    for (field, value) in &data_pairs {
        out.push((field.clone(), apply_filter(value, id, 0, &None)));
    }
    make_map(out)
}

/// A per-field spec inside `filter_var_array`: either a bare filter id, or a
/// config array with `filter`, `flags` and `options` keys.
fn resolve_spec(spec: &Value) -> (i64, i64, Option<Value>) {
    if with_host(|h| h.is_array(spec)) {
        let id = map_get(spec, "filter")
            .map(|v| resolve_filter_id(&v))
            .unwrap_or(FILTER_DEFAULT);
        let flags = map_get(spec, "flags").map(|v| flag_value(&v)).unwrap_or(0);
        let options = map_get(spec, "options");
        return (id, flags, options);
    }
    (resolve_filter_id(spec), 0, None)
}

// ── config parsing ───────────────────────────────────────────────────────────

/// Parse the third `filter_var` argument, which is either a flags bitmask or an
/// array with `flags` and `options` members. Returns `(flags, options_array)`.
fn parse_config(cfg: &Value) -> (i64, Option<Value>) {
    if with_host(|h| h.is_array(cfg)) {
        let flags = map_get(cfg, "flags").map(|v| flag_value(&v)).unwrap_or(0);
        let options = map_get(cfg, "options");
        return (flags, options);
    }
    (flag_value(cfg), None)
}

/// Look up `key` in an associative `Value` array, returning its value if present.
fn map_get(arr: &Value, key: &str) -> Option<Value> {
    with_host(|h| {
        h.array_pairs(arr)?
            .into_iter()
            .find(|(k, _)| h.to_str(k) == key)
            .map(|(_, v)| v)
    })
}

/// Fetch an option from the nested `options` map (min_range, max_range, regexp,
/// default, …).
fn opt_get(options: &Option<Value>, key: &str) -> Option<Value> {
    options.as_ref().and_then(|o| map_get(o, key))
}

/// Resolve a filter constant (integer or bareword name) to its canonical id.
fn resolve_filter_id(v: &Value) -> i64 {
    match v {
        Value::Int(n) => *n,
        Value::Str(s) => match s.as_str() {
            "FILTER_DEFAULT" | "FILTER_UNSAFE_RAW" => FILTER_DEFAULT,
            "FILTER_VALIDATE_INT" => FILTER_VALIDATE_INT,
            "FILTER_VALIDATE_BOOLEAN" | "FILTER_VALIDATE_BOOL" => FILTER_VALIDATE_BOOLEAN,
            "FILTER_VALIDATE_FLOAT" => FILTER_VALIDATE_FLOAT,
            "FILTER_VALIDATE_REGEXP" => FILTER_VALIDATE_REGEXP,
            "FILTER_VALIDATE_URL" => FILTER_VALIDATE_URL,
            "FILTER_VALIDATE_EMAIL" => FILTER_VALIDATE_EMAIL,
            "FILTER_VALIDATE_IP" => FILTER_VALIDATE_IP,
            "FILTER_VALIDATE_DOMAIN" => FILTER_VALIDATE_DOMAIN,
            "FILTER_SANITIZE_STRING" | "FILTER_SANITIZE_STRIPPED" => FILTER_SANITIZE_STRING,
            "FILTER_SANITIZE_SPECIAL_CHARS" => FILTER_SANITIZE_SPECIAL_CHARS,
            "FILTER_SANITIZE_FULL_SPECIAL_CHARS" => FILTER_SANITIZE_FULL_SPECIAL_CHARS,
            "FILTER_SANITIZE_EMAIL" => FILTER_SANITIZE_EMAIL,
            "FILTER_SANITIZE_URL" => FILTER_SANITIZE_URL,
            "FILTER_SANITIZE_NUMBER_INT" => FILTER_SANITIZE_NUMBER_INT,
            "FILTER_SANITIZE_NUMBER_FLOAT" => FILTER_SANITIZE_NUMBER_FLOAT,
            _ => FILTER_DEFAULT,
        },
        _ => FILTER_DEFAULT,
    }
}

/// Resolve a filter flag constant (integer or bareword name) to its bitmask.
fn flag_value(v: &Value) -> i64 {
    match v {
        Value::Int(n) => *n,
        Value::Str(s) => match s.as_str() {
            "FILTER_FLAG_ALLOW_OCTAL" => FLAG_ALLOW_OCTAL,
            "FILTER_FLAG_ALLOW_HEX" => FLAG_ALLOW_HEX,
            "FILTER_FLAG_ALLOW_FRACTION" => FLAG_ALLOW_FRACTION,
            "FILTER_FLAG_ALLOW_THOUSAND" => FLAG_ALLOW_THOUSAND,
            "FILTER_FLAG_ALLOW_SCIENTIFIC" => FLAG_ALLOW_SCIENTIFIC,
            "FILTER_FLAG_IPV4" => FLAG_IPV4,
            "FILTER_FLAG_IPV6" => FLAG_IPV6,
            "FILTER_FLAG_HOSTNAME" | "FILTER_FLAG_EMAIL_UNICODE" => FLAG_HOSTNAME,
            "FILTER_NULL_ON_FAILURE" => FLAG_NULL_ON_FAILURE,
            _ => 0,
        },
        _ => 0,
    }
}

// ── dispatch to the concrete filter ──────────────────────────────────────────

/// Apply a resolved filter to a value, returning the PHP result.
fn apply_filter(input: &Value, filter_id: i64, flags: i64, options: &Option<Value>) -> Value {
    // An array value is rejected by the scalar filters (see module LIMITATIONS).
    if with_host(|h| h.is_array(input)) {
        return fail(flags);
    }
    let s = with_host(|h| h.to_str(input));

    match filter_id {
        FILTER_VALIDATE_INT => match validate_int(&s, flags, options) {
            Some(n) => Value::int(n),
            None => fail(flags),
        },
        FILTER_VALIDATE_FLOAT => match validate_float(&s, flags, options) {
            Some(f) => Value::float(f),
            None => fail(flags),
        },
        FILTER_VALIDATE_BOOLEAN => match validate_bool(&s) {
            Some(b) => Value::bool(b),
            // Recognized-false is handled inside validate_bool; a genuine
            // non-match is `false` normally, `null` with NULL_ON_FAILURE.
            None => fail(flags),
        },
        FILTER_VALIDATE_EMAIL => pass_or_fail(&s, validate_email(&s), flags),
        FILTER_VALIDATE_URL => pass_or_fail(&s, validate_url(&s), flags),
        FILTER_VALIDATE_IP => pass_or_fail(&s, validate_ip(&s, flags), flags),
        FILTER_VALIDATE_DOMAIN => pass_or_fail(&s, validate_domain(&s, flags), flags),
        FILTER_VALIDATE_REGEXP => pass_or_fail(&s, validate_regexp(&s, options), flags),
        FILTER_SANITIZE_STRING => Value::str(sanitize_string(&s)),
        FILTER_SANITIZE_SPECIAL_CHARS => Value::str(sanitize_special_chars(&s)),
        FILTER_SANITIZE_FULL_SPECIAL_CHARS => Value::str(sanitize_full_special_chars(&s)),
        FILTER_SANITIZE_EMAIL => Value::str(sanitize_keep(&s, is_email_char)),
        FILTER_SANITIZE_URL => Value::str(sanitize_keep(&s, is_url_char)),
        FILTER_SANITIZE_NUMBER_INT => Value::str(sanitize_keep(&s, is_number_int_char)),
        FILTER_SANITIZE_NUMBER_FLOAT => Value::str(sanitize_number_float(&s, flags)),
        // FILTER_DEFAULT / FILTER_UNSAFE_RAW and anything unrecognized: no
        // filtering, the value as a string.
        _ => Value::str(s),
    }
}

/// Validation failure result: `null` under `FILTER_NULL_ON_FAILURE`, else `false`.
fn fail(flags: i64) -> Value {
    if flags & FLAG_NULL_ON_FAILURE != 0 {
        Value::Undef
    } else {
        Value::bool(false)
    }
}

/// Return the validated string on success, else the failure sentinel.
fn pass_or_fail(s: &str, ok: bool, flags: i64) -> Value {
    if ok {
        Value::str(s.to_string())
    } else {
        fail(flags)
    }
}

// ── validators ───────────────────────────────────────────────────────────────

/// `FILTER_VALIDATE_INT`: optional surrounding whitespace, optional sign, no
/// leading zeros (unless `ALLOW_OCTAL`/`ALLOW_HEX`), within `min_range`/`max_range`.
fn validate_int(s: &str, flags: i64, options: &Option<Value>) -> Option<i64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }

    let (neg, digits) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };

    let magnitude: i64 = if flags & FLAG_ALLOW_HEX != 0
        && (digits.starts_with("0x") || digits.starts_with("0X"))
    {
        i64::from_str_radix(&digits[2..], 16).ok()?
    } else if flags & FLAG_ALLOW_OCTAL != 0 && digits.starts_with('0') && digits.len() > 1 {
        i64::from_str_radix(&digits[1..], 8).ok()?
    } else {
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        // No leading zeros in decimal form (PHP rejects "007").
        if digits.len() > 1 && digits.starts_with('0') {
            return None;
        }
        digits.parse().ok()?
    };

    let value = if neg { magnitude.checked_neg()? } else { magnitude };

    if let Some(min) = opt_get(options, "min_range") {
        if value < min.to_int() {
            return None;
        }
    }
    if let Some(max) = opt_get(options, "max_range") {
        if value > max.to_int() {
            return None;
        }
    }
    Some(value)
}

/// `FILTER_VALIDATE_FLOAT`: optional surrounding whitespace, optional thousands
/// separators (with `ALLOW_THOUSAND`), within `min_range`/`max_range`.
fn validate_float(s: &str, flags: i64, options: &Option<Value>) -> Option<f64> {
    let mut t = s.trim().to_string();
    if t.is_empty() {
        return None;
    }
    if flags & FLAG_ALLOW_THOUSAND != 0 {
        t = t.replace(',', "");
    }
    // Rust's f64 parser accepts "inf"/"nan"/"1e3"; PHP's float filter rejects the
    // former two, so require the string to look numeric first.
    if !t
        .bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E'))
    {
        return None;
    }
    let value: f64 = t.parse().ok()?;
    if !value.is_finite() {
        return None;
    }
    if let Some(min) = opt_get(options, "min_range") {
        if value < min.to_float() {
            return None;
        }
    }
    if let Some(max) = opt_get(options, "max_range") {
        if value > max.to_float() {
            return None;
        }
    }
    Some(value)
}

/// `FILTER_VALIDATE_BOOLEAN`: `1/true/on/yes` → true, `0/false/off/no/""` → false,
/// anything else → `None` (the caller maps `None` to `false`/`null`).
fn validate_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Some(true),
        "0" | "false" | "off" | "no" | "" => Some(false),
        _ => None,
    }
}

static EMAIL_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)^[a-z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)+$",
    )
    .expect("email regex compiles")
});

/// `FILTER_VALIDATE_EMAIL`: RFC-ish local-part `@` dotted-domain.
fn validate_email(s: &str) -> bool {
    s.len() <= 320 && EMAIL_RE.is_match(s)
}

static URL_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)^[a-z][a-z0-9+.\-]*://([^\s/@]+@)?[^\s/:?#]+(:[0-9]+)?(/[^\s?#]*)?(\?[^\s#]*)?(#[^\s]*)?$",
    )
    .expect("url regex compiles")
});

/// `FILTER_VALIDATE_URL`: requires a scheme and a host (matches PHP rejecting
/// schemeless or hostless URLs).
fn validate_url(s: &str) -> bool {
    URL_RE.is_match(s)
}

/// `FILTER_VALIDATE_IP`: dotted-quad v4 and/or v6, gated by the IPV4/IPV6 flags.
fn validate_ip(s: &str, flags: i64) -> bool {
    let want4 = flags & FLAG_IPV4 != 0;
    let want6 = flags & FLAG_IPV6 != 0;
    let is4 = s.parse::<Ipv4Addr>().is_ok();
    let is6 = s.parse::<Ipv6Addr>().is_ok();
    match (want4, want6) {
        (true, false) => is4,
        (false, true) => is6,
        // Neither or both flags set → accept either family.
        _ => is4 || is6,
    }
}

static HOSTNAME_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)^(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)*[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$",
    )
    .expect("hostname regex compiles")
});

/// `FILTER_VALIDATE_DOMAIN`: a length check by default; with `FILTER_FLAG_HOSTNAME`
/// each label must be a legal RFC-1123 hostname label.
fn validate_domain(s: &str, flags: i64) -> bool {
    if s.is_empty() || s.len() > 253 {
        return false;
    }
    if flags & FLAG_HOSTNAME != 0 {
        return HOSTNAME_RE.is_match(s);
    }
    true
}

/// `FILTER_VALIDATE_REGEXP`: the value must match `options['regexp']`, a Perl-style
/// `/pattern/flags` string. Returns false when the option is missing or the
/// pattern fails to compile (Rust `regex` is a PCRE subset — no back-references).
fn validate_regexp(s: &str, options: &Option<Value>) -> bool {
    let Some(pat_val) = opt_get(options, "regexp") else {
        return false;
    };
    let pattern = with_host(|h| h.to_str(&pat_val));
    let Some(re) = compile_delimited(&pattern) else {
        return false;
    };
    re.is_match(s)
}

/// Compile a PHP-delimited regex (`/…/imsx`) into a Rust `Regex`. The first byte
/// is the delimiter; trailing letters after the closing delimiter are inline
/// flags mapped to `(?imsx)`. Unsupported flags are ignored.
fn compile_delimited(pattern: &str) -> Option<regex::Regex> {
    let bytes = pattern.as_bytes();
    if bytes.len() < 2 {
        return None;
    }
    let delim = bytes[0] as char;
    let close = if delim == '(' {
        ')'
    } else if delim == '{' {
        '}'
    } else if delim == '[' {
        ']'
    } else if delim == '<' {
        '>'
    } else {
        delim
    };
    let rest = &pattern[1..];
    let end = rest.rfind(close)?;
    let body = &rest[..end];
    let mods = &rest[end + 1..];
    let mut inline = String::new();
    for m in mods.chars() {
        if matches!(m, 'i' | 'm' | 's' | 'x') {
            inline.push(m);
        }
    }
    let full = if inline.is_empty() {
        body.to_string()
    } else {
        format!("(?{inline}){body}")
    };
    regex::Regex::new(&full).ok()
}

// ── sanitizers ───────────────────────────────────────────────────────────────

/// `FILTER_SANITIZE_STRING` (deprecated in PHP 8.1 but functional): strip HTML
/// tags, then encode single/double quotes to their numeric entities.
fn sanitize_string(s: &str) -> String {
    let stripped = strip_tags(s);
    let mut out = String::with_capacity(stripped.len());
    for c in stripped.chars() {
        match c {
            '"' => out.push_str("&#34;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// `FILTER_SANITIZE_SPECIAL_CHARS`: HTML-encode `& < > " '` to numeric entities.
fn sanitize_special_chars(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&#38;"),
            '<' => out.push_str("&#60;"),
            '>' => out.push_str("&#62;"),
            '"' => out.push_str("&#34;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// `FILTER_SANITIZE_FULL_SPECIAL_CHARS`: equivalent to `htmlspecialchars` with
/// `ENT_QUOTES` — `& < > " '` become named/numeric entities.
fn sanitize_full_special_chars(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#039;"),
            _ => out.push(c),
        }
    }
    out
}

/// `FILTER_SANITIZE_NUMBER_FLOAT`: keep digits and `+ -`; keep `.` with
/// `ALLOW_FRACTION`, `,` with `ALLOW_THOUSAND`, and `e E` with `ALLOW_SCIENTIFIC`.
fn sanitize_number_float(s: &str, flags: i64) -> String {
    let allow_frac = flags & FLAG_ALLOW_FRACTION != 0;
    let allow_thou = flags & FLAG_ALLOW_THOUSAND != 0;
    let allow_sci = flags & FLAG_ALLOW_SCIENTIFIC != 0;
    s.chars()
        .filter(|&c| {
            c.is_ascii_digit()
                || matches!(c, '+' | '-')
                || (allow_frac && c == '.')
                || (allow_thou && c == ',')
                || (allow_sci && matches!(c, 'e' | 'E'))
        })
        .collect()
}

/// Keep only the characters accepted by `keep`.
fn sanitize_keep(s: &str, keep: fn(char) -> bool) -> String {
    s.chars().filter(|&c| keep(c)).collect()
}

/// `FILTER_SANITIZE_EMAIL` allow-set.
fn is_email_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '!' | '#'
                | '$'
                | '%'
                | '&'
                | '\''
                | '*'
                | '+'
                | '-'
                | '='
                | '?'
                | '^'
                | '_'
                | '`'
                | '{'
                | '|'
                | '}'
                | '~'
                | '@'
                | '.'
                | '['
                | ']'
        )
}

/// `FILTER_SANITIZE_URL` allow-set.
fn is_url_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '$' | '-'
                | '_'
                | '.'
                | '+'
                | '!'
                | '*'
                | '\''
                | '('
                | ')'
                | ','
                | '{'
                | '}'
                | '|'
                | '\\'
                | '^'
                | '~'
                | '['
                | ']'
                | '`'
                | '<'
                | '>'
                | '#'
                | '%'
                | '"'
                | ';'
                | '/'
                | '?'
                | ':'
                | '@'
                | '&'
                | '='
        )
}

/// `FILTER_SANITIZE_NUMBER_INT` allow-set: digits and `+ -`.
fn is_number_int_char(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, '+' | '-')
}

/// Remove HTML/XML tags (`<…>`), like `strip_tags` with no allowlist.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

// ── small helpers ────────────────────────────────────────────────────────────

/// Build a PHP array from `(key, value)` pairs, preserving keys.
fn make_map(pairs: Vec<(Value, Value)>) -> Value {
    with_host(|h| {
        let arr = h.new_array();
        for (k, v) in pairs {
            h.arr_set_key(&arr, &k, v);
        }
        arr
    })
}

/// Compare two array keys by their PHP string form (keys may be int or string).
fn value_key_eq(a: &Value, b: &Value) -> bool {
    with_host(|h| h.to_str(a) == h.to_str(b))
}
