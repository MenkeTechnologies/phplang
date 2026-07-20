//! `ctype_*` end-to-end tests: PHP source in, captured `echo` output out.
//! Every predicate uses a `? "y" : "n"` ternary because `echo` on a bool prints
//! "1"/"" which is harder to read. Values chosen to exercise the string path,
//! the "int in -128..=255 is a char code" path, the "other int is a decimal
//! string" path, the empty-string-is-false rule, and the non-ASCII path.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

/// Convenience: wrap a boolean-returning PHP expression in a y/n ternary.
fn yn(expr: &str) -> String {
    run(&format!("<?php echo ({expr}) ? \"y\" : \"n\";"))
}

#[test]
fn alpha_basic() {
    assert_eq!(yn(r#"ctype_alpha("Hello")"#), "y");
    assert_eq!(yn(r#"ctype_alpha("Hello2")"#), "n");
    assert_eq!(yn(r#"ctype_alpha("hello world")"#), "n"); // space fails
    assert_eq!(yn(r#"ctype_alpha("")"#), "n"); // empty is false
}

#[test]
fn digit_basic() {
    assert_eq!(yn(r#"ctype_digit("0123456789")"#), "y");
    assert_eq!(yn(r#"ctype_digit("12.3")"#), "n");
    assert_eq!(yn(r#"ctype_digit("-5")"#), "n"); // '-' is not a digit
    assert_eq!(yn(r#"ctype_digit("")"#), "n");
}

#[test]
fn alnum_basic() {
    assert_eq!(yn(r#"ctype_alnum("abc123")"#), "y");
    assert_eq!(yn(r#"ctype_alnum("abc 123")"#), "n");
    assert_eq!(yn(r#"ctype_alnum("foo_bar")"#), "n"); // underscore is punct
}

#[test]
fn upper_lower() {
    assert_eq!(yn(r#"ctype_upper("ABC")"#), "y");
    assert_eq!(yn(r#"ctype_upper("Abc")"#), "n");
    assert_eq!(yn(r#"ctype_lower("abc")"#), "y");
    assert_eq!(yn(r#"ctype_lower("aBc")"#), "n");
}

#[test]
fn space_class() {
    assert_eq!(yn(r#"ctype_space(" \t\n\r")"#), "y");
    // vertical tab (0x0B) and form feed (0x0C) are spaces in C locale. The
    // phplang lexer has no \v/\f escapes, so build the bytes with chr().
    assert_eq!(yn(r#"ctype_space(chr(11) . chr(12))"#), "y");
    assert_eq!(yn("ctype_space(11)"), "y"); // int char-code path, VT
    assert_eq!(yn(r#"ctype_space(" a ")"#), "n");
    assert_eq!(yn(r#"ctype_space("")"#), "n");
}

#[test]
fn punct_class() {
    assert_eq!(yn(r#"ctype_punct("!@#$%^&*()")"#), "y");
    assert_eq!(yn(r#"ctype_punct("abc!")"#), "n"); // letters are not punct
    assert_eq!(yn(r#"ctype_punct("1!")"#), "n"); // digits are not punct
    assert_eq!(yn(r#"ctype_punct("! ")"#), "n"); // space is not punct
}

#[test]
fn xdigit_class() {
    assert_eq!(yn(r#"ctype_xdigit("00ffAB")"#), "y");
    assert_eq!(yn(r#"ctype_xdigit("0x1A")"#), "n"); // 'x' is not a hex digit
    assert_eq!(yn(r#"ctype_xdigit("g")"#), "n");
}

#[test]
fn cntrl_class() {
    assert_eq!(yn(r#"ctype_cntrl("\n\r\t")"#), "y");
    // NUL (0x00), unit separator (0x1F), DEL (0x7F): built with chr().
    assert_eq!(yn(r#"ctype_cntrl(chr(0) . chr(31) . chr(127))"#), "y");
    assert_eq!(yn("ctype_cntrl(127)"), "y"); // int char-code path, DEL
    assert_eq!(yn(r#"ctype_cntrl("a")"#), "n");
    assert_eq!(yn(r#"ctype_cntrl(" ")"#), "n"); // space is printable, not cntrl
}

#[test]
fn graph_and_print() {
    // graph excludes space; print includes it.
    assert_eq!(yn(r#"ctype_graph("arf12!")"#), "y");
    assert_eq!(yn(r#"ctype_graph("arf 12!")"#), "n"); // space fails graph
    assert_eq!(yn(r#"ctype_print("arf 12!")"#), "y"); // space passes print
    assert_eq!(yn(r#"ctype_print("bad\n")"#), "n"); // newline is not printable
}

#[test]
fn int_in_char_range_is_ascii_code() {
    // 65 == 'A'
    assert_eq!(yn("ctype_alpha(65)"), "y");
    // 0x20 == ' ' -> print yes, graph no, space yes
    assert_eq!(yn("ctype_print(32)"), "y");
    assert_eq!(yn("ctype_graph(32)"), "n");
    assert_eq!(yn("ctype_space(32)"), "y");
    // 48 == '0'
    assert_eq!(yn("ctype_digit(48)"), "y");
    // negative gets 256 added: -1 -> 255 (non-ASCII, no class)
    assert_eq!(yn("ctype_print(-1)"), "n");
}

#[test]
fn int_out_of_range_is_decimal_string() {
    // 1000 -> "1000" -> all digits
    assert_eq!(yn("ctype_digit(1000)"), "y");
    assert_eq!(yn("ctype_alnum(1000)"), "y");
    // 256 is out of the -128..=255 range -> "256" (still all digits)
    assert_eq!(yn("ctype_digit(256)"), "y");
    // negative out of range -> "-200" contains '-' which is not a digit
    assert_eq!(yn("ctype_digit(-200)"), "n");
    assert_eq!(yn("ctype_alpha(1000)"), "n");
}

#[test]
fn non_string_non_int_is_false() {
    assert_eq!(yn("ctype_alpha(null)"), "n");
    assert_eq!(yn("ctype_alpha(true)"), "n");
    assert_eq!(yn("ctype_alpha([])"), "n");
}
