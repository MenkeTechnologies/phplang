//! Differential parity fuzzer: reference `php -r <s>` vs phplang `php -r <s>`.
//!
//! Generates thousands of grammar-driven, deterministic-output PHP snippets, runs
//! each through both interpreters, and reports every case where stdout diverges
//! or one errors while the other does not. Each case is produced from a per-index
//! seed so any divergence replays exactly: `parity-fuzz --once --seed <N>`.
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
//! `$x = ""; $x ?? "d"`). Measured over a 62k run, 248 cases in SEVEN of the 54
//! modes — unary, coalesce, str2, strfns, stredge, arr3, closures — with no mode
//! above 6.0% of its own cases. All incidental; none is a mode whose programs
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

/// The divergence predicate: stdout must match exactly, and the two runs must
/// agree on success-vs-failure. Exact exit CODES are NOT compared — reference PHP
/// exits 255 on a fatal error while phplang exits 1, which is not a parity gap.
fn differs(a: &RunOut, b: &RunOut) -> bool {
    if a.stdout != b.stdout {
        return true;
    }
    (a.exit == 0) != (b.exit == 0)
}

/// Spawn `cmd` and wait up to `timeout`, killing it if it overruns.
fn run_with_timeout(mut cmd: Command, timeout: Duration) -> RunOut {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => {
            return RunOut {
                stdout: Vec::new(),
                exit: -999,
                timed_out: false,
                infra_fail: true,
            }
        }
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                use std::io::Read;
                let mut buf = Vec::new();
                if let Some(mut out) = child.stdout.take() {
                    let _ = out.read_to_end(&mut buf);
                }
                return RunOut {
                    stdout: buf,
                    exit: status.code().unwrap_or(-1),
                    timed_out: false,
                    infra_fail: false,
                };
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return RunOut {
                        stdout: Vec::new(),
                        exit: -1,
                        timed_out: true,
                        infra_fail: false,
                    };
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(_) => {
                return RunOut {
                    stdout: Vec::new(),
                    exit: -998,
                    timed_out: false,
                    infra_fail: true,
                }
            }
        }
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
        4 => vec![format!(
            "echo implode(\",\", array_unique([1, 1, 2, 2, 3]));"
        )],
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
        3 => vec![format!("echo str_word_count(\"one two three four\");")],
        4 => vec![format!("echo strrpos(\"{s}{s}\", \"{}\");", &s[..1])],
        5 => vec![format!(
            "echo stripos(\"{s}\", \"{}\");",
            &s[..1].to_uppercase()
        )],
        6 => vec![format!("echo addslashes(\"a'b\\\"c\");")],
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
        14 => vec![format!("echo nl2br(\"a\\nb\");")],
        _ => vec![format!("echo quotemeta(\"a.b*c\");")],
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
        13 => vec![format!(
            "echo json_encode(array_diff_key([\"a\" => 1, \"b\" => 2], [\"a\" => 9]));"
        )],
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
        4 => vec![format!(
            "enum E {{ #[Attr] case A; #[Attr] case B; }} echo E::A->name, count(E::cases());"
        )],
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
        0 => vec![format!("echo error_reporting();")],
        1 => vec![format!(
            "var_dump(error_reporting({lvl})); echo error_reporting();"
        )],
        2 => vec![format!(
            "error_reporting({lvl}); echo $undef; echo \"end\";"
        )],
        3 => vec![format!(
            "error_reporting({lvl}); class C {{}} $c = new C; $c->p = 1; echo \"end\";"
        )],
        4 => vec![format!(
            "var_dump(ini_get(\"error_reporting\")); ini_set(\"error_reporting\", \"0\"); \
             echo $undef; var_dump(ini_get(\"error_reporting\"));"
        )],
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

// ---------------------------------------------------------------------------
// Mode registry.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Mode {
    name: &'static str,
    gen: fn(u64) -> Vec<String>,
}

const MODES: &[Mode] = &[
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
];

fn build_program(stmts: &[String]) -> String {
    stmts.join("\n")
}

/// Case `i` under `base` seed → (mode, program), a pure function so any
/// divergence replays from its seed alone.
fn case_for(base: u64, i: u64) -> (Mode, Vec<String>) {
    let case_seed = {
        let mut z = base ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z ^ (z >> 31)
    };
    let mode = MODES[(case_seed >> 7) as usize % MODES.len()];
    (mode, (mode.gen)(case_seed))
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
    oracle_ok: bool,
    ours_out: String,
    ours_ok: bool,
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
                println!("parity-fuzz [--count N] [--seed S] [--jobs J] [--timeout MS] [--mode NAME] [--once --seed S] [--show N]");
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

    // `--once --seed S`: replay one case, print both sides, exit.
    if args.once.is_some() {
        let (mode, stmts) = case_for(args.base_seed, 0);
        let prog = build_program(&stmts);
        let o = run_oracle(&prog, args.timeout);
        let r = run_ours(&prog, &bin, args.timeout);
        println!("seed  : {}", args.base_seed);
        println!("mode  : {}", mode.name);
        println!("prog  : {prog}");
        println!("oracle: exit={} {:?}", o.exit, render(&o.stdout));
        println!("ours  : exit={} {:?}", r.exit, render(&r.stdout));
        println!("differ: {}", differs(&o, &r));
        return;
    }

    let mode_filter = args.mode.clone();
    if let Some(m) = &mode_filter {
        if !MODES.iter().any(|md| md.name == m) {
            eprintln!("parity-fuzz: unknown mode {m}");
            std::process::exit(2);
        }
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
        let mode_filter = mode_filter.clone();
        handles.push(std::thread::spawn(move || loop {
            let i = next.fetch_add(1, Ordering::Relaxed) as u64;
            if i >= count {
                break;
            }
            let (mode, stmts) = case_for(base, i);
            if let Some(m) = &mode_filter {
                if mode.name != m {
                    continue;
                }
            }
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
                    seed: {
                        // Store the case index so `--once --seed <i>` replays it.
                        i
                    },
                    mode: mode.name,
                    program: min_prog,
                    oracle_out: render(&o.stdout),
                    oracle_ok: o.exit == 0,
                    ours_out: render(&r.stdout),
                    ours_ok: r.exit == 0,
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
                .filter(|i| case_for(args.base_seed, *i).0.name == name)
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
                "  oracle: {}{:?}",
                if d.oracle_ok { "" } else { "(err) " },
                d.oracle_out
            );
            println!(
                "  ours  : {}{:?}",
                if d.ours_ok { "" } else { "(err) " },
                d.ours_out
            );
        }
        println!("\n--- gap classes (by frequency) ---");
        let mut sorted = classes.clone();
        sorted.sort_by_key(|c| std::cmp::Reverse(c.1));
        for (sig, n, ex) in sorted {
            println!("  {n:>4}x  {sig}");
            println!(
                "          e.g. oracle={:?} ours={:?}",
                ex.oracle_out, ex.ours_out
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
                "  oracle: exit_ok={} {:?}\n",
                d.oracle_ok, d.oracle_out
            ));
            report.push_str(&format!(
                "  ours  : exit_ok={} {:?}\n\n",
                d.ours_ok, d.ours_out
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
