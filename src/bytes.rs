//! PHP strings are byte strings; `fusevm::Value::Str` holds a Rust `String`,
//! which must be valid UTF-8. This module is the bridge.
//!
//! # Representation
//!
//! A PHP string — any byte sequence `B` — is stored as the Rust string
//! [`from_bytes`]`(B)`:
//!
//! * every maximal run of `B` that is valid UTF-8 is kept as those characters,
//!   so text costs nothing and reads as itself;
//! * every byte that is NOT part of a valid UTF-8 sequence becomes one
//!   *escape character*, `U+10FF00 + byte` — a code point in the last 128 of
//!   plane 16's private-use area (`U+10FF80`..=`U+10FFFF`);
//! * a genuine character in that escape range (bytes `F4 8F BE 80` ..
//!   `F4 8F BF BF`) is stored as its four bytes, each escaped, so an escape
//!   character never stands for itself.
//!
//! The mapping is therefore a bijection between byte strings and the Rust
//! strings it produces: [`to_bytes`] recovers `B` exactly, and the stored form
//! of a byte string is unique ("canonical"). Equality, ordering by `==`, array
//! keys and hashing of two canonical strings agree with the bytes they stand
//! for.
//!
//! # Obligations
//!
//! * A string with no escape character is plain UTF-8 and its Rust bytes ARE
//!   its PHP bytes; [`has_raw`] tells the two apart with one byte scan (every
//!   escape character's UTF-8 encoding begins with `0xF4`, a byte no other
//!   character below `U+100000` uses).
//! * Anything that measures or indexes a string BY BYTE (`strlen`, `substr`,
//!   `ord`, offsets, hashing, output) reads [`to_bytes`], not `str::as_bytes`.
//! * Anything that produces bytes builds the string with [`from_bytes`].
//! * Joining two canonical strings can produce a non-canonical one (`"\xC3"`
//!   and `"\xA9"` are, together, `"é"`), so a join of strings that carry escapes
//!   goes through [`canonical`]. Concatenation and every library function's
//!   string result are canonicalized at the engine boundary.

use std::borrow::Cow;

/// The first escape character: `U+10FF00 + byte` stands for `byte`.
const ESC_BASE: u32 = 0x10FF00;

/// The character a raw byte (`0x80..=0xFF`) is stored as.
fn escape(byte: u8) -> char {
    char::from_u32(ESC_BASE + u32::from(byte)).expect("plane-16 private use is a valid char")
}

/// The raw byte an escape character stands for, if `c` is one.
fn unescape(c: char) -> Option<u8> {
    let v = u32::from(c);
    (ESC_BASE + 0x80..=ESC_BASE + 0xFF)
        .contains(&v)
        .then(|| (v - ESC_BASE) as u8)
}

/// Whether `s` carries any escaped byte (else it is plain UTF-8 whose Rust
/// bytes are its PHP bytes).
pub fn has_raw(s: &str) -> bool {
    s.as_bytes().contains(&0xF4) && s.chars().any(|c| unescape(c).is_some())
}

/// The PHP bytes a stored string stands for.
pub fn to_bytes(s: &str) -> Cow<'_, [u8]> {
    if !has_raw(s) {
        return Cow::Borrowed(s.as_bytes());
    }
    let mut out = Vec::with_capacity(s.len());
    let mut buf = [0u8; 4];
    for c in s.chars() {
        match unescape(c) {
            Some(b) => out.push(b),
            None => out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes()),
        }
    }
    Cow::Owned(out)
}

/// The stored form of the byte string `b` — see the module note.
pub fn from_bytes(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len());
    let mut rest = b;
    loop {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                push_text(&mut out, valid);
                return out;
            }
            Err(e) => {
                let (valid, after) = rest.split_at(e.valid_up_to());
                push_text(
                    &mut out,
                    std::str::from_utf8(valid).expect("checked by from_utf8"),
                );
                // The bytes that do not decode: one for a stray byte, the
                // whole truncated prefix when the input ends mid-character.
                let bad = e.error_len().unwrap_or(after.len());
                for &byte in &after[..bad] {
                    out.push(escape(byte));
                }
                rest = &after[bad..];
            }
        }
    }
}

/// Append valid UTF-8 text, escaping any character that falls in the escape
/// range so that it is never read back as one byte.
fn push_text(out: &mut String, text: &str) {
    if !text.as_bytes().contains(&0xF4) {
        out.push_str(text);
        return;
    }
    let mut buf = [0u8; 4];
    for c in text.chars() {
        if unescape(c).is_some() {
            for &byte in c.encode_utf8(&mut buf).as_bytes() {
                out.push(escape(byte));
            }
        } else {
            out.push(c);
        }
    }
}

/// `s` in canonical form: a string that carries escapes is re-derived from its
/// bytes, which merges escaped bytes that now form valid UTF-8.
pub fn canonical(s: String) -> String {
    if has_raw(&s) {
        from_bytes(&to_bytes(&s))
    } else {
        s
    }
}

/// The string mbstring's UTF-8 decoder sees: every maximal run of bytes that is
/// not valid UTF-8 — one *character* to mbstring, however long — is `?`, as
/// `mb_substr` and the case mappers print it and `mb_strlen` counts it.
pub fn mb_text(s: &str) -> Cow<'_, str> {
    if !has_raw(s) {
        return Cow::Borrowed(s);
    }
    let bytes = to_bytes(s);
    let mut out = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        out.push_str(chunk.valid());
        if !chunk.invalid().is_empty() {
            out.push('?');
        }
    }
    Cow::Owned(out)
}

/// One step of ext/standard's UTF-8 decoder (`php_next_utf8_char`), which
/// `htmlspecialchars`, `json_encode` and `utf8_decode` share: the character at
/// the start of `b`, or `None` for an invalid unit, and how many bytes either
/// took. An invalid unit is not mbstring's maximal subpart: a truncated
/// sequence swallows the trail bytes it has, and a well-formed but disallowed
/// one (an overlong form, a surrogate, past U+10FFFF) is ONE unit of its full
/// length.
pub fn php_next_utf8_char(b: &[u8]) -> (Option<char>, usize) {
    let lead = |c: u8| c < 0x80 || (0xC2..=0xF4).contains(&c);
    let trail = |i: usize| b.get(i).is_some_and(|&c| (0x80..=0xBF).contains(&c));
    // The length of a truncated or broken `n`-byte sequence: up to (not
    // including) the first byte that could start a character.
    let broken = |n: usize| {
        (1..n)
            .find(|&i| b.get(i).is_none_or(|&c| lead(c)))
            .unwrap_or(n)
    };
    let c = b[0];
    let (n, cp, min) = match c {
        0..=0x7F => return (Some(char::from(c)), 1),
        0xC2..=0xDF => (2, u32::from(c & 0x1F), 0x80),
        0xE0..=0xEF => (3, u32::from(c & 0x0F), 0x800),
        0xF0..=0xF4 => (4, u32::from(c & 0x07), 0x10000),
        _ => return (None, 1),
    };
    if !(1..n).all(trail) {
        return (None, broken(n));
    }
    let cp = b[1..n]
        .iter()
        .fold(cp, |acc, &t| (acc << 6) | u32::from(t & 0x3F));
    if cp < min {
        return (None, n);
    }
    (char::from_u32(cp).filter(|_| cp <= 0x10FFFF), n)
}

/// `s` decoded by [`php_next_utf8_char`]: each character, or `None` for each
/// invalid unit.
pub fn php_utf8_units(s: &str) -> Vec<Option<char>> {
    let bytes = to_bytes(s);
    let mut out = Vec::with_capacity(bytes.len());
    let mut rest = &bytes[..];
    while !rest.is_empty() {
        let (c, n) = php_next_utf8_char(rest);
        out.push(c);
        rest = &rest[n..];
    }
    out
}

/// The PHP length of a stored string: its byte count.
pub fn byte_len(s: &str) -> usize {
    if has_raw(s) {
        to_bytes(s).len()
    } else {
        s.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_byte_string_round_trips() {
        let samples: Vec<Vec<u8>> = vec![
            b"plain".to_vec(),
            "h\u{e9}llo".as_bytes().to_vec(),
            vec![0xff],
            vec![0xc3],
            vec![0xc3, 0xa9, 0xff, b'a', 0xe2, 0x82],
            "\u{10FF80}".as_bytes().to_vec(),
            (0u8..=255).collect(),
        ];
        for b in samples {
            let s = from_bytes(&b);
            assert_eq!(to_bytes(&s).as_ref(), b.as_slice());
            assert_eq!(byte_len(&s), b.len());
            assert_eq!(canonical(s.clone()), s);
        }
    }

    #[test]
    fn text_is_stored_as_itself() {
        assert_eq!(from_bytes("h\u{e9}llo".as_bytes()), "h\u{e9}llo");
        assert!(!has_raw("h\u{e9}llo"));
    }

    #[test]
    fn joining_escaped_halves_canonicalizes_to_the_character() {
        let joined = format!("{}{}", from_bytes(&[0xc3]), from_bytes(&[0xa9]));
        assert_ne!(joined, "\u{e9}");
        assert_eq!(canonical(joined), "\u{e9}");
    }

    #[test]
    fn php_units_group_invalid_bytes_as_ext_standard_does() {
        let units = |b: &[u8]| {
            php_utf8_units(&from_bytes(b))
                .iter()
                .map(|u| u.unwrap_or('?'))
                .collect::<String>()
        };
        assert_eq!(units(b"\xe2\x82"), "?");
        assert_eq!(units(b"\xe2a\x82"), "?a?");
        assert_eq!(units(b"\xc0\x80"), "??");
        assert_eq!(units(b"\xed\xa0\x80"), "?");
        assert_eq!(units(b"\xf4\x90\x80\x80"), "?");
        assert_eq!(units(b"\xf0\x9f\x98a\xc3\xa9"), "?a\u{e9}");
    }

    #[test]
    fn a_genuine_escape_range_character_is_four_bytes() {
        let s = from_bytes("\u{10FFFF}".as_bytes());
        assert_eq!(s.chars().count(), 4);
        assert_eq!(to_bytes(&s).as_ref(), "\u{10FFFF}".as_bytes());
    }
}
