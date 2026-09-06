//! Differential parity fuzzer: reference `php -r <s>` vs phplang `php -r <s>`.
//!
//! Generates thousands of grammar-driven, deterministic-output PHP snippets, runs
//! each through both interpreters, and reports every case where STDOUT, STDERR or
//! the exit code differs. Each case is produced from a per-index seed so any
//! divergence replays exactly, from the SEED the report prints (not the case
//! index): `parity-fuzz --once --seed <N>`, plus `--mode NAME` when the run that
//! found it was filtered.
//!
//! All three observables are compared, and each was added because leaving it out
//! was a blind spot rather than a considered exclusion — see [`differs`], whose
//! doc records what each relaxation used to hide.
//!
//! Ported from the rubylang harness (same RunOut / render / differs /
//! run_with_timeout infra, seed→deterministic Mode dispatch, parallel workers,
//! delta-debug `minimize`, gap `signature`, `--once` replay, report file under
//! `target/parity-fuzz/divergences.txt`). Only the generators and the invocation
//! (PHP, not Ruby) differ.
//!
//! The generators are biased toward the historically weak areas of a PHP frontend
//! (float shortest-repr, integer division/modulo sign, `sprintf`/`number_format`,
//! loose-vs-strict comparison, `sort` ordering, string coercion). Pure random
//! bytes only produce mutual syntax errors that agree on both sides and teach
//! nothing. No `rand`, no time, no object ids — nothing whose output is
//! nondeterministic for reasons unrelated to parity.
//!
//! Every program is MEANT to print something deterministic, so that an
//! empty-vs-empty run cannot hide a gap. That is an aspiration, not an enforced
//! invariant, and the `Barren` verdict exists because it does not hold: an arm
//! can echo a value that is legitimately empty (`str_repeat($s, 0)`,
//! `strpbrk()` returning false, an `array_filter` that keeps nothing, `!!0`,
//! `$x = ""; $x ?? "d"`). Measured over a 62k run against the modes then present,
//! 248 cases in SEVEN of them — unary, coalesce, str2, strfns, stredge, arr3,
//! closures — with no mode above 6.0% of its own cases. All incidental; none is
//! a mode whose programs
//! structurally lack an output construct. Wrapping those arms' values in a
//! delimiter, the way `sprintf_rich` already does, would take the count to zero
//! without weakening what they compare.
//!
//! Build:  cargo build --bin parity-fuzz
//! Run:    ./target/debug/parity-fuzz --count 5000

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// PRNG — inline splitmix64, no `rand` dependency.
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn seed(s: u64) -> Rng {
        Rng(s ^ 0x9E37_79B9_7F4A_7C15)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        let i = self.below(xs.len());
        &xs[i]
    }
}

// ---------------------------------------------------------------------------
// Interpreter locations / invocation.
// ---------------------------------------------------------------------------

/// The phplang binary under test — a sibling of this harness exe. Always an
/// absolute path so it can never be confused with the reference `php` on PATH.
fn ours_bin() -> PathBuf {
    if let Ok(p) = std::env::var("PHPLANG_FUZZ_OURS") {
        return PathBuf::from(p);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let cand = dir.join("php");
            if cand.exists() {
                return cand;
            }
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("debug")
        .join("php")
}

/// The ORACLE: the reference PHP. `PHPLANG_FUZZ_PHP` names it explicitly (a hard
/// error if unusable — falling back would silently answer a different question);
/// otherwise the first existing system path wins. Never resolves to `target/`.
fn oracle_path() -> &'static str {
    static ORACLE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ORACLE.get_or_init(|| {
        if let Ok(p) = std::env::var("PHPLANG_FUZZ_PHP") {
            if !Path::new(&p).exists() {
                eprintln!("parity-fuzz: PHPLANG_FUZZ_PHP={p}: no such file");
                std::process::exit(2);
            }
            // An oracle that is phplang ITSELF compares the binary under test
            // against itself: every case agrees, and the run reports a clean
            // sweep having asked nothing. Refused rather than warned about,
            // because the result of such a run is indistinguishable from a
            // genuine one.
            if !is_reference_php(&p) {
                eprintln!(
                    "parity-fuzz: PHPLANG_FUZZ_PHP={p} is not a reference PHP \
                     (`--version` must print a `PHP …` banner that is not phplang's)"
                );
                std::process::exit(2);
            }
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
    })
}

/// Whether `path` is a REFERENCE PHP rather than the binary under test.
///
/// phplang's own `php --version` answers `php <crate version>`; the reference
/// answers `PHP 8.x.y (cli) …`. Both halves are checked, so a future phplang
/// banner that starts spelling `PHP` still cannot pass for the oracle.
fn is_reference_php(path: &str) -> bool {
    Command::new(path)
        .arg("--version")
        .output()
        .ok()
        .map(|o| {
            let banner = String::from_utf8_lossy(&o.stdout);
            banner.starts_with("PHP ") && !banner.contains("phplang")
        })
        .unwrap_or(false)
}

fn oracle_id() -> String {
    let path = oracle_path();
    let ver = Command::new(path)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    format!("{path} ({ver})")
}

/// Captured process result — raw bytes, since output need not be valid UTF-8.
struct RunOut {
    stdout: Vec<u8>,
    /// The second stream, captured rather than discarded. PHP writes every
    /// diagnostic TWICE — the `display_errors` copy on stdout and a `PHP `-
    /// prefixed `log_errors` copy here — so a harness that drops this one is
    /// structurally incapable of reporting a stderr-only divergence, however
    /// many cases it runs. It dropped it for every mode this file had, and a
    /// missing stderr copy for every `Warning` sat under those runs undetected.
    stderr: Vec<u8>,
    exit: i32,
    timed_out: bool,
    infra_fail: bool,
}

/// Render captured bytes for a report, trimming one trailing newline.
fn render(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches('\n')
        .to_string()
}

/// The divergence predicate: BOTH streams must match exactly, and so must the
/// exit code.
///
/// Every relaxation here is a whole axis the harness cannot see, so each one has
/// to earn its place, and neither of the two this predicate used to make does:
///
/// * stderr was discarded outright. It carries the `log_errors` copy of every
///   diagnostic, which is half of what PHP emits.
/// * the exit code was compared only as zero-vs-nonzero, on the stated grounds
///   that "reference PHP exits 255 on a fatal error while phplang exits 1". That
///   is not true and is not what the code does — `src/main.rs` has exited 255 on
///   a fatal since it was written (`FATAL_EXIT`), and `exit(3)` must now leave a
///   3. Under the loose form `exit(3)` and `exit(9)` were indistinguishable.
fn differs(a: &RunOut, b: &RunOut) -> bool {
    a.stdout != b.stdout || a.stderr != b.stderr || a.exit != b.exit
}

/// Spawn `cmd` and wait up to `timeout`, killing it if it overruns.
fn run_with_timeout(mut cmd: Command, timeout: Duration) -> RunOut {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return infra(-999),
    };
    // Each stream is drained on its own thread, started before the wait: two
    // piped streams read one-after-the-other after exit would deadlock the
    // moment either filled its pipe buffer, and a diagnostic-heavy program
    // fills stderr as readily as stdout.
    let out_pipe = child.stdout.take();
    let err_pipe = child.stderr.take();
    let out_thread = std::thread::spawn(move || drain(out_pipe));
    let err_thread = std::thread::spawn(move || drain(err_pipe));
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return RunOut {
                    stdout: out_thread.join().unwrap_or_default(),
                    stderr: err_thread.join().unwrap_or_default(),
                    exit: status.code().unwrap_or(-1),
                    timed_out: false,
                    infra_fail: false,
                };
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = out_thread.join();
                    let _ = err_thread.join();
                    return RunOut {
                        stdout: Vec::new(),
                        stderr: Vec::new(),
                        exit: -1,
                        timed_out: true,
                        infra_fail: false,
                    };
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(_) => {
                let _ = out_thread.join();
                let _ = err_thread.join();
                return infra(-998);
            }
        }
    }
}

/// Read one child pipe to EOF.
fn drain(pipe: Option<impl std::io::Read>) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Some(mut p) = pipe {
        let _ = p.read_to_end(&mut buf);
    }
    buf
}

/// A run that never happened — the harness could not spawn or read the child.
/// Distinct from a run that produced nothing, which is a real comparison.
fn infra(exit: i32) -> RunOut {
    RunOut {
        stdout: Vec::new(),
        stderr: Vec::new(),
        exit,
        timed_out: false,
        infra_fail: true,
    }
}

fn run_oracle(script: &str, timeout: Duration) -> RunOut {
    let mut cmd = Command::new(oracle_path());
    // No `-d error_reporting=0`: PHP writes Warning/Deprecated diagnostics to
    // *stdout* under the CLI defaults, so they are part of the output being
    // compared and silencing them on one side only would hide real divergences.
    // stderr is discarded either way (see `run_with_timeout`).
    cmd.args(["-r", script]);
    run_with_timeout(cmd, timeout)
}

fn run_ours(script: &str, bin: &Path, timeout: Duration) -> RunOut {
    let mut cmd = Command::new(bin);
    cmd.args(["-r", script]);
    run_with_timeout(cmd, timeout)
}

// ---------------------------------------------------------------------------
// Generators — one per Mode. Each returns a statement list joined by newlines.
// Every program echoes something deterministic.
// ---------------------------------------------------------------------------

const INTS: &[&str] = &[
    "0", "1", "2", "7", "10", "-3", "-7", "42", "100", "-1", "5", "9", "-2",
];
const FLOATS: &[&str] = &[
    "0.1", "0.2", "1.5", "3.14", "2.0", "-1.5", "10.0", "0.0", "100.25", "-0.5", "1.0", "0.3",
];
const WORDS: &[&str] = &[
    "foo", "bar", "baz", "hello", "world", "abc", "PHP", "Lang", "x",
];

fn ii<'a>(r: &mut Rng) -> &'a str {
    r.pick(INTS)
}
fn ff<'a>(r: &mut Rng) -> &'a str {
    r.pick(FLOATS)
}
fn ww<'a>(r: &mut Rng) -> &'a str {
    r.pick(WORDS)
}

fn gen_arith(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let ops = ["+", "-", "*"];
    let (a, b, c) = (ii(r), ii(r), ii(r));
    let (o1, o2) = (r.pick(&ops), r.pick(&ops));
    vec![format!("echo {a} {o1} {b} {o2} {c};")]
}

fn gen_intdiv(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let (a, b) = (ii(r), r.pick(&["3", "-3", "2", "-2", "7", "-7", "4", "5"]));
    match r.below(3) {
        0 => vec![format!("echo {a} % {b};")],
        1 => vec![format!("echo intdiv({a}, {b});")],
        _ => vec![format!("echo {a} / {b};")],
    }
}

fn gen_floatfmt(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    match r.below(4) {
        0 => vec![format!("echo {} + {};", ff(r), ff(r))],
        1 => vec![format!("echo {} * {};", ff(r), ff(r))],
        2 => vec![format!(
            "echo {} / {};",
            ii(r),
            r.pick(&["3", "7", "9", "4"])
        )],
        _ => vec![format!("echo {};", ff(r))],
    }
}

fn gen_pow(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let (a, b) = (
        r.pick(&["2", "3", "5", "10", "-2"]),
        r.pick(&["0", "1", "2", "3", "4"]),
    );
    vec![format!("echo {a} ** {b};")]
}

fn gen_concat(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    match r.below(4) {
        0 => vec![format!("echo \"{}\" . {};", ww(r), ii(r))],
        1 => vec![format!("echo \"{}\" + {};", ii(r), ii(r))], // numeric-string add
        2 => vec![format!("echo {} . {};", ii(r), ff(r))],
        _ => vec![format!("echo \"{}\" . \"{}\";", ww(r), ww(r))],
    }
}

fn gen_compare(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let ops = ["==", "===", "!=", "<", ">", "<=", ">="];
    let lhs = *r.pick(&[
        "1", "0", "\"1\"", "\"0\"", "\"a\"", "1.0", "\"1.0\"", "10", "\"10\"",
    ]);
    let rhs = *r.pick(&[
        "1", "0", "\"1\"", "\"0\"", "\"a\"", "1.0", "\"1.0\"", "2", "\"2\"",
    ]);
    let op = r.pick(&ops);
    vec![format!("echo ({lhs} {op} {rhs}) ? \"T\" : \"F\";")]
}

fn gen_ternary(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    match r.below(3) {
        0 => vec![format!(
            "$x = {}; echo $x ?: \"empty\";",
            r.pick(&["0", "5", "\"\"", "\"hi\""])
        )],
        1 => vec![format!("echo {} > {} ? \"a\" : \"b\";", ii(r), ii(r))],
        _ => vec![format!(
            "$x = {}; echo $x ?? \"nil\";",
            r.pick(&["null", "0", "7"])
        )],
    }
}

fn gen_strfns(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let w = ww(r);
    match r.below(8) {
        0 => vec![format!("echo strlen(\"{w}\");")],
        1 => vec![format!("echo strtoupper(\"{w}\");")],
        2 => vec![format!("echo ucfirst(\"{w}\");")],
        3 => vec![format!("echo strrev(\"{w}\");")],
        4 => vec![format!("echo str_repeat(\"{w}\", {});", r.below(4))],
        5 => vec![format!(
            "echo substr(\"{w}\", {}, {});",
            r.below(3),
            1 + r.below(3)
        )],
        6 => vec![format!("echo str_pad(\"{w}\", {}, \"-\");", 4 + r.below(4))],
        _ => vec![format!(
            "echo strpos(\"hello\", \"{}\") === false ? \"no\" : \"yes\";",
            r.pick(&["l", "z", "e"])
        )],
    }
}

fn gen_arrays(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let arr = format!("[{}, {}, {}]", ii(r), ii(r), ii(r));
    match r.below(8) {
        0 => vec![format!("echo count({arr});")],
        1 => vec![format!("echo array_sum({arr});")],
        2 => vec![format!("echo implode(\",\", {arr});")],
        3 => vec![format!("echo implode(\",\", array_reverse({arr}));")],
        4 => vec![format!("echo in_array({}, {arr}) ? \"y\" : \"n\";", ii(r))],
        5 => vec![format!("echo array_product({arr});")],
        // `count` with a MODE, on a subject that NESTS. `arr` above is three
        // scalars, so COUNT_RECURSIVE and COUNT_NORMAL agree on it no matter
        // how many cases run and the mode argument could be ignored entirely
        // without any case noticing.
        6 => vec![format!(
            "$n = [{}, [{}, {}], [[{}]]]; \
             echo count($n), '|', count($n, COUNT_RECURSIVE), '|', count($n, COUNT_NORMAL);",
            ii(r),
            ii(r),
            ii(r),
            ii(r)
        )],
        // An out-of-range mode, which is a ValueError rather than a fallback.
        _ => vec![format!(
            "try {{ echo count({arr}, {}); }} catch (Throwable $e) {{ \
             echo get_class($e), '|', $e->getMessage(); }}",
            r.pick(&["0", "1", "2", "99", "-1"])
        )],
    }
}

fn gen_sorting(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let arr = format!("[{}, {}, {}, {}]", ii(r), ii(r), ii(r), ii(r));
    // phplang's sort family returns a sorted copy; PHP sorts in place and returns
    // bool. Compare the SORTED SEQUENCE, which both agree on: reference sorts the
    // var then imploded; ours imploded the returned copy.
    match r.below(2) {
        0 => vec![format!("$a = {arr}; sort($a); echo implode(\",\", $a);")],
        _ => vec![format!("$a = {arr}; rsort($a); echo implode(\",\", $a);")],
    }
}

fn gen_assoc(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let m = format!("[\"{}\" => {}, \"{}\" => {}]", ww(r), ii(r), ww(r), ii(r));
    match r.below(3) {
        0 => vec![format!(
            "$m = {m}; $t = 0; foreach ($m as $k => $v) {{ $t += $v; }} echo $t;"
        )],
        1 => vec![format!("echo implode(\",\", array_keys({m}));")],
        _ => vec![format!(
            "echo array_key_exists(\"{}\", {m}) ? \"y\" : \"n\";",
            ww(r)
        )],
    }
}

fn gen_printf(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    match r.below(4) {
        0 => vec![format!("echo sprintf(\"%d\", {});", ii(r))],
        1 => vec![format!(
            "echo sprintf(\"%s-%d\", \"{}\", {});",
            ww(r),
            ii(r)
        )],
        2 => vec![format!("echo sprintf(\"%b\", {});", r.below(16))],
        _ => vec![format!("echo number_format({}, {});", ff(r), r.below(3))],
    }
}

fn gen_control(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    match r.below(3) {
        0 => {
            let n = ii(r);
            vec![format!(
                "$n = {n}; switch ($n) {{ case 1: echo \"one\"; break; case 2: echo \"two\"; break; default: echo \"other\"; }}"
            )]
        }
        1 => {
            let n = r.pick(&["1", "2", "3", "4"]);
            vec![format!(
                "echo match({n}) {{ 1 => \"a\", 2, 3 => \"b\", default => \"z\" }};"
            )]
        }
        _ => {
            let n = r.pick(&["0", "3", "6", "9"]);
            vec![format!(
                "$n = {n}; echo ($n % 2 == 0) ? \"even\" : \"odd\";"
            )]
        }
    }
}

fn gen_loops(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let n = r.pick(&["3", "4", "5", "6"]);
    match r.below(2) {
        0 => vec![format!(
            "$s = 0; for ($i = 1; $i <= {n}; $i++) {{ $s += $i; }} echo $s;"
        )],
        _ => vec![format!(
            "$s = 1; $i = 1; while ($i <= {n}) {{ $s *= $i; $i++; }} echo $s;"
        )],
    }
}

fn gen_funcs(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let n = r.pick(&["0", "1", "5", "6"]);
    match r.below(2) {
        0 => vec![format!(
            "function f($n) {{ if ($n <= 1) {{ return 1; }} return $n * f($n - 1); }} echo f({n});"
        )],
        _ => vec![format!(
            "function g($a, $b) {{ return $a + $b; }} echo g({}, {});",
            ii(r),
            ii(r)
        )],
    }
}

fn gen_typeconv(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let v = *r.pick(&["5", "5.7", "\"12abc\"", "\"3.9\"", "\"\"", "0", "\"42\""]);
    match r.below(4) {
        0 => vec![format!("echo intval({v});")],
        1 => vec![format!("var_dump(is_numeric({v}));")],
        2 => vec![format!("echo gettype({v});")],
        _ => vec![format!("echo (int){v} + 0;")],
    }
}

fn gen_mathfns(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    match r.below(6) {
        0 => vec![format!("echo abs({});", ii(r))],
        1 => vec![format!("echo max({}, {}, {});", ii(r), ii(r), ii(r))],
        2 => vec![format!("echo min({}, {});", ii(r), ii(r))],
        3 => vec![format!("echo floor({});", ff(r))],
        4 => vec![format!("echo ceil({});", ff(r))],
        _ => vec![format!("echo round({}, {});", ff(r), r.below(3))],
    }
}

/// `func_get_args` / `func_num_args` / `func_get_arg`, the family that reports on
/// the call frame it is standing in.
///
/// The mode exists because the generator was BLIND to all three: a grep for
/// `func_get_args` over this file returned ZERO hits, and every one of them was
/// wrong. The reference has reported a declared parameter's CURRENT value since
/// PHP 7 — `function f($a) { $a = 99; return func_get_args(); }` called as `f(1)`
/// answers `[99]` — while this engine answered `[1]` from a snapshot taken when
/// the frame was bound. Named arguments were reported in CALL order rather than
/// at their parameter's position, all three were silent at the global scope where
/// the reference raises a fatal (with three DIFFERENT messages), and an
/// out-of-range `func_get_arg` returned null instead of a `ValueError`.
///
/// The programs therefore have to mutate parameters, call with named and extra
/// and spread arguments, and reach the family from the global scope — a mode that
/// only ever called `f(1, 2)` and printed the count would score none of it.
fn gen_funcargs(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    // The global-scope arms: a fatal in the reference, one message per function.
    if r.below(6) == 0 {
        let f = *r.pick(&["func_get_args()", "func_num_args()", "func_get_arg(0)"]);
        return vec![format!("var_dump({f});")];
    }
    let params = *r.pick(&[
        "$a",
        "$a, $b",
        "$a, $b = 5",
        "$a = 1, $b = 2, $c = 3",
        "...$r",
        "$a, ...$r",
        "&$a",
        "$a, $b, $c",
    ]);
    // Mutating the parameter is the whole point of the mode: a snapshot and a
    // live read are indistinguishable until something writes to it.
    let mutate = *r.pick(&[
        "",
        "$a = 99;",
        "$a = 99; $b = 88;",
        "unset($a);",
        "$r[0] = 77;",
        "$a++;",
    ]);
    let report = *r.pick(&[
        "return func_get_args();",
        "return func_num_args();",
        "return func_get_arg(0);",
        "return func_get_arg(2);",
        "return func_get_arg(-1);",
        "return [func_num_args(), func_get_args()];",
    ]);
    let call = *r.pick(&[
        "f(1)",
        "f(1, 2)",
        "f(1, 2, 3)",
        "f()",
        "f(b: 2, a: 1)",
        "f(9, c: 7)",
        "f(...[1, 2, 3])",
        "f(1, ...[2, 3])",
    ]);
    // The same body reached through a method and a closure as well: each builds
    // its frame by a different path, and only the plain function form was ever
    // exercised by hand.
    let prog = match r.below(3) {
        0 => format!("function f({params}) {{ {mutate} {report} }} var_dump({call});"),
        1 => format!(
            "class C {{ function f({params}) {{ {mutate} {report} }} }} \
             $o = new C; var_dump($o->{call});"
        ),
        _ => format!("$f = function({params}) {{ {mutate} {report} }}; var_dump($f({call}));"),
    };
    vec![format!(
        "try {{ {prog} }} catch (Throwable $e) {{ echo get_class($e), \"|\", $e->getMessage(); }}"
    )]
}

/// `extract()` and its `EXTR_*` flags.
///
/// Another construct the generator could not see: a grep for `extract(` returned
/// ZERO hits, and neither did the flags exist. `EXTR_SKIP` and its six siblings
/// were UNDEFINED CONSTANTS, so every call that named one died with `Undefined
/// constant "EXTR_SKIP"`, and `extract()` itself ignored both `$flags` and
/// `$prefix` — it always overwrote, whatever it was asked for.
///
/// The array is built with keys that are and are not legal variable names, and
/// against variables that do and do not already exist, because half the modes
/// turn on exactly those two questions.
fn gen_extractflags(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let pre = *r.pick(&["", "$x = 1;", "$x = 1; $z = 2;", "$p_x = 0;"]);
    let arr = *r.pick(&[
        "[\"x\" => 2]",
        "[\"x\" => 2, \"z\" => 3]",
        "[\"1bad\" => 2, \"ok\" => 3]",
        "[5 => 1, \"ok\" => 2]",
        "[\"this\" => 5]",
        "[]",
        "[\"x\" => 2, \"y\" => 4, \"z\" => 6]",
    ]);
    let flags = *r.pick(&[
        "",
        ", EXTR_OVERWRITE",
        ", EXTR_SKIP",
        ", EXTR_PREFIX_SAME, \"p\"",
        ", EXTR_PREFIX_ALL, \"p\"",
        ", EXTR_PREFIX_INVALID, \"p\"",
        ", EXTR_PREFIX_IF_EXISTS, \"p\"",
        ", EXTR_IF_EXISTS",
        ", EXTR_REFS",
        ", EXTR_SKIP | EXTR_REFS",
        // The rejected forms: a mode that is not one, a prefix that is missing,
        // and a prefix that is not an identifier.
        ", 999",
        ", -1",
        ", EXTR_PREFIX_ALL",
        ", EXTR_PREFIX_ALL, \"1\"",
        ", EXTR_PREFIX_ALL, \"\"",
    ]);
    // `EXTR_REFS` is only distinguishable from a plain bind by writing THROUGH
    // the variable afterwards and looking at the array again.
    let after = *r.pick(&["", "$x = 9;", "$p_x = 9;"]);
    // Run inside a function, and name the variables to report rather than asking
    // for all of them: at the global scope `get_defined_vars()` answers with the
    // superglobals too, so the comparison would be dominated by the environment
    // the two processes were started with — which differs, is enormous, and is
    // none of this mode's business.
    let seen = "foreach ([\"x\", \"y\", \"z\", \"ok\", \"p_x\", \"p_z\", \"p_ok\", \"p_5\", \"_x\", \"p_1bad\", \"p_this\"]                 as $k) { echo $k, \"=\", isset($$k) ? var_export($$k, true) : \"-\", \"|\"; }";
    vec![format!(
        "function t() {{ {pre} $a = {arr}; $n = extract($a{flags}); \
         var_dump($n); {after} var_dump($a); {seen} }} \
         try {{ t(); }} catch (Throwable $e) {{ echo get_class($e), \"|\", $e->getMessage(); }}"
    )]
}

/// [`gen_callorder`] for every call form that is NOT a method call.
///
/// That mode settled the receiver of `$r->m(…)`, which left the same fault
/// standing in seven other spellings: an undefined function, an undeclared class
/// under both `new X(…)` and `X::m(…)`, and a `$callee(…)` whose value is an
/// array of the wrong length, a non-callable scalar, an object with no
/// `__invoke`, or a string naming nothing. Each printed its argument's output
/// before the fatal the reference reaches without printing anything.
///
/// The zero-argument spellings are in the pool on purpose: the check is emitted
/// only when a call HAS an argument, and nothing else would notice if that
/// condition were dropped and every call started paying for a second lookup.
fn gen_calleeforms(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    // A receiver/callee pool that is mostly uncallable — the successful arms are
    // there to keep the mode honest about not breaking working calls.
    let subject = *r.pick(&[
        "$n = [1];",
        "$n = false;",
        "$n = null;",
        "$n = 5;",
        "$n = \"nope\";",
        "$n = \"Nope::m\";",
        "$n = new stdClass;",
        "$n = [new stdClass, \"nope\"];",
        "$n = [1, 2, 3];",
        "class C { function m($x) { return $x * 2; } } $n = new C;",
        "$n = function ($x) { return $x; };",
        "$n = \"strtoupper\";",
    ]);
    let call = *r.pick(&[
        "$n->m(f())",
        "$n?->m(f())",
        "$n(f())",
        "undefinedfn(f())",
        "Nope::m(f())",
        "new Nope(f())",
        "C::nope(f())",
        "$n->m(f(), f())",
        "$n->m(x: f())",
        "undefinedfn(x: f())",
        "new Nope(x: f())",
        // The zero-argument spellings, where there is nothing to order and the
        // check must not have been emitted at all.
        "$n->m()",
        "undefinedfn()",
        "new Nope()",
    ]);
    vec![format!(
        "function f() {{ echo \"F\"; return 1; }} {subject} \
         try {{ var_dump({call}); }} \
         catch (Throwable $e) {{ echo get_class($e), \"|\", $e->getMessage(); }}"
    )]
}

/// PHP 8.1 first-class callable syntax, `callee(...)`, and the argument binding
/// of the `Closure` it produces.
///
/// The construct appeared ZERO times in this file before this mode: a grep for
/// `(...)` over the generators returned no hits, so every previous clean sweep
/// scored it not at all. It was in fact inert in three ways at once — the callee
/// was never checked, so `$o->nope(...)` built a closure the reference refuses
/// outright; the closure bound its arguments by POSITION, so `$f(b: 2, a: 1)`
/// filled `$a` with `2`; and the receiver was re-evaluated on every call, so
/// `f()->m(...)` ran `f` once per invocation instead of once at the syntax.
///
/// Each callee spelling carries its OWN call pool rather than drawing from a
/// shared one, because the two would otherwise be crossed into calls whose
/// arity is wrong for the callee — and a builtin enforces neither its arity nor
/// its parameter names here (`strtolower("a", "b")` answers `"a"`, and
/// `strtolower(a: "AB")` answers `"ab"` where the reference raises
/// `ArgumentCountError` and `Error: Unknown named parameter $a`). That gap is
/// real, measured, and PRE-EXISTING — it is a property of every builtin, not of
/// `(...)` — so pairing arity to callee keeps this mode scoring the callable
/// semantics it is named for instead of restating a library-wide one.
///
/// `$o?->m(...)` is NOT in the pool. The reference rejects it at COMPILE time
/// (`Cannot combine nullsafe operator with Closure creation`) through a channel
/// this engine does not have — it reports a parse error instead — so including
/// it would score the diagnostic channel rather than the callable semantics.
/// Measured against the reference, not assumed.
fn gen_fcc(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    // Declarations the callee spellings draw on. Kept in one place so a refusal
    // and the working call it mirrors name the same class.
    let decl = *r.pick(&[
        "class C { function m($a, $b = 5) { return \"$a/$b\"; } \
           static function s($a, $b = 5) { return \"s$a/$b\"; } }",
        "class C { private function m($a, $b = 5) { return \"p$a/$b\"; } \
           private static function s($a, $b = 5) { return \"ps$a/$b\"; } }",
        "class C { protected function m($a, $b = 5) { return \"q$a/$b\"; } \
           static function s($a, $b = 5) { return \"s$a/$b\"; } }",
        "class C { function __call($n, $x) { return \"call:$n:\" . count($x); } \
           static function __callStatic($n, $x) { return \"cs:$n:\" . count($x); } }",
    ]);
    // Every call spelling a two-parameter user callable accepts, including the
    // named forms whose binding the old desugaring got wrong. `$f()` is absent:
    // it raises `Too few arguments to function {closure:file:line}()`, whose
    // frame name this engine renders `{closure}` (see README) — a divergence in
    // the NAME, not in the callable.
    const USER_CALLS: &[&str] = &[
        "$f(1)",
        "$f(1, 2)",
        "$f(a: 1)",
        "$f(b: 2, a: 1)",
        "$f(1, b: 2)",
        // Never called: for a callee the reference refuses, the diagnostic must
        // already have been raised by the syntax alone.
        "\"built\"",
    ];
    // A builtin callee is called at its declared arity only, for the reason in
    // the doc comment above.
    const BUILTIN_CALLS: &[&str] = &["$f(\"Ab\")", "\"built\""];
    let (callee, calls): (&str, &[&str]) = *r.pick(&[
        // Free functions: one that exists, one that does not.
        ("strtoupper(...)", BUILTIN_CALLS),
        ("$name(...)", BUILTIN_CALLS),
        ("nosuchfunction(...)", BUILTIN_CALLS),
        // The two member forms on a declared class, reachable and not.
        ("(new C)->m(...)", USER_CALLS),
        ("(new C)->nope(...)", USER_CALLS),
        ("C::s(...)", USER_CALLS),
        ("C::nope(...)", USER_CALLS),
        // An undeclared class is reported as the missing CLASS, never as a
        // missing method.
        ("Nope::m(...)", USER_CALLS),
        // A receiver that is not an object at all, which the reference refuses
        // by the receiver's TYPE — and which is indistinguishable at run time
        // from the `$scalar::m(...)` the reference accepts.
        ("$scalar->m(...)", USER_CALLS),
        ("$scalar::s(...)", USER_CALLS),
        // A closure re-wrapped, and a receiver that must be evaluated exactly
        // ONCE — `mk()` echoes, so a second evaluation is visible in stdout.
        ("$fn(...)", USER_CALLS),
        ("mk()->m(...)", USER_CALLS),
    ]);
    let call = *r.pick(calls);
    vec![format!(
        "{decl} function mk() {{ echo \"MK\"; return new C; }} \
         $scalar = \"C\"; $name = \"strtolower\"; \
         $fn = function ($a, $b = 5) {{ return \"f$a/$b\"; }}; \
         try {{ $f = {callee}; var_dump({call}); }} \
         catch (Throwable $e) {{ echo get_class($e), \"|\", $e->getMessage(); }}"
    )]
}

/// Scalar parameter and return types, under BOTH typing modes.
///
/// This mode exists because the generator was BLIND to the whole construct: a grep
/// for a typed parameter (`(int $`, `(string $`, …) over this file returned ZERO
/// hits before it was added, and `declare` returned none either. Every previous
/// "0 divergences" run therefore scored scalar type declarations not at all — and
/// they were in fact inert, coercing nothing and rejecting nothing, so a run could
/// report a clean sweep while `function f(int $x)` handed `$x` straight through as
/// the string it was given.
///
/// Each program declares one typed function and calls it with one value, under a
/// randomly chosen mode. The call is CAUGHT and its `getMessage()` printed, which
/// is this file's established idiom for a diagnostic — and here also keeps the
/// comparison off the uncaught rendering, whose definition-site file and line
/// this engine does not yet carry (see the `DIVERGENCE` note in `README.md`).
///
/// Closures are absent on purpose and the omission is a REAL one, not an inherited
/// note: PHP 8.4 renders a closure's frame `{closure:file:line}` where this engine
/// renders `{closure}`, so a typed closure would diverge on the name rather than on
/// the typing this mode is here to test. Verified against the reference, not assumed.
fn gen_stricttypes(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    // Both spellings of the mode, plus the file that declares nothing — the
    // default is coercive, and a run that only ever emitted the `declare` would
    // score the flag's presence rather than its effect.
    let decl = *r.pick(&[
        "declare(strict_types=1);",
        "declare(strict_types=0);",
        "",
        "declare(ticks=1);",
    ]);
    let ty = *r.pick(&["int", "float", "string", "bool", "?int", "?string"]);
    // Values spanning every arm of the conversion table: exact hits, the int→float
    // widening that survives strict mode, the fully-numeric strings that convert
    // only in coercive mode, the trailing-garbage string that converts in NEITHER,
    // and the non-scalars that are a TypeError under both.
    let val = *r.pick(&[
        "5", "-3", "0", "5.0", "5.5", "\"5\"", "\"5.5\"", "\"5abc\"", "\"abc\"", "\"\"", "true",
        "false", "null", "[1]",
    ]);
    let body = match r.below(3) {
        // A return type, exercised by returning the parameter straight back.
        0 => format!("function f({ty} $x): {ty} {{ return $x; }}"),
        // A return type that does NOT match the parameter, so the return check
        // fires on a value the parameter check already accepted.
        1 => format!("function f({ty} $x): string {{ return $x; }}"),
        _ => format!("function f({ty} $x) {{ var_dump($x); }}"),
    };
    vec![format!(
        "{decl} {body} try {{ var_dump(f({val})); }} \
         catch (Throwable $e) {{ echo get_class($e), \"|\", $e->getMessage(), \"|\"; }}"
    )]
}

/// The `declare` statement's own syntax rules, every one of which PHP enforces at
/// COMPILE time — the script produces no output at all before the diagnostic, so an
/// engine that accepted the form and failed later would print the leading `echo`
/// and be caught here by that alone.
///
/// The `declare` in each program is deliberately NOT always legal: the point is the
/// rejections, and an all-valid pool would leave every rule unexercised.
fn gen_declaresyntax(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    match r.below(9) {
        // Legal, and the baseline the rejections are read against.
        0 => vec!["declare(strict_types=1); echo \"ok\";".to_string()],
        // Not the first statement — the rule most likely to be got wrong.
        1 => vec!["echo \"before\"; declare(strict_types=1);".to_string()],
        // A preceding `declare` does NOT disqualify it, unlike any other statement.
        2 => vec!["declare(ticks=1); declare(strict_types=1); echo \"ok\";".to_string()],
        // Two of them, in both orders. The mode is a LATCH rather than an
        // assignment — once on it stays on — so a `=0` after a `=1` must NOT
        // restore coercion, and an engine that simply stored the last value
        // would pass every single-`declare` arm above and fail only here.
        8 => vec![format!(
            "declare(strict_types={}); declare(strict_types={}); \
             function f(int $x) {{ var_dump($x); }} \
             try {{ f(\"5\"); }} catch (Throwable $e) {{ echo get_class($e); }}",
            r.below(2),
            r.below(2)
        )],
        // Block mode: a compile error for `strict_types` specifically…
        3 => vec!["declare(strict_types=1) { echo \"in\"; }".to_string()],
        // …but perfectly legal for `ticks`.
        4 => vec!["declare(ticks=1) { echo \"in\"; }".to_string()],
        // A value that is neither 0 nor 1, and one that is not a literal at all.
        5 => vec![format!(
            "declare(strict_types={}); echo \"ok\";",
            r.pick(&["2", "-1", "0", "1"])
        )],
        6 => vec!["declare(strict_types=$x); echo \"ok\";".to_string()],
        // An unrecognised directive warns and carries on.
        _ => vec!["declare(encoding='UTF-8'); echo \"ok\";".to_string()],
    }
}

/// `exit` / `die` — the construct the generator was blind to.
///
/// A grep for either word over the generators returned ZERO hits before this
/// mode existed, and the construct was not implemented at all: `exit(3)` was a
/// `Call to undefined function exit()` and `exit;` an `Undefined constant`. Every
/// previous clean run therefore scored the way PHP scripts end themselves not at
/// all — and could not have scored the status even if it had generated one,
/// because the harness compared exit codes only as zero-vs-nonzero, where
/// `exit(3)` and `exit(9)` are the same answer.
///
/// Each program opens with a delimiter so the case cannot be barren, and the
/// statement AFTER the `exit` is one that must not run — a construct that
/// returned instead of unwinding would print it and be caught here on stdout
/// alone, before the status is even consulted.
fn gen_exitdie(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let word = *r.pick(&["exit", "die"]);
    // Every arm of the `string|int` parameter: absent, the two silent int forms,
    // the wrapping ones, both bools, an integral and a fractional float, the
    // deprecated null, two strings (one of which LOOKS like a status and is not),
    // and the array that is a TypeError.
    let arg = *r.pick(&[
        "",
        "()",
        "(0)",
        "(3)",
        "(255)",
        "(256)",
        "(300)",
        "(-1)",
        "(true)",
        "(false)",
        "(2.0)",
        "(2.9)",
        "(null)",
        "(\"bye\")",
        "(\"7\")",
        "([1])",
    ]);
    match r.below(4) {
        0 => vec![format!("echo \"[\"; {word}{arg}; echo \"]\";")],
        1 => vec![format!(
            "function f() {{ echo \"in\"; {word}{arg}; echo \"after\"; }} \
             echo \"[\"; f(); echo \"]\";"
        )],
        // Through a `try`: no `catch` may claim the unwind and no `finally` may
        // run after it — except for the `([1])` argument, which is an ordinary
        // TypeError and therefore IS catchable, so the arm scores both answers.
        2 => vec![format!(
            "echo \"[\"; try {{ {word}{arg}; }} catch (Throwable $e) {{ echo \"C\"; }} \
             finally {{ echo \"F\"; }} echo \"]\";"
        )],
        // Out of a library callback, with an output buffer open that must still
        // be flushed on the way out.
        _ => vec![format!(
            "ob_start(); echo \"[\"; \
             array_map(function ($x) {{ if ($x == 2) {{ {word}{arg}; }} echo $x; }}, [1, 2, 3]); \
             echo \"]\";"
        )],
    }
}

// ---------------------------------------------------------------------------
// Harder generators — compound programs stressing precedence, coercion, and
// stdlib edge cases where a scaffold is most likely to disagree with real PHP.
// ---------------------------------------------------------------------------

/// A numeric leaf (int or float literal) for the precedence tree.
fn num_leaf<'a>(r: &mut Rng) -> &'a str {
    if r.below(3) == 0 {
        ff(r)
    } else {
        ii(r)
    }
}

/// An UNPARENTHESIZED flat sequence of numeric operands joined by mixed-precedence
/// operators — no parens, so the two implementations must agree on precedence and
/// associativity to produce the same value. Operands stay numeric so PHP 8's
/// non-numeric-string TypeError never enters (that is a separate mode).
fn gen_exprtree(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    // Arithmetic-only chain — no `/`/`%` (div-by-zero would just make both sides
    // error and agree) and NO chained comparisons (PHP 8 comparison operators are
    // non-associative: `1 < 2 < 3` is a fatal parse error, so a chain would only
    // test that we also reject invalid PHP, not real semantics).
    let ops = ["+", "-", "*"];
    let arith = |r: &mut Rng| {
        let n = 2 + r.below(4); // 2..=5 operands
        let mut e = num_leaf(r).to_string();
        for _ in 1..n {
            e = format!("{e} {} {}", r.pick(&ops), num_leaf(r));
        }
        e
    };
    // Optionally cap the arithmetic with a single top-level comparison.
    if r.below(2) == 0 {
        let cmp = ["<", ">", "<=", ">=", "==", "!="];
        vec![format!(
            "var_dump({} {} {});",
            arith(r),
            r.pick(&cmp),
            arith(r)
        )]
    } else {
        vec![format!("echo {};", arith(r))]
    }
}

/// Unary-operator stress: stacked `-`/`!`, mixed with `**` (which must bind
/// tighter than unary minus) and parenthesised sub-expressions.
fn gen_unary(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    match r.below(6) {
        0 => vec![format!("echo - - {};", ii(r))],
        1 => vec![format!(
            "echo -{} ** {};",
            r.pick(&["2", "3", "4"]),
            r.pick(&["2", "3"])
        )],
        2 => vec![format!(
            "echo !!{};",
            r.pick(&["0", "1", "5", "\"\"", "\"a\""])
        )],
        3 => vec![format!("echo -{} * -{};", ii(r), ii(r))],
        4 => vec![format!("echo {} - -{};", ii(r), ii(r))],
        _ => vec![format!("var_dump(!({} > {}));", ii(r), ii(r))],
    }
}

const SPRINTF_SPECS: &[&str] = &[
    "%d", "%5d", "%-5d", "%05d", "%+d", "%x", "%X", "%o", "%b", "%c", "%e", "%.2f", "%8.3f",
    "%-8.2f", "%+.1f", "%s", "%10s", "%-10s", "%%", "%'*8d", "%1\\$d", "%g",
];

/// `sprintf`/`printf` with width, precision, flags, and the full conversion set —
/// stresses the format engine well past the plain `%s %d %f` the tests cover.
fn gen_sprintf_rich(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let spec = r.pick(SPRINTF_SPECS);
    let arg = if spec.contains('f') || spec.contains('e') || spec.contains('g') {
        ff(r)
    } else if spec.contains('s') {
        return vec![format!("echo sprintf(\"[{spec}]\", \"{}\");", ww(r))];
    } else if spec.contains('c') {
        return vec![format!("echo sprintf(\"{spec}\", {});", 65 + r.below(26))];
    } else if *spec == "%%" {
        return vec!["echo sprintf(\"100%%\");".to_string()];
    } else {
        ii(r)
    };
    vec![format!("echo sprintf(\"[{spec}]\", {arg});")]
}

/// Number-formatting edge cases: values needing 14-digit precision, integer
/// overflow into float, scientific notation, very small/large magnitudes.
fn gen_numedge(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let vals = [
        "0.1 + 0.2",
        "1 / 3",
        "2 / 3",
        "10 / 7",
        "9223372036854775807 + 1",
        "9223372036854775807 * 2",
        "1.0e100",
        "1.5e-10",
        "0.0001",
        "123456789012345",
        "1234567890.12345",
        "1e20",
        "-0.0",
        "100000000000000.0",
        "3.0 / 2.0",
        "7 % 3",
        "-7 % 3",
        "7 % -3",
        "2 ** 63",
        "2 ** 64",
    ];
    vec![format!("echo {};", r.pick(&vals))]
}

/// String-function edge cases: negative offsets, pad types, replace, case ops.
fn gen_stredge(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let w = ww(r);
    match r.below(9) {
        0 => vec![format!("echo substr(\"{w}\", -{});", 1 + r.below(3))],
        1 => vec![format!(
            "echo substr(\"hello\", {}, -{});",
            r.below(2),
            1 + r.below(2)
        )],
        2 => vec![format!(
            "echo str_pad(\"{w}\", {}, \"ab\", {});",
            6 + r.below(3),
            r.below(3)
        )],
        3 => vec![format!(
            "echo str_replace(\"{}\", \"X\", \"{w}{w}\");",
            &w[..1]
        )],
        4 => vec![format!("echo ucwords(\"{} {}\");", ww(r), ww(r))],
        5 => vec![format!("echo strrev(\"{w}\");")],
        6 => vec![format!(
            "echo wordwrap(\"{w} {w} {w}\", {}, \"\\n\", true);",
            4 + r.below(6)
        )],
        7 => vec![format!("echo str_repeat(\"{}\", {});", &w[..1], r.below(6))],
        _ => vec![format!("var_dump(strpos(\"{w}\", \"{}\"));", &w[..1])],
    }
}

/// Array pipelines: map/filter/reduce with named callbacks, slice with negatives,
/// merge, unique — the compositional core of everyday PHP.
fn gen_arraypipe(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let arr = format!("[{}, {}, {}, {}, {}]", ii(r), ii(r), ii(r), ii(r), ii(r));
    match r.below(8) {
        0 => vec![format!(
            "echo implode(\",\", array_slice({arr}, {}, {}));",
            r.below(3),
            1 + r.below(3)
        )],
        1 => vec![format!(
            "echo implode(\",\", array_slice({arr}, -{}));",
            1 + r.below(3)
        )],
        2 => vec![format!("echo array_sum(array_map(\"abs\", {arr}));")],
        3 => vec![format!("echo implode(\",\", array_merge([1, 2], {arr}));")],
        4 => vec!["echo implode(\",\", array_unique([1, 1, 2, 2, 3]));".to_string()],
        5 => vec![format!("echo count(array_filter({arr}));")],
        6 => vec![format!(
            "function dbl($x) {{ return $x * 2; }} echo implode(\",\", array_map(\"dbl\", {arr}));"
        )],
        _ => vec![format!(
            "echo implode(\",\", array_reverse(array_slice({arr}, 1)));"
        )],
    }
}

/// Multi-statement accumulation programs: build state across statements, then
/// print a deterministic summary.
fn gen_multi(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    match r.below(5) {
        0 => vec![
            "$a = [];".into(),
            format!(
                "for ($i = 0; $i < {}; $i++) {{ $a[] = $i * $i; }}",
                3 + r.below(4)
            ),
            "echo implode(\",\", $a);".into(),
        ],
        1 => vec![
            format!("$s = \"\"; $n = {};", 3 + r.below(4)),
            "for ($i = 1; $i <= $n; $i++) { $s .= $i; }".into(),
            "echo $s;".into(),
        ],
        2 => vec![
            format!("$m = [\"a\" => {}, \"b\" => {}];", ii(r), ii(r)),
            "$m[\"c\"] = $m[\"a\"] + $m[\"b\"];".into(),
            "echo $m[\"c\"];".into(),
        ],
        3 => vec![
            format!("$x = {};", ii(r)),
            format!("$x += {}; $x *= 2; $x -= {};", ii(r), ii(r)),
            "echo $x;".into(),
        ],
        _ => vec![
            format!(
                "$t = 0; foreach ([{}, {}, {}] as $v) {{ if ($v % 2 == 0) {{ $t += $v; }} }}",
                ii(r),
                ii(r),
                ii(r)
            ),
            "echo $t;".into(),
        ],
    }
}

/// Bitwise operators (`& | ^ << >> ~`) and their compound assignments, mixed
/// unparenthesized with arithmetic to stress the precedence ladder.
fn gen_bitwise(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    match r.below(9) {
        0 => vec![format!("echo {} & {};", ii(r), ii(r))],
        1 => vec![format!("echo {} | {};", ii(r), ii(r))],
        2 => vec![format!("echo {} ^ {};", ii(r), ii(r))],
        3 => vec![format!("echo {} << {};", ii(r), r.below(40))],
        4 => vec![format!("echo {} >> {};", ii(r), r.below(40))],
        5 => vec![format!("echo ~{};", ii(r))],
        6 => vec![format!(
            "echo {} & {} + {} | {};",
            ii(r),
            ii(r),
            ii(r),
            ii(r)
        )],
        7 => vec![format!(
            "echo {} << {} + {} ^ {};",
            ii(r),
            r.below(4),
            ii(r),
            ii(r)
        )],
        _ => vec![format!(
            "$n = {}; $n &= {}; $n |= {}; $n ^= {}; $n <<= {}; echo $n;",
            ii(r),
            ii(r),
            ii(r),
            ii(r),
            r.below(4)
        )],
    }
}

/// Spaceship `<=>` across numeric and string operands.
fn gen_spaceship(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    match r.below(3) {
        0 => vec![format!("echo {} <=> {};", ii(r), ii(r))],
        1 => vec![format!("echo {} <=> {};", ff(r), ff(r))],
        _ => vec![format!("echo \"{}\" <=> \"{}\";", ww(r), ww(r))],
    }
}

/// String offset access, including PHP 7.1 negative offsets and `isset` on both
/// in- and out-of-range offsets.
fn gen_stroffset(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let w = ww(r);
    match r.below(4) {
        0 => vec![format!("echo \"{w}\"[{}];", r.below(5))],
        1 => vec![format!("$s = \"{w}\"; echo $s[-{}];", 1 + r.below(3))],
        2 => vec![format!(
            "$s = \"{w}\"; var_dump(isset($s[{}]));",
            r.below(8)
        )],
        _ => vec![format!("$s = \"{w}\"; echo $s[0], $s[strlen($s) - 1];")],
    }
}

/// Null-coalesce `??`, coalesce-assign `??=`, and elvis `?:` on the values PHP
/// treats specially (null, "", "0", 0).
fn gen_coalesce(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    match r.below(4) {
        0 => vec![format!(
            "$x = {}; echo $x ?? \"def\";",
            r.pick(&["null", "5", "\"\"", "0", "\"hi\""])
        )],
        1 => vec![format!(
            "$x = {}; $x ??= 7; echo $x;",
            r.pick(&["null", "3", "0"])
        )],
        2 => {
            vec!["$a = ['k' => 1]; echo $a['missing'] ?? 'miss', '|', $a['k'] ?? 'no';".to_string()]
        }
        _ => vec![format!(
            "echo {} ?: \"z\";",
            r.pick(&["0", "5", "\"\"", "\"hi\"", "null"])
        )],
    }
}

// ---------------------------------------------------------------------------
// Extended generators: the standard-library functions and language features
// added after the original corpus. Only functions that should match PHP 8
// byte-for-byte are exercised (documented deviations are excluded).
// ---------------------------------------------------------------------------

/// A small pool of quote-safe words for string tests.
const SW: &[&str] = &[
    "hello",
    "World",
    "abcABC",
    "FooBar",
    "mixedCase",
    "aaabbb",
    "level",
];

fn gen_str2(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let s = *r.pick(SW);
    match r.below(16) {
        0 => vec![format!("echo substr_count(\"{s}xx{s}\", \"{s}\");")],
        1 => vec![format!("echo ucwords(\"{s} and {s}\");")],
        2 => vec![format!("echo lcfirst(\"{s}\");")],
        3 => vec!["echo str_word_count(\"one two three four\");".to_string()],
        4 => vec![format!("echo strrpos(\"{s}{s}\", \"{}\");", &s[..1])],
        5 => vec![format!(
            "echo stripos(\"{s}\", \"{}\");",
            &s[..1].to_uppercase()
        )],
        6 => vec!["echo addslashes(\"a'b\\\"c\");".to_string()],
        7 => vec![format!("echo strtr(\"{s}\", \"lo\", \"LO\");")],
        8 => vec![format!(
            "echo wordwrap(\"the quick brown fox\", {}, \"|\", true);",
            4 + r.below(8)
        )],
        9 => vec![format!(
            "echo strncasecmp(\"{s}\", \"{}\", {}) <=> 0;",
            s.to_uppercase(),
            1 + r.below(5)
        )],
        10 => vec![format!(
            "echo str_ireplace(\"{}\", \"X\", \"{s}\");",
            &s[..1].to_uppercase()
        )],
        11 => vec![format!("echo strpbrk(\"{s}\", \"lo\");")],
        12 => vec![format!("echo strspn(\"{s}\", \"{}\");", s)],
        13 => vec![format!(
            "echo levenshtein(\"{s}\", \"{}x\");",
            &s[..s.len().saturating_sub(1)]
        )],
        14 => vec!["echo nl2br(\"a\\nb\");".to_string()],
        _ => vec!["echo quotemeta(\"a.b*c\");".to_string()],
    }
}

fn gen_arr2(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let mut list = || {
        let n = 3 + r.below(4);
        (0..n)
            .map(|_| (r.below(20)).to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let l = list();
    match r.below(15) {
        0 => vec![format!(
            "echo json_encode(array_chunk([{l}], {}));",
            1 + r.below(3)
        )],
        1 => vec![format!(
            "echo json_encode(array_pad([{l}], {}, 0));",
            1 + r.below(6)
        )],
        2 => vec![format!(
            "echo json_encode(array_slice([{l}], {}, {}));",
            r.below(3),
            1 + r.below(3)
        )],
        3 => vec![format!(
            "echo json_encode(array_count_values([{}]));",
            "1, 2, 2, 3, 3, 3"
        )],
        4 => vec![format!(
            "echo json_encode(array_flip([{}]));",
            "\"a\", \"b\", \"c\""
        )],
        5 => vec![format!(
            "echo array_key_first([{l}]), \"|\", array_key_last([{l}]);"
        )],
        6 => vec![format!("echo array_is_list([{l}]) ? \"y\" : \"n\";")],
        7 => vec![format!(
            "echo in_array({}, [{l}]) ? \"y\" : \"n\";",
            r.below(20)
        )],
        8 => vec![format!(
            "$a = [{l}]; echo array_search({}, $a) === false ? \"no\" : \"yes\";",
            r.below(20)
        )],
        9 => vec![format!("$a = [{l}]; sort($a); echo implode(\",\", $a);")],
        10 => vec![format!("$a = [{l}]; rsort($a); echo implode(\",\", $a);")],
        11 => vec![format!(
            "$a = [{l}]; usort($a, fn($x, $y) => $x - $y); echo implode(\",\", $a);"
        )],
        12 => vec![format!(
            "echo json_encode(array_fill_keys([\"a\", \"b\"], {}));",
            r.below(9)
        )],
        13 => vec![
            "echo json_encode(array_diff_key([\"a\" => 1, \"b\" => 2], [\"a\" => 9]));".to_string(),
        ],
        _ => vec![format!(
            "$a = [{l}]; echo array_sum($a), \"|\", array_product([1, 2, 3]);"
        )],
    }
}

fn gen_math2(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let n = r.below(1000);
    let neg = r.below(200) as i64 - 100;
    match r.below(16) {
        0 => vec![format!("echo dechex({n}), \"|\", hexdec(dechex({n}));")],
        1 => vec![format!("echo decbin({n}), \"|\", bindec(decbin({n}));")],
        2 => vec![format!("echo decoct({n}), \"|\", octdec(decoct({n}));")],
        3 => vec![format!("echo base_convert(\"{n}\", 10, 16);")],
        4 => vec![format!("echo abs({neg}), \"|\", abs({neg}.5);")],
        5 => vec![format!(
            "echo intdiv({}, {});",
            n as i64 - 500,
            1 + r.below(9)
        )],
        6 => vec![format!(
            "echo {} % {};",
            n as i64 - 500,
            (r.below(9) as i64) - 4
        )],
        7 => vec![format!(
            "echo max({n}, {neg}, {}), \"|\", min({n}, {neg}, {});",
            r.below(1000),
            r.below(1000)
        )],
        8 => vec![format!(
            "printf(\"%.4f\", fmod({}, {}));",
            n,
            1 + r.below(7)
        )],
        9 => vec![format!(
            "echo gmp_strval(gmp_add(\"{n}00000000000000000000\", \"{}\"));",
            r.below(1000)
        )],
        10 => vec![format!(
            "echo gmp_strval(gmp_mul(\"{n}\", \"{}\"));",
            r.below(100000)
        )],
        11 => vec![format!(
            "echo gmp_strval(gmp_pow(\"{}\", {}));",
            2 + r.below(5),
            5 + r.below(20)
        )],
        12 => vec![format!(
            "echo gmp_strval(gmp_gcd(\"{}\", \"{}\"));",
            n * 6,
            n.max(1) * 4
        )],
        13 => vec![format!(
            "echo gmp_strval(gmp_mod(\"{}\", \"{}\"));",
            n,
            1 + r.below(97)
        )],
        14 => vec![format!(
            "echo str_pad(strval(round({n}.{:03}, 2)), 1);",
            r.below(1000)
        )],
        _ => vec![format!("echo ({} <=> {});", n, r.below(1000))],
    }
}

fn gen_refs(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let x = r.below(50);
    let y = r.below(50);
    // `$b = &$a[0]` (a reference to an array ELEMENT rather than to a whole
    // variable) was suppressed here as an unsupported form. It is supported —
    // the note outlived the limitation it described, and while it stood no case
    // could reach the form to show that. Generated below.
    match r.below(8) {
        0 => vec![format!("$a = {x}; $b = &$a; $b = {y}; echo $a, \"|\", $b;")],
        1 => vec![format!("$a = {x}; $b = &$a; $a += {y}; echo $b;")],
        2 => vec![format!(
            "$a = [{}, {}, {}]; foreach ($a as &$v) {{ $v *= 2; }} unset($v); echo implode(\",\", $a);",
            x, y, r.below(50)
        )],
        3 => vec![format!(
            "function inc(&$n) {{ $n++; }} $c = {x}; inc($c); inc($c); echo $c;"
        )],
        4 => vec![format!(
            "function swap(&$p, &$q) {{ $t = $p; $p = $q; $q = $t; }} $a = {x}; $b = {y}; swap($a, $b); echo $a, \"|\", $b;"
        )],
        // A reference to an array element by INDEX: the write must land in the
        // array, not in a detached copy of the element.
        5 => vec![format!(
            "$a = [{x}, {y}]; $b = &$a[0]; $b = {}; echo implode(',', $a);",
            r.below(50)
        )],
        // The same by STRING KEY.
        6 => vec![format!(
            "$a = ['k' => {x}, 'j' => {y}]; $b = &$a['k']; $b = {}; echo json_encode($a);",
            r.below(50)
        )],
        // A NESTED element, where the chain has to be walked before the bind.
        _ => vec![format!(
            "$a = [[{x}], [{y}]]; $b = &$a[0][0]; $b = {}; echo json_encode($a);",
            r.below(50)
        )],
    }
}

fn gen_closures(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let n = r.below(20);
    let list = format!(
        "[{}, {}, {}, {}]",
        r.below(20),
        r.below(20),
        r.below(20),
        r.below(20)
    );
    match r.below(7) {
        0 => vec![format!(
            "$f = fn($x) => $x * {n}; echo $f({});",
            r.below(10)
        )],
        1 => vec![format!(
            "$b = {n}; $f = function ($x) use ($b) {{ return $x + $b; }}; echo $f({});",
            r.below(10)
        )],
        2 => vec![format!(
            "echo implode(\",\", array_map(fn($x) => $x + 1, {list}));"
        )],
        3 => vec![format!(
            "echo implode(\",\", array_filter({list}, fn($x) => $x % 2 == 0));"
        )],
        4 => vec![format!(
            "echo array_reduce({list}, fn($c, $x) => $c + $x, 0);"
        )],
        5 => vec![format!(
            "$mk = fn($b) => fn($x) => $x + $b; $add = $mk({n}); echo $add({});",
            r.below(10)
        )],
        _ => vec![format!("echo (fn($x) => $x <=> {n})({});", r.below(20))],
    }
}

fn gen_exc(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    match r.below(6) {
        0 => vec![
            "try { throw new Exception(\"e\"); } catch (Exception $x) { echo $x->getMessage(); }".into(),
        ],
        1 => vec!["try { echo \"a\"; } finally { echo \"b\"; }".into()],
        2 => vec![
            "function g() { try { throw new RuntimeException(\"x\"); } catch (Exception $e) { return \"R\"; } finally { echo \"F\"; } } echo g();".into(),
        ],
        3 => vec![
            "try { throw new TypeError(\"t\"); } catch (Exception $e) { echo \"exc\"; } catch (Error $e) { echo \"err\"; }".into(),
        ],
        4 => vec![
            "try { echo match({}) { 1 => \"a\" }; } catch (\\UnhandledMatchError $e) { echo \"unhandled\"; }"
                .replace("{}", &(2 + r.below(5)).to_string()),
        ],
        _ => vec![
            "$v = null; try { $r = $v ?? throw new Exception(\"c\"); } catch (Exception $e) { echo $e->getMessage(); }".into(),
        ],
    }
}

fn gen_typejug2(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let vals = [
        "0",
        "1",
        "-5",
        "3.14",
        "\"42\"",
        "\"3.5abc\"",
        "\"\"",
        "\"0\"",
        "true",
        "false",
        "null",
        "\"10\"",
    ];
    // COMPOSITE subjects. Every printer below draws its subject from `vals`,
    // which holds scalars only, so each one rendered a single-line result and
    // the multi-line rules — where `var_export` breaks the line BEFORE a nested
    // `array (`, where `print_r`/`var_dump` indent a child block, how an object
    // body differs from an array body — were unreachable no matter how many
    // cases ran. These are drawn from a separate pool so the scalar branches
    // keep testing exactly what they tested before.
    let comps = [
        "[1, [2, 3]]",
        "[[1, 2], [3, 4]]",
        "['a' => 1, 'b' => [2, 3]]",
        "[1, ['k' => ['z' => 2]]]",
        "[]",
        "[[]]",
        "[1.0, [2.5]]",
        "[true, [false, null]]",
        "['it\\'s', ['a\\\\b']]",
        "(object)['x' => 1]",
        "(object)['x' => [1, 2]]",
        "['k' => (object)['n' => [5]]]",
        "[1, [2, [3, [4]]]]",
    ];
    let v = *r.pick(&vals);
    let c = *r.pick(&comps);
    match r.below(15) {
        0 => vec![format!("var_dump((int){v});")],
        1 => vec![format!("var_dump((float){v});")],
        2 => vec![format!("var_dump((bool){v});")],
        3 => vec![format!("echo gettype({v});")],
        4 => vec![format!("var_dump(is_numeric({v}));")],
        5 => vec![format!("echo intval({v});")],
        6 => vec![format!("echo var_export({v}, true);")],
        7 => vec![format!("echo json_encode({v});")],
        8 => vec![format!("var_dump({v} == {});", r.pick(&vals))],
        // The nesting-sensitive printers, on a subject that actually nests.
        9 => vec![format!("echo var_export({c}, true);")],
        // The `$return = false` spelling, which writes straight to stdout
        // rather than through a returned string.
        10 => vec![format!("var_export({c});")],
        11 => vec![format!("var_dump({c});")],
        12 => vec![format!("echo json_encode({c});")],
        13 => vec![format!("echo print_r({c}, true);")],
        _ => vec![format!("echo serialize({c});")],
    }
}

fn gen_range(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    match r.below(6) {
        0 => {
            let (a, b, s) = (r.below(10), 10 + r.below(20), 1 + r.below(4));
            vec![format!("echo implode(\",\", range({a}, {b}, {s}));")]
        }
        1 => {
            let (a, b, s) = (20 + r.below(20), r.below(10), 1 + r.below(4));
            vec![format!("echo implode(\",\", range({a}, {b}, {s}));")]
        }
        2 => {
            let lo = (b'a' + r.below(10) as u8) as char;
            let hi = (b'p' + r.below(10) as u8) as char;
            vec![format!(
                "echo implode(\",\", range(\"{lo}\", \"{hi}\", {}));",
                1 + r.below(3)
            )]
        }
        3 => vec![format!(
            "echo implode(\",\", range({}, {}));",
            (r.below(20) as i64) - 10,
            r.below(20)
        )],
        4 => vec![format!(
            "echo count(range({}, {}, {}));",
            r.below(5),
            50 + r.below(50),
            1 + r.below(7)
        )],
        _ => {
            let lo = (b'A' + r.below(6) as u8) as char;
            let hi = (b'T' + r.below(6) as u8) as char;
            vec![format!("echo implode(\"\", range(\"{lo}\", \"{hi}\"));")]
        }
    }
}

fn gen_datefmt(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let tss = [
        0i64, 946684800, 1234567890, 1600000000, 1700000000, 1000000, 1609459200,
    ];
    let ts = *r.pick(&tss);
    let fmts = [
        "Y-m-d H:i:s",
        "D, d M Y",
        "l N w z",
        "H:i:s A",
        "y/n/j",
        "W F t S",
        "U G a",
    ];
    let f = *r.pick(&fmts);
    match r.below(3) {
        0 => vec![format!("echo date(\"{f}\", {ts});")],
        1 => vec![format!("echo gmdate(\"{f}\", {ts});")],
        _ => vec![format!(
            "echo date(\"{f}\", {ts}), \"|\", date(\"{f}\", {});",
            ts + 86400 * (1 + r.below(400)) as i64
        )],
    }
}

fn gen_printf2(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let n = (r.below(2000) as i64) - 1000;
    let f = r.below(100000) as f64 / 100.0;
    match r.below(9) {
        0 => vec![format!(
            "printf(\"%b|%o|%x|%X\", {}, {}, {}, {});",
            n.unsigned_abs(),
            n.unsigned_abs(),
            n.unsigned_abs(),
            n.unsigned_abs()
        )],
        1 => vec![format!("printf(\"%e|%E\", {f}, {f});")],
        2 => vec![format!("printf(\"%+d|%+d\", {}, {});", n.abs(), -n.abs())],
        3 => vec![format!(
            "printf(\"%0{}.{}f\", {f});",
            4 + r.below(6),
            r.below(5)
        )],
        4 => vec![format!(
            "printf(\"%'*{}s\", \"{}\");",
            4 + r.below(8),
            r.pick(SW)
        )],
        // Single-quoted PHP string: the `$s`/`$` must NOT be interpolated.
        5 => vec![format!(
            "printf('%1$s %2$s %1$s', \"{}\", \"{}\");",
            r.pick(SW),
            r.pick(SW)
        )],
        6 => vec![format!("printf(\"%-{}d|\", {});", 4 + r.below(6), n)],
        7 => vec![format!(
            "echo number_format({}.{:03}, {}, \".\", \",\");",
            r.below(9999999),
            r.below(1000),
            r.below(4)
        )],
        _ => vec![format!("printf(\"%g|%G\", {}, {});", f, f * 1000.0)],
    }
}

fn gen_arr3(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let list = |r: &mut Rng| {
        let n = 3 + r.below(4);
        (0..n)
            .map(|_| (r.below(15)).to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let three = |r: &mut Rng| format!("{}, {}, {}", r.below(15), r.below(15), r.below(15));
    let l = list(r);
    match r.below(11) {
        0 => vec![format!("echo implode(\",\", array_reverse([{l}]));")],
        1 => vec![format!(
            "echo implode(\",\", array_unique([{}]));",
            "1, 2, 2, 3, 1, 4, 4"
        )],
        // array_combine requires equal-length key/value lists (else PHP throws).
        2 => vec![format!(
            "echo json_encode(array_combine([\"a\", \"b\", \"c\"], [{}]));",
            three(r)
        )],
        3 => vec![format!(
            "echo implode(\",\", array_merge([{l}], [{}]));",
            list(r)
        )],
        4 => vec![format!("echo implode(\",\", array_keys([{l}]));")],
        5 => vec![format!("echo implode(\",\", array_values([{l}]));")],
        6 => vec![format!(
            "echo implode(\",\", array_map(fn($x) => $x * $x, [{l}]));"
        )],
        7 => vec![format!(
            "$a = [{l}]; echo implode(\",\", array_map(null, $a, $a)[0]);"
        )],
        8 => vec![format!(
            "echo array_sum(array_map(fn($x) => $x + 1, [{l}]));"
        )],
        9 => vec![format!(
            "echo implode(\",\", array_intersect([{l}], [{}]));",
            list(r)
        )],
        _ => vec![format!(
            "echo implode(\",\", array_diff([{l}], [{}]));",
            list(r)
        )],
    }
}

fn gen_rounding(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    // Bias toward `.xx5`-style half-way decimals where the nearest f64 sits just
    // below the boundary — the cases naive rounding gets wrong.
    let whole = r.below(100000);
    let frac = r.below(1000);
    let sign = if r.below(2) == 0 { "" } else { "-" };
    let places = r.below(4) as i64;
    let v = format!("{sign}{whole}.{frac:03}5");
    match r.below(6) {
        0 => vec![format!("echo round({v}, {places});")],
        1 => vec![format!("echo number_format({v}, {places});")],
        2 => vec![format!("echo number_format({v}, {places}, \".\", \",\");")],
        3 => vec![format!("echo round({sign}{whole}.{frac:03}, {places});")],
        4 => vec![format!("echo round({v}, {});", -(1 + r.below(3) as i64))],
        _ => vec![format!(
            "printf(\"%.{}f\", round({v}, {places}));",
            r.below(4)
        )],
    }
}

/// Dynamic-property creation: the PHP 8.2 `Deprecated` notice and the three ways
/// out of it (a declared property, `stdClass`, `#[AllowDynamicProperties]` —
/// which is inherited). Every case prints the property back, so a missing notice
/// and a wrong value both show up.
fn gen_dynprop(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let p = *r.pick(&["x", "y", "value", "n"]);
    let v = ii(r);
    match r.below(8) {
        0 => vec![format!(
            "class C {{}} $c = new C; $c->{p} = {v}; echo $c->{p};"
        )],
        1 => vec![format!(
            "class C {{ public ${p}; }} $c = new C; $c->{p} = {v}; echo $c->{p};"
        )],
        2 => vec![format!(
            "#[AllowDynamicProperties] class C {{}} $c = new C; $c->{p} = {v}; echo $c->{p};"
        )],
        3 => vec![format!(
            "#[AllowDynamicProperties] class P {{}} class C extends P {{}} \
             $c = new C; $c->{p} = {v}; echo $c->{p};"
        )],
        4 => vec![format!(
            "class P {{ public ${p} = 0; }} class C extends P {{}} \
             $c = new C; $c->{p} = {v}; echo $c->{p};"
        )],
        5 => vec![format!("$o = new stdClass; $o->{p} = {v}; echo $o->{p};")],
        6 => vec![format!(
            "class C {{}} $c = new C; $c->{p} = {v}; $c->{p} = {}; echo $c->{p};",
            ii(r)
        )],
        // Compound assignment and increment: the notice precedes the
        // undefined-property warning the read then raises.
        _ => vec![format!(
            "class C {{}} $c = new C; $c->{p} {}= {v}; echo $c->{p};",
            r.pick(&["+", ".", "*"])
        )],
    }
}

/// `#[Attr]` in every position a declaration can carry one. These change no
/// behaviour (except `AllowDynamicProperties`, covered above) — the point is that
/// the declaration still PARSES and runs, which it cannot if `#[` lexes as a
/// comment and swallows the rest of the line.
fn gen_attributes(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let n = ii(r);
    match r.below(6) {
        0 => vec![format!("#[Attr] class C {{ public $v = {n}; }} echo (new C)->v;")],
        1 => vec![format!(
            "class C {{ #[Attr] public $v = {n}; #[Attr] public function m() {{ return $this->v; }} }} \
             echo (new C)->m();"
        )],
        2 => vec![format!(
            "#[Attr(1, [2, 3])] function f(#[Attr] $x) {{ return $x; }} echo f({n});"
        )],
        3 => vec![format!(
            "#[A] #[B] class C {{}} #[C] class D {{ const K = {n}; }} echo D::K;"
        )],
        4 => vec!["enum E { #[Attr] case A; #[Attr] case B; } echo E::A->name, count(E::cases());".to_string()],
        _ => vec![format!(
            "#[\\Ns\\Attr] interface I {{}} #[Attr] class C implements I {{}} \
             echo (new C) instanceof I ? \"y{n}\" : \"n{n}\";"
        )],
    }
}

/// The `error_reporting` mask: what it returns, what it suppresses, and that a
/// COMPILE-time notice (`${var}`) is decided before any of it runs.
fn gen_errlevel(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let lvl = *r.pick(&[
        "0",
        "E_ALL",
        "E_ALL & ~E_WARNING",
        "E_ALL & ~E_DEPRECATED",
        "E_ERROR",
        "E_WARNING | E_DEPRECATED",
    ]);
    match r.below(6) {
        0 => vec!["echo error_reporting();".to_string()],
        1 => vec![format!(
            "var_dump(error_reporting({lvl})); echo error_reporting();"
        )],
        2 => vec![format!(
            "error_reporting({lvl}); echo $undef; echo \"end\";"
        )],
        3 => vec![format!(
            "error_reporting({lvl}); class C {{}} $c = new C; $c->p = 1; echo \"end\";"
        )],
        4 => vec![
            "var_dump(ini_get(\"error_reporting\")); ini_set(\"error_reporting\", \"0\"); \
             echo $undef; var_dump(ini_get(\"error_reporting\"));"
                .to_string(),
        ],
        // The `${var}` notice is raised while READING the source, so it prints
        // before the `echo` on the line before it and survives `error_reporting(0)`.
        _ => vec![format!(
            "echo \"a\"; error_reporting({lvl}); $v = {}; echo \"${{v}}\";",
            ii(r)
        )],
    }
}

/// Library argument errors: a standard-library function given arguments it
/// rejects throws a catchable exception whose `#0` trace frame is the library
/// call itself. Half the cases catch it (class + message + line), half let it
/// reach the top (the whole uncaught rendering, trace included).
fn gen_libargerr(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let call = *r.pick(&[
        "range(9, 10, 2)",
        "range(1, 2, 5)",
        "range('a', 'b', 9)",
        "array_chunk([1, 2], 0)",
        "array_combine([1, 2], [1])",
        "str_repeat('ab', -1)",
        "substr_count('aa', '')",
        "chunk_split('aa', 0)",
        "mb_str_split('ab', 0)",
        "mb_substr_count('aa', '')",
        "hash('nope', 'x')",
        "hash_hmac('nope', 'x', 'k')",
        "random_bytes(0)",
        "bcdiv('1', '0')",
        "bcmod('1', '0')",
        "bcsqrt('-1')",
        "gmp_div_q('1', '0')",
        "gmp_mod('1', '0')",
    ]);
    match r.below(4) {
        0 => vec![format!(
            "try {{ {call}; }} catch (Throwable $e) {{ \
             echo get_class($e), \"|\", $e->getMessage(); }}"
        )],
        1 => vec![format!(
            "try {{ {call}; }} catch (Throwable $e) {{ \
             echo $e->getLine(), \"|\", $e->getTraceAsString(); }}"
        )],
        // Uncaught, one frame down, so the trace has a user frame under the
        // internal one.
        2 => vec![format!("function f() {{ return {call}; }} f();")],
        _ => vec![format!("{call};")],
    }
}

/// Patterns the reference REJECTS, each with the fault it reports: an empty
/// expression, a delimiter that may not be one, an unknown modifier, an
/// unterminated delimiter of both styles, and the five PCRE2 body faults.
const BAD_PATTERNS: &[&str] = &[
    "''",
    "'   '",
    "'abc'",
    "'1a1'",
    "'/a/Z'",
    "'/a/gg'",
    "'/abc'",
    "'{abc'",
    "'(a(b)'",
    "'/[a'",
    "'/ab[cd'",
    "'/(a'",
    "'/((a)'",
    "'/a)'",
    "'/(a))'",
    "'/*a'.'/'",
    "'/a**'.'/'",
    "'/a|*'.'/'",
    "'/a{2,1}/'",
    "'/ab{5,3}c/'",
    "'/{2}/'",
    "'/x{1,2}{3}/'",
    "'/(?'.'/'",
];

/// Patterns the reference ACCEPTS, including the ones a careless delimiter or
/// quantifier scan gets wrong: an escaped delimiter, nested bracket delimiters,
/// a literal `{`, an open lower bound, and every modifier letter that is legal.
const GOOD_PATTERNS: &[&str] = &[
    "'/a/'",
    "'//'",
    "'/a\\\\//'",
    "'/a\\\\\\\\/'",
    "'{a}'",
    "'(a)'",
    "'[a]'",
    "'<a>'",
    "'#a#'",
    "'~a~'",
    "'/a{x}/'",
    "'/a{,3}/'",
    "'/a{2,}/'",
    "'/[a)]/'",
    "'/[]a]/'",
    "'/\\\\(/'",
    "'/a*?/'",
    "'/a*+/'",
    "'/(a)(b)/'",
    "'/a/imsxuADJSUX'",
    "'/A/i'",
    "'/^ab$/'",
    "'/a|b/'",
];

/// `preg_*` pattern faults, and the error state they leave behind.
///
/// Every case is a SEQUENCE, not a single call: the interesting behaviour is that
/// `preg_last_error()` persists across calls and is cleared by the next pattern
/// that compiles — not by reading it, and not by `preg_quote`. A generator that
/// only ever emitted one call and one read would agree with a wrong
/// implementation that reset the state on every read, or never reset it at all.
/// So each program interleaves calls with reads and prints the state after each
/// step, and mixes good patterns in with bad ones so both transitions are seen.
fn gen_pregerr(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let bad = *r.pick(BAD_PATTERNS);
    let bad2 = *r.pick(BAD_PATTERNS);
    let good = *r.pick(GOOD_PATTERNS);
    let subj = ww(r);
    // The state read, printed after every step.
    let st = r#"echo "|", preg_last_error(), ":", preg_last_error_msg(), "|";"#;
    match r.below(8) {
        // One fault, then the reads that must NOT clear it, then a good pattern
        // that must.
        0 => vec![format!(
            "var_dump(preg_match({bad}, '{subj}')); {st} {st} preg_quote('a'); {st} \
             var_dump(preg_match({good}, '{subj}')); {st}"
        )],
        // Every function that compiles a pattern reports the fault under its own
        // name and returns its own error sentinel.
        1 => vec![format!(
            "var_dump(preg_match({bad}, '{subj}')); {st} \
             var_dump(preg_match_all({bad2}, '{subj}', $m)); {st} \
             var_dump(preg_replace({bad}, 'z', '{subj}')); {st} \
             var_dump(preg_split({bad2}, '{subj}')); {st} \
             var_dump(preg_grep({bad}, ['{subj}'])); {st}"
        )],
        // A good pattern clears the state even when the match itself finds
        // nothing — the compile is what counts, not the outcome.
        2 => vec![format!(
            "preg_match({bad}, '{subj}'); {st} \
             var_dump(preg_match('/zzzzz/', '{subj}')); {st} \
             var_dump(preg_match({good}, '{subj}')); {st}"
        )],
        // Good patterns must stay silent and keep the state clear.
        3 => vec![format!(
            "var_dump(preg_match({good}, '{subj}', $m), $m); {st} \
             var_dump(preg_split({good}, '{subj}')); {st}"
        )],
        // `@` and the error_reporting mask hide the warning; neither touches the
        // error state, which the reads after them prove.
        4 => vec![format!(
            "var_dump(@preg_match({bad}, '{subj}')); {st} \
             error_reporting(E_ALL & ~E_WARNING); \
             var_dump(preg_match({bad2}, '{subj}')); {st} \
             error_reporting(E_ALL); var_dump(preg_match({bad}, '{subj}')); {st}"
        )],
        // An array of patterns: the first bad one ends the whole call.
        5 => vec![format!(
            "var_dump(preg_replace([{good}, {bad}], 'z', '{subj}')); {st} \
             var_dump(preg_replace([{good}], 'z', '{subj}')); {st}"
        )],
        // Inside a function, so the warning's line is the call's and not the
        // caller's.
        6 => vec![format!(
            "function f($p) {{ return preg_match($p, '{subj}'); }} \
             var_dump(f({bad})); {st} var_dump(f({good})); {st}"
        )],
        // A fault in a loop: the state must track the LAST call every time round.
        _ => vec![format!(
            "foreach ([{bad}, {good}, {bad2}] as $p) {{ \
             var_dump(@preg_match($p, '{subj}')); {st} }}"
        )],
    }
}

/// Patterns built from the PCRE constructs the `regex` crate has no support
/// for, so every one of them exercises the second engine: look-ahead and
/// look-behind (positive and negative), backreferences (numbered and named),
/// named groups, atomic groups and possessive quantifiers. Each was confirmed to
/// COMPILE on the reference — a pattern PCRE itself rejects belongs in
/// [`BAD_PATTERNS`], not here.
///
/// Written as PHP SINGLE-quoted literals: `'\1'` is a backreference, while
/// `"\1"` would be the octal escape for byte 1 and would test nothing.
const FANCY_PATTERNS: &[&str] = &[
    // look-ahead / look-behind
    "'/foo(?=bar)/'",
    "'/foo(?!bar)/'",
    "'/(?<=a)b/'",
    "'/(?<!a)b/'",
    "'/(?<=\\d)[a-z]/'",
    "'/^(?=.*a)(?=.*1).{3,}$/'",
    "'/\\b(?!x)\\w+/'",
    "'/(?<=,)\\s*/'",
    "'/(?=[A-Z])/'",
    "'/(?<=\\w)(?=\\d)/'",
    "'/(\\d)(?=(\\d{3})+$)/'",
    // backreferences, numbered and named
    "'/(\\w)\\1/'",
    "'/(\\w+) \\1/'",
    "'/(a|b)c\\1/'",
    "'/(?<w>\\w+)-(?P=w)/'",
    // named groups — the `$matches` keying, not the matching
    "'/(?<k>[a-z]+)=(?<v>\\d+)/'",
    "'/(?<h>\\d\\d):(?<m>\\d\\d)/'",
    "'/(?P<x>a)(b)(?P<y>c)?/'",
    // atomic group / possessive quantifier
    "'/(?>a+)b/'",
    "'/a++b/'",
    "'/(?>\\w+)\\d/'",
    // constructs that also reach the second engine, with a modifier attached
    "'/x(?i)y/'",
    "'/(?:(a)|(b))\\2?/'",
    "'/a(?=b)/i'",
    "'/^(?=\\w)/m'",
    "'/(?<=a) b /x'",
    "'/<(.+)>(?=<)/U'",
    "'/a.(?=c)/s'",
    "'/a(?=a)/A'",
];

/// Subjects chosen so each pattern above has both a hit and a miss among them.
const FANCY_SUBJECTS: &[&str] = &[
    "foobar",
    "foobaz",
    "ab",
    "cb",
    "1a",
    "a1c",
    "xy why",
    "abba",
    "the the",
    "aca",
    "hi-hi",
    "k=12",
    "09:45",
    "abc",
    "aaab",
    "aaa1",
    "a, b",
    "fooBarBaz",
    "a1b2",
    "1234567",
    "xY",
    "<a><b>",
];

/// The PCRE constructs the first engine cannot compile, across every function
/// that takes a pattern.
///
/// This mode exists because the generator was BLIND to them: every pattern in
/// [`GOOD_PATTERNS`] and [`BAD_PATTERNS`] compiles on the `regex` crate or fails
/// on PCRE too, so no case ever reached the fallback engine and a run could
/// report zero divergences while look-around returned the error sentinel for
/// every program in the corpus.
///
/// `$matches` is printed with `var_export`, not `count`: the named-group KEYS
/// are half of what is under test, and a count agrees with an implementation
/// that dropped every one of them.
fn gen_pregfancy(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let p = *r.pick(FANCY_PATTERNS);
    let p2 = *r.pick(FANCY_PATTERNS);
    let s = *r.pick(FANCY_SUBJECTS);
    let s2 = *r.pick(FANCY_SUBJECTS);
    // The error state must stay clear: these patterns COMPILE on the reference,
    // so nothing may be recorded for them.
    let st = r#"echo "|", preg_last_error(), "|";"#;
    match r.below(8) {
        // The match itself, with the full `$matches` shape.
        0 => vec![format!(
            "var_dump(preg_match({p}, '{s}', $m), $m); {st} \
             var_dump(preg_match({p}, '{s2}', $m2), $m2); {st}"
        )],
        // Both orders of preg_match_all, which key the named groups differently
        // (outer keys in PATTERN_ORDER, inner keys in SET_ORDER).
        1 => vec![format!(
            "var_dump(preg_match_all({p}, '{s}', $m), $m); {st} \
             var_dump(preg_match_all({p}, '{s}', $n, PREG_SET_ORDER), $n); {st}"
        )],
        // The replacement templates, whose group substitution runs off the same
        // captures.
        2 => vec![format!(
            "var_dump(preg_replace({p}, 'X', '{s}')); {st} \
             var_dump(preg_replace({p}, '[$1]', '{s}')); {st} \
             var_dump(preg_replace({p}, '<\\1>', '{s}')); {st} \
             var_dump(preg_replace({p}, '${{0}}!', '{s}')); {st}"
        )],
        // The replace LIMIT, which walks the match list rather than taking it
        // whole.
        3 => vec![format!(
            "var_dump(preg_replace({p}, 'X', '{s}', 1)); {st} \
             var_dump(preg_replace({p}, 'X', '{s}', 2)); {st} \
             var_dump(preg_replace({p}, 'X', '{s}', 0)); {st}"
        )],
        // The callback form, which hands the keyed `$matches` to user code.
        4 => vec![format!(
            "var_dump(preg_replace_callback({p}, function($m) {{ return json_encode($m); }}, \
             '{s}')); {st}"
        )],
        // Splitting on a zero-width or look-around delimiter, with the flags
        // that change the piece list.
        5 => vec![format!(
            "var_dump(preg_split({p}, '{s}')); {st} \
             var_dump(preg_split({p}, '{s}', -1, PREG_SPLIT_NO_EMPTY)); {st} \
             var_dump(preg_split({p}, '{s}', -1, PREG_SPLIT_DELIM_CAPTURE)); {st} \
             var_dump(preg_split({p}, '{s}', 2)); {st}"
        )],
        // preg_grep, the only one that answers per element.
        6 => vec![format!(
            "var_dump(preg_grep({p}, ['{s}', '{s2}', ''])); {st} \
             var_dump(preg_grep({p}, ['{s}', '{s2}'], PREG_GREP_INVERT)); {st}"
        )],
        // Two different patterns in one program, so a compile that leaked state
        // from the previous one shows up.
        _ => vec![format!(
            "var_dump(preg_match({p}, '{s}', $a), $a); {st} \
             var_dump(preg_match({p2}, '{s2}', $b), $b); {st} \
             var_dump(preg_match({p}, '{s2}', $c), $c); {st}"
        )],
    }
}

/// Multibyte (pattern, subject) pairs for the byte-offset question.
///
/// Every pattern here is free of `.`, `^` and `$`. That is deliberate, not
/// incidental: a non-ASCII subject under a pattern containing those constructs
/// is a SEPARATE, already-escalated disagreement about what a "character" is,
/// and pulling it in here would make this mode fail for a reason that has
/// nothing to do with offsets. Anchor-free literal patterns ask only the
/// question this mode exists to ask.
///
/// Each subject is paired both with and without `/u`, because the answer is the
/// same either way — PHP reports a BYTE offset even when the subject is walked
/// as UTF-8 — and an implementation that counts codepoints under `/u` is wrong
/// in exactly the cases where the two spellings would otherwise agree.
const OFFSET_MB: &[(&str, &str)] = &[
    ("'/b/'", "éb"),
    ("'/b/u'", "éb"),
    ("'/(c)/'", "éàc"),
    ("'/(c)/u'", "éàc"),
    ("'/(?<g>x)/'", "日本x"),
    ("'/(?<g>x)/u'", "日本x"),
    ("'/z(q)?(w)/'", "€zw"),
    ("'/z(q)?(w)/u'", "€zw"),
];

/// `PREG_OFFSET_CAPTURE` across every function that honours it.
///
/// This mode exists because the generator was BLIND to the flag: a grep for
/// `PREG_OFFSET_CAPTURE` over this file returned ZERO hits before it was added,
/// so every previous "0 divergences" run scored the flag not at all. The engine
/// accepted the constant and ignored it, returning the bare captured string
/// where the reference returns a `[string, offset]` pair — a wrong answer that
/// no amount of running the old corpus could ever have surfaced.
///
/// Three properties are under test and each fails independently:
///   * the PAIR SHAPE — every captured slot becomes a two-element list;
///   * the OFFSET UNIT — byte, not codepoint, including under `/u` (see
///     [`OFFSET_MB`]);
///   * the NON-PARTICIPATING SENTINEL — a group that did not take part is
///     `['', -1]`, where the flagless form yields a bare `''`. An engine that
///     wrapped the flagless value verbatim would emit `['', 0]` and pass every
///     test that only checks the shape.
///
/// `var_export` is used rather than a count or an `implode`: the offsets ARE the
/// answer, and any summary agrees with an implementation that computed them all
/// wrongly.
fn gen_pregoffset(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let p = *r.pick(FANCY_PATTERNS);
    let s = *r.pick(FANCY_SUBJECTS);
    let s2 = *r.pick(FANCY_SUBJECTS);
    let (mp, ms) = *r.pick(OFFSET_MB);
    // The error state must stay clear: these patterns COMPILE on the reference.
    let st = r#"echo "|", preg_last_error(), "|";"#;
    match r.below(8) {
        // The plain match, with and without the flag, so the pair shape is
        // pinned against the very same captures in their unwrapped form.
        0 => vec![format!(
            "var_dump(preg_match({p}, '{s}', $m), $m); {st} \
             var_dump(preg_match({p}, '{s}', $n, PREG_OFFSET_CAPTURE), $n); {st}"
        )],
        // Byte offsets on a multibyte subject — the codepoint-counter killer.
        1 => vec![format!(
            "var_dump(preg_match({mp}, '{ms}', $m, PREG_OFFSET_CAPTURE), $m); {st} \
             var_dump(preg_match_all({mp}, '{ms}', $n, PREG_OFFSET_CAPTURE), $n); {st}"
        )],
        // Both match_all orders under the flag. They nest the pair at different
        // depths, and SET_ORDER additionally truncates each row at its own last
        // participating group, so the two disagree about where `-1` can appear.
        2 => vec![format!(
            "var_dump(preg_match_all({p}, '{s}', $m, PREG_PATTERN_ORDER|PREG_OFFSET_CAPTURE), $m); \
             {st} var_dump(preg_match_all({p}, '{s}', $n, PREG_SET_ORDER|PREG_OFFSET_CAPTURE), $n); \
             {st}"
        )],
        // The callback form, which hands the wrapped `$matches` to user code —
        // a separate call path from the out-parameter one above.
        3 => vec![format!(
            "var_dump(preg_replace_callback({p}, function($m) {{ return json_encode($m); }}, \
             '{s}', -1, $c, PREG_OFFSET_CAPTURE)); var_dump($c); {st}"
        )],
        // The start-offset parameter, which shifts where matching begins but
        // NOT the origin the reported offsets are measured from.
        //
        // The out-of-range values are the sharp end and are not decorative. An
        // offset PAST the end is an error — `false`, an emptied `$matches`, and
        // `PREG_INTERNAL_ERROR` — while the end ITSELF is merely a no-match,
        // and a negative one counts back from the end and clamps at zero rather
        // than failing. An implementation that simply clamped everything into
        // range would return 0 where the reference returns false.
        4 => vec![format!(
            "var_dump(preg_match({p}, '{s}', $m, PREG_OFFSET_CAPTURE, 1), $m); {st} \
             var_dump(preg_match({p}, '{s}', $n, PREG_OFFSET_CAPTURE, 2), $n); {st} \
             var_dump(preg_match({p}, '{s}', $a, 0, strlen('{s}')), $a); {st} \
             var_dump(preg_match({p}, '{s}', $b, 0, strlen('{s}') + 1), $b); {st} \
             var_dump(preg_match({p}, '{s}', $c, PREG_OFFSET_CAPTURE, -2), $c); {st} \
             var_dump(preg_match_all({p}, '{s}', $d, PREG_PATTERN_ORDER, 1), $d); {st} \
             var_dump(preg_match_all({p}, '{s}', $e, PREG_PATTERN_ORDER, strlen('{s}') + 1)); {st}"
        )],
        // A miss: `$matches` must be emptied, not left holding a stale pair.
        5 => vec![format!(
            "var_dump(preg_match({p}, '{s}', $m, PREG_OFFSET_CAPTURE), $m); {st} \
             var_dump(preg_match('/\\bzzqq\\b/', '{s}', $m, PREG_OFFSET_CAPTURE), $m); {st}"
        )],
        // Named groups, whose pair must be stored under BOTH the name and the
        // number — two slots that an implementation can easily wrap only once.
        6 => vec![format!(
            "var_dump(preg_match('/(?<a>\\w)(?<b>\\d)?/', '{s}', $m, PREG_OFFSET_CAPTURE), $m); \
             {st} var_dump(preg_match_all('/(?<a>\\w)(?<b>\\d)?/', '{s2}', $n, \
             PREG_OFFSET_CAPTURE), $n); {st}"
        )],
        // The flag combined with the one it is most often paired with, so a
        // bitmask test that used `==` instead of `&` shows up.
        _ => vec![format!(
            "var_dump(preg_match({p}, '{s2}', $m, PREG_OFFSET_CAPTURE|PREG_UNMATCHED_AS_NULL), $m); \
             {st} var_dump(preg_match({p}, '{s2}', $n, PREG_OFFSET_CAPTURE), $n); {st}"
        )],
    }
}

/// `foreach` with a destructuring target instead of a plain `$value`.
///
/// This mode exists because the generator was BLIND to the form: the only four
/// `foreach` occurrences in this file bound `$v`, `&$v`, `$k => $v` and `$p`,
/// so the bracket spelling was never emitted and the parse error it produced
/// was invisible to every previous run.
///
/// The SHORT-ROW cases are the sharp end. When the element has fewer entries
/// than the pattern, the reference does not bind null silently — it emits
/// `Warning: Undefined array key N` and then assigns null, so an engine that
/// quietly produces null passes a value check and still diverges on stdout.
/// Warnings land on stdout under the CLI defaults, so they are compared here.
fn gen_destructure(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let a = ii(r);
    let b = ii(r);
    let c = ii(r);
    let w = ww(r);
    match r.below(13) {
        // The two spellings of the same pattern, which must agree with each
        // other as well as with the reference.
        0 => vec![format!(
            "$a = [[{a},{b}],[{b},{c}]]; \
             foreach ($a as [$x,$y]) {{ echo $x, '-', $y, ';'; }} echo '|'; \
             foreach ($a as list($x,$y)) {{ echo $x, '=', $y, ';'; }} echo \"|\\n\";"
        )],
        // The keyed form, whose targets are looked up by string key rather than
        // by successive integer index.
        1 => vec![format!(
            "$a = [['k'=>{a},'j'=>{b}],['k'=>{c},'j'=>{a}]]; \
             foreach ($a as ['j'=>$v,'k'=>$u]) {{ echo $u, '/', $v, ';'; }} echo \"|\\n\";"
        )],
        // `$k => [pattern]` — a key binding AND a destructured value together.
        2 => vec![format!(
            "$a = ['p'=>[{a},{b}], 'q'=>[{b},{c}]]; \
             foreach ($a as $k => [$x,$y]) {{ echo $k, ':', $x, ',', $y, ';'; }} echo \"|\\n\";"
        )],
        // Nesting, which recurses the same target machinery one level down.
        3 => vec![format!(
            "$a = [[{a},[{b},{c}]],[{c},[{a},{b}]]]; \
             foreach ($a as [$p,[$q,$s]]) {{ echo $p, $q, $s, ';'; }} echo \"|\\n\";"
        )],
        // A hole, which binds nothing but still consumes its index.
        4 => vec![format!(
            "$a = [[{a},{b},{c}]]; \
             foreach ($a as [, $second]) {{ echo $second, ';'; }} echo '|'; \
             foreach ($a as [, , $third]) {{ echo $third, ';'; }} echo \"|\\n\";"
        )],
        // SHORT ROWS: a warning plus null, not a silent null.
        5 => vec![format!(
            "$a = [[{a}],[{a},{b}],[]]; \
             foreach ($a as [$x,$y]) {{ var_dump($x, $y); }} echo \"|\\n\";"
        )],
        // A short row under the KEYED form, whose missing key is reported by
        // name rather than by index.
        6 => vec![format!(
            "$a = [['k'=>{a}],['k'=>{b},'j'=>{c}]]; \
             foreach ($a as ['k'=>$u,'j'=>$v]) {{ var_dump($u, $v); }} echo \"|\\n\";"
        )],
        // Destructuring a non-array element, and strings mixed in, so the
        // element-is-not-a-list path is exercised too.
        7 => vec![format!(
            "$a = [[{a},{b}], '{w}', {c}]; \
             foreach ($a as [$x,$y]) {{ var_dump($x, $y); }} echo \"|\\n\";"
        )],
        // ── BY-REFERENCE TARGETS ────────────────────────────────────────────
        // Every arm above binds by VALUE, so the source array is never written
        // through the pattern and a target list that silently dropped its `&`
        // would still have scored. `&` in a target list is what makes the
        // assignment flow backwards into the subject.
        8 => vec![format!(
            "$a = [{a},{b}]; [&$x,$y] = $a; $x = {c}; \
             echo implode(',', $a), '|', $y, \"|\\n\";"
        )],
        // The `list()` spelling of the same thing, which must agree with the
        // bracket spelling as well as with the reference.
        9 => vec![format!(
            "$a = [{a},{b}]; list(&$x,$y) = $a; $x = {c}; \
             echo implode(',', $a), '|', $y, \"|\\n\";"
        )],
        // A reference target inside `foreach`, where the write lands on the
        // ROW of the outer array rather than on a standalone variable.
        10 => vec![format!(
            "$a = [[{a},{b}],[{b},{c}]]; \
             foreach ($a as [&$x,$y]) {{ $x = {c}; }} unset($x); \
             echo json_encode($a), \"|\\n\";"
        )],
        // Holes AND references together: a hole still consumes its index, so a
        // `&` after one must land on the correct later element.
        11 => vec![format!(
            "$a = [{a},{b},{c}]; [&$x,,&$z] = $a; $x = {b}; $z = {a}; \
             echo implode(',', $a), \"|\\n\";"
        )],
        // A reference under the KEYED form, and one NESTED a level down.
        _ => vec![format!(
            "$a = [['k'=>{a},'j'=>{b}]]; \
             foreach ($a as ['k'=>&$u]) {{ $u = {c}; }} unset($u); \
             echo json_encode($a), '|'; \
             $n = [[{a},[{b}]]]; \
             foreach ($n as [$p,[&$q]]) {{ $q = {c}; }} unset($q); \
             echo json_encode($n), \"|\\n\";"
        )],
    }
}

/// Constant references written with a namespace qualification.
///
/// This mode exists because the generator was BLIND to the form: the only two
/// backslash-qualified names in this file sat in `catch` and attribute
/// position, both of which take a CLASS name through a different parser path,
/// so a qualified CONSTANT was never emitted.
///
/// The naming rule is the whole point and it is not the obvious one. A leading
/// `\` is stripped — `\NOPE` is the constant `NOPE` — but inner separators are
/// KEPT, so `Foo\BAR` is a constant literally named `Foo\BAR` and is a
/// different constant from `BAR`. Both halves are asserted here, including in
/// the failure message: an undefined `Foo\BAR` must name `Foo\BAR` in the
/// thrown `Error`, and an undefined `\NOPE` must name `NOPE`.
///
/// The undefined cases matter as much as the defined ones. PHP 8 THROWS for an
/// unknown constant where PHP 7 fell back to the bare name as a string, so a
/// qualified reference that reached a second, older resolution path would
/// silently produce the string `"Foo\BAR"` instead of an `Error`.
fn gen_nsconst(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let v = ii(r);
    let v2 = ii(r);
    let w = ww(r);
    match r.below(12) {
        // Defined under a qualified name, read back both with and without the
        // global-namespace prefix.
        0 => vec![format!(
            "define('Foo\\BAR', {v}); echo Foo\\BAR, '|', \\Foo\\BAR, '|', \
             constant('Foo\\BAR'), \"|\\n\";"
        )],
        // A deep qualification, and `defined()` agreeing with the bare read.
        1 => vec![format!(
            "define('A\\B\\C', {v}); var_dump(defined('A\\B\\C'), A\\B\\C, \\A\\B\\C); \
             var_dump(defined('C'));"
        )],
        // The qualified name is DISTINCT from its last segment — the case a
        // last-segment fold gets wrong while passing everything above.
        2 => vec![format!(
            "define('BAR', {v}); define('Foo\\BAR', {v2}); \
             var_dump(BAR, Foo\\BAR, \\BAR, \\Foo\\BAR);"
        )],
        // Undefined qualified: throws, and the message carries the FULL name.
        3 => vec![format!(
            "try {{ echo Foo\\NOPE_{w}; }} catch (Error $e) {{ echo get_class($e), ':', \
             $e->getMessage(), \"|\\n\"; }}"
        )],
        // Undefined with only the global prefix: throws naming the BARE name.
        4 => vec![format!(
            "try {{ echo \\NOPE_{w}; }} catch (Error $e) {{ echo get_class($e), ':', \
             $e->getMessage(), \"|\\n\"; }} \
             try {{ echo NOPE_{w}; }} catch (Error $e) {{ echo $e->getMessage(), \"|\\n\"; }}"
        )],
        // Qualified constants in expression position, so the parse is exercised
        // somewhere other than a bare statement head.
        5 => vec![format!(
            "define('N\\X', {v}); define('N\\Y', {v2}); \
             var_dump(N\\X + N\\Y, [N\\X, \\N\\Y], N\\X <=> N\\Y);"
        )],
        // ── `const` AS A STATEMENT ──────────────────────────────────────────
        // Every arm above reaches the constant table through `define()`, which
        // is a function call. The `const` spelling is a declaration parsed at
        // statement level and was therefore never emitted here at all — the
        // only `const` anywhere in this file sits inside a class body, where a
        // different production handles it.
        6 => vec![format!("const K_{w} = {v}; echo K_{w};")],
        // The comma list, which declares several names from ONE `const`.
        7 => vec![format!(
            "const A_{w} = {v}, B_{w} = {v2}; echo A_{w}, '|', B_{w};"
        )],
        // A constant expression that READS an earlier constant, plus an array
        // constant subscripted at the point of use.
        8 => vec![format!(
            "const C_{w} = {v}; const D_{w} = C_{w} * 2; const E_{w} = [{v}, {v2}]; \
             var_dump(D_{w}, E_{w}[1], C_{w} . 'x');"
        )],
        // `const` takes effect WHERE IT STANDS — it is not hoisted, so the
        // `defined()` before it must answer false and the one after it true.
        9 => vec![format!(
            "var_dump(defined('F_{w}')); const F_{w} = {v}; \
             var_dump(defined('F_{w}'), F_{w}, constant('F_{w}'));"
        )],
        // Redefinition: a WARNING, and the FIRST value survives — not the last,
        // and not a fatal.
        10 => vec![format!(
            "const G_{w} = {v}; const G_{w} = {v2}; echo G_{w};"
        )],
        // The two spellings meeting: `define()` first, then `const` of the same
        // name, and the reverse order.
        _ => vec![format!(
            "define('H_{w}', {v}); const H_{w} = {v2}; echo H_{w}, '|'; \
             const I_{w} = {v}; var_dump(define('I_{w}', {v2})); echo I_{w};"
        )],
    }
}

/// Property access against classes that DO and DO NOT define the magic methods.
///
/// The operations are the point, not the declarations: each program writes,
/// reads back, unsets, and re-reads, so a `__set` that never stored anything and
/// an `__unset` that removed the wrong slot both surface as a wrong value later
/// rather than as a missing echo. The four questions PHP asks differently —
/// `isset`, `empty`, `??` and `@` — are all exercised, because they consult
/// `__isset` and `__get` in different combinations.
fn gen_propmagic(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    // The magic methods this class defines. Each subset behaves differently, and
    // "none" is the arm where the access error has to appear instead.
    let magic = *r.pick(&[
        "",
        "public function __get($n) { echo \"[G$n]\"; return $this->bag[$n] ?? \"g\"; }",
        "public function __set($n, $v) { echo \"[S$n]\"; $this->bag[$n] = $v; }",
        "public function __get($n) { echo \"[G$n]\"; return $this->bag[$n] ?? \"g\"; } \
         public function __set($n, $v) { echo \"[S$n]\"; $this->bag[$n] = $v; }",
        "public function __isset($n) { echo \"[I$n]\"; return isset($this->bag[$n]); } \
         public function __unset($n) { echo \"[U$n]\"; unset($this->bag[$n]); }",
        "public function __get($n) { echo \"[G$n]\"; return $this->bag[$n] ?? \"g\"; } \
         public function __set($n, $v) { echo \"[S$n]\"; $this->bag[$n] = $v; } \
         public function __isset($n) { echo \"[I$n]\"; return isset($this->bag[$n]); } \
         public function __unset($n) { echo \"[U$n]\"; unset($this->bag[$n]); }",
    ]);
    let vis = *r.pick(&["public", "protected", "private"]);
    let v = ii(r);
    // Every declaration carries the same private bag, so `__set` has somewhere to
    // put a value and `__get` has somewhere to find it.
    let cls = format!("class C {{ {vis} $p = 1; private $bag = []; {magic} }} $c = new C;");
    // A property the class never declares — the "absent" half of the same
    // machinery, where the access error must NOT appear.
    let undeclared = format!("class D {{ private $bag = []; {magic} }} $d = new D;");
    match r.below(8) {
        // Write, read back, unset, read again — the full round trip on the
        // declared property.
        0 => vec![format!(
            "{cls} try {{ $c->p = {v}; echo \"|\", $c->p, \"|\"; unset($c->p); \
             echo \"|\", $c->p, \"|\"; }} catch (Throwable $e) {{ \
             echo get_class($e), \"|\", $e->getMessage(); }}"
        )],
        // The same round trip on a property no class declares.
        1 => vec![format!(
            "{undeclared} $d->q = {v}; echo \"|\", $d->q, \"|\"; unset($d->q); \
             echo \"|\", $d->q, \"|\";"
        )],
        // The four "is it there" questions, which disagree with each other.
        2 => vec![format!(
            "{cls} var_dump(isset($c->p)); var_dump(empty($c->p)); \
             var_dump($c->p ?? 'D'); var_dump(@$c->p);"
        )],
        3 => vec![format!(
            "{undeclared} var_dump(isset($d->q)); var_dump(empty($d->q)); \
             var_dump($d->q ?? 'D'); var_dump(@$d->q);"
        )],
        // The same questions AFTER a write, so a `__set` that stored nothing
        // shows up as the wrong answer rather than as silence.
        4 => vec![format!(
            "{undeclared} $d->q = {v}; var_dump(isset($d->q)); var_dump(empty($d->q)); \
             var_dump($d->q ?? 'D');"
        )],
        // And after an unset, which must undo the write.
        5 => vec![format!(
            "{undeclared} $d->q = {v}; unset($d->q); var_dump(isset($d->q)); \
             var_dump($d->q ?? 'D'); echo \"|\", @$d->q, \"|\";"
        )],
        // Reached from INSIDE the class, where a private property is directly
        // reachable and no magic method fires at all.
        6 => vec![format!(
            "class C {{ {vis} $p = 1; private $bag = []; {magic} \
             public function go($v) {{ $this->p = $v; unset($this->bag); \
             return $this->p; }} }} \
             $c = new C; echo $c->go({v});"
        )],
        // Compound assignment and increment, which fetch for writing and then
        // write back — two passes through the same machinery.
        _ => vec![format!(
            "{undeclared} try {{ $d->q {}= {v}; echo \"|\", $d->q, \"|\"; $d->q++; \
             echo \"|\", $d->q, \"|\"; }} catch (Throwable $e) {{ \
             echo get_class($e), \"|\", $e->getMessage(); }}",
            r.pick(&["+", ".", "*"])
        )],
    }
}

/// `ini_get` over the settings the engine supplies itself, and `ini_set` over
/// the same. Reads are interleaved with writes because `ini_set` must return the
/// PREVIOUS value and leave the new one readable — a store that dropped the
/// write, or one that invented a setting it should have refused, both show up
/// only in a later read.
/// `(setting, a value the reference ACCEPTS for it)`.
///
/// Paired rather than crossed because the reference validates per setting, and a
/// value it refuses is not a question this harness can ask: `ini_set('memory_limit',
/// '20')` is refused with a message quoting the process's live memory usage, and
/// `ini_set('date.timezone', '-1')` needs a zone database to reject. Both are
/// nondeterministic or unavailable here, and the harness exists to compare
/// deterministic output — see the DIVERGENCE note above `INI_FIXED` in
/// `src/host.rs`. What IS asked: reads of every kind of setting, writes to the
/// changeable ones, refusal of the fixed ones, and refusal of a name that does
/// not exist.
const INI_SETTINGS: &[(&str, &str)] = &[
    ("memory_limit", "256M"),
    ("date.timezone", "America/New_York"),
    ("precision", "7"),
    ("serialize_precision", "17"),
    ("display_errors", "0"),
    ("max_execution_time", "30"),
    ("default_charset", "ISO-8859-1"),
    ("zend.assertions", "0"),
    ("pcre.backtrack_limit", "500000"),
    ("date.default_latitude", "51.5"),
    ("unserialize_max_depth", "2048"),
    ("default_socket_timeout", "10"),
    ("arg_separator.output", ";"),
    ("html_errors", "1"),
    ("log_errors", "0"),
    ("error_reporting", "0"),
    // Not runtime-changeable: `ini_set` must refuse these and change nothing.
    ("post_max_size", "16M"),
    ("output_buffering", "4096"),
    ("max_input_vars", "2000"),
    ("expose_php", "0"),
    ("allow_url_fopen", "0"),
    ("zend.multibyte", "1"),
    // Not a setting at all.
    ("no.such.setting", "1"),
];

fn gen_ini(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let (name, val) = *r.pick(INI_SETTINGS);
    match r.below(4) {
        0 => vec![format!("var_dump(ini_get('{name}'));")],
        1 => vec![format!(
            "var_dump(ini_set('{name}', '{val}')); var_dump(ini_get('{name}'));"
        )],
        // Restore, and prove the restore took.
        2 => vec![format!(
            "$old = ini_get('{name}'); var_dump($old); ini_set('{name}', '{val}'); \
             var_dump(ini_get('{name}')); ini_set('{name}', $old); \
             var_dump(ini_get('{name}') === $old);"
        )],
        _ => vec![format!(
            "var_dump(ini_get('{name}') === false); var_dump(ini_set('{name}', '{val}') === false);"
        )],
    }
}

/// Operands spanning PHP 8's three-way split of a value in an arithmetic
/// context: fully numeric (silent), leading-numeric (warns, uses the prefix),
/// and no numeric reading at all (`TypeError`).
///
/// The blank and empty strings are in here on purpose — they have no numeric
/// prefix, so PHP 8 throws on them exactly as it does on `"g"`, which is the
/// single most surprising corner of the rule.
const JUGGLE_OPERANDS: &[&str] = &[
    // numeric
    "5",
    "\"5\"",
    "\" 5 \"",
    "\"5.\"",
    "\".5\"",
    "\"-5\"",
    "\"5e3\"",
    "\"1e400\"",
    "2.5",
    "true",
    "false",
    "null",
    "0",
    // leading-numeric
    "\"5g\"",
    "\"5.5g\"",
    "\"-5g\"",
    "\".5g\"",
    "\"0x1A\"",
    "\"1_000\"",
    "\"5e\"",
    "\"5 x\"",
    // no numeric reading
    "\"g\"",
    "\"\"",
    "\"   \"",
    "\"INF\"",
    "\"NAN\"",
    "\"abc\"",
    "[1]",
    "[]",
];

const JUGGLE_BINOPS: &[&str] = &["+", "-", "*", "/", "%", "**", "|", "&", "^", "<<", ">>"];

/// PHP 8's string-to-number juggling across every operator that performs it.
///
/// Each program prints the outcome of a whole operation — the value, or the
/// class and message of whatever it threw — so a missing warning, a wrong
/// operand-type name and a silently-computed result are all visible as output
/// rather than only as an exit status. Every case is wrapped in `try` for the
/// same reason: an uncaught fatal prints nothing on either side, which would
/// let two different failures read as agreement.
fn gen_numjuggle(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let a = *r.pick(JUGGLE_OPERANDS);
    let b = *r.pick(JUGGLE_OPERANDS);
    let op = *r.pick(JUGGLE_BINOPS);
    // `var_dump` for the value so int-vs-float and int-vs-string are not
    // flattened by echo's stringification.
    let show = "catch (Throwable $e) { echo get_class($e), \"|\", $e->getMessage(), \"|\"; }";
    match r.below(7) {
        // The bare binary operation.
        0 => vec![format!("try {{ var_dump({a} {op} {b}); }} {show}")],
        // Compound assignment: the same rules on the read-modify-write path.
        1 => vec![format!(
            "try {{ $x = {a}; $x {op}= {b}; var_dump($x); }} {show}"
        )],
        // Unary plus and minus, which the engine lowers to multiplication.
        2 => vec![format!(
            "try {{ var_dump(-({a})); }} {show} try {{ var_dump(+({a})); }} {show}"
        )],
        // Increment/decrement, which have their own deprecations rather than
        // the operand rules — included so a fix to one does not silently
        // rewrite the other.
        3 => vec![format!(
            "try {{ $x = {a}; $x++; var_dump($x); $x--; var_dump($x); }} {show}"
        )],
        // The warning is maskable and the TypeError is not; both are checked
        // under a suppressed error level and again under `@`.
        4 => vec![format!(
            "error_reporting(E_ALL & ~E_WARNING); try {{ var_dump({a} {op} {b}); }} {show} \
             error_reporting(E_ALL); try {{ var_dump(@({a} {op} {b})); }} {show}"
        )],
        // Through a function call, so the unwind crosses a frame.
        5 => vec![format!(
            "function f($p, $q) {{ return $p {op} $q; }} try {{ var_dump(f({a}, {b})); }} {show}"
        )],
        // Ordering: which operand is resolved first is observable when one
        // warns and the other throws.
        _ => vec![format!(
            "try {{ var_dump(({a} {op} {b}) {op} {a}); }} {show} \
             try {{ var_dump({a} {op} ({b} {op} {b})); }} {show}"
        )],
    }
}

/// `json_decode`'s CONTAINER choice, which the generator was blind to: a grep
/// for `json_decode` over this file returned zero hits before this mode existed,
/// and the decoder ignored `$associative` outright — every JSON object became a
/// PHP array, so `json_decode('{"a":1}')->a` was a fatal on a `stdClass` that
/// was never built. `var_dump` is used deliberately: it prints the container
/// KIND and, for an object, its creation-order `#N`, which is the half of the
/// behaviour a `->a` read alone would not score.
fn gen_jsondecode(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let doc = *r.pick(&[
        r#"{"a":1}"#,
        r#"{"0":1,"a":2}"#,
        r#"{}"#,
        r#"{"a":{"b":1}}"#,
        r#"{"a":[{"x":1},{"y":2}]}"#,
        r#"[{"a":1},{"b":2}]"#,
        r#"{"a":{"b":1},"c":{"d":2}}"#,
        r#"{"a":null,"b":true,"c":1.5}"#,
        r#"[1,{"a":2},3]"#,
    ]);
    // The `$associative` argument in each of its meaningful spellings.
    let assoc = *r.pick(&["", ", true", ", false", ", null"]);
    match r.below(4) {
        0 => vec![format!("var_dump(json_decode('{doc}'{assoc}));")],
        1 => vec![format!(
            "var_dump(json_decode('{doc}', null, 512, JSON_OBJECT_AS_ARRAY));"
        )],
        // A decoded object round-trips back through the encoder unchanged.
        2 => vec![format!("echo json_encode(json_decode('{doc}'{assoc}));")],
        _ => vec![format!(
            "$v = json_decode('{doc}'{assoc}); echo gettype($v), '|', \
             json_last_error(), '|', is_object($v) ? get_class($v) : count((array)$v);"
        )],
    }
}

/// The four `html*` functions. They used to collapse into two implementations —
/// `htmlentities` was `htmlspecialchars` and `html_entity_decode` was
/// `htmlspecialchars_decode` — so every input scored the pair as agreeing when
/// the reference has them differ on the whole named-entity table, and `$flags`
/// was read by none of them.
fn gen_htmlent(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let raw = *r.pick(&[
        "a&<>\\\"'z",
        "caf\u{00e9}",
        "\u{20AC}\u{03B1}\u{2665}",
        "<b class=\\\"x\\\">t</b>",
        "5 < 6 > 7",
        "\u{00A0}\u{00FF}\u{2260}",
        "",
        "plain text",
    ]);
    let enc = *r.pick(&["htmlspecialchars", "htmlentities"]);
    let dec = *r.pick(&["htmlspecialchars_decode", "html_entity_decode"]);
    let flags = *r.pick(&["", ", ENT_QUOTES", ", ENT_COMPAT", ", ENT_NOQUOTES"]);
    match r.below(4) {
        0 => vec![format!("var_dump({enc}(\"{raw}\"{flags}));")],
        1 => vec![format!("var_dump({dec}({enc}(\"{raw}\"){flags}));")],
        // A mixed reference soup: named, decimal, hex, unknown, unterminated.
        2 => vec![format!(
            "var_dump({dec}(\"&lt;&eacute;&#233;&amp;&#039;&apos;&quot;&#xE9;&nope;&amp\"{flags}));"
        )],
        _ => vec![format!(
            "echo count(get_html_translation_table(HTML_ENTITIES{flags})), '|', \
             count(get_html_translation_table(HTML_SPECIALCHARS{flags}));"
        )],
    }
}

/// `strip_tags`, whose `$allowed_tags` was not read at all — nothing was ever
/// preserved — and whose depth-counter stand-in disagreed with the reference's
/// scanner on quoted attributes, comments and `<?…?>`.
fn gen_striptags(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let doc = *r.pick(&[
        "<b>x</b><i>y</i>",
        "<p>T <b class=\\\"x\\\">bold</b></p>",
        "<br/>a<br />b",
        "<a href=\\\"x>y\\\">link</a>",
        "<!-- c --> visible",
        "<!-- <b>x</b> --> y",
        "<?php echo 1; ?>after",
        "<?xml version=\\\"1.0\\\"?><a>x</a>",
        "<!DOCTYPE html><p>x</p>",
        "a < b and c > d",
        "unclosed <b",
        "<<b>>x",
        "<B CLASS=y>x</B>",
        "</b>x",
        "<>x",
    ]);
    let allow = *r.pick(&[
        "",
        ", \"<b>\"",
        ", \"<b><i>\"",
        ", \"<a>\"",
        ", \"<br>\"",
        ", \"<p>\"",
        ", [\"b\", \"i\"]",
        ", null",
    ]);
    vec![format!("var_dump(strip_tags(\"{doc}\"{allow}));")]
}

/// `compact()`'s two diagnostics. Neither was raised, so a misspelled name and a
/// non-string argument were both silently dropped — and a variable holding NULL
/// was dropped WITH them, because an unset name and a null one share one
/// representation until the BINDING is what gets asked about.
fn gen_compact(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let init = *r.pick(&[
        "$a = 1;",
        "$a = null;",
        "$a = 1; unset($a);",
        "$a = false;",
        "$a = '';",
        "$a = [1, 2];",
        "",
    ]);
    let names = *r.pick(&[
        "'a'",
        "'a', 'b'",
        "['a', 'b']",
        "['a'], 'b'",
        "''",
        "1",
        "'a', [2]",
        "true",
        "null",
        "['a', ['b']]",
    ]);
    match r.below(3) {
        0 => vec![format!("{init} var_dump(compact({names}));")],
        1 => vec![format!("{init} echo count(compact({names}));")],
        _ => vec![format!(
            "{init} $b = 2; echo json_encode(compact({names}));"
        )],
    }
}

/// Every callable FORM, through every call site that takes one.
///
/// The forms used to be decoded in two places that disagreed: `call_user_func`
/// understood `[$obj, "m"]` and `"C::m"` while `$f(…)`, `usort` and
/// `Closure::fromCallable` did not, and `__invoke` was honoured nowhere. Worst of
/// it was silent — `array_map([$obj, "m"], $a)` judged the callback absent and
/// returned `$a` UNMAPPED, so a wrong answer came back with no diagnostic at all.
fn gen_callform(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    const DECL: &str = "class C { public $b = 10; public function m($v) { return $v * 2; } \
                        public static function s($v) { return $v * 3; } \
                        public function __invoke($v) { return $v + 1; } } $c = new C;";
    let form = *r.pick(&["[$c, 'm']", "['C', 's']", "'C::s'", "$c", "'strrev'"]);
    match r.below(6) {
        0 => vec![format!("{DECL} $f = {form}; var_dump($f(4));")],
        1 => vec![format!("{DECL} var_dump(array_map({form}, [1, 2]));")],
        2 => vec![format!("{DECL} var_dump(call_user_func({form}, 5));")],
        3 => vec![format!(
            "{DECL} var_dump(is_callable({form}), Closure::fromCallable({form})(6));"
        )],
        4 => vec![format!(
            "{DECL} var_dump(array_filter([1, 2, 3], fn($v) => call_user_func({form}, $v) > 3));"
        )],
        _ => vec![format!(
            "{DECL} $x = [3, 1, 2]; usort($x, fn($p, $q) => \
             call_user_func({form}, $p) <=> call_user_func({form}, $q)); var_dump($x);"
        )],
    }
}

/// `parse_url`'s `$component`, whose `PHP_URL_*` selector constants were never
/// seeded — the spelling every program uses died on `Undefined constant` — and
/// whose out-of-range check did not exist.
fn gen_parseurl(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let url = *r.pick(&[
        "https://user:pw@host:8080/path?q=1#frag",
        "http://h/p",
        "/just/path",
        "mailto:a@b.c",
        "//host/p",
        "host:80",
        "file:///c:/x",
        "::",
    ]);
    let comp = *r.pick(&[
        "PHP_URL_SCHEME",
        "PHP_URL_HOST",
        "PHP_URL_PORT",
        "PHP_URL_USER",
        "PHP_URL_PASS",
        "PHP_URL_PATH",
        "PHP_URL_QUERY",
        "PHP_URL_FRAGMENT",
        "-1",
        "-2",
        "8",
        "9",
    ]);
    // The out-of-range component throws, so the call is caught the way this
    // file's other diagnostic modes catch theirs.
    vec![format!(
        "try {{ var_dump(parse_url(\"{url}\", {comp})); }} \
         catch (Throwable $e) {{ echo get_class($e), ': ', $e->getMessage(), \"\\n\"; }}"
    )]
}

/// `sscanf` in both of its shapes: the two-argument array form and the
/// by-reference form, over the whole specifier alphabet. Round 2 found this
/// family unrepresented, and with it `%x`/`%o`/`%i`, the `%[…]` scan sets, the
/// null padding of unreached slots, and the by-reference form itself.
fn gen_sscanf(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let input = *r.pick(&[
        "42 foo",
        "ff 10 0x1F",
        "12:34:56",
        "abc123",
        "  42",
        "1e3",
        "a b",
        "hello5",
        "SN/2350001",
        "12abc",
        "3.14 abc",
        "[abc]",
        "-17",
        "017",
        "0x1F",
        "00x10",
        "+5",
        "a1b2",
        "XYZ",
        "abcdef",
        "50%",
        "",
        "  ",
        "99999999999999999999",
        "1e+",
        ".",
        "]ab",
        "a-b",
    ]);
    let fmt = *r.pick(&[
        "%d %s",
        "%x %o %i",
        "%d:%d:%d",
        "%[a-c]",
        "%[^0-9]",
        "%[]a]",
        "%[a-]",
        "%d",
        "%s",
        "%c",
        "%c%c%c",
        "%2c",
        "%5s",
        "%f",
        "%e",
        "%u",
        "%o",
        "%i",
        "%*c%c",
        "%3s%3s",
        "%s%n",
        "%ld %hd %Lf",
        "%d%%",
        "SN/%d",
        "%d%s",
        "%d %d %d",
        "age %d name %s",
        "[%[a-c]]",
        "%[a-z]%d%[a-z]%d",
        "x%d",
        "  ",
    ]);
    let byref = r.below(2) == 0;
    if byref {
        // Untouched-variable semantics only show up when the variables are
        // pre-set: PHP leaves a slot no conversion reached exactly as it was.
        vec![format!(
            "$a = $b = $c = 'UNSET'; \
             var_dump(sscanf(\"{input}\", \"{fmt}\", $a, $b, $c), $a, $b, $c);"
        )]
    } else {
        vec![format!("var_dump(sscanf(\"{input}\", \"{fmt}\"));")]
    }
}

/// The `php_charmask` consumers — `addcslashes`, `trim`/`ltrim`/`rtrim` and
/// `str_word_count` — plus `stripcslashes`. The charlists deliberately include
/// every malformed `..` range, because the four diagnostics are shared by all of
/// them and were missing from all of them.
fn gen_cslashes(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let subject = *r.pick(&[
        "foo[bar]",
        "zoo['.']",
        "a..b",
        "x",
        "hello",
        "\\n\\t\\x07",
        "A1z9",
        "  padded  ",
        "XYZhi",
        "aqz",
        "!hi!",
        "",
    ]);
    let list = *r.pick(&[
        "A..Z",
        "A..z",
        "a..z",
        "z..A",
        "..z",
        "a..",
        "a..b..c",
        "z..a",
        "0..9",
        "!..#",
        "",
        "abc",
        ".",
        "\\n\\t\\x07",
    ]);
    let call = *r.pick(&["addcslashes", "trim", "ltrim", "rtrim"]);
    vec![
        format!("var_dump({call}(\"{subject}\", \"{list}\"));"),
        format!("var_dump(stripcslashes(\"{subject}\"));"),
        format!("var_dump(str_word_count(\"{subject}\", 0, \"{list}\"));"),
    ]
}

/// `count_chars` and `strtok` — the two newly ported string functions whose
/// contracts are stateful or mode-driven. `strtok` is exercised as a SEQUENCE,
/// because the interesting part is that running out of tokens discards the
/// subject rather than restarting it.
fn gen_strtok_counts(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let subject = *r.pick(&[
        "a b c",
        "  a  b  ",
        "a;b.c",
        "abc",
        "",
        "aab",
        "hello world",
        "  ",
    ]);
    let delims = *r.pick(&[" ", ";.", "", "x", "abc", " \\t"]);
    let mode = *r.pick(&["0", "1", "2", "3", "5", "-1"]);
    vec![
        format!(
            "var_dump(strtok(\"{subject}\", \"{delims}\"), strtok(\"{delims}\"), \
             strtok(\"{delims}\"), strtok(\"{delims}\"));"
        ),
        format!(
            "try {{ $c = count_chars(\"{subject}\", {mode}); \
             var_dump(is_array($c) ? count($c) : $c); }} \
             catch (Throwable $e) {{ echo get_class($e), ': ', $e->getMessage(), \"\\n\"; }}"
        ),
    ]
}

/// `substr_replace` with an array in each of its four parameters, and
/// `substr_compare` across its offset/length/case-insensitive matrix. Both were
/// uncovered, and both were wrong: the array form stringified its subject to
/// `"Array"`, and `$case_insensitive` was ignored outright.
fn gen_substrx(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let subject = *r.pick(&[
        "\"Hello\"",
        "[\"ab\", \"cd\"]",
        "[\"hello\", \"world\"]",
        "[\"k\" => \"abc\"]",
        "[\"abcd\", \"efgh\"]",
        "[]",
        "\"\"",
    ]);
    let replace = *r.pick(&["\"Z\"", "[\"X\", \"Y\"]", "[\"X\"]", "\"\"", "[]"]);
    let from = *r.pick(&["0", "1", "-2", "10", "[1, 0]", "[1, 2]", "[]"]);
    let length = *r.pick(&["1", "-1", "0", "100", "[2, 1]", "null", ""]);
    let call = if length.is_empty() {
        format!("substr_replace({subject}, {replace}, {from})")
    } else {
        format!("substr_replace({subject}, {replace}, {from}, {length})")
    };

    let hay = *r.pick(&["\"Hello\"", "\"Hello World\"", "\"abc\"", "\"a\"", "\"\""]);
    let needle = *r.pick(&[
        "\"hello\"",
        "\"world\"",
        "\"abz\"",
        "\"ABD\"",
        "\"\"",
        "\"abcdef\"",
    ]);
    let off = *r.pick(&["0", "3", "6", "-3", "5"]);
    let len2 = *r.pick(&["null", "5", "2", "0", "-1", "3"]);
    let ci = *r.pick(&["true", "false"]);
    vec![
        format!(
            "try {{ var_dump({call}); }} \
             catch (Throwable $e) {{ echo get_class($e), ': ', $e->getMessage(), \"\\n\"; }}"
        ),
        format!(
            "try {{ var_dump(substr_compare({hay}, {needle}, {off}, {len2}, {ci})); }} \
             catch (Throwable $e) {{ echo get_class($e), ': ', $e->getMessage(), \"\\n\"; }}"
        ),
    ]
}

/// The recursive array pair (`array_replace_recursive`, `array_walk_recursive`)
/// and the `array_sum`/`array_product` fold. The fold's entries are chosen to
/// straddle the three outcomes upstream distinguishes: a clean number, a
/// leading-numeric string, and an operand `+`/`*` rejects outright.
fn gen_arrayfold(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let base = *r.pick(&[
        "[\"a\" => [\"b\" => 1, \"c\" => 2]]",
        "[\"a\" => 1]",
        "[\"a\" => [\"x\"]]",
        "[1, 2, 3]",
        "[]",
    ]);
    let over = *r.pick(&[
        "[\"a\" => [\"b\" => 9]]",
        "[\"a\" => [\"b\" => 2]]",
        "[\"a\" => \"s\"]",
        "[9]",
        "[]",
    ]);
    let fold = *r.pick(&[
        "[1, \"a\"]",
        "[1, \"2abc\"]",
        "[1, [2]]",
        "[2, \"a\"]",
        "[]",
        "[1, null, true, false]",
        "[1, \"1e3\"]",
        "[1, new stdClass]",
        "[PHP_INT_MAX, 1]",
        "[1, 2.5]",
    ]);
    let fname = *r.pick(&["array_sum", "array_product"]);
    let nested = *r.pick(&["[1, [2, [3]]]", "[\"x\" => 1]", "[]", "[1, 2]"]);
    vec![
        format!("var_dump(array_replace_recursive({base}, {over}));"),
        format!("var_dump({fname}({fold}));"),
        format!(
            "$a = {nested}; \
             var_dump(array_walk_recursive($a, function (&$v, $k) {{ $v = \"$k:$v\"; }}), $a);"
        ),
    ]
}

/// Heredoc and nowdoc bodies.
///
/// This mode exists because the generator was BLIND to the construct: a grep
/// for `<<<` over this file returned ZERO hits before it was added, and the
/// lexer had no heredoc at all — every program here was
/// `Parse error: syntax error, unexpected token "<<"`. A construct that appears
/// in most real PHP scored not at all, in either direction.
///
/// Three properties are under test and each fails independently:
///   * the BODY LANGUAGE — a heredoc interpolates and escapes as a double-quoted
///     string does, except that `\"` keeps its backslash, while a nowdoc is
///     verbatim to the byte;
///   * the DELIMITER — only the exact label closes the body, and the newline
///     before it belongs to the delimiter rather than to the string;
///   * the INDENTATION — PHP 7.3 strips the closing marker's indentation from
///     every line and REFUSES the block when a line has less, which is a parse
///     error naming the offending line.
///
/// `var_dump` rather than `echo`: the length is half the answer, and an engine
/// that kept the trailing newline agrees with every check that only looks at
/// the text.
fn gen_heredoc(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let n = ii(r);
    let w = ww(r);
    match r.below(7) {
        // Interpolation, in each of the three spellings.
        0 => vec![format!(
            "$n = {n}; $a = [\"k\" => \"{w}\"]; $o = new stdClass; $o->p = \"{w}\";\n\
             $s = <<<EOT\nsimple $n, sub $a[k], prop $o->p, braced {{$a[\"k\"]}}\nEOT;\n\
             var_dump($s);"
        )],
        // Escapes. `\\\"` and `\\'` keep their backslash here and nowhere else.
        1 => {
            let esc = *r.pick(&[
                "\\t|\\n|\\\\",
                "\\$n",
                "\\x41\\101",
                "\\\"|\\'",
                "\\u{48}",
                "\\e",
                "\\q",
            ]);
            vec![format!(
                "$n = {n};\n$s = <<<EOT\n{esc}\nEOT;\nvar_dump($s);"
            )]
        }
        // Nowdoc: none of the above happens.
        2 => {
            let body = *r.pick(&["$n {$n}", "\\n \\\\ \\t", "a$n\\x41", "{$n}"]);
            vec![format!(
                "$n = {n};\n$s = <<<'EOT'\n{body}\nEOT;\nvar_dump($s);"
            )]
        }
        // Flexible indentation, including the level that is refused.
        3 => {
            let (body_indent, close_indent) = *r.pick(&[
                ("    ", "    "),
                ("      ", "    "),
                ("  ", "    "),
                ("\t", "\t"),
                ("", "  "),
                ("    ", ""),
            ]);
            vec![format!(
                "$s = <<<EOT\n{body_indent}one\n{body_indent}two\n\n{close_indent}EOT;\n\
                 var_dump($s);"
            )]
        }
        // The delimiter: a longer identifier does not close the body, and the
        // label may be followed by any token rather than only `;`.
        4 => {
            let tail = *r.pick(&["EOT;", "EOT . \"z\";", "EOT , \"z\");", "EOTX\nEOT;"]);
            let open = if tail.starts_with("EOT ,") {
                "$s = implode(\"-\", array(<<<EOT\nbody\n"
            } else {
                "$s = <<<EOT\nbody\n"
            };
            vec![format!("{open}{tail}\nvar_dump($s);")]
        }
        // The empty and near-empty bodies, where the trailing-newline rule is
        // the whole answer.
        5 => {
            let body = *r.pick(&["", "\n", "a\n", "\n\n"]);
            vec![format!("$s = <<<EOT\n{body}EOT;\nvar_dump($s);")]
        }
        // A quoted label is a heredoc; an unterminated body is a parse error
        // that names the line the file ran out on.
        _ => {
            let form = *r.pick(&[
                "$s = <<<\"EOT\"\nv$n\nEOT;\nvar_dump($s);",
                "$s = <<<   EOT\nv$n\nEOT;\nvar_dump($s);",
                "$s = <<<_e1\nv$n\n_e1;\nvar_dump($s);",
                "$s = <<<EOT\nv$n\n",
                "$s = <<<9BAD\nv\n9BAD;\nvar_dump($s);",
            ]);
            vec![format!("$n = {n};\n{form}")]
        }
    }
}

/// `?->` chains longer than one link.
///
/// The generator only ever emitted a `?->` as the LAST access, so the operator's
/// defining property was never asked about: the short-circuit covers the whole
/// remaining chain, not the link that spelled it. Every program here would have
/// agreed under a one-link implementation; each of these does not.
///
/// The receiver is null in half the cases and an object in the other half,
/// because an implementation that short-circuits unconditionally passes every
/// null case and fails every other one.
fn gen_nullsafechain(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let decl = "class B { public $v = 7; public $n = null; function m($x = 1) { return $x * 2; } } \
                class A { public $b; public $z = null; function __construct() { $this->b = new B(); } }";
    let chain = *r.pick(&[
        "$r?->a->b",
        "$r?->a->b->c",
        "$r?->a[\"k\"]",
        "$r?->m()->x",
        "$r?->a->b->c()",
        "$r?->b?->v",
        "$r?->b->v",
        "$r?->b->m(3)",
        "$r?->z?->v->w",
        "$r?->b->n?->v->w",
        "$r?->b->n->v",
    ]);
    let recv = *r.pick(&["null", "new A()", "(new A())->z"]);
    match r.below(4) {
        // The value, with every diagnostic the chain raises on the way.
        0 => vec![format!("{decl} $r = {recv}; var_dump({chain});")],
        // The extent: the enclosing expression must still run.
        1 => vec![format!(
            "{decl} $r = {recv}; echo \"[\"; var_dump({chain}); echo \"]\";"
        )],
        // The skipped links' ARGUMENTS must not be evaluated.
        2 => vec![format!(
            "{decl} function f() {{ echo \"F\"; return 1; }} $r = {recv}; \
             var_dump($r?->a->m(f()));"
        )],
        // `BP_VAR_IS` mode over the same chains.
        _ => {
            let q = *r.pick(&["isset(%)", "empty(%)", "% ?? \"D\"", "@%"]);
            vec![format!(
                "{decl} $r = {recv}; var_dump({});",
                q.replace('%', chain)
            )]
        }
    }
}

/// The `UnhandledMatchError` message, over a subject of every type.
///
/// The generator reached `match` in two places and both used an integer
/// subject, so the message was only ever scored on the one type whose
/// concatenation happens to be right. `null` rendered as nothing at all,
/// `true` as `1`, a string unquoted, and an array as `Array` behind an `Array
/// to string conversion` the reference never raises.
fn gen_matcherr(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let subject = *r.pick(&[
        "null",
        "true",
        "false",
        "5",
        "-0.0",
        "1.0",
        "1e100",
        "NAN",
        "PHP_INT_MAX",
        "\"hi\"",
        "\"\"",
        "\"0\"",
        "str_repeat(\"ab\", 30)",
        "[1, 2]",
        "[]",
        "new stdClass",
        "new ArrayObject([])",
        "(fn() => 1)",
    ]);
    let arms = *r.pick(&[
        "999999 => 1",
        "1 => \"a\", 2 => \"b\"",
        "\"x\" => 1",
        "null => \"n\"",
    ]);
    match r.below(3) {
        0 => vec![format!(
            "try {{ echo match ({subject}) {{ {arms} }}; }} \
             catch (\\UnhandledMatchError $e) {{ echo get_class($e), \"|\", $e->getMessage(); }}"
        )],
        // Uncaught: the whole fatal rendering, trace included.
        1 => vec![format!("echo match ({subject}) {{ {arms} }};")],
        // A `default` means no error at all — the arm must win over the throw.
        _ => vec![format!(
            "echo \"[\", match ({subject}) {{ {arms}, default => \"D\" }}, \"]\";"
        )],
    }
}

/// Constants reached through an interface.
///
/// The lookup walked only the `parent` chain, so a constant declared in an
/// interface was `Error: Undefined constant C::K` from everywhere — through the
/// class name, through `self::`, through `static::`, and through an interface
/// that merely extends the one that declared it. No previous case asked: the
/// only `interface` in this file declared no constants.
fn gen_ifaceconst(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let v = ii(r);
    let shape = *r.pick(&[
        "interface I { const K = %; } class C implements I {}",
        "interface I { const K = %; } interface J extends I {} class C implements J {}",
        "interface I { const K = %; } class P implements I {} class C extends P {}",
        "interface I { const K = 1; } class P implements I {} class C extends P { const K = %; }",
        "interface I { const K = 1; } class C implements I { const K = %; }",
        "interface I { const K = %; } trait T {} class C implements I { use T; }",
        "interface I { const K = 1; } class C {} const K = %;",
        "abstract class I { const K = %; } class C extends I {}",
    ]);
    let decl = shape.replace('%', v);
    let read = *r.pick(&[
        "C::K",
        "I::K",
        "constant(\"C::K\")",
        "defined(\"C::K\")",
        "(new C())->r()",
    ]);
    let body = if read == "(new C())->r()" {
        // `self::`/`static::` reach the same table, through the running class.
        let via = *r.pick(&["self::K", "static::K", "C::K", "$this::K"]);
        format!(
            "{decl} class D {{}} function mk() {{ return null; }} \
                 try {{ echo (function () {{ return {via}; }})->call(new C()); }} \
                 catch (\\Throwable $e) {{ echo get_class($e), \": \", $e->getMessage(); }}"
        )
    } else {
        format!(
            "{decl} try {{ var_dump({read}); }} \
             catch (\\Throwable $e) {{ echo get_class($e), \": \", $e->getMessage(); }}"
        )
    };
    vec![body]
}

/// Generators: `yield`, `yield from`, the return value, and `send`.
///
/// A grep for `yield` over this file returned ZERO hits before this mode, so
/// the whole construct — its key numbering, its delegation, and the fact that
/// a generator body does not run until it is asked — was unscored.
///
/// The auto-KEYS are the sharp end: a `yield from` over an array replays that
/// array's own keys rather than continuing the outer counter, so a delegating
/// generator legitimately yields the key `0` twice. An implementation that
/// numbered them straight through agrees with every check that only looks at
/// values.
fn gen_generators(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let n = *r.pick(&["0", "1", "3"]);
    match r.below(6) {
        0 => vec![format!(
            "function g() {{ for ($i = 0; $i < {n}; $i++) {{ yield $i => $i * 2; }} return \"R\"; }} \
             $x = g(); foreach ($x as $k => $v) {{ echo \"$k=$v,\"; }} var_dump($x->getReturn());"
        )],
        1 => vec![
            "function g() { yield 1; yield from [10, 20]; yield from h(); yield 2; } \
             function h() { yield \"a\" => 5; } \
             foreach (g() as $k => $v) { echo \"$k=$v,\"; }"
                .to_string(),
        ],
        2 => vec![
            "function g() { $x = yield 1; echo \"got:\", var_export($x, true), \",\"; \
             $y = yield 2; echo \"got:\", var_export($y, true), \",\"; } \
             $g = g(); echo $g->current(), \",\", $g->send(\"A\"), \",\"; $g->send(\"B\"); \
             var_dump($g->valid());"
                .to_string(),
        ],
        3 => vec![
            "function g() { echo \"body,\"; yield 1; } echo \"before,\"; $g = g(); \
             echo \"made,\"; var_dump($g->key()); echo \"|\"; var_dump(iterator_to_array($g));"
                .to_string(),
        ],
        4 => vec![format!(
            "function g() {{ try {{ yield 1; yield 2; }} finally {{ echo \"F,\"; }} }} \
             foreach (g() as $v) {{ echo $v, \",\"; if ($v >= {n}) break; }} echo \"done\";"
        )],
        _ => vec![
            "function g() { yield 1; throw new RuntimeException(\"boom\"); } \
             try { foreach (g() as $v) { echo $v; } } \
             catch (\\Throwable $e) { echo get_class($e), \":\", $e->getMessage(); } \
             var_dump((function () { yield; })() instanceof Generator);"
                .to_string(),
        ],
    }
}

/// Enums, pure and backed.
///
/// Unscored before this mode: a grep for `enum ` over the generated programs
/// returned nothing. `from`/`tryFrom` are the half most likely to be wrong in
/// a way no value check notices — `from` THROWS for an unknown value where
/// `tryFrom` answers null, and `from` on a backed enum is strict about the
/// backing type.
fn gen_enums(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let backed = *r.pick(&["string", "int"]);
    let (a, b) = if backed == "int" {
        ("1", "2")
    } else {
        ("\"a\"", "\"b\"")
    };
    let probe = *r.pick(&[a, b, "\"z\"", "9", "null"]);
    match r.below(5) {
        0 => vec![format!(
            "enum E: {backed} {{ case A = {a}; case B = {b}; }} \
             var_dump(E::tryFrom({probe})); \
             try {{ var_dump(E::from({probe})); }} \
             catch (\\Throwable $e) {{ echo get_class($e), \": \", $e->getMessage(); }}"
        )],
        1 => vec![format!(
            "enum E: {backed} {{ case A = {a}; case B = {b}; }} \
             print_r(E::cases()); var_dump(E::A === E::A, E::A == E::B, E::A instanceof E);"
        )],
        2 => vec![
            "enum P { case X; case Y; } var_dump(P::X); echo P::X->name; \
             var_dump(P::X instanceof UnitEnum, P::X instanceof BackedEnum);"
                .to_string(),
        ],
        3 => vec![format!(
            "interface HasLabel {{ const PFX = \"p:\"; public function label(): string; }} \
             enum E: {backed} implements HasLabel {{ case A = {a}; \
             const DEFAULT = self::A; \
             public function label(): string {{ return self::PFX . $this->name; }} }} \
             echo E::A->label(), \"|\", E::DEFAULT->name, \"|\", E::PFX;"
        )],
        _ => vec![format!(
            "enum E: {backed} {{ case A = {a}; }} \
             try {{ $x = new E(); }} catch (\\Throwable $e) {{ echo get_class($e), \"|\"; }} \
             var_dump(json_encode([E::A]), E::A->value, isset(E::A->name));"
        )],
    }
}

/// Trait composition, including the two conflict-resolution clauses.
///
/// The generator emitted no `trait` at all. The `insteadof`/`as` clauses are
/// the part with real rules — an unresolved collision between two traits that
/// declare the same method is a FATAL, and `as` can rename a method as well as
/// change its visibility.
fn gen_traits(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    match r.below(5) {
        0 => vec![
            "trait T { public function hi() { return static::class . \"-\" . self::class; } } \
             class C { use T; } class D extends C {} echo (new C())->hi(), \"|\", (new D())->hi();"
                .to_string(),
        ],
        1 => {
            let clause = *r.pick(&[
                "A::f insteadof B; B::f as g;",
                "B::f insteadof A;",
                "A::f insteadof B;",
                "",
            ]);
            vec![format!(
                "trait A {{ public function f() {{ return \"A\"; }} }} \
                 trait B {{ public function f() {{ return \"B\"; }} }} \
                 class C {{ use A, B {{ {clause} }} }} \
                 $c = new C(); echo $c->f(); if (method_exists($c, \"g\")) {{ echo $c->g(); }}"
            )]
        }
        2 => vec![
            "trait T { public static $n = 0; public static function bump() { return ++static::$n; } \
             abstract public function need(): string; } \
             class C { use T; public function need(): string { return \"N\"; } } \
             class D { use T; public function need(): string { return \"M\"; } } \
             echo C::bump(), C::bump(), D::bump(), (new C())->need();"
                .to_string(),
        ],
        3 => vec![
            "trait T { public function f() { return \"T\"; } } \
             class C { use T; public function f() { return \"C\"; } } \
             class P { public function f() { return \"P\"; } } class Q extends P { use T; } \
             echo (new C())->f(), (new Q())->f();"
                .to_string(),
        ],
        _ => vec![
            "trait T { public function f() { return \"T\"; } } \
             class C { use T { f as protected p; } public function q() { return $this->p(); } } \
             $c = new C(); echo $c->q(); \
             try { echo $c->p(); } catch (\\Throwable $e) { echo \"|\", get_class($e), \": \", $e->getMessage(); }"
                .to_string(),
        ],
    }
}

/// Variadic parameters and `...` spread at a call site and in an array literal.
///
/// A grep for `...` over the generated programs returned ZERO hits. The
/// STRING-keyed spread is the corner worth the mode on its own: those keys are
/// named arguments, so `f(...["b" => 2, "a" => 1])` binds by NAME, and a key
/// that matches no parameter is an `Error`, not a silent drop.
fn gen_variadic(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let spread = *r.pick(&[
        "[1, 2]",
        "[]",
        "[\"b\" => 2, \"a\" => 1]",
        "[\"nope\" => 1]",
        "[1, \"b\" => 2]",
        "\"str\"",
        "null",
        "(function () { yield 1; yield 2; })()",
        "new ArrayIterator([3, 4])",
    ]);
    match r.below(5) {
        0 => vec![format!(
            "function f(...$xs) {{ return count($xs) . \":\" . implode(\",\", $xs); }} \
             try {{ echo f(...{spread}); }} \
             catch (\\Throwable $e) {{ echo get_class($e), \": \", $e->getMessage(); }}"
        )],
        1 => vec![format!(
            "function f($a = \"x\", $b = \"y\") {{ return \"$a|$b\"; }} \
             try {{ echo f(...{spread}); }} \
             catch (\\Throwable $e) {{ echo get_class($e), \": \", $e->getMessage(); }}"
        )],
        2 => vec![format!(
            "try {{ var_dump([0, ...{spread}, 9]); }} \
             catch (\\Throwable $e) {{ echo get_class($e), \": \", $e->getMessage(); }}"
        )],
        3 => vec![
            "function f(int ...$xs) { return array_sum($xs); } \
             try { echo f(1, 2, \"3\"); } catch (\\Throwable $e) { echo get_class($e), \": \", $e->getMessage(); } \
             echo \"|\"; \
             function g($first, ...$rest) { return $first . \"/\" . implode(\",\", $rest); } echo g(1, 2, 3);"
                .to_string(),
        ],
        _ => vec![format!(
            "function f(&...$xs) {{ return count($xs); }} \
             try {{ $a = 1; echo f($a); }} \
             catch (\\Throwable $e) {{ echo get_class($e), \": \", $e->getMessage(); }} \
             echo \"|\", (fn(...$a) => implode(\"-\", $a))(...{spread});"
        )],
    }
}

/// The interfaces the ENGINE consults rather than the program: `ArrayAccess`,
/// `Countable`, `Iterator`, `IteratorAggregate` and `Stringable`.
///
/// None of them appeared in a generated program. Each is a place where a
/// built-in operation (`$o[k]`, `count()`, `foreach`, string coercion) is
/// supposed to dispatch into user code, and an engine that does not is wrong in
/// a way the user code itself cannot show.
fn gen_splobj(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    match r.below(5) {
        0 => vec![
            "class C implements ArrayAccess { private $d = [\"a\" => 1]; \
             public function offsetExists($o): bool { return isset($this->d[$o]); } \
             public function offsetGet($o): mixed { return $this->d[$o] ?? \"none\"; } \
             public function offsetSet($o, $v): void { if ($o === null) { $this->d[] = $v; } else { $this->d[$o] = $v; } } \
             public function offsetUnset($o): void { unset($this->d[$o]); } } \
             $c = new C(); $c[\"b\"] = 2; $c[] = 3; \
             var_dump($c[\"a\"], isset($c[\"b\"]), isset($c[\"zz\"]), $c[0]); \
             unset($c[\"a\"]); var_dump($c[\"a\"], empty($c[\"b\"]));"
                .to_string(),
        ],
        1 => {
            let n = *r.pick(&["0", "1", "7"]);
            vec![format!(
                "class C implements Countable {{ public function count(): int {{ return {n}; }} }} \
                 var_dump(count(new C()), (new C()) instanceof Countable); \
                 try {{ var_dump(count(new stdClass)); }} \
                 catch (\\Throwable $e) {{ echo get_class($e), \": \", $e->getMessage(); }}"
            )]
        }
        2 => vec![
            "class C implements IteratorAggregate { public function getIterator(): Iterator { return new ArrayIterator([\"x\" => 1, \"y\" => 2]); } } \
             foreach (new C() as $k => $v) { echo \"$k=$v,\"; } \
             var_dump(iterator_to_array(new C()));"
                .to_string(),
        ],
        3 => vec![
            "class C implements Iterator { private $i = 0; private $a = [10, 20, 30]; \
             public function current(): mixed { return $this->a[$this->i]; } \
             public function key(): mixed { return \"k\" . $this->i; } \
             public function next(): void { $this->i++; } \
             public function rewind(): void { echo \"R,\"; $this->i = 0; } \
             public function valid(): bool { return $this->i < count($this->a); } } \
             $c = new C(); foreach ($c as $k => $v) { echo \"$k=$v,\"; } \
             foreach ($c as $v) { echo $v, \",\"; break; }"
                .to_string(),
        ],
        _ => {
            let ret = *r.pick(&["\"S\"", "\"\"", "(string) 5"]);
            vec![format!(
                "class C {{ public function __toString(): string {{ return {ret}; }} }} \
                 $c = new C(); echo $c, \"|\", \"x{{$c}}y\", \"|\"; \
                 var_dump((string) $c, $c instanceof Stringable, strlen($c), $c == \"S\");"
            )]
        }
    }
}

// ---------------------------------------------------------------------------
// Mode registry.
// ---------------------------------------------------------------------------

/// PHP's ALTERNATIVE control-structure syntax: `if (…): … endif;` and the
/// `endwhile`/`endfor`/`endforeach`/`endswitch`/`enddeclare` family.
///
/// A grep for `endif`/`endwhile`/`endfor` over the generators returned ZERO
/// hits, and the whole spelling was a parse error — every one of the six
/// constructs. It is the spelling PHP templates are written in, so a program
/// that mixes it with `?> html <?php` is the shape that matters most.
fn gen_altsyntax(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let n = ii(r);
    let m = ii(r);
    let body = *r.pick(&[
        "echo \"b\";",
        "echo $i;",
        "$t .= \"x\";",
        "if ($i > 0): echo \"p\"; else: echo \"q\"; endif;",
        "?>raw<?php ",
        "echo \"a\"; ?>|<?php echo \"z\";",
    ]);
    let shape = *r.pick(&[
        "if (#N): echo \"t\"; endif;",
        "if (#N): echo \"t\"; else: echo \"f\"; endif;",
        "if (#N): echo \"t\"; elseif (#M): echo \"e\"; else: echo \"f\"; endif;",
        "if (#N): if (#M): echo \"in\"; endif; echo \"out\"; endif;",
        "for ($i = 0; $i < 3; $i++): #B endfor;",
        "$i = 0; while ($i < 3): #B $i++; endwhile;",
        "foreach ([#N, #M] as $i): #B endforeach;",
        "foreach ([\"k\" => #N] as $k => $i): echo \"$k=$i\"; endforeach;",
        "switch (#N): case #M: echo \"m\"; break; case #N: echo \"n\"; break; default: echo \"d\"; endswitch;",
        "switch (#N): default: echo \"d\"; endswitch;",
        "declare(ticks=1): echo \"tick\"; enddeclare;",
        "if (#N): ?>YES<?php else: ?>NO<?php endif;",
        "for ($i = 0; $i < 2; $i++): if ($i): continue; endif; echo $i; endfor;",
        "$i = 0; while (true): $i++; if ($i > 2): break; endif; echo $i; endwhile;",
    ]);
    let prog = shape.replace("#N", n).replace("#M", m).replace("#B", body);
    vec![format!("$t = \"\"; {prog} echo \"|\", $t, \"|\";")]
}

/// `print` as an EXPRESSION, and the word logical operators around it.
///
/// `print` was read only at the head of a statement, so `$r = print "x"` and
/// `var_dump(print "x")` were parse errors; `and`/`or` were folded onto
/// `&&`/`||`, which binds them TIGHTER than `=` instead of looser; and `xor`
/// was not a token at all. All three are in the same precedence neighbourhood,
/// so one mode scores them together.
fn gen_printexpr(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let a = *r.pick(&[
        "true", "false", "1", "0", "\"\"", "\"s\"", "null", "[]", "2",
    ]);
    let b = *r.pick(&["true", "false", "1", "0", "\"\"", "\"s\"", "null", "3"]);
    let shape = *r.pick(&[
        "$x = #A and #B; var_dump($x);",
        "$x = #A or #B; var_dump($x);",
        "$x = #A xor #B; var_dump($x);",
        "var_dump(#A and #B, #A or #B, #A xor #B);",
        "var_dump(#A xor #B xor #A);",
        "var_dump(#A or #B and #A);",
        "$x = 1; $y = $x == 1 and #B; var_dump($x, $y);",
        "$r = print \"p\"; var_dump($r);",
        "var_dump(print \"p\");",
        "echo print(\"p\"), \"|\";",
        "print #A; echo \"|\";",
        "#A or print \"o\"; echo \"|\";",
        "$x = #A and print \"a\"; var_dump($x);",
        "var_dump(#A ? print \"y\" : print \"n\");",
        "print print \"n\"; echo \"|\";",
        "function f($v) { echo \"f\"; return $v; } var_dump(f(#A) and f(#B));",
        "function f($v) { echo \"f\"; return $v; } var_dump(f(#A) or f(#B));",
        "function f($v) { echo \"f\"; return $v; } var_dump(f(#A) xor f(#B));",
        "$a = #A; $a = #B and #A; var_dump($a);",
        "var_dump(#A && #B or #A);",
    ]);
    vec![shape.replace("#A", a).replace("#B", b)]
}

/// The reflection surface over the values that are objects WITHOUT being class
/// instances, plus the four existence predicates.
///
/// A grep for `get_class`, `is_a(`, `class_exists`, `get_object_vars` and
/// `spl_object` over the generators returned zero or near-zero hits. A closure
/// and a generator are instances of `Closure` and `Generator` to every one of
/// these, and were not: `get_class($gen)` was a `TypeError` whose own message
/// read "must be of type object, object given".
fn gen_reflect(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let subject = *r.pick(&[
        "(function () { yield 1; })()",
        "(function () { return 1; })",
        "(fn() => 1)",
        "new C()",
        "new D()",
        "E::A",
        "[1, 2]",
        "\"C\"",
        "17",
        "null",
    ]);
    let q = *r.pick(&[
        "var_dump(is_object($v));",
        "var_dump(gettype($v));",
        "var_dump(get_debug_type($v));",
        "try { var_dump(get_class($v)); } catch (\\Throwable $e) { echo get_class($e), \": \", $e->getMessage(); }",
        "try { var_dump(get_object_vars($v)); } catch (\\Throwable $e) { echo get_class($e), \": \", $e->getMessage(); }",
        "var_dump($v instanceof Generator, $v instanceof Closure);",
        "var_dump($v instanceof Traversable, $v instanceof Iterator);",
        "var_dump($v instanceof I, $v instanceof C, $v instanceof UnitEnum);",
        "var_dump(is_a($v, \"Closure\"), is_a($v, \"Generator\"), is_a($v, \"C\"));",
        "var_dump(is_subclass_of($v, \"Iterator\"), is_subclass_of($v, \"C\"));",
        "var_dump(method_exists($v, \"current\"), method_exists($v, \"bindTo\"), method_exists($v, \"go\"));",
        "var_dump(is_callable($v), is_iterable($v), is_countable($v), is_scalar($v));",
        "try { echo strlen($v); } catch (\\Throwable $e) { echo get_class($e), \": \", $e->getMessage(); }",
        "try { echo $v + 1; } catch (\\Throwable $e) { echo get_class($e), \": \", $e->getMessage(); }",
        "try { var_dump($v::class); } catch (\\Throwable $e) { echo get_class($e), \": \", $e->getMessage(); }",
        "var_dump($v == $v);",
        "$w = $v; var_dump($v === $w);",
    ]);
    let names = *r.pick(&[
        "\"C\"",
        "\"D\"",
        "\"I\"",
        "\"T\"",
        "\"E\"",
        "\"Closure\"",
        "\"Generator\"",
        "\"Traversable\"",
        "\"Iterator\"",
        "\"Countable\"",
        "\"Ghost\"",
        "\"stdClass\"",
    ]);
    let extra = r.pick(&[
        "",
        "echo \"|\", (int)class_exists(#N), (int)interface_exists(#N), (int)trait_exists(#N), (int)enum_exists(#N);",
        "echo \"|\"; var_dump(class_implements(#N));",
        "echo \"|\"; var_dump(class_uses(#N));",
        "echo \"|\"; var_dump(class_parents(#N));",
    ])
    .replace("#N", names);
    vec![format!(
        "interface I {{}} trait T {{}} enum E {{ case A; }} \
         class C implements I {{ use T; public $p = 1; function go() {{}} }} \
         class D extends C {{}} \
         $v = {subject}; {q} {extra}"
    )]
}

/// `isset()`'s operand rule, which is a COMPILE-time one.
///
/// The reference refuses `isset()` on anything that is not a variable, an
/// index, a property or a static property — before the program runs, so nothing
/// the operand would have printed appears. phplang evaluated the operand and
/// answered `true`, which is a wrong answer AND wrong output.
fn gen_issetform(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let operand = *r.pick(&[
        "$a",
        "$a[0]",
        "$a[9]",
        "$a[\"k\"]",
        "$u",
        "$o->p",
        "$o->q",
        "$o?->p",
        "C::$s",
        "C::$s[0]",
        "$$name",
        "f()",
        "f()[0]",
        "f2()->p",
        "$o->m()",
        "C::sm()",
        "new C",
        "1 + 1",
        "\"s\"",
        "K",
        "C::KK",
        "$c()",
        "$a[f()]",
    ]);
    let form = *r.pick(&["isset(#O)", "empty(#O)", "isset($a, #O)", "isset(#O, $a)"]);
    let call = form.replace("#O", operand);
    vec![format!(
        "define(\"K\", 1); \
         class C {{ const KK = 2; public static $s = [5]; public $p = 1; \
                    function m() {{ return 1; }} static function sm() {{ return 1; }} }} \
         function f() {{ echo \"F\"; return [7]; }} \
         function f2() {{ echo \"G\"; return new C(); }} \
         $a = [1, \"k\" => 2]; $o = new C(); $name = \"a\"; $c = fn() => 1; \
         echo \"start|\"; var_dump({call});"
    )]
}

/// The ORDER a call evaluates its parts in: the reference decides the callee
/// cannot be called before it evaluates one argument, so an argument that
/// echoes prints nothing when the call is going to fail.
///
/// A grep found no generator that put a side effect in an argument of a call
/// that fails, so every one of these printed the argument's output first.
fn gen_callorder(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let recv = *r.pick(&[
        "null", "5", "\"s\"", "[1]", "true", "false", "1.5", "new C()", "$g", "$c",
    ]);
    let call = *r.pick(&[
        "$r->m(f())",
        "$r->m(f(), f())",
        "$r->go(f())",
        "$r?->m(f())",
        "$r->m(x: f())",
        "$r->m(...[f()])",
    ]);
    let wrap = r.pick(&[
        "try { #C; } catch (\\Throwable $e) { echo \"|\", get_class($e), \": \", $e->getMessage(); }",
        "#C;",
        "echo @#C;",
    ])
    .replace("#C", call);
    vec![format!(
        "class C {{ function go($x = 0) {{ echo \"GO\"; return $x; }} }} \
         function f() {{ echo \"F\"; return 1; }} \
         $g = (function () {{ yield 1; }})(); $c = fn() => 1; \
         $r = {recv}; echo \"start|\"; {wrap}"
    )]
}

/// `clone` and `__clone`: a shallow copy, the hook that runs after it, and the
/// values that cannot be cloned at all.
///
/// A grep for `clone ` and `__clone` over the generators returned zero hits.
fn gen_cloning(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let subject = *r.pick(&[
        "new P()",
        "new Q()",
        "new R(1)",
        "$arr",
        "5",
        "null",
        "\"s\"",
        "$cl",
        "$gen",
        "new stdClass",
        "E::A",
    ]);
    let after = *r.pick(&[
        "var_dump($a == $b, $a === $b);",
        "$b->n = 9; var_dump($a->n ?? \"none\", $b->n ?? \"none\");",
        "$b->list[] = 3; var_dump($a->list ?? \"none\", $b->list ?? \"none\");",
        "$b->inner->k = 9; var_dump($a->inner->k ?? \"none\");",
        "var_dump(get_object_vars($b));",
        "try { $b->ro = 5; } catch (\\Throwable $e) { echo get_class($e), \": \", $e->getMessage(); }",
        "var_dump(get_class($b));",
    ]);
    vec![format!(
        "class Inner {{ public $k = 1; }} \
         class P {{ public $n = 1; public $list = [1, 2]; public $inner; \
                    function __construct() {{ $this->inner = new Inner(); }} }} \
         class Q extends P {{ public $n = 2; function __clone() {{ echo \"CL\"; $this->n++; }} }} \
         class R {{ function __construct(public readonly int $ro) {{}} }} \
         enum E {{ case A; }} \
         $arr = [1, 2]; $cl = fn() => 1; $gen = (function () {{ yield 1; }})(); \
         $a = {subject}; \
         try {{ $b = clone $a; }} catch (\\Throwable $e) {{ echo get_class($e), \": \", $e->getMessage(); return; }} \
         {after}"
    )]
}

/// `__call` / `__callStatic` — the magic dispatch that catches a method name
/// the class does not declare.
///
/// A grep for both returned zero hits: `propmagic` scores `__get`/`__set` and
/// nothing scored the method half.
fn gen_magiccall(seed: u64) -> Vec<String> {
    let r = &mut Rng::seed(seed);
    let call = *r.pick(&[
        "$o->missing()",
        "$o->missing(1, \"a\")",
        "$o->declared()",
        "$o->missing(...[1, 2])",
        "$o->missing(k: 1)",
        "M::missing()",
        "M::missing(1, 2)",
        "M::declaredStatic()",
        "N::missing()",
        "$o->privateOne()",
        "call_user_func([$o, \"missing\"], 1)",
        "call_user_func(\"M::missing\", 1)",
        "array_map([$o, \"missing\"], [1, 2])",
        "is_callable([$o, \"missing\"])",
        "method_exists($o, \"missing\")",
        "$o(3)",
    ]);
    let cls = *r.pick(&["M", "N"]);
    vec![format!(
        "class M {{ \
            function __call($n, $a) {{ echo \"C:$n/\", count($a), \"|\"; return $n; }} \
            static function __callStatic($n, $a) {{ echo \"S:$n/\", count($a), \"|\"; return $n; }} \
            function __invoke($x) {{ return $x * 2; }} \
            function declared() {{ return \"D\"; }} \
            static function declaredStatic() {{ return \"DS\"; }} \
            private function privateOne() {{ return \"P\"; }} }} \
         class N {{}} \
         $o = new {cls}(); \
         try {{ var_dump({call}); }} catch (\\Throwable $e) {{ echo get_class($e), \": \", $e->getMessage(); }}"
    )]
}

#[derive(Clone, Copy)]
struct Mode {
    name: &'static str,
    gen: fn(u64) -> Vec<String>,
}

const MODES: &[Mode] = &[
    Mode {
        name: "funcargs",
        gen: gen_funcargs,
    },
    Mode {
        name: "extractflags",
        gen: gen_extractflags,
    },
    Mode {
        name: "calleeforms",
        gen: gen_calleeforms,
    },
    Mode {
        name: "fcc",
        gen: gen_fcc,
    },
    Mode {
        name: "sscanf",
        gen: gen_sscanf,
    },
    Mode {
        name: "cslashes",
        gen: gen_cslashes,
    },
    Mode {
        name: "strtokcounts",
        gen: gen_strtok_counts,
    },
    Mode {
        name: "substrx",
        gen: gen_substrx,
    },
    Mode {
        name: "arrayfold",
        gen: gen_arrayfold,
    },
    Mode {
        name: "jsondecode",
        gen: gen_jsondecode,
    },
    Mode {
        name: "htmlent",
        gen: gen_htmlent,
    },
    Mode {
        name: "striptags",
        gen: gen_striptags,
    },
    Mode {
        name: "compact",
        gen: gen_compact,
    },
    Mode {
        name: "callform",
        gen: gen_callform,
    },
    Mode {
        name: "parseurl",
        gen: gen_parseurl,
    },
    Mode {
        name: "pregoffset",
        gen: gen_pregoffset,
    },
    Mode {
        name: "destructure",
        gen: gen_destructure,
    },
    Mode {
        name: "nsconst",
        gen: gen_nsconst,
    },
    Mode {
        name: "numjuggle",
        gen: gen_numjuggle,
    },
    Mode {
        name: "arith",
        gen: gen_arith,
    },
    Mode {
        name: "intdiv",
        gen: gen_intdiv,
    },
    Mode {
        name: "floatfmt",
        gen: gen_floatfmt,
    },
    Mode {
        name: "pow",
        gen: gen_pow,
    },
    Mode {
        name: "concat",
        gen: gen_concat,
    },
    Mode {
        name: "compare",
        gen: gen_compare,
    },
    Mode {
        name: "ternary",
        gen: gen_ternary,
    },
    Mode {
        name: "strfns",
        gen: gen_strfns,
    },
    Mode {
        name: "arrays",
        gen: gen_arrays,
    },
    Mode {
        name: "sorting",
        gen: gen_sorting,
    },
    Mode {
        name: "assoc",
        gen: gen_assoc,
    },
    Mode {
        name: "printf",
        gen: gen_printf,
    },
    Mode {
        name: "control",
        gen: gen_control,
    },
    Mode {
        name: "loops",
        gen: gen_loops,
    },
    Mode {
        name: "funcs",
        gen: gen_funcs,
    },
    Mode {
        name: "typeconv",
        gen: gen_typeconv,
    },
    Mode {
        name: "mathfns",
        gen: gen_mathfns,
    },
    Mode {
        name: "exprtree",
        gen: gen_exprtree,
    },
    Mode {
        name: "unary",
        gen: gen_unary,
    },
    Mode {
        name: "sprintf_rich",
        gen: gen_sprintf_rich,
    },
    Mode {
        name: "numedge",
        gen: gen_numedge,
    },
    Mode {
        name: "stredge",
        gen: gen_stredge,
    },
    Mode {
        name: "arraypipe",
        gen: gen_arraypipe,
    },
    Mode {
        name: "multi",
        gen: gen_multi,
    },
    Mode {
        name: "bitwise",
        gen: gen_bitwise,
    },
    Mode {
        name: "spaceship",
        gen: gen_spaceship,
    },
    Mode {
        name: "stroffset",
        gen: gen_stroffset,
    },
    Mode {
        name: "coalesce",
        gen: gen_coalesce,
    },
    Mode {
        name: "str2",
        gen: gen_str2,
    },
    Mode {
        name: "arr2",
        gen: gen_arr2,
    },
    Mode {
        name: "math2",
        gen: gen_math2,
    },
    Mode {
        name: "refs",
        gen: gen_refs,
    },
    Mode {
        name: "closures",
        gen: gen_closures,
    },
    Mode {
        name: "exc",
        gen: gen_exc,
    },
    Mode {
        name: "typejug2",
        gen: gen_typejug2,
    },
    Mode {
        name: "range",
        gen: gen_range,
    },
    Mode {
        name: "datefmt",
        gen: gen_datefmt,
    },
    Mode {
        name: "printf2",
        gen: gen_printf2,
    },
    Mode {
        name: "arr3",
        gen: gen_arr3,
    },
    Mode {
        name: "rounding",
        gen: gen_rounding,
    },
    Mode {
        name: "dynprop",
        gen: gen_dynprop,
    },
    Mode {
        name: "attributes",
        gen: gen_attributes,
    },
    Mode {
        name: "errlevel",
        gen: gen_errlevel,
    },
    Mode {
        name: "libargerr",
        gen: gen_libargerr,
    },
    Mode {
        name: "pregerr",
        gen: gen_pregerr,
    },
    Mode {
        name: "pregfancy",
        gen: gen_pregfancy,
    },
    Mode {
        name: "propmagic",
        gen: gen_propmagic,
    },
    Mode {
        name: "ini",
        gen: gen_ini,
    },
    Mode {
        name: "stricttypes",
        gen: gen_stricttypes,
    },
    Mode {
        name: "declaresyntax",
        gen: gen_declaresyntax,
    },
    Mode {
        name: "exitdie",
        gen: gen_exitdie,
    },
    Mode {
        name: "heredoc",
        gen: gen_heredoc,
    },
    Mode {
        name: "nullsafechain",
        gen: gen_nullsafechain,
    },
    Mode {
        name: "matcherr",
        gen: gen_matcherr,
    },
    Mode {
        name: "ifaceconst",
        gen: gen_ifaceconst,
    },
    Mode {
        name: "generators",
        gen: gen_generators,
    },
    Mode {
        name: "enums",
        gen: gen_enums,
    },
    Mode {
        name: "traits",
        gen: gen_traits,
    },
    Mode {
        name: "variadic",
        gen: gen_variadic,
    },
    Mode {
        name: "splobj",
        gen: gen_splobj,
    },
    Mode {
        name: "altsyntax",
        gen: gen_altsyntax,
    },
    Mode {
        name: "printexpr",
        gen: gen_printexpr,
    },
    Mode {
        name: "reflect",
        gen: gen_reflect,
    },
    Mode {
        name: "issetform",
        gen: gen_issetform,
    },
    Mode {
        name: "callorder",
        gen: gen_callorder,
    },
    Mode {
        name: "cloning",
        gen: gen_cloning,
    },
    Mode {
        name: "magiccall",
        gen: gen_magiccall,
    },
];

fn build_program(stmts: &[String]) -> String {
    stmts.join("\n")
}

/// The seed of case `i` under `base` — the whole identity of a case, and what a
/// divergence records.
///
/// The CASE SEED is reported, not the index `i`. They are not interchangeable:
/// `--once --seed S` builds its case from `S` directly, so replaying a reported
/// index produced an unrelated program under an unrelated mode, and the
/// "replays exactly" promise at the top of this file was false for every
/// divergence this harness has ever printed.
fn case_seed(base: u64, i: u64) -> u64 {
    let mut z = base ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z ^ (z >> 31)
}

/// A case from its seed → (mode, program), a pure function so any divergence
/// replays from its seed alone.
///
/// `only` forces the mode (`--mode NAME`), which the seed otherwise chooses.
/// Forcing it is what makes `--mode` mean "generate this mode's cases" rather
/// than "generate every mode's cases and throw away all but one in
/// `MODES.len()`" — under the old filter a `--count 2000` run of one mode
/// compared about 27 programs and reported the rest as never having run.
fn case_from_seed(seed: u64, only: Option<Mode>) -> (Mode, Vec<String>) {
    let mode = only.unwrap_or(MODES[(seed >> 7) as usize % MODES.len()]);
    (mode, (mode.gen)(seed))
}

/// Why a case produced no verdict. A skip is not a pass, and counting it as one
/// is how a fuzz run reports a clean number while testing nothing: a mode whose
/// programs all time out on the reference scores zero divergences forever.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Skip {
    /// The reference did not finish in time — nothing to compare against.
    OracleTimeout,
    /// The reference could not be spawned or read.
    OracleInfra,
    /// Our binary did not finish in time.
    OursTimeout,
    /// Our binary could not be spawned or read.
    OursInfra,
}

impl Skip {
    fn label(self) -> &'static str {
        match self {
            Skip::OracleTimeout => "oracle timed out",
            Skip::OracleInfra => "oracle failed to run",
            Skip::OursTimeout => "ours timed out",
            Skip::OursInfra => "ours failed to run",
        }
    }
}

/// The verdict for one case.
enum Verdict {
    Same,
    Diverged(Box<(RunOut, RunOut)>),
    Skipped(Skip),
    /// Both sides agreed, but the REFERENCE printed nothing. The case ran and
    /// matched, so it is not a skip — but it proves nothing about the behaviour
    /// it was written to exercise, and two *different* silences look identical
    /// here. Counted separately so a mode cannot hide behind them.
    ///
    /// The test is on stdout alone, not on stdout AND a failing exit: a program
    /// that exits 0 having echoed nothing is exactly as empty a comparison as
    /// one that died, and counting it as a pass is how a mode scores clean while
    /// asserting nothing.
    Barren,
}

fn judge(script: &str, bin: &Path, timeout: Duration) -> Verdict {
    let o = run_oracle(script, timeout);
    if o.timed_out {
        return Verdict::Skipped(Skip::OracleTimeout);
    }
    if o.infra_fail {
        return Verdict::Skipped(Skip::OracleInfra);
    }
    let r = run_ours(script, bin, timeout);
    if r.timed_out {
        return Verdict::Skipped(Skip::OursTimeout);
    }
    if r.infra_fail {
        return Verdict::Skipped(Skip::OursInfra);
    }
    if differs(&o, &r) {
        return Verdict::Diverged(Box::new((o, r)));
    }
    if o.stdout.is_empty() {
        return Verdict::Barren;
    }
    Verdict::Same
}

fn diverges(script: &str, bin: &Path, timeout: Duration) -> Option<(RunOut, RunOut)> {
    let o = run_oracle(script, timeout);
    if o.timed_out || o.infra_fail {
        return None;
    }
    let r = run_ours(script, bin, timeout);
    if r.timed_out || r.infra_fail {
        return None;
    }
    if differs(&o, &r) {
        Some((o, r))
    } else {
        None
    }
}

/// Delta-debug a diverging statement list toward a locally-minimal one.
fn minimize(stmts: Vec<String>, bin: &Path, timeout: Duration) -> Vec<String> {
    let mut cur = stmts;
    loop {
        let mut removed = false;
        let mut i = 0;
        while i < cur.len() {
            let mut cand = cur.clone();
            cand.remove(i);
            if !cand.is_empty() && diverges(&build_program(&cand), bin, timeout).is_some() {
                cur = cand;
                removed = true;
            } else {
                i += 1;
            }
        }
        if !removed {
            break;
        }
    }
    cur
}

/// Mask numeric/quoted literals so many instances of one gap collapse to a class.
fn signature(mode: &str, program: &str) -> String {
    let line = program
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .unwrap_or("");
    let mut out = String::with_capacity(line.len());
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'"' || c == b'\'' {
            let q = c;
            out.push('W');
            i += 1;
            while i < bytes.len() && bytes[i] != q {
                i += 1;
            }
            i += 1; // closing quote
        } else if c.is_ascii_digit() {
            out.push('N');
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
        } else {
            out.push(c as char);
            i += 1;
        }
    }
    format!("[{mode}] {out}")
}

// ---------------------------------------------------------------------------
// A recorded divergence.
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Divergence {
    seed: u64,
    mode: &'static str,
    program: String,
    oracle_out: String,
    /// The `log_errors` copy of whatever diagnostics the reference raised. Kept
    /// beside stdout rather than folded into it, because which STREAM a line
    /// landed on is itself the thing under test.
    oracle_err: String,
    oracle_exit: i32,
    ours_out: String,
    ours_err: String,
    ours_exit: i32,
    signature: String,
}

// ---------------------------------------------------------------------------
// CLI.
// ---------------------------------------------------------------------------

struct Args {
    count: u64,
    base_seed: u64,
    jobs: usize,
    timeout: Duration,
    once: Option<u64>,
    mode: Option<String>,
    show: usize,
}

fn parse_args() -> Args {
    let mut a = Args {
        count: 2000,
        base_seed: 0,
        jobs: std::thread::available_parallelism()
            .map(|n| n.get().min(8))
            .unwrap_or(4),
        timeout: Duration::from_millis(5000),
        once: None,
        mode: None,
        show: 10,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--count" => a.count = it.next().and_then(|s| s.parse().ok()).unwrap_or(a.count),
            "--seed" => a.base_seed = it.next().and_then(|s| s.parse().ok()).unwrap_or(0),
            "--jobs" => {
                a.jobs = it
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(a.jobs)
                    .max(1)
            }
            "--timeout" => {
                a.timeout = it
                    .next()
                    .and_then(|s| s.parse().ok())
                    .map(Duration::from_millis)
                    .unwrap_or(a.timeout)
            }
            "--once" => a.once = Some(1),
            "--mode" => a.mode = it.next().cloned(),
            "--show" => a.show = it.next().and_then(|s| s.parse().ok()).unwrap_or(a.show),
            "-h" | "--help" => {
                println!("parity-fuzz [--count N] [--seed S] [--jobs J] [--timeout MS] [--mode NAME] [--once --seed S [--mode NAME]] [--show N]");
                std::process::exit(0);
            }
            other => {
                eprintln!("parity-fuzz: unknown arg {other}");
                std::process::exit(2);
            }
        }
    }
    a
}

fn main() {
    let args = parse_args();
    let bin = ours_bin();
    if !bin.exists() {
        eprintln!(
            "parity-fuzz: phplang binary not found at {} (run `cargo build` first)",
            bin.display()
        );
        std::process::exit(2);
    }

    // The forced mode, resolved once: `--mode` names it and `--once` honours
    // the same name, so a divergence found under a filter replays under it.
    let forced = match &args.mode {
        Some(m) => match MODES.iter().find(|md| md.name == m) {
            Some(md) => Some(*md),
            None => {
                eprintln!("parity-fuzz: unknown mode {m}");
                std::process::exit(2);
            }
        },
        None => None,
    };

    // `--once --seed S [--mode NAME]`: replay one case, print both sides, exit.
    if args.once.is_some() {
        let (mode, stmts) = case_from_seed(args.base_seed, forced);
        let prog = build_program(&stmts);
        let o = run_oracle(&prog, args.timeout);
        let r = run_ours(&prog, &bin, args.timeout);
        // Named here as well as in a full run: a replay is what gets pasted
        // into a bug report, and a transcript that does not say which build
        // answered cannot be checked by anyone else.
        println!("oracle bin: {}", oracle_id());
        println!("ours bin: {}", bin.display());
        println!("seed  : {}", args.base_seed);
        println!("mode  : {}", mode.name);
        println!("prog  : {prog}");
        println!(
            "oracle: exit={} out={:?} err={:?}",
            o.exit,
            render(&o.stdout),
            render(&o.stderr)
        );
        println!(
            "ours  : exit={} out={:?} err={:?}",
            r.exit,
            render(&r.stdout),
            render(&r.stderr)
        );
        println!("differ: {}", differs(&o, &r));
        return;
    }

    eprintln!("parity-fuzz: oracle = {}", oracle_id());
    eprintln!("parity-fuzz: ours   = {}", bin.display());
    eprintln!(
        "parity-fuzz: {} cases, {} jobs, base seed {}",
        args.count, args.jobs, args.base_seed
    );

    let next = Arc::new(AtomicUsize::new(0));
    let divergences = Arc::new(Mutex::new(Vec::<Divergence>::new()));
    let ran = Arc::new(AtomicUsize::new(0));
    let start = Instant::now();
    // A skip is not a pass: counted and reported separately so a mode cannot
    // score zero divergences while testing nothing.
    let barren: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let skipped: Arc<Mutex<Vec<(&'static str, Skip)>>> = Arc::new(Mutex::new(Vec::new()));
    // Cases that agreed on a NON-empty reference output — the only bucket that
    // is evidence of anything. Counted so the four buckets can be reconciled
    // against `ran`: a case that reached a worker and landed in none of them was
    // lost, and a lost case must not shrink the denominator in silence.
    let scored = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();
    for _ in 0..args.jobs {
        let next = Arc::clone(&next);
        let divergences = Arc::clone(&divergences);
        let ran = Arc::clone(&ran);
        let barren = Arc::clone(&barren);
        let scored = Arc::clone(&scored);
        let skipped = Arc::clone(&skipped);
        let bin = bin.clone();
        let timeout = args.timeout;
        let base = args.base_seed;
        let count = args.count;
        handles.push(std::thread::spawn(move || loop {
            let i = next.fetch_add(1, Ordering::Relaxed) as u64;
            if i >= count {
                break;
            }
            let seed = case_seed(base, i);
            let (mode, stmts) = case_from_seed(seed, forced);
            ran.fetch_add(1, Ordering::Relaxed);
            let prog = build_program(&stmts);
            let verdict = judge(&prog, &bin, timeout);
            let (o, r) = match verdict {
                Verdict::Same => {
                    scored.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                Verdict::Barren => {
                    barren.lock().unwrap().push(mode.name);
                    continue;
                }
                Verdict::Skipped(why) => {
                    skipped.lock().unwrap().push((mode.name, why));
                    continue;
                }
                Verdict::Diverged(pair) => *pair,
            };
            {
                let min = minimize(stmts.clone(), &bin, timeout);
                let min_prog = build_program(&min);
                // Recompute both sides on the minimized reproducer for the report.
                let om = run_oracle(&min_prog, timeout);
                let rm = run_ours(&min_prog, &bin, timeout);
                let (o, r) = if differs(&om, &rm) { (om, rm) } else { (o, r) };
                let sig = signature(mode.name, &min_prog);
                divergences.lock().unwrap().push(Divergence {
                    // The case SEED, which is what `--once --seed <it>` takes.
                    seed,
                    mode: mode.name,
                    program: min_prog,
                    oracle_out: render(&o.stdout),
                    oracle_err: render(&o.stderr),
                    oracle_exit: o.exit,
                    ours_out: render(&r.stdout),
                    ours_err: render(&r.stderr),
                    ours_exit: r.exit,
                    signature: sig,
                });
            }
        }));
    }
    // A worker that panics takes its in-flight case with it. Join errors are
    // counted rather than discarded, because the alternative is a run that
    // reports a smaller corpus than it was asked for and calls it clean.
    let mut dead_workers = 0usize;
    for h in handles {
        if h.join().is_err() {
            dead_workers += 1;
        }
    }

    let mut divs = Arc::try_unwrap(divergences).unwrap().into_inner().unwrap();
    divs.sort_by(|a, b| a.signature.cmp(&b.signature).then(a.seed.cmp(&b.seed)));

    let ran = ran.load(Ordering::Relaxed);
    let elapsed = start.elapsed();

    // Group by signature for the summary.
    let mut classes: Vec<(&str, usize, &Divergence)> = Vec::new();
    for d in &divs {
        match classes.iter_mut().find(|(s, _, _)| *s == d.signature) {
            Some((_, n, _)) => *n += 1,
            None => classes.push((&d.signature, 1, d)),
        }
    }

    let barren = Arc::try_unwrap(barren).unwrap().into_inner().unwrap();
    let skipped = Arc::try_unwrap(skipped).unwrap().into_inner().unwrap();

    println!("\n=== parity-fuzz summary ===");
    println!("ran        : {ran} cases in {:.1}s", elapsed.as_secs_f64());
    println!(
        "compared   : {} (both sides produced a verdict)",
        ran.saturating_sub(skipped.len())
    );
    println!(
        "divergences: {} ({} distinct gap classes)",
        divs.len(),
        classes.len()
    );
    // Reported even at zero: "skipped: 0" is the evidence that a clean
    // divergence count was earned rather than an artefact of cases that never
    // reached a comparison.
    println!("skipped    : {}", skipped.len());
    for why in [
        Skip::OracleTimeout,
        Skip::OracleInfra,
        Skip::OursTimeout,
        Skip::OursInfra,
    ] {
        let n = skipped.iter().filter(|(_, w)| *w == why).count();
        if n > 0 {
            println!("  {n:>6}  {}", why.label());
            // Which modes are affected matters more than the total: a skip
            // concentrated in one mode means that mode is measuring nothing.
            let mut by_mode: Vec<(&str, usize)> = Vec::new();
            for (m, _) in skipped.iter().filter(|(_, w)| *w == why) {
                match by_mode.iter_mut().find(|(name, _)| name == m) {
                    Some((_, c)) => *c += 1,
                    None => by_mode.push((m, 1)),
                }
            }
            by_mode.sort_by_key(|c| std::cmp::Reverse(c.1));
            for (m, n) in by_mode.iter().take(5) {
                println!("          {n:>5}x {m}");
            }
        }
    }
    println!(
        "barren     : {} (agreed, but the reference produced no stdout — \
         proves nothing)",
        barren.len()
    );
    // BY MODE, because the total alone cannot be acted on. A mode that is
    // barren for a few seeds has an `echo` whose argument happened to be empty;
    // a mode that is barren for a large share of its cases is emitting programs
    // with no output construct at all, which is a generator bug rather than a
    // parity result. Only this breakdown tells the two apart.
    if !barren.is_empty() {
        let mut by_mode: Vec<(&str, usize)> = Vec::new();
        for m in &barren {
            match by_mode.iter_mut().find(|(name, _)| name == m) {
                Some((_, n)) => *n += 1,
                None => by_mode.push((m, 1)),
            }
        }
        by_mode.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        let ran_by_mode = |name: &str| {
            (0..args.count)
                .filter(|i| {
                    forced.map_or_else(
                        || case_from_seed(case_seed(args.base_seed, *i), None).0.name,
                        |m| m.name,
                    ) == name
                })
                .count()
        };
        println!("  barren by mode (share of that mode's cases):");
        for (name, n) in by_mode {
            let total = ran_by_mode(name);
            let pct = if total == 0 {
                0.0
            } else {
                100.0 * n as f64 / total as f64
            };
            println!("    {name:<12} {n:>5} of {total:<6} ({pct:.1}%)");
        }
    }
    let scored = scored.load(Ordering::Relaxed);
    println!("scored     : {scored} (agreed on a non-empty reference output)");
    // Reconcile: every case handed to a worker must have landed in exactly one
    // bucket. Anything unaccounted for was lost — a panicked worker, or a
    // verdict arm that forgot to count itself — and a lost case silently makes
    // the corpus smaller than the one the run claims to have covered.
    let missing = ran
        .saturating_sub(scored)
        .saturating_sub(barren.len())
        .saturating_sub(skipped.len())
        .saturating_sub(divs.len());
    if missing > 0 || dead_workers > 0 {
        println!("unaccounted: {missing} cases, {dead_workers} workers died");
    }

    // The exit status answers "did this run measure what it was asked to?", not
    // just "did it find a disagreement?". Every arm below is a run that proved
    // nothing while a plain divergence count would have read as clean:
    //   - no cases ran at all (a mode filter that matches nothing, --count 0);
    //   - every case that ran was skipped (the reference timing out on all of
    //     them is the textbook way a frontend reports 0 divergences forever);
    //   - a case reached a worker and produced no verdict;
    //   - the reference printed nothing, so the two sides "agreed" on silence.
    let compared = ran.saturating_sub(skipped.len());
    let mut faults: Vec<String> = Vec::new();
    if !divs.is_empty() {
        faults.push(format!("{} divergences", divs.len()));
    }
    if ran == 0 {
        faults.push("no cases ran".into());
    } else if scored == 0 {
        faults.push("no case produced a usable comparison".into());
    }
    if compared == 0 && ran > 0 {
        faults.push("every case that ran was skipped".into());
    }
    if !skipped.is_empty() {
        faults.push(format!("{} skipped", skipped.len()));
    }
    if !barren.is_empty() {
        faults.push(format!("{} barren", barren.len()));
    }
    if missing > 0 || dead_workers > 0 {
        faults.push(format!(
            "{missing} unaccounted, {dead_workers} dead workers"
        ));
    }

    if !divs.is_empty() {
        let n = args.show.min(divs.len());
        println!("\n--- first {n} divergences ---");
        for d in divs.iter().take(n) {
            println!("\n[seed {}] mode={}", d.seed, d.mode);
            println!("  prog  : {}", d.program.replace('\n', " ⏎ "));
            println!(
                "  oracle: exit={} out={:?} err={:?}",
                d.oracle_exit, d.oracle_out, d.oracle_err
            );
            println!(
                "  ours  : exit={} out={:?} err={:?}",
                d.ours_exit, d.ours_out, d.ours_err
            );
        }
        println!("\n--- gap classes (by frequency) ---");
        let mut sorted = classes.clone();
        sorted.sort_by_key(|c| std::cmp::Reverse(c.1));
        for (sig, n, ex) in sorted {
            println!("  {n:>4}x  {sig}");
            println!(
                "          e.g. oracle={:?}/{:?}/{} ours={:?}/{:?}/{}",
                ex.oracle_out,
                ex.oracle_err,
                ex.oracle_exit,
                ex.ours_out,
                ex.ours_err,
                ex.ours_exit
            );
        }

        // Report file, named for THIS process. A constant name is a collision:
        // several agents run this harness against the same checkout at once, and
        // a shared name means the report you open belongs to whichever run
        // finished last. Nothing here is ever deleted, for the same reason.
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("parity-fuzz");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("divergences-{}.txt", std::process::id()));
        let mut report = String::new();
        report.push_str(&format!("oracle: {}\n", oracle_id()));
        report.push_str(&format!(
            "ran {ran} cases, {} divergences, {} classes\n\n",
            divs.len(),
            classes.len()
        ));
        for d in &divs {
            report.push_str(&format!(
                "[seed {}] mode={} sig={}\n",
                d.seed, d.mode, d.signature
            ));
            report.push_str(&format!("  prog  : {}\n", d.program.replace('\n', " ; ")));
            report.push_str(&format!(
                "  oracle: exit={} out={:?} err={:?}\n",
                d.oracle_exit, d.oracle_out, d.oracle_err
            ));
            report.push_str(&format!(
                "  ours  : exit={} out={:?} err={:?}\n\n",
                d.ours_exit, d.ours_out, d.ours_err
            ));
        }
        use std::io::Write;
        if let Ok(mut f) = std::fs::File::create(&path) {
            let _ = f.write_all(report.as_bytes());
            println!("\nfull report: {}", path.display());
        }
    }

    if faults.is_empty() {
        println!(
            "\n{scored} cases compared clean — phplang matches {} on this corpus.",
            oracle_path()
        );
        return;
    }
    println!("\nRUN NOT CLEAN: {}", faults.join("; "));
    std::process::exit(1);
}
