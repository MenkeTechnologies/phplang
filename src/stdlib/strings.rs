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
        "substr_replace" => substr_replace(args),
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
        "sscanf" => sscanf(args),
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

/// `substr_replace($string, $replace, $offset, $length = null)` (scalar form).
fn substr_replace(args: &[Value]) -> Value {
    let s = str_arg(args, 0);
    let replace = str_arg(args, 1);
    let len = s.len() as i64;
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
                (start + l as usize).min(s.len())
            }
        }
        _ => s.len(),
    };
    // Byte-oriented splice so multibyte offsets never slice mid-UTF-8-char.
    let sb = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(start + replace.len() + (sb.len() - end));
    out.extend_from_slice(&sb[..start]);
    out.extend_from_slice(replace.as_bytes());
    out.extend_from_slice(&sb[end..]);
    Value::str(String::from_utf8_lossy(&out).into_owned())
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

/// `sscanf($string, $format)` (2-arg form): returns an array of the parsed
/// values. Supports `%d`/`%i`, `%f`/`%e`/`%g`, `%s`, `%c`, `%%`, optional field
/// width, and whitespace/literal matching. The by-reference (extra-args) form is
/// not supported — call it with two arguments.
fn sscanf(args: &[Value]) -> Value {
    let input: Vec<char> = str_arg(args, 0).chars().collect();
    let fmt: Vec<char> = str_arg(args, 1).chars().collect();
    let mut ii = 0;
    let mut fi = 0;
    let mut out: Vec<Value> = Vec::new();
    let skip_ws = |inp: &[char], p: &mut usize| {
        while *p < inp.len() && inp[*p].is_whitespace() {
            *p += 1;
        }
    };
    while fi < fmt.len() {
        let fc = fmt[fi];
        if fc == '%' {
            fi += 1;
            if fi >= fmt.len() {
                break;
            }
            let mut width = String::new();
            while fi < fmt.len() && fmt[fi].is_ascii_digit() {
                width.push(fmt[fi]);
                fi += 1;
            }
            if fi >= fmt.len() {
                break;
            }
            let spec = fmt[fi];
            fi += 1;
            let maxw = width.parse::<usize>().ok();
            match spec {
                'd' | 'i' => {
                    skip_ws(&input, &mut ii);
                    let s = take_number(&input, &mut ii, false);
                    if s.is_empty() {
                        break;
                    }
                    out.push(Value::int(s.parse().unwrap_or(0)));
                }
                'f' | 'e' | 'g' => {
                    skip_ws(&input, &mut ii);
                    let s = take_number(&input, &mut ii, true);
                    if s.is_empty() {
                        break;
                    }
                    out.push(Value::float(s.parse().unwrap_or(0.0)));
                }
                's' => {
                    skip_ws(&input, &mut ii);
                    let mut s = String::new();
                    while ii < input.len() && !input[ii].is_whitespace() {
                        s.push(input[ii]);
                        ii += 1;
                        if maxw.map(|w| s.chars().count() >= w).unwrap_or(false) {
                            break;
                        }
                    }
                    if s.is_empty() {
                        break;
                    }
                    out.push(Value::str(s));
                }
                'c' => {
                    if ii < input.len() {
                        out.push(Value::str(input[ii].to_string()));
                        ii += 1;
                    } else {
                        break;
                    }
                }
                '%' => {
                    if ii < input.len() && input[ii] == '%' {
                        ii += 1;
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        } else if fc.is_whitespace() {
            fi += 1;
            skip_ws(&input, &mut ii);
        } else {
            if ii < input.len() && input[ii] == fc {
                ii += 1;
                fi += 1;
            } else {
                break;
            }
        }
    }
    make_list(out)
}

/// Consume an optionally-signed integer (or float when `float`) literal.
fn take_number(inp: &[char], p: &mut usize, float: bool) -> String {
    let mut s = String::new();
    if *p < inp.len() && (inp[*p] == '-' || inp[*p] == '+') {
        s.push(inp[*p]);
        *p += 1;
    }
    while *p < inp.len() && inp[*p].is_ascii_digit() {
        s.push(inp[*p]);
        *p += 1;
    }
    if float {
        if *p < inp.len() && inp[*p] == '.' {
            s.push('.');
            *p += 1;
            while *p < inp.len() && inp[*p].is_ascii_digit() {
                s.push(inp[*p]);
                *p += 1;
            }
        }
        if *p < inp.len() && (inp[*p] == 'e' || inp[*p] == 'E') {
            let save = *p;
            let mut exp = String::from("e");
            *p += 1;
            if *p < inp.len() && (inp[*p] == '-' || inp[*p] == '+') {
                exp.push(inp[*p]);
                *p += 1;
            }
            let mut any = false;
            while *p < inp.len() && inp[*p].is_ascii_digit() {
                exp.push(inp[*p]);
                *p += 1;
                any = true;
            }
            if any {
                s.push_str(&exp);
            } else {
                *p = save;
            }
        }
    }
    // A lone sign is not a number.
    if s.is_empty() || s == "-" || s == "+" {
        String::new()
    } else {
        s
    }
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
