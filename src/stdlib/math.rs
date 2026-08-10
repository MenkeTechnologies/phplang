//! PHP standard-library `math` functions. Part of the `stdlib` chain; see
//! `src/stdlib/mod.rs`. `dispatch` returns `None` for names it does not handle.
//!
//! The core (`builtins.rs`) already provides `abs floor ceil sqrt round exp log
//! log10 pow sin cos tan pi fmod intdiv hexdec dechex min max`; those names never
//! reach here. This module fills in the remaining PHP 8 math surface: the extra
//! trig/hyperbolic/inverse family, radian/degree helpers, IEEE predicates and
//! division, the base-conversion functions (`decbin`/`bindec`/…/`base_convert`)
//! and the pseudo-random generators (`rand`/`mt_rand`/`random_int` + seeding).

use crate::stdlib::common::*;
use fusevm::Value;
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

/// PHP `RAND_MAX` / `MT_RAND_MAX` on every supported platform.
const RAND_MAX: i64 = 2_147_483_647;

// ── seeded PRNG (rand / mt_rand) ─────────────────────────────────────────────
// A per-thread SplitMix64. PHP's `rand`/`mt_rand` share a mutable generator that
// `srand`/`mt_srand` reseed; we mirror that with thread-local state. Callers only
// assert range/bounds, so the exact bit sequence need not match the C library.
thread_local! {
    static RNG_STATE: Cell<u64> = const { Cell::new(0) };
    static RNG_SEEDED: Cell<bool> = const { Cell::new(false) };
}

/// SplitMix64 mixing step over `z`.
fn splitmix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Non-deterministic 64 bits from the clock and a monotonically-incrementing
/// counter — the entropy source for auto-seeding and for `random_int`.
fn os_entropy() -> u64 {
    static CTR: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let c = CTR.fetch_add(1, Ordering::Relaxed);
    splitmix(t ^ c.wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

/// Reseed the shared generator (`srand`/`mt_srand`).
fn seed(v: u64) {
    RNG_STATE.with(|st| st.set(v));
    RNG_SEEDED.with(|s| s.set(true));
}

/// Next 64 bits from the shared generator, auto-seeding on first use.
fn next_u64() -> u64 {
    RNG_SEEDED.with(|s| {
        if !s.get() {
            RNG_STATE.with(|st| st.set(os_entropy() | 1));
            s.set(true);
        }
    });
    RNG_STATE.with(|st| {
        let z = st.get().wrapping_add(0x9E37_79B9_7F4A_7C15);
        st.set(z);
        splitmix(z)
    })
}

/// Map a random `u64` into the inclusive range `[min, max]`. The caller must
/// guarantee `min <= max`: `rand()` swaps inverted bounds first, while
/// `mt_rand()`/`random_int()` reject them (each with its own PHP 8 message).
fn to_range(bits: u64, min: i64, max: i64) -> Value {
    let span = (max as i128 - min as i128) as u128 + 1;
    let off = (bits as u128 % span) as i128;
    Value::int((min as i128 + off) as i64)
}

/// Parse the two-argument `(min, max)` bounds, defaulting to `[0, RAND_MAX]` when
/// called with no arguments (PHP's zero-arg `rand()`/`mt_rand()` form).
fn rand_bounds(args: &[Value]) -> (i64, i64) {
    if args.len() >= 2 {
        (int_arg(args, 0), int_arg(args, 1))
    } else {
        (0, RAND_MAX)
    }
}

// ── base conversion ──────────────────────────────────────────────────────────

/// Numeric value of a base-N digit character, or `None` when out of range.
fn digit_val(c: char) -> Option<u32> {
    c.to_digit(36)
}

/// Parse `s` as a base-`from` unsigned integer, silently dropping characters that
/// are not valid digits for the base — PHP's `bindec`/`octdec`/`base_convert`
/// behavior. Accumulates in `u128` to tolerate values past `u64`.
fn parse_base(s: &str, from: u32) -> u128 {
    let mut acc: u128 = 0;
    for c in s.chars() {
        if let Some(d) = digit_val(c) {
            if d < from {
                acc = acc.saturating_mul(from as u128).saturating_add(d as u128);
            }
        }
    }
    acc
}

/// Render `n` in base `to` (2..=36) using lowercase digits.
fn to_base(mut n: u128, to: u32) -> String {
    if n == 0 {
        return "0".into();
    }
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    let to = to as u128;
    while n > 0 {
        out.push(DIGITS[(n % to) as usize]);
        n /= to;
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

/// PHP returns an `int` when a decoded value fits `PHP_INT_MAX`, otherwise a
/// `float` — used by `bindec`/`octdec`.
fn int_or_float(n: u128) -> Value {
    if n <= i64::MAX as u128 {
        Value::int(n as i64)
    } else {
        Value::float(n as f64)
    }
}

/// Dispatch a `math`-category PHP function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let f = |x: f64| Some(Ok(Value::float(x)));
    let a0 = || float_arg(args, 0);
    Some(match name {
        // ── logs / exp ──────────────────────────────────────────────────────
        // NOTE: PHP has no `log2()` (use `log($x, 2)`); `function_exists("log2")`
        // is false, so we deliberately do NOT dispatch it — it must fall through
        // to "call to undefined function".
        "expm1" => return f(a0().exp_m1()),
        "log1p" => return f(a0().ln_1p()),

        // ── hyperbolic + inverses ───────────────────────────────────────────
        "sinh" => return f(a0().sinh()),
        "cosh" => return f(a0().cosh()),
        "tanh" => return f(a0().tanh()),
        "asinh" => return f(a0().asinh()),
        "acosh" => return f(a0().acosh()),
        "atanh" => return f(a0().atanh()),
        "asin" => return f(a0().asin()),
        "acos" => return f(a0().acos()),
        "atan" => return f(a0().atan()),
        "atan2" => return f(float_arg(args, 0).atan2(float_arg(args, 1))),

        // ── angle conversion ────────────────────────────────────────────────
        "deg2rad" => return f(a0() * std::f64::consts::PI / 180.0),
        "rad2deg" => return f(a0() * 180.0 / std::f64::consts::PI),

        // ── geometry / IEEE division ────────────────────────────────────────
        "hypot" => return f(float_arg(args, 0).hypot(float_arg(args, 1))),
        "fdiv" => return f(float_arg(args, 0) / float_arg(args, 1)),

        // ── IEEE predicates ─────────────────────────────────────────────────
        "is_nan" => Ok(Value::bool(a0().is_nan())),
        "is_finite" => Ok(Value::bool(a0().is_finite())),
        "is_infinite" => Ok(Value::bool(a0().is_infinite())),

        // ── base conversion ─────────────────────────────────────────────────
        "decbin" => Ok(Value::str(format!("{:b}", int_arg(args, 0)))),
        "decoct" => Ok(Value::str(format!("{:o}", int_arg(args, 0)))),
        "bindec" => Ok(int_or_float(parse_base(&str_arg(args, 0), 2))),
        "octdec" => Ok(int_or_float(parse_base(&str_arg(args, 0), 8))),
        "base_convert" => {
            let num = str_arg(args, 0);
            let from = int_arg(args, 1);
            let to = int_arg(args, 2);
            if !(2..=36).contains(&from) {
                return Some(Err(throws(
                    "ValueError",
                    "base_convert(): Argument #2 ($from_base) must be between 2 and 36 (inclusive)",
                )));
            }
            if !(2..=36).contains(&to) {
                return Some(Err(throws(
                    "ValueError",
                    "base_convert(): Argument #3 ($to_base) must be between 2 and 36 (inclusive)",
                )));
            }
            let v = parse_base(&num, from as u32);
            Ok(Value::str(to_base(v, to as u32)))
        }

        // ── pseudo-random ───────────────────────────────────────────────────
        "mt_getrandmax" | "getrandmax" => Ok(Value::int(RAND_MAX)),
        "srand" | "mt_srand" => {
            if args.is_empty() {
                seed(os_entropy() | 1);
            } else {
                seed(int_arg(args, 0) as u64);
            }
            Ok(Value::Undef)
        }
        "rand" => {
            // PHP's `rand()` SWAPS inverted bounds instead of erroring: with
            // min > max it returns a value in [max, min].
            let (mut min, mut max) = rand_bounds(args);
            if min > max {
                std::mem::swap(&mut min, &mut max);
            }
            return Some(Ok(to_range(next_u64(), min, max)));
        }
        "mt_rand" => {
            let (min, max) = rand_bounds(args);
            if min > max {
                return Some(Err(throws(
                        "ValueError",
                        "mt_rand(): Argument #2 ($max) must be greater than or equal to argument #1 ($min)",
                    )));
            }
            return Some(Ok(to_range(next_u64(), min, max)));
        }
        "random_int" => {
            let min = int_arg(args, 0);
            let max = int_arg(args, 1);
            if min > max {
                return Some(Err(throws(
                        "ValueError",
                        "random_int(): Argument #1 ($min) must be less than or equal to argument #2 ($max)",
                    )));
            }
            return Some(Ok(to_range(os_entropy(), min, max)));
        }

        _ => return None,
    })
}
