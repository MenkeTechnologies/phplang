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
    let s = crate::builtins::call_library("sprintf", call_args)?;
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
        None => return Ok(Value::int(-1)),
    };
    crate::builtins::call_library("sscanf", vec![Value::str(line), fmt])
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
            let k = match &k {
                Value::Str(s) if upper => Value::str(s.to_uppercase()),
                Value::Str(s) => Value::str(s.to_lowercase()),
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
    // Default flags = ENT_QUOTES (3) when the argument is omitted.
    let flags = if args.len() > 1 { int_arg(args, 1) } else { 3 };
    let double_q = flags & 2 != 0; // ENT_COMPAT bit
    let single_q = flags & 1 != 0; // ENT_QUOTES adds this bit

    let mut pairs: Vec<(Value, Value)> = Vec::new();
    let mut push = |ch: char, ent: &str| pairs.push((Value::str(ch.to_string()), Value::str(ent.to_string())));

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
        // HTML_ENTITIES also maps the ISO-8859-1 supplement (U+00A0..U+00FF).
        for (cp, name) in LATIN1_ENTITIES {
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

/// ISO-8859-1 named-entity supplement (code point, entity name without `&`/`;`),
/// as emitted by `get_html_translation_table(HTML_ENTITIES)` in PHP.
const LATIN1_ENTITIES: &[(u32, &str)] = &[
    (160, "nbsp"),
    (161, "iexcl"),
    (162, "cent"),
    (163, "pound"),
    (164, "curren"),
    (165, "yen"),
    (166, "brvbar"),
    (167, "sect"),
    (168, "uml"),
    (169, "copy"),
    (170, "ordf"),
    (171, "laquo"),
    (172, "not"),
    (173, "shy"),
    (174, "reg"),
    (175, "macr"),
    (176, "deg"),
    (177, "plusmn"),
    (178, "sup2"),
    (179, "sup3"),
    (180, "acute"),
    (181, "micro"),
    (182, "para"),
    (183, "middot"),
    (184, "cedil"),
    (185, "sup1"),
    (186, "ordm"),
    (187, "raquo"),
    (188, "frac14"),
    (189, "frac12"),
    (190, "frac34"),
    (191, "iquest"),
    (192, "Agrave"),
    (193, "Aacute"),
    (194, "Acirc"),
    (195, "Atilde"),
    (196, "Auml"),
    (197, "Aring"),
    (198, "AElig"),
    (199, "Ccedil"),
    (200, "Egrave"),
    (201, "Eacute"),
    (202, "Ecirc"),
    (203, "Euml"),
    (204, "Igrave"),
    (205, "Iacute"),
    (206, "Icirc"),
    (207, "Iuml"),
    (208, "ETH"),
    (209, "Ntilde"),
    (210, "Ograve"),
    (211, "Oacute"),
    (212, "Ocirc"),
    (213, "Otilde"),
    (214, "Ouml"),
    (215, "times"),
    (216, "Oslash"),
    (217, "Ugrave"),
    (218, "Uacute"),
    (219, "Ucirc"),
    (220, "Uuml"),
    (221, "Yacute"),
    (222, "THORN"),
    (223, "szlig"),
    (224, "agrave"),
    (225, "aacute"),
    (226, "acirc"),
    (227, "atilde"),
    (228, "auml"),
    (229, "aring"),
    (230, "aelig"),
    (231, "ccedil"),
    (232, "egrave"),
    (233, "eacute"),
    (234, "ecirc"),
    (235, "euml"),
    (236, "igrave"),
    (237, "iacute"),
    (238, "icirc"),
    (239, "iuml"),
    (240, "eth"),
    (241, "ntilde"),
    (242, "ograve"),
    (243, "oacute"),
    (244, "ocirc"),
    (245, "otilde"),
    (246, "ouml"),
    (247, "divide"),
    (248, "oslash"),
    (249, "ugrave"),
    (250, "uacute"),
    (251, "ucirc"),
    (252, "uuml"),
    (253, "yacute"),
    (254, "thorn"),
    (255, "yuml"),
];
