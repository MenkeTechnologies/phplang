//! End-to-end tests for the `hashext` stdlib category: file digests,
//! constant-time comparison, PBKDF2 and random bytes. PHP source in, captured
//! `echo` output out. Expected values cross-checked against reference PHP 8 /
//! RFC test vectors. File tests use unique temp paths under the system temp dir
//! and clean up after themselves; no network access.
//!
//! `random_bytes` output is random, so its tests pin only the length and the
//! argument errors, never the bytes.

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
    p.push(format!(
        "phplang_hashext_{}_{}_{}.tmp",
        tag,
        std::process::id(),
        nanos
    ));
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
    // A path that does not exist is the stream layer's warning, then PHP false
    // (php 8.5.12).
    let path = php_path(&temp_path("missing"));
    let out = run(&format!(r#"<?php var_dump(md5_file("{path}"));"#));
    assert!(
        out.contains(&format!(
            "Warning: md5_file({path}): Failed to open stream: No such file or directory"
        )) && out.ends_with("bool(false)\n"),
        "{out}"
    );
}

#[test]
fn hash_file_unknown_algo_valueerror() {
    let path = temp_path("hfbad");
    std::fs::write(&path, "x").unwrap();
    // Catchable `ValueError`, matching the core `hash()` wording. Re-verified
    // against php 8.5.9.
    let src = format!(
        r#"<?php try {{ hash_file("bogus", "{}"); }} catch (Throwable $e) {{ echo get_class($e), ': ', $e->getMessage(); }}"#,
        php_path(&path)
    );
    let out = run(&src);
    std::fs::remove_file(&path).ok();
    assert_eq!(
        out,
        "ValueError: hash_file(): Argument #1 ($algo) must be a valid hashing algorithm"
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
    assert_eq!(
        run(r#"<?php echo hash_equals("abc", "abc") === true ? "y" : "n";"#),
        "y"
    );
    assert_eq!(
        run(r#"<?php echo hash_equals("abc", "abd") === false ? "y" : "n";"#),
        "y"
    );
    // Different lengths are false.
    assert_eq!(
        run(r#"<?php echo hash_equals("abc", "ab") ? "y" : "n";"#),
        "n"
    );
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
        run(
            r#"<?php echo hash_pbkdf2("sha1", "password", "salt", 1, 20, true) === hex2bin("0c60c80f961f0e71f3a9b524af6012062fe037a6") ? "y" : "n";"#
        ),
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
        run(
            r#"<?php echo hash_pbkdf2("sha256", "password", "salt", 1, 32, true) === hex2bin("120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b") ? "y" : "n";"#
        ),
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
    // All three are catchable `ValueError`s, as the reference throws — the class
    // is asserted alongside the message so a plain engine abort cannot pass.
    let caught = |call: &str| {
        run(&format!(
            r#"<?php try {{ {call}; }}
               catch (ValueError $e) {{ echo get_class($e), "|", $e->getMessage(); }}"#
        ))
    };
    assert_eq!(
        caught(r#"hash_pbkdf2("bogus", "p", "s", 1, 8)"#),
        "ValueError|hash_pbkdf2(): Argument #1 ($algo) must be a valid cryptographic \
         hashing algorithm"
    );
    assert_eq!(
        caught(r#"hash_pbkdf2("sha1", "p", "s", 0, 8)"#),
        "ValueError|hash_pbkdf2(): Argument #4 ($iterations) must be greater than 0"
    );
    assert_eq!(
        caught(r#"hash_pbkdf2("sha1", "p", "s", 1, -1)"#),
        "ValueError|hash_pbkdf2(): Argument #5 ($length) must be greater than or equal to 0"
    );
}

#[test]
fn random_bytes_length_and_error() {
    // The result is that many BYTES.
    assert_eq!(run(r#"<?php echo strlen(random_bytes(16));"#), "16");
    assert_eq!(run(r#"<?php echo strlen(random_bytes(1));"#), "1");
    assert_eq!(run(r#"<?php echo strlen(random_bytes(100));"#), "100");
    // Zero / negative length is a catchable ValueError, as the reference throws.
    assert_eq!(
        run(r#"<?php try { random_bytes(0); }
               catch (ValueError $e) { echo get_class($e), "|", $e->getMessage(); }"#),
        "ValueError|random_bytes(): Argument #1 ($length) must be greater than 0"
    );
}

#[test]
fn file_digests_diagnose_an_unreadable_path_as_the_stream_layer_does() {
    // php 8.5.12, for every one-shot file digest: an empty path throws, a
    // directory opens but cannot be read (a notice), and both give `false`.
    let dir = php_path(&std::env::temp_dir());
    for (fname, call) in [
        ("md5_file", "md5_file(%)"),
        ("sha1_file", "sha1_file(%)"),
        ("hash_file", r#"hash_file("md5", %)"#),
        ("hash_hmac_file", r#"hash_hmac_file("md5", %, "k")"#),
    ] {
        let empty = call.replace('%', r#""""#);
        assert_eq!(
            run(&format!(
                "<?php try {{ {empty}; }} catch (ValueError $e) {{ echo $e->getMessage(); }}"
            )),
            "Path must not be empty",
            "{fname}"
        );
        let on_dir = call.replace('%', &format!(r#""{dir}""#));
        let out = run(&format!("<?php var_dump({on_dir});"));
        assert!(
            out.contains(&format!(
                "Notice: {fname}(): Read of 8192 bytes failed with errno=21 Is a directory"
            )) && out.ends_with("bool(false)\n"),
            "{fname}: {out}"
        );
    }
}

#[test]
fn hash_file_reads_options_after_opening_the_file() {
    let path = temp_path("hfopt");
    std::fs::write(&path, "hello world").unwrap();
    let p = php_path(&path);
    // php 8.5.12: the seeded digest of the file bytes, the same as `hash()`'s.
    let out = run(&format!(
        r#"<?php echo hash_file("xxh64", "{p}", false, ["seed" => 7]), "|", hash("xxh64", "hello world", false, ["seed" => 7]), "|", hash_file("ripemd160", "{p}");"#
    ));
    std::fs::remove_file(&path).ok();
    assert_eq!(
        out,
        "e3188f1f1023e4d0|e3188f1f1023e4d0|98c615784ccb5fe5936fbc0cbe9dfdb408d92f0f"
    );
    // A missing file is `false` before the conflicting options are looked at.
    let missing = php_path(&temp_path("hfoptmissing"));
    let out = run(&format!(
        r#"<?php var_dump(hash_file("xxh3", "{missing}", false, ["seed" => 1, "secret" => "s"]));"#
    ));
    assert!(out.ends_with("bool(false)\n"), "{out}");
}

/// The rest of `random_bytes`' argument contract, and the exception class the
/// OS-CSPRNG failure path throws (`Random\RandomException`, a plain
/// `Exception` subclass, internal and not final). The failure itself cannot be
/// provoked from PHP; its messages are pinned by `stdlib::csprng`'s unit tests.
/// Expected text recorded from php 8.5.12.
#[test]
fn random_bytes_argument_errors_and_exception_class() {
    assert_eq!(
        run(r#"<?php try { random_bytes(-1); } catch (ValueError $e) { echo $e->getMessage(); }"#),
        "random_bytes(): Argument #1 ($length) must be greater than 0"
    );
    assert_eq!(
        run(r#"<?php try { random_bytes("x"); } catch (TypeError $e) { echo $e->getMessage(); }"#),
        "random_bytes(): Argument #1 ($length) must be of type int, string given"
    );
    assert_eq!(
        run(r#"<?php $r = new ReflectionClass('Random\RandomException');
               var_dump($r->getName(), $r->getParentClass()->getName(), $r->isInternal(), $r->isFinal());"#),
        "string(22) \"Random\\RandomException\"\nstring(9) \"Exception\"\nbool(true)\nbool(false)\n"
    );
    // The namespaced class leaves the global name free for user code.
    assert_eq!(run(r#"<?php class RandomException {} echo get_class(new RandomException);"#), "RandomException");
}
