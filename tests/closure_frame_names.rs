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
