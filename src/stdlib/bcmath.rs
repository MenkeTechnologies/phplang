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
//! ## Operand validation
//! Every operand is checked against PHP's bcmath grammar before it is used — see
//! `bc_normalize` — and a spelling outside it raises the reference's
//! `ValueError: <fn>(): Argument #N ($name) is not well-formed`. `bcpow` and
//! `bcpowmod` additionally reject an operand they use as an integer when it
//! carries a fractional part.
//!
//! The checks run in the reference's order, which is observable when more than
//! one argument is bad: all the well-formed checks first (left to right), then
//! the fractional-part checks on `$num` and `$exponent`, then
//! `$exponent >= 0`, then the fractional-part check on `$modulus`, and only
//! then the division/modulo-by-zero faults. `bcpowmod("1", "-1", "2.5")` is
//! therefore an `$exponent` error and `bcpowmod("1", "1", "2.5")` a `$modulus`
//! one. Verified against php 8.5.9.

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

/// PHP's bcmath operand grammar: an optional single sign, then any number of
/// ASCII digits, then optionally a `.` and any number of ASCII digits. Nothing
/// else is accepted — no surrounding whitespace, no exponent (`"1e3"`), no digit
/// separators (`"1_000"`), no hex, and no non-ASCII digits. A string with no
/// digits at all is still well-formed and reads as zero, so `""`, `"."`, `"+"`
/// and `"-."` are all accepted.
///
/// Returns the operand rewritten into a spelling `BigDecimal::from_str` parses:
/// `"5."` and `".5"` satisfy bcmath but not `BigDecimal`, so the missing side of
/// the point is filled with a `0`. `None` means "not well-formed".
///
/// Verified against php 8.5.9 over 33 operand spellings.
fn bc_normalize(s: &str) -> Option<String> {
    let (sign, rest) = match s.as_bytes().first() {
        Some(b'-') => ("-", &s[1..]),
        Some(b'+') => ("", &s[1..]),
        _ => ("", s),
    };
    // Only the FIRST `.` splits, so a second one stays in `frac` and is rejected
    // by the digit check below — which is what makes `"1.2.3"` malformed.
    let (int, frac) = rest.split_once('.').unwrap_or((rest, ""));
    if !int.bytes().all(|b| b.is_ascii_digit()) || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let int = if int.is_empty() { "0" } else { int };
    let frac = if frac.is_empty() { "0" } else { frac };
    Some(format!("{sign}{int}.{frac}"))
}

/// Read the operand at `idx` as an exact decimal, raising the reference's
/// `ValueError` when it is outside the grammar. `pos`/`name` are the argument
/// number and parameter name the message quotes.
fn bc_operand(
    args: &[Value],
    idx: usize,
    func: &str,
    pos: usize,
    name: &str,
) -> Result<BigDecimal, String> {
    let raw = str_arg(args, idx);
    let norm = bc_normalize(&raw).ok_or_else(|| {
        throws(
            "ValueError",
            format!("{func}(): Argument #{pos} (${name}) is not well-formed"),
        )
    })?;
    // `bc_normalize` only returns spellings `BigDecimal` accepts, so the parse
    // cannot fail; the fallback keeps the function total rather than panicking.
    Ok(BigDecimal::from_str(&norm).unwrap_or_else(|_| BigDecimal::zero()))
}

/// Reject an operand that `bcpow`/`bcpowmod` use as an integer but which carries
/// a fractional part, and hand back the integer otherwise. PHP does not truncate
/// here — `bcpow("2", "1.5")` is an error, not `4`.
fn bc_integral(bd: &BigDecimal, func: &str, pos: usize, name: &str) -> Result<BigInt, String> {
    if bd.with_scale_round(0, RoundingMode::Down) != *bd {
        return Err(throws(
            "ValueError",
            format!("{func}(): Argument #{pos} (${name}) cannot have a fractional part"),
        ));
    }
    Ok(bd_to_bigint(bd))
}

/// The scale for a call: the explicit argument at `idx` if present (clamped to
/// be non-negative — PHP rejects a negative scale; leniently clamped here),
/// otherwise the module default from `bcscale`.
fn scale_arg(args: &[Value], idx: usize) -> i64 {
    let s = if args.len() > idx {
        int_arg(args, idx)
    } else {
        current_scale()
    };
    // Clamp to a sane maximum: a pathological scale (billions) would allocate
    // multi-GB result strings and abort the process. 2^20 fractional digits is
    // far beyond any real use.
    s.clamp(0, 1 << 20)
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
///
/// The arms run inside a `Result`-returning closure so operand validation can
/// use `?`: a malformed operand has to abandon the arm at the point it is
/// detected, because the reference reports the FIRST bad argument and computing
/// with the others first would report the wrong one. A name this module does not
/// handle escapes through `unhandled` rather than through the closure.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let mut unhandled = false;
    let out: Result<Value, String> = (|| match name {
        "bcadd" => {
            let a = bc_operand(args, 0, "bcadd", 1, "num1")?;
            let b = bc_operand(args, 1, "bcadd", 2, "num2")?;
            let s = scale_arg(args, 2);
            Ok(Value::str(format_scaled(&(a + b), s)))
        }
        "bcsub" => {
            let a = bc_operand(args, 0, "bcsub", 1, "num1")?;
            let b = bc_operand(args, 1, "bcsub", 2, "num2")?;
            let s = scale_arg(args, 2);
            Ok(Value::str(format_scaled(&(a - b), s)))
        }
        "bcmul" => {
            let a = bc_operand(args, 0, "bcmul", 1, "num1")?;
            let b = bc_operand(args, 1, "bcmul", 2, "num2")?;
            let s = scale_arg(args, 2);
            Ok(Value::str(format_scaled(&(a * b), s)))
        }
        "bcdiv" => {
            let a = bc_operand(args, 0, "bcdiv", 1, "num1")?;
            let b = bc_operand(args, 1, "bcdiv", 2, "num2")?;
            let s = scale_arg(args, 2);
            if b.is_zero() {
                Err(throws("DivisionByZeroError", "Division by zero"))
            } else {
                Ok(Value::str(format_scaled(&(a / b), s)))
            }
        }
        "bcmod" => {
            let a = bc_operand(args, 0, "bcmod", 1, "num1")?;
            let b = bc_operand(args, 1, "bcmod", 2, "num2")?;
            let s = scale_arg(args, 2);
            if b.is_zero() {
                Err(throws("DivisionByZeroError", "Modulo by zero"))
            } else {
                // r = a - b * trunc(a / b) with the quotient truncated to an
                // integer (scale 0), toward zero — PHP 7.2+ bcmod semantics.
                let q = (a.clone() / b.clone()).with_scale_round(0, RoundingMode::Down);
                let r = a - b * q;
                Ok(Value::str(format_scaled(&r, s)))
            }
        }
        "bcpow" => {
            let base = bc_operand(args, 0, "bcpow", 1, "num")?;
            let exp = bc_integral(
                &bc_operand(args, 1, "bcpow", 2, "exponent")?,
                "bcpow",
                2,
                "exponent",
            )?;
            let s = scale_arg(args, 2);
            let exp_i64 = i64::try_from(&exp).unwrap_or(if exp.sign() == num_bigint::Sign::Minus {
                i64::MIN
            } else {
                i64::MAX
            });
            // A negative exponent takes the reciprocal — undefined (division by
            // zero) when the base is 0; error cleanly instead of panicking.
            if exp_i64 < 0 && base.is_zero() {
                Err(throws("DivisionByZeroError", "Negative power of zero"))
            } else {
                Ok(Value::str(format_scaled(&bd_pow(&base, exp_i64), s)))
            }
        }
        "bcsqrt" => {
            let n = bc_operand(args, 0, "bcsqrt", 1, "num")?;
            let s = scale_arg(args, 1);
            match n.sqrt() {
                Some(r) => Ok(Value::str(format_scaled(&r, s))),
                None => Err(throws(
                    "ValueError",
                    "bcsqrt(): Argument #1 ($num) must be greater than or equal to 0",
                )),
            }
        }
        "bccomp" => {
            let a = bc_operand(args, 0, "bccomp", 1, "num1")?;
            let b = bc_operand(args, 1, "bccomp", 2, "num2")?;
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
            // The three well-formed checks come first, then the fractional-part
            // checks on `$num` and `$exponent`, then `$exponent >= 0`, and only
            // then the one on `$modulus` — so a call that is bad in two ways
            // reports whichever the reference reports. See the module header.
            let base_bd = bc_operand(args, 0, "bcpowmod", 1, "num")?;
            let exp_bd = bc_operand(args, 1, "bcpowmod", 2, "exponent")?;
            let mod_bd = bc_operand(args, 2, "bcpowmod", 3, "modulus")?;
            let base = bc_integral(&base_bd, "bcpowmod", 1, "num")?;
            let exp = bc_integral(&exp_bd, "bcpowmod", 2, "exponent")?;
            let s = scale_arg(args, 3);
            if exp.sign() == num_bigint::Sign::Minus {
                return Err(throws(
                    "ValueError",
                    "bcpowmod(): Argument #2 ($exponent) must be greater than or equal to 0",
                ));
            }
            let modulus = bc_integral(&mod_bd, "bcpowmod", 3, "modulus")?;
            if modulus.is_zero() {
                Err(throws("DivisionByZeroError", "Modulo by zero"))
            } else {
                let r = base.modpow(&exp, &modulus);
                Ok(Value::str(format_scaled(&BigDecimal::from(r), s)))
            }
        }
        _ => {
            unhandled = true;
            Ok(Value::Undef)
        }
    })();
    if unhandled {
        None
    } else {
        Some(out)
    }
}
