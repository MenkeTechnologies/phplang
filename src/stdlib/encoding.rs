//! PHP standard-library `encoding` functions (base64, hex, quoted-printable,
//! uuencode, Latin-1<->UTF-8). Part of the `stdlib` chain; see
//! `src/stdlib/mod.rs`. `dispatch` returns `None` for names it does not handle.
//!
//! Binary results are materialized one PHP "byte" per Rust `char` (code point ==
//! byte value, i.e. Latin-1), matching the runtime's own `chr`/`ord`: `chr(233)`
//! is `U+00E9`, `ord` reads the first byte. This keeps these functions composable
//! with `chr`/`ord` and exact for ASCII payloads. High bytes (>= 128) cannot be
//! held as raw bytes in this UTF-8 runtime, so `bin2hex`/echo of decoded
//! multibyte data diverges from a raw-byte PHP; ASCII round-trips are exact.

use crate::stdlib::common::*;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use fusevm::Value;

/// Materialize a byte slice as a PHP string, one byte per Latin-1 `char`
/// (matches the runtime's `chr`).
fn bytes_to_str(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// Reverse lookup for base64 decoding, ported from PHP's `base64_reverse_table`:
/// `-1` = whitespace to skip (`\t \n \r` space), `-2` = invalid, `0..=63` = data.
/// `'='` padding is handled by the caller before this table is consulted.
fn base64_reverse(b: u8) -> i16 {
    match b {
        b'A'..=b'Z' => (b - b'A') as i16,
        b'a'..=b'z' => (b - b'a') as i16 + 26,
        b'0'..=b'9' => (b - b'0') as i16 + 52,
        b'+' => 62,
        b'/' => 63,
        b'\t' | b'\n' | b'\r' | b' ' => -1,
        _ => -2,
    }
}

/// Port of PHP's `php_base64_decode_ex`. Returns `None` (PHP `false`) on failure
/// in strict mode; non-strict silently skips unrecognized bytes.
fn base64_decode_ex(input: &[u8], strict: bool) -> Option<Vec<u8>> {
    let mut result = Vec::with_capacity(input.len());
    let mut i = 0i32; // count of consumed data sextets
    let mut padding = 0i32;
    let mut cur = 0u8; // byte under construction

    for &raw in input {
        if raw == b'=' {
            padding += 1;
            continue;
        }
        let ch = base64_reverse(raw);
        if !strict {
            // skip unknown characters and whitespace
            if ch < 0 {
                continue;
            }
        } else {
            // skip whitespace
            if ch == -1 {
                continue;
            }
            // fail on bad characters or if any data follows padding
            if ch == -2 || padding != 0 {
                return None;
            }
        }
        let ch = ch as u8;
        match i % 4 {
            0 => cur = ch << 2,
            1 => {
                result.push(cur | (ch >> 4));
                cur = (ch & 0x0f) << 4;
            }
            2 => {
                result.push(cur | (ch >> 2));
                cur = (ch & 0x03) << 6;
            }
            3 => result.push(cur | ch),
            _ => unreachable!(),
        }
        i += 1;
    }

    if strict {
        // truncated (only one sextet in the final group)
        if i % 4 == 1 {
            return None;
        }
        // wrong padding length (accept zero padding, else must complete a quad)
        if padding != 0 && (padding > 2 || (i + padding) % 4 != 0) {
            return None;
        }
    }
    Some(result)
}

/// Port of PHP's `php_quot_print_encode` (soft-wrap at `PHP_QPRINT_MAXL` = 75,
/// CRLF soft breaks, `=XX` for control/high/`=`/pre-CR-space bytes).
fn qp_encode(input: &[u8]) -> Vec<u8> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    const MAXL: i32 = 75;
    let mut out = Vec::with_capacity(input.len());
    let mut lp = 0i32;
    let n = input.len();
    let mut i = 0;
    while i < n {
        let c = input[i];
        // literal CRLF pair resets the line
        if c == 0x0d && i + 1 < n && input[i + 1] == 0x0a {
            out.push(0x0d);
            out.push(0x0a);
            lp = 0;
            i += 2;
            continue;
        }
        let next = if i + 1 < n { input[i + 1] } else { 0 };
        let is_cntrl = c < 0x20 || c == 0x7f;
        if is_cntrl || (c & 0x80) != 0 || c == b'=' || (c == b' ' && next == 0x0d) {
            lp += 3;
            if lp > MAXL && c <= 0x7f {
                out.push(b'=');
                out.push(0x0d);
                out.push(0x0a);
                lp = 3;
            }
            out.push(b'=');
            out.push(HEX[(c >> 4) as usize]);
            out.push(HEX[(c & 0x0f) as usize]);
        } else {
            lp += 1;
            if lp > MAXL {
                out.push(b'=');
                out.push(0x0d);
                out.push(0x0a);
                lp = 1;
            }
            out.push(c);
        }
        i += 1;
    }
    out
}

/// Hex nibble value, or `None` for a non-hex byte.
fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Decode quoted-printable: `=XX` -> byte, `=`+CRLF/LF soft break -> removed,
/// a trailing bare `=` as the final byte -> dropped (PHP), otherwise literal
/// (PHP keeps a malformed `=` verbatim).
fn qp_decode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let n = input.len();
    let mut i = 0;
    while i < n {
        let c = input[i];
        if c == b'=' && i + 1 >= n {
            // PHP drops a trailing bare '=' that is the final byte of input.
            break;
        }
        if c == b'=' && i + 1 < n {
            let n1 = input[i + 1];
            if n1 == b'\r' {
                // soft line break: drop CR and an optional following LF
                i += 2;
                if i < n && input[i] == b'\n' {
                    i += 1;
                }
                continue;
            } else if n1 == b'\n' {
                i += 2;
                continue;
            } else if i + 2 < n {
                if let (Some(h), Some(l)) = (hex_val(n1), hex_val(input[i + 2])) {
                    out.push((h << 4) | l);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// uuencode sextet -> printable byte; `PHP_UU_ENC` maps 0 to a backtick.
fn uu_enc(c: i32) -> u8 {
    let v = c & 0x3f;
    if v != 0 {
        (v + 0x20) as u8
    } else {
        b'`'
    }
}

/// Port of PHP's `php_uuencode`: 45-byte lines, leading length char, groups of
/// three bytes -> four chars (short final group zero-padded), backtick trailer.
fn uuencode(src: &[u8]) -> Vec<u8> {
    let n = src.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        let line_len = std::cmp::min(45, n - i);
        out.push(uu_enc(line_len as i32));
        let mut j = 0;
        while j < line_len {
            let b0 = src[i + j] as i32;
            let b1 = if j + 1 < line_len {
                src[i + j + 1] as i32
            } else {
                0
            };
            let b2 = if j + 2 < line_len {
                src[i + j + 2] as i32
            } else {
                0
            };
            out.push(uu_enc(b0 >> 2));
            out.push(uu_enc(((b0 << 4) & 0x30) | ((b1 >> 4) & 0x0f)));
            out.push(uu_enc(((b1 << 2) & 0x3c) | ((b2 >> 6) & 0x03)));
            out.push(uu_enc(b2 & 0x3f));
            j += 3;
        }
        out.push(b'\n');
        i += line_len;
    }
    out.push(b'`');
    out.push(b'\n');
    out
}

/// uudecode char -> sextet (`PHP_UU_DEC`).
fn uu_dec(c: u8) -> i32 {
    (c as i32 - 0x20) & 0x3f
}

/// Inverse of [`uuencode`]; port of PHP's `php_uudecode`. A length-0 line
/// (backtick) terminates. Returns `None` (PHP `false`) on empty input or on
/// malformed data: a line whose declared length overruns the buffer, or input
/// that decodes to nothing (no valid length/terminator structure).
fn uudecode(src: &[u8]) -> Option<Vec<u8>> {
    let src_len = src.len();
    if src_len == 0 {
        return None;
    }
    let n = src_len;
    let mut out = Vec::new();
    let mut total_len: usize = 0;
    let mut i = 0;
    while i < n {
        let line_len = uu_dec(src[i]) as usize;
        i += 1;
        if line_len == 0 {
            break;
        }
        // Sanity checks ported from php_uudecode: a declared length larger than
        // the whole buffer, or a line whose encoded span overruns the input, is
        // malformed. `ee = s + (len == 45 ? 60 : floor(len * 1.37))`.
        if line_len > src_len {
            return None;
        }
        total_len += line_len;
        let span = if line_len == 45 {
            60
        } else {
            (line_len as f64 * 1.37).floor() as usize
        };
        if i + span > n {
            return None;
        }
        let mut remaining = line_len;
        while remaining > 0 {
            let d0 = uu_dec(*src.get(i).unwrap_or(&0));
            let d1 = uu_dec(*src.get(i + 1).unwrap_or(&0));
            let d2 = uu_dec(*src.get(i + 2).unwrap_or(&0));
            let d3 = uu_dec(*src.get(i + 3).unwrap_or(&0));
            if remaining > 0 {
                out.push(((d0 << 2) | (d1 >> 4)) as u8);
                remaining -= 1;
            }
            if remaining > 0 {
                out.push(((d1 << 4) | (d2 >> 2)) as u8);
                remaining -= 1;
            }
            if remaining > 0 {
                out.push(((d2 << 6) | d3) as u8);
                remaining -= 1;
            }
            i += 4;
        }
        // skip the line terminator (\r and/or \n)
        if i < n && (src[i] == b'\r' || src[i] == b'\n') {
            i += 1;
        }
    }
    if out.is_empty() {
        return None;
    }
    // PHP caps the result at the summed declared lengths.
    if out.len() > total_len {
        out.truncate(total_len);
    }
    Some(out)
}

/// Dispatch an `encoding`-category PHP function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let strict = || args.len() > 1 && crate::host::with_host(|h| h.is_truthy(&arg(args, 1)));
    let v = match name {
        "base64_encode" => Value::str(STANDARD.encode(str_arg(args, 0).as_bytes())),
        "base64_decode" => match base64_decode_ex(str_arg(args, 0).as_bytes(), strict()) {
            Some(bytes) => Value::str(bytes_to_str(&bytes)),
            None => Value::bool(false),
        },
        "hex2bin" => {
            let s = str_arg(args, 0);
            match hex::decode(s.as_bytes()) {
                Ok(bytes) => Value::str(bytes_to_str(&bytes)),
                Err(_) => Value::bool(false),
            }
        }
        "quoted_printable_encode" => {
            Value::str(bytes_to_str(&qp_encode(str_arg(args, 0).as_bytes())))
        }
        "quoted_printable_decode" => {
            Value::str(bytes_to_str(&qp_decode(str_arg(args, 0).as_bytes())))
        }
        "convert_uuencode" => Value::str(bytes_to_str(&uuencode(str_arg(args, 0).as_bytes()))),
        "convert_uudecode" => match uudecode(str_arg(args, 0).as_bytes()) {
            Some(bytes) => Value::str(bytes_to_str(&bytes)),
            None => Value::bool(false),
        },
        // Deprecated Latin-1<->UTF-8 shims. In this UTF-8 runtime they operate on
        // the string's bytes: encode widens each byte to a code point, decode
        // narrows each code point back to a byte (>0xFF -> '?'). Exact for ASCII.
        "utf8_encode" => Value::str(
            str_arg(args, 0)
                .as_bytes()
                .iter()
                .map(|&b| b as char)
                .collect::<String>(),
        ),
        "utf8_decode" => {
            let s = str_arg(args, 0);
            let bytes: Vec<u8> = s
                .chars()
                .map(|c| {
                    let cp = c as u32;
                    if cp <= 0xff {
                        cp as u8
                    } else {
                        b'?'
                    }
                })
                .collect();
            Value::str(bytes_to_str(&bytes))
        }
        _ => return None,
    };
    Some(Ok(v))
}
