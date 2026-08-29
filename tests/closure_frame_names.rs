//! What a stack frame calls a CLOSURE.
//!
//! PHP 8.4 stopped naming every closure `{closure}` and started naming the place
//! it was written: `{closure:<where>:<line>}`, where `<where>` is the enclosing
//! declaration — `K::m()` for a method, `outer()` for a function, the script's
//! own path at the top level — and nests, so a closure inside a closure reads
//! `{closure:{closure:f.php:2}:3}`. The line is the `function`/`fn` KEYWORD's,
//! not the enclosing statement's, and `Closure::bind` keeps the literal's site
//! rather than taking the binder's.
//!
//! Nothing here is written from memory: each program is run by the reference
//! `php` and by this build, and the `#N` frame lines are compared. A machine
//! with no reference `php` skips, which is the same rule `tests/parity.rs` uses.

use std::path::PathBuf;
use std::process::Command;

/// The reference interpreter, refusing a `php` that is phplang itself — that
/// would compare the binary under test against itself and pass unconditionally.
fn reference_php() -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = match std::env::var_os("PHP") {
        Some(p) => vec![PathBuf::from(p)],
        None => std::env::var_os("PATH")
            .map(|path| {
                std::env::split_paths(&path)
                    .map(|d| d.join("php"))
                    .collect()
            })
            .unwrap_or_default(),
    };
    for c in candidates {
        let Ok(out) = Command::new(&c).arg("--version").output() else {
            continue;
        };
        let banner = String::from_utf8_lossy(&out.stdout);
        if banner.starts_with("PHP ") && !banner.contains("phplang") {
            return Some(c);
        }
    }
    None
}

/// `(major, minor)` of `php`, from the first line of `php --version`
/// (`PHP 8.5.9 (cli) …`). `None` when it cannot be parsed.
fn php_major_minor(php: &PathBuf) -> Option<(u32, u32)> {
    let out = Command::new(php).arg("--version").output().ok()?;
    let banner = String::from_utf8_lossy(&out.stdout);
    let ver = banner.strip_prefix("PHP ")?.split_whitespace().next()?;
    let mut it = ver.split('.');
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

/// The `{closure:file:line}` frame name these tests compare is PHP 8.4's; 8.3
/// and earlier print a bare `{closure}` with no site in it. Against such a
/// reference there is nothing to compare — the assertion that the reference
/// printed a `{closure:` frame is what says so — so the case skips the way a
/// missing `php` does.
fn reference_names_closure_sites(php: &PathBuf) -> bool {
    match php_major_minor(php) {
        Some(v) => v >= (8, 4),
        // Unparsable banner: let the run proceed and fail loudly rather than
        // skipping on a guess.
        None => true,
    }
}

/// Just the `#N …` trace lines a run printed, which is what these programs are
/// written to produce and the only part being compared.
fn frames(bin: &PathBuf, path: &std::path::Path) -> String {
    let out = Command::new(bin)
        .arg(path)
        .env("TZ", "UTC")
        .env("LC_ALL", "C")
        .output()
        .expect("run interpreter");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.lines()
        .filter(|l| l.starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every program prints `getTraceAsString()` for a throw raised inside a
/// closure, and the two engines must agree line for line.
#[test]
fn a_closure_frame_names_where_the_literal_was_written() {
    let Some(php) = reference_php() else {
        eprintln!("skipping: no reference php on PATH");
        return;
    };
    if !reference_names_closure_sites(&php) {
        eprintln!(
            "skipping: reference php predates 8.4, which is where `{{closure:file:line}}` \
             frame names arrive — this case has nothing to compare against"
        );
        return;
    }
    let ours = PathBuf::from(env!("CARGO_BIN_EXE_php"));
    let dir = std::env::temp_dir().join(format!("phplang-closure-frames-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");

    // (name, program). Each catches its own throw and prints the trace, so a
    // program contributes its frames without ending the run.
    let cases: &[(&str, &str)] = &[
        // Top level: the enclosing name is the script's own path.
        (
            "top level",
            "<?php\n$f = function () { throw new Exception('a'); };\n\
             try { $f(); } catch (Exception $e) { echo $e->getTraceAsString(), \"\\n\"; }\n",
        ),
        // Inside a function and inside a method, instance and static. The
        // enclosing method is spelled with `::` even where the FRAME above it
        // uses `->` for the instance call.
        (
            "function and method",
            "<?php\nfunction outer() { $i = function () { throw new Exception('i'); }; $i(); }\n\
             try { outer(); } catch (Exception $e) { echo $e->getTraceAsString(), \"\\n\"; }\n\
             class K {\n\
             public function m() { $f = function () { throw new Exception('z'); }; $f(); }\n\
             public static function s() { $g = static fn() => throw new Exception('y'); $g(); }\n\
             }\n\
             try { (new K)->m(); } catch (Exception $e) { echo $e->getTraceAsString(), \"\\n\"; }\n\
             try { K::s(); } catch (Exception $e) { echo $e->getTraceAsString(), \"\\n\"; }\n",
        ),
        // A closure inside a closure nests its enclosing name whole.
        (
            "nested",
            "<?php\n$a = function () {\n\
             $b = function () { throw new Exception('n'); };\n\
             $b();\n\
             };\n\
             try { $a(); } catch (Exception $e) { echo $e->getTraceAsString(), \"\\n\"; }\n",
        ),
        // The line is the `function`/`fn` keyword's, not the statement's: both
        // literals below start on a line after the assignment they belong to.
        (
            "keyword line",
            "<?php\n$f =\n    function () { throw new Exception('a'); };\n\
             try { $f(); } catch (Exception $e) { echo $e->getTraceAsString(), \"\\n\"; }\n\
             $h = function (\n    $x\n) { throw new Exception('b'); };\n\
             try { $h(1); } catch (Exception $e) { echo $e->getTraceAsString(), \"\\n\"; }\n",
        ),
        // Rebinding does not move the site: it is the literal's, not the call's.
        (
            "rebound",
            "<?php\n$d = function () { throw new Exception('d'); };\n\
             $e2 = Closure::bind($d, null, null);\n\
             try { $e2(); } catch (Exception $e) { echo $e->getTraceAsString(), \"\\n\"; }\n",
        ),
        // An arrow function is a closure literal like any other.
        (
            "arrow fn",
            "<?php\n$g = fn($x) => intdiv($x, 0);\n\
             try { $g(1); } catch (\\Throwable $e) { echo $e->getTraceAsString(), \"\\n\"; }\n",
        ),
    ];

    let mut failures = Vec::new();
    for (name, program) in cases {
        // The path is part of the expected output at the top level, so both
        // engines must be handed the same one.
        let path = dir.join(format!("{}.php", name.replace(' ', "_")));
        std::fs::write(&path, program).expect("write case");
        let want = frames(&php, &path);
        let got = frames(&ours, &path);
        assert!(
            want.contains("{closure:"),
            "{name}: the reference printed no closure frame, so this case tests nothing:\n{want}"
        );
        if want != got {
            failures.push(format!("{name}:\n  php:\n{want}\n  phplang:\n{got}"));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// A frame entered from inside a LIBRARY function.
///
/// PHP gives every internal function that runs a callback a frame of its own,
/// and the callback's frame reports `[internal function]` as its call site
/// because there is no PHP line to name. `call_user_func` and
/// `call_user_func_array` are the measured exception: the reference invokes
/// their callee from the caller's own frame, so neither shows up.
///
/// Each program prints `getTraceAsString()` for a throw raised inside the
/// callback, and the two engines must agree line for line.
#[test]
fn a_library_callback_frame_reports_an_internal_call_site() {
    let Some(php) = reference_php() else {
        eprintln!("skipping: no reference php on PATH");
        return;
    };
    if !reference_names_closure_sites(&php) {
        eprintln!(
            "skipping: reference php predates 8.4, which is where `{{closure:file:line}}` \
             frame names arrive — this case has nothing to compare against"
        );
        return;
    }
    let ours = PathBuf::from(env!("CARGO_BIN_EXE_php"));
    let dir = std::env::temp_dir().join(format!("phplang-internal-frames-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");

    // A `catch` around each call, so one program contributes several traces.
    const PRELUDE: &str = "<?php\n\
        function boom($a = null, $b = null) { throw new Exception('x'); }\n\
        function t(callable $c) { try { $c(); } catch (Throwable $e) { echo $e->getTraceAsString(), \"\\n\"; } }\n";

    // (name, body, whether the reference renders an `[internal function]` site).
    let cases: &[(&str, &str, bool)] = &[
        (
            "map filter reduce",
            "t(fn() => array_map(fn($x) => boom($x), [1]));\n\
             t(fn() => array_map('boom', [1]));\n\
             t(fn() => array_map(fn($x, $y) => boom($x, $y), [1], [2]));\n\
             t(fn() => array_filter([1], fn($x) => boom($x)));\n\
             t(fn() => array_reduce([1], fn($c, $x) => boom($c, $x)));\n",
            true,
        ),
        (
            "user comparator sorts",
            "t(function () { $a = [2, 1]; usort($a, fn($x, $y) => boom($x, $y)); });\n\
             t(function () { $a = [2, 1]; uasort($a, fn($x, $y) => boom($x, $y)); });\n\
             t(function () { $a = [2, 1]; uksort($a, fn($x, $y) => boom($x, $y)); });\n\
             t(function () { $a = [1]; array_udiff($a, [2], fn($x, $y) => boom()); });\n",
            true,
        ),
        (
            "walks and predicates",
            "t(function () { $a = [1]; array_walk($a, fn($v, $k) => boom($v, $k)); });\n\
             t(function () { $a = [[1]]; array_walk_recursive($a, fn($v, $k) => boom($v, $k)); });\n\
             t(fn() => array_find([1], fn($v, $k) => boom($v, $k)));\n\
             t(fn() => array_find_key([1], fn($v, $k) => boom($v, $k)));\n\
             t(fn() => array_any([1], fn($v, $k) => boom($v, $k)));\n\
             t(fn() => array_all([1], fn($v, $k) => boom($v, $k)));\n",
            true,
        ),
        (
            "preg and iterator",
            "t(fn() => preg_replace_callback('/a/', fn($m) => boom(), 'a'));\n\
             t(fn() => iterator_apply(new ArrayIterator([1]), fn() => boom()));\n",
            true,
        ),
        // The rule is local: only the frame DIRECTLY above an internal one takes
        // `[internal function]`. A user function called from the callback, and
        // an inner `array_map` called from it, both report a real line.
        (
            "one level only",
            "t(fn() => array_map(function ($x) { boom($x); }, [1]));\n\
             t(fn() => array_map(function ($x) { array_map(fn($y) => boom($y), [9]); }, [1]));\n",
            true,
        ),
        // The trampolines: no frame of their own, and the callee reports the
        // caller's line rather than `[internal function]`.
        (
            "call_user_func is not a frame",
            "t(fn() => call_user_func(fn($x) => boom($x), 1));\n\
             t(fn() => call_user_func_array(fn($x) => boom($x), [1]));\n",
            false,
        ),
    ];

    let mut failures = Vec::new();
    for (name, body, wants_internal) in cases {
        let path = dir.join(format!("{}.php", name.replace(' ', "_")));
        std::fs::write(&path, format!("{PRELUDE}{body}")).expect("write case");
        let want = frames(&php, &path);
        let got = frames(&ours, &path);
        assert_eq!(
            want.contains("[internal function]"),
            *wants_internal,
            "{name}: the reference did not render the call site this case is about:\n{want}"
        );
        if want != got {
            failures.push(format!("{name}:\n  php:\n{want}\n  phplang:\n{got}"));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
