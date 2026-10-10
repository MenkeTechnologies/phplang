//! GMP arbitrary-precision integers (num-bigint). Operands and results are
//! decimal strings (no GMP object type), so calls chain: gmp_add(gmp_mul(...)...).

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn big_addition_beyond_i64() {
    let src = r#"<?php echo gmp_strval(gmp_add(
        "123456789012345678901234567890",
        "987654321098765432109876543210"));"#;
    assert_eq!(run(src), "1111111110111111111011111111100");
}

#[test]
fn pow_and_mul_chaining() {
    // 2^65 = 36893488147419103232 (well beyond i64).
    let src = r#"<?php echo gmp_strval(gmp_mul(gmp_pow("2", "64"), "2"));"#;
    assert_eq!(run(src), "36893488147419103232");
}

#[test]
fn factorial() {
    let src = r#"<?php echo gmp_strval(gmp_fact("30"));"#;
    assert_eq!(run(src), "265252859812191058636308480000000");
}

#[test]
fn gcd_lcm_cmp_mod() {
    let src = r#"<?php echo gmp_strval(gmp_gcd("48", "36")), "|",
        gmp_strval(gmp_lcm("4", "6")), "|",
        gmp_cmp("100", "99"), "|",
        gmp_strval(gmp_mod("17", "5"));"#;
    assert_eq!(run(src), "12|12|1|2");
}

#[test]
fn modular_exponentiation() {
    // 4^13 mod 497 = 445.
    let src = r#"<?php echo gmp_strval(gmp_powm("4", "13", "497"));"#;
    assert_eq!(run(src), "445");
}

#[test]
fn sign_neg_abs() {
    let src = r#"<?php echo gmp_sign("-5"), gmp_sign("0"), gmp_sign("5"), "|",
        gmp_strval(gmp_neg("7")), "|", gmp_strval(gmp_abs("-42"));"#;
    assert_eq!(run(src), "-101|-7|42");
}

#[test]
fn primality() {
    let src = r#"<?php echo gmp_prob_prime("97"), gmp_prob_prime("100"),
        gmp_prob_prime("7919");"#;
    // 97 prime (2), 100 composite (0), 7919 prime (2).
    assert_eq!(run(src), "202");
}

#[test]
fn sqrt_and_intval() {
    let src = r#"<?php echo gmp_strval(gmp_sqrt("144")), "|",
        gmp_strval(gmp_sqrt("145")), "|", gmp_intval("9999");"#;
    assert_eq!(run(src), "12|12|9999");
}

// ── reference-derived table: gmp_* results as the reference prints them ─────

/// `(expression, rendered result)`: arrays join with `,`, bools are `true`/`false`.
/// Every row was read off reference php 8.5.
const REFERENCE_ROWS: &[(&str, &str)] = &[
    ("gmp_div_q(-7, 2, GMP_ROUND_PLUSINF)", "-3"),
    ("gmp_div_q(-7, 2, GMP_ROUND_MINUSINF)", "-4"),
    ("gmp_div_q(7, 2, GMP_ROUND_PLUSINF)", "4"),
    ("gmp_div_r(-7, 2)", "-1"),
    ("gmp_div_r(-7, 2, GMP_ROUND_PLUSINF)", "-1"),
    ("gmp_div_r(7, -2, GMP_ROUND_MINUSINF)", "-1"),
    ("gmp_mod(-7, 2)", "1"),
    ("gmp_mod(7, -2)", "1"),
    ("gmp_div_qr(-7, 2)", "-3,-1"),
    ("gmp_div_qr(7, 2, GMP_ROUND_PLUSINF)", "4,-1"),
    ("gmp_div_qr(-7, -2, GMP_ROUND_PLUSINF)", "4,1"),
    ("gmp_div_qr(7, -2, GMP_ROUND_MINUSINF)", "-4,-1"),
    ("gmp_init(\"ff\", 16)", "255"),
    ("gmp_init(\"zz\", 36)", "1295"),
    ("gmp_init(\"0777\")", "511"),
    ("gmp_init(\"0b101\")", "5"),
    ("gmp_init(\"-0xff\")", "-255"),
    ("gmp_init(\"Zz\", 62)", "2231"),
    ("gmp_init(\" 12 \")", "12"),
    ("gmp_strval(255, -16)", "FF"),
    ("gmp_strval(3843, 62)", "zz"),
    ("gmp_strval(-255, 2)", "-11111111"),
    ("gmp_strval(255, 36)", "73"),
    ("gmp_gcdext(12, 18)", "6,-1,1"),
    ("gmp_gcdext(0, 0)", "0,0,0"),
    ("gmp_gcdext(-12, 18)", "6,1,1"),
    ("gmp_invert(3, 7)", "5"),
    ("gmp_invert(2, 4)", "false"),
    ("gmp_invert(3, -7)", "5"),
    ("gmp_sqrtrem(17)", "4,1"),
    ("gmp_rootrem(30, 3)", "3,3"),
    ("gmp_rootrem(-9, 3)", "-2,-1"),
    ("gmp_root(-27, 3)", "-3"),
    ("gmp_jacobi(2, 15)", "1"),
    ("gmp_kronecker(5, -8)", "-1"),
    ("gmp_kronecker(6, 4)", "0"),
    ("gmp_kronecker(-1, 0)", "1"),
    ("gmp_perfect_power(-8)", "true"),
    ("gmp_perfect_power(-4)", "false"),
    ("gmp_perfect_power(1024)", "true"),
    ("gmp_perfect_power(2)", "false"),
    ("gmp_binomial(10, 3)", "120"),
    ("gmp_binomial(-3, 2)", "6"),
    ("gmp_binomial(60, 30)", "118264581564861424"),
    ("gmp_nextprime(100)", "101"),
    ("gmp_nextprime(-100)", "2"),
    ("gmp_com(5)", "-6"),
    ("gmp_testbit(-8, 2)", "false"),
    ("gmp_testbit(-8, 3)", "true"),
    ("gmp_scan0(-8, 0)", "0"),
    ("gmp_scan1(-8, 0)", "3"),
    ("gmp_scan0(-1, 5)", "-1"),
    ("gmp_scan1(0, 5)", "-1"),
    ("gmp_popcount(255)", "8"),
    ("gmp_popcount(-1)", "-1"),
    ("gmp_hamdist(5, 3)", "2"),
    ("gmp_hamdist(-1, 3)", "-1"),
    (
        "gmp_intval(\"-99999999999999999999\")",
        "-7766279631452241919",
    ),
    (
        "gmp_intval(\"99999999999999999999\")",
        "7766279631452241919",
    ),
    (
        "gmp_intval(\"-9223372036854775808\")",
        "-9223372036854775807-1",
    ),
    ("gmp_intval(\"18446744073709551617\")", "1"),
    ("gmp_prob_prime(-3)", "2"),
    ("gmp_prob_prime(561)", "0"),
];

#[test]
fn results_match_reference() {
    for (expr, expected) in REFERENCE_ROWS {
        let src = format!(
            "<?php $r = {expr}; echo is_array($r) ? implode(',', $r) : (is_string($r) ? $r : var_export($r, true));"
        );
        assert_eq!(run(&src), *expected, "{expr}");
    }
}

#[test]
fn invalid_operands_and_arguments_throw_the_reference_errors() {
    let rows: &[(&str, &str)] = &[
        (r#"gmp_add("12x", 1)"#, "ValueError: gmp_add(): Argument #1 ($num1) is not an integer string"),
        (r#"gmp_cmp(1, "zz")"#, "ValueError: gmp_cmp(): Argument #2 ($num2) is not an integer string"),
        (r#"gmp_add(1, "+3")"#, "ValueError: gmp_add(): Argument #2 ($num2) is not an integer string"),
        (r#"gmp_init("")"#, "ValueError: gmp_init(): Argument #1 ($num) is not an integer string"),
        (r#"gmp_init("12", 1)"#, "ValueError: gmp_init(): Argument #2 ($base) must be 0 or between 2 and 62"),
        (r#"gmp_strval(5, 1)"#, "ValueError: gmp_strval(): Argument #2 ($base) must be between 2 and 62, or -2 and -36"),
        (r#"gmp_div_q(5, 0)"#, "DivisionByZeroError: gmp_div_q(): Argument #2 ($num2) Division by zero"),
        (r#"gmp_mod(5, 0)"#, "DivisionByZeroError: gmp_mod(): Argument #2 ($num2) Modulo by zero"),
        (r#"gmp_div_q(5, 2, 9)"#, "ValueError: gmp_div_q(): Argument #3 ($rounding_mode) must be one of GMP_ROUND_ZERO, GMP_ROUND_PLUSINF, or GMP_ROUND_MINUSINF"),
        (r#"gmp_powm(2, -1, 7)"#, "ValueError: gmp_powm(): Argument #2 ($exponent) must be greater than or equal to 0"),
        (r#"gmp_testbit(5, -1)"#, "ValueError: gmp_testbit(): Argument #2 ($index) must be between 0 and 2147483647 * 64"),
        (r#"gmp_sqrtrem(-1)"#, "ValueError: gmp_sqrtrem(): Argument #1 ($num) must be greater than or equal to 0"),
        (r#"gmp_rootrem(-8, 2)"#, "ValueError: gmp_rootrem(): Argument #2 ($nth) must be odd if argument #1 ($a) is negative"),
        (r#"gmp_binomial(5, -1)"#, "ValueError: gmp_binomial(): Argument #2 ($k) must be greater than or equal to 0"),
    ];
    for (expr, expected) in rows {
        let src = format!(
            "<?php try {{ {expr}; echo 'no error'; }} catch (Throwable $e) {{ echo get_class($e), ': ', $e->getMessage(); }}"
        );
        assert_eq!(run(&src), *expected, "{expr}");
    }
}
