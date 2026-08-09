//! PHP standard-library `mbstring` functions. Part of the `stdlib` chain; see
//! `src/stdlib/mod.rs`. `dispatch` returns `None` for names it does not handle.
//!
//! Every function here is codepoint-aware: it operates on Unicode scalar values
//! (`char`), not bytes, which is the defining difference from the core byte-
//! oriented `str_*` builtins. `mb_strlen`, `mb_substr`, `mb_strtoupper` and
//! `mb_strtolower` already live in the `strings` module and are intentionally
//! absent here.
//!
//! Encoding scope: phplang stores `Value::Str` as a Rust `String` (always valid
//! UTF-8), so the multibyte machine is UTF-8. `mb_convert_encoding` therefore
//! supports the UTF-8 <-> ASCII / ISO-8859-1 / Latin-1 basics at the character
//! level; a true single-byte Latin-1 output (raw bytes 0x80..0xFF) cannot be
//! represented in a UTF-8 `String`, so the codepoint is preserved instead of the
//! byte. Untranslatable characters become `?`, matching PHP's substitution.
//!
//! `mb_internal_encoding` keeps module-local (thread-local) state. Caveat: that
//! state is NOT reset between separate script evaluations that run on the same
//! thread, so a setter call leaks into a later run in the same worker thread.

use crate::host::with_host;
use crate::stdlib::common::*;
use fusevm::Value;
use std::cell::RefCell;

thread_local! {
    /// Configured internal encoding; PHP defaults to "UTF-8".
    static INTERNAL_ENCODING: RefCell<String> = RefCell::new("UTF-8".to_string());
}

/// Dispatch a `mbstring`-category PHP function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let v = match name {
        "mb_str_split" => return Some(mb_str_split(args)),
        "mb_convert_case" => Value::str(mb_convert_case(args)),
        "mb_strpos" => return Some(mb_strpos(args, false)),
        "mb_stripos" => return Some(mb_strpos(args, true)),
        "mb_strrpos" => return Some(mb_strrpos(args, false)),
        "mb_strripos" => return Some(mb_strrpos(args, true)),
        "mb_substr_count" => return Some(mb_substr_count(args)),
        "mb_str_pad" => return Some(mb_str_pad(args)),
        "mb_ord" => mb_ord(args),
        "mb_chr" => mb_chr(args),
        "mb_lcfirst" => Value::str(mb_lcfirst(&str_arg(args, 0))),
        "mb_ucfirst" => Value::str(mb_ucfirst(&str_arg(args, 0))),
        // phplang strings are always valid UTF-8, so there is never an ill-formed
        // sequence to substitute; mb_scrub returns the input unchanged.
        "mb_scrub" => Value::str(str_arg(args, 0)),
        "mb_strcut" => Value::str(mb_strcut(args)),
        "mb_split" => mb_split(args),
        "mb_convert_kana" => Value::str(mb_convert_kana(args)),
        "mb_strwidth" => Value::int(mb_strwidth(&str_arg(args, 0)) as i64),
        "mb_convert_encoding" => Value::str(mb_convert_encoding(args)),
        "mb_detect_encoding" => mb_detect_encoding(args),
        "mb_check_encoding" => Value::bool(mb_check_encoding(args)),
        "mb_internal_encoding" => mb_internal_encoding(args),
        _ => return None,
    };
    Some(Ok(v))
}

// ── splitting ────────────────────────────────────────────────────────────────

/// `mb_str_split($string, $length = 1, $encoding = null)`: split into an array of
/// codepoint chunks of `$length` characters each. PHP 8 raises a `ValueError` for
/// `$length < 1` and returns an empty array for an empty string.
fn mb_str_split(args: &[Value]) -> Result<Value, String> {
    let s = str_arg(args, 0);
    let length = match args.get(1) {
        Some(v) if !matches!(v, Value::Undef) => v.to_int(),
        _ => 1,
    };
    if length < 1 {
        return Err(throws(
            "ValueError",
            "mb_str_split(): Argument #2 ($length) must be greater than 0",
        ));
    }
    let chars: Vec<char> = s.chars().collect();
    let out: Vec<Value> = chars
        .chunks(length as usize)
        .map(|c| Value::str(c.iter().collect::<String>()))
        .collect();
    Ok(make_list(out))
}

// ── case conversion ──────────────────────────────────────────────────────────

/// `mb_convert_case($string, $mode, $encoding = null)`. `$mode` is
/// `MB_CASE_UPPER` (0), `MB_CASE_LOWER` (1) or `MB_CASE_TITLE` (2), passed either
/// as the integer or (phplang has no constant table) as the constant name string.
fn mb_convert_case(args: &[Value]) -> String {
    let s = str_arg(args, 0);
    match resolve_mode(&arg(args, 1)) {
        1 => s.to_lowercase(),
        2 => title_case(&s),
        // 0 (MB_CASE_UPPER) and any unknown mode default to upper, matching the
        // most common call and avoiding a silent wrong-case for a bad constant.
        _ => s.to_uppercase(),
    }
}

/// Resolve a case-mode argument to its integer: accepts the canonical PHP integer
/// or the bareword constant name (which reaches us as its string).
fn resolve_mode(v: &Value) -> i64 {
    if let Value::Str(name) = v {
        match name.as_str() {
            "MB_CASE_UPPER" => return 0,
            "MB_CASE_LOWER" => return 1,
            "MB_CASE_TITLE" => return 2,
            _ => {}
        }
    }
    v.to_int()
}

/// Unicode title-casing: uppercase the first letter of each run of letters,
/// lowercase the rest. A non-letter (space, digit, punctuation) resets the run,
/// so `mb_convert_case("who's who", MB_CASE_TITLE)` yields `"Who'S Who"` (PHP 8).
fn title_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_alpha = false;
    for c in s.chars() {
        if c.is_alphabetic() {
            if prev_alpha {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
            prev_alpha = true;
        } else {
            out.push(c);
            prev_alpha = false;
        }
    }
    out
}

// ── searching / positions ────────────────────────────────────────────────────

/// `mb_strpos`/`mb_stripos($haystack, $needle, $offset = 0, $encoding = null)`:
/// codepoint position of the first match of `$needle`, or `false`. `ci` selects
/// the case-insensitive `mb_stripos`. PHP 8 raises a `ValueError` when `$offset`
/// is outside `[-len, len]` (a negative offset counts from the end).
fn mb_strpos(args: &[Value], ci: bool) -> Result<Value, String> {
    let (hay, needle) = ci_pair(args, ci);
    let len = hay.len() as i64;
    let off_raw = int_arg(args, 2);
    if off_raw > len || off_raw < -len {
        return Err(format!(
            "{}(): Argument #3 ($offset) must be contained in argument #1 ($haystack)",
            if ci { "mb_stripos" } else { "mb_strpos" }
        ));
    }
    let off = if off_raw < 0 {
        (len + off_raw) as usize
    } else {
        off_raw as usize
    };
    // Empty needle matches at the offset (PHP 8 semantics).
    for i in off..=hay.len().saturating_sub(needle.len()) {
        if hay[i..].starts_with(&needle[..]) {
            return Ok(Value::int(i as i64));
        }
    }
    Ok(Value::bool(false))
}

/// `mb_strrpos`/`mb_strripos($haystack, $needle, $offset = 0, $encoding = null)`:
/// codepoint position of the LAST match, or `false`. A positive `$offset` limits
/// the search to matches starting at or after it; a negative `$offset` stops the
/// search that many characters from the end. `ci` selects `mb_strripos`. PHP 8
/// raises a `ValueError` when `$offset` is outside `[-len, len]`.
fn mb_strrpos(args: &[Value], ci: bool) -> Result<Value, String> {
    let (hay, needle) = ci_pair(args, ci);
    let len = hay.len();
    let ilen = len as i64;
    let off = int_arg(args, 2);
    if off > ilen || off < -ilen {
        return Err(format!(
            "{}(): Argument #3 ($offset) must be contained in argument #1 ($haystack)",
            if ci { "mb_strripos" } else { "mb_strrpos" }
        ));
    }
    if needle.is_empty() {
        return Ok(Value::int(len as i64));
    }
    let (lo, hi) = if off >= 0 {
        (off as usize, len.saturating_sub(needle.len()))
    } else {
        let hi = (ilen + off).max(0) as usize;
        (0, hi.min(len.saturating_sub(needle.len())))
    };
    match (lo..=hi)
        .rev()
        .find(|&i| i + needle.len() <= len && hay[i..].starts_with(&needle[..]))
    {
        Some(i) => Ok(Value::int(i as i64)),
        None => Ok(Value::bool(false)),
    }
}

/// `mb_substr_count($haystack, $needle, $encoding = null)`: number of non-
/// overlapping codepoint matches. PHP 8 raises a `ValueError` on an empty needle.
fn mb_substr_count(args: &[Value]) -> Result<Value, String> {
    let hay: Vec<char> = str_arg(args, 0).chars().collect();
    let needle: Vec<char> = str_arg(args, 1).chars().collect();
    if needle.is_empty() {
        return Err(throws(
            "ValueError",
            "mb_substr_count(): Argument #2 ($needle) must not be empty",
        ));
    }
    let mut count = 0i64;
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if hay[i..i + needle.len()] == needle[..] {
            count += 1;
            i += needle.len();
        } else {
            i += 1;
        }
    }
    Ok(Value::int(count))
}

/// Return `(haystack, needle)` as codepoint vectors, case-folded when `ci`.
fn ci_pair(args: &[Value], ci: bool) -> (Vec<char>, Vec<char>) {
    let (h, n) = (str_arg(args, 0), str_arg(args, 1));
    if ci {
        (fold_chars(&h), fold_chars(&n))
    } else {
        (h.chars().collect(), n.chars().collect())
    }
}

/// Lowercase each character to a single representative codepoint, keeping a 1:1
/// char mapping so a case-insensitive match index stays aligned with the original
/// string. Full `str::to_lowercase` can expand one char into several (e.g. `İ`
/// U+0130 → `i` + combining dot), which would shift every subsequent position and
/// make `mb_stripos`/`mb_strripos` return a wrong index.
fn fold_chars(s: &str) -> Vec<char> {
    s.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}

// ── padding ──────────────────────────────────────────────────────────────────

/// `mb_str_pad($string, $length, $pad_string = " ", $pad_type = STR_PAD_RIGHT,
/// $encoding = null)` (PHP 8.3). `$length` and padding are counted in codepoints.
/// `STR_PAD_RIGHT` (1), `STR_PAD_LEFT` (0) and `STR_PAD_BOTH` (2) are accepted as
/// integers or as their constant-name strings.
fn mb_str_pad(args: &[Value]) -> Result<Value, String> {
    let s: Vec<char> = str_arg(args, 0).chars().collect();
    let target = int_arg(args, 1);
    let pad_str = match args.get(2) {
        Some(v) if !matches!(v, Value::Undef) => str_arg(args, 2),
        _ => " ".to_string(),
    };
    let pad: Vec<char> = pad_str.chars().collect();
    let pad_type = resolve_pad_type(&arg(args, 3));

    if target <= s.len() as i64 {
        return Ok(Value::str(s.into_iter().collect::<String>()));
    }
    if pad.is_empty() {
        return Err(throws(
            "ValueError",
            "mb_str_pad(): Argument #3 ($pad_string) must not be empty",
        ));
    }
    let total = target as usize - s.len();
    let make = |n: usize| -> Vec<char> { pad.iter().cloned().cycle().take(n).collect() };

    let mut out: Vec<char> = Vec::with_capacity(target as usize);
    match pad_type {
        0 => {
            // STR_PAD_LEFT
            out.extend(make(total));
            out.extend(s);
        }
        2 => {
            // STR_PAD_BOTH: left gets the smaller half (PHP floors the left side).
            let left = total / 2;
            let right = total - left;
            out.extend(make(left));
            out.extend(s);
            out.extend(make(right));
        }
        _ => {
            // STR_PAD_RIGHT (default)
            out.extend(s);
            out.extend(make(total));
        }
    }
    Ok(Value::str(out.into_iter().collect::<String>()))
}

/// Resolve a `$pad_type` argument (STR_PAD_* integer or constant-name string).
fn resolve_pad_type(v: &Value) -> i64 {
    if let Value::Str(name) = v {
        match name.as_str() {
            "STR_PAD_LEFT" => return 0,
            "STR_PAD_RIGHT" => return 1,
            "STR_PAD_BOTH" => return 2,
            _ => {}
        }
    }
    match v {
        Value::Undef => 1,
        _ => v.to_int(),
    }
}

// ── codepoint <-> character ──────────────────────────────────────────────────

/// `mb_ord($string, $encoding = null)`: the codepoint of the first character, or
/// `false` for an empty string.
fn mb_ord(args: &[Value]) -> Value {
    match str_arg(args, 0).chars().next() {
        Some(c) => Value::int(c as i64),
        None => Value::bool(false),
    }
}

/// `mb_chr($codepoint, $encoding = null)`: the character for a Unicode codepoint,
/// or `false` when the value is not a valid scalar value (surrogate or > 0x10FFFF).
fn mb_chr(args: &[Value]) -> Value {
    let cp = int_arg(args, 0);
    match u32::try_from(cp).ok().and_then(char::from_u32) {
        Some(c) => Value::str(c.to_string()),
        None => Value::bool(false),
    }
}

// ── first-character case / cutting / splitting ───────────────────────────────

/// `mb_lcfirst($string, $encoding = null)`: lowercase only the first character,
/// leaving the rest untouched. Empty string returns empty.
fn mb_lcfirst(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `mb_ucfirst($string, $encoding = null)` (PHP 8.4): uppercase only the first
/// character, leaving the rest untouched. Empty string returns empty.
fn mb_ucfirst(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `mb_strcut($string, $start, $length = null, $encoding = null)`: byte-offset
/// substring that never splits a multibyte character. `$start`/`$length` count
/// bytes; a negative `$start` counts from the end, a negative `$length` omits
/// that many trailing bytes. Offsets landing mid-character are floored down to the
/// nearest character boundary (matching PHP's UTF-8 behavior), so the result is
/// always valid UTF-8.
fn mb_strcut(args: &[Value]) -> String {
    let s = str_arg(args, 0);
    let blen = s.len() as i64;
    let mut start = int_arg(args, 1);
    if start < 0 {
        start = (blen + start).max(0);
    }
    let start = start.clamp(0, blen) as usize;
    let end = match args.get(2) {
        Some(v) if !matches!(v, Value::Undef) => {
            let l = with_host(|h| h.to_number(v).to_int());
            if l < 0 {
                (blen + l).max(0) as usize
            } else {
                (start as i64 + l).min(blen) as usize
            }
        }
        _ => blen as usize,
    };
    let start = floor_char_boundary(&s, start);
    let end = floor_char_boundary(&s, end.max(start));
    s[start..end].to_string()
}

/// Round a byte index down to the nearest UTF-8 character boundary (or `s.len()`
/// when at/after the end). Guarantees `s[..i]` and `s[i..]` are both valid slices.
fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// `mb_split($pattern, $string, $limit = -1)`: split `$string` by the regular
/// expression `$pattern`, returning an array of pieces (or `false` on a pattern
/// that fails to compile). A positive `$limit` caps the number of pieces, the last
/// holding the remainder; `0` or negative means no limit. An empty pattern returns
/// the whole string as a single element (PHP behavior).
fn mb_split(args: &[Value]) -> Value {
    let pattern = str_arg(args, 0);
    let subject = str_arg(args, 1);
    let limit = match args.get(2) {
        Some(v) if !matches!(v, Value::Undef) => int_arg(args, 2),
        _ => -1,
    };
    if pattern.is_empty() {
        return make_list(vec![Value::str(subject)]);
    }
    let re = match regex::Regex::new(&pattern) {
        Ok(r) => r,
        Err(_) => return Value::bool(false),
    };
    let parts: Vec<Value> = if limit > 0 {
        re.splitn(&subject, limit as usize)
            .map(|p| Value::str(p.to_string()))
            .collect()
    } else {
        re.split(&subject)
            .map(|p| Value::str(p.to_string()))
            .collect()
    };
    make_list(parts)
}

/// `mb_convert_kana($string, $mode = "KV", $encoding = null)`. Implements the
/// ASCII fullwidth<->halfwidth modes, which are fully defined by codepoint math:
/// `r`/`R` letters, `n`/`N` digits, `a`/`A` all alphanumerics + symbols
/// (U+FF01..U+FF5E <-> U+0021..U+007E), `s`/`S` the ideographic space
/// (U+3000 <-> U+0020). Lowercase = fullwidth→halfwidth, uppercase = the reverse.
/// The kana-specific modes (k/K/h/H/c/C/v/V) require Japanese kana tables that are
/// out of scope here; characters they would touch are left unchanged.
fn mb_convert_kana(args: &[Value]) -> String {
    let s = str_arg(args, 0);
    let mode = match args.get(1) {
        Some(v) if !matches!(v, Value::Undef) => str_arg(args, 1),
        _ => "KV".to_string(),
    };
    let (fw_all, fw_letter, fw_digit) =
        (mode.contains('a'), mode.contains('r'), mode.contains('n'));
    let (hw_all, hw_letter, hw_digit) =
        (mode.contains('A'), mode.contains('R'), mode.contains('N'));
    let (fw_space, hw_space) = (mode.contains('s'), mode.contains('S'));

    s.chars()
        .map(|c| {
            let cp = c as u32;
            if cp == 0x3000 && fw_space {
                return ' ';
            }
            if cp == 0x20 && hw_space {
                return '\u{3000}';
            }
            // Fullwidth ASCII variants → halfwidth.
            if (0xFF01..=0xFF5E).contains(&cp) {
                let is_letter = (0xFF21..=0xFF3A).contains(&cp) || (0xFF41..=0xFF5A).contains(&cp);
                let is_digit = (0xFF10..=0xFF19).contains(&cp);
                if fw_all || (fw_letter && is_letter) || (fw_digit && is_digit) {
                    return char::from_u32(cp - 0xFEE0).unwrap_or(c);
                }
            }
            // Halfwidth ASCII printables → fullwidth.
            if (0x21..=0x7E).contains(&cp) {
                let is_letter = c.is_ascii_alphabetic();
                let is_digit = c.is_ascii_digit();
                if hw_all || (hw_letter && is_letter) || (hw_digit && is_digit) {
                    return char::from_u32(cp + 0xFEE0).unwrap_or(c);
                }
            }
            c
        })
        .collect()
}

// ── display width ────────────────────────────────────────────────────────────

/// East-Asian-width ranges that count as 2 columns, ported from PHP's
/// `mbfl_charwidth` table (mbstring/libmbfl). Every codepoint outside these
/// ranges has width 1.
const WIDE_RANGES: &[(u32, u32)] = &[
    (0x1100, 0x115F),
    (0x11A3, 0x11A7),
    (0x11FA, 0x11FF),
    (0x2329, 0x232A),
    (0x2E80, 0x2E99),
    (0x2E9B, 0x2EF3),
    (0x2F00, 0x2FD5),
    (0x2FF0, 0x2FFB),
    (0x3000, 0x303E),
    (0x3041, 0x33FF),
    (0x3400, 0x4DBF),
    (0x4E00, 0x9FFF),
    (0xA000, 0xA4CF),
    (0xA960, 0xA97C),
    (0xAC00, 0xD7A3),
    (0xF900, 0xFAFF),
    (0xFE10, 0xFE19),
    (0xFE30, 0xFE6B),
    (0xFF00, 0xFF60),
    (0xFFE0, 0xFFE6),
    (0x1B000, 0x1B001),
    (0x1F200, 0x1F251),
    (0x20000, 0x3FFFD),
];

/// `mb_strwidth($string, $encoding = null)`: sum of per-character display widths
/// (2 for the East-Asian wide/full-width ranges above, 1 for everything else).
fn mb_strwidth(s: &str) -> usize {
    s.chars()
        .map(|c| {
            let cp = c as u32;
            if WIDE_RANGES.iter().any(|&(lo, hi)| cp >= lo && cp <= hi) {
                2
            } else {
                1
            }
        })
        .sum()
}

// ── encoding conversion / detection ──────────────────────────────────────────

/// `mb_convert_encoding($string, $to_encoding, $from_encoding = null)`. Supports
/// the UTF-8 <-> ASCII / ISO-8859-1 / Latin-1 basics at the character level (see
/// the module note on the single-byte-output limitation). Characters that cannot
/// be represented in the target encoding become `?`.
fn mb_convert_encoding(args: &[Value]) -> String {
    let s = str_arg(args, 0);
    let to = normalize_encoding(&str_arg(args, 1));
    match to.as_str() {
        "ASCII" => s
            .chars()
            .map(|c| if (c as u32) < 0x80 { c } else { '?' })
            .collect(),
        "ISO-8859-1" => s
            .chars()
            .map(|c| if (c as u32) <= 0xFF { c } else { '?' })
            .collect(),
        // UTF-8 (and any unrecognized target): the Rust string is already UTF-8.
        _ => s,
    }
}

/// `mb_detect_encoding($string, $encodings = null, $strict = false)`. phplang
/// strings are always valid UTF-8, so detection reduces to: pure-ASCII input is
/// "ASCII", otherwise "UTF-8". A candidate list is honored in order (the first
/// candidate the string satisfies wins); the default order is `["ASCII","UTF-8"]`,
/// matching PHP's default `detect_order`.
fn mb_detect_encoding(args: &[Value]) -> Value {
    let s = str_arg(args, 0);
    let is_ascii = s.is_ascii();
    let candidates: Vec<String> = match args.get(1) {
        Some(v) if with_host(|h| h.is_array(v)) => with_host(|h| {
            h.array_pairs(v)
                .unwrap_or_default()
                .into_iter()
                .map(|(_, val)| normalize_encoding(&h.to_str(&val)))
                .collect()
        }),
        Some(v) if !matches!(v, Value::Undef) => str_arg(args, 1)
            .split(',')
            .map(|e| normalize_encoding(e.trim()))
            .collect(),
        _ => vec!["ASCII".to_string(), "UTF-8".to_string()],
    };
    for cand in &candidates {
        let ok = match cand.as_str() {
            "ASCII" => is_ascii,
            "UTF-8" => true, // always valid here
            _ => false,
        };
        if ok {
            return Value::str(cand.clone());
        }
    }
    Value::bool(false)
}

/// `mb_check_encoding($value = null, $encoding = null)`: whether `$value` is valid
/// in `$encoding`. UTF-8 is always valid (phplang strings are UTF-8); ASCII is
/// valid iff every byte is < 0x80. Defaults to the internal encoding.
fn mb_check_encoding(args: &[Value]) -> bool {
    let s = str_arg(args, 0);
    let enc = match args.get(1) {
        Some(v) if !matches!(v, Value::Undef) => normalize_encoding(&str_arg(args, 1)),
        _ => normalize_encoding(&current_internal_encoding()),
    };
    match enc.as_str() {
        "ASCII" => s.is_ascii(),
        _ => true,
    }
}

/// `mb_internal_encoding($encoding = null)`: getter (no arg) returns the stored
/// encoding; setter stores it and returns `true`. See the module note on the
/// thread-local reset-isolation caveat.
fn mb_internal_encoding(args: &[Value]) -> Value {
    match args.first() {
        Some(v) if !matches!(v, Value::Undef) => {
            let enc = str_arg(args, 0);
            INTERNAL_ENCODING.with(|e| *e.borrow_mut() = enc);
            Value::bool(true)
        }
        _ => Value::str(current_internal_encoding()),
    }
}

fn current_internal_encoding() -> String {
    INTERNAL_ENCODING.with(|e| e.borrow().clone())
}

/// Canonicalize an encoding label to one of "UTF-8" / "ASCII" / "ISO-8859-1".
fn normalize_encoding(name: &str) -> String {
    let up = name.trim().to_ascii_uppercase().replace('_', "-");
    match up.as_str() {
        "UTF-8" | "UTF8" => "UTF-8".to_string(),
        "US-ASCII" | "ASCII" => "ASCII".to_string(),
        "ISO-8859-1" | "LATIN1" | "LATIN-1" | "ISO8859-1" => "ISO-8859-1".to_string(),
        other => other.to_string(),
    }
}
