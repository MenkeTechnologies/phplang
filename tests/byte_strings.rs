//! PHP strings are byte strings (see `src/bytes.rs`): a byte that is not part
//! of valid UTF-8 survives every operation as one byte. Expected strings are
//! `php -r` output from php 8.5.12 (`/opt/homebrew/bin/php`); each snippet
//! prints bytes through `bin2hex` so the expectations stay text.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn a_raw_byte_is_one_byte() {
    let src = r##"<?php var_dump(strlen(chr(255)), ord(chr(200)), bin2hex("\xff\x00a"), strlen("\xc3\xa9"), "\xc3\xa9" === "é", chr(0xc3) . chr(0xa9) === "é", bin2hex(substr("héllo", 0, 2)), strlen(substr("héllo", 1, 1)), bin2hex("\377\200"));"##;
    assert_eq!(
        run(src),
        r##"int(1)
int(200)
string(6) "ff0061"
int(2)
bool(true)
bool(true)
string(4) "68c3"
int(1)
string(4) "ff80"
"##
    );
}

#[test]
fn string_functions_work_on_bytes() {
    let src = r##"<?php $b = "\xff\x00\x80abc\xc3\xa9\xc3"; echo strlen($b), " ", bin2hex(strrev($b)), " ", bin2hex(implode("|", str_split($b, 3))), " ", bin2hex(str_pad("\xff", 4, "\x80")), " ", bin2hex(strtr($b, "\xff\x80", "XY")), " ", bin2hex(strtr($b, ["\xff" => "<FF>", "é" => "E"])), " ", strpos($b, "abc"), " ", strpos($b, "\xc3", 7), " ", bin2hex($b[0] . $b[2] . $b[-1]), " ", bin2hex(substr_replace("abc", "\xff", 1, 1)), " ", bin2hex(str_replace("b", "\xff", "abc")), " ", bin2hex(sprintf("%s|%5s|%c", "\xff", "\x80", 255)), " ", strcmp("\xff", "\x80") > 0 ? "gt" : "le", " ", bin2hex(trim("\xff \x00")), " ", bin2hex(chunk_split("\xff\xff", 1, "-")), " ", substr_count("\xff\xff\xff", "\xff"), " ", bin2hex(str_repeat("\xc3", 2)), " ", similar_text("\xff\x80", "\xff"), " ", levenshtein("\xff", "\x80"), "\n";"##;
    assert_eq!(
        run(src),
        r##"9 c3a9c36362618000ff ff00807c6162637cc3a9c3 ff808080 580059616263c3a9c3 3c46463e008061626345c3 3 8 ff80c3 61ff63 61ff63 ff7c20202020807cff gt ff ff2dff2d 3 c3c3 1 1
"##
    );
}

#[test]
fn encodings_and_hashes_see_the_bytes() {
    let src = r##"<?php echo bin2hex(hex2bin("c3a9ff")), " ", strlen(base64_decode("w6n/")), " ", base64_encode("\xff\xfe"), " ", md5("\xff"), " ", bin2hex(md5("a", true)), " ", sha1("\x80\x81"), " ", crc32("\xff"), " ", hash("sha256", "\xff"), " ", bin2hex(hash("crc32b", "x", true)), " ", urlencode("\xff é"), " ", rawurlencode("\xff"), " ", bin2hex(urldecode("%FF%C3%A9")), " ", strlen(random_bytes(16)), " ", bin2hex(quoted_printable_decode("h=C3=A9=FF")), " ", quoted_printable_encode("\xff"), " ", strlen(count_chars("aab", 4)), " ", bin2hex(serialize("\xff")), " ", bin2hex(unserialize(serialize("\xff\x80"))), "\n";"##;
    assert_eq!(
        run(src),
        r##"c3a9ff 3 //4= 00594fd4f42ba43fc1ca0427a0576295 0cc175b9c0f1b6a831c399e269772661 53a73f8fdc04bc2c9b325b193bbee8fa1a7d6a88 4278190080 a8100ae6aa1940d0b663bb31cd466142ebbdbd5187131b92d93818987832eb89 8cdc1683 %FF+%C3%A9 %FF ffc3a9 16 68c3a9ff =FF 254 733a313a22ff223b ff80
"##
    );
}

#[test]
fn pack_and_unpack_round_trip_bytes() {
    let src = r##"<?php echo bin2hex(pack("q", -5)), " ", implode(",", unpack("q", pack("q", -5))), " ", bin2hex(pack("NnCa3", 0x80818283, 0xfeff, 255, "\xff")), " ", implode(",", unpack("N1a/n1b/C1c", "\x80\x81\x82\x83\xfe\xff\xff")), " ", bin2hex(unpack("a2x", "\xff\xfe")["x"]), "\n";"##;
    assert_eq!(
        run(src),
        r##"fbffffffffffffff -5 80818283feffffff0000 2155971203,65279,255 fffe
"##
    );
}

#[test]
fn invalid_utf8_is_refused_or_replaced() {
    let src = r##"<?php var_dump(json_encode("\xff"), json_last_error_msg(), json_encode("a\xffb", JSON_INVALID_UTF8_IGNORE), json_encode("a\xffb", JSON_INVALID_UTF8_SUBSTITUTE), json_encode(["a" => "\xff", "b" => 1], JSON_PARTIAL_OUTPUT_ON_ERROR), json_last_error(), mb_check_encoding("\xff"), mb_check_encoding("é"), bin2hex(htmlspecialchars("a\xffb")), htmlspecialchars("a\xffb", ENT_QUOTES), htmlspecialchars("a\xffb", ENT_QUOTES | ENT_IGNORE));"##;
    assert_eq!(
        run(src),
        r##"bool(false)
string(56) "Malformed UTF-8 characters, possibly incorrectly encoded"
string(4) ""ab""
string(10) ""a\ufffdb""
string(16) "{"a":null,"b":1}"
int(5)
bool(false)
bool(true)
string(10) "61efbfbd62"
string(0) ""
string(2) "ab"
"##
    );
}

#[test]
fn files_read_and_write_bytes() {
    let src = r##"<?php $f = sys_get_temp_dir() . "/phplang_bytes_" . getmypid() . ".bin"; $data = "\x00\xff\x80\xc3\xa9\xfe\n\xc3"; var_dump(file_put_contents($f, $data), filesize($f), bin2hex(file_get_contents($f))); $h = fopen($f, "rb"); var_dump(bin2hex(fread($h, 3)), bin2hex(fgets($h)), bin2hex(fgetc($h)), ftell($h)); fclose($h); $h = fopen($f, "ab"); var_dump(fwrite($h, "\xfd\xfc")); fclose($h); var_dump(bin2hex(file_get_contents($f, false, null, 2, 3)), bin2hex(file_get_contents($f, false, null, -2)), md5_file($f) === md5(file_get_contents($f))); $m = fopen("php://memory", "w+"); fwrite($m, "\xff\x00\xee"); rewind($m); var_dump(bin2hex(stream_get_contents($m))); unlink($f);"##;
    assert_eq!(
        run(src),
        r##"int(8)
int(8)
string(16) "00ff80c3a9fe0ac3"
string(6) "00ff80"
string(8) "c3a9fe0a"
string(2) "c3"
int(8)
int(2)
string(6) "80c3a9"
string(4) "fdfc"
bool(true)
string(6) "ff00ee"
"##
    );
}

#[test]
fn regular_expressions_match_bytes() {
    let src = r##"<?php var_dump(preg_match('/\xff/', "a\xffb"), bin2hex(preg_replace('/b/', "\x80", "a\xffb")), bin2hex(implode(",", preg_split('//', "\xff\xfe", -1, PREG_SPLIT_NO_EMPTY))), preg_match("/./u", "\xff"), preg_last_error_msg(), preg_replace("/a/u", "b", "a\xff"), preg_match("/./u", "é"), preg_last_error());"##;
    assert_eq!(
        run(src),
        r##"int(1)
string(6) "61ff80"
string(6) "ff2cfe"
bool(false)
string(56) "Malformed UTF-8 characters, possibly incorrectly encoded"
NULL
int(1)
int(0)
"##
    );
}

#[test]
fn mbstring_reads_an_invalid_sequence_as_one_character() {
    let src = r##"<?php echo bin2hex(mb_substr("\xff\xc3\xa9x", 0, 2)), " ", bin2hex(mb_strtoupper("\xffa")), " ", mb_strlen("\xff\xfe"), " ", bin2hex(mb_convert_case("\xff", MB_CASE_TITLE)), " ", mb_strpos("\xffa", "a"), " ", mb_strlen("\xe2\x82"), mb_strlen("\xe2\x82a"), " ", bin2hex(mb_strtolower("\xe2\x82A")), " ", bin2hex(mb_substr("\xe2\x82ab", 1)), "\n";"##;
    assert_eq!(
        run(src),
        r##"3fc3a9 3f41 2 3f 1 12 3f61 6162
"##
    );
}

#[test]
fn escapes_and_joins_are_canonical() {
    let src = r##"<?php var_dump(bin2hex("\u{10FFFF}"), strlen("\u{10FFFF}"), implode(["\xc3", "\xa9"]) === "é", bin2hex(ucwords("\xff a")), bin2hex(strrchr("a\xffb\xff", "\xff")), isset(["é" => 1][chr(0xc3) . chr(0xa9)]), bin2hex(stripcslashes('\xff\101')), bin2hex(var_export("\xff", true)));"##;
    assert_eq!(
        run(src),
        r##"string(8) "f48fbfbf"
int(4)
bool(true)
string(6) "ff2041"
string(2) "ff"
bool(true)
string(4) "ff41"
string(6) "27ff27"
"##
    );
}

#[test]
fn echo_writes_the_raw_bytes() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_php"))
        .args(["-r", r#"echo "\xff\xfe", chr(0x80), "é";"#])
        .output()
        .unwrap();
    assert_eq!(out.stdout, b"\xff\xfe\x80\xc3\xa9");
}

#[test]
fn a_script_may_hold_bytes_that_are_not_utf8() {
    // Latin-1 bytes in a literal and in the inline text after `?>` are the
    // script's bytes, not a read error.
    let path = std::env::temp_dir().join(format!("phplang_raw_src_{}.php", std::process::id()));
    std::fs::write(
        &path,
        b"<?php $s = \"a\xe9b\"; echo strlen($s), bin2hex($s), \"\\n\"; ?>\xfd",
    )
    .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_php"))
        .arg(&path)
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(out.stdout, b"361e962\n\xfd");
}

#[test]
fn invalid_utf8_units_are_grouped_as_ext_standard_groups_them() {
    let src = r##"<?php foreach (["\xe2\x82", "\xe2a\x82", "\xc0\x80", "\xed\xa0\x80", "\xf4\x90\x80\x80", "\xf0\x9f\x98a\xc3\xa9"] as $s) { echo bin2hex(@utf8_decode($s)), " ", bin2hex(htmlspecialchars($s, ENT_SUBSTITUTE)), " ", json_encode($s, JSON_INVALID_UTF8_SUBSTITUTE), " ", bin2hex(htmlspecialchars($s, ENT_IGNORE)), "\n"; }"##;
    assert_eq!(
        run(src),
        r##"3f efbfbd "\ufffd" 
3f613f efbfbd61efbfbd "\ufffda\ufffd" 61
3f3f efbfbdefbfbd "\ufffd\ufffd" 
3f efbfbd "\ufffd" 
3f efbfbd "\ufffd" 
3f61e9 efbfbd61c3a9 "\ufffda\u00e9" 61c3a9
"##
    );
}

#[test]
fn utf8_encode_widens_each_raw_byte() {
    let src =
        r##"<?php echo bin2hex(@utf8_encode("\xe9\xff")), " ", bin2hex(@utf8_encode("é")), "\n";"##;
    assert_eq!(
        run(src),
        r##"c3a9c3bf c383c2a9
"##
    );
}

#[test]
fn mb_convert_encoding_writes_single_byte_charsets_as_bytes() {
    let src = r##"<?php echo bin2hex(mb_convert_encoding("é€", "ISO-8859-1")), " ", bin2hex(mb_convert_encoding("é€", "ASCII")), " ", bin2hex(mb_convert_encoding("\xe9", "UTF-8", "ISO-8859-1")), " ", bin2hex(mb_convert_encoding("a\xe2\x82b", "ISO-8859-1")), " ", bin2hex(mb_convert_encoding("\xed\xa0\x80", "UTF-8")), " ", bin2hex(mb_convert_encoding("é", "UTF-8", "ASCII")), " ", bin2hex(mb_convert_encoding("\xe9t", "UTF-8", "ASCII,ISO-8859-1")), " ", bin2hex(mb_convert_encoding("\xe9t", "UTF-8", ["UTF-8", "ISO-8859-1"])), "\n";"##;
    assert_eq!(
        run(src),
        r##"e93f 3f3f c3a9 613f62 3f3f3f 3f3f c3a974 c3a974
"##
    );
}
