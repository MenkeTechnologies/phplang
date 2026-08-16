//! PHP standard-library `arrays` functions. Part of the `stdlib` chain; see
//! `src/stdlib/mod.rs`. `dispatch` returns `None` for names it does not handle.
//!
//! The functions the language core already provides (`count`, `array_map`,
//! `sort`, …) are handled before this module is consulted, so only their close
//! siblings live here. Arrays are heap objects reached by a `Value::Obj` handle,
//! so the by-reference mutators (`usort`, `shuffle`, the internal-pointer family,
//! …) mutate the array behind the handle in place, exactly as `sort` does in the
//! core — the effect is visible through the caller's `$var`.

use crate::host;
use crate::stdlib::common::*;
use fusevm::Value;
use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// Dispatch an `arrays`-category PHP function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let v = match name {
        "array_column" => host::with_host(|h| php_array_column(h, args)),
        "array_chunk" => match php_array_chunk(args) {
            Ok(v) => v,
            e => return Some(e),
        },
        "array_fill_keys" => host::with_host(|h| php_array_fill_keys(h, args)),
        "array_pad" => host::with_host(|h| php_array_pad(h, args)),
        "array_key_first" => host::with_host(|h| php_array_key_first(h, args)),
        "array_key_last" => host::with_host(|h| php_array_key_last(h, args)),
        "array_is_list" => host::with_host(|h| php_array_is_list(h, args)),
        "array_diff_key" => host::with_host(|h| php_diff_intersect_key(h, args, false)),
        "array_intersect_key" => host::with_host(|h| php_diff_intersect_key(h, args, true)),
        "array_diff_assoc" => host::with_host(|h| php_diff_intersect_assoc(h, args, false)),
        "array_intersect_assoc" => host::with_host(|h| php_diff_intersect_assoc(h, args, true)),
        "array_merge_recursive" => host::with_host(|h| php_array_merge_recursive(h, args)),
        "array_replace" => host::with_host(|h| php_array_replace(h, args)),
        "array_count_values" => host::with_host(|h| php_array_count_values(h, args)),
        "usort" => return Some(php_usort(args, SortKind::List)),
        "uasort" => return Some(php_usort(args, SortKind::Assoc)),
        "uksort" => return Some(php_usort(args, SortKind::Key)),
        "natsort" => host::with_host(|h| php_natsort(h, args, false)),
        "natcasesort" => host::with_host(|h| php_natsort(h, args, true)),
        "shuffle" => host::with_host(|h| php_shuffle(h, args)),
        "array_rand" => match host::with_host(|h| php_array_rand(h, args)) {
            Ok(v) => v,
            e => return Some(e),
        },
        "array_walk" => return Some(php_array_walk(args)),
        "compact" => host::with_host(|h| php_compact(h, args)),
        "extract" => host::with_host(|h| php_extract(h, args)),
        "end" => host::with_host(|h| ptr_end(h, args)),
        "reset" => host::with_host(|h| ptr_reset(h, args)),
        "current" | "pos" => host::with_host(|h| ptr_current(h, args)),
        "key" => host::with_host(|h| ptr_key(h, args)),
        "next" => host::with_host(|h| ptr_step(h, args, 1)),
        "prev" => host::with_host(|h| ptr_step(h, args, -1)),
        _ => return None,
    };
    Some(Ok(v))
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// The heap handle of an array value, or `None` if `v` is not an object handle.
fn as_handle(v: &Value) -> Option<u32> {
    match v {
        Value::Obj(h) => Some(*h),
        _ => None,
    }
}

/// Deep-copy a value: arrays are cloned into fresh handles (recursively) so the
/// result never shares storage with the source. Scalars clone directly. Used by
/// `array_merge_recursive`, which — unlike `array_merge` — must own its nested
/// arrays to merge into them without mutating the inputs.
fn deep_copy(h: &mut host::PhpHost, v: &Value) -> Value {
    deep_copy_seen(h, v, &mut host::Visiting::default())
}

/// [`deep_copy`] with a cycle guard: a self-referential array is copied one
/// level and the repeat becomes `null`, instead of recursing until the native
/// stack is gone.
fn deep_copy_seen(h: &mut host::PhpHost, v: &Value, seen: &mut host::Visiting) -> Value {
    if h.is_array(v) {
        if !seen.enter(v) {
            return Value::Undef;
        }
        let pairs = h.array_pairs(v).unwrap_or_default();
        let out = h.new_array();
        for (k, val) in pairs {
            let cv = deep_copy_seen(h, &val, seen);
            h.arr_set_key(&out, &k, cv);
        }
        seen.leave();
        out
    } else {
        v.clone()
    }
}

// ── array_column ─────────────────────────────────────────────────────────────

/// `array_column($rows, $column_key, $index_key = null)` — pluck one column out
/// of a list of rows, optionally re-keying by another column. A `null` column key
/// keeps the whole row (PHP 7+). Rows missing the column are skipped.
fn php_array_column(h: &mut host::PhpHost, args: &[Value]) -> Value {
    let rows = arg(args, 0);
    let col = arg(args, 1);
    let idx = arg(args, 2);
    let col_all = matches!(col, Value::Undef);
    let has_idx = args.len() > 2 && !matches!(idx, Value::Undef);
    let out = h.new_array();
    for (_, row) in h.array_pairs(&rows).unwrap_or_default() {
        // The plucked value: the whole row for a null column, else the element at
        // `col` — skipping any row that lacks that column.
        let value = if col_all {
            row.clone()
        } else if let Some(v) = row_get(h, &row, &col) {
            v
        } else {
            continue;
        };
        match if has_idx {
            row_get(h, &row, &idx)
        } else {
            None
        } {
            Some(k) => h.arr_set_key(&out, &k, value),
            None => h.arr_push_auto(&out, value),
        }
    }
    out
}

/// Read `field` out of a row (an array or an object), returning `None` when the
/// field is absent so the caller can skip the row.
fn row_get(h: &host::PhpHost, row: &Value, field: &Value) -> Option<Value> {
    if h.is_array(row) {
        let want = h.to_str(field);
        for (k, v) in h.array_pairs(row).unwrap_or_default() {
            if h.to_str(&k) == want {
                return Some(v);
            }
        }
        return None;
    }
    if h.is_object(row) {
        let v = h.prop_get(row, &h.to_str(field));
        if !matches!(v, Value::Undef) {
            return Some(v);
        }
    }
    None
}

// ── array_chunk ──────────────────────────────────────────────────────────────

/// `array_chunk($array, $size, $preserve_keys = false)` — split into a list of
/// sub-arrays of at most `$size` elements. `$size < 1` is a fatal error in PHP.
fn php_array_chunk(args: &[Value]) -> Result<Value, String> {
    let arr = arg(args, 0);
    let size = int_arg(args, 1);
    if size < 1 {
        return Err(throws(
            "ValueError",
            "array_chunk(): Argument #2 ($length) must be greater than 0",
        ));
    }
    let preserve = args.get(2).map(|v| host::with_host(|h| h.is_truthy(v)));
    let preserve = preserve.unwrap_or(false);
    Ok(host::with_host(|h| {
        let pairs = h.array_pairs(&arr).unwrap_or_default();
        let out = h.new_array();
        for chunk in pairs.chunks(size as usize) {
            let sub = h.new_array();
            for (k, v) in chunk {
                if preserve {
                    h.arr_set_key(&sub, k, v.clone());
                } else {
                    h.arr_push_auto(&sub, v.clone());
                }
            }
            h.arr_push_auto(&out, sub);
        }
        out
    }))
}

// ── array_fill_keys ──────────────────────────────────────────────────────────

/// `array_fill_keys($keys, $value)` — a new array using each element of `$keys`
/// as a key, every entry set to `$value`.
fn php_array_fill_keys(h: &mut host::PhpHost, args: &[Value]) -> Value {
    let keys = arg(args, 0);
    let value = arg(args, 1);
    let out = h.new_array();
    for (_, k) in h.array_pairs(&keys).unwrap_or_default() {
        h.arr_set_key(&out, &k, value.clone());
    }
    out
}

// ── array_pad ────────────────────────────────────────────────────────────────

/// `array_pad($array, $size, $value)` — pad to `abs($size)` elements with
/// `$value`; a positive size pads on the right, a negative one on the left. The
/// result renumbers integer keys and preserves string keys (as `array_merge`).
fn php_array_pad(h: &mut host::PhpHost, args: &[Value]) -> Value {
    let arr = arg(args, 0);
    let size = h.to_number(&arg(args, 1)).to_int();
    let value = arg(args, 2);
    let pairs = h.array_pairs(&arr).unwrap_or_default();
    let need = size.unsigned_abs() as usize;
    let pad = need.saturating_sub(pairs.len());
    let out = h.new_array();
    let emit = |h: &mut host::PhpHost, out: &Value, pairs: &[(Value, Value)]| {
        for (k, v) in pairs {
            match k {
                Value::Int(_) => h.arr_push_auto(out, v.clone()),
                _ => h.arr_set_key(out, k, v.clone()),
            }
        }
    };
    if size < 0 {
        for _ in 0..pad {
            h.arr_push_auto(&out, value.clone());
        }
        emit(h, &out, &pairs);
    } else {
        emit(h, &out, &pairs);
        for _ in 0..pad {
            h.arr_push_auto(&out, value.clone());
        }
    }
    out
}

// ── array_key_first / array_key_last / array_is_list ─────────────────────────

fn php_array_key_first(h: &mut host::PhpHost, args: &[Value]) -> Value {
    h.array_pairs(&arg(args, 0))
        .and_then(|p| p.into_iter().next())
        .map(|(k, _)| k)
        .unwrap_or(Value::Undef)
}

fn php_array_key_last(h: &mut host::PhpHost, args: &[Value]) -> Value {
    h.array_pairs(&arg(args, 0))
        .and_then(|p| p.into_iter().next_back())
        .map(|(k, _)| k)
        .unwrap_or(Value::Undef)
}

/// `array_is_list` — true when the keys are exactly `0,1,…,n-1` in order.
fn php_array_is_list(h: &mut host::PhpHost, args: &[Value]) -> Value {
    let v = arg(args, 0);
    if !h.is_array(&v) {
        return Value::bool(false);
    }
    let is_list = h
        .array_pairs(&v)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .all(|(i, (k, _))| matches!(k, Value::Int(n) if *n == i as i64));
    Value::bool(is_list)
}

// ── key/assoc diff & intersect ───────────────────────────────────────────────

/// `array_diff_key` (`intersect=false`) / `array_intersect_key` (`intersect=true`)
/// — filter the first array by whether each key is absent from every other array
/// (diff) or present in all of them (intersect). Keys compare by string form.
fn php_diff_intersect_key(h: &mut host::PhpHost, args: &[Value], intersect: bool) -> Value {
    let first = h.array_pairs(&arg(args, 0)).unwrap_or_default();
    let others: Vec<std::collections::HashSet<String>> = args
        .iter()
        .skip(1)
        .map(|a| {
            h.array_pairs(a)
                .unwrap_or_default()
                .into_iter()
                .map(|(k, _)| h.to_str(&k))
                .collect()
        })
        .collect();
    let out = h.new_array();
    for (k, v) in first {
        let ks = h.to_str(&k);
        let keep = if intersect {
            others.iter().all(|s| s.contains(&ks))
        } else {
            others.iter().all(|s| !s.contains(&ks))
        };
        if keep {
            h.arr_set_key(&out, &k, v);
        }
    }
    out
}

/// `array_diff_assoc` (`intersect=false`) / `array_intersect_assoc`
/// (`intersect=true`) — like the key variants but matching on key *and* value
/// (both compared by string form).
fn php_diff_intersect_assoc(h: &mut host::PhpHost, args: &[Value], intersect: bool) -> Value {
    let first = h.array_pairs(&arg(args, 0)).unwrap_or_default();
    let others: Vec<HashMap<String, String>> = args
        .iter()
        .skip(1)
        .map(|a| {
            h.array_pairs(a)
                .unwrap_or_default()
                .into_iter()
                .map(|(k, v)| (h.to_str(&k), h.to_str(&v)))
                .collect()
        })
        .collect();
    let out = h.new_array();
    for (k, v) in first {
        let ks = h.to_str(&k);
        let vs = h.to_str(&v);
        let matches_other = |m: &HashMap<String, String>| m.get(&ks) == Some(&vs);
        let keep = if intersect {
            others.iter().all(matches_other)
        } else {
            others.iter().all(|m| !matches_other(m))
        };
        if keep {
            h.arr_set_key(&out, &k, v);
        }
    }
    out
}

// ── array_merge_recursive ────────────────────────────────────────────────────

/// `array_merge_recursive(...)` — merge arrays, recursing into shared string
/// keys: two arrays under one key merge; anything else under a duplicate string
/// key is gathered into a flat list. Integer keys always append.
fn php_array_merge_recursive(h: &mut host::PhpHost, args: &[Value]) -> Value {
    let acc = h.new_array();
    for a in args {
        if h.is_array(a) {
            merge_recursive_into(h, &acc, a);
        }
    }
    acc
}

/// Merge every entry of `src` into the owned accumulator `acc` (see
/// `array_merge_recursive`). `acc`'s array values are always deep copies, so
/// recursing into them never touches the caller's inputs.
fn merge_recursive_into(h: &mut host::PhpHost, acc: &Value, src: &Value) {
    for (k, v) in h.array_pairs(src).unwrap_or_default() {
        match k {
            Value::Int(_) => {
                let cv = deep_copy(h, &v);
                h.arr_push_auto(acc, cv);
            }
            _ => {
                let existing = existing_by_key(h, acc, &k);
                match existing {
                    None => {
                        let cv = deep_copy(h, &v);
                        h.arr_set_key(acc, &k, cv);
                    }
                    Some(ev) if h.is_array(&ev) && h.is_array(&v) => {
                        // Guard the mutual walk the same way: `$a[] = &$a` makes
                        // `ev` and `v` the same handle, and the merge would
                        // otherwise descend into itself forever.
                        if ev != v {
                            merge_recursive_into(h, &ev, &v);
                        }
                    }
                    Some(ev) => {
                        // Not both arrays: flatten the existing value and the new
                        // one into a single fresh list.
                        let combined = h.new_array();
                        for item in as_value_list(h, &ev) {
                            let ci = deep_copy(h, &item);
                            h.arr_push_auto(&combined, ci);
                        }
                        for item in as_value_list(h, &v) {
                            let ci = deep_copy(h, &item);
                            h.arr_push_auto(&combined, ci);
                        }
                        h.arr_set_key(acc, &k, combined);
                    }
                }
            }
        }
    }
}

/// The value already stored under `key` in `acc` (by string-form key match), or
/// `None` if that key is absent.
fn existing_by_key(h: &host::PhpHost, acc: &Value, key: &Value) -> Option<Value> {
    let want = h.to_str(key);
    h.array_pairs(acc)
        .unwrap_or_default()
        .into_iter()
        .find(|(k, _)| h.to_str(k) == want)
        .map(|(_, v)| v)
}

/// A value seen as a list of values: an array's values, or a one-element list
/// holding the scalar itself.
fn as_value_list(h: &host::PhpHost, v: &Value) -> Vec<Value> {
    if h.is_array(v) {
        h.array_pairs(v)
            .unwrap_or_default()
            .into_iter()
            .map(|(_, x)| x)
            .collect()
    } else {
        vec![v.clone()]
    }
}

// ── array_replace ────────────────────────────────────────────────────────────

/// `array_replace($base, ...$rest)` — start from `$base`, then overwrite/insert
/// every entry of each later array by key (no recursion).
fn php_array_replace(h: &mut host::PhpHost, args: &[Value]) -> Value {
    let out = h.new_array();
    for a in args {
        for (k, v) in h.array_pairs(a).unwrap_or_default() {
            h.arr_set_key(&out, &k, v);
        }
    }
    out
}

// ── array_count_values ───────────────────────────────────────────────────────

/// `array_count_values($array)` — tally occurrences of the values (only int and
/// string values are counted, matching PHP; others are skipped).
fn php_array_count_values(h: &mut host::PhpHost, args: &[Value]) -> Value {
    let pairs = h.array_pairs(&arg(args, 0)).unwrap_or_default();
    let out = h.new_array();
    for (_, v) in pairs {
        if !matches!(v, Value::Int(_) | Value::Str(_)) {
            continue;
        }
        let prev = h.index_get(&out, &v);
        let n = match prev {
            Value::Undef => 1,
            other => other.to_int() + 1,
        };
        h.arr_set_key(&out, &v, Value::int(n));
    }
    out
}

// ── usort / uasort / uksort ──────────────────────────────────────────────────

enum SortKind {
    /// `usort` — sort by value, then reindex 0..n.
    List,
    /// `uasort` — sort by value, preserve keys.
    Assoc,
    /// `uksort` — sort by key, preserve keys.
    Key,
}

/// The user-comparator sorts. The comparator runs the VM (via `call_value`), so
/// it must be invoked *outside* any `with_host` borrow; the first error/throw it
/// raises stops the sort. PHP's sort is stable, as is Rust's `sort_by`.
fn php_usort(args: &[Value], kind: SortKind) -> Result<Value, String> {
    let arr = arg(args, 0);
    let cb = arg(args, 1);
    let mut pairs = host::with_host(|h| h.array_pairs(&arr)).unwrap_or_default();
    let mut err: Option<String> = None;
    // `stable_sort_by`, not `Vec::sort_by`: the comparator is arbitrary user
    // code, and Rust's sort PANICS when it catches one contradicting itself.
    stable_sort_by(&mut pairs, |(ka, va), (kb, vb)| {
        if err.is_some() {
            return Ordering::Equal;
        }
        let (a, b) = match kind {
            SortKind::Key => (ka.clone(), kb.clone()),
            _ => (va.clone(), vb.clone()),
        };
        match host::call_value(cb.clone(), vec![a, b]) {
            Ok(r) => host::with_host(|h| h.to_number(&r).to_int()).cmp(&0),
            Err(e) => {
                err = Some(e);
                Ordering::Equal
            }
        }
    });
    if let Some(e) = err {
        return Err(e);
    }
    if host::unwinding() {
        return Ok(Value::Undef);
    }
    host::with_host(|h| match kind {
        SortKind::List => {
            let vals = pairs.into_iter().map(|(_, v)| v).collect();
            h.arr_set_reindexed(&arr, vals);
        }
        _ => h.arr_set_pairs(&arr, pairs),
    });
    Ok(Value::bool(true))
}

// ── natsort / natcasesort ────────────────────────────────────────────────────

/// `natsort` / `natcasesort` — natural-order sort by value, preserving keys.
fn php_natsort(h: &mut host::PhpHost, args: &[Value], fold_case: bool) -> Value {
    let arr = arg(args, 0);
    let mut pairs = h.array_pairs(&arr).unwrap_or_default();
    let keyed: Vec<(String, (Value, Value))> = pairs
        .drain(..)
        .map(|(k, v)| (h.to_str(&v), (k, v)))
        .collect();
    let mut keyed = keyed;
    keyed.sort_by(|(a, _), (b, _)| nat_cmp(a, b, fold_case));
    let sorted: Vec<(Value, Value)> = keyed.into_iter().map(|(_, kv)| kv).collect();
    h.arr_set_pairs(&arr, sorted);
    Value::bool(true)
}

/// Natural-order string comparison (PHP `strnatcmp`): digit runs compare
/// numerically, everything else by code point; leading whitespace is skipped.
pub fn nat_cmp(a: &str, b: &str, fold_case: bool) -> Ordering {
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
            // Compare the digit runs the way PHP `strnatcmp` does (Martin Pool's
            // algorithm). If either run has a leading zero the comparison is
            // "fractional": the runs are compared left-aligned, character by
            // character, so the first differing digit decides and a shorter run
            // that is a prefix of the other sorts first. That makes "010" < "10"
            // ('0' < '1') instead of folding both to 10. Otherwise (neither run
            // has a leading zero) the runs compare by magnitude: the longer run
            // wins, and at equal length the lexical order of the digits decides.
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

// ── shuffle / array_rand ─────────────────────────────────────────────────────

thread_local! {
    /// xorshift64 state for `shuffle`/`array_rand`, lazily seeded from the clock.
    static RNG: Cell<u64> = const { Cell::new(0) };
}

/// A pseudo-random `u64` (xorshift64). Seeded lazily from the wall clock the
/// first time it runs; adequate for `shuffle`/`array_rand` (no crypto use).
fn next_rand() -> u64 {
    RNG.with(|r| {
        let mut x = r.get();
        if x == 0 {
            use std::time::{SystemTime, UNIX_EPOCH};
            x = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x9e37_79b9_7f4a_7c15)
                | 1;
        }
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        r.set(x);
        x
    })
}

/// A uniform-ish index in `0..n` (`n > 0`).
fn rand_below(n: usize) -> usize {
    (next_rand() % n as u64) as usize
}

/// `shuffle($array)` — randomize order and reindex 0..n in place; returns `true`.
fn php_shuffle(h: &mut host::PhpHost, args: &[Value]) -> Value {
    let arr = arg(args, 0);
    let mut vals: Vec<Value> = h
        .array_pairs(&arr)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, v)| v)
        .collect();
    // Fisher-Yates.
    for i in (1..vals.len()).rev() {
        let j = rand_below(i + 1);
        vals.swap(i, j);
    }
    h.arr_set_reindexed(&arr, vals);
    Value::bool(true)
}

/// `array_rand($array, $num = 1)` — a random key (num == 1) or an array of `num`
/// distinct keys (in original order). `$num` outside `1..=count` is a fatal error.
fn php_array_rand(h: &mut host::PhpHost, args: &[Value]) -> Result<Value, String> {
    let arr = arg(args, 0);
    let keys: Vec<Value> = h
        .array_pairs(&arr)
        .unwrap_or_default()
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    let len = keys.len();
    let num = if args.len() > 1 {
        h.to_number(&arg(args, 1)).to_int()
    } else {
        1
    };
    // An EMPTY array is reported against argument #1, not #2: there is no
    // `$num` that would work, so the reference names the array instead.
    if len == 0 {
        return Err(throws(
            "ValueError",
            "array_rand(): Argument #1 ($array) must not be empty",
        ));
    }
    if num < 1 || num as usize > len {
        return Err(throws(
            "ValueError",
            "array_rand(): Argument #2 ($num) must be between 1 and the number of elements in \
             argument #1 ($array)",
        ));
    }
    let num = num as usize;
    if num == 1 {
        return Ok(keys[rand_below(len)].clone());
    }
    // Partial Fisher-Yates to pick `num` distinct indices, then restore original
    // order (PHP yields the keys in the order they appear in the array).
    let mut idx: Vec<usize> = (0..len).collect();
    for i in 0..num {
        let j = i + rand_below(len - i);
        idx.swap(i, j);
    }
    let mut chosen = idx[..num].to_vec();
    chosen.sort_unstable();
    let out = h.new_array();
    for i in chosen {
        h.arr_push_auto(&out, keys[i].clone());
    }
    Ok(out)
}

// ── array_walk ───────────────────────────────────────────────────────────────

/// `array_walk($array, $callback, $extra = null)` — call `$callback($value, $key
/// [, $extra])` for every element; always returns `true`.
///
/// LIMITATION (documented, not a bug we can fix here): in PHP the callback's
/// first parameter is by-reference (`function(&$value, $key)`), so the callback
/// can modify each element in place. phplang has **no by-reference parameters** —
/// every argument is passed by value — so a *scalar* mutated inside the callback
/// does NOT propagate back into the array, unlike PHP. Fixing this faithfully
/// would require VM-level by-reference OUT-parameters, which the VM does not
/// provide and which live outside this module. Array/object elements still
/// reflect mutations, because those are shared heap handles rather than scalars.
fn php_array_walk(args: &[Value]) -> Result<Value, String> {
    let arr = arg(args, 0);
    let cb = arg(args, 1);
    let extra = arg(args, 2);
    let has_extra = args.len() > 2;
    let pairs = host::with_host(|h| h.array_pairs(&arr)).unwrap_or_default();
    for (k, v) in pairs {
        // The value goes in through a reference cell so a `function (&$v)`
        // callback can write to it; anything it stores is copied back into the
        // array under the same key.
        let (cell, slot) = host::with_host(|h| h.new_ref_cell(v));
        let mut call_args = vec![cell, k.clone()];
        if has_extra {
            call_args.push(extra.clone());
        }
        host::call_value(cb.clone(), call_args)?;
        if host::unwinding() {
            return Ok(Value::Undef);
        }
        host::with_host(|h| {
            let updated = h.ref_cell_value(slot);
            h.arr_set_key(&arr, &k, updated);
        });
    }
    Ok(Value::bool(true))
}

// ── compact / extract ────────────────────────────────────────────────────────

/// `compact(...)` — build an array from the given variable names, pulling each
/// bound value out of the current scope. A name may itself be an array of names
/// (recursively). A name that is not bound is skipped WITH a warning, and an
/// argument that is neither a string nor an array is skipped with a different
/// one — `compact()` is diagnostic-heavy and silently dropping either is how a
/// misspelled name goes unnoticed.
fn php_compact(h: &mut host::PhpHost, args: &[Value]) -> Value {
    let out = h.new_array();
    for (i, a) in args.iter().enumerate() {
        // `php_compact_var` carries the TOP-LEVEL argument number into the
        // recursion, so a bad entry nested inside an array of names is still
        // reported against the argument that array was passed as.
        compact_add(h, &out, a, i + 1);
    }
    out
}

fn compact_add(h: &mut host::PhpHost, out: &Value, name: &Value, pos: usize) {
    if h.is_array(name) {
        for (_, n) in h.array_pairs(name).unwrap_or_default() {
            compact_add(h, out, &n, pos);
        }
        return;
    }
    let Value::Str(n) = name else {
        let given = h.type_name_for_error(name);
        h.warn(format_args!(
            "compact(): Argument #{pos} must be string or array of strings, {given} given"
        ));
        return;
    };
    let n = n.to_string();
    // `var_defined`, not a non-`Undef` read: a variable holding `null` IS
    // captured, and only a name that was never bound warns.
    if h.var_defined(&n) {
        let val = h.get_var(&n);
        h.arr_set_key(out, &Value::str(n), val);
    } else {
        h.warn(format_args!("compact(): Undefined variable ${n}"));
    }
}

/// `extract($array)` — import each string-keyed entry as a variable in the
/// current scope; returns the number of variables set. Integer keys are skipped
/// (they are not valid variable names), matching PHP's default behavior.
fn php_extract(h: &mut host::PhpHost, args: &[Value]) -> Value {
    let arr = arg(args, 0);
    let mut count = 0i64;
    for (k, v) in h.array_pairs(&arr).unwrap_or_default() {
        let name = match &k {
            Value::Str(s) if is_var_name(s) => s.to_string(),
            _ => continue,
        };
        h.set_var(&name, v);
        count += 1;
    }
    Value::int(count)
}

/// Whether `s` is a valid PHP variable name (`[A-Za-z_][A-Za-z0-9_]*`).
fn is_var_name(s: &str) -> bool {
    let mut cs = s.chars();
    match cs.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    cs.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

// ── internal pointer (end/reset/current/key/next/prev) ───────────────────────
//
// PHP arrays carry an internal cursor. The heap array here has no cursor field
// and `host.rs` is off-limits, so the cursor lives in a side table keyed by the
// array's handle. A per-handle content fingerprint guards the entry: when the
// array behind a handle changes (a fresh program after `reset_host` reuses the
// same low handles), the fingerprint no longer matches and the cursor resets to
// the first element — so no stale cursor leaks across independent runs.
//
// The fingerprint is deliberately **append-invariant**: it hashes only the first
// element (its key and value), NOT the length or the last key. PHP keeps the
// internal pointer where it is when you append (`$a[] = x`), so a fingerprint
// that changed on append would wrongly rewind the cursor to the start after
// every push. Hashing the first element leaves the cursor untouched across
// appends (they never alter element 0) while still distinguishing an unrelated
// array that reuses the same handle after `reset_host`, as long as that array's
// first element differs.
//
// LIMITATION (documented, not fully fixable here): because the fingerprint is
// content-based, two *different* arrays that happen to share an identical first
// element and land on the same reused handle across a `reset_host` boundary
// could inherit a stale cursor. A fully correct fix needs a real per-array
// cursor field stored in the VM heap alongside the array (so identity is by the
// handle's lifetime, not by content) — that lives in `host.rs`, which is off
// limits for this change. The heuristic below is the closest correct-for-PHP
// behavior (append preserves the pointer) achievable without a VM change.
//
// Positions: `0..len` index the elements; `-1` is the single "invalid" state PHP
// uses once the pointer falls off either end (sticky — `prev`/`next` from it stay
// invalid and return false until `reset`/`end` re-anchors it).

thread_local! {
    static CURSORS: RefCell<HashMap<u32, (u64, i64)>> = RefCell::new(HashMap::new());
}

/// An append-invariant identity fingerprint for an array: it hashes only the
/// first element's key and value (plus whether the array is empty). Appending to
/// the array never touches element 0, so the fingerprint — and therefore the
/// stored cursor — survives an append, matching PHP (the internal pointer stays
/// put across `$a[] = x`). It still changes when an unrelated array reuses the
/// same handle after `reset_host`, provided that array's first element differs.
/// See the module-level LIMITATION note for the residual same-first-element case.
fn fingerprint(h: &host::PhpHost, pairs: &[(Value, Value)]) -> u64 {
    let mut hasher = DefaultHasher::new();
    match pairs.first() {
        Some((k, v)) => {
            1u8.hash(&mut hasher);
            h.to_str(k).hash(&mut hasher);
            h.to_str(v).hash(&mut hasher);
        }
        None => 0u8.hash(&mut hasher),
    }
    hasher.finish()
}

/// The stored cursor position for `handle`, or `0` (the fresh default) when there
/// is no entry or the array's fingerprint has changed under it.
fn cursor_pos(handle: u32, fp: u64) -> i64 {
    CURSORS.with(|c| match c.borrow().get(&handle) {
        Some((f, pos)) if *f == fp => *pos,
        _ => 0,
    })
}

fn cursor_set(handle: u32, fp: u64, pos: i64) {
    CURSORS.with(|c| {
        c.borrow_mut().insert(handle, (fp, pos));
    });
}

/// The array's handle, its `(key, value)` pairs, and its identity fingerprint.
type PtrCtx = (u32, Vec<(Value, Value)>, u64);

/// Common preamble for the pointer builtins — or `None` if the argument is not
/// an array.
fn ptr_ctx(h: &host::PhpHost, args: &[Value]) -> Option<PtrCtx> {
    let v = arg(args, 0);
    let handle = as_handle(&v)?;
    if !h.is_array(&v) {
        return None;
    }
    let pairs = h.array_pairs(&v).unwrap_or_default();
    let fp = fingerprint(h, &pairs);
    Some((handle, pairs, fp))
}

fn ptr_reset(h: &mut host::PhpHost, args: &[Value]) -> Value {
    let Some((handle, pairs, fp)) = ptr_ctx(h, args) else {
        return Value::bool(false);
    };
    if pairs.is_empty() {
        cursor_set(handle, fp, -1);
        return Value::bool(false);
    }
    cursor_set(handle, fp, 0);
    pairs[0].1.clone()
}

fn ptr_end(h: &mut host::PhpHost, args: &[Value]) -> Value {
    let Some((handle, pairs, fp)) = ptr_ctx(h, args) else {
        return Value::bool(false);
    };
    if pairs.is_empty() {
        cursor_set(handle, fp, -1);
        return Value::bool(false);
    }
    let last = pairs.len() - 1;
    cursor_set(handle, fp, last as i64);
    pairs[last].1.clone()
}

fn ptr_current(h: &mut host::PhpHost, args: &[Value]) -> Value {
    let Some((handle, pairs, fp)) = ptr_ctx(h, args) else {
        return Value::bool(false);
    };
    let pos = cursor_pos(handle, fp);
    if pos >= 0 && (pos as usize) < pairs.len() {
        pairs[pos as usize].1.clone()
    } else {
        Value::bool(false)
    }
}

fn ptr_key(h: &mut host::PhpHost, args: &[Value]) -> Value {
    let Some((handle, pairs, fp)) = ptr_ctx(h, args) else {
        return Value::Undef;
    };
    let pos = cursor_pos(handle, fp);
    if pos >= 0 && (pos as usize) < pairs.len() {
        pairs[pos as usize].0.clone()
    } else {
        Value::Undef
    }
}

/// `next` (`delta = 1`) / `prev` (`delta = -1`): step the cursor and return the
/// value at the new position, or `false` once it falls off the end (staying in
/// the sticky invalid state).
fn ptr_step(h: &mut host::PhpHost, args: &[Value], delta: i64) -> Value {
    let Some((handle, pairs, fp)) = ptr_ctx(h, args) else {
        return Value::bool(false);
    };
    let pos = cursor_pos(handle, fp);
    // Already invalid: stay invalid (PHP's off-the-end pointer is sticky).
    if pos < 0 {
        cursor_set(handle, fp, -1);
        return Value::bool(false);
    }
    let np = pos + delta;
    if np >= 0 && (np as usize) < pairs.len() {
        cursor_set(handle, fp, np);
        pairs[np as usize].1.clone()
    } else {
        cursor_set(handle, fp, -1);
        Value::bool(false)
    }
}
