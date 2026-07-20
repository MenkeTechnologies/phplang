//! PHP standard-library `bcmath` functions (arbitrary-precision decimal math).
//! Part of the `stdlib` chain; see `src/stdlib/mod.rs`. `dispatch` returns `None`
//! for names it does not handle.
//!
//! Every operand and every result is a decimal **string**, exactly as PHP's
//! bcmath extension models numbers. The optional trailing `$scale` argument is
//! the number of fractional digits to keep, and bcmath **truncates** (rounds
//! toward zero — it never rounds half-up) to that many digits, then formats the
//! result with *exactly* `$scale` fractional digits:
//!
//! ```text
//! bcadd("1", "2", 2)   == "3.00"
//! bcmul("2.5", "2.5")  == "6"        // default scale 0
//! bcdiv("10", "3", 4)  == "3.3333"
//! bcsub("0", "1.5", 2) == "-1.50"
//! ```
//!
//! The default scale is `0` unless [`bcscale`](self) raised it. That default is
//! process-thread state in PHP; here it is a `thread_local!` cell, so each host
//! thread carries its own bcmath scale — matching PHP's per-request model.
//!
//! Arbitrary precision comes from the `bigdecimal` crate (exact decimal
//! arithmetic) with `num-bigint` for the integer modular exponentiation behind
//! `bcpowmod`. Division and square root are computed at `bigdecimal`'s default
//! 100-significant-digit context and then truncated to the requested scale.
//!
//! ## Divergence from PHP 8 (documented, intentional)
//! PHP 8 raises a `ValueError` for a malformed numeric string (e.g. `"abc"`) and
//! for a fractional `bcpow`/`bcpowmod` exponent. Faithful bytecode-level error
//! objects are not wired through this dispatch layer, so instead this module is
//! **lenient**: an unparseable operand is read as `0`, and a fractional exponent
//! is truncated to its integer part. Genuine runtime faults that PHP models as
//! thrown errors — division/modulo by zero, square root of a negative number,
//! and a negative `bcpowmod` exponent — are surfaced as `Err` so the caller
//! aborts rather than producing a bogus value.

use crate::stdlib::common::*;
use bigdecimal::{BigDecimal, RoundingMode, Zero};
use fusevm::Value;
use num_bigint::BigInt;
use std::cell::Cell;
use std::str::FromStr;

thread_local! {
    /// Default number of fractional digits for bcmath operations whose call site
    /// omits an explicit `$scale`. Mirrors PHP's per-thread bcmath scale set by
    /// `bcscale()`; starts at `0`.
    static BC_SCALE: Cell<i64> = const { Cell::new(0) };
}

/// The module-default scale (never negative).
fn current_scale() -> i64 {
    BC_SCALE.with(|c| c.get())
}

/// Parse a bcmath operand string into an exact decimal. Leniently reads an
/// unparseable / empty string as `0` (PHP 8 would raise `ValueError`; see the
/// module divergence note). A leading `+` is accepted.
fn parse_bd(s: &str) -> BigDecimal {
    let t = s.trim();
    if let Ok(b) = BigDecimal::from_str(t) {
        return b;
    }
    let stripped = t.strip_prefix('+').unwrap_or(t);
    BigDecimal::from_str(stripped).unwrap_or_else(|_| BigDecimal::zero())
}

/// The scale for a call: the explicit argument at `idx` if present (clamped to
/// be non-negative — PHP rejects a negative scale; leniently clamped here),
/// otherwise the module default from `bcscale`.
fn scale_arg(args: &[Value], idx: usize) -> i64 {
    if args.len() > idx {
        int_arg(args, idx).max(0)
    } else {
        current_scale()
    }
}

/// Format an exact decimal as a bcmath result string: truncate toward zero to
/// `scale` fractional digits, then render with *exactly* `scale` fractional
/// digits (no `-0` for a truncated-to-zero negative).
fn format_scaled(bd: &BigDecimal, scale: i64) -> String {
    let scale = scale.max(0);
    let truncated = bd.with_scale_round(scale, RoundingMode::Down);
    let raw = truncated.to_plain_string();
    let neg = raw.starts_with('-');
    let mut body = raw.trim_start_matches('-').to_string();

    if scale > 0 {
        match body.find('.') {
            Some(dot) => {
                let frac = body.len() - dot - 1;
                if frac < scale as usize {
                    body.push_str(&"0".repeat(scale as usize - frac));
                }
            }
            None => {
                body.push('.');
                body.push_str(&"0".repeat(scale as usize));
            }
        }
    } else if let Some(dot) = body.find('.') {
        body.truncate(dot);
    }

    let is_zero = body.bytes().all(|b| b == b'0' || b == b'.');
    if neg && !is_zero {
        format!("-{body}")
    } else {
        body
    }
}

/// Truncate an exact decimal to an integer (toward zero) as a `BigInt`.
fn bd_to_bigint(bd: &BigDecimal) -> BigInt {
    let int_str = bd.with_scale_round(0, RoundingMode::Down).to_plain_string();
    BigInt::from_str(&int_str).unwrap_or_else(|_| BigInt::zero())
}

/// Raise `base` to an integer power via exponentiation by squaring. A negative
/// exponent yields the reciprocal (computed at the default division context).
fn bd_pow(base: &BigDecimal, exp: i64) -> BigDecimal {
    if exp == 0 {
        return BigDecimal::from(1);
    }
    let mut result = BigDecimal::from(1);
    let mut b = base.clone();
    let mut e = exp.unsigned_abs();
    while e > 0 {
        if e & 1 == 1 {
            result *= b.clone();
        }
        e >>= 1;
        if e > 0 {
            b = b.clone() * b.clone();
        }
    }
    if exp < 0 {
        BigDecimal::from(1) / result
    } else {
        result
    }
}

/// Dispatch a `bcmath`-category PHP function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let out: Result<Value, String> = match name {
        "bcadd" => {
            let a = parse_bd(&str_arg(args, 0));
            let b = parse_bd(&str_arg(args, 1));
            let s = scale_arg(args, 2);
            Ok(Value::str(format_scaled(&(a + b), s)))
        }
        "bcsub" => {
            let a = parse_bd(&str_arg(args, 0));
            let b = parse_bd(&str_arg(args, 1));
            let s = scale_arg(args, 2);
            Ok(Value::str(format_scaled(&(a - b), s)))
        }
        "bcmul" => {
            let a = parse_bd(&str_arg(args, 0));
            let b = parse_bd(&str_arg(args, 1));
            let s = scale_arg(args, 2);
            Ok(Value::str(format_scaled(&(a * b), s)))
        }
        "bcdiv" => {
            let a = parse_bd(&str_arg(args, 0));
            let b = parse_bd(&str_arg(args, 1));
            let s = scale_arg(args, 2);
            if b.is_zero() {
                Err("bcdiv(): Division by zero".to_string())
            } else {
                Ok(Value::str(format_scaled(&(a / b), s)))
            }
        }
        "bcmod" => {
            let a = parse_bd(&str_arg(args, 0));
            let b = parse_bd(&str_arg(args, 1));
            let s = scale_arg(args, 2);
            if b.is_zero() {
                Err("bcmod(): Modulo by zero".to_string())
            } else {
                // r = a - b * trunc(a / b) with the quotient truncated to an
                // integer (scale 0), toward zero — PHP 7.2+ bcmod semantics.
                let q = (a.clone() / b.clone()).with_scale_round(0, RoundingMode::Down);
                let r = a - b * q;
                Ok(Value::str(format_scaled(&r, s)))
            }
        }
        "bcpow" => {
            let base = parse_bd(&str_arg(args, 0));
            let exp = bd_to_bigint(&parse_bd(&str_arg(args, 1)));
            let s = scale_arg(args, 2);
            let exp_i64 = i64::try_from(&exp).unwrap_or(if exp.sign() == num_bigint::Sign::Minus {
                i64::MIN
            } else {
                i64::MAX
            });
            Ok(Value::str(format_scaled(&bd_pow(&base, exp_i64), s)))
        }
        "bcsqrt" => {
            let n = parse_bd(&str_arg(args, 0));
            let s = scale_arg(args, 1);
            match n.sqrt() {
                Some(r) => Ok(Value::str(format_scaled(&r, s))),
                None => Err("bcsqrt(): Argument must be greater than or equal to 0".to_string()),
            }
        }
        "bccomp" => {
            let a = parse_bd(&str_arg(args, 0));
            let b = parse_bd(&str_arg(args, 1));
            let s = scale_arg(args, 2);
            // Compare after truncating both operands to the requested scale.
            let at = a.with_scale_round(s, RoundingMode::Down);
            let bt = b.with_scale_round(s, RoundingMode::Down);
            let c = match at.cmp(&bt) {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            };
            Ok(Value::int(c))
        }
        "bcscale" => {
            if args.is_empty() {
                Ok(Value::int(current_scale()))
            } else {
                let prev = current_scale();
                let next = int_arg(args, 0).max(0);
                BC_SCALE.with(|c| c.set(next));
                // PHP's bcscale(int $scale) returns the previous scale.
                Ok(Value::int(prev))
            }
        }
        "bcpowmod" => {
            let base = bd_to_bigint(&parse_bd(&str_arg(args, 0)));
            let exp = bd_to_bigint(&parse_bd(&str_arg(args, 1)));
            let modulus = bd_to_bigint(&parse_bd(&str_arg(args, 2)));
            let s = scale_arg(args, 3);
            if modulus.is_zero() {
                Err("bcpowmod(): Modulo by zero".to_string())
            } else if exp.sign() == num_bigint::Sign::Minus {
                Err("bcpowmod(): Exponent must be greater than or equal to 0".to_string())
            } else {
                let r = base.modpow(&exp, &modulus);
                Ok(Value::str(format_scaled(&BigDecimal::from(r), s)))
            }
        }
        _ => return None,
    };
    Some(out)
}
