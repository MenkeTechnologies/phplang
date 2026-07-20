//! PHP standard-library `url` functions. Part of the `stdlib` chain; see
//! `src/stdlib/mod.rs`. `dispatch` returns `None` for names it does not handle.
//!
//! Covers percent-encoding (`urlencode`/`rawurlencode` and their inverses),
//! query-string assembly (`http_build_query`) and parsing (`parse_str`), and URL
//! decomposition (`parse_url`). The `parse_url` machine is a faithful port of
//! PHP's `ext/standard/url.c:php_url_parse_ex2`, so its edge cases (relative
//! schemes, `mailto:`, IPv6 hosts, `host:port` without a scheme, the `file://`
//! Windows-drive special case, and rejection of malformed authorities) match the
//! reference `php` byte-for-byte.

use crate::host::PhpHost;
use fusevm::Value;

/// Dispatch a `url`-category PHP function by lowercased name. Returns `None` for
/// names this module does not implement so the stdlib chain can continue.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let v = match name {
        "urlencode" => Value::str(urlencode(&str_of(args, 0))),
        "urldecode" => Value::str(percent_decode(&str_of(args, 0), true)),
        "rawurlencode" => Value::str(rawurlencode(&str_of(args, 0))),
        "rawurldecode" => Value::str(percent_decode(&str_of(args, 0), false)),
        "http_build_query" => http_build_query(args),
        "parse_url" => parse_url(args),
        "parse_str" => parse_str(args),
        _ => return None,
    };
    Some(Ok(v))
}

// ── argument helpers ───────────────────────────────────────────────────────

/// PHP string cast of the `i`-th argument (no reentrant `with_host` nesting: the
/// public builder functions grab the host once and route everything through it).
fn str_of(args: &[Value], i: usize) -> String {
    crate::host::with_host(|h| h.to_str(&args.get(i).cloned().unwrap_or(Value::Undef)))
}

// ── percent-encoding ───────────────────────────────────────────────────────

/// `A-Z a-z 0-9 - _ .` — unreserved under `application/x-www-form-urlencoded`.
fn is_unreserved_url(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')
}

/// `A-Z a-z 0-9 - _ . ~` — RFC 3986 unreserved (tilde kept, unlike `urlencode`).
fn is_unreserved_raw(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~')
}

/// Append the uppercase `%XX` escape for `b`.
fn push_pct(out: &mut String, b: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out.push('%');
    out.push(HEX[(b >> 4) as usize] as char);
    out.push(HEX[(b & 0xf) as usize] as char);
}

/// `urlencode`: RFC 1738, space becomes `+`, `~` is escaped.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if is_unreserved_url(b) {
            out.push(b as char);
        } else if b == b' ' {
            out.push('+');
        } else {
            push_pct(&mut out, b);
        }
    }
    out
}

/// `rawurlencode`: RFC 3986, space becomes `%20`, `~` is preserved.
fn rawurlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if is_unreserved_raw(b) {
            out.push(b as char);
        } else {
            push_pct(&mut out, b);
        }
    }
    out
}

/// Decode `%XX` escapes; when `plus_to_space` is set (`urldecode`), `+` maps to a
/// space (`rawurldecode` leaves `+` literal). A `%` not followed by two hex
/// digits is emitted verbatim, matching PHP.
fn percent_decode(s: &str, plus_to_space: bool) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' if plus_to_space => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < b.len() => {
                match (hex_val(b[i + 1]), hex_val(b[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push((h << 4) | l);
                        i += 3;
                    }
                    _ => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Hex digit value for one ASCII byte, or `None` if not `[0-9A-Fa-f]`.
fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ── http_build_query ───────────────────────────────────────────────────────

/// `http_build_query($data, $numeric_prefix = "", $arg_separator = "&",
/// $encoding_type = PHP_QUERY_RFC1738)`. Recursively serializes an array/object
/// into `k=v&k2=v2`, url-encoding keys (brackets and all) and values. Integer
/// keys at the top level receive `$numeric_prefix`; `null` values are skipped;
/// booleans serialize as `1`/`0`. `PHP_QUERY_RFC3986` (2) switches to
/// `rawurlencode`. Returns `false` when `$data` is not an array/object.
fn http_build_query(args: &[Value]) -> Value {
    crate::host::with_host(|h| {
        let data = args.first().cloned().unwrap_or(Value::Undef);
        let Some(pairs) = h.array_pairs(&data) else {
            return Value::bool(false);
        };
        let prefix = args
            .get(1)
            .map(|v| h.to_str(v))
            .filter(|s| !s.is_empty())
            .unwrap_or_default();
        let sep = args
            .get(2)
            .filter(|v| !matches!(v, Value::Undef))
            .map(|v| h.to_str(v))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "&".into());
        let raw = args.get(3).map(|v| h.to_number(v).to_int()).unwrap_or(1) == 2;

        let mut parts: Vec<String> = Vec::new();
        for (k, v) in pairs {
            let key = match &k {
                Value::Int(n) => format!("{prefix}{n}"),
                other => h.to_str(other),
            };
            build_query_pair(h, &key, &v, raw, &mut parts);
        }
        Value::str(parts.join(&sep))
    })
}

/// Emit one leaf (`key=value`) or recurse into a nested array building
/// `key[sub]` segments. `null` leaves are dropped entirely.
fn build_query_pair(h: &mut PhpHost, key: &str, val: &Value, raw: bool, out: &mut Vec<String>) {
    if let Some(pairs) = h.array_pairs(val) {
        for (k, v) in pairs {
            let sub = format!("{key}[{}]", h.to_str(&k));
            build_query_pair(h, &sub, &v, raw, out);
        }
        return;
    }
    let value = match val {
        Value::Undef => return, // null is skipped
        Value::Bool(true) => "1".to_string(),
        Value::Bool(false) => "0".to_string(),
        other => h.to_str(other),
    };
    let enc = if raw { rawurlencode } else { urlencode };
    out.push(format!("{}={}", enc(key), enc(&value)));
}

// ── parse_str ──────────────────────────────────────────────────────────────

/// `parse_str($string)` — parse a query string into an array. PHP fills a
/// by-reference variable; here the array is returned directly. Handles `key[]`
/// appends, `key[sub]` nesting, `+`/`%XX` decoding, and the top-level key
/// mangling that turns `.` and ` ` into `_`.
fn parse_str(args: &[Value]) -> Value {
    crate::host::with_host(|h| {
        let s = h.to_str(&args.first().cloned().unwrap_or(Value::Undef));
        let root = h.new_array();
        for pair in s.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (raw_name, raw_val) = match pair.split_once('=') {
                Some((n, v)) => (n, v),
                None => (pair, ""),
            };
            let name = percent_decode(raw_name, true);
            let value = Value::str(percent_decode(raw_val, true));

            // Split base name from any `[...]` segments.
            let (base_raw, segments) = split_key(&name);
            if base_raw.is_empty() {
                continue;
            }
            let base = mangle_base(base_raw);

            if segments.is_empty() {
                h.arr_set_key(&root, &Value::str(base), value);
                continue;
            }
            // Descend/vivify from root[base], then set/append the final segment.
            let mut container = ensure_child_array(h, &root, &Value::str(base));
            for (i, seg) in segments.iter().enumerate() {
                let last = i == segments.len() - 1;
                match (seg, last) {
                    (Some(k), true) => h.arr_set_key(&container, &Value::str(k.clone()), value.clone()),
                    (None, true) => h.arr_push_auto(&container, value.clone()),
                    (Some(k), false) => {
                        container = ensure_child_array(h, &container, &Value::str(k.clone()))
                    }
                    (None, false) => {
                        let child = h.new_array();
                        h.arr_push_auto(&container, child.clone());
                        container = child;
                    }
                }
            }
        }
        root
    })
}

/// Return `arr[key]` if it is already an array, else create, store, and return a
/// fresh empty array (PHP's auto-vivification for nested query keys).
fn ensure_child_array(h: &mut PhpHost, arr: &Value, key: &Value) -> Value {
    let cur = h.index_get(arr, key);
    if h.is_array(&cur) {
        return cur;
    }
    let child = h.new_array();
    h.arr_set_key(arr, key, child.clone());
    child
}

/// Split a decoded key into `(base, segments)` where each segment is `Some(k)`
/// for `[k]` or `None` for an empty `[]` append. Bracket contents are not
/// mangled; a malformed (unclosed) bracket stops the scan.
fn split_key(name: &str) -> (&str, Vec<Option<String>>) {
    let Some(open) = name.find('[') else {
        return (name, Vec::new());
    };
    let base = &name[..open];
    let mut rest = &name[open..];
    let mut segs = Vec::new();
    while let Some(stripped) = rest.strip_prefix('[') {
        let Some(close) = stripped.find(']') else {
            break;
        };
        let seg = &stripped[..close];
        segs.push(if seg.is_empty() {
            None
        } else {
            Some(seg.to_string())
        });
        rest = &stripped[close + 1..];
    }
    (base, segs)
}

/// Top-level `parse_str` key mangling: `.` and ` ` become `_`.
fn mangle_base(base: &str) -> String {
    base.replace([' ', '.'], "_")
}

// ── parse_url ──────────────────────────────────────────────────────────────

/// Optional components of a parsed URL. Absent components stay `None`.
#[derive(Default)]
struct UrlParts {
    scheme: Option<String>,
    host: Option<String>,
    port: Option<i64>,
    user: Option<String>,
    pass: Option<String>,
    path: Option<String>,
    query: Option<String>,
    fragment: Option<String>,
}

/// `parse_url($url, $component = -1)`. With no component (or `-1`) returns an
/// assoc array holding every found component; with a `PHP_URL_*` constant
/// returns just that piece (an int for the port). Malformed URLs yield `false`.
fn parse_url(args: &[Value]) -> Value {
    let url = str_of(args, 0);
    let component = args
        .get(1)
        .map(|v| crate::host::with_host(|h| h.to_number(v).to_int()))
        .unwrap_or(-1);

    let Some(p) = php_url_parse(&url) else {
        return Value::bool(false);
    };

    if component >= 0 {
        // PHP_URL_SCHEME=0 HOST=1 PORT=2 USER=3 PASS=4 PATH=5 QUERY=6 FRAGMENT=7
        return match component {
            0 => opt_str(p.scheme),
            1 => opt_str(p.host),
            2 => p.port.map(Value::int).unwrap_or(Value::Undef),
            3 => opt_str(p.user),
            4 => opt_str(p.pass),
            5 => opt_str(p.path),
            6 => opt_str(p.query),
            7 => opt_str(p.fragment),
            _ => Value::Undef,
        };
    }

    crate::host::with_host(|h| {
        let arr = h.new_array();
        let mut set = |k: &str, v: Value| h.arr_set_key(&arr, &Value::str(k.to_string()), v);
        // Order matches PHP's output: scheme, host, port, user, pass, path,
        // query, fragment.
        if let Some(v) = p.scheme {
            set("scheme", Value::str(v));
        }
        if let Some(v) = p.host {
            set("host", Value::str(v));
        }
        if let Some(v) = p.port {
            set("port", Value::int(v));
        }
        if let Some(v) = p.user {
            set("user", Value::str(v));
        }
        if let Some(v) = p.pass {
            set("pass", Value::str(v));
        }
        if let Some(v) = p.path {
            set("path", Value::str(v));
        }
        if let Some(v) = p.query {
            set("query", Value::str(v));
        }
        if let Some(v) = p.fragment {
            set("fragment", Value::str(v));
        }
        arr
    })
}

/// `Value::str` for `Some`, PHP `null` (`Undef`) for `None`.
fn opt_str(s: Option<String>) -> Value {
    s.map(Value::str).unwrap_or(Value::Undef)
}

/// Replace ASCII control bytes (`< 0x20` or `0x7F`) with `_`, mirroring PHP's
/// `php_replace_controlchars`, then decode as UTF-8 (URLs are byte strings).
fn replace_ctrl(bytes: &[u8]) -> String {
    let mapped: Vec<u8> = bytes
        .iter()
        .map(|&c| if c < 0x20 || c == 0x7f { b'_' } else { c })
        .collect();
    String::from_utf8_lossy(&mapped).into_owned()
}

/// First index of `ch` in `b[start..end]`, else `None`.
fn memchr(b: &[u8], start: usize, end: usize, ch: u8) -> Option<usize> {
    (start..end).find(|&i| b[i] == ch)
}

/// Last index of `ch` in `b[start..end]`, else `None`.
fn memrchr(b: &[u8], start: usize, end: usize, ch: u8) -> Option<usize> {
    (start..end).rev().find(|&i| b[i] == ch)
}

/// Index of the first byte in `set` within `b[start..end]`, else `end` (the C
/// `strcspn` idiom used by PHP's parser to bound authorities and paths).
fn binary_strcspn(b: &[u8], start: usize, end: usize, set: &[u8]) -> usize {
    (start..end)
        .find(|&i| set.contains(&b[i]))
        .unwrap_or(end)
}

/// Parse an all-digits slice into an `i64`, or `None` if empty/non-digit.
fn parse_digits(slice: &[u8]) -> Option<i64> {
    if slice.is_empty() || !slice.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(slice).ok()?.parse().ok()
}

/// Stages of the port. Mirrors the `parse_port`/`parse_host`/`just_path` labels
/// in PHP's `php_url_parse_ex2`.
enum Stage {
    ParsePort,
    ParseHost,
    JustPath,
}

/// Faithful port of `ext/standard/url.c:php_url_parse_ex2`. Returns `None` for
/// the inputs PHP rejects as `false` (bad/empty authority, out-of-range port).
fn php_url_parse(input: &str) -> Option<UrlParts> {
    let b = input.as_bytes();
    let ue = b.len();
    let mut r = UrlParts::default();
    let mut s: usize = 0;
    let mut e: usize = 0; // colon index carried into ParsePort
    let stage: Stage;

    match memchr(b, 0, ue, b':') {
        Some(colon) if colon != 0 => {
            e = colon;
            // Validate scheme = 1*( alpha | digit | "+" | "-" | "." ).
            let mut jumped: Option<Stage> = None;
            let mut p = 0;
            while p < e {
                let c = b[p];
                if !(c.is_ascii_alphanumeric() || c == b'+' || c == b'.' || c == b'-') {
                    if e + 1 < ue && e < binary_strcspn(b, 0, ue, b"?#") {
                        jumped = Some(Stage::ParsePort);
                    } else if 1 < ue && b[0] == b'/' && b[1] == b'/' {
                        s = 2;
                        jumped = Some(Stage::ParseHost);
                    } else {
                        jumped = Some(Stage::JustPath);
                    }
                    break;
                }
                p += 1;
            }
            match jumped {
                Some(st) => stage = st,
                None => {
                    if e + 1 == ue {
                        // Only a scheme is present ("mailto:").
                        r.scheme = Some(replace_ctrl(&b[0..e]));
                        return Some(r);
                    }
                    if b[e + 1] != b'/' {
                        // Schemeless "host:port" masquerading, or a "zlib:"-style
                        // scheme with a non-`//` path.
                        let mut p2 = e + 1;
                        while p2 < ue && b[p2].is_ascii_digit() {
                            p2 += 1;
                        }
                        if (p2 == ue || b[p2] == b'/') && (p2 - e) < 7 {
                            stage = Stage::ParsePort;
                        } else {
                            r.scheme = Some(replace_ctrl(&b[0..e]));
                            s = e + 1;
                            stage = Stage::JustPath;
                        }
                    } else {
                        r.scheme = Some(replace_ctrl(&b[0..e]));
                        if e + 2 < ue && b[e + 2] == b'/' {
                            s = e + 3;
                            let is_file = r
                                .scheme
                                .as_deref()
                                .is_some_and(|x| x.eq_ignore_ascii_case("file"));
                            if is_file && e + 3 < ue && b[e + 3] == b'/' {
                                // file:///c:/... — keep a leading drive letter.
                                if e + 5 < ue && b[e + 5] == b':' {
                                    s = e + 4;
                                }
                                stage = Stage::JustPath;
                            } else {
                                stage = Stage::ParseHost;
                            }
                        } else {
                            s = e + 1;
                            stage = Stage::JustPath;
                        }
                    }
                }
            }
        }
        Some(_) => {
            // Colon at position 0 — treat what follows as a port.
            e = 0;
            stage = Stage::ParsePort;
        }
        None => {
            if 1 < ue && b[0] == b'/' && b[1] == b'/' {
                s = 2;
                stage = Stage::ParseHost;
            } else {
                stage = Stage::JustPath;
            }
        }
    }

    let mut stage = stage;
    loop {
        match stage {
            Stage::ParsePort => {
                let p = e + 1;
                let mut pp = p;
                while pp < ue && pp - p < 6 && b[pp].is_ascii_digit() {
                    pp += 1;
                }
                if pp - p > 0 && pp - p < 6 && (pp == ue || b[pp] == b'/') {
                    match parse_digits(&b[p..pp]) {
                        Some(port) if (0..=65535).contains(&port) => {
                            r.port = Some(port);
                            if s + 1 < ue && b[s] == b'/' && b[s + 1] == b'/' {
                                s += 2;
                            }
                        }
                        _ => return None,
                    }
                    stage = Stage::ParseHost;
                } else if p == pp && pp == ue {
                    return None;
                } else if s + 1 < ue && b[s] == b'/' && b[s + 1] == b'/' {
                    s += 2;
                    stage = Stage::ParseHost;
                } else {
                    stage = Stage::JustPath;
                }
            }
            Stage::ParseHost => {
                e = binary_strcspn(b, s, ue, b"/?#");

                // Optional user[:pass]@ credentials.
                if let Some(at) = memrchr(b, s, e, b'@') {
                    if let Some(col) = memchr(b, s, at, b':') {
                        r.user = Some(replace_ctrl(&b[s..col]));
                        r.pass = Some(replace_ctrl(&b[col + 1..at]));
                    } else {
                        r.user = Some(replace_ctrl(&b[s..at]));
                    }
                    s = at + 1;
                }

                // Optional :port — skipped for `[IPv6]` literals.
                let pcol = if s < ue && b[s] == b'[' && e > 0 && b[e - 1] == b']' {
                    None
                } else {
                    memrchr(b, s, e, b':')
                };

                let host_end = if let Some(mut pidx) = pcol {
                    if r.port.is_none() {
                        pidx += 1;
                        if e.saturating_sub(pidx) > 5 {
                            return None;
                        } else if e > pidx {
                            match parse_digits(&b[pidx..e]) {
                                Some(port) if (0..=65535).contains(&port) => r.port = Some(port),
                                _ => return None,
                            }
                        }
                        pidx -= 1;
                    }
                    pidx
                } else {
                    e
                };

                if host_end <= s {
                    return None; // empty host — reject the URL
                }
                r.host = Some(replace_ctrl(&b[s..host_end]));

                if e == ue {
                    return Some(r);
                }
                s = e;
                stage = Stage::JustPath;
            }
            Stage::JustPath => {
                let mut end = ue;
                if let Some(hash) = memchr(b, s, end, b'#') {
                    r.fragment = Some(if hash + 1 < end {
                        replace_ctrl(&b[hash + 1..end])
                    } else {
                        String::new()
                    });
                    end = hash;
                }
                if let Some(q) = memchr(b, s, end, b'?') {
                    r.query = Some(if q + 1 < end {
                        replace_ctrl(&b[q + 1..end])
                    } else {
                        String::new()
                    });
                    end = q;
                }
                if s < end || s == ue {
                    r.path = Some(replace_ctrl(&b[s..end]));
                }
                return Some(r);
            }
        }
    }
}
