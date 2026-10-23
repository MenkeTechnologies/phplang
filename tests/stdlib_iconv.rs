//! `iconv` and the `iconv_*` string functions. Expected strings are output of
//! php 8.5.12 (`/opt/homebrew/bin/php -r`, linking libiconv 1.11); bytes are
//! printed through `bin2hex` so the expectations stay text.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn unicode_charsets_round_trip_with_their_byte_order() {
    let src = r##"<?php foreach (["UTF-16", "UTF-16LE", "UTF-16BE", "UCS-2", "UTF-32", "UTF-32LE", "UCS-4", "UNICODELITTLE", "utf16"] as $cs) { $b = iconv("UTF-8", $cs, "aé😀"); echo $cs, " ", bin2hex($b), " ", iconv($cs, "UTF-8", $b), "\n"; } echo bin2hex(iconv("UTF-8", "UTF-16", "")), "|", iconv("UTF-16", "UTF-8", "\xff\xfea\x00\xfe\xff\x00b"), "|", bin2hex(iconv("UTF-16BE", "UTF-8", "\xfe\xff\x00a")), "\n";"##;
    assert_eq!(
        run(src),
        r##"UTF-16 feff006100e9d83dde00 aé😀
UTF-16LE 6100e9003dd800de aé😀
UTF-16BE 006100e9d83dde00 aé😀
UCS-2 006100e9d83dde00 aé😀
UTF-32 0000feff00000061000000e90001f600 aé😀
UTF-32LE 61000000e900000000f60100 aé😀
UCS-4 00000061000000e90001f600 aé😀
UNICODELITTLE 6100e9003dd800de aé😀
utf16 feff006100e9d83dde00 aé😀
|ab|efbbbf61
"##
    );
}

#[test]
fn eight_bit_charsets_map_their_high_bytes() {
    let src = r##"<?php foreach (["ISO-8859-1", "LATIN1", "ISO-8859-15", "CP1252", "WINDOWS-1252"] as $cs) { echo $cs, " ", bin2hex(iconv("UTF-8", $cs, "é€ŒŸ")  ?: "-"), " ", iconv($cs, "UTF-8", "\x80\xa4\xbc\xe9"), "\n"; }"##;
    assert_eq!(
        run(src),
        r##"ISO-8859-1 
Notice: iconv(): Detected an illegal character in input string in Command line code on line 1
2d ¤¼é
LATIN1 
Notice: iconv(): Detected an illegal character in input string in Command line code on line 1
2d ¤¼é
ISO-8859-15 e9a4bcbe €Œé
CP1252 e9808c9f €¤¼é
WINDOWS-1252 e9808c9f €¤¼é
"##
    );
}

#[test]
fn the_platform_substitutes_without_translit_in_eight_bit_charsets() {
    let src = r##"<?php foreach (["ISO-8859-1", "ISO-8859-15", "CP1252"] as $cs) { echo $cs, " ", bin2hex(iconv("UTF-8", $cs, "ŒĀĆ™…—")), "\n"; } var_dump(iconv("UTF-8", "ISO-8859-1", "€"), bin2hex(iconv("UTF-8", "ISO-8859-1//TRANSLIT", "é€")));"##;
    assert_eq!(
        run(src),
        r##"ISO-8859-1 4f4541b443544d2e2e2e2d
ISO-8859-15 bc412743544d2e2e2e2d
CP1252 8c41b443998597

Notice: iconv(): Detected an illegal character in input string in Command line code on line 1
bool(false)
string(8) "e9455552"
"##
    );
}

#[test]
fn translit_and_ignore_suffixes() {
    let src = r##"<?php var_dump(iconv("UTF-8", "ASCII//TRANSLIT", "café"), iconv("UTF-8", "ASCII//TRANSLIT", "ß"), iconv("UTF-8", "ASCII//TRANSLIT", "©"), iconv("UTF-8", "ascii//translit", "€"), iconv("UTF-8", "ASCII//IGNORE", "aé日b"), iconv("UTF-8", "ASCII//TRANSLIT//IGNORE", "a日é"), iconv("UTF-8", "ASCII//IGNORE", "a\xffb\xc3c"), iconv("UTF-8", "ASCII//FOO", "ab"), iconv("UTF-8", "ASCII//TRANSLIT", "a日"));"##;
    assert_eq!(
        run(src),
        r##"
Notice: iconv(): Detected an illegal character in input string in Command line code on line 1
string(5) "caf'e"
string(2) "ss"
string(3) "(c)"
string(3) "EUR"
string(2) "ab"
string(3) "a'e"
string(3) "abc"
string(2) "ab"
bool(false)
"##
    );
}

#[test]
fn failures_raise_the_reference_diagnostics() {
    let src = r##"<?php var_dump(iconv("UTF-8", "ASCII", "aéb"), iconv("UTF-8", "UTF-16", "\xff"), iconv("UTF-8", "UTF-16", "a\xc3"), iconv("UTF-16LE", "UTF-8", "a"), iconv("UTF-16", "UTF-8", "\xdc\x00"), iconv("UTF-8//IGNORE", "ASCII", "a\xffb"), iconv("NOPE", "UTF-8", "a"), iconv("UTF-8", " ASCII", "a"), iconv("UTF-8", "ASCII//IGNORE", "a\xc3"));"##;
    assert_eq!(
        run(src),
        r##"
Notice: iconv(): Detected an illegal character in input string in Command line code on line 1

Notice: iconv(): Detected an illegal character in input string in Command line code on line 1

Notice: iconv(): Detected an incomplete multibyte character in input string in Command line code on line 1

Notice: iconv(): Detected an incomplete multibyte character in input string in Command line code on line 1

Notice: iconv(): Detected an illegal character in input string in Command line code on line 1

Notice: iconv(): Detected an illegal character in input string in Command line code on line 1

Warning: iconv(): Wrong encoding, conversion from "NOPE" to "UTF-8" is not allowed in Command line code on line 1

Warning: iconv(): Wrong encoding, conversion from "UTF-8" to " ASCII" is not allowed in Command line code on line 1

Notice: iconv(): Detected an incomplete multibyte character in input string in Command line code on line 1
bool(false)
bool(false)
bool(false)
bool(false)
bool(false)
bool(false)
bool(false)
bool(false)
bool(false)
"##
    );
}

#[test]
fn character_functions_count_characters() {
    let src = r##"<?php var_dump(iconv_strlen("héllo"), iconv_strlen("é", "ISO-8859-1"), iconv_strlen("a\xff"), iconv_strlen("a", "NOPE"), iconv_substr("héllo", 1, 2), iconv_substr("héllo", -3), iconv_substr("héllo", 1, -1), iconv_substr("abc", 5), iconv_substr("\xe9t\xe9", 1, 1, "ISO-8859-1"), iconv_strpos("héllo", "l"), iconv_strpos("héllo", "l", -2), iconv_strpos("abc", ""), iconv_strpos("abc", "z"), iconv_strrpos("héllo", "l"), iconv_strrpos("abc", ""));
try { iconv_strpos("abc", "a", 4); } catch (ValueError $e) { echo $e->getMessage(), "\n"; }"##;
    assert_eq!(
        run(src),
        r##"
Notice: iconv_strlen(): Detected an illegal character in input string in Command line code on line 1

Warning: iconv_strlen(): Wrong encoding, conversion from "NOPE" to "UCS-4LE" is not allowed in Command line code on line 1
int(5)
int(2)
bool(false)
bool(false)
string(3) "él"
string(3) "llo"
string(4) "éll"
string(0) ""
string(1) "t"
int(2)
int(3)
bool(false)
bool(false)
int(3)
bool(false)
iconv_strpos(): Argument #3 ($offset) must be contained in argument #1 ($haystack)
"##
    );
}

#[test]
fn settings_and_constants() {
    let src = r##"<?php var_dump(iconv_get_encoding(), iconv_get_encoding("output_encoding"), iconv_get_encoding("x"), ICONV_IMPL, ICONV_VERSION);"##;
    assert_eq!(
        run(src),
        r##"array(3) {
  ["input_encoding"]=>
  string(5) "UTF-8"
  ["output_encoding"]=>
  string(5) "UTF-8"
  ["internal_encoding"]=>
  string(5) "UTF-8"
}
string(5) "UTF-8"
bool(false)
string(8) "libiconv"
string(4) "1.11"
"##
    );
}
