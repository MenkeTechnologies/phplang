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

/// `%f`, `%g` and `%G` print the `LC_NUMERIC` decimal point; `%F`, `%e` and `%h`
/// never do. Run as a subprocess, since `setlocale` changes the whole process,
/// and skipped on a system with no German locale installed.
#[test]
fn sprintf_float_conversions_follow_lc_numeric() {
    let src = r#"if (setlocale(LC_NUMERIC, "de_DE", "de_DE.UTF-8", "de_DE.utf8") === false) { echo "skip"; return; }
printf("%f|%F|%e|%g|%h|%.3f|%10.2f", 1.5, 1.5, 1.5, 1.5, 1.5, -2.25, 3.14159);"#;
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_php"))
        .args(["-r", src])
        .env("LC_ALL", "C")
        .output()
        .expect("spawn php");
    let out = String::from_utf8_lossy(&out.stdout);
    if out == "skip" {
        return;
    }
    assert_eq!(
        out,
        "1,500000|1.500000|1.500000e+0|1,5|1.5|-2,250|      3,14"
    );
}

#[test]
fn sprintf_non_finite_and_negative_zero() {
    // A non-finite value is printed bare: no width, no padding, no sign flag.
    assert_eq!(
        run(r#"<?php echo sprintf("[%10.1f][%-6e][%+g][%05F]", INF, -INF, NAN, INF);"#),
        "[INF][-INF][NaN][INF]"
    );
    // -0.0 is unsigned under %f/%e and keeps its sign under %g; + applies to e/g.
    assert_eq!(
        run(r#"<?php echo sprintf("%f|%e|%g|%+f|%+.1e|%+g", -0.0, -0.0, -0.0, -0.0, 1.5, 2.5);"#),
        "0.000000|0.000000e+0|-0|+0.000000|+1.5e+0|+2.5"
    );
}

#[test]
fn a_left_justified_field_pads_with_its_padding_character() {
    // php -r 'printf(...)' — `-` keeps the padding character, except that d/i/u
    // trade `0` for a space; `%c` takes no width at all.
    let src = r#"<?php printf("[%-05x][%-05u][%-08.1e][%-05s][%-05b][%-05c][%-'#5d][%-06.1F][%-6g][%-05d][%-'x6s][%5c]", 42, 42, 1.5, "ab", 5, 65, 3, 1.5, 0.5, 42, "ab", 66);"#;
    assert_eq!(
        run(src),
        "[2a000][42   ][1.5e+000][ab000][10100][A][3####][1.5000][0.5   ][42   ][abxxxx][B]"
    );
}
