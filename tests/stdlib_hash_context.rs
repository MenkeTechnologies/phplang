//! End-to-end tests for incremental hashing: `hash_init`, `hash_update`,
//! `hash_update_stream`, `hash_update_file`, `hash_final`, `hash_copy` and the
//! `HashContext` prelude class they share. PHP source in, captured output out.
//!
//! Every expectation is the byte output of `php 8.5.11` on the same source,
//! with one deliberate exception: the object handle in the `var_dump` test is
//! pinned to what THIS engine numbers, because handle reuse differs (see
//! BUGS.md, "Object handles"). File tests use unique temp paths under the
//! system temp dir and clean up after themselves.

use phplang::eval_capture;
use std::path::{Path, PathBuf};

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

/// Run `expr`, rendering a refusal as `Class: message` the way `getMessage`
/// reads, so a test pins the class AND the wording the reference uses.
fn caught(src: &str) -> String {
    run(&format!(
        "<?php try {{ {src} }} catch (Throwable $e) {{ echo get_class($e), \": \", $e->getMessage(); }}"
    ))
}

/// A unique temp path (per pid + nanos + tag) that does not exist yet.
fn temp_path(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut p = std::env::temp_dir();
    p.push(format!(
        "phplang_hashctx_{}_{}_{}.tmp",
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
fn incremental_digest_equals_one_shot_hash() {
    assert_eq!(
        run(
            r#"<?php $c = hash_init("sha256"); hash_update($c, "ab"); hash_update($c, "c"); echo hash_final($c);"#
        ),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    // Every algorithm `hash()` knows, checksums included, is accepted.
    assert_eq!(
        run(
            r#"<?php $c = hash_init("crc32b"); hash_update($c, "The quick brown fox jumped over the lazy dog."); echo hash_final($c), " ", hash("crc32b", "The quick brown fox jumped over the lazy dog.");"#
        ),
        "82a34642 82a34642"
    );
    // The algorithm name is case-insensitive and stored lowercased; a non-string
    // update is coerced as `hash()` coerces its data.
    assert_eq!(
        run(
            r#"<?php $c = hash_init("SHA256"); var_dump(hash_update($c, 123)); echo hash_final($c, 0);"#
        ),
        "bool(true)\na665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3"
    );
}

#[test]
fn hash_final_binary_flag_matches_raw_digest() {
    assert_eq!(
        run(
            r#"<?php $e = hash_init("md5"); hash_update($e, "x"); var_dump(hash_final($e, true) === md5("x", true));"#
        ),
        "bool(true)\n"
    );
}

#[test]
fn hmac_context_matches_hash_hmac() {
    let src = r#"<?php
        $h = hash_init("sha256", HASH_HMAC, "key");
        hash_update($h, "The quick brown fox jumps over the lazy dog");
        echo hash_final($h), "\n", hash_hmac("sha256", "The quick brown fox jumps over the lazy dog", "key");"#;
    assert_eq!(
        run(src),
        "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8\n\
         f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
    );
    // The key is read only when the flag asks for it.
    assert_eq!(
        run(
            r#"<?php $c = hash_init("crc32b", 0, "k"); hash_update($c, "The quick brown fox jumped over the lazy dog."); echo hash_final($c);"#
        ),
        "82a34642"
    );
    assert_eq!(run("<?php var_dump(HASH_HMAC);"), "int(1)\n");
}

#[test]
fn hash_copy_and_clone_are_independent() {
    let src = r#"<?php
        $c = hash_init("sha256"); hash_update($c, "ab");
        $d = hash_copy($c); hash_update($c, "c");
        echo hash_final($c), "\n", hash_final($d);"#;
    assert_eq!(
        run(src),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\n\
         fb8e20fc2e4c3f248c60c39bd652f3c1347298bb977b8b4d5903b85055620603"
    );
    // The copy keeps the HMAC key.
    let src = r#"<?php
        $h = hash_init("sha256", HASH_HMAC, "key"); hash_update($h, "a");
        $h2 = hash_copy($h); hash_update($h2, "b");
        echo hash_final($h), " ", hash_final($h2), " ", hash_hmac("sha256", "ab", "key");"#;
    assert_eq!(
        run(src),
        "780c3db4ce3de5b9e55816fba98f590631d96c075271b26976238d5f4444219b \
         5c1c0c948cbef5ad7d191e46e5901fa0395f85f694fa4633f665a1a637017b65 \
         5c1c0c948cbef5ad7d191e46e5901fa0395f85f694fa4633f665a1a637017b65"
    );
    assert_eq!(
        run(
            r#"<?php $cl = clone hash_init("sha1"); hash_update($cl, "abc"); echo hash_final($cl);"#
        ),
        "a9993e364706816aba3e25717850c26c9cd0d89d"
    );
}

#[test]
fn context_is_a_hashcontext_object_showing_its_algo() {
    assert_eq!(
        run(
            r#"<?php $c = hash_init("sha256"); echo get_class($c), " ", $c instanceof HashContext ? "y" : "n", " ", gettype($c);"#
        ),
        "HashContext y object"
    );
    assert_eq!(
        run(r#"<?php var_dump(hash_init("sha256")); print_r(hash_init("md5"));"#),
        "object(HashContext)#1 (1) {\n  [\"algo\"]=>\n  string(6) \"sha256\"\n}\n\
         HashContext Object\n(\n    [algo] => md5\n)\n"
    );
}

#[test]
fn hash_init_refusals() {
    assert_eq!(
        caught(r#"hash_init("nope");"#),
        "ValueError: hash_init(): Argument #1 ($algo) must be a valid hashing algorithm"
    );
    assert_eq!(
        caught(r#"hash_init("crc32b", HASH_HMAC, "k");"#),
        "ValueError: hash_init(): Argument #1 ($algo) must be a cryptographic hashing algorithm if HMAC is requested"
    );
    for src in [
        r#"hash_init("sha256", HASH_HMAC);"#,
        r#"hash_init("sha256", HASH_HMAC, "");"#,
        // Any flags value with the HMAC bit set asks for a key.
        r#"hash_init("sha256", 5);"#,
    ] {
        assert_eq!(
            caught(src),
            "ValueError: hash_init(): Argument #3 ($key) must not be empty when HMAC is requested",
            "{src}"
        );
    }
}

#[test]
fn non_context_argument_is_a_type_error_naming_the_given_type() {
    assert_eq!(
        caught(r#"hash_update(1, "x");"#),
        "TypeError: hash_update(): Argument #1 ($context) must be of type HashContext, int given"
    );
    assert_eq!(
        caught(r#"hash_update("sha1", "x");"#),
        "TypeError: hash_update(): Argument #1 ($context) must be of type HashContext, string given"
    );
    assert_eq!(
        caught(r#"hash_final(new stdClass);"#),
        "TypeError: hash_final(): Argument #1 ($context) must be of type HashContext, stdClass given"
    );
    assert_eq!(
        caught(r#"hash_update_stream(hash_init("md5"), 5);"#),
        "TypeError: hash_update_stream(): Argument #2 ($stream) must be of type resource, int given"
    );
}

#[test]
fn finalized_context_is_refused_everywhere() {
    let src = r#"<?php
        $f = hash_init("md5"); hash_final($f);
        foreach (["hash_update", "hash_final", "hash_copy"] as $fn) {
            try { $fn === "hash_update" ? hash_update($f, "x") : $fn($f); }
            catch (Throwable $t) { echo get_class($t), ": ", $t->getMessage(), "\n"; }
        }"#;
    assert_eq!(
        run(src),
        "TypeError: hash_update(): Argument #1 ($context) must be a valid, non-finalized HashContext\n\
         TypeError: hash_final(): Argument #1 ($context) must be a valid, non-finalized HashContext\n\
         TypeError: hash_copy(): Argument #1 ($context) must be a valid, non-finalized HashContext\n"
    );
}

#[test]
fn hash_update_stream_reads_from_the_cursor_and_honours_length() {
    let path = temp_path("stream");
    std::fs::write(&path, "hello file").unwrap();
    let p = php_path(&path);
    // 0 reads nothing; a negative or oversized length reads what is left.
    let src = format!(
        r#"<?php
        foreach ([0, -1, -5, 100] as $n) {{
            $fp = fopen("{p}", "r"); $u = hash_init("md5");
            echo hash_update_stream($u, $fp, $n), " ", hash_final($u), "\n";
        }}
        $fp = fopen("{p}", "r"); fread($fp, 6); $u = hash_init("md5");
        echo hash_update_stream($u, $fp), " ", hash_final($u), " ", md5("file");"#
    );
    let out = run(&src);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        out,
        "0 d41d8cd98f00b204e9800998ecf8427e\n\
         10 cf0be5077e7ed25a944c353d77df0f10\n\
         10 cf0be5077e7ed25a944c353d77df0f10\n\
         10 cf0be5077e7ed25a944c353d77df0f10\n\
         4 8c7dd922ad47494fc02c388e12c00eac 8c7dd922ad47494fc02c388e12c00eac"
    );
}

#[test]
fn hash_update_file_feeds_the_file_bytes() {
    let path = temp_path("file");
    std::fs::write(&path, "hello file").unwrap();
    let p = php_path(&path);
    let out = run(&format!(
        r#"<?php $u = hash_init("sha1"); var_dump(hash_update_file($u, "{p}")); echo hash_final($u), " ", sha1_file("{p}");"#
    ));
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        out,
        "bool(true)\n03a798d01a518055102dd78f8b411cbd00d7a97b 03a798d01a518055102dd78f8b411cbd00d7a97b"
    );
}

#[test]
fn hash_update_file_failures() {
    assert_eq!(
        caught(r#"hash_update_file(hash_init("md5"), "");"#),
        "ValueError: Path must not be empty"
    );
    let missing = php_path(&temp_path("missing"));
    let out = run(&format!(
        r#"<?php $u = hash_init("md5"); var_dump(hash_update_file($u, "{missing}"));"#
    ));
    assert!(
        out.contains(&format!(
            "Warning: hash_update_file({missing}): Failed to open stream: No such file or directory"
        )) && out.ends_with("bool(false)\n"),
        "{out}"
    );
    let dir = php_path(&std::env::temp_dir());
    let out = run(&format!(
        r#"<?php $u = hash_init("md5"); var_dump(hash_update_file($u, "{dir}"));"#
    ));
    assert!(
        out.contains(
            "Notice: hash_update_file(): Read of 8192 bytes failed with errno=21 Is a directory"
        ) && out.ends_with("bool(false)\n"),
        "{out}"
    );
}
