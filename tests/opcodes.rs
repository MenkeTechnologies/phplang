//! The registry-key invariants: one key, one handler.
//!
//! phplang dispatches through several registries, some keyed by an integer
//! opcode and some keyed by a name. All of them share one failure shape: a
//! duplicate key does not fail to build, does not warn, and does not panic —
//! one handler silently shadows the other and the shadowed construct starts
//! doing something else entirely. Both families are pinned here.
//!
//! Every `host::ops` constant is a key into fusevm's builtin table, and
//! `register_builtin` takes the LAST registration for a number. Two constants
//! sharing a number therefore does not fail to build, does not warn, and does
//! not panic — one handler silently shadows the other and the shadowed
//! construct starts doing something else entirely. That is the worst possible
//! failure shape, so it is pinned here.
//!
//! This has bitten for real: `INDEX_ISSET` was added as 105 while `LIST_ELEM_GET`
//! already held 105, and `isset($a['k'])` quietly began dispatching to the
//! destructuring read — warning "Undefined array key" and evaluating to null
//! instead of false.
//!
//! The name-keyed registries are worse, because the shadowed handler is not
//! even reported at the shadowing site: `stdlib::dispatch` tries each category
//! in a fixed order and the FIRST that recognizes a name wins, so a function
//! added to a second category is simply never reached, and the two bodies drift
//! apart with nothing pointing at the fork. `predefined_constants`,
//! `default_ini` and the PHP preludes are the mirror image — last write wins.
//!
//! Read out of the source rather than from a hand-kept list, because a
//! hand-kept list is exactly the thing that goes stale and stops catching this.

/// `(name, number)` for every `pub const NAME: u16 = N;` in the `ops` module.
fn opcode_constants() -> Vec<(String, u16)> {
    let src = include_str!("../src/host.rs");
    let body = src
        .split_once("pub mod ops {")
        .expect("host.rs declares `pub mod ops`")
        .1;
    let mut out = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        // Stop at the end of the module — the first line that closes it at
        // column 0 in the original source.
        let Some(rest) = line.strip_prefix("pub const ") else {
            continue;
        };
        let Some((name, tail)) = rest.split_once(": u16 = ") else {
            continue;
        };
        let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(n) = digits.parse::<u16>() {
            out.push((name.to_string(), n));
        }
    }
    out
}

#[test]
fn every_opcode_number_is_used_by_exactly_one_constant() {
    let ops = opcode_constants();
    // A parse that finds nothing would pass vacuously, which would be worse than
    // no test at all.
    assert!(
        ops.len() > 50,
        "expected to read the whole ops module, found {} constants",
        ops.len()
    );
    let mut by_number: Vec<(u16, Vec<String>)> = Vec::new();
    for (name, n) in ops {
        match by_number.iter_mut().find(|(num, _)| *num == n) {
            Some((_, names)) => names.push(name),
            None => by_number.push((n, vec![name])),
        }
    }
    let clashes: Vec<String> = by_number
        .iter()
        .filter(|(_, names)| names.len() > 1)
        .map(|(n, names)| format!("{n} => {}", names.join(", ")))
        .collect();
    assert!(
        clashes.is_empty(),
        "opcode numbers used by more than one constant (the later registration \
         silently shadows the earlier): {}",
        clashes.join("; ")
    );
}

// ── name-keyed registries ───────────────────────────────────────────────────

/// Just enough of a Rust lexer to walk a `match` without being fooled by a
/// brace, a `,` or a `=>` that lives inside a string, a char literal or a
/// comment. Anything not needed for that is collapsed to [`Tok::Other`].
#[derive(Debug, PartialEq)]
enum Tok {
    Str(String),
    Ident(String),
    Sym(char),
    Arrow,
    Other,
}

fn lex(src: &str) -> Vec<Tok> {
    let b: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            _ if c.is_whitespace() => i += 1,
            '/' if b.get(i + 1) == Some(&'/') => {
                while i < b.len() && b[i] != '\n' {
                    i += 1;
                }
            }
            '/' if b.get(i + 1) == Some(&'*') => {
                i += 2;
                while i < b.len() && !(b[i] == '*' && b.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i = (i + 2).min(b.len());
            }
            // Raw string: r, then any number of #, then the quote.
            'r' if matches!(b.get(i + 1), Some('"') | Some('#')) => {
                let mut j = i + 1;
                let mut hashes = 0;
                while b.get(j) == Some(&'#') {
                    hashes += 1;
                    j += 1;
                }
                if b.get(j) != Some(&'"') {
                    // Not a raw string after all — an identifier starting `r`.
                    out.push(read_ident(&b, &mut i));
                    continue;
                }
                j += 1;
                let start = j;
                let close: String = std::iter::once('"')
                    .chain(std::iter::repeat_n('#', hashes))
                    .collect();
                let rest: String = b[start..].iter().collect();
                let end = rest.find(&close).map(|k| start + rest[..k].chars().count());
                let end = end.unwrap_or(b.len());
                out.push(Tok::Str(b[start..end].iter().collect()));
                i = (end + 1 + hashes).min(b.len());
            }
            '"' => {
                i += 1;
                let mut s = String::new();
                while i < b.len() && b[i] != '"' {
                    if b[i] == '\\' && i + 1 < b.len() {
                        // Keep the escape verbatim; no registry key needs the
                        // decoded form, and decoding would only add ways to be
                        // wrong.
                        s.push(b[i]);
                        i += 1;
                    }
                    s.push(b[i]);
                    i += 1;
                }
                i += 1;
                out.push(Tok::Str(s));
            }
            // A char literal (`'x'`, `'\n'`) or a lifetime (`'a`). Only the
            // former can hide a brace, so scan for a close quote nearby.
            '\'' => {
                let mut j = i + 1;
                if b.get(j) == Some(&'\\') {
                    j += 1;
                }
                if b.get(j + 1) == Some(&'\'') {
                    i = j + 2;
                    out.push(Tok::Other);
                } else {
                    i += 1;
                }
            }
            '=' if b.get(i + 1) == Some(&'>') => {
                out.push(Tok::Arrow);
                i += 2;
            }
            '{' | '}' | '(' | ')' | '[' | ']' | ',' | '|' => {
                out.push(Tok::Sym(c));
                i += 1;
            }
            _ if c.is_alphanumeric() || c == '_' => out.push(read_ident(&b, &mut i)),
            _ => {
                out.push(Tok::Other);
                i += 1;
            }
        }
    }
    out
}

fn read_ident(b: &[char], i: &mut usize) -> Tok {
    let start = *i;
    while *i < b.len() && (b[*i].is_alphanumeric() || b[*i] == '_') {
        *i += 1;
    }
    Tok::Ident(b[start..*i].iter().collect())
}

/// Every string literal used as a top-level arm pattern of the `match` on the
/// dispatch subject, i.e. the exact set of names that registry claims.
///
/// `subject` is the matched-on identifier (`name` / `lname`); scanning starts
/// at `anchor` so a helper `match` earlier in the file cannot be mistaken for
/// the dispatcher.
fn match_arm_names(src: &str, anchor: &str, subject: &str) -> Vec<String> {
    let at = src
        .find(anchor)
        .unwrap_or_else(|| panic!("anchor not found: {anchor}"));
    let toks = lex(&src[at..]);

    // Find `match <subject>` … `{`.
    let mut k = 0;
    let open = loop {
        assert!(k + 2 < toks.len(), "no `match {subject}` after `{anchor}`");
        if toks[k] == Tok::Ident("match".into()) && toks[k + 1] == Tok::Ident(subject.into()) {
            let mut j = k + 2;
            while j < toks.len() && toks[j] != Tok::Sym('{') {
                // Only `.as_str()` may sit between the subject and the brace.
                if matches!(&toks[j], Tok::Sym('{') | Tok::Arrow) {
                    break;
                }
                j += 1;
            }
            if toks.get(j) == Some(&Tok::Sym('{')) {
                break j;
            }
        }
        k += 1;
    };

    let (mut braces, mut parens) = (1i32, 0i32);
    let (mut in_pattern, mut guarded) = (true, false);
    let (mut pending, mut names) = (Vec::new(), Vec::new());
    for t in &toks[open + 1..] {
        // Nested groups: track depth everywhere, but only act on arm structure
        // at the match's own level.
        match t {
            Tok::Sym('(') | Tok::Sym('[') => parens += 1,
            Tok::Sym(')') | Tok::Sym(']') => parens -= 1,
            Tok::Sym('{') => braces += 1,
            Tok::Sym('}') => braces -= 1,
            _ => {}
        }
        if braces == 0 {
            break;
        }
        if braces != 1 || parens != 0 {
            continue;
        }
        match t {
            // An arm body that is a block ends the arm; the comma is optional.
            Tok::Sym('}') | Tok::Sym(',') => {
                in_pattern = true;
                guarded = false;
                pending.clear();
            }
            Tok::Arrow => {
                if in_pattern && !guarded {
                    names.append(&mut pending);
                }
                in_pattern = false;
                guarded = false;
                pending.clear();
            }
            // A guard means the arm may decline the name, so it cannot be
            // claimed outright — drop it rather than report a false clash.
            Tok::Ident(w) if in_pattern && w == "if" => guarded = true,
            Tok::Str(s) if in_pattern && !guarded => pending.push(s.clone()),
            _ => {}
        }
    }
    names
}

fn read(rel: &str) -> String {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The stdlib categories, read from the `pub mod` list rather than hand-kept.
fn stdlib_modules(mod_rs: &str) -> Vec<String> {
    mod_rs
        .lines()
        .filter_map(|l| l.trim().strip_prefix("pub mod ")?.strip_suffix(';'))
        .map(str::to_string)
        .collect()
}

#[test]
fn every_library_function_name_has_exactly_one_handler() {
    let mod_rs = read("src/stdlib/mod.rs");
    let modules = stdlib_modules(&mod_rs);
    assert!(
        modules.len() > 20,
        "expected the whole category list, found {modules:?}"
    );

    // A category that is declared but never chained into `stdlib::dispatch` is
    // dead in exactly the same silent way a shadowed name is.
    let chain = mod_rs
        .split_once("pub fn dispatch(")
        .expect("stdlib::dispatch")
        .1;
    let unchained: Vec<&String> = modules
        .iter()
        .filter(|m| !chain.contains(&format!("{m}::dispatch(")))
        .collect();
    assert!(
        unchained.is_empty(),
        "stdlib categories never consulted by stdlib::dispatch: {unchained:?}"
    );

    let mut owner: Vec<(String, String)> = Vec::new();
    for m in &modules {
        let src = read(&format!("src/stdlib/{m}.rs"));
        let names = match_arm_names(&src, "pub fn dispatch(name: &str", "name");
        assert!(
            !names.is_empty(),
            "read no function names out of stdlib::{m}::dispatch — the parse \
             broke, and a guard that reads nothing passes vacuously"
        );
        owner.extend(names.into_iter().map(|n| (n, format!("stdlib::{m}"))));
    }
    // The core match runs before the category chain, so a name it handles makes
    // the category copy unreachable too.
    let core = read("src/builtins.rs");
    let names = match_arm_names(&core, "pub fn call_library(name: &str", "lname");
    assert!(!names.is_empty(), "read no names out of call_library");
    owner.extend(
        names
            .into_iter()
            .map(|n| (n, "builtins::call_library".into())),
    );

    assert!(
        owner.len() > 400,
        "expected the whole library surface, found {} names",
        owner.len()
    );
    assert_clash_free(
        &owner,
        "PHP function names claimed by more than one dispatcher (only the first \
         in the `stdlib::dispatch` chain is ever reached)",
    );
}

#[test]
fn every_predefined_name_is_defined_once() {
    let host = read("src/host.rs");

    // `predefined_constants` inserts through the `si`/`ss`/`sf` closures.
    let body = section(&host, "fn predefined_constants()");
    let mut owner: Vec<(String, String)> = Vec::new();
    for line in body.lines() {
        let l = line.trim();
        for f in ["si(", "ss(", "sf(", "m.insert("] {
            if let Some(rest) = l.strip_prefix(f) {
                if let Some(k) = first_literal(rest) {
                    owner.push((k, "predefined_constants".into()));
                }
            }
        }
    }
    assert!(
        owner.len() > 150,
        "expected the whole constant table, found {}",
        owner.len()
    );
    assert_clash_free(&owner, "constants defined twice by predefined_constants");

    // `default_ini` is a `[(key, value), …]` list collected into a map.
    let body = section(&host, "fn default_ini()");
    let ini: Vec<(String, String)> = body
        .lines()
        .filter_map(|l| l.trim().strip_prefix("(\""))
        .filter_map(|r| Some((r.split_once('"')?.0.to_string(), "default_ini".into())))
        .collect();
    assert!(
        ini.len() > 30,
        "expected the whole ini table, found {}",
        ini.len()
    );
    assert_clash_free(&ini, "ini settings listed twice by default_ini");
}

#[test]
fn every_prelude_class_and_member_is_declared_once() {
    let lib = read("src/lib.rs");
    let mut php = String::new();
    let mut rest = lib.as_str();
    while let Some((_, tail)) = rest.split_once("_PRELUDE: &str = r#\"") {
        let (body, tail) = tail.split_once("\"#;").expect("unterminated prelude");
        php.push_str(body);
        php.push('\n');
        rest = tail;
    }
    assert!(
        php.len() > 2000,
        "expected the bundled PHP preludes, read {} bytes",
        php.len()
    );

    // `load_classes` inserts by name and `ClassDef::methods` is a map, so a
    // repeated class or member replaces the earlier one with no diagnostic —
    // where real PHP would refuse to compile the file.
    let mut classes: Vec<(String, String)> = Vec::new();
    let mut members: Vec<(String, String)> = Vec::new();
    let (mut current, mut depth) = (String::new(), 0i32);
    for line in php.lines() {
        let code = line.split("//").next().unwrap_or("").trim();
        if let Some(name) = php_type_name(code) {
            classes.push((name.to_ascii_lowercase(), "prelude".into()));
            current = name;
            depth = 0;
        }
        if depth == 1 && !current.is_empty() {
            if let Some(m) = php_member(code) {
                members.push((
                    format!("{current}::{}", m.to_ascii_lowercase()),
                    "prelude".into(),
                ));
            }
        }
        depth += count(code, '{') - count(code, '}');
        if depth <= 0 {
            current.clear();
        }
    }
    assert!(
        classes.len() > 20 && members.len() > 80,
        "expected the whole prelude, found {} classes / {} members",
        classes.len(),
        members.len()
    );
    assert_clash_free(&classes, "prelude types declared twice (the later wins)");
    assert_clash_free(&members, "prelude members declared twice (the later wins)");
}

#[test]
fn every_byref_builtin_is_listed_once() {
    let src = read("src/compiler.rs");
    let body = src
        .split_once("const BYREF_BUILTINS:")
        .expect("compiler declares BYREF_BUILTINS")
        .1;
    let body = body
        .split_once("];")
        .expect("unterminated BYREF_BUILTINS")
        .0;
    let names: Vec<(String, String)> = body
        .lines()
        .filter_map(|l| l.trim().strip_prefix("(\""))
        .filter_map(|r| Some((r.split_once('"')?.0.to_string(), "BYREF_BUILTINS".into())))
        .collect();
    assert!(
        names.len() > 5,
        "expected the by-ref table, found {}",
        names.len()
    );
    // Looked up with a linear scan, so a second entry for a name is ignored and
    // its out-parameter positions never take effect.
    assert_clash_free(
        &names,
        "by-ref builtins listed twice (only the first is used)",
    );
}

/// The body of `fn <sig> … {` up to the closing brace at column 0.
fn section<'a>(src: &'a str, sig: &str) -> &'a str {
    let at = src.find(sig).unwrap_or_else(|| panic!("not found: {sig}"));
    let rest = &src[at..];
    let end = rest.find("\n}").map(|k| at + k).unwrap_or(src.len());
    &src[at..end]
}

fn first_literal(s: &str) -> Option<String> {
    let r = s.trim_start().strip_prefix('"')?;
    Some(r.split_once('"')?.0.to_string())
}

fn count(s: &str, c: char) -> i32 {
    s.chars().filter(|x| *x == c).count() as i32
}

/// `class Foo`, `interface Foo`, `trait Foo`, `enum Foo` — with any leading
/// `final`/`abstract`/`readonly`.
fn php_type_name(code: &str) -> Option<String> {
    let mut words = code.split_whitespace();
    let mut w = words.next()?;
    while matches!(w, "final" | "abstract" | "readonly") {
        w = words.next()?;
    }
    if !matches!(w, "class" | "interface" | "trait" | "enum") {
        return None;
    }
    let n: String = words
        .next()?
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!n.is_empty()).then_some(n)
}

/// A method, property or class constant declared directly in a class body.
fn php_member(code: &str) -> Option<String> {
    if let Some(after) = code.split_once("function ") {
        let n: String = after
            .1
            .trim_start_matches('&')
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        return (!n.is_empty()).then(|| format!("fn:{n}"));
    }
    if let Some(after) = code.split_once("const ") {
        let n: String = after
            .1
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        return (!n.is_empty()).then(|| format!("const:{n}"));
    }
    // A property declaration: modifiers, then `$name`. `$this->x` inside a
    // method body cannot reach here because members are only read at depth 1.
    let at = code.find('$')?;
    if !code[..at].split_whitespace().all(|w| {
        matches!(
            w,
            "public" | "private" | "protected" | "static" | "readonly" | "var"
        )
    }) {
        return None;
    }
    let n: String = code[at + 1..]
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!n.is_empty()).then(|| format!("prop:{n}"))
}

/// Fail with every key that more than one site claims, and who claims it.
fn assert_clash_free(entries: &[(String, String)], what: &str) {
    let mut by_key: Vec<(&str, Vec<&str>)> = Vec::new();
    for (k, site) in entries {
        match by_key.iter_mut().find(|(key, _)| *key == k) {
            Some((_, sites)) => sites.push(site),
            None => by_key.push((k, vec![site])),
        }
    }
    let clashes: Vec<String> = by_key
        .iter()
        .filter(|(_, s)| s.len() > 1)
        .map(|(k, s)| format!("{k} => {}", s.join(", ")))
        .collect();
    assert!(clashes.is_empty(), "{what}: {}", clashes.join("; "));
}
