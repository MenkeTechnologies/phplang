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

/// Whether the PHP bytes of `s` are valid UTF-8. Not the same as `!has_raw`: a
/// character in the escape range is valid text stored as escaped bytes.
pub fn is_utf8(s: &str) -> bool {
    !has_raw(s) || std::str::from_utf8(&to_bytes(s)).is_ok()
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

/// The stored form of TEXT — a Rust string whose characters are meant as
/// themselves, such as a decoded entity, a JSON `\u` escape or an `mb_*`
/// result. Only a character in the escape range needs work: it becomes its
/// four escaped bytes, so it is not later read back as one byte.
///
/// Every producer that builds a string out of characters rather than bytes
/// must pass it through here (or [`from_bytes`]); a raw `Value::str(text)` is
/// only correct for text known to lie outside `U+10FF80..=U+10FFFF`.
pub fn from_text(text: &str) -> Cow<'_, str> {
    if !text.as_bytes().contains(&0xF4) || !text.chars().any(|c| unescape(c).is_some()) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() * 2);
    push_text(&mut out, text);
    Cow::Owned(out)
}

/// The escape characters at the end of `s`, at most `max` of them: the byte
/// offset where that run starts.
fn trailing_escapes(s: &str, max: usize) -> usize {
    let mut at = s.len();
    for (i, c) in s.char_indices().rev().take(max) {
        if unescape(c).is_none() {
            break;
        }
        at = i;
    }
    at
}

/// The escape characters at the start of `s`, at most `max` of them: the byte
/// length of that run.
fn leading_escapes(s: &str, max: usize) -> usize {
    s.char_indices()
        .take(max)
        .take_while(|&(_, c)| unescape(c).is_some())
        .last()
        .map_or(0, |(i, c)| i + c.len_utf8())
}

/// `x . y` for two canonical strings, in canonical form.
///
/// Only bytes at the seam can merge: a character that spans it has at most
/// three bytes on either side, and every one of them is an escape (a valid
/// character on either side is complete, and canonical text has no escaped
/// byte that could still join one beside it). So the join re-derives at most
/// the last three escapes of `x` and the first three of `y` — constant work,
/// where re-deriving the whole result made appending to a binary string
/// quadratic.
pub fn concat(x: &str, y: &str) -> String {
    let mut out = String::with_capacity(x.len() + y.len());
    out.push_str(x);
    append(&mut out, y);
    out
}

/// `x .= y` in place for two canonical strings (see [`concat`](fn@concat)): only the seam
/// is re-derived, so appending to a string the caller owns is amortized
/// constant work.
pub fn append(x: &mut String, y: &str) {
    let tail = trailing_escapes(x, 3);
    let head = leading_escapes(y, 3);
    if tail == x.len() || head == 0 {
        x.push_str(y);
        return;
    }
    let mut seam = to_bytes(&x[tail..]).into_owned();
    seam.extend_from_slice(&to_bytes(&y[..head]));
    x.truncate(tail);
    x.push_str(&from_bytes(&seam));
    x.push_str(&y[head..]);
}

/// The byte offset of the first `needle` in `hay` (an empty needle is found at
/// 0).
pub fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    memchr::memmem::find(hay, needle)
}

/// The byte offset of the last `needle` in `hay`.
pub fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    memchr::memmem::rfind(hay, needle)
}

/// `hay` with every non-overlapping `needle` (searched left to right) replaced
/// by `rep`, and how many there were. An empty needle replaces nothing.
pub fn replace(hay: &[u8], needle: &[u8], rep: &[u8]) -> (Vec<u8>, usize) {
    if needle.is_empty() {
        return (hay.to_vec(), 0);
    }
    let mut out = Vec::with_capacity(hay.len());
    let mut last = 0;
    let mut n = 0;
    for at in memchr::memmem::find_iter(hay, needle) {
        out.extend_from_slice(&hay[last..at]);
        out.extend_from_slice(rep);
        last = at + needle.len();
        n += 1;
    }
    out.extend_from_slice(&hay[last..]);
    (out, n)
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

/// One character of a string as mbstring's UTF-8 decoder splits it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MbUnit<'a> {
    Char(char),
    /// A maximal subpart that is not valid UTF-8, with its bytes.
    Invalid(&'a [u8]),
}

impl MbUnit<'_> {
    /// The unit's bytes in stored form.
    pub fn stored(self) -> String {
        match self {
            MbUnit::Char(c) => from_text(c.encode_utf8(&mut [0u8; 4])).into_owned(),
            MbUnit::Invalid(b) => from_bytes(b),
        }
    }
}

/// The characters of the byte string `b` as mbstring counts them, for the
/// functions that copy an invalid sequence through rather than printing `?`
/// (`mb_str_split`, `mb_str_pad`, `mb_ucfirst`).
pub fn mb_units(b: &[u8]) -> Vec<MbUnit<'_>> {
    let mut out = Vec::with_capacity(b.len());
    for chunk in b.utf8_chunks() {
        out.extend(chunk.valid().chars().map(MbUnit::Char));
        if !chunk.invalid().is_empty() {
            out.push(MbUnit::Invalid(chunk.invalid()));
        }
    }
    out
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

    /// Byte strings over an alphabet chosen so that every way a seam can
    /// merge occurs: leads of every length, trail bytes, the escape-range lead
    /// `F4 8F`, stray bytes and ASCII.
    fn alphabet_strings(max_len: usize) -> Vec<Vec<u8>> {
        const ALPHA: [u8; 8] = [b'a', 0xC3, 0xA9, 0xE2, 0x82, 0xF4, 0x8F, 0xBF];
        let mut all = vec![Vec::new()];
        let mut layer = vec![Vec::new()];
        for _ in 0..max_len {
            layer = layer
                .iter()
                .flat_map(|s: &Vec<u8>| {
                    ALPHA.iter().map(move |&b| {
                        let mut t = s.clone();
                        t.push(b);
                        t
                    })
                })
                .collect();
            all.extend(layer.iter().cloned());
        }
        all
    }

    #[test]
    fn concat_re_derives_only_the_seam() {
        let strs = alphabet_strings(4);
        let short: Vec<&Vec<u8>> = strs.iter().filter(|s| s.len() <= 3).collect();
        for x in &strs {
            let sx = from_bytes(x);
            for y in &short {
                let sy = from_bytes(y);
                let whole = [x.as_slice(), y.as_slice()].concat();
                assert_eq!(concat(&sx, &sy), from_bytes(&whole), "{x:02x?} . {y:02x?}");
                assert_eq!(
                    concat(&sy, &sx),
                    from_bytes(&[y.as_slice(), x.as_slice()].concat())
                );
            }
        }
    }

    #[test]
    fn every_short_byte_string_round_trips_uniquely() {
        let strs = alphabet_strings(5);
        let mut seen = std::collections::HashSet::new();
        for b in &strs {
            let s = from_bytes(b);
            assert_eq!(to_bytes(&s).as_ref(), b.as_slice());
            assert_eq!(canonical(s.clone()), s);
            assert!(seen.insert(s), "two byte strings share a stored form");
        }
    }

    #[test]
    fn text_in_the_escape_range_is_stored_as_its_bytes() {
        let text = "a\u{10FF80}\u{10FFFF}b";
        let stored = from_text(text);
        assert_eq!(to_bytes(&stored).as_ref(), text.as_bytes());
        assert_eq!(stored, from_bytes(text.as_bytes()));
        assert!(matches!(from_text("plain é"), Cow::Borrowed(_)));
    }

    #[test]
    fn a_genuine_escape_range_character_is_four_bytes() {
        let s = from_bytes("\u{10FFFF}".as_bytes());
        assert_eq!(s.chars().count(), 4);
        assert_eq!(to_bytes(&s).as_ref(), "\u{10FFFF}".as_bytes());
    }
}
