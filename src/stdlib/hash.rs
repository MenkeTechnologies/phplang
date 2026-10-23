//! PHP standard-library `hash` functions. Part of the `stdlib` chain; see
//! `src/stdlib/mod.rs`. `dispatch` returns `None` for names it does not handle.
//!
//! Digests are produced by the vetted RustCrypto crates (`md-5`, `sha1`,
//! `sha2`, `md2`, `ripemd`, `whirlpool`, `tiger`, `gost94`), `crc32fast`,
//! `xxhash-rust` and `murmur3`. Hand-written here: the thin HMAC construction,
//! MD4, SHA-3, the small checksums, and MurmurHash3 x64_128 (whose 64-bit
//! seed the `murmur3` crate cannot take).
//!
//! Byte model: phplang strings are UTF-8 `String`s, so a hash's input is read
//! as the UTF-8 bytes of the argument (matching the core `bin2hex`), and a
//! `raw_output` digest is mapped byte→codepoint (Latin-1), matching the core
//! `chr`/`ord` byte handling. Hex output is the common, fully faithful path.

use crate::host::with_host;
use crate::stdlib::common::{arg, str_arg, throws};
use fusevm::Value;

/// Dispatch a `hash`-category PHP function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    Some(match name {
        "md5" => Ok(digest_fn(md5_bytes, args)),
        "sha1" => Ok(digest_fn(sha1_bytes, args)),
        "crc32" => Ok(Value::int(crc32b(&input_bytes(args, 0)) as i64)),
        "hash" => php_hash(args),
        "hash_hmac" => php_hash_hmac(args),
        "hash_algos" => Ok(algo_names(|_| true)),
        "hash_hmac_algos" => Ok(algo_names(|a| a.block.is_some())),
        _ => return None,
    })
}

/// Byte string of the `i`-th argument (UTF-8 bytes of the PHP string cast).
fn input_bytes(args: &[Value], i: usize) -> Vec<u8> {
    crate::bytes::to_bytes(&str_arg(args, i)).into_owned()
}

/// PHP boolean truthiness of the `i`-th argument, defaulting to false.
fn bool_arg(args: &[Value], i: usize) -> bool {
    with_host(|h| h.is_truthy(&arg(args, i)))
}

/// Wrap raw digest bytes into a PHP `Value`: lowercase hex, or the binary
/// string itself when `raw` is set.
fn wrap(bytes: Vec<u8>, raw: bool) -> Value {
    if raw {
        Value::str(crate::bytes::from_bytes(&bytes))
    } else {
        Value::str(hex::encode(bytes))
    }
}

/// Shared body of `md5`/`sha1`: hash arg 0, honor the `raw_output` flag at arg 1.
fn digest_fn(f: fn(&[u8]) -> Vec<u8>, args: &[Value]) -> Value {
    wrap(f(&input_bytes(args, 0)), bool_arg(args, 1))
}

/// `hash(algo, data, binary = false)`.
fn php_hash(args: &[Value]) -> Result<Value, String> {
    let algo = str_arg(args, 0).to_ascii_lowercase();
    let data = input_bytes(args, 1);
    let raw = bool_arg(args, 2);
    match find(&algo) {
        Some(a) => {
            let init = read_options("hash", a.name, &arg(args, 3))?;
            Ok(wrap(digest_with(a, &data, &init), raw))
        }
        // PHP 8 throws a ValueError with this exact message for an unknown algo
        // (replacing the PHP 7 "Unknown hashing algorithm" wording).
        None => Err(throws(
            "ValueError",
            "hash(): Argument #1 ($algo) must be a valid hashing algorithm",
        )),
    }
}

/// `hash_hmac(algo, data, key, binary = false)` for every cryptographic algorithm
/// in [`ALGOS`].
fn php_hash_hmac(args: &[Value]) -> Result<Value, String> {
    let algo = str_arg(args, 0).to_ascii_lowercase();
    let data = input_bytes(args, 1);
    let key = input_bytes(args, 2);
    let raw = bool_arg(args, 3);
    match find(&algo).and_then(|a| a.block.map(|b| (a.digest, b))) {
        Some((f, block_size)) => Ok(wrap(hmac(block_size, f, &key, &data), raw)),
        // PHP 8 throws a ValueError with this exact message for an unknown algo.
        None => Err(throws(
            "ValueError",
            "hash_hmac(): Argument #1 ($algo) must be a valid cryptographic \
             hashing algorithm",
        )),
    }
}

/// One registered digest: PHP's name for it, the digest function, and the HMAC
/// block size, `None` for the non-cryptographic checksums, which `hash_hmac` and
/// `hash_pbkdf2` refuse.
pub(crate) struct Algo {
    pub name: &'static str,
    pub digest: fn(&[u8]) -> Vec<u8>,
    pub block: Option<usize>,
}

const fn algo(name: &'static str, digest: fn(&[u8]) -> Vec<u8>, block: Option<usize>) -> Algo {
    Algo {
        name,
        digest,
        block,
    }
}

/// Every digest this runtime implements, in the order `hash_algos()` lists them
/// (the reference's relative order). The single table `hash`, `hash_hmac`,
/// `hash_file`, `hash_pbkdf2` and `hash_algos` all read.
pub(crate) const ALGOS: &[Algo] = &[
    algo("md2", crate_digest::<md2::Md2>, Some(16)),
    algo("md4", md4_bytes, Some(64)),
    algo("md5", md5_bytes, Some(64)),
    algo("sha1", sha1_bytes, Some(64)),
    algo("sha224", sha224_bytes, Some(64)),
    algo("sha256", sha256_bytes, Some(64)),
    algo("sha384", sha384_bytes, Some(128)),
    algo("sha512/224", sha512_224_bytes, Some(128)),
    algo("sha512/256", sha512_256_bytes, Some(128)),
    algo("sha512", sha512_bytes, Some(128)),
    algo("sha3-224", |d| sha3(d, 28), Some(144)),
    algo("sha3-256", |d| sha3(d, 32), Some(136)),
    algo("sha3-384", |d| sha3(d, 48), Some(104)),
    algo("sha3-512", |d| sha3(d, 64), Some(72)),
    algo("ripemd128", crate_digest::<ripemd::Ripemd128>, Some(64)),
    algo("ripemd160", crate_digest::<ripemd::Ripemd160>, Some(64)),
    algo("ripemd256", crate_digest::<ripemd::Ripemd256>, Some(64)),
    algo("ripemd320", crate_digest::<ripemd::Ripemd320>, Some(64)),
    algo("whirlpool", crate_digest::<whirlpool::Whirlpool>, Some(64)),
    // tigerN,3: the leading N/8 bytes of the 3-pass Tiger digest
    // (`PHP_TIGER_DIGEST`). The 4-pass variants and snefru are not implemented.
    algo("tiger128,3", |d| tiger3(d, 16), Some(64)),
    algo("tiger160,3", |d| tiger3(d, 20), Some(64)),
    algo("tiger192,3", |d| tiger3(d, 24), Some(64)),
    // gost: GOST R 34.11-94 under the test S-box; gost-crypto under CryptoPro's.
    algo("gost", crate_digest::<gost94::Gost94Test>, Some(32)),
    algo(
        "gost-crypto",
        crate_digest::<gost94::Gost94CryptoPro>,
        Some(32),
    ),
    algo("adler32", |d| adler32(d).to_be_bytes().to_vec(), None),
    // crc32: PHP's non-reflected CRC-32/BZIP2 variant, rendered in reversed
    // (little-endian) byte order — a long-standing quirk that distinguishes it
    // from crc32b beyond the polynomial itself.
    algo("crc32", |d| crc32_bzip2(d).to_le_bytes().to_vec(), None),
    // crc32b: reflected CRC-32/ISO-HDLC, matching PHP's crc32() value.
    algo("crc32b", |d| crc32b(d).to_be_bytes().to_vec(), None),
    algo("crc32c", |d| crc32c(d).to_be_bytes().to_vec(), None),
    algo("fnv132", |d| fnv32(d, false).to_be_bytes().to_vec(), None),
    algo("fnv1a32", |d| fnv32(d, true).to_be_bytes().to_vec(), None),
    algo("fnv164", |d| fnv64(d, false).to_be_bytes().to_vec(), None),
    algo("fnv1a64", |d| fnv64(d, true).to_be_bytes().to_vec(), None),
    algo("joaat", |d| joaat(d).to_be_bytes().to_vec(), None),
    algo("murmur3a", |d| murmur3a(d, 0), None),
    algo("murmur3c", |d| murmur3c(d, 0), None),
    algo("murmur3f", |d| murmur3f(d, 0), None),
    algo("xxh32", |d| xxh32(d, 0), None),
    algo("xxh64", |d| xxh64(d, 0), None),
    algo("xxh3", |d| xxh3(d, &Init::Default), None),
    algo("xxh128", |d| xxh128(d, &Init::Default), None),
];

/// The registered digest named `name` (already lowercased).
pub(crate) fn find(name: &str) -> Option<&'static Algo> {
    ALGOS.iter().find(|a| a.name == name)
}

/// `hash_algos()` / `hash_hmac_algos()`: the names in [`ALGOS`] that `keep`
/// accepts, in order (`hash_hmac_algos` lists the cryptographic ones).
fn algo_names(keep: fn(&Algo) -> bool) -> Value {
    with_host(|h| {
        let arr = h.new_array();
        for a in ALGOS.iter().filter(|a| keep(a)) {
            h.arr_push_auto(&arr, Value::str(a.name.to_string()));
        }
        arr
    })
}

pub(crate) fn md5_bytes(data: &[u8]) -> Vec<u8> {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(data);
    h.finalize().to_vec()
}

pub(crate) fn sha1_bytes(data: &[u8]) -> Vec<u8> {
    use sha1::{Digest, Sha1};
    let mut h = Sha1::new();
    h.update(data);
    h.finalize().to_vec()
}

pub(crate) fn sha256_bytes(data: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().to_vec()
}

pub(crate) fn sha384_bytes(data: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha384};
    let mut h = Sha384::new();
    h.update(data);
    h.finalize().to_vec()
}

pub(crate) fn sha512_bytes(data: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha512};
    let mut h = Sha512::new();
    h.update(data);
    h.finalize().to_vec()
}

/// CRC-32/ISO-HDLC (reflected, zlib) — the value PHP's `crc32()` returns and
/// `hash('crc32b', …)` renders in hex.
pub(crate) fn crc32b(data: &[u8]) -> u32 {
    let mut h = crc32fast::Hasher::new();
    h.update(data);
    h.finalize()
}

/// CRC-32/BZIP2 (non-reflected) — PHP's `hash('crc32', …)` variant. No
/// dependency provides it, so it is computed bitwise. Parameters:
/// poly=0x04C11DB7, init/xorout=0xFFFFFFFF, refin=refout=false.
fn crc32_bzip2(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= (byte as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc ^ 0xFFFF_FFFF
}

/// Generic HMAC per RFC 2104: H((K⊕opad) ‖ H((K⊕ipad) ‖ msg)).
pub(crate) fn hmac(block_size: usize, f: fn(&[u8]) -> Vec<u8>, key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut k = if key.len() > block_size {
        f(key)
    } else {
        key.to_vec()
    };
    k.resize(block_size, 0);
    let mut inner: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    inner.extend_from_slice(msg);
    let ih = f(&inner);
    let mut outer: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    outer.extend_from_slice(&ih);
    f(&outer)
}

fn sha224_bytes(data: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha224};
    Sha224::digest(data).to_vec()
}

fn sha512_224_bytes(data: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha512_224};
    Sha512_224::digest(data).to_vec()
}

fn sha512_256_bytes(data: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha512_256};
    Sha512_256::digest(data).to_vec()
}

/// RFC 1320 MD4.
fn md4_bytes(data: &[u8]) -> Vec<u8> {
    let mut state: [u32; 4] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64).wrapping_mul(8)).to_le_bytes());
    for block in msg.chunks_exact(64) {
        let x: Vec<u32> = block
            .chunks_exact(4)
            .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
            .collect();
        let [mut a, mut b, mut c, mut d] = state;
        // Round 1: F = (b & c) | (!b & d), word order 0..15.
        for i in 0..16 {
            let f = (b & c) | (!b & d);
            let s = [3, 7, 11, 19][i % 4];
            let t = a.wrapping_add(f).wrapping_add(x[i]).rotate_left(s);
            (a, b, c, d) = (d, t, b, c);
        }
        // Round 2: G = majority, word order column-major, constant 0x5a827999.
        for i in 0..16 {
            let g = (b & c) | (b & d) | (c & d);
            let s = [3, 5, 9, 13][i % 4];
            let k = (i % 4) * 4 + i / 4;
            let t = a
                .wrapping_add(g)
                .wrapping_add(x[k])
                .wrapping_add(0x5a82_7999)
                .rotate_left(s);
            (a, b, c, d) = (d, t, b, c);
        }
        // Round 3: H = parity, bit-reversed word order, constant 0x6ed9eba1.
        for (i, &k) in [0, 8, 4, 12, 2, 10, 6, 14, 1, 9, 5, 13, 3, 11, 7, 15]
            .iter()
            .enumerate()
        {
            let h = b ^ c ^ d;
            let s = [3, 9, 11, 15][i % 4];
            let t = a
                .wrapping_add(h)
                .wrapping_add(x[k])
                .wrapping_add(0x6ed9_eba1)
                .rotate_left(s);
            (a, b, c, d) = (d, t, b, c);
        }
        let [sa, sb, sc, sd] = &mut state;
        *sa = sa.wrapping_add(a);
        *sb = sb.wrapping_add(b);
        *sc = sc.wrapping_add(c);
        *sd = sd.wrapping_add(d);
    }
    state.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// Keccak-f[1600] permutation (FIPS 202 §3.3 – 3.4).
fn keccak_f(a: &mut [u64; 25]) {
    const RC: [u64; 24] = [
        0x0000_0000_0000_0001,
        0x0000_0000_0000_8082,
        0x8000_0000_0000_808a,
        0x8000_0000_8000_8000,
        0x0000_0000_0000_808b,
        0x0000_0000_8000_0001,
        0x8000_0000_8000_8081,
        0x8000_0000_0000_8009,
        0x0000_0000_0000_008a,
        0x0000_0000_0000_0088,
        0x0000_0000_8000_8009,
        0x0000_0000_8000_000a,
        0x0000_0000_8000_808b,
        0x8000_0000_0000_008b,
        0x8000_0000_0000_8089,
        0x8000_0000_0000_8003,
        0x8000_0000_0000_8002,
        0x8000_0000_0000_0080,
        0x0000_0000_0000_800a,
        0x8000_0000_8000_000a,
        0x8000_0000_8000_8081,
        0x8000_0000_0000_8080,
        0x0000_0000_8000_0001,
        0x8000_0000_8000_8008,
    ];
    const ROT: [u32; 24] = [
        1, 3, 6, 10, 15, 21, 28, 36, 45, 55, 2, 14, 27, 41, 56, 8, 25, 43, 62, 18, 39, 61, 20, 44,
    ];
    const PI: [usize; 24] = [
        10, 7, 11, 17, 18, 3, 5, 16, 8, 21, 24, 4, 15, 23, 19, 13, 12, 2, 20, 14, 22, 9, 6, 1,
    ];
    for rc in RC {
        let mut c = [0u64; 5];
        for x in 0..5 {
            c[x] = a[x] ^ a[x + 5] ^ a[x + 10] ^ a[x + 15] ^ a[x + 20];
        }
        for x in 0..5 {
            let d = c[(x + 4) % 5] ^ c[(x + 1) % 5].rotate_left(1);
            for y in 0..5 {
                a[x + 5 * y] ^= d;
            }
        }
        let mut t = a[1];
        for i in 0..24 {
            let j = PI[i];
            let next = a[j];
            a[j] = t.rotate_left(ROT[i]);
            t = next;
        }
        for y in 0..5 {
            let row: [u64; 5] = std::array::from_fn(|x| a[x + 5 * y]);
            for x in 0..5 {
                a[x + 5 * y] = row[x] ^ (!row[(x + 1) % 5] & row[(x + 2) % 5]);
            }
        }
        a[0] ^= rc;
    }
}

/// SHA-3 (FIPS 202): Keccak sponge, rate `200 - 2 * out_len`, domain byte 0x06.
fn sha3(data: &[u8], out_len: usize) -> Vec<u8> {
    let rate = 200 - 2 * out_len;
    let mut padded = data.to_vec();
    padded.push(0x06);
    while padded.len() % rate != 0 {
        padded.push(0);
    }
    *padded.last_mut().expect("padded is non-empty") |= 0x80;
    let mut state = [0u64; 25];
    for block in padded.chunks_exact(rate) {
        for (i, lane) in block.chunks_exact(8).enumerate() {
            state[i] ^= u64::from_le_bytes(lane.try_into().expect("8-byte lane"));
        }
        keccak_f(&mut state);
    }
    state
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .take(out_len)
        .collect()
}

/// Adler-32 (RFC 1950 §8.2).
fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// CRC-32C (Castagnoli), reflected, bitwise.
fn crc32c(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0x82F6_3B78
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// FNV-1 / FNV-1a, 32 bit.
fn fnv32(data: &[u8], a_variant: bool) -> u32 {
    data.iter().fold(0x811c_9dc5u32, |h, &b| {
        if a_variant {
            (h ^ b as u32).wrapping_mul(0x0100_0193)
        } else {
            h.wrapping_mul(0x0100_0193) ^ b as u32
        }
    })
}

/// FNV-1 / FNV-1a, 64 bit.
fn fnv64(data: &[u8], a_variant: bool) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        if a_variant {
            (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)
        } else {
            h.wrapping_mul(0x0000_0100_0000_01b3) ^ b as u64
        }
    })
}

/// Bob Jenkins' one-at-a-time hash.
fn joaat(data: &[u8]) -> u32 {
    let mut h = 0u32;
    for &b in data {
        h = h.wrapping_add(b as u32);
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}

/// One-shot digest through any RustCrypto `Digest` (the `md2`, `ripemd`,
/// `whirlpool` and `gost94` crates).
fn crate_digest<D: sha2::Digest>(data: &[u8]) -> Vec<u8> {
    D::digest(data).to_vec()
}

/// 3-pass Tiger, truncated to `len` bytes as `PHP_TIGER_DIGEST` copies the
/// state out for `tiger128,3` / `tiger160,3`.
fn tiger3(data: &[u8], len: usize) -> Vec<u8> {
    let mut d = crate_digest::<tiger::Tiger>(data);
    d.truncate(len);
    d
}

// ── The seeded families: `murmur3*` and `xxh*` ────────────────────────────────
//
// These are the only algorithms whose ext/hash `hash_init` op reads the
// `$options` array (`hash_murmur.c`, `hash_xxhash.c`). Every digest is written
// as `XXH*_canonicalFromHash` / the murmur finals write it: each state word
// big-endian, first word first.

/// What `$options` initialised a seeded algorithm's state with.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Init {
    /// No usable option: seed 0 (and the default secret for `xxh3`/`xxh128`).
    Default,
    /// `["seed" => int]`, still the PHP int; each algorithm narrows it.
    Seed(i64),
    /// `["secret" => string]` for `xxh3`/`xxh128`, at most 256 bytes.
    Secret(Vec<u8>),
}

/// `XXH3_SECRET_SIZE_MIN`: the shortest secret xxHash accepts.
const XXH3_SECRET_SIZE_MIN: usize = 136;

/// `PHP_XXH3_SECRET_SIZE_MAX`: the bytes of a secret the context keeps.
const XXH3_SECRET_SIZE_MAX: usize = 256;

/// Port of the `$options` handling of `PHP_MURMUR3{A,C,F}_Init`,
/// `PHP_XXH{32,64}_Init` and `_PHP_XXH3_Init`. `fname` names the PHP function
/// the deprecations are reported against; `options` is the array (or undef
/// when the argument was not given). Algorithms that take no options ignore it.
pub(crate) fn read_options(fname: &str, algo: &str, options: &Value) -> Result<Init, String> {
    let pairs = with_host(|h| h.array_pairs(options)).unwrap_or_default();
    let find_opt = |name: &str| {
        pairs
            .iter()
            .find(|(k, _)| matches!(k, Value::Str(_)) && with_host(|h| h.to_str(k)) == name)
            .map(|(_, v)| v.clone())
    };
    let deprecated = |why: &str| {
        with_host(|h| h.deprecated(format!("{fname}(): Passing a {why}")));
    };
    match algo {
        "murmur3a" | "murmur3c" | "murmur3f" | "xxh32" | "xxh64" => match find_opt("seed") {
            Some(Value::Int(n)) => Ok(Init::Seed(n)),
            Some(_) => {
                deprecated(
                    "seed of a type other than int is deprecated because it is the same \
                     as setting the seed to 0",
                );
                Ok(Init::Default)
            }
            None => Ok(Init::Default),
        },
        "xxh3" | "xxh128" => {
            let seed = find_opt("seed");
            let secret = find_opt("secret");
            if seed.is_some() && secret.is_some() {
                return Err(throws(
                    "Error",
                    format!(
                        "{algo}: Only one of seed or secret is to be passed for initialization"
                    ),
                ));
            }
            match seed {
                Some(Value::Int(n)) => return Ok(Init::Seed(n)),
                Some(_) => {
                    deprecated("seed of a type other than int is deprecated because it is ignored")
                }
                None => {}
            }
            let Some(secret) = secret else {
                return Ok(Init::Default);
            };
            if !matches!(secret, Value::Str(_)) {
                deprecated(
                    "secret of a type other than string is deprecated because it implicitly \
                     converts to a string, potentially hiding bugs",
                );
            }
            let mut bytes = crate::bytes::to_bytes(&with_host(|h| h.to_str(&secret))).into_owned();
            if bytes.len() < XXH3_SECRET_SIZE_MIN {
                return Err(throws(
                    "Error",
                    format!(
                        "{algo}: Secret length must be >= {} bytes, {} bytes passed",
                        XXH3_SECRET_SIZE_MIN,
                        bytes.len()
                    ),
                ));
            }
            if bytes.len() > XXH3_SECRET_SIZE_MAX {
                bytes.truncate(XXH3_SECRET_SIZE_MAX);
                with_host(|h| {
                    h.warn(format!(
                        "{fname}(): {algo}: Secret content exceeding {XXH3_SECRET_SIZE_MAX} bytes discarded"
                    ))
                });
            }
            Ok(Init::Secret(bytes))
        }
        _ => Ok(Init::Default),
    }
}

/// Digest `data` with `algo` from the state `init` describes.
pub(crate) fn digest_with(algo: &Algo, data: &[u8], init: &Init) -> Vec<u8> {
    let seed = match init {
        Init::Seed(n) => *n,
        _ => 0,
    };
    match algo.name {
        // Every seed is narrowed the way its C init narrows `Z_LVAL`.
        "murmur3a" => murmur3a(data, seed as u32),
        "murmur3c" => murmur3c(data, seed as u32),
        "murmur3f" => murmur3f(data, seed as u64),
        "xxh32" => xxh32(data, seed as u32),
        "xxh64" => xxh64(data, seed as u64),
        "xxh3" => xxh3(data, init),
        "xxh128" => xxh128(data, init),
        _ => (algo.digest)(data),
    }
}

/// MurmurHash3 x86_32 (`murmur3a`).
fn murmur3a(data: &[u8], seed: u32) -> Vec<u8> {
    let h = murmur3::murmur3_32(&mut &data[..], seed).expect("reading a slice cannot fail");
    h.to_be_bytes().to_vec()
}

/// MurmurHash3 x86_128 (`murmur3c`): h1..h4, each big-endian.
fn murmur3c(data: &[u8], seed: u32) -> Vec<u8> {
    let h = murmur3::murmur3_x86_128(&mut &data[..], seed).expect("reading a slice cannot fail");
    (0..4)
        .flat_map(|i| ((h >> (32 * i)) as u32).to_be_bytes())
        .collect()
}

/// MurmurHash3 x64_128 (`murmur3f`): h1, h2, each big-endian. Ported from
/// `ext/hash/murmur/PMurHash128.c` rather than taken from the `murmur3` crate,
/// because `PHP_MURMUR3FInit` seeds both state words with the full 64-bit
/// `Z_LVAL` and the crate's seed is 32 bits.
fn murmur3f(data: &[u8], seed: u64) -> Vec<u8> {
    const C1: u64 = 0x87c3_7b91_1142_53d5;
    const C2: u64 = 0x4cf5_ad43_2745_937f;
    let mix_k1 = |k: u64| k.wrapping_mul(C1).rotate_left(31).wrapping_mul(C2);
    let mix_k2 = |k: u64| k.wrapping_mul(C2).rotate_left(33).wrapping_mul(C1);
    let fmix = |mut k: u64| {
        k ^= k >> 33;
        k = k.wrapping_mul(0xff51_afd7_ed55_8ccd);
        k ^= k >> 33;
        k = k.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        k ^ (k >> 33)
    };
    let le = |bytes: &[u8]| {
        bytes
            .iter()
            .rev()
            .fold(0u64, |acc, &b| acc << 8 | u64::from(b))
    };
    let (mut h1, mut h2) = (seed, seed);
    let mut blocks = data.chunks_exact(16);
    for block in &mut blocks {
        h1 ^= mix_k1(le(&block[..8]));
        h1 = h1
            .rotate_left(27)
            .wrapping_add(h2)
            .wrapping_mul(5)
            .wrapping_add(0x52dc_e729);
        h2 ^= mix_k2(le(&block[8..]));
        h2 = h2
            .rotate_left(31)
            .wrapping_add(h1)
            .wrapping_mul(5)
            .wrapping_add(0x3849_5ab5);
    }
    let tail = blocks.remainder();
    if tail.len() > 8 {
        h2 ^= mix_k2(le(&tail[8..]));
    }
    if !tail.is_empty() {
        h1 ^= mix_k1(le(&tail[..tail.len().min(8)]));
    }
    h1 ^= data.len() as u64;
    h2 ^= data.len() as u64;
    h1 = h1.wrapping_add(h2);
    h2 = h2.wrapping_add(h1);
    h1 = fmix(h1);
    h2 = fmix(h2);
    h1 = h1.wrapping_add(h2);
    h2 = h2.wrapping_add(h1);
    [h1.to_be_bytes(), h2.to_be_bytes()].concat()
}

fn xxh32(data: &[u8], seed: u32) -> Vec<u8> {
    xxhash_rust::xxh32::xxh32(data, seed).to_be_bytes().to_vec()
}

fn xxh64(data: &[u8], seed: u64) -> Vec<u8> {
    xxhash_rust::xxh64::xxh64(data, seed).to_be_bytes().to_vec()
}

fn xxh3(data: &[u8], init: &Init) -> Vec<u8> {
    use xxhash_rust::xxh3;
    match init {
        Init::Default => xxh3::xxh3_64(data),
        Init::Seed(n) => xxh3::xxh3_64_with_seed(data, *n as u64),
        Init::Secret(s) => xxh3::xxh3_64_with_secret(data, s),
    }
    .to_be_bytes()
    .to_vec()
}

/// XXH3-128, canonical form: the high 64 bits first.
fn xxh128(data: &[u8], init: &Init) -> Vec<u8> {
    use xxhash_rust::xxh3;
    match init {
        Init::Default => xxh3::xxh3_128(data),
        Init::Seed(n) => xxh3::xxh3_128_with_seed(data, *n as u64),
        Init::Secret(s) => xxh3::xxh3_128_with_secret(data, s),
    }
    .to_be_bytes()
    .to_vec()
}
