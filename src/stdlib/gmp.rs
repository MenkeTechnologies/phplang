//! PHP `gmp_*` arbitrary-precision integer functions, backed by `num-bigint`.
//! Part of the `stdlib` chain; see `src/stdlib/mod.rs`.
//!
//! PHP's GMP functions take and return `GMP` objects; phplang has no GMP object
//! type, so operands are accepted as integers or integer strings and results are
//! returned as decimal STRINGS. Because a returned string is itself a valid
//! operand, the usual `gmp_add(gmp_mul($a, $b), $c)` chaining still works, and
//! `gmp_strval`/`gmp_intval` convert out. A string that is not an integer is the
//! reference's `ValueError`, named for the failing parameter.

use crate::host::with_host;
use crate::stdlib::common::*;
use fusevm::Value;
use num_bigint::BigInt;
use num_bigint::Sign;

/// `GMP_ROUND_*`: the rounding mode of `gmp_div_q`, `gmp_div_r` and `gmp_div_qr`.
pub(crate) const GMP_ROUND_ZERO: i64 = 0;
pub(crate) const GMP_ROUND_PLUSINF: i64 = 1;
pub(crate) const GMP_ROUND_MINUSINF: i64 = 2;

/// Absolute value (BigInt has no inherent `abs` without `num-traits::Signed`).
fn absv(b: BigInt) -> BigInt {
    if b.sign() == Sign::Minus {
        -b
    } else {
        b
    }
}

fn is_zero(b: &BigInt) -> bool {
    b.sign() == Sign::NoSign
}

/// The name of parameter `i` of `func`, from the declared signature.
fn param_name(func: &str, i: usize) -> &'static str {
    crate::argsig::sig_of(func)
        .and_then(|s| s.params.get(i))
        .map_or("num", |p| p.name)
}

fn value_error(msg: String) -> String {
    throws("ValueError", &msg)
}

/// Digit value of `c` in a base-62 alphabet (`0-9`, `A-Z`, `a-z`); GMP folds
/// case for bases up to 36.
fn digit_of(c: char, base: u32) -> Option<u32> {
    let d = match c {
        '0'..='9' => c as u32 - '0' as u32,
        'A'..='Z' => c as u32 - 'A' as u32 + 10,
        'a'..='z' if base <= 36 => c as u32 - 'a' as u32 + 10,
        'a'..='z' => c as u32 - 'a' as u32 + 36,
        _ => return None,
    };
    (d < base).then_some(d)
}

/// `convert_to_gmp` for a string: whitespace is ignored wherever it stands, a
/// `-` sign is optional (`mpz_set_str` rejects `+`), and base 0 (or the matching explicit base) reads a `0x`,
/// `0b` or `0o` prefix, a bare leading `0` meaning octal. `None` is "not an
/// integer string".
fn parse_integer_string(s: &str, base: u32) -> Option<BigInt> {
    let t: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    let (neg, body) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.as_str()),
    };
    let lower = body.to_ascii_lowercase();
    let (base, digits) = match (base, lower.as_str()) {
        (0 | 16, d) if d.starts_with("0x") => (16, &body[2..]),
        (0 | 2, d) if d.starts_with("0b") => (2, &body[2..]),
        (0 | 8, d) if d.starts_with("0o") => (8, &body[2..]),
        (0, d) if d.len() > 1 && d.starts_with('0') => (8, &body[1..]),
        (0, _) => (10, body),
        (b, _) => (b, body),
    };
    if digits.is_empty() {
        return None;
    }
    let mut acc = BigInt::from(0);
    for c in digits.chars() {
        acc = acc * base + digit_of(c, base)?;
    }
    Some(if neg { -acc } else { acc })
}

/// Operand `i` of the GMP function `func` as a `BigInt`: an int, a bool, a float
/// (truncated), or an integer string.
fn operand(func: &str, args: &[Value], i: usize) -> Result<BigInt, String> {
    match arg(args, i) {
        Value::Int(n) => Ok(BigInt::from(n)),
        Value::Str(s) => parse_integer_string(&s, 0).ok_or_else(|| {
            value_error(format!(
                "{func}(): Argument #{} (${}) is not an integer string",
                i + 1,
                param_name(func, i)
            ))
        }),
        Value::Float(f) if f.is_finite() && f.fract() != 0.0 => {
            let shown = with_host(|h| h.to_str(&Value::Float(f)));
            with_host(|h| {
                h.deprecated(format!(
                    "Implicit conversion from float {shown} to int loses precision"
                ))
            });
            Ok(BigInt::from(f as i64))
        }
        _ => Ok(BigInt::from(int_arg(args, i))),
    }
}

fn out(b: BigInt) -> Value {
    Value::str(b.to_string())
}

fn out_list(items: Vec<BigInt>) -> Value {
    make_list(items.into_iter().map(out).collect())
}

/// Euclidean GCD (always non-negative), since `num-bigint` alone has no `gcd`.
fn gcd(mut a: BigInt, mut b: BigInt) -> BigInt {
    a = absv(a);
    b = absv(b);
    while !is_zero(&b) {
        let r = &a % &b;
        a = b;
        b = r;
    }
    a
}

/// Extended Euclid: `(g, s, t)` with `s*a + t*b == g` and `g == gcd(a, b) >= 0`.
fn gcd_ext(a: &BigInt, b: &BigInt) -> (BigInt, BigInt, BigInt) {
    if is_zero(a) && is_zero(b) {
        return (BigInt::from(0), BigInt::from(0), BigInt::from(0));
    }
    let (mut old_r, mut r) = (a.clone(), b.clone());
    let (mut old_s, mut s) = (BigInt::from(1), BigInt::from(0));
    let (mut old_t, mut t) = (BigInt::from(0), BigInt::from(1));
    while !is_zero(&r) {
        let q = &old_r / &r;
        (old_r, r) = (r.clone(), &old_r - &q * &r);
        (old_s, s) = (s.clone(), &old_s - &q * &s);
        (old_t, t) = (t.clone(), &old_t - &q * &t);
    }
    if old_r.sign() == Sign::Minus {
        (-old_r, -old_s, -old_t)
    } else {
        (old_r, old_s, old_t)
    }
}

/// Quotient and remainder of `a / b` under a `GMP_ROUND_*` mode (`b != 0`), so
/// that `q * b + r == a`. `ZERO` truncates, `PLUSINF` rounds the quotient up and
/// `MINUSINF` down; the remainder takes whatever sign the mode leaves it.
fn div_rounded(a: &BigInt, b: &BigInt, mode: i64) -> (BigInt, BigInt) {
    let mut q = a / b;
    let mut r = a % b;
    if !is_zero(&r) {
        let signs_differ = (r.sign() == Sign::Minus) != (b.sign() == Sign::Minus);
        match mode {
            GMP_ROUND_PLUSINF if !signs_differ => {
                q += 1;
                r -= b;
            }
            GMP_ROUND_MINUSINF if signs_differ => {
                q -= 1;
                r += b;
            }
            _ => {}
        }
    }
    (q, r)
}

/// `gmp_mod`: always non-negative, whatever the signs.
fn gmp_modulo(a: &BigInt, m: &BigInt) -> BigInt {
    let r = a % m;
    if r.sign() == Sign::Minus {
        r + absv(m.clone())
    } else {
        r
    }
}

/// The `$rounding_mode` argument at index `i`, defaulting to `GMP_ROUND_ZERO`.
fn rounding_mode(func: &str, args: &[Value], i: usize) -> Result<i64, String> {
    if args.len() <= i {
        return Ok(GMP_ROUND_ZERO);
    }
    match int_arg(args, i) {
        m @ (GMP_ROUND_ZERO | GMP_ROUND_PLUSINF | GMP_ROUND_MINUSINF) => Ok(m),
        _ => Err(value_error(format!(
            "{func}(): Argument #{} ($rounding_mode) must be one of GMP_ROUND_ZERO, \
             GMP_ROUND_PLUSINF, or GMP_ROUND_MINUSINF",
            i + 1
        ))),
    }
}

fn division_by_zero(func: &str, pname_index: usize, what: &str) -> String {
    throws(
        "DivisionByZeroError",
        &format!(
            "{func}(): Argument #{} (${}) {what}",
            pname_index + 1,
            param_name(func, pname_index)
        ),
    )
}

/// `gmp_strval` digits of `b` in `base` (`2..=62`, or `-2..=-36` for upper case).
fn to_base(b: &BigInt, base: i64) -> String {
    let upper = base < 0;
    let radix = base.unsigned_abs() as u32;
    let mut digits = Vec::new();
    let mut n = absv(b.clone());
    if is_zero(&n) {
        digits.push('0');
    }
    let radix_big = BigInt::from(radix);
    while !is_zero(&n) {
        let d = u32::try_from(&n % &radix_big).expect("remainder below the radix");
        n /= &radix_big;
        digits.push(match d {
            0..=9 => (b'0' + d as u8) as char,
            10..=35 if upper || radix > 36 => (b'A' + (d - 10) as u8) as char,
            10..=35 => (b'a' + (d - 10) as u8) as char,
            _ => (b'a' + (d - 36) as u8) as char,
        });
    }
    if b.sign() == Sign::Minus {
        digits.push('-');
    }
    digits.iter().rev().collect()
}

/// Dispatch a `gmp_*` function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    // The body is a closure so every arm can use `?` on its operands.
    (|| {
        let n = |i: usize| operand(name, args, i);
        let v = match name {
            "gmp_init" => {
                let base = if args.len() > 1 { int_arg(args, 1) } else { 0 };
                if base != 0 && !(2..=62).contains(&base) {
                    return Err(value_error(
                        "gmp_init(): Argument #2 ($base) must be 0 or between 2 and 62".into(),
                    ));
                }
                match arg(args, 0) {
                    Value::Str(s) => {
                        out(parse_integer_string(&s, base as u32).ok_or_else(|| {
                            value_error(
                                "gmp_init(): Argument #1 ($num) is not an integer string".into(),
                            )
                        })?)
                    }
                    _ => out(n(0)?),
                }
            }
            "gmp_add" => out(n(0)? + n(1)?),
            "gmp_sub" => out(n(0)? - n(1)?),
            "gmp_mul" => out(n(0)? * n(1)?),
            "gmp_div_q" | "gmp_div" => {
                let (a, d) = (n(0)?, n(1)?);
                if is_zero(&d) {
                    return Err(division_by_zero(name, 1, "Division by zero"));
                }
                let mode = rounding_mode(name, args, 2)?;
                out(div_rounded(&a, &d, mode).0)
            }
            "gmp_div_r" => {
                let (a, d) = (n(0)?, n(1)?);
                if is_zero(&d) {
                    return Err(division_by_zero(name, 1, "Division by zero"));
                }
                let mode = rounding_mode(name, args, 2)?;
                out(div_rounded(&a, &d, mode).1)
            }
            "gmp_div_qr" => {
                let (a, d) = (n(0)?, n(1)?);
                if is_zero(&d) {
                    return Err(division_by_zero(name, 1, "Division by zero"));
                }
                let mode = rounding_mode(name, args, 2)?;
                let (q, r) = div_rounded(&a, &d, mode);
                out_list(vec![q, r])
            }
            "gmp_mod" => {
                let (a, m) = (n(0)?, n(1)?);
                if is_zero(&m) {
                    return Err(division_by_zero(name, 1, "Modulo by zero"));
                }
                out(gmp_modulo(&a, &m))
            }
            "gmp_divexact" => {
                let (a, d) = (n(0)?, n(1)?);
                if is_zero(&d) {
                    return Err(division_by_zero(name, 1, "Division by zero"));
                }
                out(a / d)
            }
            "gmp_pow" => {
                let base = n(0)?;
                let e = int_arg(args, 1);
                if e < 0 {
                    return Err(value_error(
                        "gmp_pow(): Argument #2 ($exponent) must be greater than or equal to 0"
                            .into(),
                    ));
                }
                // `pow` takes a `u32`; an exponent past that used to truncate, so
                // `gmp_pow("2", 4294967296)` answered `1`. The reference has no
                // ceiling here — it hands the exponent to `mpz_pow_ui` and dies of
                // memory exhaustion — so stop with a fatal of the same SHAPE
                // (uncatchable, program over) rather than return a wrong number.
                // DIVERGENCE: the reference's text names a byte count that depends
                // on its `memory_limit`, so it cannot be reproduced here.
                let Ok(e32) = u32::try_from(e) else {
                    return Err(crate::builtins::fatals(format!(
                        "gmp_pow(): exponent {e} exceeds the largest computable power"
                    )));
                };
                out(base.pow(e32))
            }
            "gmp_powm" => {
                let (base, e, m) = (n(0)?, n(1)?, n(2)?);
                if e.sign() == Sign::Minus {
                    return Err(value_error(
                        "gmp_powm(): Argument #2 ($exponent) must be greater than or equal to 0"
                            .into(),
                    ));
                }
                if is_zero(&m) {
                    return Err(throws("DivisionByZeroError", "Modulo by zero"));
                }
                // GMP reduces by |m| and the result is non-negative.
                out(gmp_modulo(&base, &m).modpow(&e, &absv(m)))
            }
            "gmp_gcd" => out(gcd(n(0)?, n(1)?)),
            "gmp_gcdext" => {
                let (g, s, t) = gcd_ext(&n(0)?, &n(1)?);
                make_map(vec![
                    (Value::str("g"), out(g)),
                    (Value::str("s"), out(s)),
                    (Value::str("t"), out(t)),
                ])
            }
            "gmp_lcm" => {
                let (a, b) = (n(0)?, n(1)?);
                if is_zero(&a) || is_zero(&b) {
                    out(BigInt::from(0))
                } else {
                    out(absv(&a / gcd(a.clone(), b.clone()) * &b))
                }
            }
            "gmp_invert" => {
                let (a, m) = (n(0)?, n(1)?);
                if is_zero(&m) {
                    return Err(division_by_zero(name, 1, "Division by zero"));
                }
                let (g, s, _) = gcd_ext(&a, &absv(m.clone()));
                if g == BigInt::from(1) {
                    out(gmp_modulo(&s, &m))
                } else {
                    Value::bool(false)
                }
            }
            "gmp_abs" => out(absv(n(0)?)),
            "gmp_neg" => out(-n(0)?),
            "gmp_com" => out(-n(0)? - 1),
            "gmp_and" => out(n(0)? & n(1)?),
            "gmp_or" => out(n(0)? | n(1)?),
            "gmp_xor" => out(n(0)? ^ n(1)?),
            "gmp_cmp" => Value::int(match n(0)?.cmp(&n(1)?) {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            }),
            "gmp_sign" => Value::int(match n(0)?.sign() {
                Sign::Minus => -1,
                Sign::NoSign => 0,
                Sign::Plus => 1,
            }),
            // `BigInt::sqrt` asserts on a negative operand ("square root is
            // imaginary"), so the sign test has to come FIRST in the root functions.
            "gmp_sqrt" | "gmp_sqrtrem" => {
                let b = n(0)?;
                if b.sign() == Sign::Minus {
                    return Err(value_error(format!(
                        "{name}(): Argument #1 ($num) must be greater than or equal to 0"
                    )));
                }
                let r = b.sqrt();
                if name == "gmp_sqrt" {
                    out(r)
                } else {
                    let rem = &b - &r * &r;
                    out_list(vec![r, rem])
                }
            }
            "gmp_root" | "gmp_rootrem" => {
                let b = n(0)?;
                let k = int_arg(args, 1);
                let (limit, limit_text) = if name == "gmp_root" {
                    (1, "greater than 0")
                } else {
                    (1, "greater than or equal to 1")
                };
                if k < limit {
                    return Err(value_error(format!(
                        "{name}(): Argument #2 ($nth) must be {limit_text}"
                    )));
                }
                // An even root of a negative number is imaginary; an ODD one is not,
                // and the reference computes it.
                if b.sign() == Sign::Minus && k % 2 == 0 {
                    return Err(value_error(format!(
                        "{name}(): Argument #2 ($nth) must be odd if argument #1 ($a) is negative"
                    )));
                }
                let root = match u32::try_from(k) {
                    Ok(2) => b.sqrt(),
                    Ok(k32) => int_root(&b, k32),
                    // Truncating to `u32` used to answer `gmp_root("8", 4294967296)`
                    // with `7`. A root that large is 1 for every operand above 1.
                    Err(_) if is_zero(&b) => BigInt::from(0),
                    Err(_) => BigInt::from(1),
                };
                if name == "gmp_root" {
                    out(root)
                } else {
                    let rem = match u32::try_from(k) {
                        Ok(k32) => &b - root.pow(k32),
                        Err(_) => &b - &root,
                    };
                    out_list(vec![root, rem])
                }
            }
            "gmp_fact" => {
                let k = int_arg(args, 0);
                if k < 0 {
                    return Err(value_error(
                        "gmp_fact(): Argument #1 ($num) must be greater than or equal to 0".into(),
                    ));
                }
                out((2..=k).fold(BigInt::from(1), |acc, i| acc * i))
            }
            "gmp_binomial" => {
                let (top, k) = (n(0)?, int_arg(args, 1));
                if k < 0 {
                    return Err(value_error(
                        "gmp_binomial(): Argument #2 ($k) must be greater than or equal to 0"
                            .into(),
                    ));
                }
                // C(n, k) = n (n-1) ... (n-k+1) / k!, which for a negative `n`
                // extends to (-1)^k C(k-n-1, k) — the product form covers both.
                let mut acc = BigInt::from(1);
                for i in 0..k {
                    acc = acc * (&top - i) / (i + 1);
                }
                out(acc)
            }
            "gmp_nextprime" => {
                let start: BigInt = n(0)?;
                let mut c = (start + BigInt::from(1)).max(BigInt::from(2));
                while prob_prime(&c) == 0 {
                    c += 1;
                }
                out(c)
            }
            "gmp_pow2" => out(BigInt::from(2).pow(int_arg(args, 0).max(0) as u32)),
            "gmp_strval" => {
                let base = if args.len() > 1 { int_arg(args, 1) } else { 10 };
                let value = n(0)?;
                if !(2..=62).contains(&base) && !(-36..=-2).contains(&base) {
                    return Err(value_error(
                        "gmp_strval(): Argument #2 ($base) must be between 2 and 62, or -2 and -36"
                            .into(),
                    ));
                }
                Value::str(to_base(&value, base))
            }
            // `mpz_get_si`: it reads only the lowest 64-bit limb of the magnitude. A
            // positive value keeps its low 63 bits; a negative one is `-1 - ((limb - 1)
            // & LONG_MAX)`, which is what makes -2^63 come back as `PHP_INT_MIN`.
            "gmp_intval" => Value::int({
                let b = n(0)?;
                let limb = u64::try_from(absv(b.clone()) & BigInt::from(u64::MAX)).expect("masked");
                let mask = i64::MAX as u64;
                if b.sign() == Sign::Minus {
                    -1 - (limb.wrapping_sub(1) & mask) as i64
                } else {
                    (limb & mask) as i64
                }
            }),
            "gmp_prob_prime" => Value::int(prob_prime(&n(0)?)),
            "gmp_perfect_square" => {
                let b = n(0)?;
                if b.sign() == Sign::Minus {
                    Value::bool(false)
                } else {
                    let r = b.sqrt();
                    Value::bool(&r * &r == b)
                }
            }
            "gmp_perfect_power" => Value::bool(is_perfect_power(&n(0)?)),
            "gmp_jacobi" | "gmp_kronecker" | "gmp_legendre" => {
                Value::int(kronecker(&n(0)?, &n(1)?))
            }
            "gmp_popcount" => {
                let b = n(0)?;
                // A negative number has infinitely many one bits: `ULONG_MAX`.
                Value::int(if b.sign() == Sign::Minus {
                    -1
                } else {
                    b.magnitude().count_ones() as i64
                })
            }
            "gmp_hamdist" => {
                let (a, b) = (n(0)?, n(1)?);
                Value::int(if (a.sign() == Sign::Minus) != (b.sign() == Sign::Minus) {
                    -1
                } else {
                    (a ^ b).magnitude().count_ones() as i64
                })
            }
            "gmp_testbit" => {
                let b = n(0)?;
                let idx = bit_index(name, args, 1)?;
                Value::bool(((b >> idx) & BigInt::from(1)) == BigInt::from(1))
            }
            "gmp_scan0" | "gmp_scan1" => {
                let b = n(0)?;
                let start = bit_index(name, args, 1)?;
                let want = u8::from(name == "gmp_scan1");
                // The sign bit extends forever, so a search for the opposite of it
                // past the magnitude finds nothing (`ULONG_MAX`, `-1` in PHP).
                let width = b.bits() + start + 2;
                let found = (start..=width)
                    .find(|&i| u8::try_from((&b >> i) & BigInt::from(1)) == Ok(want));
                match found {
                    Some(i) if i < width => Value::int(i as i64),
                    _ => Value::int(-1),
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(v))
    })()
    .transpose()
}

/// The bit index argument at `i`, which must be a non-negative `zend_long` the
/// reference can address (`0 .. 2147483647 * 64`).
fn bit_index(func: &str, args: &[Value], i: usize) -> Result<u64, String> {
    let idx = int_arg(args, i);
    if !(0..=2_147_483_647 * 64).contains(&idx) {
        return Err(value_error(format!(
            "{func}(): Argument #{} (${}) must be between 0 and 2147483647 * 64",
            i + 1,
            param_name(func, i)
        )));
    }
    Ok(idx as u64)
}

/// Integer nth root (floor) by binary search.
fn int_root(x: &BigInt, n: u32) -> BigInt {
    if n == 1 || is_zero(x) || x == &BigInt::from(1) {
        return x.clone();
    }
    // An odd root of a negative number is the negated root of its magnitude —
    // `gmp_root(-8, 3)` is `-2`. The binary search below only walks upwards from
    // zero, so it answered 0 for every negative operand.
    if x.sign() == Sign::Minus {
        return -int_root(&absv(x.clone()), n);
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

/// `mpz_perfect_power_p`: zero, one and minus one are perfect powers; any other
/// value must be `r^k` for some `k >= 2` (and an odd `k` when negative).
fn is_perfect_power(x: &BigInt) -> bool {
    if is_zero(x) || x == &BigInt::from(1) || x == &BigInt::from(-1) {
        return true;
    }
    let neg = x.sign() == Sign::Minus;
    let mag = absv(x.clone());
    (2..=mag.bits() as u32)
        .filter(|k| !neg || k % 2 == 1)
        .any(|k| int_root(&mag, k).pow(k) == mag)
}

/// The Kronecker symbol `(a / b)`, which extends the Jacobi and Legendre symbols
/// to every integer pair (`gmp_jacobi` and `gmp_legendre` return the same value
/// wherever they are defined).
fn kronecker(a: &BigInt, b: &BigInt) -> i64 {
    if is_zero(b) {
        return i64::from(absv(a.clone()) == BigInt::from(1));
    }
    let two = BigInt::from(2);
    let mut a = a.clone();
    let mut b = b.clone();
    let mut result = 1i64;
    if b.sign() == Sign::Minus {
        b = -b;
        if a.sign() == Sign::Minus {
            result = -result;
        }
    }
    // Pull the factors of two out of `b`.
    let mut twos = 0;
    while (&b % &two).sign() == Sign::NoSign {
        b /= &two;
        twos += 1;
    }
    if twos > 0 {
        if (&a % &two).sign() == Sign::NoSign {
            return 0;
        }
        let a8 = i64::try_from(gmp_modulo(&a, &BigInt::from(8))).expect("below 8");
        if twos % 2 == 1 && (a8 == 3 || a8 == 5) {
            result = -result;
        }
    }
    // Jacobi symbol for the odd positive `b` that remains.
    a = gmp_modulo(&a, &b);
    while !is_zero(&a) {
        while (&a % &two).sign() == Sign::NoSign {
            a /= &two;
            let b8 = i64::try_from(&b % BigInt::from(8)).expect("below 8");
            if b8 == 3 || b8 == 5 {
                result = -result;
            }
        }
        std::mem::swap(&mut a, &mut b);
        let four = BigInt::from(4);
        if &a % &four == BigInt::from(3) && &b % &four == BigInt::from(3) {
            result = -result;
        }
        a = &a % &b;
    }
    if b == BigInt::from(1) {
        result
    } else {
        0
    }
}

/// A small primality test (`gmp_prob_prime`): returns 2 (definitely prime),
/// 1 (probably prime), or 0 (composite). Uses deterministic trial division for
/// small n and Miller–Rabin with fixed bases above. The sign is ignored, as in
/// `mpz_probab_prime_p`.
fn prob_prime(n: &BigInt) -> i64 {
    let n = &absv(n.clone());
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
