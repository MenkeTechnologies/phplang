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
//! A pattern the engine will not take splits into two cases, and only one of
//! them is visible. A fault the REFERENCE also diagnoses — a bad delimiter, an
//! unknown modifier, an empty expression, or one of the five structural body
//! faults ported from PCRE2 — raises `Warning: <fn>(): <reason>` and leaves
//! `preg_last_error()` at `PREG_INTERNAL_ERROR`, exactly as the reference does.
//! A pattern the reference would have COMPILED but this engine cannot returns
//! the sentinel silently and leaves the error state alone: there is no
//! diagnostic to copy, and inventing one would print what the reference never
//! prints. See `compile` and `scan_body`.
//!
//! The `A` modifier (PCRE2_ANCHORED) is applied by [`Pattern::captures_all`].
//! The engine exposes no anchored-search flag, but it is leftmost-first, so a
//! search started at `pos` yields the match beginning at `pos` when one exists
//! — `start() == pos` is therefore exactly the anchored question, and the match
//! returned is the anchored match.
//!
//! `J S X r` are accepted no-ops and none of them changes a result here: `S` and
//! `X` carry no semantics for patterns this engine accepts, `r` has no
//! observable effect, and `J` (duplicate group names) describes patterns the
//! Rust engine rejects outright, so they take the silent `Unsupported` path
//! rather than matching wrongly.
//!
//! DIVERGENCE — `D` (PCRE2_DOLLAR_ENDONLY) is accepted and ignored. It is the
//! inverse of the usual engine-subset gap: Rust's `$` is already end-of-haystack
//! only, so `/a$/D` is right by accident and the *unmodified* `/a$/` is what
//! differs — `preg_match("/a$/", "a\n")` is 1 in the reference and 0 here.
//! Fixing it means translating a trailing `$` to `(?:\n?\z)` in `scan_body`
//! rather than honouring `D`.
//!
//! One further engine nuance affects `preg_split`: Rust's `regex` suppresses a
//! zero-width match sitting immediately after a non-empty match, whereas PCRE
//! emits it. This diverges only for a pattern that can match *both* empty and
//! non-empty text at interleaved positions (`/x*/`, `/\d*/`); ordinary delimiter
//! patterns and the fully-empty `//` pattern split identically to PCRE.
//!
//! Matching runs on **bytes** by default (`regex::bytes::Regex`), mirroring PCRE
//! without the `/u` flag: `.` matches one byte, `\d\w\s` are ASCII, and all
//! offsets are byte offsets. The `/u` modifier switches the engine to Unicode
//! mode, where `.` matches a whole codepoint and the classes become Unicode-aware
//! — e.g. `preg_match("/^.$/", "é")` is `0` (2 bytes) without `/u` but `1` with
//! it. Subjects are UTF-8; byte slices are decoded back with lossy UTF-8.
//!
//! `$matches` by-reference out-parameter (`preg_match`, `preg_match_all`): the
//! stdlib dispatch chain receives call arguments *by value*, so the captures
//! travel back the way a user function's `&$x` parameter does — published at
//! the parameter's position (`PhpHost::byref_out_put`) for the call site to
//! store into the caller's variable. `preg_match($p, $s, $m)` therefore defines
//! `$m` whether or not the caller initialised it, and an array the caller *did*
//! pass is also written through, so a handle they kept elsewhere sees the same
//! captures.

use crate::host::with_host;
use fusevm::Value;

// PCRE (PHP without `/u`) matches BYTES, not Unicode codepoints. The bytes
// engine is the default; the `/u` flag re-enables Unicode on the builder.
use regex::bytes::{Captures, Regex, RegexBuilder};

/// Decode a matched byte slice back to a `String`. Subjects are UTF-8, but a
/// byte-mode match may land mid-codepoint, so decode lossily.
fn bstr(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

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
        // Pure readers: they report the state the last matching call left and
        // never clear it, so calling either one twice gives the same answer.
        "preg_last_error" => Ok(Value::int(with_host(|h| h.preg_error()))),
        "preg_last_error_msg" => Ok(Value::str(error_msg(with_host(|h| h.preg_error())))),
        _ => return None,
    })
}

// ── error state ──────────────────────────────────────────────────────────────

/// `PREG_NO_ERROR`.
pub const PREG_NO_ERROR: i64 = 0;
/// `PREG_INTERNAL_ERROR` — what a pattern the engine could not compile leaves
/// behind. PHP does not distinguish a bad delimiter from a bad body here: every
/// compile-time fault reports this one code.
pub const PREG_INTERNAL_ERROR: i64 = 1;

/// The `preg_last_error_msg()` text for an error code.
fn error_msg(code: i64) -> &'static str {
    match code {
        PREG_NO_ERROR => "No error",
        PREG_INTERNAL_ERROR => "Internal error",
        2 => "Backtrack limit exhausted",
        3 => "Recursion limit exhausted",
        4 => "Malformed UTF-8 characters, possibly incorrectly encoded",
        5 => "The offset did not correspond to the beginning of a valid UTF-8 code point",
        6 => "JIT stack limit exhausted",
        _ => "Internal error",
    }
}

// ── delimiter / flag parsing ─────────────────────────────────────────────────

/// Why a pattern did not produce a usable matcher.
enum PatternError {
    /// A fault the reference diagnoses itself: it prints
    /// `Warning: <fn>(): <msg>`, returns the function's error sentinel, and
    /// leaves `preg_last_error()` at `PREG_INTERNAL_ERROR`.
    Php(String),
    /// A pattern PCRE compiles but the Rust engine does not — a backreference,
    /// look-around, or the `n` modifier. The reference would have MATCHED, so
    /// there is no diagnostic to copy: this is the pre-existing engine-subset
    /// divergence (see the module header), and it stays silent rather than
    /// inventing a warning the reference never prints.
    Unsupported,
}

/// A compiled pattern plus the modifiers the engine cannot carry itself.
///
/// Only `A` (PCRE2_ANCHORED) needs this so far. It is not a property of the
/// compiled regex but of every search made with it, so it has to travel
/// alongside — see [`Pattern::captures_all`] for how it is applied.
pub(crate) struct Pattern {
    re: Regex,
    /// `A`: each match attempt must begin exactly at the offset it starts from.
    anchored: bool,
}

impl Pattern {
    /// Every non-overlapping match, left to right.
    ///
    /// Unanchored, this is the engine's own iterator. Anchored, PCRE2 retries at
    /// each *successive* offset and stops at the first one that does not match
    /// there — which is why `preg_match_all("/a/A", "aab")` is 2 but
    /// `"bab"` is 0.
    ///
    /// The engine exposes no anchored-search flag, but it is leftmost-first: a
    /// search started at `pos` returns the match beginning at `pos` if one
    /// exists, so `start() == pos` is exactly the anchored question, and the
    /// match it returns is the anchored match.
    fn captures_all<'h>(&self, hay: &'h [u8]) -> Vec<Captures<'h>> {
        if !self.anchored {
            return self.re.captures_iter(hay).collect();
        }
        let mut out = Vec::new();
        let mut pos = 0usize;
        // `captures_at` panics past `hay.len()`, and `len` itself is a valid
        // start — that is where a trailing zero-width match lives.
        while pos <= hay.len() {
            let Some(caps) = self.re.captures_at(hay, pos) else {
                break;
            };
            let m = caps.get(0).expect("group 0 always participates");
            if m.start() != pos {
                break;
            }
            let end = m.end();
            out.push(caps);
            // A zero-width match would otherwise spin on the same offset.
            pos = if end == pos { pos + 1 } else { end };
        }
        out
    }

    /// The first match, honouring `A`.
    fn captures_first<'h>(&self, hay: &'h [u8]) -> Option<Captures<'h>> {
        if !self.anchored {
            return self.re.captures(hay);
        }
        self.re
            .captures_at(hay, 0)
            .filter(|c| c.get(0).is_some_and(|m| m.start() == 0))
    }

    fn is_match(&self, hay: &[u8]) -> bool {
        self.captures_first(hay).is_some()
    }

    /// Group count including group 0, for `preg_match_all`'s row shape.
    fn captures_len(&self) -> usize {
        self.re.captures_len()
    }
}

/// Parse a PHP PCRE pattern (`/body/flags`, `#body#`, `~body~`, `{body}`, …)
/// into a compiled [`Pattern`].
///
/// The accepted modifier set is `imnrsuxADJSUX`, established by running all 62
/// alphanumerics through `preg_match("/a/$c", "a")` on the reference rather than
/// from memory; every other letter is `Unknown modifier '<c>'`.
///
/// The delimiter scan is a port of `php_pcre.c`'s, not a lookalike, because the
/// two disagree on real patterns: PHP scans FORWARD from the opening delimiter
/// honouring backslash escapes (so `/a\//` has body `a\/` and compiles), and for
/// a bracket delimiter it COUNTS NESTING (so `{a{b}` is unterminated even though
/// it ends in `}`). Scanning backwards for the last delimiter character, which
/// is the obvious implementation, gets both of those wrong.
fn compile(pattern: &str) -> Result<Pattern, PatternError> {
    let chars: Vec<char> = pattern.chars().collect();
    // Leading whitespace is allowed before the opening delimiter.
    let mut i = 0;
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    if i >= chars.len() {
        return Err(PatternError::Php("Empty regular expression".into()));
    }
    let open = chars[i];
    // The delimiter test is on the CHARACTER, before any bracket handling: PHP
    // rejects alphanumerics and backslash outright, with one message for all.
    if open.is_alphanumeric() || open == '\\' || open == '\0' {
        return Err(PatternError::Php(
            "Delimiter must not be alphanumeric, backslash, or NUL byte".into(),
        ));
    }
    let bracket = matches!(open, '(' | '{' | '[' | '<');
    let close = match open {
        '(' => ')',
        '{' => '}',
        '[' => ']',
        '<' => '>',
        c => c,
    };

    // Forward scan for the closing delimiter. A backslash escapes the next
    // character in both styles; only a bracket style tracks nesting depth.
    let body_start = i + 1;
    let mut depth = 1usize;
    let mut j = body_start;
    let close_idx = loop {
        if j >= chars.len() {
            return Err(PatternError::Php(if bracket {
                format!("No ending matching delimiter '{close}' found")
            } else {
                format!("No ending delimiter '{close}' found")
            }));
        }
        if chars[j] == '\\' && j + 1 < chars.len() {
            j += 2;
            continue;
        }
        if chars[j] == close {
            depth -= 1;
            if depth == 0 {
                break j;
            }
        } else if bracket && chars[j] == open {
            depth += 1;
        }
        j += 1;
    };

    let body: String = chars[body_start..close_idx].iter().collect();
    let flags: String = chars[close_idx + 1..].iter().collect();

    // Default (no `/u`) is byte matching, as PCRE. `/u` opts into Unicode.
    let mut unicode = false;
    // `n` — no auto-capture. The Rust engine has no such flag, so it is applied
    // by rewriting each capturing `(` in the body as `(?:` instead.
    let mut no_auto_capture = false;
    let mut case_insensitive = false;
    let mut multi_line = false;
    let mut dot_all = false;
    let mut extended = false;
    let mut swap_greed = false;
    let mut anchored = false;
    for f in flags.chars() {
        match f {
            'i' => case_insensitive = true,
            'm' => multi_line = true,
            's' => dot_all = true,
            'x' => extended = true,
            'U' => swap_greed = true,
            'u' => unicode = true,
            'n' => no_auto_capture = true,
            'A' => anchored = true,
            // Accepted by the reference, no Rust-engine analogue → no-ops.
            'r' | 'D' | 'X' | 'S' | 'J' => {}
            // PHP tolerates trailing whitespace/newlines after the pattern.
            c if c.is_whitespace() => {}
            other => {
                return Err(PatternError::Php(format!("Unknown modifier '{other}'")));
            }
        }
    }
    let translated = match scan_body(&body, no_auto_capture) {
        Ok(t) => t,
        Err(msg) => return Err(PatternError::Php(format!("Compilation failed: {msg}"))),
    };
    let mut b = RegexBuilder::new(&translated);
    b.case_insensitive(case_insensitive);
    b.multi_line(multi_line);
    b.dot_matches_new_line(dot_all);
    b.ignore_whitespace(extended);
    b.swap_greed(swap_greed);
    b.unicode(unicode);
    let re = b.build().map_err(|_| PatternError::Unsupported)?;
    Ok(Pattern { re, anchored })
}

// ── pattern body: PCRE2 faults, and the PCRE→Rust differences ────────────────

/// Walk a pattern BODY once, doing the two things that need the same parse:
/// report the PCRE2 compile error for a structurally malformed body (`Err`), and
/// rewrite the constructs where PCRE and the Rust engine disagree on syntax
/// (`Ok`, the body to hand to the builder).
///
/// One scan rather than two because the answers depend on the same state — which
/// characters are inside a class, which `{` opens a quantifier, which `(`
/// captures. Splitting them lets the validator and the rewriter drift into
/// disagreeing about a pattern, and the disagreement would be silent.
///
/// The error half is a PARTIAL port, deliberately. PCRE2's table has ~100
/// entries and its offsets come from wherever its parser happened to stop;
/// guessing at one would print a confidently wrong `at offset N`. Only the five
/// faults whose offset rule was read back off the reference for a spread of
/// patterns are reported (each rule is stated at its site below). A body
/// malformed in some OTHER way still fails to compile — the Rust engine rejects
/// it too — and takes the silent [`PatternError::Unsupported`] path, which is
/// where it already went.
///
/// The direction that matters is false POSITIVES: claiming a fault in a body
/// PCRE accepts would break working patterns. Hence the scan tracks escapes and
/// character classes, inside which `(`, `)` and quantifiers are all literal.
fn scan_body(body: &str, no_auto_capture: bool) -> Result<String, String> {
    let c: Vec<char> = body.chars().collect();
    let mut out = String::with_capacity(body.len());
    let mut depth: usize = 0;
    // Whether a quantifier (`*`, `+`, `?`, `{n,m}`) may legally appear here —
    // true only just after something repeatable. It is false at the start of the
    // pattern, after `(`, after `|`, and after another quantifier, which is
    // exactly when PCRE says "quantifier does not follow a repeatable item".
    let mut repeatable = false;
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            // An escape consumes the next character; the pair is repeatable.
            '\\' if i + 1 < c.len() => {
                out.push(c[i]);
                out.push(c[i + 1]);
                repeatable = true;
                i += 2;
            }
            // A character class: everything up to the terminating `]` is literal.
            // `]` in the first position (after an optional `^`) is a literal `]`,
            // not the terminator — PCRE's rule, and why `/[]/` is unterminated.
            '[' => {
                let mut k = i + 1;
                if k < c.len() && c[k] == '^' {
                    k += 1;
                }
                if k < c.len() && c[k] == ']' {
                    k += 1;
                }
                loop {
                    if k >= c.len() {
                        // Offset is the END of the body — PCRE reports where it
                        // ran out, not where the class opened.
                        return Err(format!(
                            "missing terminating ] for character class at offset {}",
                            c.len()
                        ));
                    }
                    if c[k] == '\\' {
                        k += 2;
                        continue;
                    }
                    if c[k] == ']' {
                        break;
                    }
                    k += 1;
                }
                out.extend(&c[i..=k]);
                repeatable = true;
                i = k + 1;
            }
            '(' => {
                depth += 1;
                repeatable = false;
                i += 1;
                if i < c.len() && c[i] == '?' {
                    // `(?:`, `(?=`, `(?<name>`, `(?i)` … — the `?` belongs to the
                    // group syntax and is NOT a quantifier. Emitting it here is
                    // what keeps `(?` from being read as one, which is why the
                    // reference calls `/(?/` a missing closing parenthesis.
                    out.push_str("(?");
                    i += 1;
                } else if no_auto_capture {
                    // `/n` — an unnamed group does not capture.
                    out.push_str("(?:");
                } else {
                    out.push('(');
                }
            }
            ')' => {
                if depth == 0 {
                    // Offset is one PAST the offending `)`.
                    return Err(format!("unmatched closing parenthesis at offset {}", i + 1));
                }
                depth -= 1;
                repeatable = true;
                out.push(')');
                i += 1;
            }
            '|' => {
                repeatable = false;
                out.push('|');
                i += 1;
            }
            '*' | '+' | '?' => {
                if !repeatable {
                    // Offset is one PAST the offending quantifier.
                    return Err(format!(
                        "quantifier does not follow a repeatable item at offset {}",
                        i + 1
                    ));
                }
                // `a*?` / `a*+` — a lazy or possessive suffix on a quantifier is
                // part of it, not a second quantifier; either way what follows
                // may not be quantified again.
                repeatable = false;
                out.push(c[i]);
                i += 1;
                if i < c.len() && (c[i] == '?' || c[i] == '+') {
                    out.push(c[i]);
                    i += 1;
                }
            }
            '{' => match brace_quantifier(&c, i) {
                // Not a quantifier at all (`a{x}`, `a{`): PCRE reads `{` as a
                // literal, which IS repeatable. Rust's parser has no such
                // fallback and rejects the pattern, so escape it.
                None => {
                    out.push_str("\\{");
                    repeatable = true;
                    i += 1;
                }
                Some((lo, hi, end)) => {
                    if !repeatable {
                        return Err(format!(
                            "quantifier does not follow a repeatable item at offset {}",
                            end + 1
                        ));
                    }
                    if let (Some(lo), Some(hi)) = (lo, hi) {
                        if lo > hi {
                            // Offset is the closing `}` itself here, not one past
                            // it — PCRE reports this one while still on the brace.
                            return Err(format!(
                                "numbers out of order in {{}} quantifier at offset {end}"
                            ));
                        }
                    }
                    // `{,m}` is `{0,m}` in PCRE2; Rust's parser does not take the
                    // open lower bound, so spell it out.
                    match lo {
                        Some(_) => out.extend(&c[i..=end]),
                        None => {
                            out.push_str("{0");
                            out.extend(&c[i + 1..=end]);
                        }
                    }
                    repeatable = false;
                    i = end + 1;
                    if i < c.len() && (c[i] == '?' || c[i] == '+') {
                        out.push(c[i]);
                        i += 1;
                    }
                }
            },
            other => {
                out.push(other);
                repeatable = true;
                i += 1;
            }
        }
    }
    if depth > 0 {
        // As with the character class, the offset is where the body ran out.
        return Err(format!("missing closing parenthesis at offset {}", c.len()));
    }
    Ok(out)
}

/// Read a `{n}` / `{n,}` / `{n,m}` / `{,m}` quantifier starting at `open` (which
/// must be `{`), returning `(low, high, index_of_closing_brace)`. `None` when the
/// braces do not spell a quantifier, in which case PCRE treats `{` as a literal.
fn brace_quantifier(c: &[char], open: usize) -> Option<(Option<u64>, Option<u64>, usize)> {
    let mut k = open + 1;
    let digits = |k: &mut usize| -> Option<u64> {
        let start = *k;
        while *k < c.len() && c[*k].is_ascii_digit() {
            *k += 1;
        }
        (*k > start).then(|| {
            c[start..*k]
                .iter()
                .collect::<String>()
                .parse()
                .unwrap_or(u64::MAX)
        })
    };
    let lo = digits(&mut k);
    let hi = if k < c.len() && c[k] == ',' {
        k += 1;
        digits(&mut k)
    } else {
        // `{n}` — a single bound is both ends, so it can never be out of order.
        lo
    };
    // At least one bound must be present, and the braces must close.
    if lo.is_none() && hi.is_none() {
        return None;
    }
    (k < c.len() && c[k] == '}').then_some((lo, hi, k))
}

/// Compile `pat` on behalf of library function `func`, reporting a pattern fault
/// the way the reference does.
///
/// The reference does NOT throw here — a bad pattern is a `Warning` and the
/// function's error sentinel, which is why this returns `Option` rather than
/// going through the tagged-throw path that library ARGUMENT errors use.
///
/// The error state is written on every outcome that reached the compiler, which
/// is what makes it observable across calls: a successful compile clears it even
/// when the match then fails, so `preg_last_error()` reports the LAST pattern
/// the engine compiled and not the last one that failed.
fn compile_for(func: &str, pat: &str) -> Option<Pattern> {
    match compile(pat) {
        Ok(re) => {
            with_host(|h| h.set_preg_error(PREG_NO_ERROR));
            Some(re)
        }
        Err(PatternError::Php(msg)) => {
            with_host(|h| {
                h.set_preg_error(PREG_INTERNAL_ERROR);
                h.warn(format_args!("{func}(): {msg}"));
            });
            None
        }
        // Silent: the reference compiled this one, so it has no error state to
        // copy and no warning to print.
        Err(PatternError::Unsupported) => None,
    }
}

// ── preg_match / preg_match_all ──────────────────────────────────────────────

/// Full capture list for one match: index 0 is the whole match, 1.. are the
/// numbered groups, unmatched groups render as the empty string. Fixed width —
/// used by `preg_match_all` where every set must line up by group index.
fn caps_full(caps: &Captures) -> Vec<Value> {
    (0..caps.len())
        .map(|i| Value::str(caps.get(i).map(|m| bstr(m.as_bytes())).unwrap_or_default()))
        .collect()
}

/// Capture list with trailing unmatched groups dropped — PHP's `preg_match` /
/// `preg_replace_callback` behaviour. A group that did not participate but is
/// followed by one that did is still emitted as the empty string.
fn caps_trimmed(caps: &Captures) -> Vec<Value> {
    let last = (0..caps.len())
        .rfind(|&i| caps.get(i).is_some())
        .unwrap_or(0);
    (0..=last)
        .map(|i| Value::str(caps.get(i).map(|m| bstr(m.as_bytes())).unwrap_or_default()))
        .collect()
}

/// Populate a caller-supplied array handle in place with `rows` (each a list of
/// values). Only works when `target` is already an array (shared handle); a
/// no-op otherwise. The array is fully cleared and re-indexed `0..n`, so a
/// no-match (empty `rows`) resets `$matches` to `[]` rather than leaving stale
/// captures from a prior call.
///
/// By-ref limitation: the stdlib dispatch chain receives arguments by value and
/// phplang has no VM-level by-reference OUT-parameters, so this can only write
/// back when the caller pre-initialised the variable as an array (`$m = []`),
/// giving a shared handle. An uninitialised variable cannot be bound; the return
/// value (match count) is always correct regardless.
/// Deliver a by-reference OUT array — `preg_match`'s `$matches` and its
/// siblings. The value is published at the parameter's position for the call
/// site to store (which is what defines the variable when the caller passed one
/// that did not exist), and also written through the handle when the caller
/// passed an array that already did.
fn fill_out(target: &Value, pos: usize, rows: Vec<Value>) {
    with_host(|h| {
        let out = h.new_array();
        h.arr_set_reindexed(&out, rows.clone());
        h.byref_out_put(pos, out);
        if h.is_array(target) {
            h.arr_set_reindexed(target, rows);
        }
    });
}

fn preg_match(args: &[Value]) -> Result<Value, String> {
    let pat = with_host(|h| h.to_str(&arg(args, 0)));
    let subject = with_host(|h| h.to_str(&arg(args, 1)));
    let Some(re) = compile_for("preg_match", &pat) else {
        return Ok(Value::bool(false));
    };
    match re.captures_first(subject.as_bytes()) {
        Some(caps) => {
            if args.len() > 2 {
                fill_out(&args[2], 2, caps_trimmed(&caps));
            }
            Ok(Value::int(1))
        }
        None => {
            if args.len() > 2 {
                fill_out(&args[2], 2, vec![]);
            }
            Ok(Value::int(0))
        }
    }
}

fn preg_match_all(args: &[Value]) -> Result<Value, String> {
    let pat = with_host(|h| h.to_str(&arg(args, 0)));
    let subject = with_host(|h| h.to_str(&arg(args, 1)));
    let flags = args.get(3).map(|v| v.to_int()).unwrap_or(0);
    let Some(re) = compile_for("preg_match_all", &pat) else {
        return Ok(Value::bool(false));
    };
    let all: Vec<Vec<Value>> = re
        .captures_all(subject.as_bytes())
        .iter()
        .map(caps_full)
        .collect();
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
        fill_out(&args[2], 2, rows);
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

/// Apply one (compiled pattern, translated replacement) over `subject` bytes up
/// to `limit` times (`limit < 0` = unlimited).
fn replace_one(re: &Pattern, repl: &[u8], subject: &[u8], limit: i64) -> Vec<u8> {
    // Spliced by hand rather than through `replace_all`/`replacen`, which have
    // no anchored form. `expand` still supplies the group substitution, so the
    // replacement syntax is unchanged.
    let mut out: Vec<u8> = Vec::new();
    let mut last = 0usize;
    for (n, caps) in re.captures_all(subject).into_iter().enumerate() {
        if limit >= 0 && n as i64 >= limit {
            break;
        }
        let whole = caps.get(0).expect("group 0 always participates");
        out.extend_from_slice(&subject[last..whole.start()]);
        caps.expand(repl, &mut out);
        last = whole.end();
    }
    out.extend_from_slice(&subject[last..]);
    out
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
    let mut compiled: Vec<(Pattern, String)> = Vec::with_capacity(pats.len());
    for (idx, p) in pats.iter().enumerate() {
        let Some(re) = compile_for("preg_replace", p) else {
            return Ok(Value::Undef);
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
        let mut cur: Vec<u8> = s.as_bytes().to_vec();
        for (re, repl) in &compiled {
            cur = replace_one(re, repl.as_bytes(), &cur, limit);
        }
        bstr(&cur)
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

    let mut compiled: Vec<Pattern> = Vec::with_capacity(pats.len());
    for p in &pats {
        let Some(re) = compile_for("preg_replace_callback", p) else {
            return Ok(Value::Undef);
        };
        compiled.push(re);
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
fn replace_all_cb(re: &Pattern, s: &str, cb: &Value, limit: i64) -> Result<String, String> {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut last = 0;
    for (n, caps) in re.captures_all(bytes).into_iter().enumerate() {
        if limit >= 0 && n as i64 >= limit {
            break;
        }
        let whole = caps.get(0).unwrap();
        out.extend_from_slice(&bytes[last..whole.start()]);
        let matches = make_list(caps_trimmed(&caps));
        let ret = crate::host::call_value(cb.clone(), vec![matches])?;
        if crate::host::has_pending_throw() {
            return Ok(String::new());
        }
        out.extend_from_slice(with_host(|h| h.to_str(&ret)).as_bytes());
        last = whole.end();
    }
    out.extend_from_slice(&bytes[last..]);
    Ok(bstr(&out))
}

// ── preg_split ───────────────────────────────────────────────────────────────

fn preg_split(args: &[Value]) -> Result<Value, String> {
    let pat = with_host(|h| h.to_str(&arg(args, 0)));
    let subject = with_host(|h| h.to_str(&arg(args, 1)));
    let limit = args.get(2).map(|v| v.to_int()).unwrap_or(-1);
    let flags = args.get(3).map(|v| v.to_int()).unwrap_or(0);
    let Some(re) = compile_for("preg_split", &pat) else {
        return Ok(Value::bool(false));
    };

    let no_empty = flags & SPLIT_NO_EMPTY != 0;
    let delim_capture = flags & SPLIT_DELIM_CAPTURE != 0;
    let offset_capture = flags & SPLIT_OFFSET_CAPTURE != 0;
    // limit <= 0 (and the PHP default -1) means no limit; limit == 1 returns the
    // whole string unsplit.
    let cap: usize = if limit <= 0 {
        usize::MAX
    } else {
        limit as usize
    };

    let bytes = subject.as_bytes();
    // `(text, offset)`. Offset is `-1` for a non-participating captured
    // delimiter (PHP's PREG_SPLIT_OFFSET_CAPTURE convention).
    let mut pieces: Vec<(String, i64)> = Vec::new();
    let mut last = 0usize;
    // `captures_iter` yields non-overlapping matches and self-advances past a
    // zero-width match, so an empty pattern (`//`) still splits between every
    // character exactly as PCRE does — no manual loop guard needed. The
    // enumerate index `n` counts split PIECES only (one text segment per match);
    // the limit therefore ignores the captured delimiters that DELIM_CAPTURE
    // interleaves.
    for (n, caps) in re.captures_all(bytes).into_iter().enumerate() {
        let whole = caps.get(0).unwrap();
        // Honour the limit: once cap-1 pieces are emitted, stop splitting so the
        // final piece holds the remainder.
        if n + 1 >= cap {
            break;
        }
        pieces.push((bstr(&bytes[last..whole.start()]), last as i64));
        if delim_capture {
            // PHP emits captured groups up to the last participating one; a
            // non-participating group *before* a participating one is emitted as
            // "" (trailing non-participating groups are dropped, like preg_match).
            if let Some(last_grp) = (1..caps.len()).rfind(|&g| caps.get(g).is_some()) {
                for g in 1..=last_grp {
                    match caps.get(g) {
                        Some(m) => pieces.push((bstr(m.as_bytes()), m.start() as i64)),
                        None => pieces.push((String::new(), -1)),
                    }
                }
            }
        }
        last = whole.end();
    }
    pieces.push((bstr(&bytes[last..]), last as i64));

    let mut out: Vec<Value> = Vec::with_capacity(pieces.len());
    for (piece, off) in pieces {
        if no_empty && piece.is_empty() {
            continue;
        }
        if offset_capture {
            out.push(make_list(vec![Value::str(piece), Value::int(off)]));
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
    let Some(re) = compile_for("preg_grep", &pat) else {
        return Ok(Value::bool(false));
    };
    let pairs = with_host(|h| h.array_pairs(&input)).unwrap_or_default();
    let mut kept: Vec<(Value, Value)> = Vec::new();
    for (k, v) in pairs {
        let s = with_host(|h| h.to_str(&v));
        if re.is_match(s.as_bytes()) != invert {
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
