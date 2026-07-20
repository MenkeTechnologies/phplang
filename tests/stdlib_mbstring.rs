//! Tests for the `stdlib::mbstring` category. Every expected value mirrors the
//! output of the reference PHP 8 function of the same name. These run headless
//! (no `php` on PATH required).

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn str_split_codepoints() {
    // "héllo" is 5 codepoints; the 2-byte é must not be split mid-char.
    assert_eq!(run(r#"<?php $a = mb_str_split("héllo"); echo count($a), "|", $a[1];"#), "5|é");
    assert_eq!(
        run(r#"<?php $a = mb_str_split("héllo", 2); echo count($a), "|", $a[0], "|", $a[2];"#),
        "3|hé|o"
    );
    // Empty string yields an empty array (PHP 8).
    assert_eq!(run(r#"<?php echo count(mb_str_split(""));"#), "0");
    // length < 1 is a ValueError.
    assert!(eval_capture(r#"<?php mb_str_split("abc", 0);"#).is_err());
}

#[test]
fn convert_case_modes() {
    // Integer modes: 0 upper, 1 lower, 2 title.
    assert_eq!(run(r#"<?php echo mb_convert_case("héllo wörld", 0);"#), "HÉLLO WÖRLD");
    assert_eq!(run(r#"<?php echo mb_convert_case("HÉLLO", 1);"#), "héllo");
    assert_eq!(run(r#"<?php echo mb_convert_case("hello world", 2);"#), "Hello World");
    // Constant names reach the fn as strings (no constant table); both work.
    assert_eq!(run(r#"<?php echo mb_convert_case("abc", MB_CASE_UPPER);"#), "ABC");
    assert_eq!(run(r#"<?php echo mb_convert_case("ABC", MB_CASE_LOWER);"#), "abc");
    assert_eq!(run(r#"<?php echo mb_convert_case("a b", MB_CASE_TITLE);"#), "A B");
    // Non-letter resets the word run: apostrophe boundary uppercases the "s".
    assert_eq!(run(r#"<?php echo mb_convert_case("who's who", MB_CASE_TITLE);"#), "Who'S Who");
}

#[test]
fn positions_codepoint_aware() {
    // é is one codepoint here, so "l" is at codepoint index 3 (byte index 4).
    assert_eq!(run(r#"<?php echo mb_strpos("héllo", "l");"#), "2");
    assert_eq!(run(r#"<?php echo mb_strpos("héllo", "l", 3);"#), "3");
    assert_eq!(run(r#"<?php var_dump(mb_strpos("héllo", "z"));"#), "bool(false)\n");
    // Case-insensitive.
    assert_eq!(run(r#"<?php echo mb_stripos("HÉLLO", "é");"#), "1");
    // Last occurrence.
    assert_eq!(run(r#"<?php echo mb_strrpos("héllo", "l");"#), "3");
    assert_eq!(run(r#"<?php echo mb_strripos("aAaA", "a");"#), "3");
}

#[test]
fn substr_count_and_pad() {
    assert_eq!(run(r#"<?php echo mb_substr_count("héllo héllo", "é");"#), "2");
    assert_eq!(run(r#"<?php echo mb_substr_count("aaa", "aa");"#), "1");
    assert!(eval_capture(r#"<?php mb_substr_count("abc", "");"#).is_err());
    // Codepoint-counted padding.
    assert_eq!(run(r#"<?php echo mb_str_pad("é", 3, "-");"#), "é--");
    assert_eq!(run(r#"<?php echo mb_str_pad("x", 4, "ab", STR_PAD_LEFT);"#), "abax");
    assert_eq!(run(r#"<?php echo mb_str_pad("x", 5, "-", STR_PAD_BOTH);"#), "--x--");
    // Already long enough: unchanged.
    assert_eq!(run(r#"<?php echo mb_str_pad("hello", 3);"#), "hello");
}

#[test]
fn ord_and_chr() {
    assert_eq!(run(r#"<?php echo mb_ord("A");"#), "65");
    assert_eq!(run(r#"<?php echo mb_ord("é");"#), "233");
    assert_eq!(run(r#"<?php echo mb_ord("€");"#), "8364");
    assert_eq!(run(r#"<?php var_dump(mb_ord(""));"#), "bool(false)\n");
    assert_eq!(run(r#"<?php echo mb_chr(65);"#), "A");
    assert_eq!(run(r#"<?php echo mb_chr(233);"#), "é");
    assert_eq!(run(r#"<?php echo mb_chr(8364);"#), "€");
    // ord/chr round-trip.
    assert_eq!(run(r#"<?php echo mb_chr(mb_ord("λ"));"#), "λ");
    // Invalid codepoint -> false.
    assert_eq!(run(r#"<?php var_dump(mb_chr(1114112));"#), "bool(false)\n");
}

#[test]
fn strwidth_east_asian() {
    // ASCII: one column each.
    assert_eq!(run(r#"<?php echo mb_strwidth("hello");"#), "5");
    // CJK ideographs are 2 columns each; "日本" -> 4.
    assert_eq!(run(r#"<?php echo mb_strwidth("日本");"#), "4");
    // Mixed: "aあb" -> 1 + 2 + 1.
    assert_eq!(run(r#"<?php echo mb_strwidth("aあb");"#), "4");
}

#[test]
fn convert_and_detect_encoding() {
    // Non-ASCII collapses to '?' when converting to ASCII.
    assert_eq!(run(r#"<?php echo mb_convert_encoding("héllo", "ASCII");"#), "h?llo");
    // Pure ASCII survives any conversion.
    assert_eq!(run(r#"<?php echo mb_convert_encoding("hello", "UTF-8");"#), "hello");
    // é (U+00E9 <= 0xFF) survives ISO-8859-1.
    assert_eq!(run(r#"<?php echo mb_convert_encoding("héllo", "ISO-8859-1");"#), "héllo");
    // Detection: pure ASCII vs UTF-8.
    assert_eq!(run(r#"<?php echo mb_detect_encoding("hello");"#), "ASCII");
    assert_eq!(run(r#"<?php echo mb_detect_encoding("héllo");"#), "UTF-8");
    // Candidate list honored in order.
    assert_eq!(run(r#"<?php echo mb_detect_encoding("héllo", ["ASCII", "UTF-8"]);"#), "UTF-8");
    assert_eq!(run(r#"<?php echo mb_detect_encoding("hello", ["UTF-8", "ASCII"]);"#), "UTF-8");
}

#[test]
fn check_and_internal_encoding() {
    assert_eq!(run(r#"<?php var_dump(mb_check_encoding("hello", "ASCII"));"#), "bool(true)\n");
    assert_eq!(run(r#"<?php var_dump(mb_check_encoding("héllo", "ASCII"));"#), "bool(false)\n");
    assert_eq!(run(r#"<?php var_dump(mb_check_encoding("héllo", "UTF-8"));"#), "bool(true)\n");
    // Getter returns the default; setter returns true and updates the getter.
    assert_eq!(run(r#"<?php echo mb_internal_encoding();"#), "UTF-8");
    assert_eq!(
        run(r#"<?php var_dump(mb_internal_encoding("UTF-8")); echo mb_internal_encoding();"#),
        "bool(true)\nUTF-8"
    );
}
