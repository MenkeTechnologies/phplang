//! PHP standard-library `preg` (PCRE) functions. Part of the `stdlib` chain;
//! see `src/stdlib/mod.rs`. `dispatch` returns `None` for names it does not
//! handle.
//!
//! Backed by TWO engines, tried in that order by `compile`:
//!
//! 1. The Rust `regex` crate — linear-time, byte-oriented, and the one that runs
//!    for nearly every pattern. It is a *subset* of PCRE: no backreferences, no
//!    look-around, no atomic groups or possessive quantifiers.
//! 2. `fancy-regex` — a backtracking engine that HAS those constructs. A pattern
//!    the first engine will not compile is retried here, so `/foo(?=bar)/`,
//!    `/(?<=a)b/`, `/(a)\1/` and `/(?>a+)b/` answer instead of failing.
//!
//! The second engine matches over `&str` and is Unicode-mode only, so a pattern
//! that lands on it behaves as if `/u` were set: `.` is one codepoint rather
//! than one byte, and offsets, while still byte offsets, fall on codepoint
//! boundaries. That only diverges from PCRE for a NON-ASCII subject, and only
//! for a pattern the first engine already refused — where the previous answer
//! was the error sentinel. `U` (ungreedy) has no builder switch there and is
//! carried as a leading inline `(?U)` instead.
//!
//! A pattern NEITHER engine will take splits into two cases, and only one of
//! them is visible. A fault the REFERENCE also diagnoses — a bad delimiter, an
//! unknown modifier, an empty expression, or one of the five structural body
//! faults ported from PCRE2 — raises `Warning: <fn>(): <reason>` and leaves
//! `preg_last_error()` at `PREG_INTERNAL_ERROR`, exactly as the reference does.
//! A pattern the reference would have COMPILED but neither engine can returns
//! the sentinel silently and leaves the error state alone: there is no
//! diagnostic to copy, and inventing one would print what the reference never
//! prints. See `compile` and `scan_body`.
//!
//! A backtracking blow-up is a RUNTIME fault, not a compile-time one: PCRE
//! reports it as `PREG_BACKTRACK_LIMIT_ERROR` from `preg_last_error()` and
//! returns the sentinel, and so does this — see `Pattern::captures_all`.
//!
//! The `A` modifier (PCRE2_ANCHORED) is applied by `Pattern::captures_all`.
//! The engine exposes no anchored-search flag, but it is leftmost-first, so a
//! search started at `pos` yields the match beginning at `pos` when one exists
//! — `start() == pos` is therefore exactly the anchored question, and the match
//! returned is the anchored match.
//!
//! `J S X r` are accepted no-ops and none of them changes a result here: `S` and
//! `X` carry no semantics for patterns these engines accept, `r` has no
//! observable effect, and `J` (duplicate group names) describes patterns both
//! engines reject outright, so they take the silent `Unsupported` path rather
//! than matching wrongly.
//!
//! PCRE's end-of-subject anchors are the inverse of the usual engine-subset gap.
//! Both engines' `$` is end-of-haystack only, which is what `D`
//! (PCRE2_DOLLAR_ENDONLY) asks for, so `/a$/D` is right as it stands and the
//! *unmodified* `/a$/` is what differs: PCRE's `$` also matches just before a
//! newline that ENDS the subject, so `preg_match("/a$/", "a\n")` is 1. The only
//! construct either crate has for that is the look-ahead `END_ANCHOR`, which
//! lives on the backtracking engine — the `regex` crate has neither it nor
//! `\Z`.
//!
//! Moving every pattern containing a `$` onto that engine would be a steep
//! price, and it is not the price: the second position only EXISTS on a subject
//! that ends in a newline, which is a property of the subject, not the pattern.
//! So the compiled engine keeps the pattern and answers for every other subject,
//! and the rewritten body is built — and compiled — only when one ends in `\n`.
//! See `Pattern::dollar_variant`. `/m` and `/D` opt out entirely: both already
//! give `$` a meaning the compiled engine has.
//!
//! `\Z` is the same anchor under every modifier, so it is rewritten once at scan
//! time and needs no such variant. It is NOT spelled `\Z` on the second engine,
//! which has a `\Z` of its own that also matches before a NON-final newline
//! (`/a\Z/` on `"a\n\n"` is 1 there and 0 in the reference).
//!
//! DIVERGENCE — a subject that is not ASCII and a pattern without `/u` keep the
//! old answer for `$`. The rewritten body runs on the `&str` engine, where `.`
//! is a codepoint and PCRE without `/u` reads a byte, so routing it there would
//! trade this divergence for another (`preg_match('/^.$/', "\xc3\xa9\n")` is 0
//! in the reference, and 1 with a codepoint `.`). With `/u` the two agree and
//! the rewrite applies.
//!
//! Neither crate's own match ITERATOR walks a subject the way PCRE does — both
//! suppress a zero-width match sitting immediately after a non-empty one, and
//! neither retries an offset that matched empty for a NON-empty match there.
//! `/a*/` turns on the first difference and the lazy `/a*?/` on the second, so
//! `Pattern::captures_all` drives the walk by hand as a port of the loop in
//! `php_pcre.c`.
//!
//! Matching runs on **bytes** by default (`regex::bytes::Regex`), mirroring PCRE
//! without the `/u` flag: `.` matches one byte, `\d\w\s` are ASCII, and all
//! offsets are byte offsets. The `/u` modifier switches the engine to Unicode
//! mode, where `.` matches a whole codepoint and the classes become Unicode-aware
//! — e.g. `preg_match("/^.$/", "é")` is `0` (2 bytes) without `/u` but `1` with
//! it. Subjects are UTF-8; byte slices are decoded back with lossy UTF-8.
//!
//! `$matches` keys follow PHP's own rule: a `(?<name>…)` group is published
//! TWICE, under its name and then under its index, and the two slots hold
//! independent values rather than one shared array. A trailing group that did
//! not participate is dropped with its name — per SET, so `preg_match_all`'s
//! rows under `PREG_SET_ORDER` are ragged while `PREG_PATTERN_ORDER` keeps every
//! column at full width.
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
use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::rc::Rc;

// PCRE (PHP without `/u`) matches BYTES, not Unicode codepoints. The bytes
// engine is the default; the `/u` flag re-enables Unicode on the builder.
use regex::bytes::{Regex, RegexBuilder};

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
// PHP `$matches`-shaping flag bits, shared by preg_match, preg_match_all and
// preg_replace_callback. They occupy their own high bits precisely so they can
// be OR-ed with an order flag, so both must be tested with `&`.
const OFFSET_CAPTURE: i64 = 256;
const UNMATCHED_AS_NULL: i64 = 512;

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
/// `PREG_BACKTRACK_LIMIT_ERROR` — a run-time failure, not a compile-time one:
/// the pattern was fine, the subject made the backtracking engine give up.
pub const PREG_BACKTRACK_LIMIT_ERROR: i64 = 2;

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

/// One capture group's byte span in the subject, plus the subject itself so the
/// text can be read back. The engine-neutral stand-in for either crate's
/// `Match`.
#[derive(Clone, Copy)]
pub(crate) struct Span<'h> {
    start: usize,
    end: usize,
    hay: &'h [u8],
}

impl<'h> Span<'h> {
    fn start(&self) -> usize {
        self.start
    }
    fn end(&self) -> usize {
        self.end
    }
    fn as_bytes(&self) -> &'h [u8] {
        &self.hay[self.start..self.end]
    }
}

/// One match, normalised across the two engines: group 0 first, then the
/// numbered groups in order, each either a byte span or absent (the group did
/// not participate).
pub(crate) struct Caps<'h> {
    groups: Vec<Option<(usize, usize)>>,
    hay: &'h [u8],
}

impl<'h> Caps<'h> {
    fn len(&self) -> usize {
        self.groups.len()
    }

    fn get(&self, i: usize) -> Option<Span<'h>> {
        self.groups.get(i).copied().flatten().map(|(s, e)| Span {
            start: s,
            end: e,
            hay: self.hay,
        })
    }

    /// Substitute group references in a replacement template that
    /// [`translate_replacement`] has already normalised to the `regex` crate's
    /// syntax — `${N}` for a group, `$$` for a literal `$`, everything else
    /// literal. A reference to a group that does not exist or did not
    /// participate expands to nothing, as it does in both crates and in PCRE.
    ///
    /// Hand-rolled rather than delegated because the two engines' own
    /// `expand` methods take different string types, and the substitution has to
    /// be identical whichever engine produced the match.
    fn expand(&self, repl: &[u8], out: &mut Vec<u8>) {
        let mut i = 0usize;
        while i < repl.len() {
            if repl[i] != b'$' {
                out.push(repl[i]);
                i += 1;
                continue;
            }
            // `$$` — one literal dollar.
            if repl.get(i + 1) == Some(&b'$') {
                out.push(b'$');
                i += 2;
                continue;
            }
            // `${N}` or the bare `$N`, both of which the `regex` crate accepts.
            let (digits_at, braced) = match repl.get(i + 1) {
                Some(b'{') => (i + 2, true),
                _ => (i + 1, false),
            };
            let mut j = digits_at;
            while j < repl.len() && repl[j].is_ascii_digit() {
                j += 1;
            }
            let closed = !braced || repl.get(j) == Some(&b'}');
            if j == digits_at || !closed {
                // Not a group reference after all: a lone `$`.
                out.push(b'$');
                i += 1;
                continue;
            }
            let n: usize = std::str::from_utf8(&repl[digits_at..j])
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(usize::MAX);
            if let Some(m) = self.get(n) {
                out.extend_from_slice(m.as_bytes());
            }
            i = if braced { j + 1 } else { j };
        }
    }
}

/// Which engine compiled a pattern. See the module header for why there are two.
enum Engine {
    /// The `regex` crate over bytes — the default, and PCRE's byte semantics.
    Bytes(Regex),
    /// `fancy-regex` over `&str` — look-around, backreferences, atomic groups.
    Fancy(fancy_regex::Regex),
}

/// The look-ahead that spells out PCRE's default end-of-subject anchor: the end
/// of the subject, or just before a newline that ENDS it. Neither engine's `$`
/// means that (both are end-of-subject only) and neither has PCRE's `\Z`, so a
/// body carrying either is rewritten to this — see [`scan_body`].
///
/// `fancy-regex` DOES have a `\Z`, and it is not this one: `\Z` there also
/// matches before a newline that is not final, so `/a\Z/` on `"a\n\n"` answers 1
/// where the reference answers 0. The look-ahead is what agrees; `\Z` was
/// measured against the reference and rejected.
const END_ANCHOR: &str = "(?=\\n?\\z)";

/// The `$`-rewritten form of a pattern, compiled on demand.
///
/// It exists because PCRE's default `$` has a second match position — one before
/// a newline that ends the subject — that only shows on a subject that ends in a
/// newline. That is a property of the SUBJECT, not of the pattern, which is what
/// keeps the cost off the common path: the compiled engine already answers
/// correctly for every subject that does not end in `\n`, and this variant is
/// built (and even compiled) only when one does.
struct DollarNl {
    /// The body with each anchoring `$` replaced by [`END_ANCHOR`].
    body: String,
    /// `/u` was set, so PCRE is in UTF mode and agrees with the codepoint-
    /// oriented engine about a non-ASCII subject. Without it the two read a
    /// non-ASCII byte differently and this variant is not used — see
    /// [`Pattern::dollar_variant`].
    unicode: bool,
    /// Built on first need. `None` once built means the rewritten body is one
    /// the backtracking engine will not take, and the compiled engine answers.
    re: std::cell::OnceCell<Option<fancy_regex::Regex>>,
    /// The same rewritten body compiled to reject a zero-length match — the
    /// NOTEMPTY companion of [`Pattern::notempty`], for this variant.
    notempty: std::cell::OnceCell<Option<fancy_regex::Regex>>,
}

/// The builder settings a pattern was compiled with, kept so the NOTEMPTY
/// companion of [`Pattern`] can be built from the same body later.
#[derive(Clone)]
struct Flags {
    case_insensitive: bool,
    multi_line: bool,
    dot_all: bool,
    extended: bool,
    swap_greed: bool,
}

/// A compiled pattern plus the modifiers the engine cannot carry itself.
///
/// `A` (PCRE2_ANCHORED) is one: it is not a property of the compiled regex but
/// of every search made with it, so it has to travel alongside — see
/// [`Pattern::captures_all`]. The translated body and its flags travel too, for
/// the same method's empty-match retry.
pub(crate) struct Pattern {
    engine: Engine,
    /// `A`: each match attempt must begin exactly at the offset it starts from.
    anchored: bool,
    /// The body handed to the builder, in the engines' own syntax.
    body: String,
    flags: Flags,
    /// The same body compiled to reject a zero-length match — PCRE2_NOTEMPTY.
    /// Built on first need, because only a pattern that actually matches empty
    /// ever asks for it, and `None` once built means it cannot exist (a pattern
    /// that matches ONLY empty, or a body the second engine will not take).
    notempty: std::cell::OnceCell<Option<fancy_regex::Regex>>,
    /// The `$`-rewritten form, for a subject that ends in a newline. `None` when
    /// the body has no anchoring `$`, or when `/m` or `/D` already gives the
    /// compiled engine's `$` the right meaning.
    dollar_nl: Option<DollarNl>,
}

impl Pattern {
    /// Every non-overlapping match, left to right — a port of the match loop in
    /// `php_pcre.c`, which is not the same walk as either crate's own iterator.
    ///
    /// PCRE records an empty match and then RETRIES the same offset demanding a
    /// non-empty one (`PCRE2_NOTEMPTY_ATSTART | PCRE2_ANCHORED`), only stepping
    /// forward a character when that retry fails. Both crates instead step
    /// immediately, which loses the non-empty match at that offset: for the lazy
    /// `/a*?/` over `"abc"` PCRE finds five matches (`""`, `"a"`, `""`, `""`,
    /// `""`) and a plain step finds four, so `preg_replace` produced `zazbzcz`
    /// where the reference produces `zzzbzcz`. The retry is what makes those
    /// agree.
    ///
    /// `A` (anchored) rides along: PCRE2 retries at each *successive* offset and
    /// stops at the first one that does not match there, which is why
    /// `preg_match_all("/a/A", "aab")` is 2 but `"bab"` is 0. Neither engine
    /// exposes an anchored-search flag, but both are leftmost-first: a search
    /// started at `pos` returns the match beginning at `pos` if one exists, so
    /// `start() == pos` is exactly the anchored question.
    ///
    /// A backtracking failure on the second engine (its backtrack limit) records
    /// `PREG_BACKTRACK_LIMIT_ERROR` and truncates the match list there, which is
    /// what PCRE reports for the same subject.
    fn captures_all<'h>(&self, hay: &'h [u8]) -> Vec<Caps<'h>> {
        self.captures_all_from(hay, 0)
    }

    /// Every match at or after byte `from` — `preg_match_all`'s `$offset`.
    ///
    /// As in [`Pattern::captures_first_from`], the subject is not sliced: only
    /// the starting position of the walk moves, so anchors and look-behind keep
    /// seeing the whole string.
    fn captures_all_from<'h>(&self, hay: &'h [u8], from: usize) -> Vec<Caps<'h>> {
        if from > hay.len() {
            return Vec::new();
        }
        // Resolved once for the whole walk: it depends on the subject, not the
        // offset, and the test that picks it scans the subject.
        let dn = self.dollar_variant(hay);
        let mut out = Vec::new();
        let mut pos = from;
        // PCRE's `g_notempty`: set after an empty match, it turns the next
        // attempt at the SAME offset into an anchored, non-empty one.
        let mut retry_nonempty = false;
        // A start offset of `hay.len()` is valid — that is where a trailing
        // zero-width match lives — but anything past it is not.
        while pos <= hay.len() {
            let hit = if retry_nonempty {
                self.captures_nonempty_at(dn, hay, pos)
            } else {
                self.captures_at(dn, hay, pos)
                    .filter(|c| !self.anchored || c.get(0).is_some_and(|m| m.start() == pos))
            };
            let Some(caps) = hit else {
                // A failed retry is not the end of the subject: step one whole
                // character (a byte-boundary offset is rejected outright by the
                // `&str` engine and splits a character for the byte one) and
                // search normally from there.
                if retry_nonempty && pos < hay.len() {
                    retry_nonempty = false;
                    pos = next_boundary(hay, pos);
                    continue;
                }
                break;
            };
            let m = caps.get(0).expect("group 0 always participates");
            let (start, end) = (m.start(), m.end());
            out.push(caps);
            retry_nonempty = end == start;
            pos = end;
        }
        out
    }

    /// The match at exactly `pos` that consumes at least one character, or
    /// `None` when there is none — PCRE2's `NOTEMPTY_ATSTART | ANCHORED`.
    ///
    /// Answered by a second compilation of the same body with `fancy-regex`'s
    /// `find_not_empty`, which is the only switch either crate offers for this.
    /// A body that can ONLY match empty is rejected at compile time there, and a
    /// non-UTF-8 subject cannot reach the `&str` engine at all; both are "no
    /// such match", which is the right answer in each case.
    fn captures_nonempty_at<'h>(
        &self,
        dn: Option<&DollarNl>,
        hay: &'h [u8],
        pos: usize,
    ) -> Option<Caps<'h>> {
        // The retry has to ask the same question the walk is asking, so it uses
        // the `$`-rewritten body whenever the walk does.
        let (cell, body) = match dn {
            Some(d) => (&d.notempty, &d.body),
            None => (&self.notempty, &self.body),
        };
        let re = cell
            .get_or_init(|| build_fancy(body, &self.flags, true))
            .as_ref()?;
        let caps = fancy_captures_at(re, hay, pos)?;
        // `captures_from_pos` is not anchored; PCRE's retry is.
        if caps.get(0)?.start() != pos {
            return None;
        }
        Some(caps)
    }

    /// The `$`-rewritten variant to run against `hay`, or `None` when the
    /// compiled engine already answers correctly for it.
    ///
    /// Three tests, cheapest first. A subject that does not end in `\n` has no
    /// second `$` position at all, so the engines agree and nothing is built —
    /// that is the common path, and it costs one byte comparison.
    ///
    /// A non-ASCII subject is left to the compiled engine unless `/u` is set:
    /// the rewritten body runs on the `&str` engine, where `.` is a codepoint,
    /// and without `/u` PCRE reads a byte. Routing it there would trade this
    /// divergence for another one — `preg_match('/^.$/', "\xc3\xa9\n")` is 0 in
    /// the reference, and a codepoint `.` makes it 1. With `/u` PCRE is in UTF
    /// mode too and the two agree.
    ///
    /// A rewritten body the backtracking engine will not take is `None`, which
    /// falls back to the compiled engine — the answer it gave before.
    fn dollar_variant(&self, hay: &[u8]) -> Option<&DollarNl> {
        let d = self.dollar_nl.as_ref()?;
        if !hay.ends_with(b"\n") {
            return None;
        }
        if !hay.is_ascii() && !(d.unicode && std::str::from_utf8(hay).is_ok()) {
            return None;
        }
        d.re.get_or_init(|| build_fancy(&d.body, &self.flags, false))
            .as_ref()?;
        Some(d)
    }

    /// The first match, honouring `A`.
    fn captures_first<'h>(&self, hay: &'h [u8]) -> Option<Caps<'h>> {
        self.captures_first_from(hay, 0)
    }

    /// The first match at or after byte `from` — `preg_match`'s `$offset`.
    ///
    /// `from` moves only where the SEARCH begins; the subject stays whole. That
    /// distinction is the reason this is not `captures_first(&hay[from..])`:
    /// PCRE keeps the full subject in view, so `^` still refers to the real
    /// start and a look-behind can read the bytes before `from`. Slicing would
    /// silently change both answers.
    ///
    /// An offset past the end matches nothing rather than panicking, and `A`
    /// anchors to `from`, not to zero.
    fn captures_first_from<'h>(&self, hay: &'h [u8], from: usize) -> Option<Caps<'h>> {
        if from > hay.len() {
            return None;
        }
        let dn = self.dollar_variant(hay);
        self.captures_at(dn, hay, from)
            .filter(|c| !self.anchored || c.get(0).is_some_and(|m| m.start() == from))
    }

    /// The leftmost match at or after `pos`, whichever engine holds the pattern.
    fn captures_at<'h>(
        &self,
        dn: Option<&DollarNl>,
        hay: &'h [u8],
        pos: usize,
    ) -> Option<Caps<'h>> {
        if let Some(d) = dn {
            // `dollar_variant` returns `Some` only after this compiled.
            let re = d.re.get()?.as_ref()?;
            return fancy_captures_at(re, hay, pos);
        }
        match &self.engine {
            Engine::Bytes(re) => {
                let caps = re.captures_at(hay, pos)?;
                Some(Caps {
                    groups: (0..caps.len())
                        .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
                        .collect(),
                    hay,
                })
            }
            Engine::Fancy(re) => fancy_captures_at(re, hay, pos),
        }
    }

    pub(crate) fn is_match(&self, hay: &[u8]) -> bool {
        self.captures_first(hay).is_some()
    }

    /// Group count including group 0, for `preg_match_all`'s row shape.
    fn captures_len(&self) -> usize {
        match &self.engine {
            Engine::Bytes(re) => re.captures_len(),
            Engine::Fancy(re) => re.captures_len(),
        }
    }

    /// The `(?<name>…)` label of each group by index, `None` for an unnamed one.
    /// Index 0 is the whole match and is never named.
    fn group_names(&self) -> Vec<Option<String>> {
        match &self.engine {
            Engine::Bytes(re) => re.capture_names().map(|n| n.map(str::to_string)).collect(),
            Engine::Fancy(re) => re.capture_names().map(|n| n.map(str::to_string)).collect(),
        }
    }
}

/// Emit one group of a `$matches` row the way PHP keys it: a NAMED group is
/// published twice, under its name first and then under its index, and an
/// unnamed one only under its index.
/// The two slots hold independent values, not one shared one: `preg_match_all`
/// puts an ARRAY in each, and writing through `$m['name']` must not be visible
/// through `$m[1]`, so the duplicate goes through the host's value-semantics
/// copy.
fn push_keyed(out: &mut Vec<(Value, Value)>, names: &[Option<String>], i: usize, v: Value) {
    if let Some(Some(name)) = names.get(i) {
        let dup = with_host(|h| h.copy_on_assign(v.clone()));
        out.push((Value::str(name.clone()), dup));
    }
    out.push((Value::int(i as i64), v));
}

/// Build `body` on the backtracking engine with a pattern's modifiers.
///
/// `U` (ungreedy) has no builder switch there, so it rides as a leading inline
/// `(?U)`, which applies to the whole pattern — what the modifier means.
/// `not_empty` is PCRE2_NOTEMPTY; a body that can match ONLY empty is rejected
/// with it set, which is the right answer for the retry that asks for it.
fn build_fancy(body: &str, flags: &Flags, not_empty: bool) -> Option<fancy_regex::Regex> {
    let body = if flags.swap_greed {
        format!("(?U){body}")
    } else {
        body.to_string()
    };
    let mut b = fancy_regex::RegexBuilder::new(&body);
    b.case_insensitive(flags.case_insensitive);
    b.multi_line(flags.multi_line);
    b.dot_matches_new_line(flags.dot_all);
    b.ignore_whitespace(flags.extended);
    b.find_not_empty(not_empty);
    b.build().ok()
}

/// One match attempt on the backtracking engine, which reads `&str`.
///
/// It cannot be handed a non-UTF-8 subject, nor a start offset inside a
/// codepoint. Either is a no-match rather than a panic. A backtrack blow-up is a
/// runtime fault: it records the code PCRE reports and yields no match.
fn fancy_captures_at<'h>(re: &fancy_regex::Regex, hay: &'h [u8], pos: usize) -> Option<Caps<'h>> {
    let text = std::str::from_utf8(hay).ok()?;
    if !text.is_char_boundary(pos) {
        return None;
    }
    match re.captures_from_pos(text, pos) {
        Ok(caps) => {
            let caps = caps?;
            Some(Caps {
                groups: (0..caps.len())
                    .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
                    .collect(),
                hay,
            })
        }
        Err(e) => {
            with_host(|h| h.set_preg_error(runtime_error_code(&e)));
            None
        }
    }
}

/// The next index at or after `from` that starts a UTF-8 codepoint, saturating
/// at the end of the subject. A non-UTF-8 subject only ever reaches the byte
/// engine, where every index is a valid start, so the scan stops on the first
/// non-continuation byte.
fn next_boundary(hay: &[u8], from: usize) -> usize {
    let mut i = from + 1;
    while i < hay.len() && (hay[i] & 0xC0) == 0x80 {
        i += 1;
    }
    i
}

/// Map a `fancy-regex` run-time failure onto the `preg_last_error()` code PCRE
/// reports for the same condition. Only the backtrack limit is reachable in
/// practice; anything else keeps the generic internal code.
fn runtime_error_code(e: &fancy_regex::Error) -> i64 {
    match e {
        fancy_regex::Error::RuntimeError(fancy_regex::RuntimeError::BacktrackLimitExceeded) => {
            PREG_BACKTRACK_LIMIT_ERROR
        }
        _ => PREG_INTERNAL_ERROR,
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
    // `D` — PCRE2_DOLLAR_ENDONLY. It drops `$`'s second match position, which is
    // exactly what both engines' `$` already does, so it is applied by NOT
    // rewriting. `/m` overrides it in PCRE (`/$/Dm` on "a\n" matches twice), and
    // the rewrite is off under `/m` anyway.
    let mut dollar_endonly = false;
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
            'D' => dollar_endonly = true,
            // Accepted by the reference, no Rust-engine analogue → no-ops.
            'r' | 'X' | 'S' | 'J' => {}
            // PHP tolerates trailing whitespace/newlines after the pattern.
            c if c.is_whitespace() => {}
            other => {
                return Err(PatternError::Php(format!("Unknown modifier '{other}'")));
            }
        }
    }
    let scanned = match scan_body(&body, no_auto_capture) {
        Ok(t) => t,
        Err(msg) => return Err(PatternError::Php(format!("Compilation failed: {msg}"))),
    };
    let translated = scanned.text;
    let flags = Flags {
        case_insensitive,
        multi_line,
        dot_all,
        extended,
        swap_greed,
    };
    // `/m` gives `$` its end-of-LINE meaning, which both engines already have,
    // and `/D` drops the second position entirely. The rewrite is for the
    // unmodified `$` only.
    let dollar_nl = scanned
        .dollar_nl
        .filter(|_| !multi_line && !dollar_endonly)
        .map(|body| DollarNl {
            body,
            unicode,
            re: std::cell::OnceCell::new(),
            notempty: std::cell::OnceCell::new(),
        });
    let mut b = RegexBuilder::new(&translated);
    b.case_insensitive(case_insensitive);
    b.multi_line(multi_line);
    b.dot_matches_new_line(dot_all);
    b.ignore_whitespace(extended);
    b.swap_greed(swap_greed);
    b.unicode(unicode);
    let engine = match b.build() {
        Ok(re) => Engine::Bytes(re),
        // Look-around, a backreference, an atomic group, a possessive
        // quantifier — all PCRE, none of them the `regex` crate's. Retry on the
        // backtracking engine, which has them.
        Err(_) => {
            Engine::Fancy(build_fancy(&translated, &flags, false).ok_or(PatternError::Unsupported)?)
        }
    };
    Ok(Pattern {
        engine,
        anchored,
        body: translated,
        flags,
        notempty: std::cell::OnceCell::new(),
        dollar_nl,
    })
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
fn scan_body(body: &str, no_auto_capture: bool) -> Result<Scanned, String> {
    let c: Vec<char> = body.chars().collect();
    let mut out = String::with_capacity(body.len());
    // Where an anchoring `$` was emitted into `out`, for the rewritten variant,
    // and whether the body sets `m` inline — which changes what `$` means and so
    // takes the variant off the table.
    let mut dollars: Vec<usize> = Vec::new();
    let mut inline_multi_line = false;
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
                // PCRE's `\Z` is end-of-subject-or-before-a-final-newline, which
                // no engine here has — `regex` rejects `\Z` outright and
                // `fancy-regex` spells a DIFFERENT anchor with it. Both are
                // answered by the look-ahead. Unlike `$`, `\Z` means this under
                // every modifier, so it is rewritten here once and needs no
                // subject-dependent variant.
                if c[i + 1] == 'Z' {
                    out.push_str(END_ANCHOR);
                } else {
                    out.push(c[i]);
                    out.push(c[i + 1]);
                }
                repeatable = true;
                i += 2;
            }
            // Outside a class and unescaped, `$` is always PCRE's end anchor.
            // It is emitted unchanged — the compiled engine's `$` is right for
            // every subject that does not end in a newline — and its offset is
            // recorded so the variant for the subjects that do can be spliced.
            '$' => {
                dollars.push(out.len());
                out.push('$');
                repeatable = true;
                i += 1;
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
                    // An inline flag set — `(?m)`, `(?im-sx:` — that touches `m`
                    // changes what `$` means partway through the body. The scan
                    // only reads letters and `-`, so `(?<name>`, `(?=`, `(?:`
                    // and `(?P<name>` all stop before any of them counts.
                    let mut k = i;
                    while k < c.len() && (c[k].is_ascii_alphabetic() || c[k] == '-') {
                        if c[k] == 'm' {
                            inline_multi_line = true;
                        }
                        k += 1;
                    }
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
    let dollar_nl = (!dollars.is_empty() && !inline_multi_line).then(|| {
        let mut v = String::with_capacity(out.len() + dollars.len() * END_ANCHOR.len());
        let mut last = 0;
        for &d in &dollars {
            v.push_str(&out[last..d]);
            v.push_str(END_ANCHOR);
            last = d + 1; // `$` is one byte
        }
        v.push_str(&out[last..]);
        v
    });
    Ok(Scanned {
        text: out,
        dollar_nl,
    })
}

/// What one [`scan_body`] walk produces: the body in the engines' syntax, and
/// the `$`-rewritten form for a subject that ends in a newline.
struct Scanned {
    text: String,
    /// `None` when the body has no anchoring `$`, or sets `m` inline.
    dollar_nl: Option<String>,
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
pub(crate) fn compile_for(func: &str, pat: &str) -> Option<Rc<Pattern>> {
    // The OUTCOME is memoized, and its side effects are replayed on every call.
    // That is what keeps an invalid pattern warning each time it is used rather
    // than only the first time, and keeps `preg_last_error()` reporting the
    // last pattern this call compiled.
    match cached_compile(pat) {
        Cached::Ok(re) => {
            with_host(|h| h.set_preg_error(PREG_NO_ERROR));
            Some(re)
        }
        Cached::Php(msg) => {
            with_host(|h| {
                h.set_preg_error(PREG_INTERNAL_ERROR);
                h.warn(format_args!("{func}(): {msg}"));
            });
            None
        }
        // Silent: the reference compiled this one, so it has no error state to
        // copy and no warning to print.
        Cached::Unsupported => None,
    }
}

/// A memoized compile outcome. Cloning shares the compiled engine.
#[derive(Clone)]
enum Cached {
    Ok(Rc<Pattern>),
    Php(String),
    Unsupported,
}

/// How many distinct patterns are kept before the cache is emptied.
///
/// The reference bounds its own compiled-pattern cache the same way and clears
/// it wholesale when full (`pcre_clean_cache`), which is what keeps a program
/// that builds patterns from data — `preg_match("/$needle/", …)` in a loop over
/// a large list — from growing this without limit.
const PATTERN_CACHE_LIMIT: usize = 4096;

thread_local! {
    /// Compiled patterns, keyed by the pattern EXACTLY as the program wrote it.
    ///
    /// The key is the whole argument — delimiters and trailing modifiers
    /// included — because `/a/i` and `/a/` are different engines and `#a#` and
    /// `/a/` are the same one only by accident of body.
    ///
    /// `preg_*` take their pattern as a runtime string, so a match inside a loop
    /// handed the same text to the compiler on every iteration: building the
    /// regex (`aho_corasick` NFA construction, `regex_automata` byte classes)
    /// dominated the profile of a `preg_match`/`preg_split`/`preg_replace` pass
    /// over 20k lines. The reference caches compiled patterns too, so this is
    /// parity of cost as well as of answer.
    static PATTERN_CACHE: RefCell<FxHashMap<String, Cached>> =
        RefCell::new(FxHashMap::default());
}

/// [`compile`], memoized. See [`PATTERN_CACHE`].
fn cached_compile(pat: &str) -> Cached {
    if let Some(hit) = PATTERN_CACHE.with(|c| c.borrow().get(pat).cloned()) {
        return hit;
    }
    let outcome = match compile(pat) {
        Ok(re) => Cached::Ok(Rc::new(re)),
        Err(PatternError::Php(msg)) => Cached::Php(msg),
        Err(PatternError::Unsupported) => Cached::Unsupported,
    };
    PATTERN_CACHE.with(|c| {
        let mut map = c.borrow_mut();
        if map.len() >= PATTERN_CACHE_LIMIT {
            map.clear();
        }
        map.insert(pat.to_string(), outcome.clone());
    });
    outcome
}

// ── preg_match / preg_match_all ──────────────────────────────────────────────

/// Full capture list for one match: index 0 is the whole match, 1.. are the
/// numbered groups, unmatched groups render as the empty string. Fixed width —
/// used by `preg_match_all` where every set must line up by group index.
///
/// Values only: `preg_match_all`'s PATTERN_ORDER transposes these into per-group
/// columns before keying them.
fn caps_values(caps: &Caps<'_>, fmt: CellFmt) -> Vec<Value> {
    (0..caps.len()).map(|i| fmt.cell(caps, i)).collect()
}

/// Index of the last group that took part in this match — where PHP truncates a
/// `$matches` row. Group 0 always participates, so this is never empty.
fn last_participating(caps: &Caps<'_>) -> usize {
    (0..caps.len())
        .rfind(|&i| caps.get(i).is_some())
        .unwrap_or(0)
}

/// How one `$matches` cell is shaped. Resolved once per call from the flag word
/// so every cell of a result agrees, and threaded to each of the three functions
/// that build rows.
#[derive(Clone, Copy, Default)]
struct CellFmt {
    /// `PREG_OFFSET_CAPTURE` — the cell becomes `[text, byte-offset]`.
    offset: bool,
    /// `PREG_UNMATCHED_AS_NULL` — a group that did not participate reads `null`
    /// rather than `''`.
    as_null: bool,
}

impl CellFmt {
    fn from_flags(flags: i64) -> Self {
        Self {
            offset: flags & OFFSET_CAPTURE != 0,
            as_null: flags & UNMATCHED_AS_NULL != 0,
        }
    }

    /// One cell of a `$matches` row.
    ///
    /// The offset is a BYTE position. That is what PHP reports even under `/u`,
    /// where the subject is validated and walked as UTF-8 but the position is
    /// still counted in bytes — verified against the reference rather than
    /// assumed, because a codepoint counter agrees on every ASCII subject and
    /// only ever differs where it matters.
    ///
    /// A group that did not participate reports `-1`, never `0`. Wrapping the
    /// flagless value would give `['', 0]` and claim the group matched at the
    /// start of the subject, which passes any check that only asserts the pair
    /// shape.
    fn cell(&self, caps: &Caps<'_>, i: usize) -> Value {
        let m = caps.get(i);
        let text = match &m {
            Some(m) => Value::str(bstr(m.as_bytes())),
            // PHP's null; phplang has no distinct Null variant.
            None if self.as_null => Value::Undef,
            None => Value::str(String::new()),
        };
        if !self.offset {
            return text;
        }
        let at = m.map(|m| m.start() as i64).unwrap_or(-1);
        make_list(vec![text, Value::int(at)])
    }

    /// The last group index a row carries.
    ///
    /// PHP normally truncates a `$matches` row at its last participating group,
    /// but `PREG_UNMATCHED_AS_NULL` SUPPRESSES that trim: the whole point of the
    /// flag is to report every group, so a trailing non-participating one must
    /// survive as `null` instead of being dropped.
    fn row_end(&self, caps: &Caps<'_>) -> usize {
        if self.as_null {
            caps.len().saturating_sub(1)
        } else {
            last_participating(caps)
        }
    }
}

/// A row of per-group values keyed the way PHP keys it — see [`push_keyed`].
fn key_row(values: Vec<Value>, names: &[Option<String>]) -> Vec<(Value, Value)> {
    let mut out = Vec::with_capacity(values.len());
    for (i, v) in values.into_iter().enumerate() {
        push_keyed(&mut out, names, i, v);
    }
    out
}

/// Capture list with trailing unmatched groups dropped — PHP's `preg_match` /
/// `preg_replace_callback` behaviour. A group that did not participate but is
/// followed by one that did is still emitted as the empty string. A group
/// dropped here loses its NAME key with it.
fn caps_trimmed(caps: &Caps<'_>, names: &[Option<String>], fmt: CellFmt) -> Vec<(Value, Value)> {
    let last = fmt.row_end(caps);
    let mut out = Vec::with_capacity(last + 1);
    for i in 0..=last {
        push_keyed(&mut out, names, i, fmt.cell(caps, i));
    }
    out
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
fn fill_out(target: &Value, pos: usize, rows: Vec<(Value, Value)>) {
    with_host(|h| {
        let out = h.new_array();
        for (k, v) in &rows {
            h.arr_set_key(&out, k, v.clone());
        }
        h.byref_out_put(pos, out);
        if h.is_array(target) {
            // Reindexing with nothing is the clear: it drops every entry and
            // resets the next auto index, so the refill below cannot inherit
            // stale keys from a prior call.
            h.arr_set_reindexed(target, vec![]);
            for (k, v) in rows {
                h.arr_set_key(target, &k, v);
            }
        }
    });
}

fn preg_match(args: &[Value]) -> Result<Value, String> {
    let pat = with_host(|h| h.to_str(&arg(args, 0)));
    let subject = with_host(|h| h.to_str(&arg(args, 1)));
    let fmt = CellFmt::from_flags(args.get(3).map(|v| v.to_int()).unwrap_or(0));
    let Some(re) = compile_for("preg_match", &pat) else {
        return Ok(Value::bool(false));
    };
    // Checked AFTER the pattern compiles, so a bad pattern still reports itself.
    let Some(from) = start_offset(args.get(4), subject.len()) else {
        return bad_offset(args);
    };
    match re.captures_first_from(subject.as_bytes(), from) {
        Some(caps) => {
            if args.len() > 2 {
                fill_out(&args[2], 2, caps_trimmed(&caps, &re.group_names(), fmt));
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

/// Resolve a `$offset` argument to a byte position in a subject of `len` bytes,
/// or `None` when it is out of range.
///
/// A negative offset counts back from the end and CLAMPS at zero, so `-100` on a
/// short subject starts at the beginning rather than failing. A positive offset
/// PAST the end fails instead: the reference returns `false` and leaves
/// `preg_last_error()` at `PREG_INTERNAL_ERROR`. `len` itself is in range — that
/// is where a trailing zero-width match lives — so only `> len` is rejected.
fn start_offset(v: Option<&Value>, len: usize) -> Option<usize> {
    let raw = v.map(|v| v.to_int()).unwrap_or(0);
    if raw < 0 {
        return Some((len as i64 + raw).max(0) as usize);
    }
    (raw as u64 <= len as u64).then_some(raw as usize)
}

/// The `false` an out-of-range `$offset` produces, with the error state and the
/// emptied `$matches` that accompany it.
fn bad_offset(args: &[Value]) -> Result<Value, String> {
    with_host(|h| h.set_preg_error(PREG_INTERNAL_ERROR));
    if args.len() > 2 {
        fill_out(&args[2], 2, vec![]);
    }
    Ok(Value::bool(false))
}

fn preg_match_all(args: &[Value]) -> Result<Value, String> {
    let pat = with_host(|h| h.to_str(&arg(args, 0)));
    let subject = with_host(|h| h.to_str(&arg(args, 1)));
    let flags = args.get(3).map(|v| v.to_int()).unwrap_or(0);
    let Some(re) = compile_for("preg_match_all", &pat) else {
        return Ok(Value::bool(false));
    };
    let names = re.group_names();
    let fmt = CellFmt::from_flags(flags);
    let Some(from) = start_offset(args.get(4), subject.len()) else {
        return bad_offset(args);
    };
    // Full-width values plus the index of the last group each set carries:
    // PREG_SET_ORDER truncates each set THERE (its rows are ragged), while
    // PREG_PATTERN_ORDER keeps every column at full width.
    let all: Vec<(Vec<Value>, usize)> = re
        .captures_all_from(subject.as_bytes(), from)
        .iter()
        .map(|c| (caps_values(c, fmt), fmt.row_end(c)))
        .collect();
    let count = all.len();

    if args.len() > 2 {
        let group_count = re.captures_len(); // includes group 0
        let rows: Vec<(Value, Value)> = if flags & SET_ORDER != 0 {
            // PREG_SET_ORDER: matches[set][group]. The OUTER keys are the set
            // numbers, so only the inner rows carry group names.
            all.into_iter()
                .enumerate()
                .map(|(i, (mut row, last))| {
                    row.truncate(last + 1);
                    (Value::int(i as i64), make_map(key_row(row, &names)))
                })
                .collect()
        } else {
            // PREG_PATTERN_ORDER (default): matches[group][set]. Here it is the
            // OUTER keys that name the groups.
            let mut rows = Vec::with_capacity(group_count);
            for g in 0..group_count {
                let col = make_list(all.iter().map(|(row, _)| row[g].clone()).collect());
                push_keyed(&mut rows, &names, g, col);
            }
            rows
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
/// `count` ACCUMULATES across calls rather than being reset here, because
/// `preg_replace` runs this once per pattern and PHP reports the total over all
/// of them, not the last one's tally.
fn replace_one(re: &Pattern, repl: &[u8], subject: &[u8], limit: i64, count: &mut i64) -> Vec<u8> {
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
        *count += 1;
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
    let mut compiled: Vec<(Rc<Pattern>, String)> = Vec::with_capacity(pats.len());
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
    let mut count: i64 = 0;
    let mut apply = |s: &str| -> String {
        let mut cur: Vec<u8> = s.as_bytes().to_vec();
        for (re, repl) in &compiled {
            cur = replace_one(re, repl.as_bytes(), &cur, limit, &mut count);
        }
        bstr(&cur)
    };

    let out = if with_host(|h| h.is_array(&subj)) {
        let pairs = with_host(|h| h.array_pairs(&subj)).unwrap_or_default();
        make_map(
            pairs
                .into_iter()
                .map(|(k, v)| (k, Value::str(apply(&with_host(|h| h.to_str(&v))))))
                .collect(),
        )
    } else {
        Value::str(apply(&with_host(|h| h.to_str(&subj))))
    };
    // `$count` is written unconditionally when the caller supplied it, including
    // the zero-replacement case — PHP defines it there too.
    if args.len() > 4 {
        with_host(|h| h.byref_out_put(4, Value::int(count)));
    }
    Ok(out)
}

fn preg_replace_callback(args: &[Value]) -> Result<Value, String> {
    let pats = pattern_list(&arg(args, 0));
    let cb = arg(args, 1);
    let limit = args.get(3).map(|v| v.to_int()).unwrap_or(-1);

    let mut compiled: Vec<Rc<Pattern>> = Vec::with_capacity(pats.len());
    for p in &pats {
        let Some(re) = compile_for("preg_replace_callback", p) else {
            return Ok(Value::Undef);
        };
        compiled.push(re);
    }

    let subj = arg(args, 2);
    // `$flags` sits AFTER `$count` in the signature, so it is argument 5.
    let fmt = CellFmt::from_flags(args.get(5).map(|v| v.to_int()).unwrap_or(0));
    let mut count: i64 = 0;
    let run = |s: &str, count: &mut i64| -> Result<String, String> {
        let mut cur = s.to_string();
        for re in &compiled {
            cur = replace_all_cb(re, &cur, &cb, limit, fmt, count)?;
        }
        Ok(cur)
    };

    let out = if with_host(|h| h.is_array(&subj)) {
        let pairs = with_host(|h| h.array_pairs(&subj)).unwrap_or_default();
        let mut out = Vec::with_capacity(pairs.len());
        for (k, v) in pairs {
            let s = with_host(|h| h.to_str(&v));
            out.push((k, Value::str(run(&s, &mut count)?)));
        }
        make_map(out)
    } else {
        let s = with_host(|h| h.to_str(&subj));
        Value::str(run(&s, &mut count)?)
    };
    if args.len() > 4 {
        with_host(|h| h.byref_out_put(4, Value::int(count)));
    }
    Ok(out)
}

/// Replace every (up to `limit`) match of `re` in `s`, calling `cb($matches)`
/// for each and substituting its string return. Propagates a thrown exception.
fn replace_all_cb(
    re: &Pattern,
    s: &str,
    cb: &Value,
    limit: i64,
    fmt: CellFmt,
    count: &mut i64,
) -> Result<String, String> {
    let bytes = s.as_bytes();
    let names = re.group_names();
    let mut out: Vec<u8> = Vec::new();
    let mut last = 0;
    for (n, caps) in re.captures_all(bytes).into_iter().enumerate() {
        if limit >= 0 && n as i64 >= limit {
            break;
        }
        let whole = caps.get(0).unwrap();
        out.extend_from_slice(&bytes[last..whole.start()]);
        *count += 1;
        let matches = make_map(caps_trimmed(&caps, &names, fmt));
        let ret = crate::host::call_value(cb.clone(), vec![matches])?;
        if crate::host::unwinding() {
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
