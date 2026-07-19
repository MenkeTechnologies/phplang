//! Number-formatting and `sprintf` regression tests — every expected value here
//! was confirmed against reference PHP 8 by the parity fuzzer (modes
//! `sprintf_rich`, `numedge`, `floatfmt`, `stredge`).

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn sprintf_radix_and_char() {
    // Negative ints render as 64-bit two's complement in x/X/o/b, like PHP.
    assert_eq!(run(r#"<?php echo sprintf("%X", -1);"#), "FFFFFFFFFFFFFFFF");
    assert_eq!(run(r#"<?php echo sprintf("%x", 255);"#), "ff");
    assert_eq!(run(r#"<?php echo sprintf("%o", 8);"#), "10");
    assert_eq!(run(r#"<?php echo sprintf("%b", 5);"#), "101");
    assert_eq!(run(r#"<?php echo sprintf("%c", 65);"#), "A");
}

#[test]
fn sprintf_width_flags_precision() {
    assert_eq!(run(r#"<?php echo sprintf("%5d", -2);"#), "   -2");
    assert_eq!(run(r#"<?php echo sprintf("%-5d", 3);"#), "3    ");
    assert_eq!(run(r#"<?php echo sprintf("%05d", 7);"#), "00007");
    assert_eq!(run(r#"<?php echo sprintf("%+d", 5);"#), "+5");
    assert_eq!(run(r#"<?php echo sprintf("%8.3f", -1.5);"#), "  -1.500");
    assert_eq!(run(r#"<?php echo sprintf("%e", 1000.0);"#), "1.000000e+3");
    assert_eq!(run(r#"<?php echo sprintf("%g", 0.5);"#), "0.5");
}

#[test]
fn sprintf_positional_args() {
    // The format must be single-quoted: in a double-quoted string `$s` would
    // interpolate a variable (as it does in real PHP), not stay literal.
    assert_eq!(run(r#"<?php echo sprintf('%2$s-%1$s', "a", "b");"#), "b-a");
}

#[test]
fn float_scientific_notation() {
    // Large/small magnitudes switch to PHP's precision-14 scientific form.
    assert_eq!(run(r#"<?php echo 1e100;"#), "1.0E+100");
    assert_eq!(run(r#"<?php echo 1.5e-10;"#), "1.5E-10");
    assert_eq!(run(r#"<?php echo 100000000000000.0;"#), "1.0E+14");
    assert_eq!(
        run(r#"<?php echo 9223372036854775807 * 2;"#),
        "1.844674407371E+19"
    );
}

#[test]
fn float_fixed_notation() {
    assert_eq!(run(r#"<?php echo 0.1 + 0.2;"#), "0.3");
    assert_eq!(run(r#"<?php echo 10 / 3;"#), "3.3333333333333");
    assert_eq!(run(r#"<?php echo 2.0;"#), "2");
    assert_eq!(run(r#"<?php echo -0.0;"#), "-0");
}

#[test]
fn wordwrap_cut_and_wrap() {
    assert_eq!(
        run(r#"<?php echo wordwrap("aaa bbb ccc", 3, "/", true);"#),
        "aaa/bbb/ccc"
    );
    assert_eq!(
        run(r#"<?php echo wordwrap("The quick brown fox", 10);"#),
        "The quick\nbrown fox"
    );
}
