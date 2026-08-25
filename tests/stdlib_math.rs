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
    // Echoing the NaN CONVERTS it, and the reference warns when it does:
    //   php -r 'echo fdiv(0, 0);'
    //   Warning: unexpected NAN value was coerced to string in Command line code on line 1
    //   NAN
    // This used to assert the bare "NAN", which no PHP has printed for this
    // snippet. The infinities above are the control — they convert silently.
    assert_eq!(run("<?php echo fdiv(0, 0);"), format!("{NAN_COERCED}NAN"));
    // The VALUE is unaffected by the diagnostic; `is_nan` reads it without
    // converting and so says nothing.
    assert_eq!(run("<?php var_dump(is_nan(fdiv(0, 0)));"), "bool(true)\n");
}

/// The reference's warning for a NaN that reaches a string conversion.
const NAN_COERCED: &str =
    "\nWarning: unexpected NAN value was coerced to string in Command line code on line 1\n";

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
    // A base-matching literal prefix is dropped, with no diagnostic:
    //
    // ```text
    // $ php -r "echo bindec('0b101');"   => 5
    // $ php -r "echo octdec('0o17');"    => 15
    // $ php -r "echo hexdec('0x1f');"    => 31
    // ```
    assert_eq!(run("<?php echo bindec('0b101');"), "5");
    assert_eq!(run("<?php echo octdec('0o17');"), "15");
    assert_eq!(run("<?php echo hexdec('0x1f');"), "31");
    // Characters the base cannot use ARE ignored — but not silently. The old
    // expectation here was `"5"` with no diagnostic, which pinned phplang's own
    // behaviour rather than the reference's:
    //
    // ```text
    // $ php -r "echo bindec('1a0b1');"
    //
    // Deprecated: Invalid characters passed for attempted conversion, these have been ignored in Command line code on line 1
    // 5
    // ```
    let ignored = "\nDeprecated: Invalid characters passed for attempted conversion, \
                   these have been ignored in Command line code on line 1\n";
    assert_eq!(run("<?php echo bindec('1a0b1');"), format!("{ignored}5"));
    assert_eq!(run("<?php echo hexdec('zz');"), format!("{ignored}0"));
    assert_eq!(run("<?php echo octdec('9');"), format!("{ignored}0"));
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

/// The BOUNDARY, which the message test beside this one does not cover: 2 and 36
/// are accepted and 1 and 37 are not. `is_err()` on the two failing calls alone
/// would still pass if the accepted range had shrunk to nothing.
#[test]
fn base_convert_out_of_range_errors() {
    for bad in ["base_convert('1', 1, 10)", "base_convert('1', 10, 37)"] {
        assert!(
            eval_capture(&format!("<?php echo {bad};")).is_err(),
            "{bad} should be rejected"
        );
    }
    // Both ends of the accepted range work.
    assert_eq!(run("<?php echo base_convert('101', 2, 10);"), "5");
    assert_eq!(run("<?php echo base_convert('z', 36, 10);"), "35");
    assert_eq!(run("<?php echo base_convert('5', 10, 2);"), "101");
    assert_eq!(run("<?php echo base_convert('35', 10, 36);"), "z");
}

// Regression (fix 4): the out-of-range message ends with "(inclusive)" to match
// PHP 8 exactly, for both the $from_base and $to_base arguments — and it is a
// CATCHABLE `ValueError`, not a host-level abort, so a `try` block sees it.
// Re-verified against php 8.5.9.
#[test]
fn base_convert_out_of_range_message() {
    assert_eq!(
        run("<?php try { base_convert('1', 1, 10); } \
             catch (Throwable $e) { echo get_class($e), ': ', $e->getMessage(); }"),
        "ValueError: base_convert(): Argument #2 ($from_base) must be between 2 and 36 (inclusive)"
    );
    assert_eq!(
        run("<?php try { base_convert('1', 10, 37); } \
             catch (Throwable $e) { echo get_class($e), ': ', $e->getMessage(); }"),
        "ValueError: base_convert(): Argument #3 ($to_base) must be between 2 and 36 (inclusive)"
    );
    // The catch has to run and the program continue — an abort would print
    // neither the message nor what follows it.
    assert_eq!(
        run("<?php try { base_convert('1', 1, 10); } catch (ValueError $e) {} echo 'after';"),
        "after"
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
    // The bound is pinned to the value PHP fixes for mt_getrandmax(), not read
    // back out of the engine under test: `$r <= mt_getrandmax()` alone compares
    // the engine against itself, and `$r >= 0` is trivially true, so the old
    // form passed for an `mt_rand` hardwired to return 0.
    assert_eq!(
        run("<?php echo mt_getrandmax();"),
        "2147483647",
        "mt_getrandmax() is fixed at 2^31-1 in the reference"
    );
    assert_eq!(
        run("<?php $r = mt_rand(); echo ($r >= 0 && $r <= 2147483647) ? 'ok' : 'bad';"),
        "ok"
    );
    // …and that it actually varies. A constant generator passes every range
    // check ever written; 40 draws collapsing to one value does not happen by
    // chance for a real PRNG.
    assert_eq!(
        run(
            "<?php $s = []; for ($i = 0; $i < 40; $i++) { $s[mt_rand()] = 1; } \
             echo count($s) > 1 ? 'varies' : 'constant';"
        ),
        "varies"
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

/// The boundary beside the message test: EQUAL bounds are accepted, so the guard
/// must be `min > max` and not `min >= max`. Two `is_err()` calls cannot see the
/// difference.
#[test]
fn inverted_bounds_error() {
    assert!(eval_capture("<?php echo mt_rand(10, 1);").is_err());
    assert!(eval_capture("<?php echo random_int(10, 1);").is_err());
    assert_eq!(run("<?php echo mt_rand(4, 4);"), "4");
    assert_eq!(run("<?php echo random_int(4, 4);"), "4");
    assert_eq!(run("<?php echo mt_rand(-4, -4);"), "-4");
}

// Regression (fix 3): mt_rand and random_int each emit their own PHP 8 message
// on inverted bounds (and rand does NOT error — covered above).
#[test]
fn inverted_bounds_error_messages() {
    // Both are catchable `ValueError`s, and the two messages are NOT the same
    // sentence with the function name swapped — `mt_rand` blames `$max` and
    // `random_int` blames `$min`. Re-verified against php 8.5.9.
    assert_eq!(
        run(
            "<?php try { mt_rand(10, 1); } \
             catch (Throwable $e) { echo get_class($e), ': ', $e->getMessage(); }"
        ),
        "ValueError: mt_rand(): Argument #2 ($max) must be greater than or equal to argument #1 ($min)"
    );
    assert_eq!(
        run(
            "<?php try { random_int(10, 1); } \
             catch (Throwable $e) { echo get_class($e), ': ', $e->getMessage(); }"
        ),
        "ValueError: random_int(): Argument #1 ($min) must be less than or equal to argument #2 ($max)"
    );
    // `rand()` SWAPS inverted bounds instead of raising, so the guard must not
    // have been hoisted into the shared bounds helper.
    assert_eq!(
        run("<?php $v = rand(10, 1); echo ($v >= 1 && $v <= 10) ? 'in' : 'out';"),
        "in"
    );
}

// ── `0 ** <negative>` ────────────────────────────────────────────────────────

/// PHP 8.4 deprecated a zero base raised to a negative exponent. The value was
/// already `INF`; only the diagnostic was missing, and it was missing from BOTH
/// places a power is computed — the `**` operator and the `pow()` function.
#[test]
fn zero_base_and_a_negative_exponent_is_deprecated() {
    const MSG: &str =
        "\nDeprecated: Power of base 0 and negative exponent is deprecated in Command line code on line 1\n";
    assert_eq!(run(r#"<?php echo 0 ** -1;"#), format!("{MSG}INF"));
    assert_eq!(run(r#"<?php echo pow(0, -1);"#), format!("{MSG}INF"));
    // `**=` compiles to the same opcode as `**`.
    assert_eq!(
        run(r#"<?php $x = 0; $x **= -1; echo $x;"#),
        format!("{MSG}INF")
    );
    // The test is on the COERCED base, so a numeric string and `false` fire too.
    assert_eq!(run(r#"<?php echo "0" ** -1;"#), format!("{MSG}INF"));
    assert_eq!(run(r#"<?php echo false ** -1;"#), format!("{MSG}INF"));
    // A `-0.0` base keeps `powf`'s sign for an odd exponent.
    assert_eq!(run(r#"<?php echo (-0.0) ** -1;"#), format!("{MSG}-INF"));
    assert_eq!(run(r#"<?php echo (-0.0) ** -2;"#), format!("{MSG}INF"));
}

/// The exponent test is `< 0`, not `<= 0`, and NAN is not negative — both of
/// these are silent, and a non-zero base never fires at all.
#[test]
fn a_zero_or_nan_exponent_and_a_non_zero_base_stay_silent() {
    assert_eq!(run(r#"<?php echo 0 ** 0;"#), "1");
    assert_eq!(run(r#"<?php var_dump(0 ** -0.0);"#), "float(1)\n");
    assert_eq!(run(r#"<?php echo 1 ** -1;"#), "1");
    assert_eq!(run(r#"<?php echo 2 ** -1;"#), "0.5");
    // Suppressed by the error-reporting mask like any other deprecation.
    assert_eq!(run(r#"<?php error_reporting(0); echo 0 ** -1;"#), "INF");
    assert_eq!(run(r#"<?php echo @(0 ** -1);"#), "INF");
}

// ── round() tie-break modes ──────────────────────────────────────────────────

/// The `$mode` argument was accepted and IGNORED, so every mode behaved as
/// `PHP_ROUND_HALF_UP` and three of the four constants were decorative. Only a
/// value exactly halfway between two integers can tell them apart, so a test
/// using ordinary values passes with the argument dropped entirely.
///
/// All 20 pairs re-verified against php 8.5.9.
#[test]
fn round_honours_every_half_mode() {
    // (value, HALF_UP, HALF_DOWN, HALF_EVEN, HALF_ODD)
    let cases = [
        (0.5, "1", "0", "0", "1"),
        (1.5, "2", "1", "2", "1"),
        (2.5, "3", "2", "2", "3"),
        (3.5, "4", "3", "4", "3"),
        (-0.5, "-1", "-0", "-0", "-1"),
        (-1.5, "-2", "-1", "-2", "-1"),
        (-2.5, "-3", "-2", "-2", "-3"),
    ];
    for (v, up, down, even, odd) in cases {
        for (mode, want) in [
            ("PHP_ROUND_HALF_UP", up),
            ("PHP_ROUND_HALF_DOWN", down),
            ("PHP_ROUND_HALF_EVEN", even),
            ("PHP_ROUND_HALF_ODD", odd),
        ] {
            assert_eq!(
                run(&format!("<?php echo round({v}, 0, {mode});")),
                want,
                "round({v}, 0, {mode})"
            );
        }
    }
    // The default mode is HALF_UP, and a value that is not a tie is unaffected
    // by any of them.
    assert_eq!(run("<?php echo round(2.5);"), "3");
    for mode in [
        "PHP_ROUND_HALF_UP",
        "PHP_ROUND_HALF_DOWN",
        "PHP_ROUND_HALF_EVEN",
        "PHP_ROUND_HALF_ODD",
    ] {
        assert_eq!(run(&format!("<?php echo round(2.4, 0, {mode});")), "2");
        assert_eq!(run(&format!("<?php echo round(2.6, 0, {mode});")), "3");
    }
    // The mode also reaches the PRE-ROUNDING step, which is the part a fix that
    // only patched the final rounding would miss.
    assert_eq!(run("<?php echo round(1.005, 2, PHP_ROUND_HALF_DOWN);"), "1");
    assert_eq!(
        run("<?php echo round(1.005, 2, PHP_ROUND_HALF_UP);"),
        "1.01"
    );
    assert_eq!(
        run("<?php echo round(5.045, 2, PHP_ROUND_HALF_EVEN);"),
        "5.04"
    );
}
