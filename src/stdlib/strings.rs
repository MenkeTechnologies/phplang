//! PHP standard-library `strings` functions. Part of the `stdlib` chain; see
//! `src/stdlib/mod.rs`. `dispatch` returns `None` for names it does not handle.
//!
//! Everything the interpreter core already provides (`strlen`, `substr`,
//! `str_replace`, `sprintf`, …) is intentionally absent here — the core match in
//! `builtins::call_library` wins for those names and never consults this module.
//! What remains are the second-tier string helpers PHP scripts reach for:
//! searching (`strstr`/`strpos` siblings), translation (`strtr`), escaping
//! (`addslashes`/`quotemeta`), distance metrics (`levenshtein`/`similar_text`),
//! and the multibyte (`mb_*`) codepoint-aware variants.
//!
//! ASCII note: PHP's non-`mb_` string functions operate on bytes. Positions and
//! slices below use byte offsets to match PHP exactly; for the ASCII inputs these
//! functions almost always see, byte and codepoint indices coincide. The `mb_*`
//! family is explicitly codepoint-aware.

use crate::host::with_host;
use crate::stdlib::common::*;
use fusevm::Value;

/// Dispatch a `strings`-category PHP function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let v = match name {
        "substr_count" => return Some(substr_count(args)),
        "substr_replace" => return Some(substr_replace(args)),
        "strtr" => strtr(args),
        "strstr" | "strchr" => strstr(args, false),
        "stristr" => strstr(args, true),
        "strrchr" => strrchr(args),
        "strpbrk" => strpbrk(args),
        "strspn" => strspn(args, true),
        "strcspn" => strspn(args, false),
        "stripos" => return Some(strpos_ci(args)),
        "strrpos" => return Some(strrpos(args, false)),
        "strripos" => return Some(strrpos(args, true)),
        "strncasecmp" => return Some(strncasecmp(args)),
        "str_ireplace" => str_ireplace(args),
        "nl2br" => Value::str(nl2br(&str_arg(args, 0))),
        "chunk_split" => return Some(chunk_split(args)),
        "quotemeta" => Value::str(quotemeta(&str_arg(args, 0))),
        "addslashes" => Value::str(addslashes(&str_arg(args, 0))),
        "stripslashes" => Value::str(stripslashes(&str_arg(args, 0))),
        "str_rot13" => Value::str(str_rot13(&str_arg(args, 0))),
        "addcslashes" => addcslashes(args),
        "stripcslashes" => stripcslashes(args),
        "count_chars" => return Some(count_chars(args)),
        "strtok" => strtok(args),
        "similar_text" => {
            let (a, b) = (str_arg(args, 0), str_arg(args, 1));
            let common = similar_text_bytes(a.as_bytes(), b.as_bytes());
            // `$percent` is a by-reference OUT parameter. PHP computes it as the
            // single expression `sim * 200.0 / (len(a) + len(b))`; splitting that
            // into a divide and a multiply rounds differently in the last bit.
            let total = a.len() + b.len();
            let percent = if total == 0 {
                0.0
            } else {
                common as f64 * 200.0 / total as f64
            };
            with_host(|h| h.byref_out_put(2, Value::float(percent)));
            Value::int(common as i64)
        }
        "levenshtein" => levenshtein(args),
        "vsprintf" => return Some(vformat(args, false)),
        "vprintf" => return Some(vformat(args, true)),
        "sscanf" => return Some(sscanf(args)),
        "htmlspecialchars_decode" | "html_entity_decode" => {
            let flags = match args.get(1) {
                Some(v) if !matches!(v, Value::Undef) => with_host(|h| h.to_number(v).to_int()),
                _ => crate::stdlib::textx::ENT_DEFAULT,
            };
            let named = name == "html_entity_decode";
            Value::str(crate::stdlib::textx::html_decode(
                &str_arg(args, 0),
                flags,
                named,
            ))
        }
        "strip_tags" => strip_tags(args),
        "mb_strlen" => Value::int(str_arg(args, 0).chars().count() as i64),
        "mb_strtoupper" => Value::str(str_arg(args, 0).to_uppercase()),
        "mb_strtolower" => Value::str(str_arg(args, 0).to_lowercase()),
        "mb_substr" => Value::str(mb_substr(args)),
        _ => return None,
    };
    Some(Ok(v))
}

// ── searching / positions ────────────────────────────────────────────────────

/// `substr_count($haystack, $needle, $offset = 0, $length = null)`.
fn substr_count(args: &[Value]) -> Result<Value, String> {
    let hay = str_arg(args, 0);
    let needle = str_arg(args, 1);
    if needle.is_empty() {
        return Err(throws(
            "ValueError",
            "substr_count(): Argument #2 ($needle) must not be empty",
        ));
    }
    let bytes = hay.as_bytes();
    let len = bytes.len() as i64;
    let mut start = int_arg(args, 2);
    if start < 0 {
        start = (len + start).max(0);
    }
    let start = start.clamp(0, len) as usize;
    let end = match args.get(3) {
        Some(v) if !matches!(v, Value::Undef) => {
            let l = v.to_int();
            if l < 0 {
                ((len + l).max(start as i64)) as usize
            } else {
                (start + l as usize).min(bytes.len())
            }
        }
        _ => bytes.len(),
    };
    // PHP strings are byte-oriented; count non-overlapping needle matches over
    // the byte window so multibyte haystacks never slice mid-UTF-8-char.
    let slice = &bytes[start..end];
    let nb = needle.as_bytes();
    let mut count = 0i64;
    let mut i = 0;
    while i + nb.len() <= slice.len() {
        if &slice[i..i + nb.len()] == nb {
            count += 1;
            i += nb.len();
        } else {
            i += 1;
        }
    }
    Ok(Value::int(count))
}

/// `strstr`/`stristr`: portion of haystack from the first match of needle. When
/// `before_needle` (3rd arg) is truthy, the portion *before* the match instead.
/// Returns `false` when not found. `ci` selects the case-insensitive variant.
fn strstr(args: &[Value], ci: bool) -> Value {
    let hay = str_arg(args, 0);
    let needle = str_arg(args, 1);
    // PHP 8: an empty needle matches at position 0, so strstr returns the whole
    // haystack (and the empty string when $before_needle is set). `find("")` and
    // `ci_find` both yield `Some(0)` here, so no special-case is needed.
    let before = args.get(2).map(is_truthy).unwrap_or(false);
    let found = if ci {
        ci_find(&hay, &needle)
    } else {
        hay.find(&needle)
    };
    match found {
        Some(i) if before => Value::str(hay[..i].to_string()),
        Some(i) => Value::str(hay[i..].to_string()),
        None => Value::bool(false),
    }
}

/// `strrchr($haystack, $needle)`: from the LAST occurrence of `$needle`'s first
/// byte to the end of the haystack, or `false`.
fn strrchr(args: &[Value]) -> Value {
    let hay = str_arg(args, 0);
    let needle = str_arg(args, 1);
    let Some(&c) = needle.as_bytes().first() else {
        return Value::bool(false);
    };
    match hay.bytes().rposition(|b| b == c) {
        Some(i) => Value::str(hay[i..].to_string()),
        None => Value::bool(false),
    }
}

/// `strpbrk($string, $characters)`: from the first byte of `$string` present in
/// `$characters` to the end, or `false`.
fn strpbrk(args: &[Value]) -> Value {
    let s = str_arg(args, 0);
    let set = str_arg(args, 1);
    // The search is over BYTES, so a match can land inside a multi-byte
    // character — `strpbrk("é", "©")` matches the shared trailing 0xA9. Slicing
    // the `&str` at that index panics, so cut the byte vector instead and let
    // the lossy conversion produce what the reference's byte string shows.
    match s.bytes().position(|b| set.as_bytes().contains(&b)) {
        Some(i) => Value::str(String::from_utf8_lossy(&s.as_bytes()[i..]).into_owned()),
        None => Value::bool(false),
    }
}

/// `strspn`/`strcspn($subject, $mask, $start = 0, $length = null)`: length of the
/// initial segment of `$subject` consisting entirely of (`want=true`, `strspn`)
/// or entirely NOT of (`want=false`, `strcspn`) bytes in `$mask`.
fn strspn(args: &[Value], want: bool) -> Value {
    let subject = str_arg(args, 0);
    let mask = str_arg(args, 1);
    let bytes = subject.as_bytes();
    let len = bytes.len() as i64;
    let mut start = int_arg(args, 2);
    if start < 0 {
        start = (len + start).max(0);
    }
    let start = start.clamp(0, len) as usize;
    let end = match args.get(3) {
        Some(v) if !matches!(v, Value::Undef) => {
            let l = v.to_int();
            if l < 0 {
                ((len + l).max(start as i64)) as usize
            } else {
                (start + l as usize).min(bytes.len())
            }
        }
        _ => bytes.len(),
    };
    let n = bytes[start..end]
        .iter()
        .take_while(|b| mask.as_bytes().contains(b) == want)
        .count();
    Value::int(n as i64)
}

/// The `$offset` shared by `strpos`/`stripos`/`strrpos`/`strripos`, validated.
///
/// An offset outside `[-strlen, strlen]` is a ValueError naming the calling
/// function — the reference distinguishes "looked and did not find" (false) from
/// "you cannot look there" (a throw), and clamping silently answered `false` for
/// both.
fn checked_offset(func: &str, hay_len: usize, raw: i64) -> Result<i64, String> {
    let len = hay_len as i64;
    let start = if raw < 0 { len + raw } else { raw };
    if start < 0 || start > len {
        return Err(throws(
            "ValueError",
            format!("{func}(): Argument #3 ($offset) must be contained in argument #1 ($haystack)"),
        ));
    }
    Ok(start)
}

/// `stripos($haystack, $needle, $offset = 0)`: case-insensitive `strpos`.
fn strpos_ci(args: &[Value]) -> Result<Value, String> {
    let hay = str_arg(args, 0);
    let needle = str_arg(args, 1);
    let off = checked_offset("stripos", hay.len(), int_arg(args, 2))? as usize;
    // Byte-oriented search: slice the byte view so a multibyte offset can't panic.
    Ok(
        match ci_find_bytes(&hay.as_bytes()[off..], needle.as_bytes()) {
            Some(i) => Value::int((off + i) as i64),
            None => Value::bool(false),
        },
    )
}

/// `strrpos`/`strripos($haystack, $needle, $offset = 0)`: last occurrence. `ci`
/// selects the case-insensitive `strripos`. Positive offset limits the search to
/// start at that byte; negative offset stops the search that many bytes from the
/// end. Returns `false` when not found.
fn strrpos(args: &[Value], ci: bool) -> Result<Value, String> {
    let hay = str_arg(args, 0);
    let needle = str_arg(args, 1);
    let len = hay.len();
    let func = if ci { "strripos" } else { "strrpos" };
    // Validated BEFORE the empty-needle shortcut: `strrpos("abc", "", 10)` is an
    // offset error, not `strlen($haystack)`.
    let off = int_arg(args, 2);
    checked_offset(func, len, off)?;
    // PHP 8: an empty needle yields strlen($haystack), not false.
    if needle.is_empty() {
        return Ok(Value::int(len as i64));
    }
    // Determine the inclusive window [lo, hi] where a match may START.
    let (lo, hi) = if off >= 0 {
        (off as usize, len.saturating_sub(needle.len()))
    } else {
        // Negative offset: the match may start no later than len+off (PHP's
        // stop-this-many-bytes-from-the-end semantics), independent of needle len.
        let hi = (len as i64 + off).max(0) as usize;
        (0, hi.min(len.saturating_sub(needle.len())))
    };
    if lo > len {
        return Ok(Value::bool(false));
    }
    let eq = |i: usize| {
        let w = &hay.as_bytes()[i..i + needle.len()];
        if ci {
            w.eq_ignore_ascii_case(needle.as_bytes())
        } else {
            w == needle.as_bytes()
        }
    };
    Ok(
        match (lo..=hi).rev().find(|&i| i + needle.len() <= len && eq(i)) {
            Some(i) => Value::int(i as i64),
            None => Value::bool(false),
        },
    )
}

// ── comparison ───────────────────────────────────────────────────────────────

/// `strncasecmp($s1, $s2, $n)`: case-insensitive compare of the first `$n` bytes.
/// PHP 8 raises a `ValueError` when `$length` is negative rather than coercing it.
fn strncasecmp(args: &[Value]) -> Result<Value, String> {
    let a = str_arg(args, 0);
    let b = str_arg(args, 1);
    let n_raw = int_arg(args, 2);
    if n_raw < 0 {
        return Err(throws(
            "ValueError",
            "strncasecmp(): Argument #3 ($length) must be greater than or equal to 0",
        ));
    }
    let n = n_raw as usize;
    let ab = &a.as_bytes()[..n.min(a.len())];
    let bb = &b.as_bytes()[..n.min(b.len())];
    let la: Vec<u8> = ab.iter().map(|c| c.to_ascii_lowercase()).collect();
    let lb: Vec<u8> = bb.iter().map(|c| c.to_ascii_lowercase()).collect();
    // PHP returns `memcmp`'s value: the difference of the first differing byte,
    // not its sign. See `builtins::binary_strcmp`.
    Ok(Value::int(crate::builtins::binary_strcmp(&la, &lb)))
}

// ── translation / replacement ────────────────────────────────────────────────

/// `strtr`: 3-arg `strtr($str, $from, $to)` byte-wise translation (both truncated
/// to the shorter length), or 2-arg `strtr($str, $pairs)` longest-match,
/// non-overlapping, single-pass substring replacement.
fn strtr(args: &[Value]) -> Value {
    let s = str_arg(args, 0);
    if args.len() >= 3 && !matches!(arg(args, 2), Value::Undef) {
        let from = str_arg(args, 1);
        let to = str_arg(args, 2);
        let n = from.len().min(to.len());
        let (fb, tb) = (from.as_bytes(), to.as_bytes());
        let out: Vec<u8> = s
            .bytes()
            .map(|b| {
                fb[..n]
                    .iter()
                    .position(|&x| x == b)
                    .map(|idx| tb[idx])
                    .unwrap_or(b)
            })
            .collect();
        return Value::str(String::from_utf8_lossy(&out).into_owned());
    }
    // 2-arg map form.
    let mut pairs = with_host(|h| h.array_pairs(&arg(args, 1)).unwrap_or_default())
        .into_iter()
        .map(|(k, v)| (with_host(|h| h.to_str(&k)), with_host(|h| h.to_str(&v))))
        .filter(|(k, _)| !k.is_empty())
        .collect::<Vec<_>>();
    // Longest keys win at each position.
    pairs.sort_by_key(|p| std::cmp::Reverse(p.0.len()));
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    'outer: while i < bytes.len() {
        for (k, v) in &pairs {
            if bytes[i..].starts_with(k.as_bytes()) {
                out.push_str(v);
                i += k.len();
                continue 'outer;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    Value::str(out)
}

/// One splice: replace `l` bytes of `s` starting at `f` with `replace`, where
/// `f` and `l` have already been clamped by [`substr_replace_bounds`].
fn substr_splice(s: &str, replace: &str, f: usize, l: usize) -> String {
    let sb = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(sb.len() - l + replace.len());
    out.extend_from_slice(&sb[..f]);
    out.extend_from_slice(replace.as_bytes());
    out.extend_from_slice(&sb[f + l..]);
    String::from_utf8_lossy(&out).into_owned()
}

/// Clamp a raw `$offset`/`$length` pair against a subject length, exactly as
/// `PHP_FUNCTION(substr_replace)` does (php-src 8.5
/// `ext/standard/string.c:2366`): a negative offset counts from the end, a
/// negative length stops that many bytes from the end, and the pair is finally
/// trimmed so `f + l` never runs past the subject.
fn substr_replace_bounds(slen: usize, from: i64, len: i64) -> (usize, usize) {
    let n = slen as i64;
    let f = if from < 0 {
        (n + from).max(0)
    } else {
        from.min(n)
    };
    let mut l = if len < 0 { ((n - f) + len).max(0) } else { len };
    if l > n {
        l = n;
    }
    if f + l > n {
        l = n - f;
    }
    (f as usize, l as usize)
}

/// `substr_replace($string, $replace, $offset, $length = null)`.
///
/// Every one of the four parameters may be an array. An array `$string` makes
/// the result an array spliced element-wise, with `$replace`, `$offset` and
/// `$length` each consumed positionally — one entry per subject, falling back to
/// `""` / `0` / "to the end" once that array is exhausted.
fn substr_replace(args: &[Value]) -> Result<Value, String> {
    let subject = arg(args, 0);
    let repl_v = arg(args, 1);
    let from_v = arg(args, 2);
    let len_v = arg(args, 3);
    let len_is_null = args.len() < 4 || matches!(len_v, Value::Undef);
    let (is_arr_subj, is_arr_repl, is_arr_from, is_arr_len) = with_host(|h| {
        (
            h.is_array(&subject),
            h.is_array(&repl_v),
            h.is_array(&from_v),
            h.is_array(&len_v),
        )
    });
    let seq = |v: &Value| -> Vec<Value> {
        with_host(|h| {
            h.array_pairs(v)
                .unwrap_or_default()
                .into_iter()
                .map(|(_, val)| val)
                .collect()
        })
    };
    let as_int = |v: &Value| with_host(|h| h.to_number(v).to_int());

    if !is_arr_subj {
        let s = str_arg(args, 0);
        // An array offset or length against a SINGLE string is rejected outright
        // (php-src `ext/standard/string.c:2352`), even though an array $replace
        // in the same position is accepted and read for its first element.
        if is_arr_from {
            return Err(throws(
                "TypeError",
                "substr_replace(): Argument #3 ($offset) cannot be an array when working on a single string",
            ));
        }
        if is_arr_len {
            return Err(throws(
                "TypeError",
                "substr_replace(): Argument #4 ($length) cannot be an array when working on a single string",
            ));
        }
        let replace = if is_arr_repl {
            seq(&repl_v)
                .first()
                .map(|v| with_host(|h| h.to_str(v)))
                .unwrap_or_default()
        } else {
            str_arg(args, 1)
        };
        let l = if len_is_null {
            s.len() as i64
        } else {
            int_arg(args, 3)
        };
        let (f, l) = substr_replace_bounds(s.len(), int_arg(args, 2), l);
        return Ok(Value::str(substr_splice(&s, &replace, f, l)));
    }

    let repls = if is_arr_repl {
        seq(&repl_v)
    } else {
        Vec::new()
    };
    let froms = if is_arr_from {
        seq(&from_v)
    } else {
        Vec::new()
    };
    let lens = if is_arr_len { seq(&len_v) } else { Vec::new() };
    let pairs = with_host(|h| h.array_pairs(&subject).unwrap_or_default());
    let mut out: Vec<(Value, Value)> = Vec::with_capacity(pairs.len());
    for (i, (key, val)) in pairs.into_iter().enumerate() {
        let s = with_host(|h| h.to_str(&val));
        let raw_from = if is_arr_from {
            froms.get(i).map(&as_int).unwrap_or(0)
        } else {
            as_int(&from_v)
        };
        let raw_len = if is_arr_len {
            lens.get(i).map(|v| v.to_int()).unwrap_or(s.len() as i64)
        } else if !len_is_null {
            as_int(&len_v)
        } else {
            s.len() as i64
        };
        let (f, l) = substr_replace_bounds(s.len(), raw_from, raw_len);
        let replace = if is_arr_repl {
            repls
                .get(i)
                .map(|v| with_host(|h| h.to_str(v)))
                .unwrap_or_default()
        } else {
            with_host(|h| h.to_str(&repl_v))
        };
        out.push((key, Value::str(substr_splice(&s, &replace, f, l))));
    }
    Ok(make_map(out))
}

/// `str_ireplace($search, $replace, $subject)`: case-insensitive `str_replace`.
/// Supports array `$search` (with array or scalar `$replace`); scalar `$subject`.
fn str_ireplace(args: &[Value]) -> Value {
    let subject = str_arg(args, 2);
    let search_v = arg(args, 0);
    let replace_v = arg(args, 1);
    if with_host(|h| h.is_array(&search_v)) {
        let searches = with_host(|h| h.array_pairs(&search_v).unwrap_or_default());
        let replaces = if with_host(|h| h.is_array(&replace_v)) {
            with_host(|h| {
                h.array_pairs(&replace_v)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(_, v)| h.to_str(&v))
                    .collect::<Vec<_>>()
            })
        } else {
            Vec::new()
        };
        let scalar_replace = with_host(|h| {
            if h.is_array(&replace_v) {
                String::new()
            } else {
                h.to_str(&replace_v)
            }
        });
        let mut cur = subject;
        for (idx, (_, sv)) in searches.into_iter().enumerate() {
            let s = with_host(|h| h.to_str(&sv));
            let r = if replaces.is_empty() {
                scalar_replace.clone()
            } else {
                replaces.get(idx).cloned().unwrap_or_default()
            };
            cur = ci_replace(&cur, &s, &r);
        }
        Value::str(cur)
    } else {
        let s = with_host(|h| h.to_str(&search_v));
        let r = with_host(|h| h.to_str(&replace_v));
        Value::str(ci_replace(&subject, &s, &r))
    }
}

// ── escaping / encoding ──────────────────────────────────────────────────────

fn nl2br(s: &str) -> String {
    let cs: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 8);
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c == '\r' && i + 1 < cs.len() && cs[i + 1] == '\n' {
            out.push_str("<br />\r\n");
            i += 2;
        } else if c == '\n' && i + 1 < cs.len() && cs[i + 1] == '\r' {
            out.push_str("<br />\n\r");
            i += 2;
        } else if c == '\r' {
            out.push_str("<br />\r");
            i += 1;
        } else if c == '\n' {
            out.push_str("<br />\n");
            i += 1;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// `chunk_split($body, $length = 76, $end = "\r\n")`.
fn chunk_split(args: &[Value]) -> Result<Value, String> {
    let body = str_arg(args, 0);
    let length = match args.get(1) {
        Some(v) if !matches!(v, Value::Undef) => v.to_int(),
        _ => 76,
    };
    if length <= 0 {
        return Err(throws(
            "ValueError",
            "chunk_split(): Argument #2 ($length) must be greater than 0",
        ));
    }
    let end = match args.get(2) {
        Some(v) if !matches!(v, Value::Undef) => str_arg(args, 2),
        _ => "\r\n".to_string(),
    };
    let cs: Vec<char> = body.chars().collect();
    let mut out = String::new();
    for chunk in cs.chunks(length as usize) {
        out.extend(chunk.iter());
        out.push_str(&end);
    }
    Ok(Value::str(out))
}

fn quotemeta(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(
            c,
            '.' | '\\' | '+' | '*' | '?' | '[' | '^' | ']' | '$' | '(' | ')'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn addslashes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\'' | '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\0' => out.push_str("\\0"),
            _ => out.push(c),
        }
    }
    out
}

fn stripslashes(s: &str) -> String {
    let cs: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < cs.len() {
        if cs[i] == '\\' {
            if let Some(&next) = cs.get(i + 1) {
                // addslashes maps NUL -> "\0"; reverse it here.
                out.push(if next == '0' { '\0' } else { next });
                i += 2;
            } else {
                i += 1; // trailing backslash is dropped
            }
        } else {
            out.push(cs[i]);
            i += 1;
        }
    }
    out
}

// ── addcslashes / stripcslashes ──────────────────────────────────────────────

/// Port of `php_addcslashes_str` (php-src 8.5 `ext/standard/string.c:3812`).
///
/// A byte selected by the charlist is escaped. Outside printable ASCII the
/// escape is the C mnemonic (`\n`, `\t`, `\r`, `\a`, `\v`, `\b`, `\f`) or a
/// three-digit octal escape; inside it, a plain backslash prefix.
fn addcslashes(args: &[Value]) -> Value {
    let s = str_arg(args, 0);
    let what = str_arg(args, 1);
    // Both short-circuits sit ABOVE `php_charmask` (php-src
    // `ext/standard/string.c:3691`), so neither an empty subject nor an empty
    // charlist can raise the malformed-range warning.
    if s.is_empty() || what.is_empty() {
        return Value::str(s);
    }
    let flags = with_host(|h| charmask(h, what.as_bytes(), "addcslashes"));
    let mut out: Vec<u8> = Vec::with_capacity(s.len());
    for &c in s.as_bytes() {
        if flags[c as usize] {
            if c < 32 || c > 126 {
                out.push(b'\\');
                match c {
                    b'\n' => out.push(b'n'),
                    b'\t' => out.push(b't'),
                    b'\r' => out.push(b'r'),
                    0x07 => out.push(b'a'),
                    0x0b => out.push(b'v'),
                    0x08 => out.push(b'b'),
                    0x0c => out.push(b'f'),
                    _ => out.extend_from_slice(format!("{c:03o}").as_bytes()),
                }
                continue;
            }
            out.push(b'\\');
        }
        out.push(c);
    }
    Value::str(String::from_utf8_lossy(&out).into_owned())
}

/// Port of `php_stripcslashes` (php-src 8.5 `ext/standard/string.c:3749`) — the
/// inverse of [`addcslashes`], understanding the C mnemonics plus `\xHH`
/// (one or two hex digits) and `\NNN` (up to three octal digits). An unknown
/// escape yields the escaped character itself, so `\z` is `z`.
fn stripcslashes(args: &[Value]) -> Value {
    let s = str_arg(args, 0);
    let b = s.as_bytes();
    let n = b.len();
    let mut out: Vec<u8> = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        if b[i] != b'\\' || i + 1 >= n {
            out.push(b[i]);
            i += 1;
            continue;
        }
        i += 1;
        let c = b[i];
        match c {
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b'a' => out.push(0x07),
            b't' => out.push(b'\t'),
            b'v' => out.push(0x0b),
            b'b' => out.push(0x08),
            b'f' => out.push(0x0c),
            b'\\' => out.push(b'\\'),
            _ => {
                // `\x` takes one or two hex digits; with none it falls through to
                // the octal/default arm, exactly as upstream's ZEND_FALLTHROUGH.
                let hex = c == b'x' && i + 1 < n && b[i + 1].is_ascii_hexdigit();
                if hex {
                    i += 1;
                    let mut v = (b[i] as char).to_digit(16).unwrap();
                    if i + 1 < n && b[i + 1].is_ascii_hexdigit() {
                        i += 1;
                        v = v * 16 + (b[i] as char).to_digit(16).unwrap();
                    }
                    out.push(v as u8);
                } else {
                    let mut digits = 0usize;
                    let mut v: u32 = 0;
                    while i < n && (b'0'..=b'7').contains(&b[i]) && digits < 3 {
                        v = v * 8 + (b[i] - b'0') as u32;
                        i += 1;
                        digits += 1;
                    }
                    if digits > 0 {
                        out.push(v as u8);
                        i -= 1;
                    } else {
                        out.push(b[i]);
                    }
                }
            }
        }
        i += 1;
    }
    Value::str(String::from_utf8_lossy(&out).into_owned())
}

// ── count_chars ──────────────────────────────────────────────────────────────

/// Port of `PHP_FUNCTION(count_chars)` (php-src 8.5
/// `ext/standard/string.c:5574`) — the per-byte histogram of `$string`.
///
/// Mode 0 reports all 256 counters, 1 only the non-zero ones, 2 only the zero
/// ones; modes 3 and 4 answer a STRING of the bytes that did / did not occur.
fn count_chars(args: &[Value]) -> Result<Value, String> {
    let s = str_arg(args, 0);
    let mode = int_arg(args, 1);
    if !(0..=4).contains(&mode) {
        return Err(throws(
            "ValueError",
            "count_chars(): Argument #2 ($mode) must be between 0 and 4 (inclusive)",
        ));
    }
    let mut chars = [0i64; 256];
    for &b in s.as_bytes() {
        chars[b as usize] += 1;
    }
    if mode >= 3 {
        // Byte-valued output: this is one of the places the engine's UTF-8
        // `Value::Str` cannot hold the reference's answer for a non-ASCII
        // subject. See BUGS.md, "A non-ASCII byte cannot be represented".
        let want_zero = mode == 4;
        let bytes: Vec<u8> = (0..=255u8)
            .filter(|&i| (chars[i as usize] == 0) == want_zero)
            .collect();
        return Ok(Value::str(String::from_utf8_lossy(&bytes).into_owned()));
    }
    Ok(make_map(
        (0..256)
            .filter(|&i| match mode {
                1 => chars[i] != 0,
                2 => chars[i] == 0,
                _ => true,
            })
            .map(|i| (Value::int(i as i64), Value::int(chars[i])))
            .collect(),
    ))
}

// ── strtok ───────────────────────────────────────────────────────────────────

/// Port of `PHP_FUNCTION(strtok)` (php-src 8.5 `ext/standard/string.c:1129`).
///
/// Two arguments install a new subject and return its first token; one argument
/// continues the saved subject. Running out of tokens returns `false` AND
/// discards the subject, so a further one-argument call keeps answering `false`
/// rather than restarting.
fn strtok(args: &[Value]) -> Value {
    let two_arg = args.len() > 1 && !matches!(arg(args, 1), Value::Undef);
    let (subject, mut pos) = if two_arg {
        (str_arg(args, 0), 0usize)
    } else {
        match with_host(|h| h.strtok_state()) {
            Some(st) => st,
            None => {
                with_host(|h| {
                    h.warn("strtok(): Both arguments must be provided when starting tokenization")
                });
                return Value::bool(false);
            }
        }
    };
    // One argument means the subject IS the delimiter list (upstream's
    // `if (!tok) tok = str;`), which is how `strtok(" ")` reads.
    let delims = if two_arg {
        str_arg(args, 1)
    } else {
        str_arg(args, 0)
    };
    let mut table = [false; 256];
    for &d in delims.as_bytes() {
        table[d as usize] = true;
    }
    if two_arg {
        with_host(|h| h.set_strtok_state(Some((subject.clone(), 0))));
    }
    let b = subject.as_bytes();
    if pos >= b.len() {
        return Value::bool(false);
    }
    while pos < b.len() && table[b[pos] as usize] {
        pos += 1;
    }
    if pos >= b.len() {
        // Nothing but delimiters left: upstream releases the subject here.
        with_host(|h| h.set_strtok_state(None));
        return Value::bool(false);
    }
    let tok_start = pos;
    while pos < b.len() && !table[b[pos] as usize] {
        pos += 1;
    }
    let token = String::from_utf8_lossy(&b[tok_start..pos]).into_owned();
    with_host(|h| h.set_strtok_state(Some((subject, pos + 1))));
    Value::str(token)
}

fn str_rot13(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' => (b'a' + (c as u8 - b'a' + 13) % 26) as char,
            'A'..='Z' => (b'A' + (c as u8 - b'A' + 13) % 26) as char,
            _ => c,
        })
        .collect()
}

/// C `isspace`: space plus the five control whitespace bytes. Rust's
/// `u8::is_ascii_whitespace` leaves out the vertical tab, which the C machine
/// counts, so the two are NOT interchangeable here.
fn c_isspace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// `strip_tags($string, $allowed_tags = null)` — a port of `php_strip_tags_ex`
/// (ext/standard/string.c), state machine and all.
///
/// The states are the reference's: 0 outside a tag, 1 inside one, 2 inside
/// `<? … ?>`, 3 inside `<! … >`, 4 inside `<!-- … -->`. `depth` counts nested
/// `<`, `in_q` holds the open quote inside a tag, and `lc` the last significant
/// byte — all of which the `<?xml`, `<!DOCTYPE` and comment exits read back.
///
/// `allow` is the allow-list as PHP spells it (`"<b><i>"`), already lowercased.
/// While it is set, everything scanned between `<` and `>` is buffered, and the
/// buffer is re-emitted verbatim when [`php_tag_find`] accepts it — which is why
/// an allowed `<b class="x">` keeps its attributes.
///
/// Reading past the end yields `\0` rather than panicking, matching the C, which
/// walks an `estrndup` copy and so may legally read the terminator (`p[1]` on the
/// final byte, and `*(p-1)` guarded by an explicit position test).
fn strip_tags_ex(buf: &[u8], allow: Option<&[u8]>) -> Vec<u8> {
    let end = buf.len();
    let at = |i: usize| -> u8 {
        if i < end {
            buf[i]
        } else {
            0
        }
    };

    let mut rp: Vec<u8> = Vec::with_capacity(end);
    let mut tbuf: Vec<u8> = Vec::new();
    let mut p = 0usize;
    let mut lc: u8 = 0;
    let mut br: i64 = 0;
    let mut depth: i64 = 0;
    let mut in_q: u8 = 0;
    let mut state: u8 = 0;
    let mut is_xml = false;

    while p < end {
        let c = at(p);
        match state {
            0 => match c {
                0 => {}
                b'<' => {
                    if in_q == 0 {
                        if c_isspace(at(p + 1)) {
                            rp.push(c);
                        } else {
                            lc = b'<';
                            state = 1;
                            if allow.is_some() {
                                tbuf.push(b'<');
                            }
                            p += 1;
                            continue;
                        }
                    }
                }
                b'>' => {
                    if depth != 0 {
                        depth -= 1;
                    } else if in_q == 0 {
                        rp.push(c);
                    }
                }
                _ => rp.push(c),
            },
            1 => {
                let mut reg_char = false;
                match c {
                    0 => {}
                    b'<' => {
                        if in_q != 0 {
                        } else if c_isspace(at(p + 1)) {
                            reg_char = true;
                        } else {
                            depth += 1;
                        }
                    }
                    b'>' => {
                        if depth != 0 {
                            depth -= 1;
                        } else if in_q != 0 {
                        } else {
                            lc = b'>';
                            // `-->` closing an `<?xml` run is not the tag's end.
                            if !(is_xml && p >= 1 && at(p - 1) == b'-') {
                                in_q = 0;
                                state = 0;
                                is_xml = false;
                                if let Some(set) = allow {
                                    tbuf.push(b'>');
                                    if php_tag_find(&tbuf, set) {
                                        rp.extend_from_slice(&tbuf);
                                    }
                                    tbuf.clear();
                                }
                                p += 1;
                                continue;
                            }
                        }
                    }
                    b'"' | b'\'' => {
                        if p != 0 && (in_q == 0 || c == in_q) {
                            in_q = if in_q != 0 { 0 } else { c };
                        }
                        reg_char = true;
                    }
                    b'!' => {
                        // `<!` — comment, CDATA or DOCTYPE, not an HTML tag.
                        if p >= 1 && at(p - 1) == b'<' {
                            state = 3;
                            lc = c;
                            p += 1;
                            continue;
                        }
                        reg_char = true;
                    }
                    b'?' => {
                        if p >= 1 && at(p - 1) == b'<' {
                            br = 0;
                            state = 2;
                            p += 1;
                            continue;
                        }
                        reg_char = true;
                    }
                    _ => reg_char = true,
                }
                if reg_char && allow.is_some() {
                    tbuf.push(c);
                }
            }
            2 => match c {
                b'(' => {
                    if lc != b'"' && lc != b'\'' {
                        lc = b'(';
                        br += 1;
                    }
                }
                b')' => {
                    if lc != b'"' && lc != b'\'' {
                        lc = b')';
                        br -= 1;
                    }
                }
                b'>' => {
                    if depth != 0 {
                        depth -= 1;
                    } else if in_q == 0 && br == 0 && p >= 1 && lc != b'"' && at(p - 1) == b'?' {
                        in_q = 0;
                        state = 0;
                        tbuf.clear();
                        p += 1;
                        continue;
                    }
                }
                b'"' | b'\'' => {
                    if p >= 1 && at(p - 1) != b'\\' {
                        if lc == c {
                            lc = 0;
                        } else if lc != b'\\' {
                            lc = c;
                        }
                        if p != 0 && (in_q == 0 || c == in_q) {
                            in_q = if in_q != 0 { 0 } else { c };
                        }
                    }
                }
                // `<?xml` is markup, not PHP: rejoin the HTML state machine. An
                // `l` that does not complete that prefix falls to the no-op arm,
                // which is the C's plain `break`.
                b'l' | b'L'
                    if p > 4
                        && (at(p - 1) | 0x20) == b'm'
                        && (at(p - 2) | 0x20) == b'x'
                        && at(p - 3) == b'?'
                        && at(p - 4) == b'<' =>
                {
                    state = 1;
                    is_xml = true;
                    p += 1;
                    continue;
                }
                _ => {}
            },
            3 => match c {
                b'>' => {
                    if depth != 0 {
                        depth -= 1;
                    } else if in_q == 0 {
                        in_q = 0;
                        state = 0;
                        tbuf.clear();
                        p += 1;
                        continue;
                    }
                }
                b'"' | b'\'' => {
                    if p != 0 && at(p - 1) != b'\\' && (in_q == 0 || c == in_q) {
                        in_q = if in_q != 0 { 0 } else { c };
                    }
                }
                b'-' => {
                    // `<!--` opens a comment, which runs to `-->`.
                    if p >= 2 && at(p - 1) == b'-' && at(p - 2) == b'!' {
                        state = 4;
                        p += 1;
                        continue;
                    }
                }
                // `<!DOCTYPE` is not a comment; its body is tag-like.
                b'E' | b'e'
                    if p > 6
                        && (at(p - 1) | 0x20) == b'p'
                        && (at(p - 2) | 0x20) == b'y'
                        && (at(p - 3) | 0x20) == b't'
                        && (at(p - 4) | 0x20) == b'c'
                        && (at(p - 5) | 0x20) == b'o'
                        && (at(p - 6) | 0x20) == b'd' =>
                {
                    state = 1;
                    p += 1;
                    continue;
                }
                _ => {}
            },
            _ => {
                if c == b'>' && in_q == 0 && p >= 2 && at(p - 1) == b'-' && at(p - 2) == b'-' {
                    in_q = 0;
                    state = 0;
                    tbuf.clear();
                }
            }
        }
        p += 1;
    }
    rp
}

/// `php_tag_find`: is the scanned tag in the allow-list `set`?
///
/// `tag` is the raw `<…>` span. It is normalized first — lowercased, leading and
/// trailing whitespace dropped, attributes cut at the first space, and a closing
/// `</b>` or self-closing `<br/>` reduced to `<b>` / `<br>` — and the result is
/// then looked for as a SUBSTRING of `set`. Substring, not element: that is why
/// `strip_tags($s, "<b>")` keeps `<b>` but not `<body>`, while a `set` of
/// `"<body>"` keeps neither `<b>` (`<b>` is not a substring of `<body>`) nor…
/// it keeps `<body>` alone.
fn php_tag_find(tag: &[u8], set: &[u8]) -> bool {
    if tag.is_empty() {
        return false;
    }
    let at = |i: usize| -> u8 {
        if i < tag.len() {
            tag[i]
        } else {
            0
        }
    };
    let mut norm: Vec<u8> = Vec::with_capacity(tag.len() + 1);
    let mut state = 0u8;
    let mut done = false;
    let mut t = 0usize;
    let mut c = at(t).to_ascii_lowercase();
    while !done {
        match c {
            b'<' => norm.push(c),
            b'>' => done = true,
            _ => {
                if !c_isspace(c) {
                    if state == 0 {
                        state = 1;
                    }
                    // A `/` is dropped only where it marks the form of the tag —
                    // right after `<` (a closing tag) or right before `>` (a
                    // self-closing one). Anywhere else it is part of the name.
                    let prev = if t >= 1 { at(t - 1) } else { 0 };
                    if c != b'/' || (prev != b'<' && at(t + 1) != b'>') {
                        norm.push(c);
                    }
                } else if state == 1 {
                    done = true;
                }
            }
        }
        t += 1;
        c = at(t).to_ascii_lowercase();
    }
    norm.push(b'>');
    set.windows(norm.len()).any(|w| w == norm.as_slice())
}

/// `strip_tags` as PHP's function: `$allowed_tags` is a string (`"<b><i>"`), an
/// array of bare tag names (`["b", "i"]`, PHP 7.4+), or `null`.
fn strip_tags(args: &[Value]) -> Value {
    let subject = str_arg(args, 0);
    let allow: Option<Vec<u8>> = match args.get(1) {
        None | Some(Value::Undef) => None,
        Some(v) => Some(with_host(|h| match h.array_pairs(v) {
            // The array form is assembled into the string form first, exactly as
            // `PHP_FUNCTION(strip_tags)` does, so both share one code path.
            Some(pairs) => {
                let mut s = String::new();
                for (_, tag) in pairs {
                    s.push('<');
                    s.push_str(&h.to_str(&tag));
                    s.push('>');
                }
                s.to_ascii_lowercase().into_bytes()
            }
            None => h.to_str(v).to_ascii_lowercase().into_bytes(),
        })),
    };
    let out = strip_tags_ex(subject.as_bytes(), allow.as_deref());
    Value::str(String::from_utf8_lossy(&out).into_owned())
}

// ── distance metrics ─────────────────────────────────────────────────────────

/// `similar_text` core algorithm: length of the longest common substring, then
/// recurse on the segments to its left and right (PHP's `php_similar_str`).
fn similar_text_bytes(s1: &[u8], s2: &[u8]) -> usize {
    if s1.is_empty() || s2.is_empty() {
        return 0;
    }
    let (mut max, mut p1, mut p2) = (0usize, 0usize, 0usize);
    for i in 0..s1.len() {
        for j in 0..s2.len() {
            let mut k = 0;
            while i + k < s1.len() && j + k < s2.len() && s1[i + k] == s2[j + k] {
                k += 1;
            }
            if k > max {
                max = k;
                p1 = i;
                p2 = j;
            }
        }
    }
    if max == 0 {
        return 0;
    }
    max + similar_text_bytes(&s1[..p1], &s2[..p2])
        + similar_text_bytes(&s1[p1 + max..], &s2[p2 + max..])
}

/// `levenshtein($s1, $s2, $ins = 1, $repl = 1, $del = 1)`.
fn levenshtein(args: &[Value]) -> Value {
    let a = str_arg(args, 0);
    let b = str_arg(args, 1);
    let cins = args.get(2).map(|v| v.to_int()).unwrap_or(1);
    let crep = args.get(3).map(|v| v.to_int()).unwrap_or(1);
    let cdel = args.get(4).map(|v| v.to_int()).unwrap_or(1);
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    let (n, m) = (ab.len(), bb.len());
    // Single-row DP; row[j] is the distance to bb[..j].
    //
    // Every combination is WRAPPING. The three costs come straight from the
    // caller with no range check — the reference has none either, and answers
    // `levenshtein("a", "bb", PHP_INT_MAX)` with a wrapped `PHP_INT_MIN` rather
    // than refusing. Plain `+`/`*` panic on that in a debug build.
    let mut row: Vec<i64> = (0..=m as i64).map(|j| j.wrapping_mul(cins)).collect();
    for i in 1..=n {
        let mut prev = row[0];
        row[0] = (i as i64).wrapping_mul(cdel);
        for j in 1..=m {
            let cur = row[j];
            row[j] = if ab[i - 1] == bb[j - 1] {
                prev
            } else {
                prev.wrapping_add(crep)
                    .min(row[j].wrapping_add(cdel))
                    .min(row[j - 1].wrapping_add(cins))
            };
            prev = cur;
        }
    }
    Value::int(row[m])
}

// ── sprintf family / scanning ────────────────────────────────────────────────

/// `vsprintf`/`vprintf`: spread the args array into the core `sprintf`/`printf`.
fn vformat(args: &[Value], echo: bool) -> Result<Value, String> {
    let fmt = arg(args, 0);
    let arr = arg(args, 1);
    let mut call_args = vec![fmt];
    let pairs = with_host(|h| h.array_pairs(&arr).unwrap_or_default());
    for (_, v) in pairs {
        call_args.push(v);
    }
    // NOT `call_library("sprintf", ...)`: the array form reports a short argument
    // list as a ValueError about the array, where `sprintf` reports an
    // ArgumentCountError about loose parameters. Only the engine knows which.
    let out = with_host(|h| crate::builtins::php_sprintf(h, &call_args, true))?;
    if echo {
        with_host(|h| h.write_out(&out));
        Ok(Value::int(out.len() as i64))
    } else {
        Ok(Value::str(out))
    }
}

// ── sscanf ───────────────────────────────────────────────────────────────────
//
// Port of `php_sscanf_internal` (php-src 8.5 `ext/standard/scanf.c:574`), which
// is itself Tcl's scanner. The shape worth keeping in mind while reading:
//
//   * The result array is PRE-FILLED with one null per non-suppressed specifier
//     and conversions overwrite slots in place, so a format that runs out of
//     input leaves the tail as nulls (`scanf.c:626`, and the never-implemented
//     "prune the list" TODO at `:1176`).
//   * `underflow` (input exhausted) is distinct from a plain MISMATCH. Only
//     `underflow` with zero conversions produces the all-or-nothing failure
//     answer — null in the two-argument form, `-1` in the by-reference one.
//   * `nconversions` counts specifiers PROCESSED, so `%*d` and `%n` both count
//     even though `%*d` assigns nothing and `%n` consumes nothing.

/// A `%[…]` scan set: literal members plus inclusive ranges, optionally negated.
/// `CharInSet` (`scanf.c:238`) compares as a *signed* `char`, so ranges are
/// ordered by `i8` and one spanning the high half of the byte range behaves
/// accordingly.
struct ScanSet {
    exclude: bool,
    chars: Vec<u8>,
    ranges: Vec<(i8, i8)>,
}

impl ScanSet {
    fn contains(&self, c: u8) -> bool {
        let sc = c as i8;
        let hit = self.chars.contains(&c) || self.ranges.iter().any(|&(a, b)| a <= sc && sc <= b);
        hit != self.exclude
    }
}

/// Port of `BuildCharSet` (`scanf.c:137`). `fmt[i..]` starts just past the `[`;
/// returns the set and the index just past its closing `]`.
///
/// The quirks are upstream's: a `]` in first position is a literal member rather
/// than the terminator, a `-` in first or last position is a literal, and a
/// reversed range like `z-a` is normalized rather than rejected.
fn build_char_set(fmt: &[u8], mut i: usize) -> (ScanSet, usize) {
    let mut set = ScanSet {
        exclude: false,
        chars: Vec::new(),
        ranges: Vec::new(),
    };
    if fmt.get(i) == Some(&b'^') {
        set.exclude = true;
        i += 1;
    }
    let mut start = *fmt.get(i).unwrap_or(&0);
    if matches!(fmt.get(i), Some(&b']') | Some(&b'-')) {
        set.chars.push(fmt[i]);
        i += 1;
    }
    while i < fmt.len() && fmt[i] != b']' {
        let c = fmt[i];
        let next = fmt.get(i + 1).copied();
        if next == Some(b'-') {
            // Might open a range — hold it back until the next iteration decides.
            start = c;
        } else if c == b'-' {
            if next == Some(b']') || next.is_none() {
                // A trailing `-` is a literal, and so is the character it held.
                set.chars.push(start);
                set.chars.push(c);
            } else {
                i += 1;
                let end = fmt[i];
                let (a, b) = (start as i8, end as i8);
                set.ranges.push(if a < b { (a, b) } else { (b, a) });
            }
        } else {
            set.chars.push(c);
        }
        i += 1;
    }
    (set, i + 1)
}

/// One parsed conversion specifier.
struct ScanSpec {
    suppress: bool,
    /// `0` means "unset", which each conversion reads as its own default.
    width: usize,
    conv: u8,
    set: Option<ScanSet>,
}

/// One element of a parsed format.
enum ScanItem {
    /// A run of format whitespace: matches zero or more input whitespace bytes.
    Space,
    Literal(u8),
    Conv(ScanSpec),
}

/// Split a format into literals, whitespace and conversion specifiers, following
/// the grammar both `ValidateFormat` (`scanf.c:307`) and the scanner accept:
/// `% [*] [width] [l|L|h] conv`. An unterminated `%` at the very end is dropped,
/// as upstream's parser walks off the same way.
fn parse_scan_format(fmt: &[u8]) -> Vec<ScanItem> {
    let mut items = Vec::new();
    let mut i = 0;
    while i < fmt.len() {
        let c = fmt[i];
        if c.is_ascii_whitespace() {
            items.push(ScanItem::Space);
            while i < fmt.len() && fmt[i].is_ascii_whitespace() {
                i += 1;
            }
            continue;
        }
        if c != b'%' {
            items.push(ScanItem::Literal(c));
            i += 1;
            continue;
        }
        i += 1;
        if i >= fmt.len() {
            break;
        }
        if fmt[i] == b'%' {
            items.push(ScanItem::Literal(b'%'));
            i += 1;
            continue;
        }
        let mut suppress = false;
        if fmt[i] == b'*' {
            suppress = true;
            i += 1;
        }
        let mut width = 0usize;
        while i < fmt.len() && fmt[i].is_ascii_digit() {
            width = width * 10 + (fmt[i] - b'0') as usize;
            i += 1;
        }
        // The size modifiers are parsed and discarded, exactly as upstream does.
        if matches!(fmt.get(i), Some(b'l') | Some(b'L') | Some(b'h')) {
            i += 1;
        }
        let Some(&conv) = fmt.get(i) else { break };
        i += 1;
        let set = if conv == b'[' {
            let (s, next) = build_char_set(fmt, i);
            i = next;
            Some(s)
        } else {
            None
        };
        items.push(ScanItem::Conv(ScanSpec {
            suppress,
            width,
            conv,
            set,
        }));
    }
    items
}

/// What the numeric DFAs cap a field at: `buf[64]` in `php_sscanf_internal`.
const SCAN_NUM_MAX: usize = 63;

/// Port of the integer DFA (`scanf.c:919`). Consumes at most `width` bytes of
/// `s` from `p` and returns the digits collected plus the base finally in force,
/// or `None` when no digit was ever scanned (upstream's `SCAN_NODIGITS` abort).
///
/// `base == 0` is `%i`'s auto-detection: a leading `0` selects octal and arms
/// the `0x` probe, a leading `1`-`9` selects decimal.
fn scan_int_field(s: &[u8], p: &mut usize, width: usize, mut base: u32) -> Option<(String, u32)> {
    let width = if width == 0 || width > SCAN_NUM_MAX {
        SCAN_NUM_MAX
    } else {
        width
    };
    let mut buf: Vec<u8> = Vec::new();
    let (mut signok, mut nodigits, mut xok, mut nozero) = (true, true, false, true);
    let mut taken = 0usize;
    while taken < width && *p < s.len() {
        let c = s[*p];
        let accept = match c {
            b'0' => {
                if base == 16 {
                    xok = true;
                }
                if base == 0 {
                    base = 8;
                    xok = true;
                }
                if nozero {
                    signok = false;
                    nodigits = false;
                    nozero = false;
                } else {
                    signok = false;
                    xok = false;
                    nodigits = false;
                }
                true
            }
            b'1'..=b'7' => {
                if base == 0 {
                    base = 10;
                }
                signok = false;
                xok = false;
                nodigits = false;
                true
            }
            b'8' | b'9' => {
                if base == 0 {
                    base = 10;
                }
                if base <= 8 {
                    false
                } else {
                    signok = false;
                    xok = false;
                    nodigits = false;
                    true
                }
            }
            b'a'..=b'f' | b'A'..=b'F' => {
                if base <= 10 {
                    false
                } else {
                    signok = false;
                    xok = false;
                    nodigits = false;
                    true
                }
            }
            b'+' | b'-' => {
                if signok {
                    signok = false;
                    true
                } else {
                    false
                }
            }
            b'x' | b'X' => {
                // Only ever the SECOND byte of the buffer, so `-0x10` is not hex.
                if xok && buf.len() == 1 {
                    base = 16;
                    xok = false;
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        if !accept {
            break;
        }
        buf.push(c);
        *p += 1;
        taken += 1;
    }
    if nodigits {
        return None;
    }
    // A `0x` that no hex digit followed: give the `x` back to the input.
    if matches!(buf.last(), Some(b'x') | Some(b'X')) {
        buf.pop();
        *p -= 1;
    }
    Some((String::from_utf8_lossy(&buf).into_owned(), base))
}

/// Port of the float DFA (`scanf.c:1061`). Returns the collected literal, or
/// `None` when nothing was scanned.
fn scan_float_field(s: &[u8], p: &mut usize, width: usize) -> Option<String> {
    let width = if width == 0 || width > SCAN_NUM_MAX {
        SCAN_NUM_MAX
    } else {
        width
    };
    let mut buf: Vec<u8> = Vec::new();
    let (mut signok, mut nodigits, mut ptok, mut expok) = (true, true, true, true);
    let mut taken = 0usize;
    while taken < width && *p < s.len() {
        let c = s[*p];
        let accept = match c {
            b'0'..=b'9' => {
                signok = false;
                nodigits = false;
                true
            }
            b'+' | b'-' => {
                if signok {
                    signok = false;
                    true
                } else {
                    false
                }
            }
            b'.' => {
                if ptok {
                    ptok = false;
                    signok = false;
                    true
                } else {
                    false
                }
            }
            b'e' | b'E' => {
                // An exponent needs a mantissa digit already and no earlier `e`.
                if !nodigits && expok {
                    expok = false;
                    ptok = false;
                    signok = true;
                    nodigits = true;
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        if !accept {
            break;
        }
        buf.push(c);
        *p += 1;
        taken += 1;
    }
    if nodigits {
        if expok {
            // Nothing was ever scanned.
            return None;
        }
        // A dangling exponent: hand back the `e` and any sign that followed it.
        buf.pop();
        *p -= 1;
        if !matches!(buf.last(), Some(b'e') | Some(b'E')) {
            // The popped byte was the exponent's sign, so the `e` goes back too.
        } else {
            buf.pop();
            *p -= 1;
        }
    }
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// The outcome of one scan: the value produced per specifier index (`None` for a
/// slot no conversion reached), how many specifiers were processed, whether the
/// scan ended because the input ran out, and how many slots the two-argument
/// form pre-allocates.
struct ScanResult {
    values: Vec<Option<Value>>,
    nconversions: usize,
    underflow: bool,
    total_vars: usize,
}

/// `sscanf($string, $format, &...$vars)` — port of `php_sscanf_internal`.
///
/// With two arguments the return is an array of the converted values, padded
/// with nulls to one entry per non-suppressed specifier. With by-reference
/// arguments the return is the number of specifiers processed, the values land
/// in the variables, and a variable no conversion reached is left untouched.
fn sscanf(args: &[Value]) -> Result<Value, String> {
    let input = str_arg(args, 0);
    let fmt = str_arg(args, 1);
    let num_vars = args.len().saturating_sub(2);

    // `ValidateFormat` (`scanf.c:523`) checks the by-reference arity against the
    // format BEFORE any input is read, so both of these fire even on input that
    // could never have matched.
    if num_vars > 0 {
        let nspecs = parse_scan_format(fmt.as_bytes())
            .iter()
            .filter(|it| matches!(it, ScanItem::Conv(s) if !s.suppress))
            .count();
        if num_vars > nspecs {
            return Err(throws(
                "ValueError",
                "Variable is not assigned by any conversion specifiers",
            ));
        }
        if nspecs > num_vars {
            return Err(throws(
                "ValueError",
                "Different numbers of variable names and field specifiers",
            ));
        }
    }
    let r = sscanf_scan(input.as_bytes(), fmt.as_bytes(), num_vars);

    // Nothing converted AND the input ran out: `scan_set_error_return`
    // (`scanf.c:1184`) replaces the whole answer.
    if r.underflow && r.nconversions == 0 {
        return Ok(if num_vars > 0 {
            Value::int(-1)
        } else {
            Value::Undef
        });
    }
    if num_vars > 0 {
        for (i, v) in r.values.iter().enumerate() {
            if let Some(v) = v {
                // Argument 0 is the subject and 1 the format, so the first
                // by-reference slot is argument 2.
                with_host(|h| h.byref_out_put(i + 2, v.clone()));
            }
        }
        return Ok(Value::int(r.nconversions as i64));
    }
    Ok(make_list(
        (0..r.total_vars)
            .map(|i| r.values.get(i).cloned().flatten().unwrap_or(Value::Undef))
            .collect(),
    ))
}

fn sscanf_scan(input: &[u8], fmt: &[u8], num_vars: usize) -> ScanResult {
    let items = parse_scan_format(fmt);
    let spec_slots = items
        .iter()
        .filter(|it| matches!(it, ScanItem::Conv(s) if !s.suppress))
        .count();
    let total_vars = if num_vars > 0 { num_vars } else { spec_slots };
    let mut r = ScanResult {
        values: vec![None; total_vars],
        nconversions: 0,
        underflow: false,
        total_vars,
    };
    let mut obj = 0usize;
    let mut p = 0usize;
    let store = |r: &mut ScanResult, obj: &mut usize, v: Value| {
        if *obj < r.values.len() {
            r.values[*obj] = Some(v);
        }
        *obj += 1;
    };

    for item in &items {
        match item {
            ScanItem::Space => {
                while p < input.len() && input[p].is_ascii_whitespace() {
                    p += 1;
                }
            }
            ScanItem::Literal(c) => {
                if p >= input.len() {
                    r.underflow = true;
                    return r;
                }
                let got = input[p];
                p += 1;
                if got != *c {
                    // A mismatch stops the scan without being an underflow, so
                    // whatever was converted so far survives.
                    return r;
                }
            }
            ScanItem::Conv(spec) => {
                // `%n` never touches the input, so it is answered before the
                // end-of-input and whitespace-skip guards.
                if spec.conv == b'n' {
                    if !spec.suppress {
                        store(&mut r, &mut obj, Value::int(p as i64));
                    }
                    r.nconversions += 1;
                    continue;
                }
                if p >= input.len() {
                    r.underflow = true;
                    return r;
                }
                // `%c` and `%[` are the two that do NOT skip leading whitespace.
                if !matches!(spec.conv, b'c' | b'[') {
                    while p < input.len() && input[p].is_ascii_whitespace() {
                        p += 1;
                    }
                    if p >= input.len() {
                        r.underflow = true;
                        return r;
                    }
                }
                let value = match spec.conv {
                    b'd' | b'D' | b'i' | b'o' | b'x' | b'X' | b'u' => {
                        let base = match spec.conv {
                            b'i' => 0,
                            b'o' => 8,
                            b'x' | b'X' => 16,
                            _ => 10,
                        };
                        let Some((digits, base)) = scan_int_field(input, &mut p, spec.width, base)
                        else {
                            if p >= input.len() {
                                r.underflow = true;
                            }
                            return r;
                        };
                        let (neg, body) = match digits.as_bytes().first() {
                            Some(b'-') => (true, &digits[1..]),
                            Some(b'+') => (false, &digits[1..]),
                            _ => (false, digits.as_str()),
                        };
                        let body =
                            if base == 16 && (body.starts_with("0x") || body.starts_with("0X")) {
                                &body[2..]
                            } else {
                                body
                            };
                        let mag = u64::from_str_radix(body, base.max(2)).unwrap_or(u64::MAX);
                        if spec.conv == b'u' {
                            // `%u` runs through `strtoul`, so a value that does
                            // not fit a signed long comes back as a STRING.
                            let wrapped = if neg { mag.wrapping_neg() } else { mag };
                            if wrapped > i64::MAX as u64 {
                                Value::str(wrapped.to_string())
                            } else {
                                Value::int(wrapped as i64)
                            }
                        } else {
                            let n = mag.min(i64::MAX as u64) as i64;
                            Value::int(if neg { -n } else { n })
                        }
                    }
                    b'f' | b'e' | b'E' | b'g' => {
                        let Some(lit) = scan_float_field(input, &mut p, spec.width) else {
                            if p >= input.len() {
                                r.underflow = true;
                            }
                            return r;
                        };
                        Value::float(lit.parse::<f64>().unwrap_or(0.0))
                    }
                    b's' | b'c' => {
                        // `%c` is `%s` with the whitespace skip off and a default
                        // width of 1, so it can legitimately produce "".
                        let width = if spec.conv == b'c' && spec.width == 0 {
                            1
                        } else {
                            spec.width
                        };
                        let start = p;
                        while p < input.len() && !input[p].is_ascii_whitespace() {
                            p += 1;
                            if width != 0 && p - start >= width {
                                break;
                            }
                        }
                        Value::str(String::from_utf8_lossy(&input[start..p]).into_owned())
                    }
                    b'[' => {
                        let set = spec.set.as_ref();
                        let start = p;
                        while p < input.len() && set.map(|s| s.contains(input[p])).unwrap_or(false)
                        {
                            p += 1;
                            if spec.width != 0 && p - start >= spec.width {
                                break;
                            }
                        }
                        if p == start {
                            // The one conversion that aborts the whole scan on a
                            // zero-length match.
                            return r;
                        }
                        Value::str(String::from_utf8_lossy(&input[start..p]).into_owned())
                    }
                    _ => return r,
                };
                if !spec.suppress {
                    store(&mut r, &mut obj, value);
                }
                r.nconversions += 1;
            }
        }
    }
    r
}

// ── multibyte ────────────────────────────────────────────────────────────────

/// `mb_substr($str, $start, $length = null)`: codepoint-aware `substr`.
fn mb_substr(args: &[Value]) -> String {
    let s = str_arg(args, 0);
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as i64;
    let mut start = int_arg(args, 1);
    if start < 0 {
        start = (len + start).max(0);
    }
    let start = start.clamp(0, len) as usize;
    let count = match args.get(2) {
        Some(v) if !matches!(v, Value::Undef) => {
            let l = v.to_int();
            if l < 0 {
                (len - start as i64 + l).max(0) as usize
            } else {
                l as usize
            }
        }
        _ => chars.len() - start,
    };
    chars[start..(start + count).min(chars.len())]
        .iter()
        .collect()
}

// ── private helpers ──────────────────────────────────────────────────────────

fn is_truthy(v: &Value) -> bool {
    with_host(|h| h.is_truthy(v))
}

/// ASCII case-insensitive search over `&str`: index of the first match of `needle`.
fn ci_find(hay: &str, needle: &str) -> Option<usize> {
    ci_find_bytes(hay.as_bytes(), needle.as_bytes())
}

/// ASCII case-insensitive byte search: index of the first match of `nb` in `hb`.
/// Byte-oriented so callers can search multibyte haystacks without char-boundary
/// panics (PHP's non-`mb_` string functions are all byte-oriented).
fn ci_find_bytes(hb: &[u8], nb: &[u8]) -> Option<usize> {
    if nb.is_empty() {
        return Some(0);
    }
    if nb.len() > hb.len() {
        return None;
    }
    (0..=hb.len() - nb.len()).find(|&i| hb[i..i + nb.len()].eq_ignore_ascii_case(nb))
}

/// ASCII case-insensitive replace-all (non-overlapping, left to right).
fn ci_replace(subject: &str, search: &str, replace: &str) -> String {
    if search.is_empty() {
        return subject.to_string();
    }
    let mut out = String::with_capacity(subject.len());
    let mut rest = subject;
    while let Some(pos) = ci_find(rest, search) {
        out.push_str(&rest[..pos]);
        out.push_str(replace);
        rest = &rest[pos + search.len()..];
    }
    out.push_str(rest);
    out
}
