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
        "strnatcmp" => str_natcmp(args, false),
        "strnatcasecmp" => str_natcmp(args, true),
        "soundex" => Value::str(soundex(&str_arg(args, 0))),
        "str_getcsv" => str_getcsv(args),
        "str_word_count" => str_word_count(args),
        "metaphone" => Value::str(php_metaphone(&str_arg(args, 0), int_arg(args, 1))),
        "uniqid" => uniqid(args),
        "array_walk_recursive" => return Some(array_walk_recursive(args)),
        "array_find" => return Some(array_find(args, false)),
        "array_find_key" => return Some(array_find(args, true)),
        "array_any" => return Some(array_predicate(args, Predicate::Any)),
        "array_all" => return Some(array_predicate(args, Predicate::All)),
        "array_udiff" => return Some(array_ucompare(args, false, false)),
        "array_uintersect" => return Some(array_ucompare(args, true, false)),
        "array_diff_ukey" => return Some(array_ucompare(args, false, true)),
        "array_intersect_ukey" => return Some(array_ucompare(args, true, true)),
        "array_multisort" => return Some(array_multisort(args)),
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
    if i < s.len() { s[i] } else { 0 }
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

/// `str_getcsv($string, $separator = ",", $enclosure = "\"", $escape = "\\")` —
/// parse a single CSV line into an array of fields. Follows PHP's parser: fields
/// may be wrapped in the enclosure (a doubled enclosure inside is a literal), the
/// escape character keeps itself AND the following character verbatim (PHP's
/// documented quirk — the escape byte is not stripped), and an empty input string
/// yields a single `null` field.
fn str_getcsv(args: &[Value]) -> Value {
    let s = str_arg(args, 0);
    if s.is_empty() {
        // PHP returns a one-element array holding null for a wholly empty line.
        return make_list(vec![Value::Undef]);
    }
    let sep = nth_char_or(args, 1, ',');
    let enc = nth_char_or(args, 2, '"');
    // An empty escape string disables escaping (PHP 7.4+ semantics).
    let esc = if args.len() > 3 {
        str_arg(args, 3).chars().next()
    } else {
        Some('\\')
    };

    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut fields: Vec<Value> = Vec::new();
    let mut i = 0usize;
    loop {
        let mut field = String::new();
        // Peek past leading blanks to decide if this field is enclosure-wrapped.
        let mut j = i;
        while j < n && (chars[j] == ' ' || chars[j] == '\t') {
            j += 1;
        }
        if j < n && chars[j] == enc {
            // Enclosed field: leading blanks are dropped, the enclosure opens.
            i = j + 1;
            while i < n {
                let c = chars[i];
                if esc == Some(c) && i + 1 < n {
                    field.push(c);
                    field.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                if c == enc {
                    if i + 1 < n && chars[i + 1] == enc {
                        field.push(enc);
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                field.push(c);
                i += 1;
            }
            // Anything between the closing enclosure and the separator is appended.
            while i < n && chars[i] != sep {
                field.push(chars[i]);
                i += 1;
            }
        } else {
            // Unenclosed field: blanks are part of the value; read up to separator.
            while i < n && chars[i] != sep {
                let c = chars[i];
                if esc == Some(c) && i + 1 < n {
                    field.push(c);
                    field.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                field.push(c);
                i += 1;
            }
        }
        fields.push(Value::str(field));
        if i < n && chars[i] == sep {
            i += 1; // consume the separator, then parse the next field
        } else {
            break;
        }
    }
    make_list(fields)
}

/// The first character of the `i`-th argument, or `default` when the argument is
/// missing or empty.
fn nth_char_or(args: &[Value], i: usize, default: char) -> char {
    if args.len() > i {
        str_arg(args, i).chars().next().unwrap_or(default)
    } else {
        default
    }
}

// ── array_walk_recursive ─────────────────────────────────────────────────────

/// `array_walk_recursive($array, $callback, $extra = null)` — call
/// `$callback($value, $key [, $extra])` for every **leaf** (non-array) element,
/// descending into nested arrays; always returns `true`.
///
/// LIMITATION (documented, not a bug fixable here): PHP's callback takes its value
/// parameter by reference (`function(&$value, $key)`) so it can rewrite each leaf
/// in place. phplang has **no by-reference parameters** — arguments pass by value
/// — so mutating a *scalar* leaf inside the callback does NOT write back into the
/// array, unlike PHP. A faithful fix needs VM-level by-reference OUT-parameters,
/// which live outside this module. (Object/array leaves are shared heap handles,
/// but this walker only ever invokes the callback on non-array leaves anyway.)
fn array_walk_recursive(args: &[Value]) -> Result<Value, String> {
    let arr = arg(args, 0);
    let cb = arg(args, 1);
    let extra = arg(args, 2);
    let has_extra = args.len() > 2;
    walk_recursive(&arr, &cb, has_extra.then(|| extra.clone()))?;
    if host::has_pending_throw() {
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
        let mut call_args = vec![v, k];
        if let Some(e) = &extra {
            call_args.push(e.clone());
        }
        host::call_value(cb.clone(), call_args)?;
        if host::has_pending_throw() {
            return Ok(());
        }
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
        if host::has_pending_throw() {
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
        if host::has_pending_throw() {
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

// ── array_udiff / array_uintersect / array_diff_ukey / array_intersect_ukey ──

/// The user-comparator diff/intersect family. `by_key` picks whether the callback
/// compares keys (`array_diff_ukey`/`array_intersect_ukey`) or values
/// (`array_udiff`/`array_uintersect`); `intersect` picks keep-if-present-in-all
/// vs keep-if-absent-from-all. The comparator is the LAST argument and every array
/// before it is one of the operands. Keys of the first array are preserved.
///
/// The callback runs the VM, so it is invoked outside any `with_host` borrow. This
/// is the O(n·m) direct form (compare each first-array element against every other
/// element); PHP sorts first to compare fewer pairs, but the observable result —
/// membership decided by the comparator returning `0` — is identical.
fn array_ucompare(args: &[Value], intersect: bool, by_key: bool) -> Result<Value, String> {
    let n = args.len();
    // Need at least: one operand, one other array, and the comparator.
    if n < 3 {
        return Ok(make_list(vec![]));
    }
    let cb = arg(args, n - 1);
    let first = host::with_host(|h| h.array_pairs(&arg(args, 0))).unwrap_or_default();
    // For each other array, collect the side (keys or values) we compare against.
    let others: Vec<Vec<Value>> = (1..n - 1)
        .map(|idx| {
            host::with_host(|h| h.array_pairs(&arg(args, idx)))
                .unwrap_or_default()
                .into_iter()
                .map(|(k, v)| if by_key { k } else { v })
                .collect()
        })
        .collect();

    let mut out: Vec<(Value, Value)> = Vec::new();
    for (k, v) in first {
        let probe = if by_key { k.clone() } else { v.clone() };
        let keep = if intersect {
            // Present (comparator == 0) in EVERY other array.
            let mut all = true;
            for o in &others {
                if !ucompare_member(&cb, &probe, o)? {
                    all = false;
                    break;
                }
                if host::has_pending_throw() {
                    return Ok(Value::Undef);
                }
            }
            all
        } else {
            // Absent from every other array.
            let mut absent = true;
            for o in &others {
                if ucompare_member(&cb, &probe, o)? {
                    absent = false;
                    break;
                }
                if host::has_pending_throw() {
                    return Ok(Value::Undef);
                }
            }
            absent
        };
        if host::has_pending_throw() {
            return Ok(Value::Undef);
        }
        if keep {
            out.push((k, v));
        }
    }
    Ok(make_map(out))
}

/// Does `probe` compare equal (callback returns `0`) to any element of `pool`?
fn ucompare_member(cb: &Value, probe: &Value, pool: &[Value]) -> Result<bool, String> {
    for item in pool {
        let r = host::call_value(cb.clone(), vec![probe.clone(), item.clone()])?;
        if host::has_pending_throw() {
            return Ok(false);
        }
        if host::with_host(|h| h.to_number(&r).to_int()) == 0 {
            return Ok(true);
        }
    }
    Ok(false)
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
    order.sort_by(|&x, &y| {
        for (ci, col) in cols.iter().enumerate() {
            let mut ord = multisort_cmp(&data[ci][x], &data[ci][y], col.sort_type);
            if col.order == -1 {
                ord = ord.reverse();
            }
            if ord != Ordering::Equal {
                return ord;
            }
        }
        Ordering::Equal
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
fn str_word_count(args: &[Value]) -> Value {
    let s = str_arg(args, 0);
    let bytes = s.as_bytes();
    let format = int_arg(args, 1);
    let has_cl = args.len() > 2 && !matches!(arg(args, 2), Value::Undef);
    let charlist = if has_cl {
        str_arg(args, 2)
    } else {
        String::new()
    };
    let mask = charmask(charlist.as_bytes());
    let is_word = |c: u8| {
        c.is_ascii_alphabetic() || (has_cl && mask[c as usize]) || c == b'\'' || c == b'-'
    };

    let n = bytes.len();
    if n == 0 {
        return match format {
            1 | 2 => make_list(vec![]),
            _ => Value::int(0),
        };
    }

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
                1 => words.push(Value::str(String::from_utf8_lossy(&bytes[start..p]).into_owned())),
                2 => pairs.push((
                    Value::int(start as i64),
                    Value::str(String::from_utf8_lossy(&bytes[start..p]).into_owned()),
                )),
                _ => count += 1,
            }
        }
        p += 1;
    }
    match format {
        1 => make_list(words),
        2 => make_map(pairs),
        _ => Value::int(count),
    }
}

/// Port of PHP's `php_charmask`: build a 256-entry membership table from a
/// character list that may contain `a..z` inclusive ranges.
fn charmask(list: &[u8]) -> [bool; 256] {
    let mut mask = [false; 256];
    let n = list.len();
    let mut i = 0;
    while i < n {
        let c = list[i];
        if i + 3 < n && list[i + 1] == b'.' && list[i + 2] == b'.' && list[i + 3] >= c {
            for x in c..=list[i + 3] {
                mask[x as usize] = true;
            }
            i += 4;
        } else {
            mask[c as usize] = true;
            i += 1;
        }
    }
    mask
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
