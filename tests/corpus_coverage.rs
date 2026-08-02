//! Coverage gate: the documentation corpus (`src/corpus.rs`) must stay pinned to
//! the runtime's own registration tables, in BOTH directions.
//!
//! Why this test exists: `docs/reference.html` — and the paid reference manual
//! built from it — is generated entirely from `corpus()`. Nothing else checks
//! that a newly registered builtin ever gets documented, so without this gate a
//! function can ship implemented-but-undocumented indefinitely. It has happened:
//! the corpus once documented 135 names against a 504-function library surface.
//!
//! The registration tables are the source of truth, and there are THREE of them:
//!   1. `call_library` in `src/builtins.rs` — the core match.
//!   2. `dispatch` in each of the 26 `src/stdlib/*.rs` modules.
//!   3. `array_mutator_subop` in `src/compiler.rs` — the by-reference array
//!      mutators, which lower to an `ARR_MUT` op and never reach `call_library`.
//!
//! Each is a Rust `match` whose arms are string literals, so the scanner below
//! reads the sources with `include_str!` and lifts those literals directly. That
//! keeps the gate honest: it cannot drift from the code the way a hand-maintained
//! list of expected names would.
//!
//! Names the corpus documents that are NOT library functions — keywords,
//! operators, constants, prelude classes, synthesized object methods — live in
//! the chapters listed in `NON_LIBRARY_CHAPTERS` and are exempt from the reverse
//! check. Every other chapter is treated as library surface, so adding a new
//! chapter of functions gets checked automatically rather than silently skipped.

use std::collections::BTreeSet;

use phplang::corpus::CORPUS;

/// Chapters whose entries are deliberately not `call_library` names. Every other
/// chapter must consist of real, registered library functions.
const NON_LIBRARY_CHAPTERS: &[&str] = &[
    "Keyword",
    "Language construct",
    "Operator",
    "Cast",
    "Magic method",
    "Predefined constant",
    "Prelude class",
    "Built-in object methods",
];

/// Each registration table as `(label, source, the `fn` line that opens it)`.
/// The label is only used to make a failure message say where a name came from.
fn tables() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        (
            "builtins.rs::call_library",
            include_str!("../src/builtins.rs"),
            "pub fn call_library(",
        ),
        (
            "compiler.rs::array_mutator_subop",
            include_str!("../src/compiler.rs"),
            "fn array_mutator_subop(",
        ),
        (
            "stdlib::arrays",
            include_str!("../src/stdlib/arrays.rs"),
            "pub fn dispatch(",
        ),
        (
            "stdlib::bcmath",
            include_str!("../src/stdlib/bcmath.rs"),
            "pub fn dispatch(",
        ),
        (
            "stdlib::callable",
            include_str!("../src/stdlib/callable.rs"),
            "pub fn dispatch(",
        ),
        (
            "stdlib::constants",
            include_str!("../src/stdlib/constants.rs"),
            "pub fn dispatch(",
        ),
        (
            "stdlib::ctype",
            include_str!("../src/stdlib/ctype.rs"),
            "pub fn dispatch(",
        ),
        (
            "stdlib::datefn",
            include_str!("../src/stdlib/datefn.rs"),
            "pub fn dispatch(",
        ),
        (
            "stdlib::datetime",
            include_str!("../src/stdlib/datetime.rs"),
            "pub fn dispatch(",
        ),
        (
            "stdlib::encoding",
            include_str!("../src/stdlib/encoding.rs"),
            "pub fn dispatch(",
        ),
        (
            "stdlib::fileio",
            include_str!("../src/stdlib/fileio.rs"),
            "pub fn dispatch(",
        ),
        (
            "stdlib::fileres",
            include_str!("../src/stdlib/fileres.rs"),
            "pub fn dispatch(",
        ),
        (
            "stdlib::filter",
            include_str!("../src/stdlib/filter.rs"),
            "pub fn dispatch(",
        ),
        (
            "stdlib::gmp",
            include_str!("../src/stdlib/gmp.rs"),
            "pub fn dispatch(",
        ),
        (
            "stdlib::hash",
            include_str!("../src/stdlib/hash.rs"),
            "pub fn dispatch(",
        ),
        (
            "stdlib::hashext",
            include_str!("../src/stdlib/hashext.rs"),
            "pub fn dispatch(",
        ),
        (
            "stdlib::json",
            include_str!("../src/stdlib/json.rs"),
            "pub fn dispatch(",
        ),
        (
            "stdlib::math",
            include_str!("../src/stdlib/math.rs"),
            "pub fn dispatch(",
        ),
        (
            "stdlib::mbstring",
            include_str!("../src/stdlib/mbstring.rs"),
            "pub fn dispatch(",
        ),
        (
            "stdlib::misc",
            include_str!("../src/stdlib/misc.rs"),
            "pub fn dispatch(",
        ),
        (
            "stdlib::preg",
            include_str!("../src/stdlib/preg.rs"),
            "pub fn dispatch(",
        ),
        (
            "stdlib::reflection",
            include_str!("../src/stdlib/reflection.rs"),
            "pub fn dispatch(",
        ),
        (
            "stdlib::runtime",
            include_str!("../src/stdlib/runtime.rs"),
            "pub fn dispatch(",
        ),
        (
            "stdlib::strings",
            include_str!("../src/stdlib/strings.rs"),
            "pub fn dispatch(",
        ),
        (
            "stdlib::system",
            include_str!("../src/stdlib/system.rs"),
            "pub fn dispatch(",
        ),
        (
            "stdlib::textx",
            include_str!("../src/stdlib/textx.rs"),
            "pub fn dispatch(",
        ),
        (
            "stdlib::types",
            include_str!("../src/stdlib/types.rs"),
            "pub fn dispatch(",
        ),
        (
            "stdlib::url",
            include_str!("../src/stdlib/url.rs"),
            "pub fn dispatch(",
        ),
    ]
}

/// Lift the string literals that open a `match` arm out of one registration
/// function.
///
/// Scanning starts at `opener` and stops at the first column-0 `}`, i.e. the end
/// of that function — so helper functions further down the file, which often
/// match on unrelated string sets (hash algorithm names, `strtotime` keywords),
/// are never mistaken for registrations.
///
/// A registration arm looks like `"a" => …` or `"a" | "b" => …`, optionally with
/// a match guard before the `=>`. Anything else on the line is skipped, which is
/// what keeps arm BODIES (`Value::Str(s) => …`, string literals inside
/// expressions) out of the result.
fn arm_names(src: &str, opener: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut inside = false;
    for line in src.lines() {
        if !inside {
            if line.contains(opener) {
                inside = true;
            }
            continue;
        }
        if line == "}" {
            break;
        }
        if let Some(names) = arm_literals(line.trim_start()) {
            out.extend(names);
        }
    }
    assert!(
        inside,
        "opener {opener:?} not found — did the fn get renamed?"
    );
    out
}

/// Parse `"a" | "b" [if guard] =>` at the head of a trimmed line, returning the
/// literals. `None` when the line does not open a match arm.
fn arm_literals(line: &str) -> Option<Vec<String>> {
    let mut rest = line;
    let mut names = Vec::new();
    loop {
        let body = rest.strip_prefix('"')?;
        let end = body.find('"')?;
        let name = &body[..end];
        // A registration name is a bare PHP identifier. This rejects arms that
        // begin with some other string literal (a message, a path, a format).
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return None;
        }
        names.push(name.to_string());
        rest = body[end + 1..].trim_start();
        match rest.strip_prefix('|') {
            Some(after) => rest = after.trim_start(),
            None => break,
        }
    }
    // Allow a match guard between the last literal and the `=>`.
    if let Some(after_if) = rest.strip_prefix("if ") {
        rest = &after_if[after_if.find("=>")?..];
    }
    if rest.starts_with("=>") {
        Some(names)
    } else {
        None
    }
}

/// Every library function the runtime registers, from all three tables.
fn implemented() -> BTreeSet<String> {
    let mut all = BTreeSet::new();
    for (_label, src, opener) in tables() {
        all.extend(arm_names(src, opener));
    }
    all
}

/// Every name the corpus documents, whatever its chapter.
fn documented() -> BTreeSet<String> {
    CORPUS.iter().map(|(name, ..)| name.to_string()).collect()
}

/// Forward direction: nothing may be implemented without being documented.
/// This is the check that would have caught the 135-of-504 gap.
#[test]
fn every_registered_function_is_documented() {
    let documented = documented();
    let mut missing: Vec<String> = Vec::new();
    for (label, src, opener) in tables() {
        for name in arm_names(src, opener) {
            if !documented.contains(&name) {
                missing.push(format!("{name}  ({label})"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "{} registered function(s) have no entry in src/corpus.rs — a new builtin \
         must be documented in the same change that registers it:\n  {}",
        missing.len(),
        missing.join("\n  ")
    );
}

/// Reverse direction: a library chapter may not document a name the runtime does
/// not register. This catches a typo'd entry and an entry left behind after the
/// function it described was renamed or removed.
#[test]
fn every_documented_function_is_registered() {
    let implemented = implemented();
    let stray: Vec<String> = CORPUS
        .iter()
        .filter(|(_, chapter, ..)| !NON_LIBRARY_CHAPTERS.contains(chapter))
        .filter(|(name, ..)| !implemented.contains(*name))
        .map(|(name, chapter, ..)| format!("{name}  (chapter {chapter})"))
        .collect();
    assert!(
        stray.is_empty(),
        "{} corpus entry/entries name a function the runtime does not register. \
         Either the name is misspelled, or the entry outlived its implementation:\n  {}",
        stray.len(),
        stray.join("\n  ")
    );
}

/// Every `Predefined constant` entry must name a constant the host actually
/// seeds, so the constants chapter cannot document a name a program cannot read.
#[test]
fn every_documented_constant_is_seeded() {
    let missing: Vec<&str> = CORPUS
        .iter()
        .filter(|(_, chapter, ..)| *chapter == "Predefined constant")
        .map(|(name, ..)| *name)
        .filter(|name| !phplang::host::with_host(|h| h.const_defined(name)))
        .collect();
    assert!(
        missing.is_empty(),
        "documented constant(s) are not in predefined_constants() in src/host.rs: {missing:?}"
    );
}

/// Every `Prelude class` entry must name a class the PHP-source prelude declares.
#[test]
fn every_documented_prelude_class_is_declared() {
    let prelude = include_str!("../src/lib.rs");
    let missing: Vec<&str> = CORPUS
        .iter()
        .filter(|(_, chapter, ..)| *chapter == "Prelude class")
        .map(|(name, ..)| *name)
        .filter(|name| {
            !prelude.contains(&format!("class {name} "))
                && !prelude.contains(&format!("class {name} {{"))
        })
        .collect();
    assert!(
        missing.is_empty(),
        "documented prelude class(es) are not declared in the src/lib.rs prelude: {missing:?}"
    );
}

/// Shape gate: the reference renders `signature` as the entry's code block, so an
/// empty one would emit a blank `<pre>`. Names and chapters are equally required.
#[test]
fn every_entry_is_well_formed() {
    for (name, chapter, sig, doc, _example) in CORPUS {
        assert!(!name.is_empty(), "entry with an empty name in {chapter}");
        assert!(!chapter.is_empty(), "entry {name} has no chapter");
        assert!(!sig.is_empty(), "entry {name} has no signature");
        assert!(!doc.is_empty(), "entry {name} has no description");
    }
}

/// No duplicate `(name, chapter)` pair. A name may legitimately repeat across
/// chapters, but the same name twice in one chapter is a copy-paste slip.
#[test]
fn no_duplicate_entries_within_a_chapter() {
    let mut seen: BTreeSet<(&str, &str)> = BTreeSet::new();
    for (name, chapter, ..) in CORPUS {
        assert!(
            seen.insert((name, chapter)),
            "duplicate corpus entry {name:?} in chapter {chapter:?}"
        );
    }
}
