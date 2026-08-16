//! PHP standard-library `textx` functions: stream-directed `printf` family
//! (`fprintf`/`vfprintf`/`fscanf`) plus text/array long-tail helpers
//! (`array_change_key_case`, `get_html_translation_table`). Part of the `stdlib`
//! chain; see `src/stdlib/mod.rs`. `dispatch` returns `None` for unknown names.

use crate::host::with_host;
use crate::stdlib::common::*;
use fusevm::Value;

/// Dispatch a `textx`-category PHP function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let r = match name {
        "fprintf" => fprintf(args),
        "vfprintf" => vfprintf(args),
        "fscanf" => fscanf(args),
        "array_change_key_case" => Ok(array_change_key_case(args)),
        "get_html_translation_table" => Ok(get_html_translation_table(args)),
        _ => return None,
    };
    Some(r)
}

/// Flush a resource's buffered content to disk if it is dirty, so writes to a
/// file stream persist without an explicit `fclose` (mirrors `fileres::flush`).
fn flush(res: &Value) {
    if let Some((path, buf)) = with_host(|h| h.res_flush_data(res)) {
        let _ = std::fs::write(path, buf);
    }
}

/// `fprintf($stream, $format, ...$args)` — format via `sprintf` and write the
/// result to `$stream`, returning the number of bytes written (or `false`).
fn fprintf(args: &[Value]) -> Result<Value, String> {
    let res = arg(args, 0);
    let mut call_args = vec![arg(args, 1)];
    if args.len() > 2 {
        call_args.extend_from_slice(&args[2..]);
    }
    write_formatted(&res, call_args)
}

/// `vfprintf($stream, $format, $args)` — like `fprintf` but the arguments arrive
/// as a single array.
fn vfprintf(args: &[Value]) -> Result<Value, String> {
    let res = arg(args, 0);
    let mut call_args = vec![arg(args, 1)];
    let pairs = with_host(|h| h.array_pairs(&arg(args, 2)).unwrap_or_default());
    for (_, v) in pairs {
        call_args.push(v);
    }
    write_formatted(&res, call_args)
}

/// Run `sprintf(...call_args)` and write the bytes to `$res`.
fn write_formatted(res: &Value, call_args: Vec<Value>) -> Result<Value, String> {
    let s = crate::builtins::call_library("sprintf", &call_args)?;
    let bytes = with_host(|h| h.to_str(&s)).into_bytes();
    let written = with_host(|h| h.res_write(res, &bytes));
    flush(res);
    Ok(match written {
        Some(n) => Value::int(n as i64),
        None => Value::bool(false),
    })
}

/// `fscanf($stream, $format)` (2-arg form) — read one line from `$stream` and
/// parse it with `sscanf`, returning the array of parsed values. Returns `-1` at
/// end of file, matching PHP. The by-reference (extra-args) form is not
/// supported; call it with two arguments.
fn fscanf(args: &[Value]) -> Result<Value, String> {
    let res = arg(args, 0);
    let fmt = arg(args, 1);
    let line = match with_host(|h| h.res_gets(&res, None)) {
        Some(l) => l,
        // At EOF the 2-arg form returns false (not -1); returning a truthy -1
        // would make the idiomatic `while ($r = fscanf(...))` loop spin forever.
        None => return Ok(Value::bool(false)),
    };
    crate::builtins::call_library("sscanf", &[Value::str(line), fmt])
}

/// `array_change_key_case($array, $case = CASE_LOWER)` — return a copy of
/// `$array` with all string keys lower- (`CASE_LOWER` = 0) or upper-cased
/// (`CASE_UPPER` = 1). Integer keys are left unchanged.
fn array_change_key_case(args: &[Value]) -> Value {
    let upper = resolve_case(&arg(args, 1)) == 1;
    let pairs = with_host(|h| h.array_pairs(&arg(args, 0)).unwrap_or_default());
    let mapped = pairs
        .into_iter()
        .map(|(k, v)| {
            // PHP folds ASCII only (locale-independent); non-ASCII bytes are left
            // untouched, so key length never changes.
            let k = match &k {
                Value::Str(s) if upper => Value::str(s.to_ascii_uppercase()),
                Value::Str(s) => Value::str(s.to_ascii_lowercase()),
                _ => k,
            };
            (k, v)
        })
        .collect();
    make_map(mapped)
}

/// Resolve a `$case` argument: the canonical PHP integer (`0`/`1`) or the
/// bareword constant name (which reaches phplang as its string).
fn resolve_case(v: &Value) -> i64 {
    if let Value::Str(name) = v {
        match name.as_str() {
            "CASE_LOWER" => return 0,
            "CASE_UPPER" => return 1,
            _ => {}
        }
    }
    v.to_int()
}

/// `get_html_translation_table($table = HTML_SPECIALCHARS, $flags = ENT_QUOTES)`
/// — return the char→entity map used by `htmlspecialchars`/`htmlentities`.
/// `$table` is `HTML_SPECIALCHARS` (0) or `HTML_ENTITIES` (1); `$flags` selects
/// which quote characters are included (`ENT_NOQUOTES`/`ENT_COMPAT`/`ENT_QUOTES`).
fn get_html_translation_table(args: &[Value]) -> Value {
    let table = resolve_table(&arg(args, 0));
    let flags = if args.len() > 1 {
        int_arg(args, 1)
    } else {
        ENT_DEFAULT
    };
    let (double_q, single_q) = quote_flags(flags);

    let mut pairs: Vec<(Value, Value)> = Vec::new();
    let mut push =
        |ch: char, ent: &str| pairs.push((Value::str(ch.to_string()), Value::str(ent.to_string())));

    // Both tables share the specialchars core (in PHP's ordering).
    if double_q {
        push('"', "&quot;");
    }
    push('&', "&amp;");
    if single_q {
        // ENT_HTML401 (default) uses the numeric single-quote entity.
        push('\'', "&#039;");
    }
    push('<', "&lt;");
    push('>', "&gt;");

    if table == 1 {
        // HTML_ENTITIES adds every named entity of the document type.
        for (cp, name) in HTML401_ENTITIES {
            let ch = char::from_u32(*cp).unwrap();
            pairs.push((Value::str(ch.to_string()), Value::str(format!("&{name};"))));
        }
    }
    make_map(pairs)
}

/// Resolve a `$table` argument: the integer or the bareword constant name.
fn resolve_table(v: &Value) -> i64 {
    if let Value::Str(name) = v {
        match name.as_str() {
            "HTML_SPECIALCHARS" => return 0,
            "HTML_ENTITIES" => return 1,
            _ => {}
        }
    }
    v.to_int()
}

// ── HTML entities ───────────────────────────────────────────────────────────

/// The HTML 4.01 named character entities beyond ASCII — the `ENT_HTML401`
/// document type, which is PHP's default for every `html*` function.
///
/// Code point and entity name (no `&`/`;`), ascending by code point, which is
/// also the order `get_html_translation_table` reports. It is the full W3C set
/// (Latin-1 supplement, then the `HTMLsymbol` Greek/math/arrow block, then
/// `HTMLspecial`), not just the Latin-1 part: `htmlentities` maps `€` to
/// `&euro;` and `α` to `&alpha;` exactly as it maps `é` to `&eacute;`.
pub(crate) const HTML401_ENTITIES: &[(u32, &str)] = &[
    (0x00A0, "nbsp"),
    (0x00A1, "iexcl"),
    (0x00A2, "cent"),
    (0x00A3, "pound"),
    (0x00A4, "curren"),
    (0x00A5, "yen"),
    (0x00A6, "brvbar"),
    (0x00A7, "sect"),
    (0x00A8, "uml"),
    (0x00A9, "copy"),
    (0x00AA, "ordf"),
    (0x00AB, "laquo"),
    (0x00AC, "not"),
    (0x00AD, "shy"),
    (0x00AE, "reg"),
    (0x00AF, "macr"),
    (0x00B0, "deg"),
    (0x00B1, "plusmn"),
    (0x00B2, "sup2"),
    (0x00B3, "sup3"),
    (0x00B4, "acute"),
    (0x00B5, "micro"),
    (0x00B6, "para"),
    (0x00B7, "middot"),
    (0x00B8, "cedil"),
    (0x00B9, "sup1"),
    (0x00BA, "ordm"),
    (0x00BB, "raquo"),
    (0x00BC, "frac14"),
    (0x00BD, "frac12"),
    (0x00BE, "frac34"),
    (0x00BF, "iquest"),
    (0x00C0, "Agrave"),
    (0x00C1, "Aacute"),
    (0x00C2, "Acirc"),
    (0x00C3, "Atilde"),
    (0x00C4, "Auml"),
    (0x00C5, "Aring"),
    (0x00C6, "AElig"),
    (0x00C7, "Ccedil"),
    (0x00C8, "Egrave"),
    (0x00C9, "Eacute"),
    (0x00CA, "Ecirc"),
    (0x00CB, "Euml"),
    (0x00CC, "Igrave"),
    (0x00CD, "Iacute"),
    (0x00CE, "Icirc"),
    (0x00CF, "Iuml"),
    (0x00D0, "ETH"),
    (0x00D1, "Ntilde"),
    (0x00D2, "Ograve"),
    (0x00D3, "Oacute"),
    (0x00D4, "Ocirc"),
    (0x00D5, "Otilde"),
    (0x00D6, "Ouml"),
    (0x00D7, "times"),
    (0x00D8, "Oslash"),
    (0x00D9, "Ugrave"),
    (0x00DA, "Uacute"),
    (0x00DB, "Ucirc"),
    (0x00DC, "Uuml"),
    (0x00DD, "Yacute"),
    (0x00DE, "THORN"),
    (0x00DF, "szlig"),
    (0x00E0, "agrave"),
    (0x00E1, "aacute"),
    (0x00E2, "acirc"),
    (0x00E3, "atilde"),
    (0x00E4, "auml"),
    (0x00E5, "aring"),
    (0x00E6, "aelig"),
    (0x00E7, "ccedil"),
    (0x00E8, "egrave"),
    (0x00E9, "eacute"),
    (0x00EA, "ecirc"),
    (0x00EB, "euml"),
    (0x00EC, "igrave"),
    (0x00ED, "iacute"),
    (0x00EE, "icirc"),
    (0x00EF, "iuml"),
    (0x00F0, "eth"),
    (0x00F1, "ntilde"),
    (0x00F2, "ograve"),
    (0x00F3, "oacute"),
    (0x00F4, "ocirc"),
    (0x00F5, "otilde"),
    (0x00F6, "ouml"),
    (0x00F7, "divide"),
    (0x00F8, "oslash"),
    (0x00F9, "ugrave"),
    (0x00FA, "uacute"),
    (0x00FB, "ucirc"),
    (0x00FC, "uuml"),
    (0x00FD, "yacute"),
    (0x00FE, "thorn"),
    (0x00FF, "yuml"),
    (0x0152, "OElig"),
    (0x0153, "oelig"),
    (0x0160, "Scaron"),
    (0x0161, "scaron"),
    (0x0178, "Yuml"),
    (0x0192, "fnof"),
    (0x02C6, "circ"),
    (0x02DC, "tilde"),
    (0x0391, "Alpha"),
    (0x0392, "Beta"),
    (0x0393, "Gamma"),
    (0x0394, "Delta"),
    (0x0395, "Epsilon"),
    (0x0396, "Zeta"),
    (0x0397, "Eta"),
    (0x0398, "Theta"),
    (0x0399, "Iota"),
    (0x039A, "Kappa"),
    (0x039B, "Lambda"),
    (0x039C, "Mu"),
    (0x039D, "Nu"),
    (0x039E, "Xi"),
    (0x039F, "Omicron"),
    (0x03A0, "Pi"),
    (0x03A1, "Rho"),
    (0x03A3, "Sigma"),
    (0x03A4, "Tau"),
    (0x03A5, "Upsilon"),
    (0x03A6, "Phi"),
    (0x03A7, "Chi"),
    (0x03A8, "Psi"),
    (0x03A9, "Omega"),
    (0x03B1, "alpha"),
    (0x03B2, "beta"),
    (0x03B3, "gamma"),
    (0x03B4, "delta"),
    (0x03B5, "epsilon"),
    (0x03B6, "zeta"),
    (0x03B7, "eta"),
    (0x03B8, "theta"),
    (0x03B9, "iota"),
    (0x03BA, "kappa"),
    (0x03BB, "lambda"),
    (0x03BC, "mu"),
    (0x03BD, "nu"),
    (0x03BE, "xi"),
    (0x03BF, "omicron"),
    (0x03C0, "pi"),
    (0x03C1, "rho"),
    (0x03C2, "sigmaf"),
    (0x03C3, "sigma"),
    (0x03C4, "tau"),
    (0x03C5, "upsilon"),
    (0x03C6, "phi"),
    (0x03C7, "chi"),
    (0x03C8, "psi"),
    (0x03C9, "omega"),
    (0x03D1, "thetasym"),
    (0x03D2, "upsih"),
    (0x03D6, "piv"),
    (0x2002, "ensp"),
    (0x2003, "emsp"),
    (0x2009, "thinsp"),
    (0x200C, "zwnj"),
    (0x200D, "zwj"),
    (0x200E, "lrm"),
    (0x200F, "rlm"),
    (0x2013, "ndash"),
    (0x2014, "mdash"),
    (0x2018, "lsquo"),
    (0x2019, "rsquo"),
    (0x201A, "sbquo"),
    (0x201C, "ldquo"),
    (0x201D, "rdquo"),
    (0x201E, "bdquo"),
    (0x2020, "dagger"),
    (0x2021, "Dagger"),
    (0x2022, "bull"),
    (0x2026, "hellip"),
    (0x2030, "permil"),
    (0x2032, "prime"),
    (0x2033, "Prime"),
    (0x2039, "lsaquo"),
    (0x203A, "rsaquo"),
    (0x203E, "oline"),
    (0x2044, "frasl"),
    (0x20AC, "euro"),
    (0x2111, "image"),
    (0x2118, "weierp"),
    (0x211C, "real"),
    (0x2122, "trade"),
    (0x2135, "alefsym"),
    (0x2190, "larr"),
    (0x2191, "uarr"),
    (0x2192, "rarr"),
    (0x2193, "darr"),
    (0x2194, "harr"),
    (0x21B5, "crarr"),
    (0x21D0, "lArr"),
    (0x21D1, "uArr"),
    (0x21D2, "rArr"),
    (0x21D3, "dArr"),
    (0x21D4, "hArr"),
    (0x2200, "forall"),
    (0x2202, "part"),
    (0x2203, "exist"),
    (0x2205, "empty"),
    (0x2207, "nabla"),
    (0x2208, "isin"),
    (0x2209, "notin"),
    (0x220B, "ni"),
    (0x220F, "prod"),
    (0x2211, "sum"),
    (0x2212, "minus"),
    (0x2217, "lowast"),
    (0x221A, "radic"),
    (0x221D, "prop"),
    (0x221E, "infin"),
    (0x2220, "ang"),
    (0x2227, "and"),
    (0x2228, "or"),
    (0x2229, "cap"),
    (0x222A, "cup"),
    (0x222B, "int"),
    (0x2234, "there4"),
    (0x223C, "sim"),
    (0x2245, "cong"),
    (0x2248, "asymp"),
    (0x2260, "ne"),
    (0x2261, "equiv"),
    (0x2264, "le"),
    (0x2265, "ge"),
    (0x2282, "sub"),
    (0x2283, "sup"),
    (0x2284, "nsub"),
    (0x2286, "sube"),
    (0x2287, "supe"),
    (0x2295, "oplus"),
    (0x2297, "otimes"),
    (0x22A5, "perp"),
    (0x22C5, "sdot"),
    (0x2308, "lceil"),
    (0x2309, "rceil"),
    (0x230A, "lfloor"),
    (0x230B, "rfloor"),
    (0x2329, "lang"),
    (0x232A, "rang"),
    (0x25CA, "loz"),
    (0x2660, "spades"),
    (0x2663, "clubs"),
    (0x2665, "hearts"),
    (0x2666, "diams"),
];

/// The quote handling `$flags` asks for: `ENT_COMPAT` (2) escapes the double
/// quote, `ENT_QUOTES` (3) both, `ENT_NOQUOTES` (0) neither.
pub(crate) fn quote_flags(flags: i64) -> (bool, bool) {
    (flags & 2 != 0, flags & 1 != 0)
}

/// PHP 8.1 raised the default `$flags` of every `html*` function from
/// `ENT_COMPAT` to `ENT_QUOTES | ENT_SUBSTITUTE | ENT_HTML401`.
pub(crate) const ENT_DEFAULT: i64 = 3 | 8;

/// Escape the five characters `htmlspecialchars` handles, honoring `$flags`.
/// `named` additionally maps every code point in [`HTML401_ENTITIES`], which is
/// what separates `htmlentities` from `htmlspecialchars`.
pub(crate) fn html_encode(s: &str, flags: i64, named: bool) -> String {
    let (dq, sq) = quote_flags(flags);
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if dq => out.push_str("&quot;"),
            // ENT_HTML401 has no `&apos;`, so the single quote is numeric.
            '\'' if sq => out.push_str("&#039;"),
            _ => match named.then(|| entity_name(ch as u32)).flatten() {
                Some(name) => {
                    out.push('&');
                    out.push_str(name);
                    out.push(';');
                }
                None => out.push(ch),
            },
        }
    }
    out
}

/// The HTML 4.01 entity name for a code point, if it has one.
fn entity_name(cp: u32) -> Option<&'static str> {
    HTML401_ENTITIES
        .binary_search_by_key(&cp, |(c, _)| *c)
        .ok()
        .map(|i| HTML401_ENTITIES[i].1)
}

/// The code point an entity name stands for, if any.
fn entity_cp(name: &str) -> Option<u32> {
    HTML401_ENTITIES
        .iter()
        .find(|(_, n)| *n == name)
        .map(|(c, _)| *c)
}

/// Decode `&…;` references.
///
/// `named` distinguishes the two decoders PHP ships: `html_entity_decode`
/// resolves the whole entity table AND numeric references, while
/// `htmlspecialchars_decode` resolves only what `htmlspecialchars` can produce —
/// so it leaves `&eacute;` and even `&#233;` standing, and the sole numeric form
/// it knows is the `&#039;`/`&#39;` it writes for the single quote itself.
///
/// An unrecognized reference is copied through verbatim, ampersand and all, and
/// so is a `&amp` with no terminating semicolon.
pub(crate) fn html_decode(s: &str, flags: i64, named: bool) -> String {
    let (dq, sq) = quote_flags(flags);
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < b.len() {
        if b[i] != b'&' {
            let start = i;
            while i < b.len() && b[i] != b'&' {
                i += 1;
            }
            out.push_str(&s[start..i]);
            continue;
        }
        // `&` … find the terminating `;`. PHP scans no further than the longest
        // entity name, so an unterminated `&` is never mistaken for one.
        let Some(semi) = b[i + 1..]
            .iter()
            .position(|&c| c == b';')
            .map(|p| i + 1 + p)
            .filter(|semi| semi - i <= 10)
        else {
            out.push('&');
            i += 1;
            continue;
        };
        let body = &s[i + 1..semi];
        let decoded = if let Some(digits) = body.strip_prefix('#') {
            let cp = match digits.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok(),
                None => digits.parse::<u32>().ok().filter(|_| !digits.is_empty()),
            };
            match cp.and_then(char::from_u32) {
                // `$flags` gates a NUMERIC quote reference exactly as it gates
                // the named one, in BOTH decoders: under `ENT_COMPAT` a `&#039;`
                // survives untouched even though `&#34;` next to it decodes.
                Some('\'') => sq.then_some('\''),
                Some('"') => dq.then_some('"'),
                // Any other numeric reference is out of the specialchars
                // decoder's reach — it can only undo what the encoder writes.
                Some(_) if !named => None,
                other => other,
            }
        } else {
            match body {
                "lt" => Some('<'),
                "gt" => Some('>'),
                "amp" => Some('&'),
                "quot" => dq.then_some('"'),
                _ if named => entity_cp(body).and_then(char::from_u32),
                _ => None,
            }
        };
        match decoded {
            Some(ch) => {
                out.push(ch);
                i = semi + 1;
            }
            None => {
                out.push('&');
                i += 1;
            }
        }
    }
    out
}
