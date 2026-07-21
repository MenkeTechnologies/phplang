//! PHP `gmp_*` arbitrary-precision integer functions, backed by `num-bigint`.
//! Part of the `stdlib` chain; see `src/stdlib/mod.rs`.
//!
//! PHP's GMP functions take and return `GMP` objects; phplang has no GMP object
//! type, so operands are accepted as integer strings/ints and results are
//! returned as decimal STRINGS. Because a returned string is itself a valid
//! operand, the usual `gmp_add(gmp_mul($a, $b), $c)` chaining still works, and
//! `gmp_strval`/`gmp_intval` convert out. Non-numeric operands parse as 0.

use crate::stdlib::common::*;
use fusevm::Value;
use num_bigint::BigInt;
use num_bigint::Sign;

/// Absolute value (BigInt has no inherent `abs` without `num-traits::Signed`).
fn absv(b: BigInt) -> BigInt {
    if b.sign() == Sign::Minus {
        -b
    } else {
        b
    }
}

/// Parse an operand to a `BigInt` (integer string or int; leading-numeric prefix,
/// else 0), matching GMP's lenient string handling.
fn big(args: &[Value], i: usize) -> BigInt {
    let s = str_arg(args, i);
    let t = s.trim();
    // A base-prefixed literal (0x/0b/0o) or plain decimal.
    let (radix, digits) = if let Some(r) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        (16, r)
    } else if let Some(r) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
        (2, r)
    } else {
        (10, t)
    };
    BigInt::parse_bytes(digits.as_bytes(), radix).unwrap_or_else(|| BigInt::from(0))
}

fn out(b: BigInt) -> Value {
    Value::str(b.to_string())
}

/// Euclidean GCD (always non-negative), since `num-bigint` alone has no `gcd`.
fn gcd(mut a: BigInt, mut b: BigInt) -> BigInt {
    a = absv(a);
    b = absv(b);
    while b.sign() != Sign::NoSign {
        let r = &a % &b;
        a = b;
        b = r;
    }
    a
}

/// `gmp_mod`/`gmp_div_r` follow GMP: the result has the sign of the divisor, so
/// for a positive modulus it lands in `[0, |m|)`.
fn gmp_modulo(a: &BigInt, m: &BigInt) -> BigInt {
    if m.sign() == Sign::NoSign {
        return BigInt::from(0);
    }
    let r = a % m;
    if r.sign() == Sign::Minus {
        r + absv(m.clone())
    } else {
        r
    }
}

/// Dispatch a `gmp_*` function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let v = match name {
        "gmp_init" => out(big(args, 0)),
        "gmp_add" => out(big(args, 0) + big(args, 1)),
        "gmp_sub" => out(big(args, 0) - big(args, 1)),
        "gmp_mul" => out(big(args, 0) * big(args, 1)),
        "gmp_div_q" | "gmp_div" => {
            let d = big(args, 1);
            if d.sign() == Sign::NoSign {
                return Some(Err("gmp_div_q(): Division by zero".to_string()));
            }
            out(big(args, 0) / d)
        }
        "gmp_div_r" | "gmp_mod" => {
            let m = big(args, 1);
            if m.sign() == Sign::NoSign {
                return Some(Err("gmp_mod(): Division by zero".to_string()));
            }
            out(gmp_modulo(&big(args, 0), &m))
        }
        "gmp_pow" => {
            let e = int_arg(args, 1);
            if e < 0 {
                out(BigInt::from(0))
            } else {
                out(big(args, 0).pow(e as u32))
            }
        }
        "gmp_powm" => {
            let m = big(args, 2);
            if m.sign() == Sign::NoSign {
                return Some(Err("gmp_powm(): Modulus may not be zero".to_string()));
            }
            let e = big(args, 1);
            if e.sign() == Sign::Minus {
                return Some(Err("gmp_powm(): Negative exponent not supported".to_string()));
            }
            out(big(args, 0).modpow(&e, &m))
        }
        "gmp_gcd" => out(gcd(big(args, 0), big(args, 1))),
        "gmp_lcm" => {
            let a = big(args, 0);
            let b = big(args, 1);
            if a.sign() == Sign::NoSign || b.sign() == Sign::NoSign {
                out(BigInt::from(0))
            } else {
                out(absv(&a / gcd(a.clone(), b.clone()) * &b))
            }
        }
        "gmp_abs" => out(absv(big(args, 0))),
        "gmp_neg" => out(-big(args, 0)),
        "gmp_and" => out(big(args, 0) & big(args, 1)),
        "gmp_or" => out(big(args, 0) | big(args, 1)),
        "gmp_xor" => out(big(args, 0) ^ big(args, 1)),
        "gmp_cmp" => {
            let c = big(args, 0).cmp(&big(args, 1));
            Value::int(match c {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            })
        }
        "gmp_sign" => Value::int(match big(args, 0).sign() {
            Sign::Minus => -1,
            Sign::NoSign => 0,
            Sign::Plus => 1,
        }),
        "gmp_sqrt" => out(big(args, 0).sqrt()),
        "gmp_root" => {
            // Integer nth root (n>=1) via binary search; default/others -> sqrt.
            let n = int_arg(args, 1).max(1);
            if n == 2 {
                out(big(args, 0).sqrt())
            } else {
                out(int_root(&big(args, 0), n as u32))
            }
        }
        "gmp_fact" => {
            let n = int_arg(args, 0).max(0);
            let mut acc = BigInt::from(1);
            let mut i = BigInt::from(2);
            let target = BigInt::from(n);
            while i <= target {
                acc *= &i;
                i += 1;
            }
            out(acc)
        }
        "gmp_pow2" => out(BigInt::from(2).pow(int_arg(args, 0).max(0) as u32)),
        "gmp_strval" => {
            let base = if args.len() > 1 { int_arg(args, 1) } else { 10 };
            let b = big(args, 0);
            match base {
                16 => Value::str(b.to_str_radix(16)),
                2 => Value::str(b.to_str_radix(2)),
                8 => Value::str(b.to_str_radix(8)),
                _ => Value::str(b.to_string()),
            }
        }
        "gmp_intval" => Value::int({
            let b = big(args, 0);
            i64::try_from(&b).unwrap_or(if b.sign() == Sign::Minus {
                i64::MIN
            } else {
                i64::MAX
            })
        }),
        "gmp_prob_prime" => Value::int(prob_prime(&big(args, 0))),
        "gmp_perfect_square" => {
            let b = big(args, 0);
            let r = b.sqrt();
            Value::bool(b.sign() != Sign::Minus && &r * &r == b)
        }
        _ => return None,
    };
    Some(Ok(v))
}

/// Integer nth root (floor) by binary search.
fn int_root(x: &BigInt, n: u32) -> BigInt {
    if x.sign() == Sign::NoSign || x == &BigInt::from(1) {
        return x.clone();
    }
    let (mut lo, mut hi) = (BigInt::from(0), x.clone());
    while lo < &hi - 1 {
        let mid = (&lo + &hi) / BigInt::from(2);
        if mid.pow(n) <= *x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

/// A small primality test (`gmp_prob_prime`): returns 2 (definitely prime),
/// 1 (probably prime), or 0 (composite). Uses deterministic trial division for
/// small n and Miller–Rabin with fixed bases above.
fn prob_prime(n: &BigInt) -> i64 {
    let two = BigInt::from(2);
    if n < &two {
        return 0;
    }
    if n == &two || n == &BigInt::from(3) {
        return 2;
    }
    if (n % &two).sign() == Sign::NoSign {
        return 0;
    }
    // Trial division by small odd numbers.
    let mut d = BigInt::from(3);
    let limit = n.sqrt();
    while d <= limit && d <= BigInt::from(100_000) {
        if (n % &d).sign() == Sign::NoSign {
            return 0;
        }
        d += 2;
    }
    if limit <= BigInt::from(100_000) {
        return 2; // fully trial-divided
    }
    // Miller–Rabin with a few fixed bases (probabilistic for large n).
    let n1 = n - BigInt::from(1);
    let mut d = n1.clone();
    let mut r = 0u32;
    while (&d % &two).sign() == Sign::NoSign {
        d /= 2;
        r += 1;
    }
    for a in [2i64, 3, 5, 7, 11, 13, 17] {
        let a = BigInt::from(a);
        if &a >= n {
            continue;
        }
        let mut x = a.modpow(&d, n);
        if x == BigInt::from(1) || x == n1 {
            continue;
        }
        let mut composite = true;
        for _ in 0..r.saturating_sub(1) {
            x = x.modpow(&two, n);
            if x == n1 {
                composite = false;
                break;
            }
        }
        if composite {
            return 0;
        }
    }
    1
}
