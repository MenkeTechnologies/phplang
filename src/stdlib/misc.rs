//! PHP standard-library `misc` functions. Part of the `stdlib` chain; see
//! `src/stdlib/mod.rs`. `dispatch` returns `None` for names it does not handle.
//!
//! What lives here is the leftover grab-bag that does not fit a tidier category:
//! natural-order string comparison (`strnatcmp`/`strnatcasecmp`), the `soundex`
//! phonetic hash, single-line CSV parsing (`str_getcsv`), the recursive array
//! walker (`array_walk_recursive`), and the PHP 8.4 predicate helpers
//! (`array_find`/`array_find_key`/`array_any`/`array_all`). Every behavior below
//! was cross-checked against the reference `php` 8.5 CLI.

use crate::host;
use crate::stdlib::common::*;
use fusevm::Value;
use std::cmp::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

/// Dispatch a `misc`-category PHP function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let v = match name {
        "pack" => return Some(php_pack(args)),
        "unpack" => return Some(php_unpack(args)),
        "strnatcmp" => str_natcmp(args, false),
        "strnatcasecmp" => str_natcmp(args, true),
        "soundex" => Value::str(soundex(&str_arg(args, 0))),
        "str_getcsv" => return Some(str_getcsv(args)),
        "str_word_count" => return Some(str_word_count(args)),
        "metaphone" => Value::str(php_metaphone(&str_arg(args, 0), int_arg(args, 1))),
        "uniqid" => uniqid(args),
        "array_walk_recursive" => return Some(array_walk_recursive(args)),
        "array_find" => return Some(array_find(args, false)),
        "array_find_key" => return Some(array_find(args, true)),
        "array_any" => return Some(array_predicate(args, Predicate::Any)),
        "array_all" => return Some(array_predicate(args, Predicate::All)),
        "array_udiff" => {
            return Some(php_array_set_op(
                name,
                args,
                false,
                SetBehavior::Normal,
                true,
                false,
            ))
        }
        "array_uintersect" => {
            return Some(php_array_set_op(
                name,
                args,
                true,
                SetBehavior::Normal,
                true,
                false,
            ))
        }
        "array_diff_ukey" => {
            return Some(php_array_set_op(
                name,
                args,
                false,
                SetBehavior::Key,
                false,
                true,
            ))
        }
        "array_intersect_ukey" => {
            return Some(php_array_set_op(
                name,
                args,
                true,
                SetBehavior::Key,
                false,
                true,
            ))
        }
        "array_diff_uassoc" => {
            return Some(php_array_set_op(
                name,
                args,
                false,
                SetBehavior::Assoc,
                false,
                true,
            ))
        }
        "array_intersect_uassoc" => {
            return Some(php_array_set_op(
                name,
                args,
                true,
                SetBehavior::Assoc,
                false,
                true,
            ))
        }
        "array_udiff_uassoc" => {
            return Some(php_array_set_op(
                name,
                args,
                false,
                SetBehavior::Assoc,
                true,
                true,
            ))
        }
        "array_uintersect_uassoc" => {
            return Some(php_array_set_op(
                name,
                args,
                true,
                SetBehavior::Assoc,
                true,
                true,
            ))
        }
        "array_udiff_assoc" => return Some(php_array_set_op_key(name, args, false)),
        "array_uintersect_assoc" => return Some(php_array_set_op_key(name, args, true)),
        "array_multisort" => return Some(array_multisort(args)),
        "version_compare" => return Some(version_compare(args)),
        "ip2long" => return Some(ip2long(args)),
        "long2ip" => Value::str(long2ip(int_arg(args, 0))),
        "strcoll" => Value::int(strcoll(&str_arg(args, 0), &str_arg(args, 1))),
        _ => return None,
    };
    Some(Ok(v))
}

// ── strnatcmp / strnatcasecmp ────────────────────────────────────────────────

/// `strnatcmp($a, $b)` / `strnatcasecmp($a, $b)` — compare two strings in
/// "natural" order, returning the sign of the comparison (`-1`, `0`, `1`) exactly
/// as PHP does for these inputs.
fn str_natcmp(args: &[Value], fold_case: bool) -> Value {
    let a = str_arg(args, 0);
    let b = str_arg(args, 1);
    let ord = nat_cmp(&a, &b, fold_case);
    Value::int(match ord {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    })
}

/// Natural-order string comparison — a faithful byte-for-byte port of PHP 8.5's
/// `strnatcmp_ex` (`ext/standard/strnatcmp.c`, Martin Pool's algorithm as PHP
/// currently ships it). PHP compares on raw bytes, so this does too (avoiding any
/// char-boundary slicing and matching PHP for multibyte input).
///
/// Key details that the older textbook version gets wrong (all verified against
/// the `php` 8.5 CLI): leading zeros are stripped **once at the very start**, not
/// per digit-run, so `strnatcmp("0","00") === 0` and `strnatcmp("1","01") === 0`;
/// a digit run is "fractional" (compared left-aligned, first differing digit
/// wins) when the *current* char of either side is `'0'`, otherwise it compares by
/// magnitude (longest run wins, ties by value); and a trailing non-matched
/// character makes the longer string greater, so `strnatcmp("a ","a") === 1`.
fn nat_cmp(a: &str, b: &str, fold_case: bool) -> Ordering {
    strnat(a.as_bytes(), b.as_bytes(), fold_case).cmp(&0)
}

/// C `isspace` (used by `strnatcmp`): space, tab, newline, vertical tab, form
/// feed, carriage return. Rust's `is_ascii_whitespace` omits the vertical tab, so
/// spell it out to stay byte-identical to PHP.
fn c_isspace(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// The byte at `i`, or `0` past the end. PHP strings are NUL-terminated, so the C
/// code reads the terminator (a `0` byte) at `aend`; returning `0` for
/// out-of-range indices reproduces that exactly.
fn byte_at(s: &[u8], i: usize) -> u8 {
    if i < s.len() {
        s[i]
    } else {
        0
    }
}

/// `compare_left`: two left-aligned digit runs, the first differing digit wins.
/// Returns `(result, new_ai, new_bi)`; the indices land on the first non-digit of
/// each run (matching where the C pointers stop).
fn compare_left(a: &[u8], mut ai: usize, b: &[u8], mut bi: usize) -> (i32, usize, usize) {
    loop {
        let ad = ai < a.len() && a[ai].is_ascii_digit();
        let bd = bi < b.len() && b[bi].is_ascii_digit();
        if !ad && !bd {
            return (0, ai, bi);
        } else if !ad {
            return (-1, ai, bi);
        } else if !bd {
            return (1, ai, bi);
        } else if a[ai] < b[bi] {
            return (-1, ai, bi);
        } else if a[ai] > b[bi] {
            return (1, ai, bi);
        }
        ai += 1;
        bi += 1;
    }
}

/// `compare_right`: two magnitude-aligned digit runs — the longest run wins; on
/// equal length the greatest value wins (tracked in `bias`).
fn compare_right(a: &[u8], mut ai: usize, b: &[u8], mut bi: usize) -> (i32, usize, usize) {
    let mut bias = 0i32;
    loop {
        let ad = ai < a.len() && a[ai].is_ascii_digit();
        let bd = bi < b.len() && b[bi].is_ascii_digit();
        if !ad && !bd {
            return (bias, ai, bi);
        } else if !ad {
            return (-1, ai, bi);
        } else if !bd {
            return (1, ai, bi);
        } else if a[ai] < b[bi] {
            if bias == 0 {
                bias = -1;
            }
        } else if a[ai] > b[bi] && bias == 0 {
            bias = 1;
        }
        ai += 1;
        bi += 1;
    }
}

/// The core of `strnatcmp_ex`, returning `-1`/`0`/`+1`.
fn strnat(a: &[u8], b: &[u8], ci: bool) -> i32 {
    if a.is_empty() || b.is_empty() {
        return match a.len().cmp(&b.len()) {
            Ordering::Equal => 0,
            Ordering::Greater => 1,
            Ordering::Less => -1,
        };
    }
    let mut ap = 0usize;
    let mut bp = 0usize;
    let mut ca = byte_at(a, ap);
    let mut cb = byte_at(b, bp);
    // Skip leading zeros — once, at the very start of each string.
    while ca == b'0' && ap + 1 < a.len() && byte_at(a, ap + 1).is_ascii_digit() {
        ap += 1;
        ca = byte_at(a, ap);
    }
    while cb == b'0' && bp + 1 < b.len() && byte_at(b, bp + 1).is_ascii_digit() {
        bp += 1;
        cb = byte_at(b, bp);
    }
    loop {
        while c_isspace(ca) {
            ap += 1;
            ca = byte_at(a, ap);
        }
        while c_isspace(cb) {
            bp += 1;
            cb = byte_at(b, bp);
        }
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let fractional = ca == b'0' || cb == b'0';
            let (result, nap, nbp) = if fractional {
                compare_left(a, ap, b, bp)
            } else {
                compare_right(a, ap, b, bp)
            };
            ap = nap;
            bp = nbp;
            if result != 0 {
                return result;
            } else if ap >= a.len() && bp >= b.len() {
                return 0;
            } else if ap >= a.len() {
                return -1;
            } else if bp >= b.len() {
                return 1;
            }
            ca = byte_at(a, ap);
            cb = byte_at(b, bp);
        }
        let (mut xa, mut xb) = (ca, cb);
        if ci {
            xa = xa.to_ascii_uppercase();
            xb = xb.to_ascii_uppercase();
        }
        if xa < xb {
            return -1;
        } else if xa > xb {
            return 1;
        }
        ap += 1;
        bp += 1;
        if ap >= a.len() && bp >= b.len() {
            return 0;
        } else if ap >= a.len() {
            return -1;
        } else if bp >= b.len() {
            return 1;
        }
        ca = byte_at(a, ap);
        cb = byte_at(b, bp);
    }
}

// ── soundex ──────────────────────────────────────────────────────────────────

/// `soundex($str)` — the four-character Soundex phonetic key. A faithful port of
/// PHP's `ext/standard/soundex.c`: the first letter is kept verbatim, subsequent
/// letters contribute their code only when it differs from the previous code and
/// is non-zero, and the result is padded to four characters with `'0'`. A string
/// with no letters (or an empty string) yields `"0000"`, matching PHP 8.5.
fn soundex(s: &str) -> String {
    // Codes for A..Z; 0 marks the "not coded" letters (vowels, H, W, Y).
    const TABLE: [u8; 26] = [
        0, b'1', b'2', b'3', 0, b'1', b'2', 0, 0, b'2', b'2', b'4', b'5', b'5', 0, b'1', b'2',
        b'6', b'2', b'3', 0, b'1', 0, b'2', 0, b'2',
    ];
    let mut out: Vec<u8> = Vec::with_capacity(4);
    let mut last: u8 = 0;
    for ch in s.chars() {
        if out.len() >= 4 {
            break;
        }
        let c = ch.to_ascii_uppercase();
        if !c.is_ascii_alphabetic() {
            continue;
        }
        let code = TABLE[(c as u8 - b'A') as usize];
        if out.is_empty() {
            // First valid character is kept verbatim.
            out.push(c as u8);
            last = code;
        } else if code != last {
            if code != 0 {
                out.push(code);
            }
            last = code;
        }
    }
    while out.len() < 4 {
        out.push(b'0');
    }
    // Only ASCII bytes were pushed, so this is always valid UTF-8.
    String::from_utf8(out).unwrap_or_default()
}

// ── str_getcsv ───────────────────────────────────────────────────────────────

/// `PHP_CSV_NO_ESCAPE`: an empty `$escape`.
pub(crate) const CSV_NO_ESCAPE: i32 = -1;

/// The `$separator`/`$enclosure` argument at `idx`: one byte, or the
/// `must be a single character` `ValueError`.
pub(crate) fn csv_char_arg(
    func: &str,
    args: &[Value],
    idx: usize,
    pname: &str,
    default: u8,
) -> Result<u8, String> {
    if args.len() <= idx || matches!(args[idx], Value::Undef) {
        return Ok(default);
    }
    let s = str_arg(args, idx);
    match s.as_bytes() {
        [c] => Ok(*c),
        _ => Err(throws(
            "ValueError",
            format!(
                "{func}(): Argument #{} (${pname}) must be a single character",
                idx + 1
            ),
        )),
    }
}

/// `php_csv_handle_escape_argument`: an omitted `$escape` is the 8.4
/// deprecation and means `\`; an empty one disables escaping; anything longer
/// than a byte is a `ValueError`.
pub(crate) fn csv_escape_arg(func: &str, args: &[Value], idx: usize) -> Result<i32, String> {
    if args.len() <= idx || matches!(args[idx], Value::Undef) {
        host::with_host(|h| {
            h.deprecated(format!(
                "{func}(): the $escape parameter must be provided as its default value will change"
            ))
        });
        return Ok(b'\\' as i32);
    }
    let s = str_arg(args, idx);
    match s.as_bytes() {
        [] => Ok(CSV_NO_ESCAPE),
        [c] => Ok(*c as i32),
        _ => Err(throws(
            "ValueError",
            format!(
                "{func}(): Argument #{} ($escape) must be empty or a single character",
                idx + 1
            ),
        )),
    }
}

/// `php_fgetcsv_lookup_trailing_spaces`: where `buf` ends once a trailing
/// `\n`, `\r` or `\r\n` is set aside (despite the name, nothing else is).
fn csv_trailing(buf: &[u8]) -> usize {
    let n = buf.len();
    match (n.checked_sub(2).map(|i| buf[i]), buf.last()) {
        (Some(b'\r'), Some(b'\n')) => n - 2,
        (_, Some(b'\n' | b'\r')) => n - 1,
        _ => n,
    }
}

/// C `isspace`: space, `\t`, `\n`, `\v`, `\f`, `\r`.
fn c_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// `php_fgetcsv` (`ext/standard/file.c`), followed pointer for pointer.
///
/// `first` is the line read; `next_line` supplies the next one when an
/// enclosure is still open at the end of it (`fgetcsv`), or `None` when there
/// is nothing more (`str_getcsv`, or the stream at its end). `None` back means
/// a blank line, which both callers turn into `[null]`.
///
/// The reference steps through the line with `php_mblen`; every byte the
/// separator, enclosure or escape can be is a single byte in every locale
/// PHP accepts them in, so stepping byte by byte reaches the same fields.
pub(crate) fn php_fgetcsv(
    delimiter: u8,
    enclosure: u8,
    escape: i32,
    first: Vec<u8>,
    mut next_line: impl FnMut() -> Option<Vec<u8>>,
) -> Option<Vec<Vec<u8>>> {
    let mut buf = first;
    // Past the end reads as the C string's terminating NUL.
    let at = |buf: &[u8], i: usize| buf.get(i).copied().unwrap_or(0);
    let is_escape = |c: u8| escape != CSV_NO_ESCAPE && c as i32 == escape;
    let span = |buf: &[u8], a: usize, b: usize| buf[a.min(buf.len())..b.min(buf.len())].to_vec();
    let mut bptr = 0usize;
    let mut limit = csv_trailing(&buf);
    let mut line_end = limit;
    let mut line_end_len = buf.len() - limit;
    let mut values = Vec::new();
    let mut first_field = true;
    loop {
        let mut temp: Vec<u8> = Vec::new();
        let mut inc_len = usize::from(bptr < limit);
        if inc_len == 1 {
            let mut tmp = bptr;
            while at(&buf, tmp) != delimiter && c_space(at(&buf, tmp)) {
                tmp += 1;
            }
            if at(&buf, tmp) == enclosure && tmp < limit {
                bptr = tmp;
            }
        }
        if first_field && bptr == line_end {
            return None;
        }
        first_field = false;
        if inc_len != 0 && at(&buf, bptr) == enclosure {
            // 2A. An enclosed field.
            let mut state = 0;
            bptr += 1;
            let mut hunk = bptr;
            'enclosed: loop {
                if inc_len == 0 {
                    match state {
                        2 => {
                            temp.extend(span(&buf, hunk, bptr - 1));
                            hunk = bptr;
                            break 'enclosed;
                        }
                        _ => {
                            if state == 1 {
                                temp.extend(span(&buf, hunk, bptr));
                                hunk = bptr;
                            }
                            if hunk != line_end {
                                temp.extend(span(&buf, hunk, bptr));
                                hunk = bptr;
                            }
                            // The line end belongs to the field.
                            temp.extend(span(&buf, line_end, line_end + line_end_len));
                            match next_line() {
                                Some(nb) => {
                                    buf = nb;
                                    bptr = 0;
                                    hunk = 0;
                                    limit = csv_trailing(&buf);
                                    line_end = limit;
                                    line_end_len = buf.len() - limit;
                                    state = 0;
                                }
                                None => {
                                    // An unterminated enclosure takes the rest.
                                    if bptr > limit {
                                        if hunk == bptr {
                                            hunk -= 1;
                                        }
                                        bptr -= 1;
                                    }
                                    break 'enclosed;
                                }
                            }
                        }
                    }
                } else {
                    match state {
                        1 => {
                            // The byte after an escape is taken as it is.
                            bptr += 1;
                            state = 0;
                        }
                        2 => {
                            if at(&buf, bptr) != enclosure {
                                // A real closing enclosure.
                                temp.extend(span(&buf, hunk, bptr - 1));
                                hunk = bptr;
                                break 'enclosed;
                            }
                            // A doubled enclosure is one literal.
                            temp.extend(span(&buf, hunk, bptr));
                            bptr += 1;
                            hunk = bptr;
                            state = 0;
                        }
                        _ => {
                            let c = at(&buf, bptr);
                            if c == enclosure {
                                state = 2;
                            } else if is_escape(c) {
                                state = 1;
                            }
                            bptr += 1;
                        }
                    }
                }
                inc_len = usize::from(bptr < limit);
            }
            // Whatever follows the closing enclosure, up to the separator, is
            // part of the field.
            while inc_len != 0 && at(&buf, bptr) != delimiter {
                bptr += inc_len;
                inc_len = usize::from(bptr < limit);
            }
            temp.extend(span(&buf, hunk, bptr));
            bptr += inc_len;
        } else {
            // 2B. A bare field: up to the separator, trailing line end dropped.
            let hunk = bptr;
            while inc_len != 0 && at(&buf, bptr) != delimiter {
                bptr += inc_len;
                inc_len = usize::from(bptr < limit);
            }
            temp.extend(span(&buf, hunk, bptr));
            let keep = csv_trailing(&temp);
            temp.truncate(keep);
            if at(&buf, bptr) == delimiter {
                bptr += 1;
            }
        }
        values.push(temp);
        if inc_len == 0 {
            break;
        }
    }
    Some(values)
}

/// `[null]` for a blank line (`php_bc_fgetcsv_empty_line`), else the fields.
pub(crate) fn csv_fields_value(fields: Option<Vec<Vec<u8>>>) -> Value {
    match fields {
        None => make_list(vec![Value::Undef]),
        Some(fs) => make_list(
            fs.into_iter()
                .map(|f| Value::str(String::from_utf8_lossy(&f).into_owned()))
                .collect(),
        ),
    }
}

/// `str_getcsv($string, $separator = ",", $enclosure = "\"", $escape = "\\")`:
/// the arguments checked in the reference's order, then [`php_fgetcsv`] with
/// no stream to read further lines from.
fn str_getcsv(args: &[Value]) -> Result<Value, String> {
    let delimiter = csv_char_arg("str_getcsv", args, 1, "separator", b',')?;
    let enclosure = csv_char_arg("str_getcsv", args, 2, "enclosure", b'"')?;
    let escape = csv_escape_arg("str_getcsv", args, 3)?;
    let s = str_arg(args, 0).into_bytes();
    Ok(csv_fields_value(php_fgetcsv(
        delimiter,
        enclosure,
        escape,
        s,
        || None,
    )))
}

// ── array_walk_recursive ─────────────────────────────────────────────────────

/// `array_walk_recursive($array, $callback, $extra = null)` — call
/// `$callback($value, $key [, $extra])` for every **leaf** (non-array) element,
/// descending into nested arrays; always returns `true`.
///
/// The value goes in through a reference cell, so a `function (&$v)` callback
/// rewrites the leaf in place — the same mechanism `array_walk` uses.
fn array_walk_recursive(args: &[Value]) -> Result<Value, String> {
    let arr = arg(args, 0);
    let cb = arg(args, 1);
    let extra = arg(args, 2);
    let has_extra = args.len() > 2;
    walk_recursive(&arr, &cb, has_extra.then(|| extra.clone()))?;
    if host::unwinding() {
        return Ok(Value::Undef);
    }
    Ok(Value::bool(true))
}

/// Recurse `array`, invoking `cb` on every non-array leaf. The callback runs the
/// VM, so it is called outside any `with_host` borrow; the first error stops the
/// walk. Nested arrays are descended into (their entries handled recursively) but
/// never passed to the callback, matching PHP.
fn walk_recursive(array: &Value, cb: &Value, extra: Option<Value>) -> Result<(), String> {
    let pairs = host::with_host(|h| h.array_pairs(array)).unwrap_or_default();
    for (k, v) in pairs {
        let is_arr = host::with_host(|h| h.is_array(&v));
        if is_arr {
            walk_recursive(&v, cb, extra.clone())?;
            continue;
        }
        // Same by-reference plumbing as `array_walk`: hand the leaf over in a
        // reference cell and copy whatever the callback left there back under the
        // same key, so `function (&$v) { $v *= 2; }` mutates the array.
        let (cell, slot) = host::with_host(|h| h.new_ref_cell(v));
        let mut call_args = vec![cell, k.clone()];
        if let Some(e) = &extra {
            call_args.push(e.clone());
        }
        host::call_value(cb.clone(), call_args)?;
        if host::unwinding() {
            return Ok(());
        }
        host::with_host(|h| {
            let updated = h.ref_cell_value(slot);
            h.arr_set_key(array, &k, updated);
        });
    }
    Ok(())
}

// ── array_find / array_find_key ──────────────────────────────────────────────

/// `array_find($array, $callback)` (`want_key = false`) returns the first
/// **value** whose `$callback($value, $key)` is truthy, or `null` if none match;
/// `array_find_key` (`want_key = true`) returns the matching key instead. Both are
/// PHP 8.4 functions.
fn array_find(args: &[Value], want_key: bool) -> Result<Value, String> {
    let arr = arg(args, 0);
    let cb = arg(args, 1);
    let pairs = host::with_host(|h| h.array_pairs(&arr)).unwrap_or_default();
    for (k, v) in pairs {
        let r = host::call_value(cb.clone(), vec![v.clone(), k.clone()])?;
        if host::unwinding() {
            return Ok(Value::Undef);
        }
        if host::with_host(|h| h.is_truthy(&r)) {
            return Ok(if want_key { k } else { v });
        }
    }
    Ok(Value::Undef)
}

// ── array_any / array_all ────────────────────────────────────────────────────

enum Predicate {
    /// `array_any` — true if the callback is truthy for at least one element.
    Any,
    /// `array_all` — true if the callback is truthy for every element.
    All,
}

/// `array_any($array, $callback)` / `array_all($array, $callback)` (PHP 8.4) —
/// evaluate `$callback($value, $key)` across the array and reduce to a bool.
/// `array_any` short-circuits on the first truthy result; `array_all` on the first
/// falsy one. Empty arrays yield `false` for `array_any` and `true` for
/// `array_all` (the vacuous cases, matching PHP).
fn array_predicate(args: &[Value], kind: Predicate) -> Result<Value, String> {
    let arr = arg(args, 0);
    let cb = arg(args, 1);
    let pairs = host::with_host(|h| h.array_pairs(&arr)).unwrap_or_default();
    for (k, v) in pairs {
        let r = host::call_value(cb.clone(), vec![v, k])?;
        if host::unwinding() {
            return Ok(Value::Undef);
        }
        let truthy = host::with_host(|h| h.is_truthy(&r));
        match kind {
            Predicate::Any if truthy => return Ok(Value::bool(true)),
            Predicate::All if !truthy => return Ok(Value::bool(false)),
            _ => {}
        }
    }
    Ok(Value::bool(matches!(kind, Predicate::All)))
}

// ── the user-comparator diff / intersect family ─────────────────────────────

/// `DIFF_NORMAL`/`DIFF_KEY`/`DIFF_ASSOC` (and the `INTERSECT_*` twins, which
/// share the values): what an entry is matched on.
#[derive(Clone, Copy, PartialEq)]
enum SetBehavior {
    /// By value alone (`array_udiff`, `array_uintersect`).
    Normal,
    /// By key alone (`array_diff_ukey`, `array_intersect_ukey`).
    Key,
    /// By key, then value (`array_diff_uassoc`, `array_udiff_uassoc`, …).
    Assoc,
}

/// One entry of a pre-sorted operand: a `Bucket` of the C. `pos` is its place
/// in its own array, which is how a deletion finds it in the result.
#[derive(Clone)]
struct SetBucket {
    pos: usize,
    key: Value,
    val: Value,
}

/// The parameter parsing shared by the whole family: `"+f"` / `"+ff"` in
/// `zend_parse_parameters` (at least one array, then the callbacks, each
/// checked in turn), followed by the body's own is-it-an-array loop.
fn set_op_args(func: &str, args: &[Value], ncb: usize) -> Result<(Vec<Value>, Vec<Value>), String> {
    let min = 1 + ncb;
    if args.len() < min {
        return Err(throws(
            "ArgumentCountError",
            format!(
                "{func}() expects at least {min} arguments, {} given",
                args.len()
            ),
        ));
    }
    let split = args.len() - ncb;
    for (i, cb) in args[split..].iter().enumerate() {
        if let Some(reason) = crate::stdlib::callable::callable_reason(cb) {
            return Err(throws(
                "TypeError",
                format!(
                    "{func}(): Argument #{} must be a valid callback, {reason}",
                    split + i + 1
                ),
            ));
        }
    }
    for (i, a) in args[..split].iter().enumerate() {
        if !host::with_host(|h| h.is_array(a)) {
            // `zend_argument_type_error` names the parameter when it has one:
            // only the first, `$array`, is declared before the variadic.
            let name = if i == 0 { " ($array)" } else { "" };
            let given = host::with_host(|h| h.type_name_for_error(a));
            return Err(throws(
                "TypeError",
                format!(
                    "{func}(): Argument #{}{name} must be of type array, {given} given",
                    i + 1
                ),
            ));
        }
    }
    Ok((args[..split].to_vec(), args[split..].to_vec()))
}

/// The first array with the entries at `removed` positions dropped — the
/// `zend_array_dup` of the C, minus its `zend_hash_del`s.
fn set_op_result(first: Vec<(Value, Value)>, removed: &[bool]) -> Value {
    make_map(
        first
            .into_iter()
            .zip(removed)
            .filter(|(_, gone)| !**gone)
            .map(|(kv, _)| kv)
            .collect(),
    )
}

/// The value/key comparators of `php_array_diff` / `php_array_intersect`.
///
/// `current` is `BG(user_compare_fci)`: the C keeps ONE active callback and
/// swaps the key and value callbacks in and out of it as it walks, and a user
/// comparator always calls whichever is active. The swaps are reproduced
/// exactly, because where the C leaves the wrong one active (a matched value
/// in `array_uintersect_uassoc` against a third array), the reference calls the
/// value callback with keys.
struct SetCmp<'a> {
    ucmp: crate::stdlib::arrays::UserCmp<'a>,
    current: Value,
    data_user: bool,
}

impl SetCmp<'_> {
    /// `php_array_user_compare_unstable`, or `php_array_data_compare_string_unstable`.
    fn data(&mut self, a: &SetBucket, b: &SetBucket) -> i32 {
        if self.data_user {
            let cb = self.current.clone();
            self.ucmp.sorting(&cb, &a.val, &b.val)
        } else {
            host::with_host(|h| {
                let (x, y) = (h.to_str_diag(&a.val), h.to_str_diag(&b.val));
                x.cmp(&y) as i32
            })
        }
    }

    /// `php_array_user_key_compare_unstable` — the only key comparison the
    /// family reaches, since every key-matching member takes a key callback.
    fn key(&mut self, a: &SetBucket, b: &SetBucket) -> i32 {
        let cb = self.current.clone();
        self.ucmp.sorting(&cb, &a.key, &b.key)
    }
}

/// `php_array_diff` / `php_array_intersect` (`ext/standard/array.c`): every
/// operand is copied into a bucket list and `zend_sort`ed with the UNSTABLE
/// comparator (by value for `Normal`, by key otherwise), then the lists are
/// walked in step and entries of the first are deleted from a copy of it. The
/// walk is followed line for line: which pairs the user callbacks see, and in
/// which order, is the observable part.
fn php_array_set_op(
    func: &str,
    args: &[Value],
    intersect: bool,
    behavior: SetBehavior,
    data_user: bool,
    key_user: bool,
) -> Result<Value, String> {
    let ncb = data_user as usize + key_user as usize;
    let (arrays, cbs) = set_op_args(func, args, ncb)?;
    // `fci1` is the value callback when there is one, `fci2` the key callback.
    let cb_data = if data_user {
        cbs[0].clone()
    } else {
        Value::Undef
    };
    let cb_key = if key_user {
        cbs[ncb - 1].clone()
    } else {
        Value::Undef
    };
    let assoc_key_user = behavior != SetBehavior::Normal && key_user;
    let mut cmp = SetCmp {
        ucmp: crate::stdlib::arrays::UserCmp::new(func),
        current: if behavior == SetBehavior::Normal {
            cb_data.clone()
        } else {
            cb_key.clone()
        },
        data_user,
    };

    let first = host::with_host(|h| h.array_pairs(&arrays[0])).unwrap_or_default();
    let mut lists: Vec<Vec<SetBucket>> = Vec::with_capacity(arrays.len());
    for a in &arrays {
        let pairs = host::with_host(|h| h.array_pairs(a)).unwrap_or_default();
        let mut list: Vec<SetBucket> = pairs
            .into_iter()
            .enumerate()
            .map(|(pos, (key, val))| SetBucket { pos, key, val })
            .collect();
        if behavior == SetBehavior::Normal {
            crate::stdlib::zsort::zend_sort(&mut list, &mut |a, b| cmp.data(a, b));
        } else {
            crate::stdlib::zsort::zend_sort(&mut list, &mut |a, b| cmp.key(a, b));
        }
        lists.push(list);
    }

    let mut removed = vec![false; first.len()];
    if intersect {
        intersect_walk(
            &lists,
            &mut removed,
            &mut cmp,
            behavior,
            assoc_key_user,
            &cb_data,
            &cb_key,
        );
    } else {
        diff_walk(
            &lists,
            &mut removed,
            &mut cmp,
            behavior,
            assoc_key_user,
            &cb_data,
            &cb_key,
        );
    }
    if let Some(e) = cmp.ucmp.err {
        return Err(e);
    }
    if host::unwinding() {
        return Ok(Value::Undef);
    }
    Ok(set_op_result(first, &removed))
}

/// The `while (Z_TYPE(ptrs[0]->val) != IS_UNDEF)` loop of `php_array_diff`.
/// `c` keeps its value across a comparison loop that does not run, as the C's
/// does, and that is load-bearing.
fn diff_walk(
    lists: &[Vec<SetBucket>],
    removed: &mut [bool],
    cmp: &mut SetCmp,
    behavior: SetBehavior,
    assoc_key_user: bool,
    cb_data: &Value,
    cb_key: &Value,
) {
    let mut ptrs = vec![0usize; lists.len()];
    let l0 = &lists[0];
    while ptrs[0] < l0.len() {
        if assoc_key_user {
            cmp.current = cb_key.clone();
        }
        let mut c = 1;
        for i in 1..lists.len() {
            let li = &lists[i];
            let mut ptr = ptrs[i];
            if behavior == SetBehavior::Normal {
                while ptrs[i] < li.len() {
                    c = cmp.data(&l0[ptrs[0]], &li[ptrs[i]]);
                    if c <= 0 {
                        break;
                    }
                    ptrs[i] += 1;
                }
            } else {
                while ptr < li.len() {
                    c = cmp.key(&l0[ptrs[0]], &li[ptr]);
                    if c == 0 {
                        break;
                    }
                    ptr += 1;
                }
            }
            if c == 0 {
                match behavior {
                    SetBehavior::Normal => {
                        if ptrs[i] < li.len() {
                            ptrs[i] += 1;
                        }
                        break;
                    }
                    SetBehavior::Assoc => {
                        if ptr < li.len() {
                            if cmp.data_user {
                                cmp.current = cb_data.clone();
                            }
                            if cmp.data(&l0[ptrs[0]], &li[ptr]) != 0 {
                                c = -1;
                                if assoc_key_user {
                                    cmp.current = cb_key.clone();
                                }
                            } else {
                                break;
                            }
                        }
                    }
                    SetBehavior::Key => break,
                }
            }
        }
        if c == 0 {
            // In one of the others: delete it and every following equal value.
            loop {
                removed[l0[ptrs[0]].pos] = true;
                ptrs[0] += 1;
                if ptrs[0] == l0.len() {
                    return;
                }
                if behavior != SetBehavior::Normal || cmp.data(&l0[ptrs[0] - 1], &l0[ptrs[0]]) != 0
                {
                    break;
                }
            }
        } else {
            // In none of the others: skip it and every following equal value.
            loop {
                ptrs[0] += 1;
                if ptrs[0] == l0.len() {
                    return;
                }
                if behavior != SetBehavior::Normal || cmp.data(&l0[ptrs[0] - 1], &l0[ptrs[0]]) != 0
                {
                    break;
                }
            }
        }
    }
}

/// The `while (Z_TYPE(ptrs[0]->val) != IS_UNDEF)` loop of `php_array_intersect`.
fn intersect_walk(
    lists: &[Vec<SetBucket>],
    removed: &mut [bool],
    cmp: &mut SetCmp,
    behavior: SetBehavior,
    assoc_key_user: bool,
    cb_data: &Value,
    cb_key: &Value,
) {
    let mut ptrs = vec![0usize; lists.len()];
    let l0 = &lists[0];
    let mut c = 0;
    while ptrs[0] < l0.len() {
        if assoc_key_user {
            cmp.current = cb_key.clone();
        }
        let mut i = 1;
        while i < lists.len() {
            let li = &lists[i];
            if behavior == SetBehavior::Normal {
                while ptrs[i] < li.len() {
                    c = cmp.data(&l0[ptrs[0]], &li[ptrs[i]]);
                    if c <= 0 {
                        break;
                    }
                    ptrs[i] += 1;
                }
            } else {
                while ptrs[i] < li.len() {
                    c = cmp.key(&l0[ptrs[0]], &li[ptrs[i]]);
                    if c <= 0 {
                        break;
                    }
                    ptrs[i] += 1;
                }
                if c == 0 && ptrs[i] < li.len() && behavior == SetBehavior::Assoc {
                    if cmp.data_user {
                        cmp.current = cb_data.clone();
                    }
                    if cmp.data(&l0[ptrs[0]], &li[ptrs[i]]) != 0 {
                        c = 1;
                        if assoc_key_user {
                            cmp.current = cb_key.clone();
                        }
                    }
                }
            }
            if ptrs[i] == li.len() {
                // This operand is exhausted: nothing left in the first can be
                // in all of them.
                for b in &l0[ptrs[0]..] {
                    removed[b.pos] = true;
                }
                return;
            }
            if c != 0 {
                break;
            }
            ptrs[i] += 1;
            i += 1;
        }
        if c != 0 {
            // Not in every operand: delete it and every following value that
            // still sorts below the operand that refused it.
            loop {
                removed[l0[ptrs[0]].pos] = true;
                ptrs[0] += 1;
                if ptrs[0] == l0.len() {
                    return;
                }
                if behavior != SetBehavior::Normal
                    || cmp.data(&l0[ptrs[0]], &lists[i][ptrs[i]]) >= 0
                {
                    break;
                }
            }
        } else {
            // In every operand: keep it and every following equal value.
            loop {
                ptrs[0] += 1;
                if ptrs[0] == l0.len() {
                    return;
                }
                if behavior != SetBehavior::Normal || cmp.data(&l0[ptrs[0] - 1], &l0[ptrs[0]]) != 0
                {
                    break;
                }
            }
        }
    }
}

/// `php_array_diff_key` / `php_array_intersect_key` with a user value
/// comparator — `array_udiff_assoc` / `array_uintersect_assoc`. No sorting:
/// each entry of the first array is looked up BY KEY in every other one, and
/// only a key hit runs `zval_user_compare` (which, unlike the sorting
/// comparators, takes a bool result silently).
fn php_array_set_op_key(func: &str, args: &[Value], intersect: bool) -> Result<Value, String> {
    let (arrays, cbs) = set_op_args(func, args, 1)?;
    let cb = &cbs[0];
    let mut ucmp = crate::stdlib::arrays::UserCmp::new(func);
    let first = host::with_host(|h| h.array_pairs(&arrays[0])).unwrap_or_default();
    let mut out = Vec::new();
    for (k, v) in first {
        let mut ok = true;
        for other in &arrays[1..] {
            let hit = host::with_host(|h| {
                h.array_has_key(other, &k)
                    .unwrap_or(false)
                    .then(|| h.index_get(other, &k))
            });
            let matched = match hit {
                Some(data) => ucmp.plain(cb, &v, &data) == 0,
                None => false,
            };
            // diff: drop on the first match; intersect: drop on the first miss.
            if matched != intersect {
                ok = false;
                break;
            }
        }
        if ok {
            out.push((k, v));
        }
    }
    if let Some(e) = ucmp.err {
        return Err(e);
    }
    if host::unwinding() {
        return Ok(Value::Undef);
    }
    Ok(make_map(out))
}

// ── array_multisort ──────────────────────────────────────────────────────────

/// One sortable column: its array handle plus the direction/type flags that
/// followed it in the argument list.
struct MsortCol {
    arr: Value,
    /// `1` ascending, `-1` descending (`SORT_ASC` / `SORT_DESC`).
    order: i32,
    /// `SORT_REGULAR` (0) / `SORT_NUMERIC` (1) / `SORT_STRING` (2).
    sort_type: i64,
}

/// `array_multisort($arr1, [flags…], $arr2, …)` (basic form) — sort one or more
/// equal-length arrays as parallel columns. The first array is the primary sort
/// key; later arrays break ties in order. Flags (`SORT_ASC`/`SORT_DESC` and
/// `SORT_REGULAR`/`SORT_NUMERIC`/`SORT_STRING`) apply to the array they follow.
/// Every array is reordered by the same permutation and reindexed `0..n`.
///
/// LIMITATION: PHP mutates the arrays by reference. phplang arrays are shared heap
/// handles (the same mechanism `usort` relies on), so in-place mutation works for
/// array variables. This implementation covers the single-array and
/// several-parallel-column cases with direction/type flags; it does not implement
/// the full flag-precedence corner cases of the C `array_multisort`.
fn array_multisort(args: &[Value]) -> Result<Value, String> {
    let mut cols: Vec<MsortCol> = Vec::new();
    for a in args {
        let is_arr = host::with_host(|h| h.is_array(a));
        if is_arr {
            cols.push(MsortCol {
                arr: a.clone(),
                order: 1,
                sort_type: 0,
            });
        } else if let Some(col) = cols.last_mut() {
            // A flag following an array: SORT_ASC/DESC set direction, the type
            // flags set the comparison mode.
            let f = host::with_host(|h| h.to_number(a).to_int());
            match f {
                3 => col.order = -1, // SORT_DESC
                4 => col.order = 1,  // SORT_ASC
                0..=2 => col.sort_type = f,
                _ => {}
            }
        }
    }
    if cols.is_empty() {
        return Ok(Value::bool(false));
    }
    // Snapshot each column's values (multisort discards keys / reindexes).
    let data: Vec<Vec<Value>> = cols
        .iter()
        .map(|c| {
            host::with_host(|h| h.array_pairs(&c.arr))
                .unwrap_or_default()
                .into_iter()
                .map(|(_, v)| v)
                .collect()
        })
        .collect();
    let rows = data[0].len();
    if data.iter().any(|d| d.len() != rows) {
        // PHP requires equal sizes; mismatched inputs fail rather than panic.
        return Ok(Value::bool(false));
    }
    let mut order: Vec<usize> = (0..rows).collect();
    // `SORT_REGULAR` compares numerically when both sides look numeric and
    // lexically otherwise, which is not transitive across a mix of numeric and
    // non-numeric strings; `zend_sort` returns the permutation the reference does.
    crate::stdlib::zsort::zend_sort_stable(&mut order, |&x, &y| {
        for (ci, col) in cols.iter().enumerate() {
            let mut ord = multisort_cmp(&data[ci][x], &data[ci][y], col.sort_type);
            if col.order == -1 {
                ord = ord.reverse();
            }
            if ord != Ordering::Equal {
                return ord as i32;
            }
        }
        0
    });
    host::with_host(|h| {
        for (ci, col) in cols.iter().enumerate() {
            let vals: Vec<Value> = order.iter().map(|&i| data[ci][i].clone()).collect();
            h.arr_set_reindexed(&col.arr, vals);
        }
    });
    Ok(Value::bool(true))
}

/// Compare two values for `array_multisort` under a `SORT_*` type flag.
fn multisort_cmp(a: &Value, b: &Value, sort_type: i64) -> Ordering {
    host::with_host(|h| match sort_type {
        2 => h.to_str(a).cmp(&h.to_str(b)), // SORT_STRING
        1 => cmp_f64(h.to_number(a).to_float(), h.to_number(b).to_float()), // SORT_NUMERIC
        _ => {
            // SORT_REGULAR: numeric when both sides are numeric, else string.
            if is_numeric_val(h, a) && is_numeric_val(h, b) {
                cmp_f64(h.to_number(a).to_float(), h.to_number(b).to_float())
            } else {
                h.to_str(a).cmp(&h.to_str(b))
            }
        }
    })
}

/// Total order over `f64` for sort comparators (NaN sorts as equal).
fn cmp_f64(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

/// Whether `v` participates in a numeric `SORT_REGULAR` comparison: numbers, bools
/// and numeric strings do; everything else falls back to string comparison.
fn is_numeric_val(h: &host::PhpHost, v: &Value) -> bool {
    match v {
        Value::Int(_) | Value::Float(_) | Value::Bool(_) => true,
        Value::Str(_) => {
            let s = h.to_str(v);
            let t = s.trim();
            !t.is_empty() && t.parse::<f64>().is_ok()
        }
        _ => false,
    }
}

// ── str_word_count ───────────────────────────────────────────────────────────

/// `str_word_count($string, $format = 0, $characters = null)` — count words, or
/// return them. A word is a maximal run of alphabetic bytes that may also contain
/// (but not begin the string with) `'` and `-`, plus any extra bytes from the
/// `$characters` list. Faithful to PHP's `str_word_count` (`ext/standard`):
/// `format 0` returns the count, `1` a list of the words, `2` a
/// `byte-position => word` map. The first character of the whole string cannot be
/// `'` or `-` and the last cannot be `-`, unless the char list allows them.
///
/// SHADOWED: `builtins::call_library`'s core match currently owns the
/// `str_word_count` name with a whitespace-token count stub, so the stdlib chain
/// (and thus this faithful implementation, including the `$format`/`$characters`
/// modes) is only reached once that core entry is removed. The implementation is
/// kept complete and correct so it activates as soon as the shadow is lifted;
/// editing the core stub is out of scope for this module.
fn str_word_count(args: &[Value]) -> Result<Value, String> {
    let s = str_arg(args, 0);
    let bytes = s.as_bytes();
    let format = int_arg(args, 1);
    // Only 0 (count), 1 (list of words) and 2 (offset => word) exist.
    if !(0..=2).contains(&format) {
        return Err(throws(
            "ValueError",
            "str_word_count(): Argument #2 ($format) must be a valid format value",
        ));
    }
    let has_cl = args.len() > 2 && !matches!(arg(args, 2), Value::Undef);
    let charlist = if has_cl {
        str_arg(args, 2)
    } else {
        String::new()
    };
    // Upstream answers an empty subject BEFORE it builds the character mask
    // (php-src `ext/standard/string.c:6081`), so a malformed `..` range in
    // `$characters` raises no diagnostic when there is nothing to scan.
    let n = bytes.len();
    if n == 0 {
        return Ok(match format {
            1 | 2 => make_list(vec![]),
            _ => Value::int(0),
        });
    }

    let mask = host::with_host(|h| charmask(h, charlist.as_bytes(), "str_word_count"));
    let is_word =
        |c: u8| c.is_ascii_alphabetic() || (has_cl && mask[c as usize]) || c == b'\'' || c == b'-';

    let mut p = 0usize;
    let mut e = n;
    // First char cannot be ' or - (unless explicitly allowed); last cannot be -.
    if (bytes[0] == b'\'' && !(has_cl && mask[b'\'' as usize]))
        || (bytes[0] == b'-' && !(has_cl && mask[b'-' as usize]))
    {
        p += 1;
    }
    if bytes[e - 1] == b'-' && !(has_cl && mask[b'-' as usize]) {
        e -= 1;
    }

    let mut count = 0i64;
    let mut words: Vec<Value> = Vec::new();
    let mut pairs: Vec<(Value, Value)> = Vec::new();
    while p < e {
        let start = p;
        while p < e && is_word(bytes[p]) {
            p += 1;
        }
        if p > start {
            match format {
                1 => words.push(Value::str(
                    String::from_utf8_lossy(&bytes[start..p]).into_owned(),
                )),
                2 => pairs.push((
                    Value::int(start as i64),
                    Value::str(String::from_utf8_lossy(&bytes[start..p]).into_owned()),
                )),
                _ => count += 1,
            }
        }
        p += 1;
    }
    Ok(match format {
        1 => make_list(words),
        2 => make_map(pairs),
        _ => Value::int(count),
    })
}

// ── metaphone ────────────────────────────────────────────────────────────────

/// The per-letter code table from PHP's `ext/standard/metaphone.c` (`_codes`),
/// indexed by `A..Z`. The bit flags below classify each letter.
const MP_CODES: [u8; 26] = [
    1, 16, 4, 16, 9, 2, 4, 16, 9, 2, 0, 2, 2, 2, 1, 4, 0, 2, 4, 4, 1, 0, 0, 0, 8, 0,
];

/// The metaphone class bits for an already-uppercased letter (0 for non-letters).
fn mp_enc(c: u8) -> u8 {
    if c.is_ascii_uppercase() {
        MP_CODES[(c - b'A') as usize]
    } else {
        0
    }
}
fn mp_isvowel(c: u8) -> bool {
    mp_enc(c) & 1 != 0
}
fn mp_affecth(c: u8) -> bool {
    mp_enc(c) & 4 != 0
}
fn mp_makesoft(c: u8) -> bool {
    mp_enc(c) & 8 != 0
}
fn mp_noghtof(c: u8) -> bool {
    mp_enc(c) & 16 != 0
}
/// Look `how_far` letters ahead of `from`, stopping at the string end (returns the
/// uppercased byte, or `0` at/after the terminator) — PHP's `Look_Ahead_Letter`.
fn mp_lookahead(word: &[u8], from: usize, how_far: usize) -> u8 {
    let mut idx = 0;
    while byte_at(word, from + idx) != 0 && idx < how_far {
        idx += 1;
    }
    byte_at(word, from + idx).to_ascii_uppercase()
}

/// `metaphone($string, $phonemes = 0)` — a faithful port of PHP's traditional
/// metaphone (`ext/standard/metaphone.c`, called with `traditional = 1`).
/// `$phonemes` caps the output length (0 = unlimited). Non-letters are skipped;
/// input with no letters yields an empty string.
fn php_metaphone(input: &str, max_phonemes: i64) -> String {
    let word = input.as_bytes();
    let max = if max_phonemes < 0 {
        0
    } else {
        max_phonemes as usize
    };
    const SH: u8 = b'X';
    const TH: u8 = b'0';
    let mut out: Vec<u8> = Vec::new();
    let up = |i: usize| byte_at(word, i).to_ascii_uppercase();

    // Find the first letter (bail out on a letterless string).
    let mut w = 0usize;
    loop {
        let c = byte_at(word, w);
        if c == 0 {
            return String::new();
        }
        if c.is_ascii_alphabetic() {
            break;
        }
        w += 1;
    }

    // The first phoneme is special-cased.
    let first = up(w);
    match first {
        b'A' => {
            if up(w + 1) == b'E' {
                out.push(b'E');
                w += 2;
            } else {
                out.push(b'A');
                w += 1;
            }
        }
        b'G' | b'K' | b'P' => {
            if up(w + 1) == b'N' {
                out.push(b'N');
                w += 2;
            }
        }
        b'W' => {
            let nl = up(w + 1);
            if nl == b'R' {
                out.push(b'R');
                w += 2;
            } else if nl == b'H' || mp_isvowel(nl) {
                out.push(b'W');
                w += 2;
            }
        }
        b'X' => {
            out.push(b'S');
            w += 1;
        }
        b'E' | b'I' | b'O' | b'U' => {
            out.push(first);
            w += 1;
        }
        _ => {}
    }

    while byte_at(word, w) != 0 && (max == 0 || out.len() < max) {
        let raw_curr = byte_at(word, w);
        if !raw_curr.is_ascii_alphabetic() {
            w += 1;
            continue;
        }
        let curr = raw_curr.to_ascii_uppercase();
        let prev = if w >= 1 { up(w - 1) } else { 0 };
        // Drop duplicate letters, except CC.
        if curr == prev && curr != b'C' {
            w += 1;
            continue;
        }
        let next = up(w + 1);
        let after_next = if byte_at(word, w + 1) != 0 {
            up(w + 2)
        } else {
            0
        };
        let mut skip = 0usize;
        match curr {
            b'B' => {
                if prev != b'M' {
                    out.push(b'B');
                }
            }
            b'C' => {
                if mp_makesoft(next) {
                    if next == b'I' && after_next == b'A' {
                        out.push(SH);
                    } else if prev == b'S' {
                        // -SCI-/-SCE-/-SCY-: dropped (handled by the preceding S).
                    } else {
                        out.push(b'S');
                    }
                } else if next == b'H' {
                    // traditional mode: CH → 'sh'.
                    out.push(SH);
                    skip += 1;
                } else {
                    out.push(b'K');
                }
            }
            b'D' => {
                if next == b'G' && mp_makesoft(after_next) {
                    out.push(b'J');
                    skip += 1;
                } else {
                    out.push(b'T');
                }
            }
            b'G' => {
                if next == b'H' {
                    let lb3 = if w >= 3 { up(w - 3) } else { 0 };
                    let lb4 = if w >= 4 { up(w - 4) } else { 0 };
                    if !(mp_noghtof(lb3) || lb4 == b'H') {
                        out.push(b'F');
                        skip += 1;
                    }
                    // else silent
                } else if next == b'N' {
                    let brk = !after_next.is_ascii_alphabetic();
                    if brk || (after_next == b'E' && mp_lookahead(word, w, 3) == b'D') {
                        // -GN / -GNED: dropped
                    } else {
                        out.push(b'K');
                    }
                } else if mp_makesoft(next) && prev != b'G' {
                    out.push(b'J');
                } else {
                    out.push(b'K');
                }
            }
            b'H' => {
                if mp_isvowel(next) && !mp_affecth(prev) {
                    out.push(b'H');
                }
            }
            b'K' => {
                if prev != b'C' {
                    out.push(b'K');
                }
            }
            b'P' => {
                if next == b'H' {
                    out.push(b'F');
                } else {
                    out.push(b'P');
                }
            }
            b'Q' => out.push(b'K'),
            b'S' => {
                if next == b'I' && (after_next == b'O' || after_next == b'A') {
                    out.push(SH);
                } else if next == b'H' {
                    out.push(SH);
                    skip += 1;
                } else {
                    out.push(b'S');
                }
            }
            b'T' => {
                if next == b'I' && (after_next == b'O' || after_next == b'A') {
                    out.push(SH);
                } else if next == b'H' {
                    out.push(TH);
                    skip += 1;
                } else if !(next == b'C' && after_next == b'H') {
                    out.push(b'T');
                }
            }
            b'V' => out.push(b'F'),
            b'W' => {
                if mp_isvowel(next) {
                    out.push(b'W');
                }
            }
            b'X' => {
                out.push(b'K');
                out.push(b'S');
            }
            b'Y' => {
                if mp_isvowel(next) {
                    out.push(b'Y');
                }
            }
            b'Z' => out.push(b'S'),
            b'F' | b'J' | b'L' | b'M' | b'N' | b'R' => out.push(curr),
            _ => {}
        }
        w += skip + 1;
    }

    // Only ASCII phoneme bytes were pushed, so this is always valid UTF-8.
    String::from_utf8(out).unwrap_or_default()
}

// ── uniqid ───────────────────────────────────────────────────────────────────

/// `uniqid($prefix = "", $more_entropy = false)` — a time-based identifier.
/// Mirrors PHP's format: `$prefix` followed by 8 hex digits of the current epoch
/// seconds and 5 hex digits of microseconds; with `$more_entropy`, a further
/// `.` plus 8 fractional digits (`%.8F` of a pseudo-random `0..10` value).
///
/// LIMITATION: PHP seeds the entropy tail from `php_combined_lcg()`; without a VM
/// RNG hook here it is derived from the sub-microsecond clock instead. `uniqid`
/// is time-based and non-deterministic in PHP too, so only the shape is stable.
fn uniqid(args: &[Value]) -> Value {
    let prefix = if args.is_empty() {
        String::new()
    } else {
        str_arg(args, 0)
    };
    let more_entropy = args.len() > 1 && host::with_host(|h| h.is_truthy(&arg(args, 1)));
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let sec = now.as_secs() & 0xffff_ffff;
    let usec = now.subsec_micros() & 0xf_ffff;
    let mut s = format!("{prefix}{sec:08x}{usec:05x}");
    if more_entropy {
        // A 0..10 fractional value from the sub-microsecond clock, à la "%.8F".
        let frac = (now.subsec_nanos() % 1_000) as f64 / 1_000.0 * 10.0;
        s.push_str(&format!("{frac:.8}"));
    }
    Value::str(s)
}

// ── pack / unpack (ext/standard/pack.c) ──────────────────────────────────────
//
// Bytes are produced one per Latin-1 `char`, as `chr` produces them, and read
// back as the string's bytes, as `ord` and `strlen` read them (see the module
// note in `stdlib::encoding`): exact for every byte below 0x80.

/// `php_pack`: the low `size` bytes of the value as an integer, in `big`
/// or little-endian order.
fn pack_int(v: &Value, size: usize, big: bool, out: &mut Vec<u8>) {
    let n = crate::host::with_host(|h| h.to_number(v).to_int()) as u64;
    let le = n.to_le_bytes();
    if big {
        out.extend(le[..size].iter().rev());
    } else {
        out.extend_from_slice(&le[..size]);
    }
}

/// `PHP_FUNCTION(pack)`.
fn php_pack(args: &[Value]) -> Result<Value, String> {
    let format = str_arg(args, 0).into_bytes();
    let argv = &args[args.len().min(1)..];
    let num_args = argv.len() as i64;
    let mut codes: Vec<(u8, i64)> = Vec::new();
    let mut currentarg: i64 = 0;
    let mut i = 0;
    while i < format.len() {
        let code = format[i];
        i += 1;
        let mut arg: i64 = 1;
        if i < format.len() {
            let c = format[i];
            if c == b'*' {
                arg = -1;
                i += 1;
            } else if c.is_ascii_digit() {
                let start = i;
                while i < format.len() && format[i].is_ascii_digit() {
                    i += 1;
                }
                // `atoi`: a value past INT_MAX wraps, as the C does.
                arg = std::str::from_utf8(&format[start..i])
                    .ok()
                    .and_then(|s| s.parse::<i64>().ok())
                    .map_or(i32::MAX as i64, |v| v as i32 as i64);
            }
        }
        let c = code as char;
        match code {
            b'x' | b'X' | b'@' => {
                if arg < 0 {
                    crate::host::with_host(|h| h.warn(format!("pack(): Type {c}: '*' ignored")));
                    arg = 1;
                }
            }
            b'a' | b'A' | b'Z' | b'h' | b'H' => {
                if currentarg >= num_args {
                    return Err(throws(
                        "ValueError",
                        format!("Type {c}: not enough arguments"),
                    ));
                }
                if arg < 0 {
                    arg = str_arg(argv, currentarg as usize).len() as i64;
                    if code == b'Z' {
                        arg += 1;
                    }
                }
                currentarg += 1;
            }
            b'q' | b'Q' | b'J' | b'P' | b'c' | b'C' | b's' | b'S' | b'i' | b'I' | b'l' | b'L'
            | b'n' | b'N' | b'v' | b'V' | b'f' | b'g' | b'G' | b'd' | b'e' | b'E' => {
                if arg < 0 {
                    arg = num_args - currentarg;
                }
                currentarg += arg;
                if currentarg > num_args {
                    return Err(throws("ValueError", format!("Type {c}: too few arguments")));
                }
            }
            _ => {
                return Err(throws(
                    "ValueError",
                    format!("Type {c}: unknown format code"),
                ))
            }
        }
        codes.push((code, arg));
    }
    if currentarg < num_args {
        let unused = num_args - currentarg;
        crate::host::with_host(|h| h.warn(format!("pack(): {unused} arguments unused")));
    }
    // The size pass: only its overflow refusal and `X`'s warning are visible.
    let mut outputpos: i64 = 0;
    for &(code, arg) in &codes {
        let unit = match code {
            b'h' | b'H' => {
                let n = arg / 2 + arg % 2;
                if n < 0 || (i32::MAX as i64 - outputpos) < n {
                    return Err(throws(
                        "ValueError",
                        format!("Type {}: integer overflow in format string", code as char),
                    ));
                }
                outputpos += n;
                continue;
            }
            b'a' | b'A' | b'Z' | b'c' | b'C' | b'x' => 1,
            b's' | b'S' | b'n' | b'v' => 2,
            b'i' | b'I' | b'l' | b'L' | b'N' | b'V' | b'f' | b'g' | b'G' => 4,
            b'q' | b'Q' | b'J' | b'P' | b'd' | b'e' | b'E' => 8,
            b'X' => {
                outputpos -= arg;
                if outputpos < 0 {
                    crate::host::with_host(|h| {
                        h.warn(format!("pack(): Type {}: outside of string", code as char))
                    });
                    outputpos = 0;
                }
                continue;
            }
            b'@' => {
                outputpos = arg;
                continue;
            }
            _ => continue,
        };
        if arg < 0 || (i32::MAX as i64 - outputpos) / unit < arg {
            return Err(throws(
                "ValueError",
                format!("Type {}: integer overflow in format string", code as char),
            ));
        }
        outputpos += arg * unit;
    }
    let mut out: Vec<u8> = Vec::new();
    let mut ai = 0usize;
    let mut next = || {
        let v = arg(argv, ai);
        ai += 1;
        v
    };
    for &(code, arg) in &codes {
        let n = arg.max(0) as usize;
        match code {
            b'a' | b'A' | b'Z' => {
                let s = crate::host::with_host(|h| h.to_str(&next())).into_bytes();
                let cp = if code == b'Z' { n.saturating_sub(1) } else { n };
                let fill = if code == b'A' { b' ' } else { 0 };
                let start = out.len();
                out.resize(start + n, fill);
                let k = s.len().min(cp);
                out[start..start + k].copy_from_slice(&s[..k]);
            }
            b'h' | b'H' => {
                let s = crate::host::with_host(|h| h.to_str(&next())).into_bytes();
                let mut count = n;
                if count > s.len() {
                    let c = code as char;
                    crate::host::with_host(|h| {
                        h.warn(format!("pack(): Type {c}: not enough characters in string"))
                    });
                    count = s.len();
                }
                let mut shift = if code == b'h' { 0 } else { 4 };
                let mut first = true;
                for &ch in &s[..count] {
                    let d = match ch {
                        b'0'..=b'9' => ch - b'0',
                        b'A'..=b'F' => ch - b'A' + 10,
                        b'a'..=b'f' => ch - b'a' + 10,
                        _ => {
                            let (c, bad) = (code as char, ch as char);
                            crate::host::with_host(|h| {
                                h.warn(format!("pack(): Type {c}: illegal hex digit {bad}"))
                            });
                            0
                        }
                    };
                    if first {
                        out.push(0);
                    }
                    first = !first;
                    *out.last_mut().expect("a nibble byte") |= d << shift;
                    shift = (shift + 4) & 7;
                }
            }
            b'c' | b'C' => (0..n).for_each(|_| pack_int(&next(), 1, false, &mut out)),
            b's' | b'S' | b'v' => (0..n).for_each(|_| pack_int(&next(), 2, false, &mut out)),
            b'n' => (0..n).for_each(|_| pack_int(&next(), 2, true, &mut out)),
            b'i' | b'I' | b'l' | b'L' | b'V' => {
                (0..n).for_each(|_| pack_int(&next(), 4, false, &mut out))
            }
            b'N' => (0..n).for_each(|_| pack_int(&next(), 4, true, &mut out)),
            b'q' | b'Q' | b'P' => (0..n).for_each(|_| pack_int(&next(), 8, false, &mut out)),
            b'J' => (0..n).for_each(|_| pack_int(&next(), 8, true, &mut out)),
            b'f' | b'g' | b'G' => {
                for _ in 0..n {
                    let f = crate::host::with_host(|h| h.to_number(&next()).to_float()) as f32;
                    if code == b'G' {
                        out.extend_from_slice(&f.to_be_bytes());
                    } else {
                        out.extend_from_slice(&f.to_le_bytes());
                    }
                }
            }
            b'd' | b'e' | b'E' => {
                for _ in 0..n {
                    let f = crate::host::with_host(|h| h.to_number(&next()).to_float());
                    if code == b'E' {
                        out.extend_from_slice(&f.to_be_bytes());
                    } else {
                        out.extend_from_slice(&f.to_le_bytes());
                    }
                }
            }
            b'x' => out.resize(out.len() + n, 0),
            b'X' => out.truncate(out.len().saturating_sub(n)),
            b'@' => out.resize(n, 0),
            _ => {}
        }
    }
    Ok(Value::str(
        out.iter().map(|&b| b as char).collect::<String>(),
    ))
}

/// `PHP_FUNCTION(unpack)`.
fn php_unpack(args: &[Value]) -> Result<Value, String> {
    let format = str_arg(args, 0).into_bytes();
    let data = str_arg(args, 1).into_bytes();
    let offset = if args.len() > 2 { int_arg(args, 2) } else { 0 };
    if offset < 0 || offset > data.len() as i64 {
        return Err(throws(
            "ValueError",
            "unpack(): Argument #3 ($offset) must be contained in argument #2 ($data)",
        ));
    }
    let input = &data[offset as usize..];
    let inputlen = input.len() as i64;
    let mut inputpos: i64 = 0;
    let mut out: Vec<(Value, Value)> = Vec::new();
    let mut put = |k: Value, v: Value| match out.iter_mut().find(|(ok, _)| {
        crate::host::with_host(|h| h.to_str(ok)) == crate::host::with_host(|h| h.to_str(&k))
    }) {
        Some(slot) => slot.1 = v,
        None => out.push((k, v)),
    };
    let mut f = 0usize;
    while f < format.len() {
        let ty = format[f];
        f += 1;
        let c = ty as char;
        let mut repetitions: i64 = 1;
        if f < format.len() {
            let d = format[f];
            if d.is_ascii_digit() {
                let start = f;
                while f < format.len() && format[f].is_ascii_digit() {
                    f += 1;
                }
                match std::str::from_utf8(&format[start..f])
                    .ok()
                    .and_then(|s| s.parse::<i64>().ok())
                {
                    Some(v) if v <= i32::MAX as i64 => repetitions = v,
                    _ => {
                        crate::host::with_host(|h| {
                            h.warn(format!("unpack(): Type {c}: integer overflow"))
                        });
                        return Ok(Value::bool(false));
                    }
                }
            } else if d == b'*' {
                repetitions = -1;
                f += 1;
            }
        }
        let name_start = f;
        while f < format.len() && format[f] != b'/' {
            f += 1;
        }
        let name = &format[name_start..(name_start + (f - name_start).min(200))];
        let argb = repetitions;
        let mut size: i64 = match ty {
            b'X' => {
                if repetitions < 0 {
                    crate::host::with_host(|h| h.warn(format!("unpack(): Type {c}: '*' ignored")));
                    repetitions = 1;
                }
                -1
            }
            b'@' => 0,
            b'a' | b'A' | b'Z' => {
                let s = repetitions;
                repetitions = 1;
                s
            }
            b'h' | b'H' => {
                let s = if repetitions > 0 {
                    (repetitions + 1) / 2
                } else {
                    repetitions
                };
                repetitions = 1;
                s
            }
            b'c' | b'C' | b'x' => 1,
            b's' | b'S' | b'n' | b'v' => 2,
            b'i' | b'I' | b'l' | b'L' | b'N' | b'V' | b'f' | b'g' | b'G' => 4,
            b'q' | b'Q' | b'J' | b'P' | b'd' | b'e' | b'E' => 8,
            _ => return Err(throws("ValueError", format!("Invalid format type {c}"))),
        };
        let mut i: i64 = 0;
        while i != repetitions {
            if inputpos + size <= inputlen {
                let key = if name.is_empty() {
                    Value::int(i + 1)
                } else if repetitions == 1 {
                    Value::str(String::from_utf8_lossy(name).into_owned())
                } else {
                    Value::str(format!("{}{}", String::from_utf8_lossy(name), i + 1))
                };
                let at = inputpos.max(0) as usize;
                let bytes = |n: usize| -> [u8; 8] {
                    let mut b = [0u8; 8];
                    b[..n].copy_from_slice(&input[at..at + n]);
                    b
                };
                let latin1 =
                    |b: &[u8]| Value::str(b.iter().map(|&x| x as char).collect::<String>());
                let val = match ty {
                    b'a' | b'A' | b'Z' => {
                        let mut len = inputlen - inputpos;
                        if size >= 0 && len > size {
                            len = size;
                        }
                        size = len;
                        let mut s = &input[at..at + len as usize];
                        if ty == b'A' {
                            while let Some(&last) = s.last() {
                                if matches!(last, 0 | b' ' | b'\t' | b'\r' | b'\n') {
                                    s = &s[..s.len() - 1];
                                } else {
                                    break;
                                }
                            }
                        } else if ty == b'Z' {
                            if let Some(z) = s.iter().position(|&x| x == 0) {
                                s = &s[..z];
                            }
                        }
                        Some(latin1(s))
                    }
                    b'h' | b'H' => {
                        let mut len = (inputlen - inputpos) * 2;
                        if size >= 0 && len > size * 2 {
                            len = size * 2;
                        }
                        if len > 0 && argb > 0 {
                            len -= argb % 2;
                        }
                        let mut shift = if ty == b'h' { 0 } else { 4 };
                        let mut s = String::new();
                        for k in 0..len.max(0) as usize {
                            let byte = input[at + k / 2];
                            let nib = (byte >> shift) & 0xf;
                            s.push(char::from_digit(nib as u32, 16).expect("a nibble"));
                            shift = (shift + 4) & 7;
                        }
                        Some(Value::str(s))
                    }
                    b'c' => Some(Value::int(input[at] as i8 as i64)),
                    b'C' => Some(Value::int(input[at] as i64)),
                    b's' => Some(Value::int(
                        i16::from_le_bytes([input[at], input[at + 1]]) as i64
                    )),
                    b'S' | b'v' => Some(Value::int(
                        u16::from_le_bytes([input[at], input[at + 1]]) as i64
                    )),
                    b'n' => Some(Value::int(
                        u16::from_be_bytes([input[at], input[at + 1]]) as i64
                    )),
                    b'i' | b'l' => {
                        let b = bytes(4);
                        Some(Value::int(
                            i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as i64
                        ))
                    }
                    b'I' | b'L' | b'V' => {
                        let b = bytes(4);
                        Some(Value::int(
                            u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as i64
                        ))
                    }
                    b'N' => {
                        let b = bytes(4);
                        Some(Value::int(
                            u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as i64
                        ))
                    }
                    b'q' | b'Q' | b'P' => Some(Value::int(i64::from_le_bytes(bytes(8)))),
                    b'J' => Some(Value::int(i64::from_be_bytes(bytes(8)))),
                    b'f' | b'g' | b'G' => {
                        let b = bytes(4);
                        let raw = [b[0], b[1], b[2], b[3]];
                        let v = if ty == b'G' {
                            f32::from_be_bytes(raw)
                        } else {
                            f32::from_le_bytes(raw)
                        };
                        Some(Value::float(v as f64))
                    }
                    b'd' | b'e' | b'E' => {
                        let v = if ty == b'E' {
                            f64::from_be_bytes(bytes(8))
                        } else {
                            f64::from_le_bytes(bytes(8))
                        };
                        Some(Value::float(v))
                    }
                    b'x' => None,
                    b'X' => {
                        if inputpos < size {
                            inputpos = -size;
                            i = repetitions - 1;
                            if repetitions >= 0 {
                                crate::host::with_host(|h| {
                                    h.warn(format!("unpack(): Type {c}: outside of string"))
                                });
                            }
                        }
                        None
                    }
                    b'@' => {
                        if repetitions <= inputlen {
                            inputpos = repetitions;
                        } else {
                            crate::host::with_host(|h| {
                                h.warn(format!("unpack(): Type {c}: outside of string"))
                            });
                        }
                        i = repetitions - 1;
                        None
                    }
                    _ => None,
                };
                if let Some(v) = val {
                    put(key, v);
                }
                inputpos += size;
                if inputpos < 0 {
                    if size != -1 {
                        crate::host::with_host(|h| {
                            h.warn(format!("unpack(): Type {c}: outside of string"))
                        });
                    }
                    inputpos = 0;
                }
            } else if repetitions < 0 {
                break;
            } else {
                let left = inputlen - inputpos;
                let verb = if left == 1 { "was" } else { "were" };
                crate::host::with_host(|h| {
                    h.warn(format!(
                        "unpack(): Type {c}: not enough input values, need {size} values but only {left} {verb} provided"
                    ))
                });
                return Ok(Value::bool(false));
            }
            i += 1;
        }
        if f < format.len() {
            f += 1;
        }
    }
    Ok(make_map(out))
}

// ── version_compare (ext/standard/versioning.c) ─────────────────────────────

/// `isdigit` in the C locale.
fn ver_isdig(c: u8) -> bool {
    c.is_ascii_digit()
}

/// `isndig`: neither a digit nor the `.` separator.
fn ver_isndig(c: u8) -> bool {
    !c.is_ascii_digit() && c != b'.'
}

/// Port of `php_canonicalize_version`: `-`, `_`, `+` and any other
/// non-alphanumeric byte become a `.` (never two in a row), and a `.` is
/// inserted wherever a digit run meets a non-digit run; a trailing `.` is
/// dropped.
fn canonicalize_version(v: &[u8]) -> Vec<u8> {
    let mut q = Vec::with_capacity(v.len() * 2);
    let Some((&first, rest)) = v.split_first() else {
        return q;
    };
    q.push(first);
    let mut lp = first;
    for &p in rest {
        let last = *q.last().unwrap_or(&0);
        if matches!(p, b'-' | b'_' | b'+') {
            if last != b'.' {
                q.push(b'.');
            }
        } else if (ver_isndig(lp) && ver_isdig(p)) || (ver_isdig(lp) && ver_isndig(p)) {
            if last != b'.' {
                q.push(b'.');
            }
            q.push(p);
        } else if !p.is_ascii_alphanumeric() {
            if last != b'.' {
                q.push(b'.');
            }
        } else {
            q.push(p);
        }
        lp = p;
    }
    // A trailing `.` would leave an empty last component; upstream drops it.
    if q.last() == Some(&b'.') {
        q.pop();
    }
    q
}

/// Port of `compare_special_version_forms`: each form is ranked by the first
/// entry of the table it STARTS with (`strncmp` over the entry's length), and
/// an unranked form sorts below `dev`.
fn compare_special_version_forms(a: &[u8], b: &[u8]) -> i64 {
    const FORMS: &[(&[u8], i64)] = &[
        (b"dev", 0),
        (b"alpha", 1),
        (b"a", 1),
        (b"beta", 2),
        (b"b", 2),
        (b"RC", 3),
        (b"rc", 3),
        (b"#", 4),
        (b"pl", 5),
        (b"p", 5),
    ];
    let rank = |f: &[u8]| {
        FORMS
            .iter()
            .find(|(name, _)| f.starts_with(name))
            .map_or(-1, |&(_, order)| order)
    };
    (rank(a) - rank(b)).signum()
}

/// C `strtol(p, NULL, 10)` over a component that starts with a digit:
/// the leading digit run, saturating at `LONG_MAX` as `strtol` does.
fn ver_strtol(p: &[u8]) -> i64 {
    p.iter()
        .take_while(|c| c.is_ascii_digit())
        .fold(0i64, |n, &c| {
            n.saturating_mul(10).saturating_add(i64::from(c - b'0'))
        })
}

/// Port of `php_version_compare`: walk both canonical versions one
/// `.`-separated component at a time; numbers compare numerically, special
/// forms by rank, and a number outranks every special form except `pl`/`p`.
/// When one side runs out, its missing component counts as `#N#`.
fn php_version_compare(v1: &[u8], v2: &[u8]) -> i64 {
    if v1.is_empty() || v2.is_empty() {
        return match (v1.is_empty(), v2.is_empty()) {
            (true, true) => 0,
            (false, _) => 1,
            _ => -1,
        };
    }
    let canon = |v: &[u8]| {
        if v[0] == b'#' {
            v.to_vec()
        } else {
            canonicalize_version(v)
        }
    };
    let (ver1, ver2) = (canon(v1), canon(v2));
    // `p1`/`p2` are the start of the current component; `n1`/`n2` the index of
    // the `.` that ends it, `None` once a side has no further separator.
    let (mut p1, mut p2) = (0usize, 0usize);
    let (mut n1, mut n2): (Option<usize>, Option<usize>) = (Some(0), Some(0));
    let mut compare = 0;
    let end = |v: &[u8], from: usize| v[from..].iter().position(|&c| c == b'.').map(|i| from + i);
    let comp = |v: &[u8], from: usize, to: Option<usize>| v[from..to.unwrap_or(v.len())].to_vec();
    while p1 < ver1.len() && p2 < ver2.len() && n1.is_some() && n2.is_some() {
        n1 = end(&ver1, p1);
        n2 = end(&ver2, p2);
        let (c1, c2) = (comp(&ver1, p1, n1), comp(&ver2, p2, n2));
        let d1 = c1.first().is_some_and(|&c| ver_isdig(c));
        let d2 = c2.first().is_some_and(|&c| ver_isdig(c));
        compare = match (d1, d2) {
            (true, true) => (ver_strtol(&c1) - ver_strtol(&c2)).signum(),
            (false, false) => compare_special_version_forms(&c1, &c2),
            (true, false) => compare_special_version_forms(b"#N#", &c2),
            (false, true) => compare_special_version_forms(&c1, b"#N#"),
        };
        if compare != 0 {
            break;
        }
        if let Some(n) = n1 {
            p1 = n + 1;
        }
        if let Some(n) = n2 {
            p2 = n + 1;
        }
    }
    if compare == 0 {
        if n1.is_some() {
            compare = if ver1.get(p1).is_some_and(|&c| ver_isdig(c)) {
                1
            } else {
                php_version_compare(&ver1[p1.min(ver1.len())..], b"#N#")
            };
        } else if n2.is_some() {
            compare = if ver2.get(p2).is_some_and(|&c| ver_isdig(c)) {
                -1
            } else {
                php_version_compare(b"#N#", &ver2[p2.min(ver2.len())..])
            };
        }
    }
    compare
}

/// `version_compare($version1, $version2, $operator = null)` — the sign of
/// [`php_version_compare`], or, with an operator, whether it holds.
fn version_compare(args: &[Value]) -> Result<Value, String> {
    // Upstream reads both as C strings, so each ends at its first NUL.
    let cstr = |s: &str| s.split('\0').next().unwrap_or_default().to_string();
    let (v1, v2) = (str_arg(args, 0), str_arg(args, 1));
    let compare = php_version_compare(cstr(&v1).as_bytes(), cstr(&v2).as_bytes());
    let op = arg(args, 2);
    if matches!(op, Value::Undef) {
        return Ok(Value::int(compare));
    }
    let holds =
        match str_arg(args, 2).as_str() {
            "<" | "lt" => compare == -1,
            "<=" | "le" => compare != 1,
            ">" | "gt" => compare == 1,
            ">=" | "ge" => compare != -1,
            "==" | "eq" => compare == 0,
            "!=" | "<>" | "ne" => compare != 0,
            _ => return Err(throws(
                "ValueError",
                "version_compare(): Argument #3 ($operator) must be a valid comparison operator",
            )),
        };
    Ok(Value::Bool(holds))
}

// ── ip2long / long2ip / strcoll ──────────────────────────────────────────────

/// `ip2long($ip)` (`ext/standard/basic_functions.c`): the dotted quad as the
/// host-order integer of `inet_pton(AF_INET, …)`, or false. The platform's own
/// `inet_pton` is the parser, as it is the reference's — which is why a leading
/// zero (`"01.2.3.4"`) is accepted on BSD/macOS and refused by glibc.
fn ip2long(args: &[Value]) -> Result<Value, String> {
    let ip = str_arg(args, 0);
    // `Z_PARAM_PATH`.
    if ip.contains('\0') {
        return Err(throws(
            "ValueError",
            "ip2long(): Argument #1 ($ip) must not contain any null bytes",
        ));
    }
    if ip.is_empty() {
        return Ok(Value::bool(false));
    }
    let Ok(c) = std::ffi::CString::new(ip) else {
        return Ok(Value::bool(false));
    };
    extern "C" {
        // POSIX; the `libc` crate does not bind it.
        fn inet_pton(
            af: libc::c_int,
            src: *const libc::c_char,
            dst: *mut libc::c_void,
        ) -> libc::c_int;
    }
    let mut addr = libc::in_addr { s_addr: 0 };
    // SAFETY: `c` is a NUL-terminated string and `addr` an `in_addr`, which is
    // what `inet_pton(AF_INET, …)` writes.
    let ok = unsafe {
        inet_pton(
            libc::AF_INET,
            c.as_ptr(),
            (&mut addr as *mut libc::in_addr).cast(),
        )
    };
    Ok(if ok == 1 {
        Value::int(u32::from_be(addr.s_addr) as i64)
    } else {
        Value::bool(false)
    })
}

/// `long2ip($ip)`: the low 32 bits, network order, as `inet_ntop` prints them.
fn long2ip(ip: i64) -> String {
    std::net::Ipv4Addr::from(ip as u32).to_string()
}

/// `strcoll($string1, $string2)`: the C library's `strcoll` under the current
/// `LC_COLLATE`, its raw answer included (not only its sign). A string holding
/// a NUL compares up to it, as the C call does.
fn strcoll(a: &str, b: &str) -> i64 {
    let cut =
        |s: &str| std::ffi::CString::new(s.split('\0').next().unwrap_or("")).unwrap_or_default();
    let (a, b) = (cut(a), cut(b));
    // SAFETY: both are NUL-terminated C strings.
    unsafe { libc::strcoll(a.as_ptr(), b.as_ptr()) as i64 }
}
