//! End-to-end tests for the `bcmath` stdlib category (`src/stdlib/bcmath.rs`):
//! arbitrary-precision decimal arithmetic where every operand and result is a
//! string and the trailing `$scale` argument truncates (never rounds) to an
//! exact number of fractional digits. Expected values were captured from the
//! reference `php` 8 bcmath extension.

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
    assert!(e.to_lowercase().contains("greater than or equal"), "got: {e}");
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
    let out = run(
        r#"<?php
$prev = bcscale(3);
echo $prev, ":", bcscale(), ":", bcadd("1", "2"), ":", bcscale(0);"#,
    );
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

// ── lenient parsing (documented divergence) ──────────────────────────────────

#[test]
fn empty_and_malformed_operands_read_as_zero() {
    assert_eq!(run(r#"<?php echo bcadd("", "", 2);"#), "0.00");
    // PHP 8 raises ValueError for "abc"; this port leniently treats it as 0.
    assert_eq!(run(r#"<?php echo bcadd("abc", "2", 2);"#), "2.00");
}
