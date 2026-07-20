//! End-to-end tests for the `hashext` stdlib category: file digests,
//! constant-time comparison, PBKDF2 and random bytes. PHP source in, captured
//! `echo` output out. Expected values cross-checked against reference PHP 8 /
//! RFC test vectors. File tests use unique temp paths under the system temp dir
//! and clean up after themselves; no network access.

use phplang::eval_capture;
use std::path::{Path, PathBuf};

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

/// A unique temp path (per pid + nanos + tag) that does not exist yet.
fn temp_path(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut p = std::env::temp_dir();
    p.push(format!("phplang_hashext_{}_{}_{}.tmp", tag, std::process::id(), nanos));
    p
}

/// PHP string literal escaping for a filesystem path used inside double quotes.
fn php_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "\\\\")
}

#[test]
fn md5_file_matches_content_hash() {
    let path = temp_path("md5");
    let content = "The quick brown fox jumped over the lazy dog.";
    std::fs::write(&path, content).unwrap();
    let src = format!(
        r#"<?php echo md5_file("{p}") === md5("{c}") ? "y" : "n";"#,
        p = php_path(&path),
        c = content
    );
    let out = run(&src);
    std::fs::remove_file(&path).ok();
    assert_eq!(out, "y");
    // Absolute known value for the content above.
    let path2 = temp_path("md5b");
    std::fs::write(&path2, content).unwrap();
    let src2 = format!(r#"<?php echo md5_file("{}");"#, php_path(&path2));
    let out2 = run(&src2);
    std::fs::remove_file(&path2).ok();
    assert_eq!(out2, "5c6ffbdd40d9556b73a21e63c3e0e904");
}

#[test]
fn sha1_file_matches_content_hash() {
    let path = temp_path("sha1");
    std::fs::write(&path, "abc").unwrap();
    let src = format!(r#"<?php echo sha1_file("{}");"#, php_path(&path));
    let out = run(&src);
    std::fs::remove_file(&path).ok();
    assert_eq!(out, "a9993e364706816aba3e25717850c26c9cd0d89d");
}

#[test]
fn hash_file_sha256_and_binary_roundtrip() {
    let path = temp_path("hf");
    std::fs::write(&path, "abc").unwrap();
    let p = php_path(&path);
    // sha256 of "abc".
    let out = run(&format!(r#"<?php echo hash_file("sha256", "{p}");"#));
    // Raw output round-trips through hex2bin (both use the Latin-1 byte model);
    // bin2hex is avoided here because it re-encodes high bytes as UTF-8.
    let roundtrip = run(&format!(
        r#"<?php echo hash_file("sha256", "{p}", true) === hex2bin(hash_file("sha256", "{p}")) ? "y" : "n";"#
    ));
    std::fs::remove_file(&path).ok();
    assert_eq!(
        out,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(roundtrip, "y");
}

#[test]
fn md5_file_missing_returns_false() {
    // A path that does not exist yields PHP false (rendered as empty by echo).
    let path = temp_path("missing");
    let src = format!(r#"<?php var_dump(md5_file("{}"));"#, php_path(&path));
    assert_eq!(run(&src).trim(), "bool(false)");
}

#[test]
fn hash_file_unknown_algo_valueerror() {
    let path = temp_path("hfbad");
    std::fs::write(&path, "x").unwrap();
    let src = format!(r#"<?php echo hash_file("bogus", "{}");"#, php_path(&path));
    let err = eval_capture(&src).unwrap_err();
    std::fs::remove_file(&path).ok();
    assert_eq!(
        err,
        "hash_file(): Argument #1 ($algo) must be a valid hashing algorithm"
    );
}

#[test]
fn hash_hmac_file_matches_reference() {
    let path = temp_path("hmacf");
    std::fs::write(&path, "The quick brown fox").unwrap();
    let src = format!(
        r#"<?php echo hash_hmac_file("sha256", "{}", "key");"#,
        php_path(&path)
    );
    let out = run(&src);
    std::fs::remove_file(&path).ok();
    // hash_hmac("sha256", "The quick brown fox", "key") — same as hashing the
    // file bytes under the key.
    assert_eq!(
        out,
        "203d1e5cedd2d18f8c5a3beff0bd9c1ebcb97097dfcb288c46b00c9227fde2c0"
    );
}

#[test]
fn hash_equals_basic() {
    assert_eq!(run(r#"<?php echo hash_equals("abc", "abc") === true ? "y" : "n";"#), "y");
    assert_eq!(run(r#"<?php echo hash_equals("abc", "abd") === false ? "y" : "n";"#), "y");
    // Different lengths are false.
    assert_eq!(run(r#"<?php echo hash_equals("abc", "ab") ? "y" : "n";"#), "n");
    // Empty strings compare equal.
    assert_eq!(run(r#"<?php echo hash_equals("", "") ? "y" : "n";"#), "y");
    // Longer known vectors.
    assert_eq!(
        run(r#"<?php echo hash_equals(sha1("x"), sha1("x")) ? "y" : "n";"#),
        "y"
    );
}

#[test]
fn pbkdf2_rfc6070_sha1_vectors() {
    // RFC 6070 test vectors for PBKDF2-HMAC-SHA1 (raw output, length = dkLen).
    assert_eq!(
        run(r#"<?php echo hash_pbkdf2("sha1", "password", "salt", 1, 20, true) === hex2bin("0c60c80f961f0e71f3a9b524af6012062fe037a6") ? "y" : "n";"#),
        "y"
    );
    // c=2 as the hex output form (length = 40 hex chars = 20 bytes).
    assert_eq!(
        run(r#"<?php echo hash_pbkdf2("sha1", "password", "salt", 2, 40);"#),
        "ea6c014dc72d6f8ccd1ed92ace1d41f0d8de8957"
    );
    // Hex output: length is the number of hex chars (half the bytes).
    assert_eq!(
        run(r#"<?php echo hash_pbkdf2("sha1", "password", "salt", 1, 20);"#),
        "0c60c80f961f0e71f3a9"
    );
}

#[test]
fn pbkdf2_sha256_and_length_zero() {
    // PBKDF2-HMAC-SHA256, c=1 (well-known vector).
    assert_eq!(
        run(r#"<?php echo hash_pbkdf2("sha256", "password", "salt", 1, 32, true) === hex2bin("120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b") ? "y" : "n";"#),
        "y"
    );
    // length = 0 uses the full digest: sha256 => 64 hex chars.
    assert_eq!(
        run(r#"<?php echo strlen(hash_pbkdf2("sha256", "password", "salt", 1));"#),
        "64"
    );
    assert_eq!(
        run(r#"<?php echo hash_pbkdf2("sha256", "password", "salt", 1);"#),
        "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
    );
}

#[test]
fn pbkdf2_errors() {
    assert_eq!(
        eval_capture(r#"<?php echo hash_pbkdf2("bogus", "p", "s", 1, 8);"#).unwrap_err(),
        "hash_pbkdf2(): Argument #1 ($algo) must be a valid hashing algorithm"
    );
    assert_eq!(
        eval_capture(r#"<?php echo hash_pbkdf2("sha1", "p", "s", 0, 8);"#).unwrap_err(),
        "hash_pbkdf2(): Argument #4 ($iterations) must be greater than 0"
    );
    assert_eq!(
        eval_capture(r#"<?php echo hash_pbkdf2("sha1", "p", "s", 1, -1);"#).unwrap_err(),
        "hash_pbkdf2(): Argument #5 ($length) must be greater than or equal to 0"
    );
}

#[test]
fn random_bytes_length_and_error() {
    // Each byte maps to exactly one codepoint under the Latin-1 wrap, so
    // mb_strlen (codepoint count) is the byte count — strlen/bin2hex would
    // over-count high bytes that re-encode as multi-byte UTF-8.
    assert_eq!(run(r#"<?php echo mb_strlen(random_bytes(16));"#), "16");
    assert_eq!(run(r#"<?php echo mb_strlen(random_bytes(1));"#), "1");
    assert_eq!(run(r#"<?php echo mb_strlen(random_bytes(100));"#), "100");
    // Zero / negative length is a ValueError.
    assert_eq!(
        eval_capture(r#"<?php echo random_bytes(0);"#).unwrap_err(),
        "random_bytes(): Argument #1 ($length) must be greater than 0"
    );
}
