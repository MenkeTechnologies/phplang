//! End-to-end tests for the `hash` stdlib category: PHP source in, captured
//! `echo` output out. Expected values cross-checked against reference PHP 8.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn md5_known_vectors() {
    assert_eq!(run(r#"<?php echo md5("");"#), "d41d8cd98f00b204e9800998ecf8427e");
    assert_eq!(
        run(r#"<?php echo md5("The quick brown fox jumped over the lazy dog.");"#),
        "5c6ffbdd40d9556b73a21e63c3e0e904"
    );
}

#[test]
fn sha1_known_vectors() {
    assert_eq!(
        run(r#"<?php echo sha1("");"#),
        "da39a3ee5e6b4b0d3255bfef95601890afd80709"
    );
    assert_eq!(
        run(r#"<?php echo sha1("abc");"#),
        "a9993e364706816aba3e25717850c26c9cd0d89d"
    );
}

#[test]
fn hash_sha256_sha512() {
    assert_eq!(
        run(r#"<?php echo hash("sha256", "");"#),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        run(r#"<?php echo hash("sha512", "");"#),
        "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce\
         47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"
    );
    assert_eq!(
        run(r#"<?php echo hash("sha256", "abc");"#),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn hash_md5_sha1_via_algo() {
    assert_eq!(run(r#"<?php echo hash("md5", "");"#), "d41d8cd98f00b204e9800998ecf8427e");
    assert_eq!(
        run(r#"<?php echo hash("sha1", "abc");"#),
        "a9993e364706816aba3e25717850c26c9cd0d89d"
    );
}

#[test]
fn crc32_function_value() {
    // 64-bit PHP returns the unsigned crc as a positive int.
    assert_eq!(
        run(r#"<?php echo crc32("The quick brown fox jumped over the lazy dog.");"#),
        "2191738434"
    );
    assert_eq!(run(r#"<?php echo crc32("");"#), "0");
    assert_eq!(run(r#"<?php echo crc32("123456789");"#), "3421780262");
}

#[test]
fn hash_crc32_variants() {
    let dog = "The quick brown fox jumped over the lazy dog.";
    // crc32b == hexdec matches crc32(); crc32 is the distinct BZIP2 variant.
    assert_eq!(run(&format!(r#"<?php echo hash("crc32b", "{dog}");"#)), "82a34642");
    assert_eq!(run(&format!(r#"<?php echo hash("crc32", "{dog}");"#)), "413a86af");
    assert_eq!(run(r#"<?php echo hash("crc32b", "");"#), "00000000");
    // Cross-check crc32() equals hexdec(hash('crc32b', …)).
    assert_eq!(
        run(&format!(r#"<?php echo crc32("{dog}") === hexdec(hash("crc32b","{dog}")) ? "y":"n";"#)),
        "y"
    );
}

#[test]
fn hash_hmac_vectors() {
    assert_eq!(
        run(r#"<?php echo hash_hmac("sha256", "The quick brown fox", "key");"#),
        "203d1e5cedd2d18f8c5a3beff0bd9c1ebcb97097dfcb288c46b00c9227fde2c0"
    );
    assert_eq!(
        run(r#"<?php echo hash_hmac("md5", "data", "secret");"#),
        "df08aef118f36b32e29d2f47cda649b6"
    );
    assert_eq!(
        run(r#"<?php echo hash_hmac("sha1", "message", "key");"#),
        "2088df74d5f2146b48146caf4965377e9d0be3a4"
    );
}

#[test]
fn hash_hmac_sha2_family() {
    // sha256/sha384/sha512 (block sizes 64/128/128) cross-checked against
    // `openssl dgst -<algo> -hmac key` (matches PHP hash_hmac).
    assert_eq!(
        run(r#"<?php echo hash_hmac("sha256", "abc", "key");"#),
        "9c196e32dc0175f86f4b1cb89289d6619de6bee699e4c378e68309ed97a1a6ab"
    );
    assert_eq!(
        run(r#"<?php echo hash_hmac("sha384", "abc", "key");"#),
        "30ddb9c8f347cffbfb44e519d814f074cf4047a55d6f563324f1c6a33920e5ed\
         fb2a34bac60bdc96cd33a95623d7d638"
    );
    assert_eq!(
        run(r#"<?php echo hash_hmac("sha512", "abc", "key");"#),
        "3926a207c8c42b0c41792cbd3e1a1aaaf5f7a25704f62dfc939c4987dd7ce060\
         009c5bb1c2447355b3216f10b537e9afa7b64a4e5391b0d631172d07939e087a"
    );
    // Longer message exercises the 128-byte block padding for sha512.
    assert_eq!(
        run(r#"<?php echo hash_hmac("sha512", "The quick brown fox", "key");"#),
        "36f44b125a8a90639dc46733039571792e081e0fd8685ff746784b02ed14aa35\
         629d562c7117cde4a701570551faa5a5e1b7ef1eb5c3bcd4cc1fdb8923fcf14e"
    );
}

#[test]
fn hash_unknown_algo_php8_valueerror() {
    // PHP 8 ValueError text (not the PHP 7 "Unknown hashing algorithm").
    assert_eq!(
        eval_capture(r#"<?php echo hash("bogus", "x");"#).unwrap_err(),
        "hash(): Argument #1 ($algo) must be a valid hashing algorithm"
    );
    assert_eq!(
        eval_capture(r#"<?php echo hash_hmac("bogus", "x", "k");"#).unwrap_err(),
        "hash_hmac(): Argument #1 ($algo) must be a valid cryptographic hashing algorithm"
    );
}

#[test]
fn hash_hmac_long_key() {
    // Key longer than the 64-byte block is pre-hashed; verifies that path
    // against a fixed reference value from PHP.
    let long = "k".repeat(100);
    assert_eq!(
        run(&format!(r#"<?php echo hash_hmac("md5", "msg", "{long}");"#)),
        "a908a4d5326a80f4b50c9a1951513b67"
    );
}

#[test]
fn hash_algos_list() {
    assert_eq!(
        run(r#"<?php echo implode(",", hash_algos());"#),
        "md5,sha1,sha256,sha512,crc32,crc32b"
    );
    assert_eq!(run(r#"<?php echo in_array("sha512", hash_algos()) ? "y":"n";"#), "y");
}

#[test]
fn raw_output_binary_string() {
    // ASCII-safe digest (crc32b of "" is four NUL bytes) round-trips exactly
    // through bin2hex, matching the hex form.
    assert_eq!(
        run(r#"<?php echo bin2hex(hash("crc32b", "", true));"#),
        run(r#"<?php echo hash("crc32b", "");"#)
    );
    assert_eq!(run(r#"<?php echo strlen(hash("crc32b", "", true));"#), "4");
    // Raw output follows the codebase's chr/ord byte model: the leading raw
    // md5 byte (0xd4) is emitted as chr(0xd4), so ord() agrees with chr().
    assert_eq!(
        run(r#"<?php echo ord(md5("", true)) === ord(chr(212)) ? "y":"n";"#),
        "y"
    );
}
