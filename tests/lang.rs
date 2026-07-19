//! Language-construct tests for the features exercised by the Drupal byte-parity
//! harness (`src/bin/parity_drupal.rs`): `isset`/`empty`, string-offset access,
//! and UTF-8 preservation in single-quoted strings.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn isset_on_variables() {
    assert_eq!(run(r#"<?php $x = 5; var_dump(isset($x));"#), "bool(true)\n");
    assert_eq!(run(r#"<?php var_dump(isset($undefined));"#), "bool(false)\n");
    assert_eq!(run(r#"<?php $x = null; var_dump(isset($x));"#), "bool(false)\n");
    // Multiple operands: true only if all are set.
    assert_eq!(run(r#"<?php $a = 1; $b = 2; var_dump(isset($a, $b));"#), "bool(true)\n");
    assert_eq!(run(r#"<?php $a = 1; var_dump(isset($a, $missing));"#), "bool(false)\n");
}

#[test]
fn isset_on_array_and_string_offsets() {
    assert_eq!(run(r#"<?php $a = ["k" => 1]; var_dump(isset($a["k"]));"#), "bool(true)\n");
    assert_eq!(run(r#"<?php $a = ["k" => 1]; var_dump(isset($a["nope"]));"#), "bool(false)\n");
    assert_eq!(run(r#"<?php $s = "hi"; var_dump(isset($s[0]));"#), "bool(true)\n");
    assert_eq!(run(r#"<?php $s = "hi"; var_dump(isset($s[9]));"#), "bool(false)\n");
}

#[test]
fn empty_matches_php_falsiness() {
    for (v, expect) in [
        (r#""""#, true),
        (r#""0""#, true),
        ("0", true),
        ("[]", true),
        ("null", true),
        (r#""a""#, false),
        ("5", false),
        (r#""0.0""#, false), // the string "0.0" is truthy in PHP
    ] {
        let got = run(&format!("<?php var_dump(empty({v}));"));
        assert_eq!(got, format!("bool({expect})\n"), "empty({v})");
    }
    assert_eq!(run(r#"<?php var_dump(empty($never_set));"#), "bool(true)\n");
}

#[test]
fn string_offset_access() {
    assert_eq!(run(r#"<?php $s = "abc"; echo $s[0], $s[2];"#), "ac");
    assert_eq!(run(r#"<?php echo "hello"[1];"#), "e");
}

#[test]
fn utf8_preserved_in_single_quotes() {
    // A raw byte→char cast used to mojibake multibyte text; single- and
    // double-quoted strings must both round-trip UTF-8 verbatim.
    assert_eq!(run(r#"<?php echo 'café';"#), "café");
    assert_eq!(run(r#"<?php echo 'Ünïcödé';"#), "Ünïcödé");
    assert_eq!(run(r#"<?php echo "Ünïcödé";"#), "Ünïcödé");
    // Byte length (strlen counts bytes, as in PHP): "é" is 2 bytes.
    assert_eq!(run(r#"<?php echo strlen('café');"#), "5");
}
