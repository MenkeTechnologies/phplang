//! Byte-parity harness for real PHP standard-library code.
//!
//! The sibling `parity-drupal` harness runs procedural Drupal functions; this one
//! runs the pure-PHP reimplementations of PHP built-ins from the Symfony
//! polyfills. Each target is a `public static function` method whose body uses
//! only features phplang supports; the harness lifts the body into a plain
//! function (renamed `pf_*` so it never collides with a real PHP built-in),
//! composes it with a driver, and asserts phplang's stdout matches the reference
//! `php` **byte for byte**.
//!
//! The Symfony polyfills are MIT-licensed but, like the Drupal corpus, are fetched
//! into a gitignored checkout (`scripts/fetch-php-polyfill.sh`) rather than
//! vendored — only phplang's own drivers and this runner are part of the crate.
//!
//! Setup:  ./scripts/fetch-php-polyfill.sh
//! Run:    cargo build --bin php --bin parity-stdlib && ./target/debug/parity-stdlib

use std::path::{Path, PathBuf};
use std::process::Command;

/// One byte-parity case: the polyfill methods it needs (source file + method
/// name, extracted and renamed to `pf_<method>`) and a driver exercising them.
struct Case {
    label: &'static str,
    deps: &'static [(&'static str, &'static str)],
    driver: &'static str,
}

/// The curated corpus — Symfony polyfill functions phplang can run today.
const CASES: &[Case] = &[
    Case {
        label: "str_contains (polyfill-php80)",
        deps: &[("Php80.php", "str_contains")],
        driver: r#"
foreach (["hello world", "abc", "", "café"] as $h) {
    foreach (["ell", "", "abc", "z", "fé"] as $n) {
        echo pf_str_contains($h, $n) ? "1" : "0";
    }
    echo "\n";
}"#,
    },
    Case {
        label: "str_starts_with (polyfill-php80)",
        deps: &[("Php80.php", "str_starts_with")],
        driver: r#"
foreach (["hello", "abc", ""] as $h) {
    foreach (["he", "", "abc", "abcd", "x"] as $n) {
        echo pf_str_starts_with($h, $n) ? "1" : "0";
    }
    echo "\n";
}"#,
    },
    Case {
        label: "str_ends_with (polyfill-php80)",
        deps: &[("Php80.php", "str_ends_with")],
        driver: r#"
foreach (["hello", "abc", ""] as $h) {
    foreach (["lo", "", "abc", "xabc", "x"] as $n) {
        echo pf_str_ends_with($h, $n) ? "1" : "0";
    }
    echo "\n";
}"#,
    },
];

fn main() {
    let src = polyfill_src();
    let ours = ours_bin();
    if !ours.exists() {
        eprintln!(
            "parity-stdlib: phplang binary not found at {} (run `cargo build`)",
            ours.display()
        );
        std::process::exit(2);
    }
    let php = oracle_path();
    eprintln!("parity-stdlib: reference = {}", oracle_id());
    eprintln!("parity-stdlib: source    = {}", src.display());
    eprintln!("parity-stdlib: Symfony polyfill functions verified byte-for-byte:\n");

    let mut pass = 0;
    let mut fail = 0;
    let mut skip = 0;
    for case in CASES {
        let mut prog = String::new();
        let mut missing = false;
        for (file, method) in case.deps {
            let text = read_file(&src, file);
            match extract_method(&text, method) {
                Some(func) => prog.push_str(&func),
                None => {
                    println!("  SKIP  {} — `{method}` not found in {file}", case.label);
                    missing = true;
                }
            }
        }
        if missing {
            skip += 1;
            continue;
        }
        prog.push_str(case.driver);

        // BOTH sides get the same flags — the reference used to be muted with
        // `-d error_reporting=0` while phplang got none, which asks the two
        // engines different questions. phplang honours `-d error_reporting` too.
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

    println!("\nparity-stdlib: {pass} byte-identical, {fail} divergent, {skip} skipped");
    // A skip is not a pass, and neither is an empty corpus: either means the run
    // verified nothing, which must not read as success.
    if fail > 0 || skip > 0 || pass == 0 {
        std::process::exit(1);
    }
}

/// Print one side of a failed comparison — all three observables.
fn report_side(who: &str, r: &Run) {
    println!(
        "        {who} (exit {}): out={:?} err={:?}",
        r.1,
        String::from_utf8_lossy(&r.0),
        String::from_utf8_lossy(&r.2)
    );
}

/// One captured run: stdout, exit code, stderr. All three are compared — PHP
/// writes every diagnostic to both streams, so reading only stdout sees half.
type Run = (Vec<u8>, i32, Vec<u8>);

fn run(bin: &str, extra: &[&str], code: &str) -> Run {
    let out = Command::new(bin).args(extra).args(["-r", code]).output();
    match out {
        Ok(o) => (o.stdout, o.status.code().unwrap_or(-1), o.stderr),
        Err(_) => (Vec::new(), -1, Vec::new()),
    }
}

/// Lift a `public static function NAME(...): T { ... }` class method into a
/// standalone `function pf_NAME(...) { ... }`: strip the visibility/`static`
/// keywords and the return type, drop namespace backslashes (`\strlen`), and
/// rename so the function never collides with a real PHP built-in.
fn extract_method(src: &str, method: &str) -> Option<String> {
    let head = format!("public static function {method}(");
    let lines: Vec<&str> = src.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.trim_start().starts_with(&head))?;
    // Class methods are indented four spaces; the body closes at that `    }`.
    let mut end = None;
    for (i, l) in lines.iter().enumerate().skip(start + 1) {
        if *l == "    }" {
            end = Some(i);
            break;
        }
    }
    let end = end?;

    let mut out = String::new();
    for (i, raw) in lines[start..=end].iter().enumerate() {
        let line = raw.strip_prefix("    ").unwrap_or(raw); // outdent one level
        if i == 0 {
            // Signature line: rewrite the header and drop the return type.
            let sig = line
                .replacen(
                    &format!("public static function {method}("),
                    &format!("function pf_{method}("),
                    1,
                )
                .to_string();
            // Remove a trailing `: <type>` return declaration.
            let sig = match sig.rfind(')') {
                Some(p) => format!("{})", &sig[..p]),
                None => sig,
            };
            out.push_str(&sig.replace('\\', ""));
        } else {
            out.push_str(&line.replace('\\', ""));
        }
        out.push('\n');
    }
    Some(out)
}

fn read_file(src: &Path, name: &str) -> String {
    let p = src.join(name);
    std::fs::read_to_string(&p).unwrap_or_else(|_| {
        eprintln!(
            "parity-stdlib: cannot read {} — run ./scripts/fetch-php-polyfill.sh first",
            p.display()
        );
        std::process::exit(2);
    })
}

/// The polyfill checkout: `$PHPLANG_POLYFILL_SRC`, else `third_party/php-polyfill`,
/// else `/tmp`.
fn polyfill_src() -> PathBuf {
    if let Ok(p) = std::env::var("PHPLANG_POLYFILL_SRC") {
        return PathBuf::from(p);
    }
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("third_party/php-polyfill");
    if repo.join("Php80.php").exists() {
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
