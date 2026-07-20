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

/// Dispatch a `misc`-category PHP function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let v = match name {
        "strnatcmp" => str_natcmp(args, false),
        "strnatcasecmp" => str_natcmp(args, true),
        "soundex" => Value::str(soundex(&str_arg(args, 0))),
        "str_getcsv" => str_getcsv(args),
        "array_walk_recursive" => return Some(array_walk_recursive(args)),
        "array_find" => return Some(array_find(args, false)),
        "array_find_key" => return Some(array_find(args, true)),
        "array_any" => return Some(array_predicate(args, Predicate::Any)),
        "array_all" => return Some(array_predicate(args, Predicate::All)),
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

/// Natural-order string comparison (Martin Pool's `strnatcmp` algorithm, as PHP
/// ports it): digit runs compare numerically, everything else by code point, and
/// whitespace is skipped. Leading-zero runs compare "fractionally" (left-aligned,
/// character by character) so `a010` < `a10`; runs without a leading zero compare
/// by magnitude (longer run wins, ties broken lexically).
fn nat_cmp(a: &str, b: &str, fold_case: bool) -> Ordering {
    let owned_a;
    let owned_b;
    let (a, b) = if fold_case {
        owned_a = a.to_lowercase();
        owned_b = b.to_lowercase();
        (owned_a.as_str(), owned_b.as_str())
    } else {
        (a, b)
    };
    let ca: Vec<char> = a.chars().collect();
    let cb: Vec<char> = b.chars().collect();
    let (mut i, mut j) = (0usize, 0usize);
    loop {
        while i < ca.len() && ca[i].is_whitespace() {
            i += 1;
        }
        while j < cb.len() && cb[j].is_whitespace() {
            j += 1;
        }
        if i >= ca.len() || j >= cb.len() {
            break;
        }
        if ca[i].is_ascii_digit() && cb[j].is_ascii_digit() {
            let mut i2 = i;
            while i2 < ca.len() && ca[i2].is_ascii_digit() {
                i2 += 1;
            }
            let mut j2 = j;
            while j2 < cb.len() && cb[j2].is_ascii_digit() {
                j2 += 1;
            }
            let na: &[char] = &ca[i..i2];
            let nb: &[char] = &cb[j..j2];
            let fractional = na.first() == Some(&'0') || nb.first() == Some(&'0');
            let cmp = if fractional {
                na.cmp(nb)
            } else if na.len() != nb.len() {
                na.len().cmp(&nb.len())
            } else {
                na.cmp(nb)
            };
            if cmp != Ordering::Equal {
                return cmp;
            }
            i = i2;
            j = j2;
            continue;
        }
        if ca[i] != cb[j] {
            return ca[i].cmp(&cb[j]);
        }
        i += 1;
        j += 1;
    }
    (ca.len() - i).cmp(&(cb.len() - j))
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
