//! PHP standard-library `hashext` functions: file digests, constant-time
//! comparison, PBKDF2 key derivation and random bytes. Part of the `stdlib`
//! chain; see `src/stdlib/mod.rs`. `dispatch` returns `None` for names it does
//! not handle.
//!
//! Digests reuse the vetted RustCrypto crates (`md-5`, `sha1`, `sha2`) and
//! `crc32fast`; no cryptographic primitive is hand-rolled. The only local
//! constructions are the thin HMAC wrapper (RFC 2104) and the PBKDF2 loop (RFC
//! 2898), both of which layer over those crate digests — the module cannot call
//! the equivalents in `src/stdlib/hash.rs` because they are private to that
//! module, so the minimal wrappers are duplicated here.
//!
//! Byte model: file contents are hashed as the raw bytes on disk (`std::fs::read`),
//! which is byte-faithful for binary files — unlike routing the bytes back
//! through phplang's UTF-8 `String` type, which would re-encode any byte >= 0x80.
//! A `raw_output` digest is mapped byte→codepoint (Latin-1), matching the core
//! `chr`/`ord` byte handling and `src/stdlib/hash.rs`; the hex form (default) is
//! fully faithful. See `wrap` for the `strlen` caveat on raw binary output.

use crate::host::with_host;
use crate::stdlib::common::{arg, int_arg, str_arg, throws};
use fusevm::Value;

/// A raw-digest function (`data -> digest bytes`) paired with its HMAC block
/// size. Named so the algorithm tables stay readable.
type HmacAlgo = (fn(&[u8]) -> Vec<u8>, usize);

/// Dispatch a `hashext`-category PHP function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    Some(match name {
        "md5_file" => file_digest_fn(md5_bytes, args),
        "sha1_file" => file_digest_fn(sha1_bytes, args),
        "hash_file" => hash_file(args),
        "hash_hmac_file" => hash_hmac_file(args),
        "hash_equals" => Ok(Value::bool(hash_equals(args))),
        "hash_pbkdf2" => hash_pbkdf2(args),
        "random_bytes" => random_bytes(args),
        _ => return None,
    })
}

/// PHP boolean truthiness of the `i`-th argument, defaulting to false.
fn bool_arg(args: &[Value], i: usize) -> bool {
    with_host(|h| h.is_truthy(&arg(args, i)))
}

/// Wrap raw digest bytes into a PHP `Value`: lowercase hex, or a Latin-1 binary
/// string when `raw` is set.
///
/// String-model limitation (identical to `src/stdlib/hash.rs`): phplang strings
/// are UTF-8 `String`s, so a `raw = true` digest is mapped byte→codepoint
/// (Latin-1). Any digest byte >= 0x80 becomes a multi-byte UTF-8 codepoint, so
/// `strlen()` on a raw digest counts UTF-8 bytes, not digest bytes. The hex path
/// (default) is fully faithful.
fn wrap(bytes: Vec<u8>, raw: bool) -> Value {
    if raw {
        Value::str(bytes.iter().map(|&b| char::from(b)).collect::<String>())
    } else {
        Value::str(hex::encode(bytes))
    }
}

/// Read a file's raw bytes; `None` on any I/O error (open/read failure).
fn read_file(path: &str) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

/// Shared body of `md5_file`/`sha1_file`: read arg 0, hash, honor `raw_output`
/// at arg 1. Returns PHP `false` when the file cannot be read (PHP emits a
/// warning and returns `false`).
fn file_digest_fn(f: fn(&[u8]) -> Vec<u8>, args: &[Value]) -> Result<Value, String> {
    let path = str_arg(args, 0);
    let raw = bool_arg(args, 1);
    match read_file(&path) {
        Some(bytes) => Ok(wrap(f(&bytes), raw)),
        None => Ok(Value::bool(false)),
    }
}

/// `hash_file(algo, filename, binary = false)`. Validates the algorithm before
/// touching the filesystem (PHP throws the ValueError regardless of the file),
/// then reads and digests the raw file bytes.
fn hash_file(args: &[Value]) -> Result<Value, String> {
    let algo = str_arg(args, 0).to_ascii_lowercase();
    let path = str_arg(args, 1);
    let raw = bool_arg(args, 2);
    if !is_supported_algo(&algo) {
        // PHP 8 ValueError text (mirrors the core `hash()` wording).
        return Err(
            "hash_file(): Argument #1 ($algo) must be a valid hashing algorithm".to_string(),
        );
    }
    let bytes = match read_file(&path) {
        Some(b) => b,
        None => return Ok(Value::bool(false)),
    };
    Ok(wrap(
        digest_bytes(&algo, &bytes).expect("algo checked above"),
        raw,
    ))
}

/// `hash_hmac_file(algo, filename, key, binary = false)`. Validates the
/// algorithm first, then HMACs the raw file bytes under `key`.
fn hash_hmac_file(args: &[Value]) -> Result<Value, String> {
    let algo = str_arg(args, 0).to_ascii_lowercase();
    let path = str_arg(args, 1);
    let key = str_arg(args, 2).into_bytes();
    let raw = bool_arg(args, 3);
    let (f, block_size) = match hmac_algo(&algo) {
        Some(pair) => pair,
        // PHP 8 ValueError text for an unknown HMAC algorithm.
        None => {
            return Err(throws(
                "ValueError",
                "hash_hmac_file(): Argument #1 ($algo) must be a valid cryptographic \
             hashing algorithm",
            ));
        }
    };
    let bytes = match read_file(&path) {
        Some(b) => b,
        None => return Ok(Value::bool(false)),
    };
    Ok(wrap(hmac(block_size, f, &key, &bytes), raw))
}

/// `hash_equals(known, user)` — length-independent (constant-time over equal
/// lengths) comparison. Returns PHP `false` when the lengths differ (length is
/// not treated as secret, matching PHP), otherwise accumulates a byte XOR so the
/// running time does not depend on the position of the first difference.
fn hash_equals(args: &[Value]) -> bool {
    let known = str_arg(args, 0).into_bytes();
    let user = str_arg(args, 1).into_bytes();
    if known.len() != user.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (a, b) in known.iter().zip(user.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// `hash_pbkdf2(algo, password, salt, iterations, length = 0, binary = false)`.
/// Standard PBKDF2 (RFC 2898) over HMAC of the named algorithm.
///
/// `length` is the length of the OUTPUT string: for `binary = true` it is the
/// byte-length of the derived key; for hex output it is the number of hex chars
/// (half a byte each). `length = 0` uses the full digest (`digest_size` bytes,
/// rendered as `2 * digest_size` hex chars).
fn hash_pbkdf2(args: &[Value]) -> Result<Value, String> {
    let algo = str_arg(args, 0).to_ascii_lowercase();
    let password = str_arg(args, 1).into_bytes();
    let salt = str_arg(args, 2).into_bytes();
    let iterations = int_arg(args, 3);
    let length = int_arg(args, 4);
    let raw = bool_arg(args, 5);

    let (f, block_size) = match hmac_algo(&algo) {
        Some(pair) => pair,
        None => {
            return Err(throws(
                "ValueError",
                "hash_pbkdf2(): Argument #1 ($algo) must be a valid cryptographic \
                 hashing algorithm",
            ));
        }
    };
    if iterations <= 0 {
        return Err(throws(
            "ValueError",
            "hash_pbkdf2(): Argument #4 ($iterations) must be greater than 0",
        ));
    }
    if length < 0 {
        return Err(throws(
            "ValueError",
            "hash_pbkdf2(): Argument #5 ($length) must be greater than or equal to 0",
        ));
    }
    // Cap the derived-key length so a pathological request cannot abort the
    // process on allocation.
    if length > (1 << 24) {
        return Err("hash_pbkdf2(): Argument #5 ($length) is too large".to_string());
    }

    let hlen = f(&[]).len();
    let length = length as usize;
    // Bytes of derived key to produce, and whether we return the full digest.
    let (dk_len, full) = if length == 0 {
        (hlen, true)
    } else if raw {
        (length, false)
    } else {
        // Hex output: each byte is two chars, so ceil(length / 2) bytes.
        (length.div_ceil(2), false)
    };
    let dk = pbkdf2(f, block_size, &password, &salt, iterations as usize, dk_len);
    if raw {
        Ok(wrap(dk, true))
    } else {
        let hexed = hex::encode(dk);
        if full {
            Ok(Value::str(hexed))
        } else {
            // Truncate to exactly `length` hex chars (handles odd lengths).
            Ok(Value::str(hexed[..length].to_string()))
        }
    }
}

/// `random_bytes(length)` — return `length` random bytes as a binary string.
///
/// NOT A CSPRNG. No `getrandom`/OS entropy dependency is available in this
/// crate, so the bytes come from a SplitMix64 generator seeded from the wall
/// clock (nanoseconds) and process id. Suitable for tests and non-security
/// filler ONLY; never use for keys, tokens, salts, or anything security
/// sensitive. Real PHP `random_bytes` is a CSPRNG — this is not.
fn random_bytes(args: &[Value]) -> Result<Value, String> {
    let len = int_arg(args, 0);
    if len < 1 {
        // PHP 8 ValueError text.
        return Err(throws(
            "ValueError",
            "random_bytes(): Argument #1 ($length) must be greater than 0",
        ));
    }
    // Cap the request so a pathological length cannot abort the process on
    // allocation (16 MiB is far beyond any real key/token size).
    if len > (1 << 24) {
        return Err("random_bytes(): Argument #1 ($length) is too large".to_string());
    }
    let len = len as usize;
    let mut out = Vec::with_capacity(len);
    let mut state = seed();
    while out.len() < len {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        for b in splitmix64(state).to_le_bytes() {
            if out.len() < len {
                out.push(b);
            }
        }
    }
    Ok(wrap(out, true))
}

// ---- algorithm tables -------------------------------------------------------

/// Whether `hash_file` supports a digest name (subset of PHP's `hash_algos`,
/// limited to the crate-backed digests wired here).
fn is_supported_algo(algo: &str) -> bool {
    matches!(
        algo,
        "md5" | "sha1" | "sha256" | "sha384" | "sha512" | "crc32b"
    )
}

/// Digest by algorithm name for `hash_file`; `None` for an unsupported name.
fn digest_bytes(algo: &str, data: &[u8]) -> Option<Vec<u8>> {
    Some(match algo {
        "md5" => md5_bytes(data),
        "sha1" => sha1_bytes(data),
        "sha256" => sha256_bytes(data),
        "sha384" => sha384_bytes(data),
        "sha512" => sha512_bytes(data),
        "crc32b" => crc32b_bytes(data),
        _ => return None,
    })
}

/// `(digest fn, HMAC block size)` for the HMAC/PBKDF2 algorithms; `None` for an
/// unknown name. Block size is the algorithm's internal block size: 64 bytes for
/// md5/sha1/sha256, 128 bytes for sha384/sha512.
fn hmac_algo(algo: &str) -> Option<HmacAlgo> {
    Some(match algo {
        "md5" => (md5_bytes, 64),
        "sha1" => (sha1_bytes, 64),
        "sha256" => (sha256_bytes, 64),
        "sha384" => (sha384_bytes, 128),
        "sha512" => (sha512_bytes, 128),
        _ => return None,
    })
}

// ---- digest primitives (RustCrypto crates) ---------------------------------

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

/// CRC-32/ISO-HDLC (reflected, zlib) rendered big-endian — the value PHP's
/// `hash('crc32b', …)` produces.
fn crc32b_bytes(data: &[u8]) -> Vec<u8> {
    let mut h = crc32fast::Hasher::new();
    h.update(data);
    h.finalize().to_be_bytes().to_vec()
}

// ---- HMAC / PBKDF2 ----------------------------------------------------------

/// Generic HMAC per RFC 2104: H((K⊕opad) ‖ H((K⊕ipad) ‖ msg)). Layers over the
/// crate digests above; duplicated from `src/stdlib/hash.rs` because that copy
/// is private to its module.
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

/// PBKDF2 (RFC 2898 §5.2) producing `dk_len` bytes of derived key using
/// HMAC-`f`. Each block T_i = U_1 ⊕ U_2 ⊕ … ⊕ U_c with
/// U_1 = HMAC(pass, salt ‖ INT_BE(i)) and U_j = HMAC(pass, U_{j-1}).
fn pbkdf2(
    f: fn(&[u8]) -> Vec<u8>,
    block_size: usize,
    password: &[u8],
    salt: &[u8],
    iterations: usize,
    dk_len: usize,
) -> Vec<u8> {
    let hlen = f(&[]).len();
    let n_blocks = dk_len.div_ceil(hlen);
    let mut out = Vec::with_capacity(n_blocks * hlen);
    for i in 1..=n_blocks as u32 {
        let mut msg = salt.to_vec();
        msg.extend_from_slice(&i.to_be_bytes());
        let mut u = hmac(block_size, f, password, &msg);
        let mut t = u.clone();
        for _ in 1..iterations {
            u = hmac(block_size, f, password, &u);
            for (tb, ub) in t.iter_mut().zip(u.iter()) {
                *tb ^= *ub;
            }
        }
        out.extend_from_slice(&t);
    }
    out.truncate(dk_len);
    out
}

// ---- non-cryptographic PRNG (random_bytes) ---------------------------------

/// Seed the SplitMix64 generator from the wall clock (nanoseconds) and pid.
/// Best-effort entropy only — see `random_bytes` for the CSPRNG caveat.
fn seed() -> u64 {
    let pid = std::process::id() as u64;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    nanos ^ pid.wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

/// SplitMix64 finalizer (Steele/Vigna). Deterministic mix of one 64-bit state.
fn splitmix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
