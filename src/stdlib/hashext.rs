//! PHP standard-library `hashext` functions: file digests, constant-time
//! comparison, PBKDF2 key derivation, random bytes, and the incremental
//! `hash_init` … `hash_final` family over a `HashContext`. Part of the `stdlib`
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
use crate::stdlib::fileres::{read as stream_read, strerror};
use crate::stdlib::hash::{digest_bytes, find, hmac, md5_bytes, sha1_bytes};
use fusevm::Value;

/// The digest function paired with its HMAC block size.
type HmacAlgo = (fn(&[u8]) -> Vec<u8>, usize);

/// `(digest fn, HMAC block size)` for the cryptographic algorithms of
/// `hash::ALGOS`; `None` for an unknown name or a plain checksum.
fn hmac_algo(algo: &str) -> Option<HmacAlgo> {
    find(algo).and_then(|a| a.block.map(|b| (a.digest, b)))
}

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
        "hash_init" => hash_init(args),
        "hash_update" => hash_update(args),
        "hash_update_stream" => hash_update_stream(args),
        "hash_update_file" => hash_update_file(args),
        "hash_final" => hash_final(args),
        "hash_copy" => hash_copy(args),
        _ => return None,
    })
}

/// PHP boolean truthiness of the `i`-th argument, defaulting to false.
fn bool_arg(args: &[Value], i: usize) -> bool {
    with_host(|h| h.is_truthy(&arg(args, i)))
}

/// Wrap raw digest bytes into a PHP `Value`: lowercase hex, or the binary string
/// itself when `raw` is set.
fn wrap(bytes: Vec<u8>, raw: bool) -> Value {
    if raw {
        Value::str(crate::bytes::from_bytes(&bytes))
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
    if find(&algo).is_none() {
        // PHP 8 ValueError text (mirrors the core `hash()` wording).
        return Err(throws(
            "ValueError",
            "hash_file(): Argument #1 ($algo) must be a valid hashing algorithm",
        ));
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
    let key = crate::bytes::to_bytes(&str_arg(args, 2)).into_owned();
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
    let known = crate::bytes::to_bytes(&str_arg(args, 0)).into_owned();
    let user = crate::bytes::to_bytes(&str_arg(args, 1)).into_owned();
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
    let password = crate::bytes::to_bytes(&str_arg(args, 1)).into_owned();
    let salt = crate::bytes::to_bytes(&str_arg(args, 2)).into_owned();
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

// ---- PBKDF2 ------------------------------------------------------------------

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

// ── Incremental hashing: `hash_init` … `hash_final` over a `HashContext` ──────
//
// The reference keeps a live digest state inside a `HashContext` object
// (`ext/hash/hash.c`, `php_hashcontext_object`). phplang's digests are one-shot
// `fn(&[u8]) -> Vec<u8>` (see `hash::ALGOS`), so the context accumulates the
// bytes it is fed and digests them once at `hash_final`. Every observable result
// — the digest, the HMAC, `hash_copy` independence, the refusals and their
// wording — is the reference's; only the memory profile differs.
//
// The state lives in the private properties of the prelude's `HashContext`
// class (`HASH_PRELUDE` in `src/lib.rs`): `algo`, `key` (the HMAC key, or null),
// `data` and `finalized`. Keeping it in properties rather than a host-side
// table is what makes `clone $ctx` behave as the reference's clone handler
// does — the copy has its own state — and lets `__debugInfo` show the `algo`
// the reference's `var_dump` shows.
//
// `data` holds BYTES, one per `char` (Latin-1), the same byte→codepoint model
// `wrap` uses for a raw digest. A PHP string fed to `hash_update` contributes
// its UTF-8 bytes — exactly what `hash()` digests — while `hash_update_file`
// and `hash_update_stream` contribute the raw bytes they read, so a binary file
// hashes byte-faithfully instead of being re-encoded.

/// The flag bit `hash_init` reads from `$flags`: the context HMACs under `$key`.
pub(crate) const HASH_HMAC: i64 = 1;

/// The class every context is an instance of.
const CONTEXT_CLASS: &str = "HashContext";

/// `hash_init(algo, flags = 0, key = "", options = [])`.
///
/// The three refusals are `php_hash_init`'s, in its order: an unknown
/// algorithm, a checksum where `HASH_HMAC` asks for a cryptographic digest, and
/// an empty key with `HASH_HMAC` set. A key given without the flag is ignored.
fn hash_init(args: &[Value]) -> Result<Value, String> {
    let algo_name = str_arg(args, 0).to_ascii_lowercase();
    let hmac = int_arg(args, 1) & HASH_HMAC != 0;
    let Some(algo) = find(&algo_name) else {
        return Err(throws(
            "ValueError",
            "hash_init(): Argument #1 ($algo) must be a valid hashing algorithm",
        ));
    };
    let key = if hmac {
        if algo.block.is_none() {
            return Err(throws(
                "ValueError",
                "hash_init(): Argument #1 ($algo) must be a cryptographic hashing \
                 algorithm if HMAC is requested",
            ));
        }
        let key = str_arg(args, 2);
        if key.is_empty() {
            return Err(throws(
                "ValueError",
                "hash_init(): Argument #3 ($key) must not be empty when HMAC is requested",
            ));
        }
        Value::str(key)
    } else {
        Value::Undef
    };
    let props = vec![
        ("algo".to_string(), Value::str(algo.name.to_string())),
        ("key".to_string(), key),
        ("data".to_string(), Value::str(String::new())),
        ("finalized".to_string(), Value::bool(false)),
    ];
    Ok(with_host(|h| h.new_object_bare(CONTEXT_CLASS, props)))
}

/// The `$context` argument of `fname`, checked the way the reference checks it:
/// a `TypeError` naming the type given for anything but a `HashContext`, and
/// the `valid, non-finalized` `TypeError` for a context `hash_final` has closed.
fn context_arg(fname: &str, args: &[Value]) -> Result<Value, String> {
    let ctx = arg(args, 0);
    let (is_context, finalized) = with_host(|h| {
        let is_context = h
            .object_class(&ctx)
            .is_some_and(|c| c.eq_ignore_ascii_case(CONTEXT_CLASS));
        let finalized = is_context && h.is_truthy(&h.prop_get(&ctx, "finalized"));
        (is_context, finalized)
    });
    if !is_context {
        let given = with_host(|h| h.type_name_for_error(&ctx));
        return Err(throws(
            "TypeError",
            format!("{fname}(): Argument #1 ($context) must be of type HashContext, {given} given"),
        ));
    }
    if finalized {
        return Err(throws(
            "TypeError",
            format!("{fname}(): Argument #1 ($context) must be a valid, non-finalized HashContext"),
        ));
    }
    Ok(ctx)
}

/// Append raw bytes to the context's buffered input (one Latin-1 `char` each).
fn context_feed(ctx: &Value, bytes: &[u8]) {
    with_host(|h| {
        let mut data = h.to_str(&h.prop_get(ctx, "data"));
        data.extend(bytes.iter().map(|&b| char::from(b)));
        h.prop_set(ctx, "data", Value::str(data));
    });
}

/// `hash_update(context, data)`: feed a PHP string's bytes. Always `true`.
fn hash_update(args: &[Value]) -> Result<Value, String> {
    let ctx = context_arg("hash_update", args)?;
    context_feed(&ctx, &crate::bytes::to_bytes(&str_arg(args, 1)));
    Ok(Value::bool(true))
}

/// `hash_update_stream(context, stream, length = -1)`: feed up to `length`
/// bytes from the stream's cursor (everything left when negative), returning
/// how many were read.
fn hash_update_stream(args: &[Value]) -> Result<Value, String> {
    let ctx = context_arg("hash_update_stream", args)?;
    let stream = arg(args, 1);
    if !with_host(|h| h.is_resource(&stream)) {
        let given = with_host(|h| h.type_name_for_error(&stream));
        return Err(throws(
            "TypeError",
            format!("hash_update_stream(): Argument #2 ($stream) must be of type resource, {given} given"),
        ));
    }
    let length = match arg(args, 2) {
        Value::Undef => -1,
        _ => int_arg(args, 2),
    };
    let want = usize::try_from(length).unwrap_or(usize::MAX);
    let bytes =
        stream_read("hash_update_stream", &stream, |s| Some(s.read(want))).unwrap_or_default();
    context_feed(&ctx, &bytes);
    Ok(Value::int(bytes.len() as i64))
}

/// `hash_update_file(context, filename, stream_context = null)`: feed a file's
/// raw bytes. An empty path is the stream layer's `ValueError`; an unopenable
/// one is its `Failed to open stream` warning and `false`; a directory opens
/// but cannot be read, which is the reference's `Notice` and `false`.
fn hash_update_file(args: &[Value]) -> Result<Value, String> {
    let ctx = context_arg("hash_update_file", args)?;
    let path = str_arg(args, 1);
    if path.is_empty() {
        return Err(throws("ValueError", "Path must not be empty"));
    }
    match std::fs::read(&path) {
        Ok(bytes) => {
            context_feed(&ctx, &bytes);
            Ok(Value::bool(true))
        }
        Err(e) if std::path::Path::new(&path).is_dir() => {
            with_host(|h| {
                h.notice(format!(
                    "hash_update_file(): Read of 8192 bytes failed with errno=21 {}",
                    strerror(&e)
                ))
            });
            Ok(Value::bool(false))
        }
        Err(e) => {
            with_host(|h| {
                h.warn(format!(
                    "hash_update_file({path}): Failed to open stream: {}",
                    strerror(&e)
                ))
            });
            Ok(Value::bool(false))
        }
    }
}

/// `hash_final(context, binary = false)`: the digest — or the HMAC, when the
/// context was opened with `HASH_HMAC` — of everything fed so far. The context
/// is finalized: every later `hash_update`/`hash_final`/`hash_copy` on it is
/// refused, and its buffered input is released.
fn hash_final(args: &[Value]) -> Result<Value, String> {
    let ctx = context_arg("hash_final", args)?;
    let raw = bool_arg(args, 1);
    let (algo_name, key, data) = with_host(|h| {
        let key = match h.prop_get(&ctx, "key") {
            Value::Undef => None,
            k => Some(crate::bytes::to_bytes(&h.to_str(&k)).into_owned()),
        };
        let data: Vec<u8> = h
            .to_str(&h.prop_get(&ctx, "data"))
            .chars()
            .map(|c| c as u32 as u8)
            .collect();
        h.prop_set(&ctx, "data", Value::str(String::new()));
        h.prop_set(&ctx, "finalized", Value::bool(true));
        (h.to_str(&h.prop_get(&ctx, "algo")), key, data)
    });
    let algo = find(&algo_name).expect("hash_init only stores a registered algorithm");
    let digest = match (key, algo.block) {
        (Some(key), Some(block)) => hmac(block, algo.digest, &key, &data),
        _ => (algo.digest)(&data),
    };
    Ok(wrap(digest, raw))
}

/// `hash_copy(context)`: an independent context in the same state, as the
/// reference's `php_hashcontext_clone` produces — later updates to either side
/// do not reach the other.
fn hash_copy(args: &[Value]) -> Result<Value, String> {
    let ctx = context_arg("hash_copy", args)?;
    Ok(with_host(|h| {
        let props = ["algo", "key", "data", "finalized"]
            .iter()
            .map(|name| (name.to_string(), h.prop_get(&ctx, name)))
            .collect();
        h.new_object_bare(CONTEXT_CLASS, props)
    }))
}
