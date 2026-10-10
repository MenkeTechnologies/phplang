//! End-to-end tests for the `hash` stdlib category: PHP source in, captured
//! `echo` output out. Expected values cross-checked against reference PHP 8,
//! with two deliberate exceptions, both verified against `php 8.5.9`:
//!
//!   * `hash_algos()` pins THIS engine's algorithms (the reference's relative
//!     order, minus the ones not implemented), not the reference's sixty-odd. It
//!     is a coverage statement about `src/stdlib/hash.rs`, and it is the one assertion in this file that would fail against the reference
//!     by design.
//!   * `ord(md5("", true))` omits the `Deprecated: ord(): Providing a string
//!     that is not one byte long is deprecated` the reference prints first.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn md5_known_vectors() {
    assert_eq!(
        run(r#"<?php echo md5("");"#),
        "d41d8cd98f00b204e9800998ecf8427e"
    );
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
    assert_eq!(
        run(r#"<?php echo hash("md5", "");"#),
        "d41d8cd98f00b204e9800998ecf8427e"
    );
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
    assert_eq!(
        run(&format!(r#"<?php echo hash("crc32b", "{dog}");"#)),
        "82a34642"
    );
    assert_eq!(
        run(&format!(r#"<?php echo hash("crc32", "{dog}");"#)),
        "413a86af"
    );
    assert_eq!(run(r#"<?php echo hash("crc32b", "");"#), "00000000");
    // Cross-check crc32() equals hexdec(hash('crc32b', …)).
    assert_eq!(
        run(&format!(
            r#"<?php echo crc32("{dog}") === hexdec(hash("crc32b","{dog}")) ? "y":"n";"#
        )),
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
    // PHP 8 ValueError text (not the PHP 7 "Unknown hashing algorithm") — and a
    // real, CATCHABLE `ValueError`, as the reference throws, rather than an
    // engine-level abort. Asserting through `catch` pins both the class and the
    // message; the uncaught rendering (with the `#0 hash('bogus', 'x')` internal
    // frame) is covered in tests/fatal_errors.rs.
    assert_eq!(
        run(r#"<?php try { hash("bogus", "x"); }
               catch (ValueError $e) { echo get_class($e), "|", $e->getMessage(); }"#),
        "ValueError|hash(): Argument #1 ($algo) must be a valid hashing algorithm"
    );
    assert_eq!(
        run(r#"<?php try { hash_hmac("bogus", "x", "k"); }
               catch (ValueError $e) { echo get_class($e), "|", $e->getMessage(); }"#),
        "ValueError|hash_hmac(): Argument #1 ($algo) must be a valid cryptographic \
         hashing algorithm"
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
        "md4,md5,sha1,sha224,sha256,sha384,sha512/224,sha512/256,sha512,sha3-224,sha3-256,\
         sha3-384,sha3-512,adler32,crc32,crc32b,crc32c,fnv132,fnv1a32,fnv164,fnv1a64,joaat"
    );
    assert_eq!(
        run(r#"<?php echo in_array("sha512", hash_algos()) ? "y":"n";"#),
        "y"
    );
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
#[test]
fn digests_beyond_the_core_set_match_reference() {
    // (algo, input, digest) read off reference php 8.5. The 135-byte input sits one
    // byte under the sha3-256 rate, so its padding byte is 0x06 | 0x80.
    let long = "x".repeat(135);
    let cases: &[(&str, &str, &str)] = &[
        ("md4", "", "31d6cfe0d16ae931b73c59d7e0c089c0"),
        ("md4", "hello", "866437cb7a794bce2b727acc0362ee27"),
        ("md4", &long, "be0e505ce33f9ee5ddf247bda7c64035"),
        ("sha224", "", "d14a028c2a3a2bc9476102bb288234c415a2b01f828ea62ac5b3e42f"),
        ("sha224", "hello", "ea09ae9cc6768c50fcee903ed054556e5bfc8347907f12598aa24193"),
        ("sha224", &long, "c146edc094e2e9327b8d48fb2e37189359c83a2f5fb1e04d32d26d77"),
        ("sha512/224", "", "6ed0dd02806fa89e25de060c19d3ac86cabb87d6a0ddd05c333b84f4"),
        ("sha512/224", "hello", "fe8509ed1fb7dcefc27e6ac1a80eddbec4cb3d2c6fe565244374061c"),
        ("sha512/224", &long, "285f136d2decefb83c42d694e43b76910840a2ee927c6fc25e13e2e6"),
        ("sha512/256", "", "c672b8d1ef56ed28ab87c3622c5114069bdd3ad7b8f9737498d0c01ecef0967a"),
        ("sha512/256", "hello", "e30d87cfa2a75db545eac4d61baf970366a8357c7f72fa95b52d0accb698f13a"),
        ("sha512/256", &long, "fd4e56a487589d7488dd1fe347cd806820f798dcf7deeb191e5f00593ec08728"),
        ("sha3-224", "", "6b4e03423667dbb73b6e15454f0eb1abd4597f9a1b078e3f5b5a6bc7"),
        ("sha3-224", "hello", "b87f88c72702fff1748e58b87e9141a42c0dbedc29a78cb0d4a5cd81"),
        ("sha3-224", &long, "c89bfc35994db464160c9b93ac67c5b9dd32012bcadce75c4ed4548b"),
        ("sha3-256", "", "a7ffc6f8bf1ed76651c14756a061d662f580ff4de43b49fa82d80a4b80f8434a"),
        ("sha3-256", "hello", "3338be694f50c5f338814986cdf0686453a888b84f424d792af4b9202398f392"),
        ("sha3-256", &long, "c150125edc74b56fb5cbfdd024fabe20ea5a99bd3c97305bbf7cb55885c106fe"),
        ("sha3-384", "", "0c63a75b845e4f7d01107d852e4c2485c51a50aaaa94fc61995e71bbee983a2ac3713831264adb47fb6bd1e058d5f004"),
        ("sha3-384", "hello", "720aea11019ef06440fbf05d87aa24680a2153df3907b23631e7177ce620fa1330ff07c0fddee54699a4c3ee0ee9d887"),
        ("sha3-384", &long, "0183ceb1ae9b947885fa71b419e10cb384fe5a8084780c6bf684ede36d83470786a72c5333ae0aa472ab664aa5efeff4"),
        ("sha3-512", "", "a69f73cca23a9ac5c8b567dc185a756e97c982164fe25859e0d1dcc1475c80a615b2123af1f5f94c11e3e9402c3ac558f500199d95b6d3e301758586281dcd26"),
        ("sha3-512", "hello", "75d527c368f2efe848ecf6b073a36767800805e9eef2b1857d5f984f036eb6df891d75f72d9b154518c1cd58835286d1da9a38deba3de98b5a53e5ed78a84976"),
        ("sha3-512", &long, "8e6c97077cf3abcbeff3f6e9dfb54d30b21139d2cc91fa6eb8885fa06ef6179b70dd6101dab98351d8108e3298977f1ce264895cf9a13223dc5862e85c877cbb"),
        ("adler32", "", "00000001"),
        ("adler32", "hello", "062c0215"),
        ("adler32", &long, "d0973f49"),
        ("crc32c", "", "00000000"),
        ("crc32c", "hello", "9a71bb4c"),
        ("crc32c", &long, "ec4f4eac"),
        ("fnv132", "", "811c9dc5"),
        ("fnv132", "hello", "b6fa7167"),
        ("fnv132", &long, "faa820cf"),
        ("fnv1a32", "", "811c9dc5"),
        ("fnv1a32", "hello", "4f9f2cab"),
        ("fnv1a32", &long, "8a5d01df"),
        ("fnv164", "", "cbf29ce484222325"),
        ("fnv164", "hello", "7b495389bdbdd4c7"),
        ("fnv164", &long, "8b5a2fd5f71ea80f"),
        ("fnv1a64", "", "cbf29ce484222325"),
        ("fnv1a64", "hello", "a430d84680aabd0b"),
        ("fnv1a64", &long, "e508b853075d075f"),
        ("joaat", "", "00000000"),
        ("joaat", "hello", "c8fd181b"),
        ("joaat", &long, "3a530ae4"),
    ];
    for (algo, input, expected) in cases {
        let src = format!("<?php echo hash('{algo}', '{input}');");
        assert_eq!(run(&src), *expected, "hash({algo}, {input:?})");
    }
}

#[test]
fn hmac_and_pbkdf2_over_new_algorithms() {
    let long_key = "k".repeat(300);
    let cases: &[(&str, &str, &str, &str)] = &[
        ("md4", "dbb72bdb593dc1eeec298f0f83d7e380", "9c1e5cc31a2c9b860982c2d870f3a4b4", "60cffb796acc6ac864216a026d1e4e2b35a513cb"),
        ("sha224", "10476a2b9f1f7bc096417cee1655c9d36dfc8ec2dd2969b37b7c35c1", "4424d156c9e047cca091919142770fae90c680f7f0b0eb85ccd30f78", "26ada39be31f195500c2ef7890d39b8a230230af"),
        ("sha512/256", "1fc8dfa35bed730ced23ef2916316a7a96bc1cd8aba86e2f776495d63e927718", "0ac8272672c090a00977226d943b8343ab256248a5aa3df719d3a20ed9bd4fc8", "bab0fe07d3104d41954ec2444e7708d0b6302bf8"),
        ("sha3-256", "3f52197c35a65814440e4b01bdcfeda3d0a70430e7957f5e9156f1bebda99388", "2009d9da673912233f8c6e89d706785509f9fd2fd8e28424d6b70cbd52d2da67", "aed9faa5b67ac0547e5e2d5de5354f5b2be85356"),
        ("sha3-512", "7ea185b240d79aa18949de679381c0577fdb1dc07e0b564daba08d1d7170c17d3e71ccf53be0887fa56df7d76bd14d1d88a5787e5a21dc9815e5841d61d0e7d6", "23c713b6217ce4404ed69c6930df94873c57f135a4acfadb30b0cbe4cdc5b4bd7d7aae2916a4263214d1c5ace43fabd6447dbaea72e1794a6647234a616f7833", "79c1e3b3e1fd2148eb6c32f68e5d39adb0924b25"),
    ];
    for (algo, short, long, derived) in cases {
        let src = format!(
            "<?php echo hash_hmac('{algo}', 'data', 'k'), '|', hash_hmac('{algo}', 'data', '{long_key}'), '|', hash_pbkdf2('{algo}', 'pw', 'salt', 3, 40);"
        );
        assert_eq!(run(&src), format!("{short}|{long}|{derived}"), "{algo}");
    }
}

#[test]
fn hmac_refuses_non_cryptographic_algorithms() {
    for (f, call) in [
        ("hash_hmac", "hash_hmac('crc32', 'a', 'b')"),
        ("hash_pbkdf2", "hash_pbkdf2('adler32', 'a', 'b', 1)"),
    ] {
        let src =
            format!("<?php try {{ {call}; }} catch (ValueError $e) {{ echo $e->getMessage(); }}");
        assert_eq!(
            run(&src),
            format!("{f}(): Argument #1 ($algo) must be a valid cryptographic hashing algorithm")
        );
    }
}
