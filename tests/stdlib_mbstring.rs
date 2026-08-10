//! Tests for the `stdlib::mbstring` category. Expected values mirror the output
//! of the reference PHP 8 function of the same name, and run headless (no `php`
//! on PATH required).
//!
//! THREE do not, each re-verified against `php 8.5.9` and recorded rather than
//! implied:
//!
//!   * `mb_ord("")` answers `false` here; the reference throws
//!     `ValueError: mb_ord(): Argument #1 ($string) must not be empty`.
//!   * `mb_convert_encoding("héllo", "ISO-8859-1")` returns the input unchanged
//!     here; the reference transcodes, producing a byte this engine's UTF-8
//!     string model cannot hold.
//!   * `mb_split("(", "abc")` returns `false` without the reference's
//!     `Warning: mb_split(): mbregex compile err: end pattern with unmatched
//!     parenthesis`.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn str_split_codepoints() {
    // "héllo" is 5 codepoints; the 2-byte é must not be split mid-char.
    assert_eq!(
        run(r#"<?php $a = mb_str_split("héllo"); echo count($a), "|", $a[1];"#),
        "5|é"
    );
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
    assert_eq!(
        run(r#"<?php echo mb_convert_case("héllo wörld", 0);"#),
        "HÉLLO WÖRLD"
    );
    assert_eq!(run(r#"<?php echo mb_convert_case("HÉLLO", 1);"#), "héllo");
    assert_eq!(
        run(r#"<?php echo mb_convert_case("hello world", 2);"#),
        "Hello World"
    );
    // Constant names reach the fn as strings (no constant table); both work.
    assert_eq!(
        run(r#"<?php echo mb_convert_case("abc", MB_CASE_UPPER);"#),
        "ABC"
    );
    assert_eq!(
        run(r#"<?php echo mb_convert_case("ABC", MB_CASE_LOWER);"#),
        "abc"
    );
    assert_eq!(
        run(r#"<?php echo mb_convert_case("a b", MB_CASE_TITLE);"#),
        "A B"
    );
    // A CASE-IGNORABLE character is transparent to the word run, so the letter
    // after an apostrophe continues the word it was in.
    //
    // The old expectation here was "Who'S Who", which no PHP has ever produced;
    // it was written from the implementation rather than captured from a run.
    //
    //   $ php -r 'echo mb_convert_case("who\'s who", MB_CASE_TITLE);'
    //   Who's Who
    assert_eq!(
        run(r#"<?php echo mb_convert_case("who's who", MB_CASE_TITLE);"#),
        "Who's Who"
    );
    // The distinction is real and narrow: `.` and `:` are case-ignorable and `,`
    // and `~` are not, so a test using only letters and spaces cannot see it.
    //
    //   $ php -r 'echo mb_convert_case("x.y x,y x:y a~b", MB_CASE_TITLE);'
    //   X.y X,Y X:y A~B
    assert_eq!(
        run(r#"<?php echo mb_convert_case("x.y x,y x:y a~b", MB_CASE_TITLE);"#),
        "X.y X,Y X:y A~B"
    );
}

#[test]
fn positions_codepoint_aware() {
    // é is one codepoint here, so "l" is at codepoint index 3 (byte index 4).
    assert_eq!(run(r#"<?php echo mb_strpos("héllo", "l");"#), "2");
    assert_eq!(run(r#"<?php echo mb_strpos("héllo", "l", 3);"#), "3");
    assert_eq!(
        run(r#"<?php var_dump(mb_strpos("héllo", "z"));"#),
        "bool(false)\n"
    );
    // Case-insensitive.
    assert_eq!(run(r#"<?php echo mb_stripos("HÉLLO", "é");"#), "1");
    // Last occurrence.
    assert_eq!(run(r#"<?php echo mb_strrpos("héllo", "l");"#), "3");
    assert_eq!(run(r#"<?php echo mb_strripos("aAaA", "a");"#), "3");
}

#[test]
fn substr_count_and_pad() {
    assert_eq!(
        run(r#"<?php echo mb_substr_count("héllo héllo", "é");"#),
        "2"
    );
    assert_eq!(run(r#"<?php echo mb_substr_count("aaa", "aa");"#), "1");
    assert!(eval_capture(r#"<?php mb_substr_count("abc", "");"#).is_err());
    // Codepoint-counted padding.
    assert_eq!(run(r#"<?php echo mb_str_pad("é", 3, "-");"#), "é--");
    assert_eq!(
        run(r#"<?php echo mb_str_pad("x", 4, "ab", STR_PAD_LEFT);"#),
        "abax"
    );
    assert_eq!(
        run(r#"<?php echo mb_str_pad("x", 5, "-", STR_PAD_BOTH);"#),
        "--x--"
    );
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
    assert_eq!(
        run(r#"<?php echo mb_convert_encoding("héllo", "ASCII");"#),
        "h?llo"
    );
    // Pure ASCII survives any conversion.
    assert_eq!(
        run(r#"<?php echo mb_convert_encoding("hello", "UTF-8");"#),
        "hello"
    );
    // é (U+00E9 <= 0xFF) survives ISO-8859-1.
    assert_eq!(
        run(r#"<?php echo mb_convert_encoding("héllo", "ISO-8859-1");"#),
        "héllo"
    );
    // Detection: pure ASCII vs UTF-8.
    assert_eq!(run(r#"<?php echo mb_detect_encoding("hello");"#), "ASCII");
    assert_eq!(run(r#"<?php echo mb_detect_encoding("héllo");"#), "UTF-8");
    // Candidate list honored in order.
    assert_eq!(
        run(r#"<?php echo mb_detect_encoding("héllo", ["ASCII", "UTF-8"]);"#),
        "UTF-8"
    );
    assert_eq!(
        run(r#"<?php echo mb_detect_encoding("hello", ["UTF-8", "ASCII"]);"#),
        "UTF-8"
    );
}

#[test]
fn strpos_offset_out_of_range_is_valueerror() {
    // PHP 8: an offset outside [-len, len] raises a ValueError (not `false`).
    assert!(eval_capture(r#"<?php mb_strpos("abc", "b", 10);"#).is_err());
    assert!(eval_capture(r#"<?php mb_strpos("abc", "a", -4);"#).is_err());
    assert!(eval_capture(r#"<?php mb_strrpos("abc", "a", 10);"#).is_err());
    assert!(eval_capture(r#"<?php mb_strrpos("abc", "a", -10);"#).is_err());
    // Boundary offsets (== len, == -len) are valid, not errors.
    assert_eq!(run(r#"<?php echo mb_strpos("abc", "", 3);"#), "3");
    assert_eq!(run(r#"<?php echo mb_strpos("abc", "a", -3);"#), "0");
    assert_eq!(
        run(r#"<?php var_dump(mb_strpos("abc", "c", 3));"#),
        "bool(false)\n"
    );
}

#[test]
fn stripos_position_preserving_case_fold() {
    // U+0130 'İ' full-lowercases to two codepoints ('i' + combining dot); a naive
    // to_lowercase() would shift the reported index. mb_stripos must return 1.
    assert_eq!(run(r#"<?php echo mb_stripos("İa", "a");"#), "1");
    assert_eq!(run(r#"<?php echo mb_strripos("İaa", "a");"#), "2");
}

#[test]
fn strrpos_offset_semantics() {
    // Positive offset: only matches at or after it.
    assert_eq!(run(r#"<?php echo mb_strrpos("abcabc", "a", 3);"#), "3");
    // Negative offset: stop that many chars from the end.
    assert_eq!(run(r#"<?php echo mb_strrpos("abcabc", "bc", -2);"#), "4");
}

#[test]
fn lcfirst_ucfirst() {
    assert_eq!(run(r#"<?php echo mb_lcfirst("HÉLLO");"#), "hÉLLO");
    assert_eq!(run(r#"<?php echo mb_ucfirst("éllo");"#), "Éllo");
    // Multibyte first char is handled without breaking the rest.
    assert_eq!(run(r#"<?php echo mb_ucfirst("ärger");"#), "Ärger");
    assert_eq!(run(r#"<?php echo mb_lcfirst("");"#), "");
    assert_eq!(run(r#"<?php echo mb_ucfirst("");"#), "");
}

#[test]
fn scrub_passthrough() {
    // phplang strings are valid UTF-8: mb_scrub returns them unchanged.
    assert_eq!(run(r#"<?php echo mb_scrub("héllo");"#), "héllo");
}

#[test]
fn strcut_byte_offsets_no_split() {
    // 3 bytes from 0: "hé" (h=1 byte, é=2 bytes), never a partial char.
    assert_eq!(run(r#"<?php echo mb_strcut("héllo", 0, 3);"#), "hé");
    assert_eq!(run(r#"<?php echo mb_strcut("héllo", 1, 3);"#), "él");
    // Start landing mid-'é' (byte 2) floors down to the char start.
    assert_eq!(run(r#"<?php echo mb_strcut("héllo", 2);"#), "éllo");
    // Negative start counts from the end (in bytes).
    assert_eq!(run(r#"<?php echo mb_strcut("héllo", -2);"#), "lo");
    // Negative length omits trailing bytes.
    assert_eq!(run(r#"<?php echo mb_strcut("hello", 1, -1);"#), "ell");
}

#[test]
fn split_regex() {
    assert_eq!(
        run(r#"<?php echo implode("|", mb_split(",", "a,b,c"));"#),
        "a|b|c"
    );
    assert_eq!(
        run(r#"<?php echo implode("|", mb_split("\s+", "a  b   c"));"#),
        "a|b|c"
    );
    // Limit caps the pieces; the last holds the remainder.
    assert_eq!(
        run(r#"<?php echo implode("|", mb_split(",", "a,b,c,d", 2));"#),
        "a|b,c,d"
    );
    // Empty pattern returns the whole string as one element.
    assert_eq!(
        run(r#"<?php $a = mb_split("", "abc"); echo count($a), "|", $a[0];"#),
        "1|abc"
    );
    // Invalid regex returns false.
    assert_eq!(
        run(r#"<?php var_dump(mb_split("(", "abc"));"#),
        "bool(false)\n"
    );
}

#[test]
fn convert_kana_ascii_widths() {
    // Fullwidth alphanumerics + ideographic space -> halfwidth (mode "as").
    assert_eq!(
        run("<?php echo mb_convert_kana(\"ＡＢＣ１２３　\", \"as\");"),
        "ABC123 "
    );
    // Halfwidth -> fullwidth (mode "AS").
    assert_eq!(
        run(r#"<?php echo mb_convert_kana("ABC123", "AS");"#),
        "ＡＢＣ１２３"
    );
    // Letters only: 'r' converts fullwidth letters, leaves digits alone.
    assert_eq!(
        run("<?php echo mb_convert_kana(\"ＡＢ１２\", \"r\");"),
        "AB１２"
    );
    // Digits only: 'n' converts fullwidth digits, leaves letters alone.
    assert_eq!(
        run("<?php echo mb_convert_kana(\"ＡＢ１２\", \"n\");"),
        "ＡＢ12"
    );
}

#[test]
fn check_and_internal_encoding() {
    assert_eq!(
        run(r#"<?php var_dump(mb_check_encoding("hello", "ASCII"));"#),
        "bool(true)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(mb_check_encoding("héllo", "ASCII"));"#),
        "bool(false)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(mb_check_encoding("héllo", "UTF-8"));"#),
        "bool(true)\n"
    );
    // Getter returns the default; setter returns true and updates the getter.
    assert_eq!(run(r#"<?php echo mb_internal_encoding();"#), "UTF-8");
    assert_eq!(
        run(r#"<?php var_dump(mb_internal_encoding("UTF-8")); echo mb_internal_encoding();"#),
        "bool(true)\nUTF-8"
    );
}
