//! Byte-parity harness for real Drupal code.
//!
//! Extracts individual procedural functions from a Drupal 7 core checkout,
//! composes each with a fixed driver that calls it and echoes the result, then
//! runs the composed program through BOTH the reference `php` and phplang and
//! asserts their stdout is **byte-for-byte identical**. Any mismatch is printed
//! with both outputs and makes the process exit non-zero.
//!
//! Drupal is GPL-2.0-or-later and is never committed here (see
//! `scripts/fetch-drupal.sh`); this harness reads it from a gitignored checkout.
//! Only phplang's own drivers and this runner are part of the MIT crate.
//!
//! Setup:  ./scripts/fetch-drupal.sh
//! Run:    cargo build --bin php --bin parity-drupal && ./target/debug/parity-drupal

use std::path::{Path, PathBuf};
use std::process::Command;

/// One byte-parity case: the Drupal functions it needs (in dependency order) and
/// a driver that exercises them with fixed inputs.
struct Case {
    label: &'static str,
    deps: &'static [&'static str],
    driver: &'static str,
}

/// The curated corpus — real Drupal 7 procedural functions phplang can run today
/// (no `static`, `preg_*`, `t()`, globals, `mb_*`, references, or objects).
const CASES: &[Case] = &[
    Case {
        label: "check_plain (HTML escaping, ENT_QUOTES)",
        deps: &["check_plain"],
        driver: r#"
foreach (['plain text', '<script>alert("x")</script>', "a & b < c > d 'q'", 'Ünïcödé & co'] as $s) {
    echo check_plain($s), "\n";
}"#,
    },
    Case {
        label: "drupal_placeholder (wraps check_plain)",
        deps: &["check_plain", "drupal_placeholder"],
        driver: r#"
echo drupal_placeholder('user & "input"'), "\n";
echo drupal_placeholder('<b>bold</b>'), "\n";"#,
    },
    Case {
        label: "element_property (# render-array keys)",
        deps: &["element_property"],
        driver: r#"
foreach (['#type', 'title', '', '#weight', '0'] as $k) {
    var_dump(element_property($k));
}"#,
    },
    Case {
        label: "element_child (non-# render-array keys)",
        deps: &["element_child"],
        driver: r#"
foreach (['#type', 'title', '0', ''] as $k) {
    var_dump(element_child($k));
}"#,
    },
    Case {
        label: "drupal_sort_weight (weight comparator)",
        deps: &["drupal_sort_weight"],
        driver: r#"
echo drupal_sort_weight(['weight' => 5], ['weight' => 2]), "|";
echo drupal_sort_weight(['weight' => 1], ['weight' => 9]), "|";
echo drupal_sort_weight(['weight' => 3], ['weight' => 3]), "|";
echo drupal_sort_weight('nope', ['weight' => -4]), "\n";"#,
    },
    Case {
        label: "drupal_map_assoc (array_combine helper)",
        deps: &["drupal_map_assoc"],
        driver: r#"
echo implode(",", array_keys(drupal_map_assoc(['red', 'green', 'blue']))), "\n";
echo implode(",", drupal_map_assoc(['a', 'b', 'c'])), "\n";
echo count(drupal_map_assoc([])), "\n";
echo implode(",", drupal_map_assoc([1, 2, 3], 'strval')), "\n";"#,
    },
];

fn main() {
    let src = drupal_src();
    let bootstrap = read_inc(&src, "bootstrap.inc");
    let common = read_inc(&src, "common.inc");
    let corpus = format!("{bootstrap}\n{common}");

    let ours = ours_bin();
    if !ours.exists() {
        eprintln!(
            "parity-drupal: phplang binary not found at {} (run `cargo build`)",
            ours.display()
        );
        std::process::exit(2);
    }
    let php = oracle_path();
    eprintln!("parity-drupal: reference = {}", oracle_id());
    eprintln!("parity-drupal: source    = {}", src.display());
    eprintln!("parity-drupal: functions verified byte-for-byte:\n");

    let mut pass = 0;
    let mut fail = 0;
    let mut skip = 0;
    for case in CASES {
        // Compose: the extracted Drupal function sources + the driver.
        //
        // A dependency the checkout does not carry skips the whole CASE. It used
        // to `continue` the DEPENDENCY loop instead, which left the driver
        // composed against a program missing the very function it calls — a
        // case that then "ran" and compared two undefined-function failures.
        let mut prog = String::new();
        let mut missing = false;
        for dep in case.deps {
            match extract_fn(&corpus, dep) {
                Some(body) => {
                    prog.push_str(&body);
                    prog.push('\n');
                }
                None => {
                    println!(
                        "  SKIP  {} — function `{dep}` not found in checkout",
                        case.label
                    );
                    missing = true;
                    break;
                }
            }
        }
        if missing {
            skip += 1;
            continue;
        }
        prog.push_str(case.driver);

        // BOTH sides get the same flags. They used to differ — the reference was
        // muted with `-d error_reporting=0` and phplang given nothing — so the
        // one engine was answering a different question from the other, and a
        // divergence that consisted of a diagnostic could only ever be read as a
        // failure of the muted side. phplang honours `-d error_reporting` too.
        const FLAGS: &[&str] = &["-d", "error_reporting=0"];
        let a = run(&php, FLAGS, &prog);
        let b = run(&ours.to_string_lossy(), FLAGS, &prog);
        if a == b {
            println!("  \u{2713} PASS  {} ({} bytes)", case.label, a.0.len());
            pass += 1;
        } else {
            println!("  \u{2717} FAIL  {}", case.label);
            report_side("php ", &a);
            report_side("ours", &b);
            fail += 1;
        }
    }

    println!("\nparity-drupal: {pass} byte-identical, {fail} divergent, {skip} skipped");
    // A skip is not a pass. A run that skipped everything has verified nothing,
    // and exiting 0 on it is how a harness reports clean while measuring nothing.
    if fail > 0 || skip > 0 || pass == 0 {
        std::process::exit(1);
    }
}

/// Print one side of a failed comparison — all three observables, since any of
/// the three can be the thing that differs.
fn report_side(who: &str, r: &Run) {
    println!(
        "        {who} (exit {}): out={:?} err={:?}",
        r.1,
        String::from_utf8_lossy(&r.0),
        String::from_utf8_lossy(&r.2)
    );
}

/// One captured run: stdout, exit code, stderr. All three are compared.
type Run = (Vec<u8>, i32, Vec<u8>);

/// Run raw PHP (no `<?php` tag) through an interpreter via `-r`. phplang's `-r`
/// prepends the open tag; reference PHP `-r` runs raw code — so both receive the
/// same source.
///
/// stderr is captured rather than dropped: PHP writes every diagnostic twice,
/// the `display_errors` copy on stdout and the `log_errors` copy here, so a
/// comparison that reads only stdout sees half of what the engine emitted.
fn run(bin: &str, extra: &[&str], code: &str) -> Run {
    let out = Command::new(bin).args(extra).args(["-r", code]).output();
    match out {
        Ok(o) => (o.stdout, o.status.code().unwrap_or(-1), o.stderr),
        Err(_) => (Vec::new(), -1, Vec::new()),
    }
}

/// Extract a top-level `function NAME(...) { ... }` body from Drupal source. Relies
/// on Drupal's coding standard (the closing brace of a top-level function is a
/// `}` in column 0).
fn extract_fn(src: &str, name: &str) -> Option<String> {
    let head = format!("function {name}(");
    let start = src.lines().position(|l| l.starts_with(&head))?;
    let mut body = String::new();
    for line in src.lines().skip(start) {
        body.push_str(line);
        body.push('\n');
        if line == "}" {
            return Some(body);
        }
    }
    None
}

fn read_inc(src: &Path, name: &str) -> String {
    let p = src.join(name);
    std::fs::read_to_string(&p).unwrap_or_else(|_| {
        eprintln!(
            "parity-drupal: cannot read {} — run ./scripts/fetch-drupal.sh first",
            p.display()
        );
        std::process::exit(2);
    })
}

/// The Drupal checkout's `includes/` dir: `$PHPLANG_DRUPAL_SRC/includes`, else
/// `third_party/drupal/includes`, else `/tmp` (where an ad-hoc fetch may land).
fn drupal_src() -> PathBuf {
    if let Ok(p) = std::env::var("PHPLANG_DRUPAL_SRC") {
        return PathBuf::from(p).join("includes");
    }
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("third_party/drupal/includes");
    if repo.join("bootstrap.inc").exists() {
        return repo;
    }
    PathBuf::from("/tmp")
}

fn ours_bin() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let cand = dir.join("php");
            if cand.exists() {
                return cand;
            }
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/php")
}

fn oracle_path() -> String {
    if let Ok(p) = std::env::var("PHPLANG_FUZZ_PHP") {
        return p;
    }
    for p in [
        "/opt/homebrew/bin/php",
        "/usr/local/bin/php",
        "/usr/bin/php",
    ] {
        if Path::new(p).exists() {
            return p.to_string();
        }
    }
    "php".to_string()
}

fn oracle_id() -> String {
    let path = oracle_path();
    let ver = Command::new(&path)
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.lines().next().map(|l| l.trim().to_string()))
        .unwrap_or_else(|| "unknown".into());
    format!("{path} ({ver})")
}
