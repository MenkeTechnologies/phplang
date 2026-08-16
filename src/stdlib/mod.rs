//! The extended PHP standard library, one module per category. Each module
//! exposes `dispatch(name, args) -> Option<Result<Value, String>>`, returning
//! `None` when the name is not one of its functions. `builtins::call_library`
//! consults this chain for every name its own core match does not handle, so
//! categories can be developed independently without touching a shared table.
//!
//! Functions reach the runtime through `crate::host::with_host` and produce a
//! `fusevm::Value`; behavior mirrors PHP 8 (verified against the reference `php`
//! where possible). Vetted crates supply the primitives that must not be
//! hand-rolled: `regex` (preg), `chrono` (date), `md-5`/`sha1`/`sha2`/`crc32fast`
//! (hashing), `base64`/`hex`/`urlencoding` (encoding).

use fusevm::Value;

pub mod arrays;
pub mod bcmath;
pub mod callable;
pub mod constants;
pub mod ctype;
pub mod datefn;
pub mod datetime;
pub mod encoding;
pub mod fileio;
pub mod fileres;
pub mod filter;
pub mod gmp;
pub mod hash;
pub mod hashext;
pub mod json;
pub mod math;
pub mod mbstring;
pub mod misc;
pub mod preg;
pub mod reflection;
pub mod runtime;
pub mod strings;
pub mod system;
pub mod textx;
pub mod types;
pub mod url;

/// Try each category in turn; the first that recognizes `name` wins.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    strings::dispatch(name, args)
        .or_else(|| arrays::dispatch(name, args))
        .or_else(|| math::dispatch(name, args))
        .or_else(|| ctype::dispatch(name, args))
        .or_else(|| types::dispatch(name, args))
        .or_else(|| preg::dispatch(name, args))
        .or_else(|| datetime::dispatch(name, args))
        .or_else(|| datefn::dispatch(name, args))
        .or_else(|| hash::dispatch(name, args))
        .or_else(|| encoding::dispatch(name, args))
        .or_else(|| url::dispatch(name, args))
        .or_else(|| json::dispatch(name, args))
        .or_else(|| fileio::dispatch(name, args))
        .or_else(|| reflection::dispatch(name, args))
        .or_else(|| callable::dispatch(name, args))
        .or_else(|| filter::dispatch(name, args))
        .or_else(|| mbstring::dispatch(name, args))
        .or_else(|| misc::dispatch(name, args))
        .or_else(|| constants::dispatch(name, args))
        .or_else(|| system::dispatch(name, args))
        .or_else(|| fileres::dispatch(name, args))
        .or_else(|| gmp::dispatch(name, args))
        .or_else(|| bcmath::dispatch(name, args))
        .or_else(|| hashext::dispatch(name, args))
        .or_else(|| textx::dispatch(name, args))
        .or_else(|| runtime::dispatch(name, args))
}

/// Shared helpers for the category modules. Each helper is used by at least one
/// category once filled in; `allow(dead_code)` keeps the scaffold warning-free
/// before those modules land.
#[allow(dead_code)]
pub(crate) mod common {
    use crate::host::with_host;
    use fusevm::Value;

    /// Report an argument error as the PHP exception it is — see
    /// [`crate::builtins::throws`]. Re-exported here because every stdlib module
    /// already glob-imports this prelude, and an argument error is a normal thing
    /// for a library function to raise.
    pub use crate::builtins::throws;

    /// Stable merge sort that never validates its comparator.
    ///
    /// `slice::sort_by` panics with "user-provided comparison function does not
    /// correctly implement a total order" when it detects an inconsistent
    /// ordering. A PHP comparison callback is arbitrary user code and is very
    /// often inconsistent — `usort($a, fn() => random_int(-1, 1))` is a classic
    /// — and the reference answers with SOME permutation rather than failing.
    /// A panic there is uncatchable, so the user-callback sorts route through
    /// this instead.
    ///
    /// Merge sort is also the right shape for the contract: PHP 8.0 made every
    /// sort stable, so equal elements must keep their original order.
    pub fn stable_sort_by<T: Clone>(
        v: &mut Vec<T>,
        mut cmp: impl FnMut(&T, &T) -> std::cmp::Ordering,
    ) {
        let n = v.len();
        if n < 2 {
            return;
        }
        let mut src = std::mem::take(v);
        let mut dst: Vec<T> = Vec::with_capacity(n);
        let mut width = 1;
        while width < n {
            let mut i = 0;
            while i < n {
                let mid = (i + width).min(n);
                let end = (i + 2 * width).min(n);
                let (mut l, mut r) = (i, mid);
                while l < mid || r < end {
                    let take_left = if l >= mid {
                        false
                    } else if r >= end {
                        true
                    } else {
                        // `!= Greater` keeps a tie in its original order, which
                        // is what makes the sort stable.
                        cmp(&src[l], &src[r]) != std::cmp::Ordering::Greater
                    };
                    if take_left {
                        dst.push(src[l].clone());
                        l += 1;
                    } else {
                        dst.push(src[r].clone());
                        r += 1;
                    }
                }
                i = end;
            }
            std::mem::swap(&mut src, &mut dst);
            dst.clear();
            width *= 2;
        }
        *v = src;
    }

    /// The `i`-th argument, or `Undef` (PHP: a missing argument reads as null).
    pub fn arg(args: &[Value], i: usize) -> Value {
        args.get(i).cloned().unwrap_or(Value::Undef)
    }

    /// PHP string cast of the `i`-th argument.
    pub fn str_arg(args: &[Value], i: usize) -> String {
        with_host(|h| h.to_str(&arg(args, i)))
    }

    /// PHP integer cast of the `i`-th argument.
    pub fn int_arg(args: &[Value], i: usize) -> i64 {
        with_host(|h| h.to_number(&arg(args, i)).to_int())
    }

    /// PHP float cast of the `i`-th argument.
    pub fn float_arg(args: &[Value], i: usize) -> f64 {
        with_host(|h| h.to_number(&arg(args, i)).to_float())
    }

    /// Port of `php_charmask` (php-src 8.5 `ext/standard/string.c:475`): build a
    /// 256-entry membership table from a character list in which `a..z` denotes
    /// an inclusive, *incrementing* byte range.
    ///
    /// The four diagnostics are part of the contract, not decoration — every
    /// caller of `php_charmask` inherits them, so `trim`, `str_word_count` and
    /// `addcslashes` all report a malformed range under their own name. That is
    /// why `fname` is a parameter: upstream `php_error_docref` prefixes the
    /// message with whichever function is on the stack.
    ///
    /// A malformed range contributes nothing to the mask; upstream `continue`s
    /// past it without setting a bit, which is why `addcslashes("a..b", "..z")`
    /// escapes the literal dots rather than a range.
    ///
    /// The host is threaded through rather than reached for with `with_host`
    /// because most callers already hold the borrow, and a second `borrow_mut`
    /// on the thread-local `RefCell` panics.
    pub fn charmask(h: &mut crate::host::PhpHost, list: &[u8], fname: &str) -> [bool; 256] {
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
            } else if i + 1 < n && list[i] == b'.' && list[i + 1] == b'.' {
                let why = if i == 0 {
                    "no character to the left of '..'"
                } else if i + 2 >= n {
                    "no character to the right of '..'"
                } else if list[i - 1] > list[i + 2] {
                    "'..'-range needs to be incrementing"
                } else {
                    // Upstream's own FIXME: `a..b..c` is the only shape left.
                    ""
                };
                let msg = if why.is_empty() {
                    format!("{fname}(): Invalid '..'-range")
                } else {
                    format!("{fname}(): Invalid '..'-range, {why}")
                };
                h.warn(msg);
                i += 1;
            } else {
                mask[c as usize] = true;
                i += 1;
            }
        }
        mask
    }

    /// Build a PHP array (list) from an ordered value list.
    pub fn make_list(vals: Vec<Value>) -> Value {
        with_host(|h| {
            let arr = h.new_array();
            for v in vals {
                h.arr_push_auto(&arr, v);
            }
            arr
        })
    }

    /// Build a PHP array from `(key, value)` pairs, preserving keys.
    pub fn make_map(pairs: Vec<(Value, Value)>) -> Value {
        with_host(|h| {
            let arr = h.new_array();
            for (k, v) in pairs {
                h.arr_set_key(&arr, &k, v);
            }
            arr
        })
    }
}
