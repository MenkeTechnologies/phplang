//! End-to-end tests for the `bcmath` stdlib category (`src/stdlib/bcmath.rs`):
//! arbitrary-precision decimal arithmetic where every operand and result is a
//! string and the trailing `$scale` argument truncates (never rounds) to an
//! exact number of fractional digits. Expected values were captured from the
//! reference `php` 8 bcmath extension.
//!
//! `bcadd("abc", "2", 2)` used to be recorded here as answering `"2.00"` — a
//! malformed operand read as zero. Operands are now validated against the
//! reference's grammar, and the divergence is gone; the tests below pin both
//! halves of that grammar, including the digitless spellings (`""`, `"."`,
//! `"+"`) that ARE well-formed and do read as zero.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

// ── bcadd / bcsub ────────────────────────────────────────────────────────────

#[test]
fn add_pads_to_scale() {
    assert_eq!(run(r#"<?php echo bcadd("1", "2", 2);"#), "3.00");
    assert_eq!(run(r#"<?php echo bcadd("1.1", "2.2", 1);"#), "3.3");
    // Truncation, not rounding: 3.005 -> 3.00 at scale 2.
    assert_eq!(run(r#"<?php echo bcadd("1.005", "2", 2);"#), "3.00");
    // Big integers exceed i64; exact decimal arithmetic must still be correct.
    assert_eq!(
        run(r#"<?php echo bcadd("12345678901234567890", "98765432109876543210");"#),
        "111111111011111111100"
    );
}

#[test]
fn sub_negative_and_zero() {
    assert_eq!(run(r#"<?php echo bcsub("0", "1.5", 2);"#), "-1.50");
    // Result truncates to exactly zero -> no "-0.00".
    assert_eq!(run(r#"<?php echo bcsub("5", "5", 2);"#), "0.00");
    assert_eq!(run(r#"<?php echo bcsub("10", "3");"#), "7");
}

// ── bcmul ────────────────────────────────────────────────────────────────────

#[test]
fn mul_scale_and_sign() {
    assert_eq!(run(r#"<?php echo bcmul("2.5", "2.5", 2);"#), "6.25");
    // Default scale 0 truncates the fractional product.
    assert_eq!(run(r#"<?php echo bcmul("2.5", "2.5");"#), "6");
    assert_eq!(run(r#"<?php echo bcmul("-2.5", "2.5", 3);"#), "-6.250");
    assert_eq!(run(r#"<?php echo bcmul("0", "0", 3);"#), "0.000");
}

// ── bcdiv ────────────────────────────────────────────────────────────────────

#[test]
fn div_truncates_repeating() {
    assert_eq!(run(r#"<?php echo bcdiv("1", "3", 5);"#), "0.33333");
    assert_eq!(run(r#"<?php echo bcdiv("10", "3", 4);"#), "3.3333");
    assert_eq!(run(r#"<?php echo bcdiv("-10", "3", 2);"#), "-3.33");
    // Default scale 0: 10/3 -> 3, 10/4 -> 2 (truncation toward zero).
    assert_eq!(run(r#"<?php echo bcdiv("10", "3");"#), "3");
    assert_eq!(run(r#"<?php echo bcdiv("10", "4");"#), "2");
}

#[test]
fn div_by_zero_errors() {
    let e = eval_capture(r#"<?php echo bcdiv("1", "0", 2);"#).unwrap_err();
    assert!(e.to_lowercase().contains("division by zero"), "got: {e}");
}

// ── bcmod ────────────────────────────────────────────────────────────────────

#[test]
fn mod_integer_and_fractional() {
    assert_eq!(run(r#"<?php echo bcmod("10", "3", 2);"#), "1.00");
    assert_eq!(run(r#"<?php echo bcmod("-10", "3", 2);"#), "-1.00");
    // Fractional operands: 8.5 mod 2.1 -> 8.5 - 2.1*4 = 0.1.
    assert_eq!(run(r#"<?php echo bcmod("8.5", "2.1", 4);"#), "0.1000");
    assert_eq!(run(r#"<?php echo bcmod("10.5", "3", 4);"#), "1.5000");
    assert_eq!(run(r#"<?php echo bcmod("10", "3");"#), "1");
}

#[test]
fn mod_by_zero_errors() {
    let e = eval_capture(r#"<?php echo bcmod("1", "0");"#).unwrap_err();
    assert!(e.to_lowercase().contains("modulo by zero"), "got: {e}");
}

// ── bcpow ────────────────────────────────────────────────────────────────────

#[test]
fn pow_integer_exponent() {
    assert_eq!(run(r#"<?php echo bcpow("2", "10", 0);"#), "1024");
    assert_eq!(run(r#"<?php echo bcpow("2.5", "2", 2);"#), "6.25");
    assert_eq!(run(r#"<?php echo bcpow("-2", "3");"#), "-8");
    assert_eq!(run(r#"<?php echo bcpow("2", "0", 2);"#), "1.00");
}

#[test]
fn pow_negative_exponent_is_reciprocal() {
    assert_eq!(run(r#"<?php echo bcpow("2", "-2", 4);"#), "0.2500");
    // 2^-3 = 0.125, truncated to scale 0 -> 0.
    assert_eq!(run(r#"<?php echo bcpow("2", "-3", 0);"#), "0");
}

// ── bcsqrt ───────────────────────────────────────────────────────────────────

#[test]
fn sqrt_truncates_to_scale() {
    assert_eq!(run(r#"<?php echo bcsqrt("2", 5);"#), "1.41421");
    assert_eq!(run(r#"<?php echo bcsqrt("9", 2);"#), "3.00");
    assert_eq!(run(r#"<?php echo bcsqrt("0", 2);"#), "0.00");
    assert_eq!(run(r#"<?php echo bcsqrt("2", 0);"#), "1");
}

#[test]
fn sqrt_negative_errors() {
    let e = eval_capture(r#"<?php echo bcsqrt("-4", 2);"#).unwrap_err();
    assert!(
        e.to_lowercase().contains("greater than or equal"),
        "got: {e}"
    );
}

// ── bccomp ───────────────────────────────────────────────────────────────────

#[test]
fn comp_at_scale() {
    // 1.0 vs 1.00 are equal numerically and at scale 2.
    assert_eq!(run(r#"<?php echo bccomp("1.0", "1.00", 2);"#), "0");
    assert_eq!(run(r#"<?php echo bccomp("1", "2");"#), "-1");
    assert_eq!(run(r#"<?php echo bccomp("2", "1");"#), "1");
    // Truncated to scale 2 both become 1.00 -> equal.
    assert_eq!(run(r#"<?php echo bccomp("1.001", "1.002", 2);"#), "0");
    // At scale 3 the third fractional digit distinguishes them.
    assert_eq!(run(r#"<?php echo bccomp("1.001", "1.002", 3);"#), "-1");
}

// ── bcscale ──────────────────────────────────────────────────────────────────

#[test]
fn scale_default_state() {
    // bcscale($s) returns the previous scale and sets the new module default,
    // which subsequent scale-less calls use. Restore to 0 at the end so the
    // per-thread default does not leak to sibling tests on the same thread.
    let out = run(r#"<?php
$prev = bcscale(3);
echo $prev, ":", bcscale(), ":", bcadd("1", "2"), ":", bcscale(0);"#);
    assert_eq!(out, "0:3:3.000:3");
}

// ── bcpowmod ─────────────────────────────────────────────────────────────────

#[test]
fn powmod_modular_exponentiation() {
    assert_eq!(run(r#"<?php echo bcpowmod("2", "10", "7", 0);"#), "2");
    assert_eq!(run(r#"<?php echo bcpowmod("3", "20", "100", 0);"#), "1");
    assert_eq!(run(r#"<?php echo bcpowmod("2", "0", "7", 0);"#), "1");
    assert_eq!(run(r#"<?php echo bcpowmod("2", "10", "1", 0);"#), "0");
    // Scale pads the (integer) modular result.
    assert_eq!(run(r#"<?php echo bcpowmod("2", "10", "7", 2);"#), "2.00");
}

#[test]
fn powmod_by_zero_errors() {
    let e = eval_capture(r#"<?php echo bcpowmod("2", "10", "0");"#).unwrap_err();
    assert!(e.to_lowercase().contains("modulo by zero"), "got: {e}");
}

// ── operand grammar ──────────────────────────────────────────────────────────

/// bcmath accepts `[+-]? DIGIT* ( '.' DIGIT* )?` and nothing else. A string with
/// no digits at all is still well-formed and reads as zero, which is why `""`
/// and `"."` are values rather than errors — the surprising half of the grammar,
/// and the half a "reject anything that is not a number" implementation breaks.
#[test]
fn digitless_but_well_formed_operands_read_as_zero() {
    for spelling in [r#""""#, r#"".""#, r#""+""#, r#""-""#, r#""-.""#, r#""+.""#] {
        assert_eq!(
            run(&format!(r#"<?php echo bcadd({spelling}, "0", 2);"#)),
            "0.00",
            "bcadd({spelling}, \"0\", 2)"
        );
    }
    // A point on either side is well-formed and keeps its digits.
    assert_eq!(run(r#"<?php echo bcadd("5.", "0", 2);"#), "5.00");
    assert_eq!(run(r#"<?php echo bcadd(".5", "0", 2);"#), "0.50");
    assert_eq!(run(r#"<?php echo bcadd("00012", "0", 2);"#), "12.00");
    assert_eq!(run(r#"<?php echo bcadd("-0", "0", 2);"#), "0.00");
}

/// A malformed operand is a `ValueError`, not a lenient zero. The message names
/// the FIRST bad argument, so the position and parameter name both have to be
/// right — an implementation that validated after computing, or that checked
/// argument 2 first, would still throw but name the wrong one.
///
/// Every expectation re-verified against php 8.5.9.
#[test]
fn malformed_operands_are_value_errors_naming_the_first_bad_argument() {
    let cases = [
        (r#"bcadd("abc", "2")"#, "bcadd(): Argument #1 ($num1)"),
        (r#"bcadd("2", "abc")"#, "bcadd(): Argument #2 ($num2)"),
        (r#"bcsub("2", "xyz")"#, "bcsub(): Argument #2 ($num2)"),
        (r#"bcmul("q", "1")"#, "bcmul(): Argument #1 ($num1)"),
        // The well-formed check runs BEFORE the zero divisor is noticed, so this
        // is an operand error and not a DivisionByZeroError.
        (r#"bcdiv("a", "0")"#, "bcdiv(): Argument #1 ($num1)"),
        (r#"bcmod("a", "0")"#, "bcmod(): Argument #1 ($num1)"),
        (r#"bccomp("0", "a")"#, "bccomp(): Argument #2 ($num2)"),
        (r#"bcpow("2", "a")"#, "bcpow(): Argument #2 ($exponent)"),
        (r#"bcsqrt("a")"#, "bcsqrt(): Argument #1 ($num)"),
        (
            r#"bcpowmod("1", "1", "a")"#,
            "bcpowmod(): Argument #3 ($modulus)",
        ),
    ];
    for (call, prefix) in cases {
        assert_eq!(
            run(&format!(
                r#"<?php try {{ {call}; }} catch (Throwable $e) {{ echo get_class($e), ': ', $e->getMessage(); }}"#
            )),
            format!("ValueError: {prefix} is not well-formed"),
            "{call}"
        );
    }
    // Spellings that look numeric to a general-purpose parser but are outside
    // bcmath's grammar — these are the ones a `f64::from_str` fallback accepts.
    for spelling in [
        r#""1e3""#,
        r#"" 1""#,
        r#""1 ""#,
        r#""0x10""#,
        r#""1_000""#,
        r#""1.2.3""#,
    ] {
        assert_eq!(
            run(&format!(
                r#"<?php try {{ bcadd({spelling}, "0"); }} catch (Throwable $e) {{ echo $e->getMessage(); }}"#
            )),
            "bcadd(): Argument #1 ($num1) is not well-formed",
            "bcadd({spelling}, \"0\")"
        );
    }
}

/// `bcpow`/`bcpowmod` use some operands as integers and reject a fractional part
/// rather than truncating it. The check order is observable when a call is bad in
/// two ways at once, and it is not left-to-right: `$exponent >= 0` is tested
/// between the `$num`/`$exponent` fractional checks and the `$modulus` one.
#[test]
fn integral_operand_checks_run_in_the_reference_order() {
    let cases = [
        (
            r#"bcpow("2", "1.5")"#,
            "bcpow(): Argument #2 ($exponent) cannot have a fractional part",
        ),
        (
            r#"bcpowmod("2.5", "3", "7")"#,
            "bcpowmod(): Argument #1 ($num) cannot have a fractional part",
        ),
        (
            r#"bcpowmod("1", "1", "2.5")"#,
            "bcpowmod(): Argument #3 ($modulus) cannot have a fractional part",
        ),
        // A negative exponent outranks a fractional MODULUS...
        (
            r#"bcpowmod("1", "-1", "2.5")"#,
            "bcpowmod(): Argument #2 ($exponent) must be greater than or equal to 0",
        ),
        // ...but a fractional $num outranks the negative exponent.
        (
            r#"bcpowmod("1.5", "-1", "7")"#,
            "bcpowmod(): Argument #1 ($num) cannot have a fractional part",
        ),
        // A malformed operand outranks every one of them.
        (
            r#"bcpowmod("a", "-1", "0")"#,
            "bcpowmod(): Argument #1 ($num) is not well-formed",
        ),
    ];
    for (call, msg) in cases {
        assert_eq!(
            run(&format!(
                r#"<?php try {{ {call}; }} catch (Throwable $e) {{ echo $e->getMessage(); }}"#
            )),
            msg,
            "{call}"
        );
    }
}
