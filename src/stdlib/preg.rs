//! PHP standard-library `preg` (PCRE) functions. Part of the `stdlib` chain;
//! see `src/stdlib/mod.rs`. `dispatch` returns `None` for names it does not
//! handle.
//!
//! Backed by the Rust `regex` crate, which is a *subset* of PCRE: it has no
//! backreferences and no look-around (`\1`, `(?=)`, `(?<=)`, …). Patterns using
//! those constructs fail to compile and the function returns the PHP error
//! sentinel (`false` for the match/grep family, `null` for the replace family).
//! Everything else — character classes, quantifiers, anchors, alternation,
//! named/numbered groups, the `imsxuU` flags — is supported.
//!
//! One further engine nuance affects `preg_split`: Rust's `regex` suppresses a
//! zero-width match sitting immediately after a non-empty match, whereas PCRE
//! emits it. This diverges only for a pattern that can match *both* empty and
//! non-empty text at interleaved positions (`/x*/`, `/\d*/`); ordinary delimiter
//! patterns and the fully-empty `//` pattern split identically to PCRE.
//!
//! `$matches` by-reference out-parameter (`preg_match`, `preg_match_all`): the
//! stdlib dispatch chain receives call arguments *by value*, and the by-ref
//! lowering the compiler performs for `array_push` & friends is keyed on the
//! function name in the compiler (which this module may not edit). So the out
//! array is populated *in place* only when the caller pre-initialises it as an
//! array (`$m = []; preg_match($p, $s, $m);`) — the array handle is shared, so
//! writing through it is visible to the caller. When the caller passes an
//! uninitialised variable, the captures cannot be bound back; the return value
//! (match count) is always correct. Captures are also fully reachable through
//! `preg_replace` backreferences and `preg_replace_callback`.

use crate::host::with_host;
use fusevm::Value;

use regex::{Regex, RegexBuilder};

// PHP preg_split flag bits.
const SPLIT_NO_EMPTY: i64 = 1;
const SPLIT_DELIM_CAPTURE: i64 = 2;
const SPLIT_OFFSET_CAPTURE: i64 = 4;
// PHP preg_grep flag bit.
const GREP_INVERT: i64 = 1;
// PHP preg_match_all order flags.
const SET_ORDER: i64 = 2;

/// Dispatch a `preg`-category PHP function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    Some(match name {
        "preg_match" => preg_match(args),
        "preg_match_all" => preg_match_all(args),
        "preg_replace" => preg_replace(args),
        "preg_replace_callback" => preg_replace_callback(args),
        "preg_split" => preg_split(args),
        "preg_quote" => preg_quote(args),
        "preg_grep" => preg_grep(args),
        "preg_last_error" => Ok(Value::int(0)),
        "preg_last_error_msg" => Ok(Value::str("No error")),
        _ => return None,
    })
}

// ── delimiter / flag parsing ─────────────────────────────────────────────────

/// Parse a PHP PCRE pattern (`/body/flags`, `#body#`, `~body~`, `{body}`, …)
/// into a compiled `Regex`. Returns `Err` for a malformed delimiter/flag set or
/// a body the Rust engine cannot compile (e.g. backreferences).
fn compile(pattern: &str) -> Result<Regex, String> {
    let chars: Vec<char> = pattern.chars().collect();
    // Leading whitespace is allowed before the opening delimiter in PCRE.
    let mut i = 0;
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    if i >= chars.len() {
        return Err("empty pattern".into());
    }
    let open = chars[i];
    let close = match open {
        '(' => ')',
        '{' => '}',
        '[' => ']',
        '<' => '>',
        c if c.is_alphanumeric() || c == '\\' || c.is_whitespace() => {
            return Err(format!("invalid delimiter `{c}`"));
        }
        c => c,
    };
    // Find the matching closing delimiter, scanning from the end so the body may
    // contain the delimiter char when it is not a bracket pair.
    let body_start = i + 1;
    let close_idx = (body_start..chars.len())
        .rev()
        .find(|&j| chars[j] == close)
        .ok_or_else(|| format!("no ending delimiter `{close}` found"))?;
    if close_idx < body_start {
        return Err("no ending delimiter found".into());
    }
    let body: String = chars[body_start..close_idx].iter().collect();
    let flags: String = chars[close_idx + 1..].iter().collect();

    let mut b = RegexBuilder::new(&body);
    for f in flags.chars() {
        match f {
            'i' => {
                b.case_insensitive(true);
            }
            'm' => {
                b.multi_line(true);
            }
            's' => {
                b.dot_matches_new_line(true);
            }
            'x' => {
                b.ignore_whitespace(true);
            }
            'U' => {
                b.swap_greed(true);
            }
            // `u` (unicode) is the Rust default; `D`, `A`, `X`, `S` have no
            // Rust-engine analogue and are accepted as no-ops.
            'u' | 'D' | 'A' | 'X' | 'S' => {}
            other => return Err(format!("unknown modifier `{other}`")),
        }
    }
    b.build().map_err(|e| e.to_string())
}

// ── preg_match / preg_match_all ──────────────────────────────────────────────

/// Full capture list for one match: index 0 is the whole match, 1.. are the
/// numbered groups, unmatched groups render as the empty string. Fixed width —
/// used by `preg_match_all` where every set must line up by group index.
fn caps_full(caps: &regex::Captures) -> Vec<Value> {
    (0..caps.len())
        .map(|i| Value::str(caps.get(i).map(|m| m.as_str()).unwrap_or("")))
        .collect()
}

/// Capture list with trailing unmatched groups dropped — PHP's `preg_match` /
/// `preg_replace_callback` behaviour. A group that did not participate but is
/// followed by one that did is still emitted as the empty string.
fn caps_trimmed(caps: &regex::Captures) -> Vec<Value> {
    let last = (0..caps.len())
        .rfind(|&i| caps.get(i).is_some())
        .unwrap_or(0);
    (0..=last)
        .map(|i| Value::str(caps.get(i).map(|m| m.as_str()).unwrap_or("")))
        .collect()
}

/// Populate a caller-supplied array handle in place with `rows` (each a list of
/// values). Only works when `target` is already an array (shared handle); a
/// no-op otherwise. Existing integer-keyed slots are overwritten by position.
fn fill_out(target: &Value, rows: Vec<Value>) {
    with_host(|h| {
        if !h.is_array(target) {
            return;
        }
        for (i, row) in rows.into_iter().enumerate() {
            h.arr_set_key(target, &Value::int(i as i64), row);
        }
    });
}

fn preg_match(args: &[Value]) -> Result<Value, String> {
    let pat = with_host(|h| h.to_str(&arg(args, 0)));
    let subject = with_host(|h| h.to_str(&arg(args, 1)));
    let re = match compile(&pat) {
        Ok(r) => r,
        Err(_) => return Ok(Value::bool(false)),
    };
    match re.captures(&subject) {
        Some(caps) => {
            if args.len() > 2 {
                fill_out(&args[2], caps_trimmed(&caps));
            }
            Ok(Value::int(1))
        }
        None => {
            if args.len() > 2 {
                fill_out(&args[2], vec![]);
            }
            Ok(Value::int(0))
        }
    }
}

fn preg_match_all(args: &[Value]) -> Result<Value, String> {
    let pat = with_host(|h| h.to_str(&arg(args, 0)));
    let subject = with_host(|h| h.to_str(&arg(args, 1)));
    let flags = args.get(3).map(|v| v.to_int()).unwrap_or(0);
    let re = match compile(&pat) {
        Ok(r) => r,
        Err(_) => return Ok(Value::bool(false)),
    };
    let all: Vec<Vec<Value>> = re.captures_iter(&subject).map(|c| caps_full(&c)).collect();
    let count = all.len();

    if args.len() > 2 {
        let group_count = re.captures_len(); // includes group 0
        let rows: Vec<Value> = if flags & SET_ORDER != 0 {
            // PREG_SET_ORDER: matches[set][group].
            all.into_iter().map(make_list).collect()
        } else {
            // PREG_PATTERN_ORDER (default): matches[group][set].
            (0..group_count)
                .map(|g| make_list(all.iter().map(|row| row[g].clone()).collect()))
                .collect()
        };
        fill_out(&args[2], rows);
    }
    Ok(Value::int(count as i64))
}

// ── preg_replace / preg_replace_callback ─────────────────────────────────────

/// Translate a PHP replacement template (`$1`, `${1}`, `\1`, literal `$`/`\`)
/// into the Rust `regex` replacement syntax (`${1}`, literal `$$`).
fn translate_replacement(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let take_digits = |from: usize| -> usize {
        let mut j = from;
        while j < chars.len() && chars[j].is_ascii_digit() {
            j += 1;
        }
        j
    };
    while i < chars.len() {
        match chars[i] {
            '\\' => {
                if i + 1 < chars.len() && chars[i + 1].is_ascii_digit() {
                    let j = take_digits(i + 1);
                    out.push_str("${");
                    out.extend(&chars[i + 1..j]);
                    out.push('}');
                    i = j;
                } else if i + 1 < chars.len() && chars[i + 1] == '\\' {
                    out.push('\\');
                    i += 2;
                } else {
                    out.push('\\');
                    i += 1;
                }
            }
            '$' => {
                if i + 1 < chars.len() && chars[i + 1] == '{' {
                    let j = take_digits(i + 2);
                    if j > i + 2 && j < chars.len() && chars[j] == '}' {
                        out.push_str("${");
                        out.extend(&chars[i + 2..j]);
                        out.push('}');
                        i = j + 1;
                    } else {
                        out.push_str("$$");
                        i += 1;
                    }
                } else if i + 1 < chars.len() && chars[i + 1].is_ascii_digit() {
                    let j = take_digits(i + 1);
                    out.push_str("${");
                    out.extend(&chars[i + 1..j]);
                    out.push('}');
                    i = j;
                } else {
                    out.push_str("$$");
                    i += 1;
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Apply one (compiled pattern, translated replacement) over `subject` up to
/// `limit` times (`limit < 0` = unlimited).
fn replace_one(re: &Regex, repl: &str, subject: &str, limit: i64) -> String {
    if limit < 0 {
        re.replace_all(subject, repl).into_owned()
    } else {
        re.replacen(subject, limit as usize, repl).into_owned()
    }
}

/// Collect a pattern argument that may be a single string or an array of
/// patterns. Returns the ordered list of pattern strings.
fn pattern_list(v: &Value) -> Vec<String> {
    with_host(|h| {
        if h.is_array(v) {
            h.array_pairs(v)
                .unwrap_or_default()
                .into_iter()
                .map(|(_, p)| h.to_str(&p))
                .collect()
        } else {
            vec![h.to_str(v)]
        }
    })
}

fn preg_replace(args: &[Value]) -> Result<Value, String> {
    let pats = pattern_list(&arg(args, 0));
    let repl_arg = arg(args, 1);
    let repls: Vec<String> = pattern_list(&repl_arg); // reuse: string or array
    let repl_is_array = with_host(|h| h.is_array(&repl_arg));
    let limit = args.get(3).map(|v| v.to_int()).unwrap_or(-1);

    // Pre-compile the patterns; a bad pattern makes the whole call return null.
    let mut compiled: Vec<(Regex, String)> = Vec::with_capacity(pats.len());
    for (idx, p) in pats.iter().enumerate() {
        let re = match compile(p) {
            Ok(r) => r,
            Err(_) => return Ok(Value::Undef),
        };
        let repl = if repl_is_array {
            // Fewer replacements than patterns → the surplus patterns delete.
            repls.get(idx).cloned().unwrap_or_default()
        } else {
            repls.first().cloned().unwrap_or_default()
        };
        compiled.push((re, translate_replacement(&repl)));
    }

    let subj = arg(args, 2);
    let apply = |s: &str| -> String {
        let mut cur = s.to_string();
        for (re, repl) in &compiled {
            cur = replace_one(re, repl, &cur, limit);
        }
        cur
    };

    if with_host(|h| h.is_array(&subj)) {
        let pairs = with_host(|h| h.array_pairs(&subj)).unwrap_or_default();
        Ok(make_map(
            pairs
                .into_iter()
                .map(|(k, v)| (k, Value::str(apply(&with_host(|h| h.to_str(&v))))))
                .collect(),
        ))
    } else {
        Ok(Value::str(apply(&with_host(|h| h.to_str(&subj)))))
    }
}

fn preg_replace_callback(args: &[Value]) -> Result<Value, String> {
    let pats = pattern_list(&arg(args, 0));
    let cb = arg(args, 1);
    let limit = args.get(3).map(|v| v.to_int()).unwrap_or(-1);

    let mut compiled: Vec<Regex> = Vec::with_capacity(pats.len());
    for p in &pats {
        match compile(p) {
            Ok(r) => compiled.push(r),
            Err(_) => return Ok(Value::Undef),
        }
    }

    let subj = arg(args, 2);
    let run = |s: &str| -> Result<String, String> {
        let mut cur = s.to_string();
        for re in &compiled {
            cur = replace_all_cb(re, &cur, &cb, limit)?;
        }
        Ok(cur)
    };

    if with_host(|h| h.is_array(&subj)) {
        let pairs = with_host(|h| h.array_pairs(&subj)).unwrap_or_default();
        let mut out = Vec::with_capacity(pairs.len());
        for (k, v) in pairs {
            let s = with_host(|h| h.to_str(&v));
            out.push((k, Value::str(run(&s)?)));
        }
        Ok(make_map(out))
    } else {
        let s = with_host(|h| h.to_str(&subj));
        Ok(Value::str(run(&s)?))
    }
}

/// Replace every (up to `limit`) match of `re` in `s`, calling `cb($matches)`
/// for each and substituting its string return. Propagates a thrown exception.
fn replace_all_cb(re: &Regex, s: &str, cb: &Value, limit: i64) -> Result<String, String> {
    let mut out = String::new();
    let mut last = 0;
    for (n, caps) in re.captures_iter(s).enumerate() {
        if limit >= 0 && n as i64 >= limit {
            break;
        }
        let whole = caps.get(0).unwrap();
        out.push_str(&s[last..whole.start()]);
        let matches = make_list(caps_trimmed(&caps));
        let ret = crate::host::call_value(cb.clone(), vec![matches])?;
        if crate::host::has_pending_throw() {
            return Ok(String::new());
        }
        out.push_str(&with_host(|h| h.to_str(&ret)));
        last = whole.end();
    }
    out.push_str(&s[last..]);
    Ok(out)
}

// ── preg_split ───────────────────────────────────────────────────────────────

fn preg_split(args: &[Value]) -> Result<Value, String> {
    let pat = with_host(|h| h.to_str(&arg(args, 0)));
    let subject = with_host(|h| h.to_str(&arg(args, 1)));
    let limit = args.get(2).map(|v| v.to_int()).unwrap_or(-1);
    let flags = args.get(3).map(|v| v.to_int()).unwrap_or(0);
    let re = match compile(&pat) {
        Ok(r) => r,
        Err(_) => return Ok(Value::bool(false)),
    };

    let no_empty = flags & SPLIT_NO_EMPTY != 0;
    let delim_capture = flags & SPLIT_DELIM_CAPTURE != 0;
    let offset_capture = flags & SPLIT_OFFSET_CAPTURE != 0;
    // limit <= 0 (and the PHP default -1) means no limit; limit == 1 returns the
    // whole string unsplit.
    let cap: usize = if limit <= 0 { usize::MAX } else { limit as usize };

    let mut pieces: Vec<(String, usize)> = Vec::new();
    let mut last = 0;
    // `captures_iter` yields non-overlapping matches and self-advances past a
    // zero-width match, so an empty pattern (`//`) still splits between every
    // character exactly as PCRE does — no manual loop guard needed.
    for caps in re.captures_iter(&subject) {
        let whole = caps.get(0).unwrap();
        // Honour the limit: once cap-1 pieces are emitted, stop splitting so the
        // final piece holds the remainder.
        if pieces.len() + 1 >= cap {
            break;
        }
        pieces.push((subject[last..whole.start()].to_string(), last));
        if delim_capture {
            for g in 1..caps.len() {
                if let Some(m) = caps.get(g) {
                    pieces.push((m.as_str().to_string(), m.start()));
                }
            }
        }
        last = whole.end();
    }
    pieces.push((subject[last..].to_string(), last));

    let mut out: Vec<Value> = Vec::with_capacity(pieces.len());
    for (piece, off) in pieces {
        if no_empty && piece.is_empty() {
            continue;
        }
        if offset_capture {
            out.push(make_list(vec![Value::str(piece), Value::int(off as i64)]));
        } else {
            out.push(Value::str(piece));
        }
    }
    Ok(make_list(out))
}

// ── preg_quote ───────────────────────────────────────────────────────────────

fn preg_quote(args: &[Value]) -> Result<Value, String> {
    let s = with_host(|h| h.to_str(&arg(args, 0)));
    let delim = with_host(|h| h.to_str(&arg(args, 1)));
    let delim_ch = delim.chars().next();
    // The exact PCRE special set escaped by PHP's preg_quote.
    const SPECIAL: &str = ".\\+*?[^]$(){}=!<>|:-#";
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '\0' {
            out.push_str("\\000");
        } else if SPECIAL.contains(c) || Some(c) == delim_ch {
            out.push('\\');
            out.push(c);
        } else {
            out.push(c);
        }
    }
    Ok(Value::str(out))
}

// ── preg_grep ────────────────────────────────────────────────────────────────

fn preg_grep(args: &[Value]) -> Result<Value, String> {
    let pat = with_host(|h| h.to_str(&arg(args, 0)));
    let input = arg(args, 1);
    let flags = args.get(2).map(|v| v.to_int()).unwrap_or(0);
    let invert = flags & GREP_INVERT != 0;
    let re = match compile(&pat) {
        Ok(r) => r,
        Err(_) => return Ok(Value::bool(false)),
    };
    let pairs = with_host(|h| h.array_pairs(&input)).unwrap_or_default();
    let mut kept: Vec<(Value, Value)> = Vec::new();
    for (k, v) in pairs {
        let s = with_host(|h| h.to_str(&v));
        if re.is_match(&s) != invert {
            kept.push((k, v));
        }
    }
    Ok(make_map(kept))
}

// ── local arg helpers (mirror `stdlib::common`, private to this module) ───────

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Undef)
}

fn make_list(vals: Vec<Value>) -> Value {
    with_host(|h| {
        let arr = h.new_array();
        for v in vals {
            h.arr_push_auto(&arr, v);
        }
        arr
    })
}

fn make_map(pairs: Vec<(Value, Value)>) -> Value {
    with_host(|h| {
        let arr = h.new_array();
        for (k, v) in pairs {
            h.arr_set_key(&arr, &k, v);
        }
        arr
    })
}
