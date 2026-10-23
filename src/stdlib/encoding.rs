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

/// The `ZEND_ACC_DEPRECATED` notice the reference raises when `utf8_encode` /
/// `utf8_decode` is called (PHP 8.2).
fn deprecated_function(name: &str) {
    crate::host::with_host(|h| {
        h.deprecated(format!(
            "Function {name}() is deprecated since 8.2, visit the php.net documentation \
             for various alternatives"
        ))
    });
}

/// Materialize a byte slice as a PHP string (see `crate::bytes`).
fn bytes_to_str(bytes: &[u8]) -> String {
    crate::bytes::from_bytes(bytes)
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
        "iconv" => return iconv_family(name, args),
        "iconv_strlen" => return iconv_family(name, args),
        "iconv_substr" => return iconv_family(name, args),
        "iconv_strpos" => return iconv_family(name, args),
        "iconv_strrpos" => return iconv_family(name, args),
        "iconv_get_encoding" => return iconv_family(name, args),
        "base64_encode" => Value::str(STANDARD.encode(bytes_arg(args, 0))),
        "base64_decode" => match base64_decode_ex(&bytes_arg(args, 0), strict()) {
            Some(bytes) => Value::str(bytes_to_str(&bytes)),
            None => Value::bool(false),
        },
        // `PHP_FUNCTION(hex2bin)`: the length is checked before the digits, and
        // each refusal is a warning before the `false`.
        "hex2bin" => {
            let s = str_arg(args, 0);
            if s.len() % 2 != 0 {
                crate::host::with_host(|h| {
                    h.warn("hex2bin(): Hexadecimal input string must have an even length")
                });
                return Some(Ok(Value::bool(false)));
            }
            match hex::decode(s.as_bytes()) {
                Ok(bytes) => Value::str(bytes_to_str(&bytes)),
                Err(_) => {
                    crate::host::with_host(|h| {
                        h.warn("hex2bin(): Input string must be hexadecimal string")
                    });
                    Value::bool(false)
                }
            }
        }
        "quoted_printable_encode" => Value::str(bytes_to_str(&qp_encode(&bytes_arg(args, 0)))),
        "quoted_printable_decode" => Value::str(bytes_to_str(&qp_decode(&bytes_arg(args, 0)))),
        "convert_uuencode" => Value::str(bytes_to_str(&uuencode(&bytes_arg(args, 0)))),
        "convert_uudecode" => match uudecode(&bytes_arg(args, 0)) {
            Some(bytes) => Value::str(bytes_to_str(&bytes)),
            None => Value::bool(false),
        },
        // The deprecated Latin-1 <-> UTF-8 shims: encode widens each byte to
        // the codepoint of the same value; decode narrows each character to
        // its byte, writing `?` for one past U+00FF and for each invalid unit.
        "utf8_encode" => {
            deprecated_function("utf8_encode");
            Value::str(
                bytes_arg(args, 0)
                    .iter()
                    .map(|&b| char::from(b))
                    .collect::<String>(),
            )
        }
        "utf8_decode" => {
            deprecated_function("utf8_decode");
            let bytes: Vec<u8> = crate::bytes::php_utf8_units(&str_arg(args, 0))
                .into_iter()
                .map(|u| match u {
                    Some(c) if u32::from(c) <= 0xFF => u32::from(c) as u8,
                    _ => b'?',
                })
                .collect();
            Value::str(bytes_to_str(&bytes))
        }
        _ => return None,
    };
    Some(Ok(v))
}

// ── iconv ────────────────────────────────────────────────────────────────────
//
// A port of ext/iconv's `php_iconv_string` and the `iconv_*` string functions
// over the charsets below. What the platform libiconv does that is not a
// property of the charset itself — its aliases, the spelling its 8-bit
// converters substitute for a character they lack, and its `//TRANSLIT`
// spellings — is measured off the reference into `iconv_tables.rs` by
// `scripts/gen_iconv_tables.php`. Strings are byte strings (`crate::bytes`), so
// every charset's bytes are exact.

#[path = "iconv_tables.rs"]
mod tables;

/// A charset `iconv` converts from or to. Plain `Utf16`/`Utf32` write a
/// big-endian byte-order mark and honour one on input; the others carry their
/// byte order in the name.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Charset {
    Utf8,
    Ascii,
    Latin1,
    Cp1252,
    Latin9,
    Utf16,
    Utf16Be,
    Utf16Le,
    Utf32,
    Utf32Be,
    Utf32Le,
}

/// The charset a name spells, and whether it carries `//TRANSLIT` and
/// `//IGNORE` (case-insensitively; any other `//` suffix is ignored).
fn charset(name: &str) -> Option<(Charset, bool, bool)> {
    let mut parts = name.split("//");
    let base = parts.next().unwrap_or("").to_ascii_uppercase();
    let cs = tables::ALIASES
        .binary_search_by(|&(alias, _)| alias.cmp(base.as_str()))
        .ok()
        .map(|i| tables::ALIASES[i].1)?;
    let (mut translit, mut ignore) = (false, false);
    for suffix in parts {
        translit |= suffix.eq_ignore_ascii_case("TRANSLIT");
        ignore |= suffix.eq_ignore_ascii_case("IGNORE");
    }
    Some((cs, translit, ignore))
}

/// Why a conversion stopped: libiconv's `EILSEQ` and `EINVAL`.
enum Bad {
    Illegal,
    Incomplete,
}

/// The highest codepoint libiconv's UCS-4 decoder accepts: it predates the
/// U+10FFFF ceiling.
const UCS4_MAX: u32 = 0x7FFF_FFFF;

/// The codepoints `bytes` encode in `cs`. With `ignore` (the target carried
/// `//IGNORE`), a byte that cannot start a character is skipped rather than
/// refused, as `php_iconv_string` steps over each `EILSEQ`; a character cut
/// short by the end of the input is refused either way.
fn decode(cs: Charset, bytes: &[u8], ignore: bool) -> Result<Vec<u32>, Bad> {
    let mut out = Vec::with_capacity(bytes.len());
    // An illegal unit: skipped under `//IGNORE`, else the conversion fails.
    let illegal = || if ignore { Ok(()) } else { Err(Bad::Illegal) };
    match cs {
        Charset::Utf8 => {
            let mut seen = 0;
            for chunk in bytes.utf8_chunks() {
                out.extend(chunk.valid().chars().map(u32::from));
                let bad = chunk.invalid();
                seen += chunk.valid().len() + bad.len();
                if !bad.is_empty() {
                    if seen == bytes.len() && bad[0] >= 0xC2 && bad[0] <= 0xF4 {
                        return Err(Bad::Incomplete);
                    }
                    illegal()?;
                }
            }
        }
        Charset::Ascii | Charset::Latin1 | Charset::Cp1252 | Charset::Latin9 => {
            for &b in bytes {
                let cp = match (cs, b) {
                    (_, 0..=0x7F) | (Charset::Latin1, _) => u32::from(b),
                    (Charset::Cp1252, _) => tables::CP1252[usize::from(b - 0x80)],
                    (Charset::Latin9, _) => tables::ISO_8859_15[usize::from(b - 0x80)],
                    _ => 0,
                };
                if cp == 0 && b != 0 {
                    illegal()?;
                } else {
                    out.push(cp);
                }
            }
        }
        Charset::Utf16 | Charset::Utf16Be | Charset::Utf16Le => {
            let mut le = cs == Charset::Utf16Le;
            let mut units = bytes.chunks_exact(2);
            let mut pending: Option<u32> = None;
            for pair in units.by_ref() {
                let u = u32::from(if le {
                    u16::from_le_bytes([pair[0], pair[1]])
                } else {
                    u16::from_be_bytes([pair[0], pair[1]])
                });
                // UTF-16 reads a byte-order mark wherever one appears.
                if cs == Charset::Utf16 && pending.is_none() && (u == 0xFEFF || u == 0xFFFE) {
                    le ^= u == 0xFFFE;
                    continue;
                }
                match (pending.take(), u) {
                    (None, 0xD800..=0xDBFF) => pending = Some(u),
                    (None, 0xDC00..=0xDFFF) => illegal()?,
                    (None, _) => out.push(u),
                    (Some(hi), 0xDC00..=0xDFFF) => {
                        out.push(0x10000 + ((hi - 0xD800) << 10) + (u - 0xDC00))
                    }
                    (Some(_), _) => illegal()?,
                }
            }
            if pending.is_some() || !units.remainder().is_empty() {
                return Err(Bad::Incomplete);
            }
        }
        Charset::Utf32 | Charset::Utf32Be | Charset::Utf32Le => {
            let mut le = cs == Charset::Utf32Le;
            let mut units = bytes.chunks_exact(4);
            for quad in units.by_ref() {
                let q = [quad[0], quad[1], quad[2], quad[3]];
                let u = if le {
                    u32::from_le_bytes(q)
                } else {
                    u32::from_be_bytes(q)
                };
                if cs == Charset::Utf32 && (u == 0xFEFF || u == 0xFFFE_0000) {
                    le ^= u == 0xFFFE_0000;
                    continue;
                }
                if u > UCS4_MAX || (0xD800..=0xDFFF).contains(&u) {
                    illegal()?;
                } else {
                    out.push(u);
                }
            }
            if !units.remainder().is_empty() {
                return Err(Bad::Incomplete);
            }
        }
    }
    Ok(out)
}

/// A generated `(codepoint, bytes)` table, sorted by codepoint.
type Substitutions = &'static [(u32, &'static [u8])];

/// The substitutions libiconv makes for `cs`: `(fallback, translit)`, where
/// the first applies to every conversion and the second only under
/// `//TRANSLIT`. Both are empty for a Unicode charset, which holds everything.
fn substitutions(cs: Charset) -> (Substitutions, Substitutions) {
    match cs {
        Charset::Ascii => (tables::FALLBACK_ASCII, tables::TRANSLIT_ASCII),
        Charset::Latin1 => (tables::FALLBACK_LATIN1, tables::TRANSLIT_LATIN1),
        Charset::Latin9 => (tables::FALLBACK_LATIN9, tables::TRANSLIT_LATIN9),
        Charset::Cp1252 => (tables::FALLBACK_CP1252, tables::TRANSLIT_CP1252),
        _ => (&[], &[]),
    }
}

/// The bytes a substitution table spells `cp` with.
fn substitute(table: Substitutions, cp: u32) -> Option<&'static [u8]> {
    table
        .binary_search_by_key(&cp, |&(c, _)| c)
        .ok()
        .map(|i| table[i].1)
}

/// Append `cp` in `cs` as the charset itself spells it; false when it has no
/// byte sequence for it.
fn encode_native(cs: Charset, cp: u32, out: &mut Vec<u8>) -> bool {
    let high = |table: &[u32; 128], out: &mut Vec<u8>| match table.iter().position(|&t| t == cp) {
        Some(i) => {
            out.push(0x80 + i as u8);
            true
        }
        None => false,
    };
    match cs {
        _ if cp < 0x80
            && !matches!(cs, Charset::Utf16 | Charset::Utf16Be | Charset::Utf16Le)
            && !matches!(cs, Charset::Utf32 | Charset::Utf32Be | Charset::Utf32Le) =>
        {
            out.push(cp as u8);
            true
        }
        Charset::Utf8 => {
            encode_utf8_ucs4(cp, out);
            true
        }
        Charset::Ascii => false,
        Charset::Latin1 => {
            let ok = cp <= 0xFF;
            if ok {
                out.push(cp as u8);
            }
            ok
        }
        Charset::Cp1252 => high(&tables::CP1252, out),
        Charset::Latin9 => high(&tables::ISO_8859_15, out),
        Charset::Utf16 | Charset::Utf16Be | Charset::Utf16Le => {
            // libiconv writes `?` for a codepoint past U+10FFFF, which only
            // its UCS-4 decoder can produce.
            let c = char::from_u32(cp).unwrap_or('?');
            for u in c.encode_utf16(&mut [0u16; 2]) {
                out.extend_from_slice(&if cs == Charset::Utf16Le {
                    u.to_le_bytes()
                } else {
                    u.to_be_bytes()
                });
            }
            true
        }
        Charset::Utf32 | Charset::Utf32Be | Charset::Utf32Le => {
            out.extend_from_slice(&if cs == Charset::Utf32Le {
                cp.to_le_bytes()
            } else {
                cp.to_be_bytes()
            });
            true
        }
    }
}

/// `cp` in UTF-8, in the original (RFC 2279) form that runs to six bytes for
/// the codepoints past U+10FFFF libiconv's UCS-4 decoder admits.
fn encode_utf8_ucs4(cp: u32, out: &mut Vec<u8>) {
    if let Some(c) = char::from_u32(cp) {
        out.extend_from_slice(c.encode_utf8(&mut [0u8; 4]).as_bytes());
        return;
    }
    let len = match cp {
        0..=0x1F_FFFF => 4,
        0x20_0000..=0x3FF_FFFF => 5,
        _ => 6,
    };
    let lead_mark: u8 = [0xF0, 0xF8, 0xFC][len - 4];
    out.push(lead_mark | (cp >> (6 * (len - 1))) as u8);
    for i in (0..len - 1).rev() {
        out.push(0x80 | ((cp >> (6 * i)) & 0x3F) as u8);
    }
}

/// The `…(): Detected …` notice for a failed conversion, raised by `fname`.
fn bad_notice(fname: &str, bad: &Bad) {
    let what = match bad {
        Bad::Illegal => "an illegal character",
        Bad::Incomplete => "an incomplete multibyte character",
    };
    crate::host::with_host(|h| h.notice(format!("{fname}(): Detected {what} in input string")));
}

/// The `Wrong encoding` warning for a conversion libiconv cannot open.
fn wrong_encoding(fname: &str, from: &str, to: &str) {
    crate::host::with_host(|h| {
        h.warn(format!(
            "{fname}(): Wrong encoding, conversion from \"{from}\" to \"{to}\" is not allowed"
        ))
    });
}

/// `iconv($from_encoding, $to_encoding, $string)`.
fn php_iconv(args: &[Value]) -> Value {
    let (from_name, to_name) = (str_arg(args, 0), str_arg(args, 1));
    let (Some((from, ..)), Some((to, translit, ignore))) = (charset(&from_name), charset(&to_name))
    else {
        wrong_encoding("iconv", &from_name, &to_name);
        return Value::bool(false);
    };
    let cps = match decode(from, &bytes_arg(args, 2), ignore) {
        Ok(c) => c,
        Err(bad) => {
            bad_notice("iconv", &bad);
            return Value::bool(false);
        }
    };
    let mut out = Vec::new();
    if matches!(to, Charset::Utf16 | Charset::Utf32) && !cps.is_empty() {
        encode_native(to, 0xFEFF, &mut out);
    }
    let (fallback, translit_table) = substitutions(to);
    for cp in cps {
        if encode_native(to, cp, &mut out) {
            continue;
        }
        let spelled = substitute(fallback, cp)
            .or_else(|| translit.then(|| substitute(translit_table, cp)).flatten());
        match spelled {
            Some(bytes) => out.extend_from_slice(bytes),
            None if ignore => {}
            None => {
                bad_notice("iconv", &Bad::Illegal);
                return Value::bool(false);
            }
        }
    }
    bytes_value(&out)
}

/// The codepoints of argument `subject` in the charset named by argument `at`
/// (UTF-8 when absent or null) — the decode every `iconv_*` string function
/// starts with — or `None`, with the reference's diagnostic raised, when the
/// charset is unknown or the string does not decode.
fn iconv_cps(
    fname: &str,
    args: &[Value],
    subject: usize,
    at: usize,
) -> Option<(Charset, Vec<u32>)> {
    let cs = match args.get(at) {
        _ if given(args, at) => {
            let name = str_arg(args, at);
            match charset(&name) {
                Some((cs, ..)) => cs,
                None => {
                    wrong_encoding(fname, &name, "UCS-4LE");
                    return None;
                }
            }
        }
        _ => Charset::Utf8,
    };
    match decode(cs, &bytes_arg(args, subject), false) {
        Ok(c) => Some((cs, c)),
        Err(bad) => {
            bad_notice(fname, &bad);
            None
        }
    }
}

/// Codepoints back into `cs`, every one of which was decoded from it.
fn encode_all(cs: Charset, cps: &[u32]) -> Value {
    let mut out = Vec::new();
    for &cp in cps {
        encode_native(cs, cp, &mut out);
    }
    bytes_value(&out)
}

/// The `[start, end)` character window `iconv_substr` takes.
fn iconv_window(len: i64, offset: i64, length: Option<i64>) -> (usize, usize) {
    let start = if offset < 0 {
        (len + offset).max(0)
    } else {
        offset.min(len)
    };
    let end = match length {
        None => len,
        Some(l) if l < 0 => len + l,
        Some(l) => start.saturating_add(l).min(len),
    };
    (start as usize, end.max(start) as usize)
}

/// Whether the optional argument `i` was passed as something other than null.
fn given(args: &[Value], i: usize) -> bool {
    args.get(i).is_some_and(|v| !matches!(v, Value::Undef))
}

/// The `iconv` family.
fn iconv_family(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let v = match name {
        "iconv" => php_iconv(args),
        "iconv_strlen" => match iconv_cps(name, args, 0, 1) {
            Some((_, c)) => Value::int(c.len() as i64),
            None => Value::bool(false),
        },
        "iconv_substr" => match iconv_cps(name, args, 0, 3) {
            Some((cs, c)) => {
                let length = given(args, 2).then(|| int_arg(args, 2));
                let (s, e) = iconv_window(c.len() as i64, int_arg(args, 1), length);
                encode_all(cs, &c[s..e])
            }
            None => Value::bool(false),
        },
        "iconv_strpos" => {
            // An empty needle is `false` before the offset is looked at.
            if bytes_arg(args, 1).is_empty() {
                return Some(Ok(Value::bool(false)));
            }
            let Some((_, hay)) = iconv_cps(name, args, 0, 3) else {
                return Some(Ok(Value::bool(false)));
            };
            let Some((_, needle)) = iconv_cps(name, args, 1, 3) else {
                return Some(Ok(Value::bool(false)));
            };
            let offset = if given(args, 2) { int_arg(args, 2) } else { 0 };
            let len = hay.len() as i64;
            if offset > len || offset < -len {
                return Some(Err(throws(
                    "ValueError",
                    "iconv_strpos(): Argument #3 ($offset) must be contained in argument #1 ($haystack)",
                )));
            }
            let from = if offset < 0 { len + offset } else { offset } as usize;
            let found = (from..=hay.len().saturating_sub(needle.len()))
                .find(|&i| hay[i..].starts_with(&needle));
            found.map_or(Value::bool(false), |i| Value::int(i as i64))
        }
        "iconv_strrpos" => {
            if bytes_arg(args, 1).is_empty() {
                return Some(Ok(Value::bool(false)));
            }
            let Some((_, hay)) = iconv_cps(name, args, 0, 2) else {
                return Some(Ok(Value::bool(false)));
            };
            let Some((_, needle)) = iconv_cps(name, args, 1, 2) else {
                return Some(Ok(Value::bool(false)));
            };
            if needle.len() > hay.len() {
                return Some(Ok(Value::bool(false)));
            }
            let found = (0..=hay.len() - needle.len())
                .rev()
                .find(|&i| hay[i..].starts_with(&needle));
            found.map_or(Value::bool(false), |i| Value::int(i as i64))
        }
        "iconv_get_encoding" => {
            const SETTINGS: [&str; 3] = ["input_encoding", "output_encoding", "internal_encoding"];
            let ty = if given(args, 0) {
                str_arg(args, 0)
            } else {
                "all".to_string()
            };
            if ty == "all" {
                make_map(
                    SETTINGS
                        .iter()
                        .map(|k| (Value::str(k.to_string()), Value::str("UTF-8".to_string())))
                        .collect(),
                )
            } else if SETTINGS.contains(&ty.as_str()) {
                Value::str("UTF-8".to_string())
            } else {
                Value::bool(false)
            }
        }
        _ => return None,
    };
    Some(Ok(v))
}
