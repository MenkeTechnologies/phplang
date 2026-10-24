//! Differential fuzz of the byte-string model (`src/bytes.rs`).
//!
//! A seeded generator builds random byte strings biased toward the inputs the
//! model has to get right: stray bytes, truncated UTF-8 sequences, valid
//! multibyte characters, and the four-byte encodings of the characters the
//! model uses as escapes (`U+10FF80..=U+10FFFF`, bytes `F4 8F BE|BF xx`) —
//! written as text, as halves that only meet under concatenation, and as
//! `"\u{…}"` escapes. Each case is one `php -r` program that pushes its strings
//! through the byte-observable operations (length, hex, hashes, slicing,
//! reversal, splitting, searching and replacing, joining, regexes, formatting,
//! array keys, serialization, JSON, file I/O) and prints every result as hex,
//! so any byte the engine gained, lost, merged or reinterpreted shows up.
//!
//! Same two-test shape as `tests/parity.rs`:
//!
//! * [`byte_fuzz_matches_frozen_php`] replays every case through the built
//!   `php` and compares against `tests/data/byte_fuzz_expected.bin`, recorded
//!   from the reference. It needs no PHP installed.
//! * [`byte_fuzz_snapshot_still_matches_live_php`] re-derives the snapshot
//!   from a reference `php` and asserts the file matches; it skips when none
//!   is installed.
//!
//! Regenerate the snapshot from a reference interpreter — never by hand:
//!
//! ```text
//! PHPLANG_BYTE_FUZZ_BLESS=1 cargo test --test byte_string_fuzz
//! ```

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Number of generated programs. Each carries [`STRINGS`] random strings.
const CASES: u64 = 160;
const STRINGS: usize = 4;

/// splitmix64 — deterministic, no dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + self.below((hi - lo + 1) as u64) as i64
    }
}

/// One random fragment of a byte string.
fn fragment(r: &mut Rng, out: &mut Vec<u8>) {
    match r.below(10) {
        0 | 1 => out.push(b"abcxyz0 |<>"[r.below(11) as usize]),
        // A byte that is never valid on its own.
        2 => out.push(0x80 + r.below(0x80) as u8),
        // Valid multibyte characters: é, €, 😀, U+10FFFD-ish neighbours.
        3 => out.extend_from_slice(
            ["é", "€", "😀", "Ω", "\u{10FF7F}", "\u{FFFD}"][r.below(6) as usize].as_bytes(),
        ),
        // A character in the escape range, written as its real bytes.
        4 | 5 => out.extend_from_slice(&[
            0xF4,
            0x8F,
            0xBE + r.below(2) as u8,
            0x80 + r.below(0x40) as u8,
        ]),
        // A truncated sequence: the lead (and some trail) of a character.
        6 => {
            let c = ["é", "€", "😀", "\u{10FFFF}"][r.below(4) as usize].as_bytes();
            out.extend_from_slice(&c[..1 + r.below(c.len() as u64 - 1) as usize]);
        }
        // A trail byte, or a run of them.
        7 => (0..1 + r.below(3)).for_each(|_| out.push(0x80 + r.below(0x40) as u8)),
        // Overlong, surrogate and past-U+10FFFF forms.
        8 => out.extend_from_slice(
            [
                &[0xC0u8, 0x80][..],
                &[0xED, 0xA0, 0x80],
                &[0xF4, 0x90, 0x80, 0x80],
                &[0xFF],
                &[0x00],
            ][r.below(5) as usize],
        ),
        _ => out.push(r.below(0x80) as u8),
    }
}

fn random_bytes(r: &mut Rng) -> Vec<u8> {
    let mut b = Vec::new();
    for _ in 0..r.below(9) {
        fragment(r, &mut b);
    }
    b
}

/// `b` as a PHP double-quoted literal: every byte that is not a plain
/// alphanumeric is a `\xHH` escape, so the program text is ASCII.
fn lit(b: &[u8]) -> String {
    let mut s = String::from("\"");
    for &c in b {
        if c.is_ascii_alphanumeric() {
            s.push(c as char);
        } else {
            let _ = write!(s, "\\x{c:02X}");
        }
    }
    s.push('"');
    s
}

/// The PHP program for case `seed`.
fn program(seed: u64) -> String {
    let mut r = Rng(seed.wrapping_mul(0x2545_F491_4F6C_DD1D) ^ 0xB5AD_4ECE_DA1C_E2A9);
    let strs: Vec<Vec<u8>> = (0..STRINGS).map(|_| random_bytes(&mut r)).collect();
    let mut p =
        String::from("$h = fn($v) => is_string($v) ? bin2hex($v) : var_export($v, true);\n");
    for (i, s) in strs.iter().enumerate() {
        // Some strings are built by joining two halves that only form a
        // character (or an escape-range character) once they meet.
        if s.len() > 1 && r.below(3) == 0 {
            let cut = 1 + r.below(s.len() as u64 - 1) as usize;
            let _ = writeln!(p, "$s{i} = {} . {};", lit(&s[..cut]), lit(&s[cut..]));
        } else {
            let _ = writeln!(p, "$s{i} = {};", lit(s));
        }
    }
    // A string holding escape-range characters written as `\u{…}` escapes.
    let _ = writeln!(
        p,
        "$u = \"a\\u{{10FF{:02X}}}b\\u{{10FFFF}}\";",
        0x80 + r.below(0x80)
    );
    for i in 0..STRINGS {
        let s = &strs[i];
        let j = (i + 1) % STRINGS;
        let (a, b) = (r.range(-6, 8), r.range(-6, 8));
        let k = r.range(1, 4);
        // A needle cut from the string's own bytes (possibly mid-character).
        let needle = if s.is_empty() {
            vec![0xA9]
        } else {
            let at = r.below(s.len() as u64) as usize;
            let len = 1 + r.below((s.len() - at).min(3) as u64) as usize;
            s[at..at + len].to_vec()
        };
        let n = lit(&needle);
        let _ = writeln!(
            p,
            r##"$s = $s{i}; $t = $s{j}; $n = {n};
echo "{i}:", strlen($s), " ", bin2hex($s), " ", md5($s), " ", crc32($s), "\n";
echo $h(substr($s, {a}, {b})), " ", $h(substr($s, {a})), " ", $h(strrev($s)), " ", implode(",", array_map('bin2hex', str_split($s, {k}))), "\n";
echo $h($s . $t), " ", var_export($s . $t === {st}, true), " ", strlen($t . $s . $u), " ", $h($u . $s), "\n";
echo $h(str_replace($n, "<>", $s)), " ", implode(",", array_map('bin2hex', explode($n, $s))), " ", var_export(strpos($s, $n), true), " ", var_export(strrpos($s, $n), true), " ", substr_count($s, $n), "\n";
echo json_encode([str_contains($s, $n), str_starts_with($s, $n), str_ends_with($s, $n)]), " ", $h(strstr($s, $n)), " ", $h(strrchr($s, $n)), " ", $h(stristr($s, $n)), "\n";
echo $h(implode($n, [$s, $t, $u])), " ", $h(preg_replace('/' . preg_quote($n, '/') . '/', "#", $s)), " ", implode(",", array_map('bin2hex', preg_split('/' . preg_quote($n, '/') . '/', $s))), " ", preg_match_all('/./s', $s), "\n";
echo $h(sprintf("[%6s|%-6s|%.3s|%'#8s]", $s, $t, $s, $n)), " ", $h(str_pad($s, {pad}, $n)), " ", $h(wordwrap($s, {k}, "|", true)), " ", $h(chunk_split($s, {k}, "|")), "\n";
$a = [$s => 1, $t => 2, $u => 3, $n => 4]; echo count($a), " ", implode(",", array_map('bin2hex', array_keys($a))), " ", $h(array_search(2, $a)), "\n";
echo bin2hex(serialize([$s, $u])), " ", var_export(unserialize(serialize($s)) === $s, true), " ", bin2hex(var_export($s, true)), "\n";
echo $h(json_encode($s)), " ", json_last_error(), " ", $h(json_encode($s, JSON_INVALID_UTF8_SUBSTITUTE)), " ", $h(json_encode($u)), "\n";
echo $h(strtoupper($s)), " ", $h(ucfirst($s)), " ", $h(trim($s)), " ", $h(addslashes($s)), " ", $h(nl2br($s)), " ", strcmp($s, $t) <=> 0, "\n";
$f = sys_get_temp_dir() . "/phplang_byte_fuzz_" . getmypid(); file_put_contents($f, $s . $u); echo $h(file_get_contents($f)), " ", filesize($f), "\n"; unlink($f);
echo "[", $s, "]\n";
"##,
            st = lit(&[&s[..], &strs[j][..]].concat()),
            pad = r.range(0, 12),
        );
    }
    p
}

const SYSTEM_PHP: &[&str] = &[
    "/opt/homebrew/bin/php",
    "/usr/local/bin/php",
    "/usr/bin/php",
];

/// The reference interpreter as an absolute path, refusing anything under this
/// crate's `target/` or answering with a non-reference banner (see
/// `tests/parity.rs`, whose search this mirrors).
fn reference_php() -> Option<PathBuf> {
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
    let candidates: Vec<PathBuf> = match std::env::var_os("PHP") {
        Some(p) => vec![PathBuf::from(p)],
        None => SYSTEM_PHP.iter().map(PathBuf::from).collect(),
    };
    candidates.into_iter().find_map(|c| {
        let abs = c.canonicalize().ok()?;
        if abs.starts_with(&target) {
            return None;
        }
        let out = Command::new(&abs).arg("--version").output().ok()?;
        let banner = String::from_utf8_lossy(&out.stdout);
        (banner.starts_with("PHP ") && !banner.contains("phplang")).then_some(abs)
    })
}

fn snapshot_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/byte_fuzz_expected.bin")
}

/// stdout, stderr and exit status of one program, as one record.
fn run(bin: &Path, src: &str) -> Vec<u8> {
    let out = Command::new(bin)
        .args(["-d", "xdebug.mode=off", "-r", src])
        .env("TZ", "UTC")
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .output()
        .unwrap_or_else(|e| panic!("failed to run {}: {e}", bin.display()));
    let mut rec = format!(
        "exit={} out={} err={}\n",
        out.status.code().unwrap_or(-1),
        out.stdout.len(),
        out.stderr.len()
    )
    .into_bytes();
    rec.extend_from_slice(&out.stdout);
    rec.extend_from_slice(&out.stderr);
    rec
}

/// All records, each behind a `#CASE n len` header carrying its byte count, so
/// no output can be mistaken for a separator.
fn transcript(bin: &Path) -> Vec<u8> {
    let mut buf = Vec::new();
    for seed in 0..CASES {
        let rec = run(bin, &program(seed));
        buf.extend_from_slice(format!("#CASE {seed} {}\n", rec.len()).as_bytes());
        buf.extend_from_slice(&rec);
        buf.push(b'\n');
    }
    buf
}

/// Split a transcript back into per-case records.
fn records(mut buf: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    while !buf.is_empty() {
        let nl = buf.iter().position(|&b| b == b'\n').expect("record header");
        let header = std::str::from_utf8(&buf[..nl]).expect("header is ASCII");
        let len: usize = header
            .rsplit(' ')
            .next()
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("bad record header {header:?}"));
        out.push(buf[nl + 1..nl + 1 + len].to_vec());
        buf = &buf[nl + 2 + len..];
    }
    out
}

#[test]
fn byte_fuzz_matches_frozen_php() {
    let snapshot = std::fs::read(snapshot_path()).unwrap_or_else(|e| {
        panic!(
            "missing {}: {e}\nregenerate it from a reference interpreter with \
             PHPLANG_BYTE_FUZZ_BLESS=1 cargo test --test byte_string_fuzz",
            snapshot_path().display()
        )
    });
    let want = records(&snapshot);
    assert_eq!(want.len() as u64, CASES, "snapshot is stale; re-bless it");
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_php"));
    let mut failures = Vec::new();
    for (seed, want) in (0..CASES).zip(&want) {
        let src = program(seed);
        let got = run(&bin, &src);
        if &got != want {
            let (w, g) = (String::from_utf8_lossy(want), String::from_utf8_lossy(&got));
            let first = w
                .lines()
                .skip(1)
                .zip(g.lines().skip(1))
                .find(|(a, b)| a != b)
                .map(|(a, b)| format!("\n  php    : {a}\n  phplang: {b}"))
                .unwrap_or_default();
            failures.push(format!("── case {seed}{first}\n{src}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {CASES} byte-string cases diverge from the reference:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

#[test]
fn byte_fuzz_snapshot_still_matches_live_php() {
    let Some(php) = reference_php() else {
        eprintln!("no reference `php` installed — skipping live re-derivation");
        return;
    };
    let live = transcript(&php);
    if std::env::var_os("PHPLANG_BYTE_FUZZ_BLESS").is_some() {
        std::fs::write(snapshot_path(), &live).expect("write snapshot");
        return;
    }
    let frozen = std::fs::read(snapshot_path()).expect("read snapshot");
    assert!(
        frozen == live,
        "tests/data/byte_fuzz_expected.bin no longer matches {}; re-bless it with \
         PHPLANG_BYTE_FUZZ_BLESS=1 cargo test --test byte_string_fuzz",
        php.display()
    );
}
