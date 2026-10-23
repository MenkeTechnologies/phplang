//! End-to-end tests for the `hash` stdlib category: PHP source in, captured
//! `echo` output out. Expected values cross-checked against reference PHP 8,
//! with two deliberate exceptions, both verified against `php 8.5.9`:
//!
//!   * `hash_algos()` pins THIS engine's algorithms (the reference's relative
//!     order, minus the ones not implemented), not the reference's sixty-odd. It
//!     is a coverage statement about `src/stdlib/hash.rs`, and it is the one
//!     assertion in this file that would fail against the reference by design.
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
        "md2,md4,md5,sha1,sha224,sha256,sha384,sha512/224,sha512/256,sha512,sha3-224,\
         sha3-256,sha3-384,sha3-512,ripemd128,ripemd160,ripemd256,ripemd320,whirlpool,\
         tiger128,3,tiger160,3,tiger192,3,gost,gost-crypto,adler32,crc32,crc32b,crc32c,\
         fnv132,fnv1a32,fnv164,fnv1a64,joaat,murmur3a,murmur3c,murmur3f,xxh32,xxh64,xxh3,\
         xxh128"
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

#[test]
fn hash_hmac_algos_lists_the_cryptographic_algorithms() {
    // php 8.5.12 lists these same 26 first, then the `tiger*,4`, `snefru*` and
    // `haval*` variants this engine does not implement.
    assert_eq!(
        run(r#"<?php echo implode(",", hash_hmac_algos());"#),
        "md2,md4,md5,sha1,sha224,sha256,sha384,sha512/224,sha512/256,sha512,sha3-224,\
         sha3-256,sha3-384,sha3-512,ripemd128,ripemd160,ripemd256,ripemd320,whirlpool,\
         tiger128,3,tiger160,3,tiger192,3,gost,gost-crypto"
    );
}

#[test]
fn ripemd_whirlpool_tiger_gost_murmur_xxh_digests_match_reference() {
    // (algo, input, digest) read off reference php 8.5.12. The 135-byte input
    // spans more than one block of every algorithm here and leaves a tail, so
    // the block loop, the padding and (for murmur/xxh) the tail mix all run.
    let long = "x".repeat(135);
    let cases: &[(&str, &str, &str)] = &[
        ("md2", "", "8350e5a3e24c153df2275c9f80692773"),
        ("md2", "hello", "a9046c73e00331af68917d3804f70655"),
        ("md2", &long, "ba29fc236ad56995c87c664c4274f828"),
        ("ripemd128", "", "cdf26213a150dc3ecb610f18f6b38b46"),
        ("ripemd128", "hello", "789d569f08ed7055e94b4289a4195012"),
        ("ripemd128", &long, "56d40d9c8894f0ce35faff99000d0908"),
        ("ripemd160", "", "9c1185a5c5e9fc54612808977ee8f548b2258d31"),
        ("ripemd160", "hello", "108f07b8382412612c048d07d13f814118445acd"),
        ("ripemd160", &long, "db84a5b8cd2d38b8c9d6538f612e405d61bbd04d"),
        ("ripemd256", "", "02ba4c4e5f8ecd1877fc52d64d30e37a2d9774fb1e5d026380ae0168e3c5522d"),
        ("ripemd256", "hello", "cc1d2594aece0a064b7aed75a57283d9490fd5705ed3d66bf9adfe3a58b25de5"),
        ("ripemd256", &long, "124cb6caa1e47b075a2febabaec7e159f2ad4b9233a31b94d0de39e0c5b9f42e"),
        ("ripemd320", "", "22d65d5661536cdc75c1fdf5c6de7b41b9f27325ebc61e8557177d705a0ec880151c3a32a00899b8"),
        ("ripemd320", "hello", "eb0cf45114c56a8421fbcb33430fa22e0cd607560a88bbe14ce70bdf59bf55b11a3906987c487992"),
        ("ripemd320", &long, "925d5dd4b13d94fcf9b6ed8852aacef3f3210fdbbe025b3ff6b3028a69b6ab8a7af4a27942f80495"),
        ("whirlpool", "", "19fa61d75522a4669b44e39c1d2e1726c530232130d407f89afee0964997f7a73e83be698b288febcf88e3e03c4f0757ea8964e59b63d93708b138cc42a66eb3"),
        ("whirlpool", "hello", "0a25f55d7308eca6b9567a7ed3bd1b46327f0f1ffdc804dd8bb5af40e88d78b88df0d002a89e2fdbd5876c523f1b67bc44e9f87047598e7548298ea1c81cfd73"),
        ("whirlpool", &long, "afbff24ecfb53fe1e1b2c772c04915280d6a61fdf222e6a5522a14621987fa4f6efb4d4e78573bbf3e7aefdce33705061de770ccfea97a0ac3abfe09e1954704"),
        ("tiger128,3", "", "3293ac630c13f0245f92bbb1766e1616"),
        ("tiger128,3", "hello", "2cfd7f6f336288a7f2741b9bf874388a"),
        ("tiger128,3", &long, "416083809a44df0960f34d0f364dbd0d"),
        ("tiger160,3", "", "3293ac630c13f0245f92bbb1766e16167a4e5849"),
        ("tiger160,3", "hello", "2cfd7f6f336288a7f2741b9bf874388a54026639"),
        ("tiger160,3", &long, "416083809a44df0960f34d0f364dbd0dcbd2fe73"),
        ("tiger192,3", "", "3293ac630c13f0245f92bbb1766e16167a4e58492dde73f3"),
        ("tiger192,3", "hello", "2cfd7f6f336288a7f2741b9bf874388a54026639cadb7bf2"),
        ("tiger192,3", &long, "416083809a44df0960f34d0f364dbd0dcbd2fe731db54c74"),
        ("gost", "", "ce85b99cc46752fffee35cab9a7b0278abb4c2d2055cff685af4912c49490f8d"),
        ("gost", "hello", "a7eb5d08ddf2363f1ea0317a803fcef81d33863c8b2f9f6d7d14951d229f4567"),
        ("gost", &long, "497baa4dee8d364d4c3b76cc210c6c2cb15bb98aeb5f60c95840ded58d1b8f56"),
        ("gost-crypto", "", "981e5f3ca30c841487830f84fb433e13ac1101569b9c13584ac483234cd656c0"),
        ("gost-crypto", "hello", "92ea6ddbaf40020df3651f278fd7151217a24aa8d22ebd2519cfd4d89e6450ea"),
        ("gost-crypto", &long, "da12e14e188c6f28ea3d4deb4933a5cc8fca9a8885b47311e21ceafdccd4b8ab"),
        ("murmur3a", "", "00000000"),
        ("murmur3a", "hello", "248bfa47"),
        ("murmur3a", &long, "21f4f36d"),
        ("murmur3c", "", "00000000000000000000000000000000"),
        ("murmur3c", "hello", "2b2444a0db91def79adb31b69adb31b6"),
        ("murmur3c", &long, "e6180fcb7c9c337d42ffddad42046bc1"),
        ("murmur3f", "", "00000000000000000000000000000000"),
        ("murmur3f", "hello", "cbd8a7b341bd9b025b1e906a48ae1d19"),
        ("murmur3f", &long, "55cf37cda30cf67cf5922319f2228fca"),
        ("xxh32", "", "02cc5d05"),
        ("xxh32", "hello", "fb0077f9"),
        ("xxh32", &long, "47fc636f"),
        ("xxh64", "", "ef46db3751d8e999"),
        ("xxh64", "hello", "26c7827d889f6da3"),
        ("xxh64", &long, "7aa3b5401fc03189"),
        ("xxh3", "", "2d06800538d394c2"),
        ("xxh3", "hello", "9555e8555c62dcfd"),
        ("xxh3", &long, "e154f968d7eb7a39"),
        ("xxh128", "", "99aa06d3014798d86001c324468d497f"),
        ("xxh128", "hello", "b5e9c1ad071b3e7fc779cfaa5e523818"),
        ("xxh128", &long, "b646d8f568690ac09ddf0324e8c213ba"),
    ];
    for (algo, input, expected) in cases {
        let src = format!("<?php echo hash('{algo}', '{input}');");
        assert_eq!(run(&src), *expected, "hash({algo}, {input:?})");
    }
}

#[test]
fn hmac_and_pbkdf2_over_ripemd_whirlpool_tiger_gost() {
    // The HMAC block sizes are ext/hash's: 16 for md2, 32 for gost, 64 for the
    // rest. The 300-byte key is longer than every block, so it is pre-hashed.
    let long_key = "k".repeat(300);
    let cases: &[(&str, &str, &str, &str)] = &[
        ("md2", "dfea6346b255d0a4da8a597c3f1a99fc", "c9737acdb74869033da7338558d25386", "e38ae910ce15f15e91af9fa308e39a5872c8eb9a"),
        ("ripemd160", "1e145b01da19d9b9fc95086d8352d2483cf8af4f", "5ec4daa0f0b4a6bf202e3f23354b538b24c9ab7b", "872082fba2e5e614e84d9290769acc835cdf6bf3"),
        ("ripemd320", "d813deb95596f4adcad31b16851c4af9c6ef9c13adb49f47c2f0f663b0fe4a7c7163e5ef922f2b5d", "ba0e6f56672a524d98133e57db79c75d4d59dee3b76876b4c5fc2e672aa200af860575da2033e4cf", "634f15464c33cff047e700fcd654ebca762ab274"),
        ("whirlpool", "1ee909beed9c6b00e02f8f7a379a49bb91412730bea51c86b6312b75e4f71f865a140fa82852c3db5cf6099beda82b59677a60ef149319b0d2a67082d80ccb97", "b3eb0298e461d6d80fd294b13417198e0b98ca01a6bead92a75b22cf50c440af7c8179df16fefec326161104699c8a1fc887b980c0083b87480a5f1eb35861fb", "8cb2f6f2593c27e03acb5a68cad09097b7a1c238"),
        ("tiger192,3", "77641c3371aa5a2511a878312a3338de9d04dc58aa5ab03e", "7b4acc20d1e6784b0c1b7ddc3235f7f82e79fe55a9bbcedc", "2ea3a8aeeab181e12009971b7de988fc1a7c167f"),
        ("gost", "aad9c21c2434a4a20eddf3a8dd5d5de201b2ad1e3ebdcff57a7b2dced7ffb818", "5b89f5d6fbc2510fc585a2b49f6de6d004943e9d2e23e6fccd2775bed7666d9f", "83d63fc3f469c55bf9afa265748b74dc3e8df54a"),
        ("gost-crypto", "f0a882bb3cecab67cb455a2ce711ac9648b91c339c24bf52691b40daa8ead6e4", "6ed6d2617447db82929c39a626853f634ef0b41fd36d56753ac568254525dbf6", "6eddfe39108b4fc084c614ac10ae69ebe11207b1"),
    ];
    for (algo, short, long, derived) in cases {
        let src = format!(
            "<?php echo hash_hmac('{algo}', 'data', 'k'), '|', hash_hmac('{algo}', 'data', '{long_key}'), '|', hash_pbkdf2('{algo}', 'pw', 'salt', 3, 40);"
        );
        assert_eq!(run(&src), format!("{short}|{long}|{derived}"), "{algo}");
    }
}

#[test]
fn murmur_and_xxh_read_the_seed_option() {
    // php 8.5.12. Each seed is narrowed as its C init narrows `Z_LVAL`: murmur3a,
    // murmur3c and xxh32 to 32 bits (so -1 and 2**40 differ from 42 but 2**40
    // is seed 0), murmur3f, xxh64, xxh3 and xxh128 keep all 64.
    let cases: &[(&str, &str, &str, &str)] = &[
        ("murmur3a", "4e4f1e68", "fc80c2af", "b3dd93fa"),
        (
            "murmur3c",
            "5bcf0fbe8741bbe98741bbe98741bbe9",
            "d110201cb5400af6b5400af6b5400af6",
            "75cdc6d1a2b006a5a2b006a5a2b006a5",
        ),
        (
            "murmur3f",
            "0d85089fb3cff7d67510712b42353d30",
            "eb585078eed87597b99b158c7b34431b",
            "e31430ed32eb7e9c44bfab8765602d99",
        ),
        ("xxh32", "0147abfe", "b22b1420", "32d153ff"),
        (
            "xxh64",
            "13c1d910702770e6",
            "28306e589cc02176",
            "86a3a26f1058d383",
        ),
        (
            "xxh3",
            "d8438def21bbdcc3",
            "291c3db09146c9c9",
            "b721c5e458954479",
        ),
        (
            "xxh128",
            "4bc24859f045e0b4d8438def21bbdcc3",
            "c3e8eb7686bbf97d291c3db09146c9c9",
            "fca6ac31fd276414b721c5e458954479",
        ),
    ];
    for (algo, s42, s_neg1, s_2_40) in cases {
        let src = format!(
            r#"<?php foreach ([42, -1, 2 ** 40] as $s) echo hash("{algo}", "abc", false, ["seed" => $s]), "|";"#
        );
        assert_eq!(run(&src), format!("{s42}|{s_neg1}|{s_2_40}|"), "{algo}");
    }
    // Algorithms without a seeded init ignore the option entirely.
    assert_eq!(
        run(r#"<?php echo hash("md5", "abc", false, ["seed" => 42]);"#),
        "900150983cd24fb0d6963f7d28e17f72"
    );
}

#[test]
fn a_seed_that_is_not_an_int_is_deprecated() {
    // The two wordings are the reference's: murmur and xxh32/64 fall back to
    // seed 0, xxh3/xxh128 ignore the option.
    let out = run(r#"<?php echo hash("xxh32", "abc", false, ["seed" => "42"]), "|";"#);
    assert!(
        out.contains(
            "Deprecated: hash(): Passing a seed of a type other than int is deprecated \
             because it is the same as setting the seed to 0"
        ) && out.ends_with("32d153ff|"),
        "{out}"
    );
    let out = run(r#"<?php echo hash("xxh3", "abc", false, ["seed" => 1.5]), "|";"#);
    assert!(
        out.contains(
            "Deprecated: hash(): Passing a seed of a type other than int is deprecated \
             because it is ignored"
        ) && out.ends_with("78af5f94892f3950|"),
        "{out}"
    );
}

#[test]
fn xxh3_and_xxh128_read_the_secret_option() {
    // php 8.5.12: a secret of 136 bytes or more replaces the default one; more
    // than 256 is cut to 256 with a warning, so a 300-byte secret digests as its
    // first 256 bytes.
    assert_eq!(
        run(
            r#"<?php echo hash("xxh3", "abc", false, ["secret" => str_repeat("x", 136)]), "|",
                     hash("xxh128", "abc", false, ["secret" => str_repeat("x", 136)]);"#
        ),
        "fd6e986c28fac4d3|42d23058767a303ffd6e986c28fac4d3"
    );
    let out = run(
        r#"<?php echo hash("xxh128", "abc", false, ["secret" => str_repeat("y", 300)]) ===
                     hash("xxh128", "abc", false, ["secret" => str_repeat("y", 256)]) ? "same" : "differ";"#,
    );
    assert!(
        out.contains("Warning: hash(): xxh128: Secret content exceeding 256 bytes discarded")
            && out.ends_with("same"),
        "{out}"
    );
    let caught = |opts: &str| {
        run(&format!(
            r#"<?php try {{ hash("xxh3", "abc", false, {opts}); }} catch (Error $e) {{ echo get_class($e), ": ", $e->getMessage(); }}"#
        ))
    };
    assert_eq!(
        caught(r#"["secret" => "short"]"#),
        "Error: xxh3: Secret length must be >= 136 bytes, 5 bytes passed"
    );
    assert_eq!(
        caught(r#"["secret" => str_repeat("x", 136), "seed" => 1]"#),
        "Error: xxh3: Only one of seed or secret is to be passed for initialization"
    );
}
