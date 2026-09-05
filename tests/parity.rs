//! Differential parity against the reference PHP interpreter.
//!
//! Each block of `tests/data/parity_corpus.php` is run as `php -r <block>` and
//! all THREE observables are compared byte for byte: stdout, stderr, and the
//! exit status. Comparing only stdout hides a diagnostic that phplang failed to
//! emit (they go to stderr as well as stdout on the CLI) and hides a script that
//! agreed on its output and then exited 255.
//!
//! Two tests, because the reference is not installed everywhere:
//!
//! * [`corpus_matches_frozen_php`] replays the corpus through the BUILT `php`
//!   binary and compares against `tests/data/parity_expected.bin`, a snapshot of
//!   what the reference produced. It needs no PHP installed, so CI runs it.
//! * [`frozen_snapshot_still_matches_live_php`] re-derives that snapshot from a
//!   reference `php` on PATH and asserts the FILE matches. It skips when there
//!   is none. Without it, a snapshot quietly edited to match a phplang bug would
//!   look exactly like a passing parity run forever.
//!
//! Regenerate the snapshot with a reference interpreter — never by hand:
//!
//! ```text
//! PHPLANG_PARITY_BLESS=1 cargo test --test parity
//! ```
//!
//! The environment is PINNED (TZ/LANG/LC_ALL) rather than inherited: the
//! reference's date and locale functions read all three, so a replay under a
//! different one would be comparing against a transcript taken under different
//! conditions.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Separator between corpus blocks — a line containing exactly this.
const SEP: &str = "#==#";

/// Absolute locations a system PHP is installed at, searched before `PATH`.
///
/// `PATH` is searched only as a fallback, and what it yields is made absolute
/// before it is used: a relative `php`, or one picked up from a directory the
/// build itself writes to, is the one way this harness can end up comparing the
/// binary under test against ITSELF and reporting a clean sweep for it.
const SYSTEM_PHP: &[&str] = &[
    "/opt/homebrew/bin/php",
    "/usr/local/bin/php",
    "/usr/bin/php",
];

/// The reference interpreter, if one is installed, as an ABSOLUTE path.
/// `PHP` overrides the search so a specific build can be pinned.
///
/// Two candidates are refused rather than used:
///
/// * one whose `--version` is not a reference banner — phplang's own `php`
///   answers `php <crate version>`, so a `target/` directory that reached
///   `PATH` cannot be mistaken for the oracle;
/// * one that lives under this crate's `target/`, whatever it prints. The
///   banner test alone is a weak reed: it holds only while phplang's
///   `--version` stays un-PHP-shaped, and the failure it guards against —
///   the harness comparing the binary under test against itself and passing
///   unconditionally — is silent.
fn reference_php() -> Option<PathBuf> {
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
    let candidates: Vec<PathBuf> = match std::env::var_os("PHP") {
        Some(p) => vec![PathBuf::from(p)],
        None => SYSTEM_PHP
            .iter()
            .map(PathBuf::from)
            .chain(
                std::env::var_os("PATH")
                    .map(|path| {
                        std::env::split_paths(&path)
                            .map(|d| d.join("php"))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
            )
            .collect(),
    };
    for c in candidates {
        let abs = c.canonicalize().unwrap_or(c);
        if abs.starts_with(&target) {
            continue;
        }
        let Ok(out) = Command::new(&abs).arg("--version").output() else {
            continue;
        };
        let banner = String::from_utf8_lossy(&out.stdout);
        if banner.starts_with("PHP ") && !banner.contains("phplang") {
            return Some(abs);
        }
    }
    None
}

/// The first line of `php --version`, which names the build the comparison was
/// actually made against.
fn php_banner(php: &Path) -> String {
    Command::new(php)
        .arg("--version")
        .output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn corpus_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/parity_corpus.php")
}

/// Where the blessing interpreter's `major.minor` is recorded, so a later run
/// can tell "the snapshot is stale" from "this is a different PHP".
fn reference_version_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/parity_reference_version.txt")
}

/// `major.minor` of `php`, from the first line of `php --version`
/// (`PHP 8.5.9 (cli) …`). `None` when it cannot be parsed.
fn php_major_minor(php: &Path) -> Option<String> {
    let out = Command::new(php).arg("--version").output().ok()?;
    let banner = String::from_utf8_lossy(&out.stdout);
    let ver = banner.strip_prefix("PHP ")?.split_whitespace().next()?;
    let mut it = ver.split('.');
    Some(format!("{}.{}", it.next()?, it.next()?))
}

fn snapshot_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/parity_expected.bin")
}

/// Split the corpus into runnable blocks, dropping the leading comment header.
fn blocks(text: &str) -> Vec<String> {
    text.lines()
        .collect::<Vec<_>>()
        .split(|l| l.trim_end() == SEP)
        .map(|ls| ls.join("\n").trim().to_string())
        .filter(|b| !b.is_empty())
        .collect()
}

/// What one block produced.
struct Run {
    out: Vec<u8>,
    err: Vec<u8>,
    exit: i32,
}

fn run(bin: &Path, src: &str) -> Run {
    let out = Command::new(bin)
        // xdebug REPLACES `var_dump`: it prefixes every dump with
        // `Command line code:<line>:` and prints array keys as `[0] =>` rather
        // than `[0]=>`. The ubuntu runner's php has it loaded and this machine's
        // does not, so the same corpus produced two different references and the
        // snapshot could only ever match one of them. Turning its develop mode
        // off restores stock `var_dump`. phplang accepts and ignores `-d`, so
        // both sides can take the same flag.
        .arg("-d")
        .arg("xdebug.mode=off")
        .arg("-r")
        .arg(src)
        .env("TZ", "UTC")
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .output()
        .unwrap_or_else(|e| panic!("failed to run {}: {e}", bin.display()));
    Run {
        out: out.stdout,
        err: out.stderr,
        // A signal death has no code. -1 is not a status any exit() can
        // produce, so a crash can never be recorded as a normal one.
        exit: out.status.code().unwrap_or(-1),
    }
}

/// Serialize runs as length-prefixed records.
///
/// The bodies are stored raw, with their byte counts in the header, so no
/// escaping is needed and no output can be mistaken for a delimiter — including
/// one that itself contains a line reading `#REC`.
fn encode(runs: &[Run]) -> Vec<u8> {
    let mut buf = Vec::new();
    for (i, r) in runs.iter().enumerate() {
        buf.extend_from_slice(
            format!(
                "#REC {i} exit={} out={} err={}\n",
                r.exit,
                r.out.len(),
                r.err.len()
            )
            .as_bytes(),
        );
        buf.extend_from_slice(&r.out);
        buf.extend_from_slice(&r.err);
        buf.push(b'\n');
    }
    buf
}

fn decode(mut buf: &[u8]) -> Vec<Run> {
    let mut runs = Vec::new();
    while !buf.is_empty() {
        let nl = buf
            .iter()
            .position(|&b| b == b'\n')
            .expect("snapshot record header has no newline");
        let header = std::str::from_utf8(&buf[..nl]).expect("snapshot header is not UTF-8");
        let field = |name: &str| -> usize {
            header
                .split_whitespace()
                .find_map(|f| f.strip_prefix(name))
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| panic!("snapshot header missing {name}: {header:?}"))
        };
        let exit: i32 = header
            .split_whitespace()
            .find_map(|f| f.strip_prefix("exit="))
            .and_then(|v| v.parse().ok())
            .expect("snapshot header missing exit=");
        let (no, ne) = (field("out="), field("err="));
        buf = &buf[nl + 1..];
        let out = buf[..no].to_vec();
        let err = buf[no..no + ne].to_vec();
        // The trailing newline is a separator this writes, not part of the body.
        buf = &buf[no + ne + 1..];
        runs.push(Run { out, err, exit });
    }
    runs
}

/// Render a difference between two runs, or `None` when they agree.
fn diff_named(want: &Run, got: &Run, want_label: &str, got_label: &str) -> Option<String> {
    let mut parts = Vec::new();
    if want.out != got.out {
        parts.push(format!(
            "  stdout\n    {want_label:<8}: {:?}\n    {got_label:<8}: {:?}",
            String::from_utf8_lossy(&want.out),
            String::from_utf8_lossy(&got.out)
        ));
    }
    if want.err != got.err {
        parts.push(format!(
            "  stderr\n    {want_label:<8}: {:?}\n    {got_label:<8}: {:?}",
            String::from_utf8_lossy(&want.err),
            String::from_utf8_lossy(&got.err)
        ));
    }
    if want.exit != got.exit {
        parts.push(format!(
            "  exit status\n    {want_label:<8}: {}\n    {got_label:<8}: {}",
            want.exit, got.exit
        ));
    }
    (!parts.is_empty()).then(|| parts.join("\n"))
}

/// The first line of a block, for naming it in a failure report.
fn label(src: &str) -> &str {
    src.lines()
        .find(|l| !l.trim_start().starts_with("//") && !l.trim().is_empty())
        .unwrap_or("")
}

#[test]
fn corpus_matches_frozen_php() {
    let corpus = std::fs::read_to_string(corpus_path()).expect("read parity corpus");
    let blocks = blocks(&corpus);
    // An empty corpus satisfies every check below — the count compares 0 to 0
    // and the loop runs zero times — so the test would report PASS having
    // replayed nothing. A corpus that stopped splitting is a harness failure,
    // not a passing parity run.
    assert!(
        blocks.len() >= 20,
        "only {} corpus blocks parsed from {} — the corpus or its `{SEP}` \
         separator is broken; a replay over nothing is not a passing run",
        blocks.len(),
        corpus_path().display()
    );

    let snapshot = std::fs::read(snapshot_path()).unwrap_or_else(|e| {
        panic!(
            "missing {}: {e}\nregenerate it from a reference interpreter with \
             PHPLANG_PARITY_BLESS=1 cargo test --test parity",
            snapshot_path().display()
        )
    });
    let expected = decode(&snapshot);
    assert_eq!(
        blocks.len(),
        expected.len(),
        "corpus has {} blocks but the snapshot has {} records; regenerate with \
         PHPLANG_PARITY_BLESS=1",
        blocks.len(),
        expected.len()
    );

    let bin = PathBuf::from(env!("CARGO_BIN_EXE_php"));
    let mut failures = Vec::new();
    for (i, (src, want)) in blocks.iter().zip(&expected).enumerate() {
        let got = run(&bin, src);
        if let Some(d) = diff_named(want, &got, "snapshot", "phplang") {
            failures.push(format!("──── block #{i}: {}\n{d}", label(src)));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} corpus blocks diverge from the reference:\n\n{}",
        failures.len(),
        blocks.len(),
        failures.join("\n\n")
    );
}

#[test]
fn frozen_snapshot_still_matches_live_php() {
    let Some(php) = reference_php() else {
        // No reference installed. The frozen replay above still runs, so this
        // is a reduction in coverage rather than a hole: CI keeps comparing
        // phplang against the recorded reference behaviour.
        eprintln!("no reference `php` on PATH — skipping live re-derivation");
        return;
    };
    // Say WHICH interpreter answered. A parity result is a claim about a
    // specific build, and a run that does not name it cannot be reproduced or
    // contradicted — the snapshot's `major.minor` pin below is a coarser
    // version of the same thing.
    eprintln!("parity oracle: {} ({})", php.display(), php_banner(&php));
    let corpus = std::fs::read_to_string(corpus_path()).expect("read parity corpus");
    let blocks = blocks(&corpus);
    let live: Vec<Run> = blocks.iter().map(|b| run(&php, b)).collect();
    let encoded = encode(&live);

    if std::env::var_os("PHPLANG_PARITY_BLESS").is_some() {
        std::fs::write(snapshot_path(), &encoded).expect("write snapshot");
        if let Some(v) = php_major_minor(&php) {
            std::fs::write(reference_version_path(), format!("{v}\n"))
                .expect("write reference version");
        }
        eprintln!(
            "blessed {} records into {} from {}",
            live.len(),
            snapshot_path().display(),
            php.display()
        );
        return;
    }

    // A snapshot records ONE interpreter's behaviour, and PHP changes it between
    // releases: 8.4 added the "float is not representable as an int" warning
    // that 8.3 does not emit, so a snapshot blessed on one reports every such
    // block as drift against the other. Compare only when the reference is the
    // same major.minor that blessed it; otherwise say so and stop, the way the
    // no-reference path above does. The frozen replay
    // (corpus_matches_frozen_php) still runs either way, so phplang is still
    // held to the recorded behaviour — what is skipped is only the
    // re-derivation that proves the recording is honest.
    let blessed_with = std::fs::read_to_string(reference_version_path())
        .ok()
        .map(|s| s.trim().to_string());
    let local = php_major_minor(&php);
    if let (Some(b), Some(l)) = (&blessed_with, &local) {
        if b != l {
            eprintln!(
                "snapshot was blessed with PHP {b}, this reference is PHP {l} — \
                 skipping live re-derivation. Re-bless on {b} (or update the \
                 pin) with PHPLANG_PARITY_BLESS=1 cargo test --test parity"
            );
            return;
        }
    }

    let frozen = std::fs::read(snapshot_path()).expect("read snapshot");
    if frozen == encoded {
        return;
    }
    // Report WHICH block drifted rather than "the files differ": the cause is
    // either a reference upgrade or a snapshot edited by hand, and the block
    // that changed is what tells them apart.
    let stored = decode(&frozen);
    let mut drift = Vec::new();
    for (i, l) in live.iter().enumerate() {
        match stored.get(i) {
            Some(s) => {
                if let Some(d) = diff_named(s, l, "snapshot", "live php") {
                    drift.push(format!("──── block #{i}: {}\n{d}", label(&blocks[i])));
                }
            }
            None => drift.push(format!(
                "──── block #{i}: {} has no record",
                label(&blocks[i])
            )),
        }
    }
    if stored.len() > live.len() {
        drift.push(format!(
            "snapshot has {} records for {} corpus blocks",
            stored.len(),
            live.len()
        ));
    }
    panic!(
        "tests/data/parity_expected.bin does not match {} ({} block(s) drifted).\n\
         Regenerate it FROM THE REFERENCE — never edit it to match phplang:\n\
         PHPLANG_PARITY_BLESS=1 cargo test --test parity\n\n{}",
        php.display(),
        drift.len(),
        drift.join("\n\n")
    );
}
