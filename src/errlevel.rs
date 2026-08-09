//! PHP's `E_*` error levels and the `error_reporting` mask.
//!
//! Every diagnostic PHP emits carries one of these bits, and it is displayed only
//! when the bit is set in the current `error_reporting` mask. That is the whole
//! mechanism: `error_reporting()` and `ini_set('error_reporting', …)` write the
//! mask, `-d error_reporting=…` seeds it before the program is even read, and
//! `@expr` suppresses locally without touching it.
//!
//! The values are fixed by the engine, not configurable, so they live here as
//! constants and are published to PHP code as predefined constants from
//! `host::predefined_constants`.

/// `E_STRICT` (2048) is absent from [`E_ALL`]: PHP 8.4 removed the level, and
/// `E_ALL` shrank from 32767 to 30719 to match. The constant itself still exists
/// (deprecated) so old `E_ALL & ~E_STRICT` expressions keep parsing.
pub const E_ERROR: i64 = 1;
pub const E_WARNING: i64 = 2;
pub const E_PARSE: i64 = 4;
pub const E_NOTICE: i64 = 8;
pub const E_CORE_ERROR: i64 = 16;
pub const E_CORE_WARNING: i64 = 32;
pub const E_COMPILE_ERROR: i64 = 64;
pub const E_COMPILE_WARNING: i64 = 128;
pub const E_USER_ERROR: i64 = 256;
pub const E_USER_WARNING: i64 = 512;
pub const E_USER_NOTICE: i64 = 1024;
pub const E_STRICT: i64 = 2048;
pub const E_RECOVERABLE_ERROR: i64 = 4096;
pub const E_DEPRECATED: i64 = 8192;
pub const E_USER_DEPRECATED: i64 = 16384;
pub const E_ALL: i64 = 30719;

/// The `E_*` names an ini value may spell, for [`parse_ini_level`].
const NAMES: &[(&str, i64)] = &[
    ("E_ERROR", E_ERROR),
    ("E_WARNING", E_WARNING),
    ("E_PARSE", E_PARSE),
    ("E_NOTICE", E_NOTICE),
    ("E_CORE_ERROR", E_CORE_ERROR),
    ("E_CORE_WARNING", E_CORE_WARNING),
    ("E_COMPILE_ERROR", E_COMPILE_ERROR),
    ("E_COMPILE_WARNING", E_COMPILE_WARNING),
    ("E_USER_ERROR", E_USER_ERROR),
    ("E_USER_WARNING", E_USER_WARNING),
    ("E_USER_NOTICE", E_USER_NOTICE),
    ("E_STRICT", E_STRICT),
    ("E_RECOVERABLE_ERROR", E_RECOVERABLE_ERROR),
    ("E_DEPRECATED", E_DEPRECATED),
    ("E_USER_DEPRECATED", E_USER_DEPRECATED),
    ("E_ALL", E_ALL),
];

/// Parse an ini-file spelling of an error level: a bare integer, or the constant
/// expression PHP's ini scanner accepts — `E_ALL & ~E_DEPRECATED`, `E_ERROR |
/// E_WARNING`, `E_ALL ^ E_NOTICE`. Returns `None` for anything unparseable, which
/// the caller reports as an ignored setting rather than silently taking as zero
/// (taking it as zero would mute every diagnostic on a typo).
///
/// The grammar is flat: `~` binds to the term it precedes and `&`, `|`, `^` are
/// applied strictly left to right, which is what the reference scanner does — it
/// has no precedence table for this, only a fold.
pub fn parse_ini_level(text: &str) -> Option<i64> {
    let mut acc: Option<i64> = None;
    let mut op = '|';
    let mut it = text.split_whitespace().flat_map(split_ops).peekable();
    while let Some(tok) = it.next() {
        match tok.as_str() {
            "&" | "|" | "^" => {
                op = tok.chars().next()?;
                continue;
            }
            _ => {}
        }
        // A `~` that stands alone negates the NEXT token; one written against a
        // name (`~E_NOTICE`) negates that name.
        let (negate, name) = match tok.strip_prefix('~') {
            Some("") => (true, it.next()?),
            Some(rest) => (true, rest.to_string()),
            None => (false, tok),
        };
        let mut v = term(&name)?;
        if negate {
            v = !v;
        }
        acc = Some(match acc {
            None => v,
            Some(a) => match op {
                '&' => a & v,
                '^' => a ^ v,
                _ => a | v,
            },
        });
    }
    acc
}

/// One term: a decimal/hex integer or an `E_*` name (case-insensitively, as the
/// ini scanner matches them).
fn term(name: &str) -> Option<i64> {
    if let Some(hex) = name.strip_prefix("0x").or_else(|| name.strip_prefix("0X")) {
        return i64::from_str_radix(hex, 16).ok();
    }
    if let Ok(n) = name.parse::<i64>() {
        return Some(n);
    }
    NAMES
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| *v)
}

/// Split operator characters off a whitespace-free chunk so `E_ALL&~E_NOTICE`
/// tokenizes the same as `E_ALL & ~E_NOTICE`.
fn split_ops(chunk: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in chunk.chars() {
        if matches!(c, '&' | '|' | '^') {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            out.push(c.to_string());
        } else if c == '~' {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            cur.push('~');
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ini_level_spellings() {
        assert_eq!(parse_ini_level("0"), Some(0));
        assert_eq!(parse_ini_level("E_ALL"), Some(30719));
        assert_eq!(
            parse_ini_level("E_ALL & ~E_DEPRECATED"),
            Some(30719 & !8192)
        );
        assert_eq!(parse_ini_level("E_ALL&~E_NOTICE"), Some(30719 & !8));
        assert_eq!(parse_ini_level("E_ERROR | E_WARNING"), Some(3));
        assert_eq!(parse_ini_level("32767"), Some(32767));
        assert_eq!(parse_ini_level("nonsense"), None);
    }
}
