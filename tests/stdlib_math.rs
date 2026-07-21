//! End-to-end tests for the `math` stdlib category (`src/stdlib/math.rs`):
//! the extended trig/hyperbolic family, radian/degree helpers, IEEE division and
//! predicates, base conversion, and the PRNG functions. Deterministic functions
//! assert exact echoed output (PHP `precision=14` formatting); the pseudo-random
//! generators assert only range/bounds, since the bit sequence is unspecified.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

// ── logs / exp ───────────────────────────────────────────────────────────────

#[test]
fn log1p_expm1() {
    assert_eq!(run("<?php echo expm1(0);"), "0");
    assert_eq!(run("<?php echo log1p(0);"), "0");
    // expm1/log1p round-trip near zero with full precision.
    assert_eq!(run("<?php echo round(log1p(expm1(0.5)), 10);"), "0.5");
}

// Regression (fix 1): PHP has no `log2()` — `function_exists("log2")` is false,
// so calling it must raise "call to undefined function", not compute log base 2.
#[test]
fn log2_is_undefined() {
    let e = eval_capture("<?php echo log2(8);").unwrap_err();
    assert!(e.contains("undefined function log2"), "got: {e}");
}

// ── hyperbolic + inverses ────────────────────────────────────────────────────

#[test]
fn hyperbolic_zero_identities() {
    assert_eq!(run("<?php echo sinh(0);"), "0");
    assert_eq!(run("<?php echo cosh(0);"), "1");
    assert_eq!(run("<?php echo tanh(0);"), "0");
    assert_eq!(run("<?php echo asinh(0);"), "0");
    assert_eq!(run("<?php echo acosh(1);"), "0");
    assert_eq!(run("<?php echo atanh(0);"), "0");
}

#[test]
fn inverse_trig_landmarks() {
    // asin(1) = pi/2, acos(1) = 0, atan(1) = pi/4, atan2(1,1) = pi/4.
    assert_eq!(run("<?php echo round(asin(1), 10);"), "1.5707963268");
    assert_eq!(run("<?php echo acos(1);"), "0");
    assert_eq!(run("<?php echo round(atan(1), 10);"), "0.7853981634");
    assert_eq!(run("<?php echo round(atan2(1, 1), 10);"), "0.7853981634");
    assert_eq!(run("<?php echo round(atan2(0, -1), 10);"), "3.1415926536");
}

// ── angle conversion ─────────────────────────────────────────────────────────

#[test]
fn deg_rad_roundtrip() {
    assert_eq!(
        run("<?php echo round(deg2rad(180), 13);"),
        "3.1415926535898"
    );
    assert_eq!(run("<?php echo rad2deg(3.141592653589793);"), "180");
    assert_eq!(run("<?php echo round(rad2deg(deg2rad(90)), 6);"), "90");
}

// ── geometry / IEEE division ─────────────────────────────────────────────────

#[test]
fn hypot_pythagorean() {
    assert_eq!(run("<?php echo hypot(3, 4);"), "5");
    assert_eq!(run("<?php echo hypot(5, 12);"), "13");
}

#[test]
fn fdiv_ieee_edges() {
    assert_eq!(run("<?php echo fdiv(10, 4);"), "2.5");
    assert_eq!(run("<?php echo fdiv(1, 0);"), "INF");
    assert_eq!(run("<?php echo fdiv(-1, 0);"), "-INF");
    assert_eq!(run("<?php echo fdiv(0, 0);"), "NAN");
}

// ── IEEE predicates ──────────────────────────────────────────────────────────

#[test]
fn ieee_predicates() {
    assert_eq!(run("<?php echo is_nan(fdiv(0, 0)) ? 'y' : 'n';"), "y");
    assert_eq!(run("<?php echo is_nan(1.5) ? 'y' : 'n';"), "n");
    assert_eq!(run("<?php echo is_infinite(fdiv(1, 0)) ? 'y' : 'n';"), "y");
    assert_eq!(run("<?php echo is_infinite(1.5) ? 'y' : 'n';"), "n");
    assert_eq!(run("<?php echo is_finite(1.5) ? 'y' : 'n';"), "y");
    assert_eq!(run("<?php echo is_finite(fdiv(1, 0)) ? 'y' : 'n';"), "n");
}

// ── base conversion ──────────────────────────────────────────────────────────

#[test]
fn decimal_to_base_strings() {
    assert_eq!(run("<?php echo decbin(10);"), "1010");
    assert_eq!(run("<?php echo decbin(255);"), "11111111");
    assert_eq!(run("<?php echo decoct(64);"), "100");
    assert_eq!(run("<?php echo decoct(8);"), "10");
}

#[test]
fn base_to_decimal_ints() {
    assert_eq!(run("<?php echo bindec('1010');"), "10");
    assert_eq!(run("<?php echo bindec('11111111');"), "255");
    assert_eq!(run("<?php echo octdec('100');"), "64");
    assert_eq!(run("<?php echo octdec('777');"), "511");
    // Invalid digits are silently ignored (PHP behavior).
    assert_eq!(run("<?php echo bindec('1a0b1');"), "5");
}

#[test]
fn base_convert_various() {
    assert_eq!(run("<?php echo base_convert('ff', 16, 2);"), "11111111");
    assert_eq!(
        run("<?php echo base_convert('a37334', 16, 2);"),
        "101000110111001100110100"
    );
    assert_eq!(run("<?php echo base_convert('2557', 10, 16);"), "9fd");
    assert_eq!(run("<?php echo base_convert('z', 36, 10);"), "35");
    assert_eq!(run("<?php echo base_convert('0', 10, 2);"), "0");
}

#[test]
fn base_convert_out_of_range_errors() {
    assert!(eval_capture("<?php echo base_convert('1', 1, 10);").is_err());
    assert!(eval_capture("<?php echo base_convert('1', 10, 37);").is_err());
}

// Regression (fix 4): the out-of-range message ends with "(inclusive)" to match
// PHP 8 exactly, for both the $from_base and $to_base arguments.
#[test]
fn base_convert_out_of_range_message() {
    let e = eval_capture("<?php echo base_convert('1', 1, 10);").unwrap_err();
    assert_eq!(
        e,
        "base_convert(): Argument #2 ($from_base) must be between 2 and 36 (inclusive)"
    );
    let e = eval_capture("<?php echo base_convert('1', 10, 37);").unwrap_err();
    assert_eq!(
        e,
        "base_convert(): Argument #3 ($to_base) must be between 2 and 36 (inclusive)"
    );
}

// ── pseudo-random ────────────────────────────────────────────────────────────

#[test]
fn randmax_constants() {
    assert_eq!(run("<?php echo mt_getrandmax();"), "2147483647");
    assert_eq!(run("<?php echo getrandmax();"), "2147483647");
}

#[test]
fn rand_bounds_are_inclusive_and_respected() {
    // A degenerate range [7,7] must always yield 7; a seeded sequence must stay
    // within [1,6]. Exact values are unspecified, only membership is asserted.
    assert_eq!(run("<?php echo rand(7, 7);"), "7");
    assert_eq!(run("<?php echo mt_rand(3, 3);"), "3");
    assert_eq!(
        run("<?php mt_srand(42); $ok = true; for ($i = 0; $i < 200; $i++) { $r = mt_rand(1, 6); if ($r < 1 || $r > 6) $ok = false; } echo $ok ? 'ok' : 'bad';"),
        "ok"
    );
    assert_eq!(
        run("<?php srand(1); $ok = true; for ($i = 0; $i < 200; $i++) { $r = rand(0, 1); if ($r !== 0 && $r !== 1) $ok = false; } echo $ok ? 'ok' : 'bad';"),
        "ok"
    );
}

#[test]
fn rand_no_args_within_getrandmax() {
    assert_eq!(
        run("<?php $r = mt_rand(); echo ($r >= 0 && $r <= mt_getrandmax()) ? 'ok' : 'bad';"),
        "ok"
    );
}

#[test]
fn random_int_bounds() {
    assert_eq!(run("<?php echo random_int(5, 5);"), "5");
    assert_eq!(
        run("<?php $ok = true; for ($i = 0; $i < 200; $i++) { $r = random_int(-3, 3); if ($r < -3 || $r > 3) $ok = false; } echo $ok ? 'ok' : 'bad';"),
        "ok"
    );
}

// Regression (fix 2): `rand()` with min > max SWAPS the bounds (PHP does not
// error), so the result stays within [max, min].
#[test]
fn rand_swaps_inverted_bounds() {
    // Degenerate inverted range [5,5] still yields 5.
    assert_eq!(run("<?php echo rand(5, 5);"), "5");
    assert_eq!(
        run("<?php srand(7); $ok = true; for ($i = 0; $i < 200; $i++) { $r = rand(10, 3); if ($r < 3 || $r > 10) $ok = false; } echo $ok ? 'ok' : 'bad';"),
        "ok"
    );
}

#[test]
fn inverted_bounds_error() {
    assert!(eval_capture("<?php echo mt_rand(10, 1);").is_err());
    assert!(eval_capture("<?php echo random_int(10, 1);").is_err());
}

// Regression (fix 3): mt_rand and random_int each emit their own PHP 8 message
// on inverted bounds (and rand does NOT error — covered above).
#[test]
fn inverted_bounds_error_messages() {
    assert_eq!(
        eval_capture("<?php echo mt_rand(10, 1);").unwrap_err(),
        "mt_rand(): Argument #2 ($max) must be greater than or equal to argument #1 ($min)"
    );
    assert_eq!(
        eval_capture("<?php echo random_int(10, 1);").unwrap_err(),
        "random_int(): Argument #1 ($min) must be less than or equal to argument #2 ($max)"
    );
}
