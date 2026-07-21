//! High-value tests for the `encoding` stdlib category. Every expected value is
//! cross-checked against reference `php` 8.5 but hard-coded here so the suite
//! passes in headless CI without a `php` on PATH.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).expect("eval failed")
}

#[test]
fn base64_encode_basic() {
    assert_eq!(run(r#"<?php echo base64_encode("Hello");"#), "SGVsbG8=");
    assert_eq!(run(r#"<?php echo base64_encode("");"#), "");
    // padding variants
    assert_eq!(run(r#"<?php echo base64_encode("a");"#), "YQ==");
    assert_eq!(run(r#"<?php echo base64_encode("ab");"#), "YWI=");
    assert_eq!(run(r#"<?php echo base64_encode("abc");"#), "YWJj");
}

#[test]
fn base64_decode_basic_and_roundtrip() {
    assert_eq!(run(r#"<?php echo base64_decode("SGVsbG8=");"#), "Hello");
    assert_eq!(
        run(r#"<?php echo base64_decode(base64_encode("round trip!"));"#),
        "round trip!"
    );
    // missing padding is accepted (non-strict)
    assert_eq!(run(r#"<?php echo base64_decode("SGVsbG8");"#), "Hello");
}

#[test]
fn base64_decode_nonstrict_skips_junk() {
    // non-strict silently drops characters outside the alphabet
    assert_eq!(run(r#"<?php echo base64_decode("!!SGVsbG8=??");"#), "Hello");
    assert_eq!(run(r#"<?php echo base64_decode("SG Vs\nbG8=");"#), "Hello");
}

#[test]
fn base64_decode_strict() {
    // strict allows whitespace but rejects other invalid characters -> false
    assert_eq!(
        run(r#"<?php var_dump(base64_decode("SG Vs\nbG8=", true));"#),
        "string(5) \"Hello\"\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(base64_decode("!!SGVsbG8=??", true));"#),
        "bool(false)\n"
    );
    // data after padding is rejected in strict mode
    assert_eq!(
        run(r#"<?php var_dump(base64_decode("SGVsbG8=extra", true));"#),
        "bool(false)\n"
    );
    // no padding is fine even in strict mode
    assert_eq!(
        run(r#"<?php var_dump(base64_decode("SGVsbG8", true));"#),
        "string(5) \"Hello\"\n"
    );
    // lone padding char is malformed
    assert_eq!(
        run(r#"<?php var_dump(base64_decode("=", true));"#),
        "bool(false)\n"
    );
}

#[test]
fn hex2bin_basic() {
    assert_eq!(run(r#"<?php echo hex2bin("48656c6c6f");"#), "Hello");
    assert_eq!(run(r#"<?php echo hex2bin("");"#), "");
    // bin2hex is core; verify the round trip through it
    assert_eq!(run(r#"<?php echo hex2bin(bin2hex("Test"));"#), "Test");
}

#[test]
fn hex2bin_invalid_returns_false() {
    // odd length -> false
    assert_eq!(run(r#"<?php var_dump(hex2bin("abc"));"#), "bool(false)\n");
    // non-hex character -> false
    assert_eq!(run(r#"<?php var_dump(hex2bin("xy"));"#), "bool(false)\n");
}

#[test]
fn quoted_printable_encode_basic() {
    assert_eq!(
        run(r#"<?php echo quoted_printable_encode("a=b");"#),
        "a=3Db"
    );
    // tab (control) is encoded; plain ASCII passes through
    assert_eq!(
        run(r#"<?php echo quoted_printable_encode("x\ty");"#),
        "x=09y"
    );
}

#[test]
fn quoted_printable_encode_soft_wrap() {
    // 80 'a's wrap after 75 with a "=\r\n" soft break, then the remaining 5
    let out = run(r#"<?php echo quoted_printable_encode(str_repeat("a", 80));"#);
    let expected = format!("{}=\r\n{}", "a".repeat(75), "a".repeat(5));
    assert_eq!(out, expected);
}

#[test]
fn quoted_printable_decode_basic() {
    assert_eq!(
        run(r#"<?php echo quoted_printable_decode("a=3Db");"#),
        "a=b"
    );
    // soft line break is removed
    assert_eq!(
        run(r#"<?php echo quoted_printable_decode("line1=\r\nline2");"#),
        "line1line2"
    );
    // malformed '=' is kept verbatim
    assert_eq!(
        run(r#"<?php echo quoted_printable_decode("a=zz");"#),
        "a=zz"
    );
    // round trip on ASCII
    assert_eq!(
        run(r#"<?php echo quoted_printable_decode(quoted_printable_encode("plain=text!"));"#),
        "plain=text!"
    );
}

#[test]
fn convert_uuencode_matches_php() {
    // exact byte layout cross-checked against php: convert_uuencode("Cat")
    assert_eq!(
        run(r#"<?php echo bin2hex(convert_uuencode("Cat"));"#),
        "23305625540a600a"
    );
    // empty input yields just the backtick trailer line
    assert_eq!(run(r#"<?php echo bin2hex(convert_uuencode(""));"#), "600a");
}

#[test]
fn convert_uu_roundtrip() {
    assert_eq!(
        run(r#"<?php echo convert_uudecode(convert_uuencode("Hello"));"#),
        "Hello"
    );
    assert_eq!(
        run(r#"<?php echo convert_uudecode(convert_uuencode("Hello World, this is a test!"));"#),
        "Hello World, this is a test!"
    );
    // multi-line payload (> 45 bytes) exercises line-length handling
    assert_eq!(
        run(r#"<?php echo convert_uudecode(convert_uuencode(str_repeat("z", 90)));"#),
        "z".repeat(90)
    );
}

#[test]
fn quoted_printable_decode_trailing_bare_equals_dropped() {
    // PHP drops a trailing bare '=' that is the final byte of input.
    assert_eq!(run(r#"<?php echo quoted_printable_decode("a=");"#), "a");
    assert_eq!(run(r#"<?php echo quoted_printable_decode("=");"#), "");
    assert_eq!(run(r#"<?php echo quoted_printable_decode("=41=");"#), "A");
}

#[test]
fn convert_uudecode_invalid_returns_false() {
    // empty input -> false
    assert_eq!(
        run(r#"<?php var_dump(convert_uudecode(""));"#),
        "bool(false)\n"
    );
    // a single length char declaring a 45-byte line with no data overruns the
    // buffer -> malformed -> false ('M' == chr(45 + 0x20))
    assert_eq!(
        run(r#"<?php var_dump(convert_uudecode("M"));"#),
        "bool(false)\n"
    );
    // valid data still decodes (guard against over-rejection)
    assert_eq!(
        run(r#"<?php echo convert_uudecode(convert_uuencode("ok"));"#),
        "ok"
    );
}

#[test]
fn utf8_shims_ascii_identity() {
    assert_eq!(
        run(r#"<?php echo utf8_encode("plain ascii");"#),
        "plain ascii"
    );
    assert_eq!(
        run(r#"<?php echo utf8_decode("plain ascii");"#),
        "plain ascii"
    );
}
