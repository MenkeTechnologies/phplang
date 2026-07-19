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
//! nothing. Every program prints something deterministic so an empty-vs-empty run
//! can never hide a gap. No `rand`, no time, no object ids — nothing whose output
//! is nondeterministic for reasons unrelated to parity.
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
    // `-d error_reporting=0` silences PHP notices/warnings so a deprecation note
    // on stderr never perturbs the run; we compare stdout + success only.
    cmd.args(["-d", "error_reporting=0", "-r", script]);
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
    match r.below(6) {
        0 => vec![format!("echo count({arr});")],
        1 => vec![format!("echo array_sum({arr});")],
        2 => vec![format!("echo implode(\",\", {arr});")],
        3 => vec![format!("echo implode(\",\", array_reverse({arr}));")],
        4 => vec![format!("echo in_array({}, {arr}) ? \"y\" : \"n\";", ii(r))],
        _ => vec![format!("echo array_product({arr});")],
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

    let mut handles = Vec::new();
    for _ in 0..args.jobs {
        let next = Arc::clone(&next);
        let divergences = Arc::clone(&divergences);
        let ran = Arc::clone(&ran);
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
            if let Some((o, r)) = diverges(&prog, &bin, timeout) {
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
    for h in handles {
        let _ = h.join();
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

    println!("\n=== parity-fuzz summary ===");
    println!("ran        : {ran} cases in {:.1}s", elapsed.as_secs_f64());
    println!(
        "divergences: {} ({} distinct gap classes)",
        divs.len(),
        classes.len()
    );

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

        // Report file.
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("parity-fuzz");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("divergences.txt");
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
        std::process::exit(1);
    } else {
        println!(
            "\nno divergences — phplang matches {} on this corpus.",
            oracle_path()
        );
    }
}
