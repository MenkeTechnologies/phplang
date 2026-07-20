//! Integer literal forms: hexadecimal (`0x`), octal (`0o` and leading-`0`),
//! binary (`0b`), and underscore digit separators.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn hexadecimal_literals() {
    assert_eq!(run("<?php echo 0xFF;"), "255");
    assert_eq!(run("<?php echo 0x1a;"), "26");
    assert_eq!(run("<?php echo 0XdeadBEEF;"), "3735928559");
}

#[test]
fn octal_literals() {
    assert_eq!(run("<?php echo 0755;"), "493"); // leading-zero octal
    assert_eq!(run("<?php echo 0o17;"), "15"); // PHP 8.1 explicit octal
    assert_eq!(run("<?php echo 0O777;"), "511");
    assert_eq!(run("<?php echo 0;"), "0"); // a lone zero stays decimal zero
}

#[test]
fn binary_literals() {
    assert_eq!(run("<?php echo 0b101;"), "5");
    assert_eq!(run("<?php echo 0B1111;"), "15");
}

#[test]
fn underscore_separators() {
    assert_eq!(run("<?php echo 1_000_000;"), "1000000");
    assert_eq!(run("<?php echo 0xFF_FF;"), "65535");
    assert_eq!(run("<?php echo 0b1010_0101;"), "165");
}

#[test]
fn literals_in_expressions() {
    assert_eq!(run("<?php echo 0x10 + 0o10 + 0b10;"), "26"); // 16 + 8 + 2
    assert_eq!(run("<?php echo 0755 & 0644;"), "420"); // octal bitwise-and
}
