//! PHP standard-library `hash` functions. Part of the `stdlib` chain; see
//! `src/stdlib/mod.rs`. `dispatch` returns `None` for names it does not handle.
//!
//! Digests are produced by the vetted RustCrypto crates (`md-5`, `sha1`,
//! `sha2`) and `crc32fast`; no crypto is hand-rolled here (only the thin HMAC
//! construction and the CRC-32/BZIP2 variant, which no dependency provides).
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
        "hash_algos" => Ok(hash_algos()),
        _ => return None,
    })
}

/// Byte string of the `i`-th argument (UTF-8 bytes of the PHP string cast).
fn input_bytes(args: &[Value], i: usize) -> Vec<u8> {
    str_arg(args, i).into_bytes()
}

/// PHP boolean truthiness of the `i`-th argument, defaulting to false.
fn bool_arg(args: &[Value], i: usize) -> bool {
    with_host(|h| h.is_truthy(&arg(args, i)))
}

/// Wrap raw digest bytes into a PHP `Value`: lowercase hex, or a Latin-1
/// binary string when `raw` is set.
///
/// String-model limitation: phplang strings are UTF-8 `String`s, so a
/// `raw_output = true` digest is mapped byte→codepoint (Latin-1). Any digest
/// byte >= 0x80 becomes a multi-byte UTF-8 codepoint, so `strlen()` on a raw
/// digest counts UTF-8 bytes, not digest bytes — e.g. `strlen(md5("", true))`
/// is not 16 whenever the digest contains a high byte (the raw md5 of "" starts
/// with 0xd4). A faithful fix needs a byte-string type at the VM level, which
/// phplang does not have; the hex path (default) is fully faithful. `ord`/`chr`
/// round-trip per byte because they share this same Latin-1 model.
fn wrap(bytes: Vec<u8>, raw: bool) -> Value {
    if raw {
        Value::str(bytes.iter().map(|&b| char::from(b)).collect::<String>())
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
    match digest_bytes(&algo, &data) {
        Some(b) => Ok(wrap(b, raw)),
        // PHP 8 throws a ValueError with this exact message for an unknown algo
        // (replacing the PHP 7 "Unknown hashing algorithm" wording).
        None => Err(throws(
            "ValueError",
            "hash(): Argument #1 ($algo) must be a valid hashing algorithm",
        )),
    }
}

/// `hash_hmac(algo, data, key, binary = false)` for md5/sha1/sha256/sha384/sha512.
fn php_hash_hmac(args: &[Value]) -> Result<Value, String> {
    let algo = str_arg(args, 0).to_ascii_lowercase();
    let data = input_bytes(args, 1);
    let key = input_bytes(args, 2);
    let raw = bool_arg(args, 3);
    // HMAC block size is the algorithm's internal block size: 64 bytes for
    // md5/sha1/sha256, 128 bytes for sha384/sha512. Using the wrong block size
    // yields wrong digests for the SHA-384/512 family, so it is paired per algo.
    let f: fn(&[u8]) -> Vec<u8>;
    let block_size = match algo.as_str() {
        "md5" => {
            f = md5_bytes;
            64
        }
        "sha1" => {
            f = sha1_bytes;
            64
        }
        "sha256" => {
            f = sha256_bytes;
            64
        }
        "sha384" => {
            f = sha384_bytes;
            128
        }
        "sha512" => {
            f = sha512_bytes;
            128
        }
        // PHP 8 throws a ValueError with this exact message for an unknown algo.
        _ => {
            return Err(throws(
                "ValueError",
                "hash_hmac(): Argument #1 ($algo) must be a valid cryptographic \
                 hashing algorithm",
            ));
        }
    };
    Ok(wrap(hmac(block_size, f, &key, &data), raw))
}

/// Digest by algorithm name; `None` for an unknown algorithm.
fn digest_bytes(algo: &str, data: &[u8]) -> Option<Vec<u8>> {
    Some(match algo {
        "md5" => md5_bytes(data),
        "sha1" => sha1_bytes(data),
        "sha256" => sha256_bytes(data),
        "sha512" => sha512_bytes(data),
        // crc32b: reflected CRC-32/ISO-HDLC, matching PHP's crc32() value.
        "crc32b" => crc32b(data).to_be_bytes().to_vec(),
        // crc32: PHP's non-reflected CRC-32/BZIP2 variant. PHP renders it in
        // reversed (little-endian) byte order — a long-standing quirk that
        // distinguishes it from crc32b beyond the polynomial itself.
        "crc32" => crc32_bzip2(data).to_le_bytes().to_vec(),
        _ => return None,
    })
}

/// Algorithms this module supports, as a PHP list. A subset of PHP's full
/// `hash_algos()`, limited to the digests implemented here.
fn hash_algos() -> Value {
    with_host(|h| {
        let arr = h.new_array();
        for a in ["md5", "sha1", "sha256", "sha512", "crc32", "crc32b"] {
            h.arr_push_auto(&arr, Value::str(a.to_string()));
        }
        arr
    })
}

fn md5_bytes(data: &[u8]) -> Vec<u8> {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(data);
    h.finalize().to_vec()
}

fn sha1_bytes(data: &[u8]) -> Vec<u8> {
    use sha1::{Digest, Sha1};
    let mut h = Sha1::new();
    h.update(data);
    h.finalize().to_vec()
}

fn sha256_bytes(data: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().to_vec()
}

fn sha384_bytes(data: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha384};
    let mut h = Sha384::new();
    h.update(data);
    h.finalize().to_vec()
}

fn sha512_bytes(data: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha512};
    let mut h = Sha512::new();
    h.update(data);
    h.finalize().to_vec()
}

/// CRC-32/ISO-HDLC (reflected, zlib) — the value PHP's `crc32()` returns and
/// `hash('crc32b', …)` renders in hex.
fn crc32b(data: &[u8]) -> u32 {
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
fn hmac(block_size: usize, f: fn(&[u8]) -> Vec<u8>, key: &[u8], msg: &[u8]) -> Vec<u8> {
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
