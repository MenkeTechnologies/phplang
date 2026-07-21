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
