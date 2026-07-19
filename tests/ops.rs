//! Operator tests for bitwise (`& | ^ << >> ~`), spaceship (`<=>`), null-coalesce
//! assignment (`??=`), and negative string offsets — all confirmed against
//! reference PHP 8 by the parity fuzzer (modes bitwise/spaceship/stroffset/
//! coalesce).

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn bitwise_operators() {
    assert_eq!(run(r#"<?php echo 5 & 3;"#), "1");
    assert_eq!(run(r#"<?php echo 5 | 2;"#), "7");
    assert_eq!(run(r#"<?php echo 6 ^ 3;"#), "5");
    assert_eq!(run(r#"<?php echo 1 << 4;"#), "16");
    assert_eq!(run(r#"<?php echo 255 >> 2;"#), "63");
    assert_eq!(run(r#"<?php echo ~5;"#), "-6");
}

#[test]
fn bitwise_precedence_and_compound() {
    // `+` binds tighter than `<<`, which binds tighter than `^`/`&`/`|`.
    assert_eq!(run(r#"<?php echo 2 + 3 & 4;"#), "4"); // (2+3) & 4
    assert_eq!(run(r#"<?php echo 1 | 2 ^ 3;"#), "1"); // 1 | (2^3)
    assert_eq!(run(r#"<?php echo 8 >> 1 + 1;"#), "2"); // 8 >> (1+1)
    assert_eq!(
        run(r#"<?php $n = 12; $n &= 10; $n |= 1; $n <<= 2; echo $n;"#),
        "36"
    );
}

#[test]
fn spaceship_operator() {
    assert_eq!(run(r#"<?php echo 3 <=> 7;"#), "-1");
    assert_eq!(run(r#"<?php echo 5 <=> 5;"#), "0");
    assert_eq!(run(r#"<?php echo 9 <=> 2;"#), "1");
    assert_eq!(run(r#"<?php echo "a" <=> "b";"#), "-1");
}

#[test]
fn null_coalesce_assignment() {
    assert_eq!(run(r#"<?php $x = null; $x ??= 5; echo $x;"#), "5");
    assert_eq!(run(r#"<?php $y = 10; $y ??= 99; echo $y;"#), "10");
    assert_eq!(
        run(r#"<?php $a = ['k' => 1]; $a['m'] ??= 8; echo $a['m'], $a['k'];"#),
        "81"
    );
}

#[test]
fn negative_string_offsets() {
    assert_eq!(run(r#"<?php echo "abc"[-1];"#), "c");
    assert_eq!(run(r#"<?php $s = "hello"; echo $s[-2];"#), "l");
    assert_eq!(run(r#"<?php $s = "hi"; echo $s[0], $s[-1];"#), "hi");
}
