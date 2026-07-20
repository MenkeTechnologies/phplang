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
        "stripos" => strpos_ci(args),
        "strrpos" => strrpos(args, false),
        "strripos" => strrpos(args, true),
        "strncasecmp" => return Some(strncasecmp(args)),
        "str_ireplace" => str_ireplace(args),
        "nl2br" => Value::str(nl2br(&str_arg(args, 0))),
        "chunk_split" => return Some(chunk_split(args)),
        "quotemeta" => Value::str(quotemeta(&str_arg(args, 0))),
        "addslashes" => Value::str(addslashes(&str_arg(args, 0))),
        "stripslashes" => Value::str(stripslashes(&str_arg(args, 0))),
        "str_rot13" => Value::str(str_rot13(&str_arg(args, 0))),
        "similar_text" => Value::int(similar_text_bytes(
            str_arg(args, 0).as_bytes(),
            str_arg(args, 1).as_bytes(),
        ) as i64),
        "levenshtein" => levenshtein(args),
        "vsprintf" => return Some(vformat(args, false)),
        "vprintf" => return Some(vformat(args, true)),
        "sscanf" => sscanf(args),
        "htmlspecialchars_decode" | "html_entity_decode" => {
            Value::str(html_decode(&str_arg(args, 0)))
        }
        "strip_tags" => Value::str(strip_tags(&str_arg(args, 0))),
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
        return Err("substr_count(): Argument #2 ($needle) cannot be empty".into());
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
    match s.bytes().position(|b| set.as_bytes().contains(&b)) {
        Some(i) => Value::str(s[i..].to_string()),
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

/// `stripos($haystack, $needle, $offset = 0)`: case-insensitive `strpos`.
fn strpos_ci(args: &[Value]) -> Value {
    let hay = str_arg(args, 0);
    let needle = str_arg(args, 1);
    let len = hay.len() as i64;
    let mut off = int_arg(args, 2);
    if off < 0 {
        off = (len + off).max(0);
    }
    let off = off.clamp(0, len) as usize;
    // Byte-oriented search: slice the byte view so a multibyte offset can't panic.
    match ci_find_bytes(&hay.as_bytes()[off..], needle.as_bytes()) {
        Some(i) => Value::int((off + i) as i64),
        None => Value::bool(false),
    }
}

/// `strrpos`/`strripos($haystack, $needle, $offset = 0)`: last occurrence. `ci`
/// selects the case-insensitive `strripos`. Positive offset limits the search to
/// start at that byte; negative offset stops the search that many bytes from the
/// end. Returns `false` when not found.
fn strrpos(args: &[Value], ci: bool) -> Value {
    let hay = str_arg(args, 0);
    let needle = str_arg(args, 1);
    let len = hay.len();
    // PHP 8: an empty needle yields strlen($haystack), not false.
    if needle.is_empty() {
        return Value::int(len as i64);
    }
    let off = int_arg(args, 2);
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
        return Value::bool(false);
    }
    let eq = |i: usize| {
        let w = &hay.as_bytes()[i..i + needle.len()];
        if ci {
            w.eq_ignore_ascii_case(needle.as_bytes())
        } else {
            w == needle.as_bytes()
        }
    };
    match (lo..=hi).rev().find(|&i| i + needle.len() <= len && eq(i)) {
        Some(i) => Value::int(i as i64),
        None => Value::bool(false),
    }
}

// ── comparison ───────────────────────────────────────────────────────────────

/// `strncasecmp($s1, $s2, $n)`: case-insensitive compare of the first `$n` bytes.
/// PHP 8 raises a `ValueError` when `$length` is negative rather than coercing it.
fn strncasecmp(args: &[Value]) -> Result<Value, String> {
    let a = str_arg(args, 0);
    let b = str_arg(args, 1);
    let n_raw = int_arg(args, 2);
    if n_raw < 0 {
        return Err(
            "strncasecmp(): Argument #3 ($length) must be greater than or equal to 0".into(),
        );
    }
    let n = n_raw as usize;
    let ab = &a.as_bytes()[..n.min(a.len())];
    let bb = &b.as_bytes()[..n.min(b.len())];
    let la: Vec<u8> = ab.iter().map(|c| c.to_ascii_lowercase()).collect();
    let lb: Vec<u8> = bb.iter().map(|c| c.to_ascii_lowercase()).collect();
    Ok(Value::int(match la.cmp(&lb) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Greater => 1,
        std::cmp::Ordering::Equal => 0,
    }))
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
        return Err("chunk_split(): Argument #2 ($length) must be greater than 0".into());
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
        if matches!(c, '.' | '\\' | '+' | '*' | '?' | '[' | '^' | ']' | '$' | '(' | ')') {
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

/// `htmlspecialchars_decode` / `html_entity_decode` (the five entities the core
/// `htmlspecialchars` produces, plus the common `&#39;` and `&apos;`).
fn html_decode(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Basic `strip_tags`: drop every `<...>` span. No allow-list support.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0u32;
    for c in s.chars() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth = depth.saturating_sub(1);
            }
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
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
    let mut row: Vec<i64> = (0..=m as i64).map(|j| j * cins).collect();
    for i in 1..=n {
        let mut prev = row[0];
        row[0] = i as i64 * cdel;
        for j in 1..=m {
            let cur = row[j];
            row[j] = if ab[i - 1] == bb[j - 1] {
                prev
            } else {
                (prev + crep).min(row[j] + cdel).min(row[j - 1] + cins)
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
    if echo {
        let s = crate::builtins::call_library("sprintf", call_args)?;
        let out = with_host(|h| h.to_str(&s));
        with_host(|h| h.write_out(&out));
        Ok(Value::int(out.len() as i64))
    } else {
        crate::builtins::call_library("sprintf", call_args)
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
    chars[start..(start + count).min(chars.len())].iter().collect()
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
