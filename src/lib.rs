//! phplang — PHP as a fusevm frontend.
//!
//! Pipeline: `lexer` → `parser` builds a PHP AST → `compiler` lowers it to a
//! `fusevm::Chunk` (plus a table of function sub-chunks) → fusevm executes it,
//! calling back into the `host` (through registered builtins and the strict
//! numeric hook) for every PHP-specific operation. There is no bespoke VM or
//! JIT here — execution and codegen live in fusevm.

pub mod argtypes;
pub mod ast;
pub mod banner;
pub mod builtins;
pub mod cli;
pub mod compiler;
pub mod corpus;
pub mod dap;
pub mod errlevel;
pub mod host;
pub mod intercepts;
pub mod lexer;
pub mod lsp;
pub mod parser;
pub mod promote;
pub mod repl;
pub mod rust_ffi;
pub mod stdlib;
pub mod tiers;

pub use fusevm::Value;

/// Compile a PHP source string to a runnable program.
pub fn compile(src: &str) -> Result<compiler::Program, String> {
    compile_with_meta(src, false)
}

/// Compile with per-statement DAP line markers enabled (`php --dap`).
pub fn compile_debug(src: &str) -> Result<compiler::Program, String> {
    compile_with_meta(src, true)
}

/// The shared body of [`compile`] and [`compile_debug`].
///
/// The file-level facts the parse turns up have to be applied HERE rather than only
/// on the CLI path: `declare(strict_types=1)` changes how every later call binds its
/// arguments, so a program compiled through the library API would otherwise run in
/// coercive mode however it was written.
fn compile_with_meta(src: &str, debug: bool) -> Result<compiler::Program, String> {
    let (stmts, meta) = parser::parse_meta(src).map_err(|e| e.message)?;
    host::with_host(|h| {
        for (line, msg) in &meta.declare_warnings {
            h.warn_at(msg, *line);
        }
        h.set_strict_types(meta.strict_types);
    });
    compiler::compile(&stmts, debug)
}

/// The built-in exception class hierarchy, written in PHP so it flows through the
/// real class system: `throw`/`catch` resolve these as ordinary classes, and user
/// code can subclass them. `Exception` and `Error` are the two disjoint roots
/// (`catch (Throwable)` — handled in the host — matches either); the rest inherit
/// their `__construct`/`getMessage`/`getCode`/`getPrevious`/`__toString`.
///
/// `file`, `line` and `trace` are filled in by `host::seed_throwable` at
/// construction, exactly when the reference engine records them, so `getFile`,
/// `getLine` and `getTraceAsString` report the `new` site rather than the
/// `throw` site. DIVERGENCE: `getTrace()` (the structured array form) is not
/// provided — only the rendered `getTraceAsString()`.
const EXCEPTION_PRELUDE: &str = r#"<?php
class Exception {
    protected $message = "";
    protected $code = 0;
    protected $previous = null;
    protected $file = "";
    protected $line = 0;
    protected $trace = "";
    public function __construct($message = "", $code = 0, $previous = null) {
        $this->message = $message;
        $this->code = $code;
        $this->previous = $previous;
    }
    public function getMessage() { return $this->message; }
    public function getCode() { return $this->code; }
    public function getPrevious() { return $this->previous; }
    public function getFile() { return $this->file; }
    public function getLine() { return $this->line; }
    public function getTraceAsString() { return $this->trace; }
    public function __toString() { return $this->message; }
}
class Error {
    protected $message = "";
    protected $code = 0;
    protected $previous = null;
    protected $file = "";
    protected $line = 0;
    protected $trace = "";
    public function __construct($message = "", $code = 0, $previous = null) {
        $this->message = $message;
        $this->code = $code;
        $this->previous = $previous;
    }
    public function getMessage() { return $this->message; }
    public function getCode() { return $this->code; }
    public function getPrevious() { return $this->previous; }
    public function getFile() { return $this->file; }
    public function getLine() { return $this->line; }
    public function getTraceAsString() { return $this->trace; }
    public function __toString() { return $this->message; }
}
class RuntimeException extends Exception {}
class LogicException extends Exception {}
class InvalidArgumentException extends LogicException {}
class ArithmeticError extends Error {}
class DivisionByZeroError extends ArithmeticError {}
class TypeError extends Error {}
class ArgumentCountError extends TypeError {}
class ValueError extends Error {}
class JsonException extends Exception {}
class UnhandledMatchError extends Error {}
"#;

/// The date/time classes, written in PHP over the `date`/`strtotime`/`mktime`
/// standard-library functions (UTC, like those functions). Appended to the
/// prelude source (no `<?php` tag — it is concatenated after the exceptions).
const DATETIME_PRELUDE: &str = r#"
class DateInterval {
    public $y = 0;
    public $m = 0;
    public $d = 0;
    public $h = 0;
    public $i = 0;
    public $s = 0;
    public $f = 0;
    public $invert = 0;
    public $days = false;
    public function __construct($spec = "") {
        $len = strlen($spec);
        $in_time = false;
        $num = "";
        for ($p = 0; $p < $len; $p++) {
            $ch = $spec[$p];
            if ($ch === "P") { continue; }
            if ($ch === "T") { $in_time = true; continue; }
            if (ctype_digit($ch)) { $num = $num . $ch; continue; }
            $val = (int) $num;
            $num = "";
            if ($ch === "Y") { $this->y = $val; }
            elseif ($ch === "W") { $this->d = $val * 7; }
            elseif ($ch === "D") { $this->d = $val; }
            elseif ($ch === "H") { $this->h = $val; }
            elseif ($ch === "S") { $this->s = $val; }
            elseif ($ch === "M" && $in_time) { $this->i = $val; }
            elseif ($ch === "M") { $this->m = $val; }
        }
    }
    public function _seconds() {
        return $this->s + $this->i * 60 + $this->h * 3600 + $this->d * 86400
            + $this->m * 2592000 + $this->y * 31536000;
    }
    public function format($fmt) {
        $out = "";
        $len = strlen($fmt);
        for ($p = 0; $p < $len; $p++) {
            $ch = $fmt[$p];
            if ($ch === "%" && $p + 1 < $len) {
                $p++;
                $n = $fmt[$p];
                if ($n === "y") { $out = $out . $this->y; }
                elseif ($n === "m") { $out = $out . $this->m; }
                elseif ($n === "d") { $out = $out . $this->d; }
                elseif ($n === "h") { $out = $out . $this->h; }
                elseif ($n === "i") { $out = $out . $this->i; }
                elseif ($n === "s") { $out = $out . $this->s; }
                elseif ($n === "a") { $out = $out . $this->days; }
                elseif ($n === "R") { $out = $out . ($this->invert ? "-" : "+"); }
                elseif ($n === "%") { $out = $out . "%"; }
                else { $out = $out . $n; }
            } else {
                $out = $out . $ch;
            }
        }
        return $out;
    }
}
class DateTime {
    public $ts = 0;
    public function __construct($datetime = "now") {
        if ($datetime === "now" || $datetime === "") {
            $this->ts = time();
        } else {
            $this->ts = strtotime($datetime);
        }
    }
    public function format($format) { return date($format, $this->ts); }
    public function getTimestamp() { return $this->ts; }
    public function setTimestamp($ts) { $this->ts = $ts; return $this; }
    public function modify($modifier) { $this->ts = strtotime($modifier, $this->ts); return $this; }
    public function setDate($y, $m, $d) {
        $this->ts = mktime((int) date("H", $this->ts), (int) date("i", $this->ts),
            (int) date("s", $this->ts), $m, $d, $y);
        return $this;
    }
    public function setTime($h, $i, $s = 0) {
        $this->ts = mktime($h, $i, $s, (int) date("n", $this->ts),
            (int) date("j", $this->ts), (int) date("Y", $this->ts));
        return $this;
    }
    public function add($interval) { $this->ts = $this->ts + $interval->_seconds(); return $this; }
    public function sub($interval) { $this->ts = $this->ts - $interval->_seconds(); return $this; }
    public function diff($other) {
        $secs = $other->getTimestamp() - $this->ts;
        $inv = 0;
        if ($secs < 0) { $inv = 1; $secs = 0 - $secs; }
        $di = new DateInterval("");
        $di->days = intdiv($secs, 86400);
        $di->d = intdiv($secs, 86400);
        $rem = $secs % 86400;
        $di->h = intdiv($rem, 3600);
        $rem = $rem % 3600;
        $di->i = intdiv($rem, 60);
        $di->s = $rem % 60;
        $di->invert = $inv;
        return $di;
    }
}
class DateTimeImmutable {
    public $ts = 0;
    public function __construct($datetime = "now") {
        if ($datetime === "now" || $datetime === "") {
            $this->ts = time();
        } else {
            $this->ts = strtotime($datetime);
        }
    }
    public function format($format) { return date($format, $this->ts); }
    public function getTimestamp() { return $this->ts; }
    public function modify($modifier) {
        $n = new DateTimeImmutable();
        $n->ts = strtotime($modifier, $this->ts);
        return $n;
    }
    public function add($interval) {
        $n = new DateTimeImmutable();
        $n->ts = $this->ts + $interval->_seconds();
        return $n;
    }
    public function sub($interval) {
        $n = new DateTimeImmutable();
        $n->ts = $this->ts - $interval->_seconds();
        return $n;
    }
}
"#;

/// The SPL data-structure classes, written in PHP. They store elements in an
/// internal array property and mutate it through `$this`, which is the only way
/// a PHP array — a value, not a handle — can be mutated in place. Aliasing it to
/// a local first would copy it, and the write would be lost. Method-driven
/// (offsetGet/push/…), with `getIterator` returning the backing array.
const SPL_PRELUDE: &str = r#"
class stdClass {}
// `unserialize` restores an object of an unknown class into this placeholder,
// carrying the original name in `__PHP_Incomplete_Class_Name`.
class __PHP_Incomplete_Class {}
class SplDoublyLinkedList implements ArrayAccess, Countable, IteratorAggregate {
    public $dll = [];
    public function push($v) { $this->dll[] = $v; }
    public function pop() { return array_pop($this->dll); }
    public function shift() { return array_shift($this->dll); }
    public function unshift($v) { array_unshift($this->dll, $v); }
    public function top() { $n = count($this->dll); return $n > 0 ? $this->dll[$n - 1] : null; }
    public function bottom() { return count($this->dll) > 0 ? $this->dll[0] : null; }
    public function count() { return count($this->dll); }
    public function isEmpty() { return count($this->dll) === 0; }
    public function toArray() { return $this->dll; }
    public function offsetGet($i) { return $this->dll[$i]; }
    public function offsetSet($i, $v) { if ($i === null) { $this->dll[] = $v; } else { $this->dll[$i] = $v; } }
    public function offsetExists($i) { return isset($this->dll[$i]); }
    public function offsetUnset($i) { unset($this->dll[$i]); }
    public function getIterator() { return $this->dll; }
}
class SplStack extends SplDoublyLinkedList {}
class SplQueue extends SplDoublyLinkedList {
    public function enqueue($v) { $this->dll[] = $v; }
    public function dequeue() { return array_shift($this->dll); }
}
class SplFixedArray implements ArrayAccess, Countable, IteratorAggregate {
    public $data = [];
    public $sz = 0;
    public function __construct($size = 0) {
        $this->sz = $size;
        for ($i = 0; $i < $size; $i++) { $this->data[$i] = null; }
    }
    public function offsetGet($i) { return $this->data[$i]; }
    public function offsetSet($i, $v) { $this->data[$i] = $v; }
    public function offsetExists($i) { return $i >= 0 && $i < $this->sz; }
    public function getSize() { return $this->sz; }
    // Shrinking DISCARDS the elements past the new end and growing pads with
    // null, so `toArray()` always has exactly `sz` entries.
    public function setSize($size) {
        for ($i = $size; $i < $this->sz; $i++) { unset($this->data[$i]); }
        for ($i = $this->sz; $i < $size; $i++) { $this->data[$i] = null; }
        $this->sz = $size;
    }
    public function count() { return $this->sz; }
    public function toArray() { return $this->data; }
    public function getIterator() { return $this->data; }
    public static function fromArray($array) {
        $fa = new SplFixedArray(count($array));
        $i = 0;
        foreach ($array as $v) { $fa[$i] = $v; $i++; }
        return $fa;
    }
}
class ArrayObject implements ArrayAccess, Countable, IteratorAggregate {
    public $storage = [];
    public function __construct($array = []) { $this->storage = $array; }
    public function offsetGet($k) { return $this->storage[$k]; }
    public function offsetSet($k, $v) { if ($k === null) { $this->storage[] = $v; } else { $this->storage[$k] = $v; } }
    public function offsetExists($k) { return isset($this->storage[$k]); }
    public function offsetUnset($k) { unset($this->storage[$k]); }
    public function append($v) { $this->storage[] = $v; }
    public function count() { return count($this->storage); }
    public function getArrayCopy() { return $this->storage; }
    public function getIterator() { return $this->storage; }
}
class ArrayIterator extends ArrayObject {}
class SplObjectStorage implements ArrayAccess, Countable {
    public $store = [];
    public function attach($obj, $data = null) { $this->store[spl_object_id($obj)] = $data; }
    public function detach($obj) { unset($this->store[spl_object_id($obj)]); }
    public function contains($obj) { return array_key_exists(spl_object_id($obj), $this->store); }
    public function count() { return count($this->store); }
    public function offsetGet($obj) { return $this->store[spl_object_id($obj)] ?? null; }
    public function offsetSet($obj, $data) { $this->store[spl_object_id($obj)] = $data; }
    public function offsetExists($obj) { return array_key_exists(spl_object_id($obj), $this->store); }
    public function offsetUnset($obj) { unset($this->store[spl_object_id($obj)]); }
}
class SplPriorityQueue implements Countable {
    public $items = [];
    public function insert($value, $priority) { $this->items[] = [$priority, $value]; }
    public function count() { return count($this->items); }
    public function isEmpty() { return count($this->items) === 0; }
    public function _best() {
        $best = -1;
        $bp = null;
        foreach ($this->items as $i => $pv) {
            if ($best < 0 || $pv[0] > $bp) { $best = $i; $bp = $pv[0]; }
        }
        return $best;
    }
    public function top() { $b = $this->_best(); return $b < 0 ? null : $this->items[$b][1]; }
    public function extract() {
        $b = $this->_best();
        if ($b < 0) { return null; }
        $v = $this->items[$b][1];
        array_splice($this->items, $b, 1);
        return $v;
    }
}
class SplHeap implements Countable {
    public $items = [];
    public function insert($v) { $this->items[] = $v; }
    public function count() { return count($this->items); }
    public function isEmpty() { return count($this->items) === 0; }
    public function compare($a, $b) { return $a <=> $b; }
    public function _best() {
        $best = -1;
        $bv = null;
        foreach ($this->items as $i => $v) {
            if ($best < 0 || $this->compare($v, $bv) > 0) { $best = $i; $bv = $v; }
        }
        return $best;
    }
    public function top() { $b = $this->_best(); return $b < 0 ? null : $this->items[$b]; }
    public function extract() {
        $b = $this->_best();
        if ($b < 0) { return null; }
        $v = $this->items[$b];
        array_splice($this->items, $b, 1);
        return $v;
    }
}
class SplMaxHeap extends SplHeap {
    public function compare($a, $b) { return $a <=> $b; }
}
class SplMinHeap extends SplHeap {
    public function compare($a, $b) { return $b <=> $a; }
}
"#;

/// A compiled program's installable definitions: `(functions, classes)`.
type PreludeDefs = (Vec<(String, host::FuncDef)>, Vec<(String, host::ClassDef)>);

/// The compiled prelude's functions and classes, built once and merged onto every
/// fresh host before the user program (so user declarations of the same name win).
fn prelude_defs() -> &'static PreludeDefs {
    use std::sync::OnceLock;
    static CACHE: OnceLock<PreludeDefs> = OnceLock::new();
    CACHE.get_or_init(|| {
        let src = format!("{EXCEPTION_PRELUDE}{DATETIME_PRELUDE}{SPL_PRELUDE}");
        // NOT `compile`: that applies the file-level facts of whatever it is given,
        // and this runs from `load_merged` — after the user program was compiled —
        // so going through it would reset the typing mode the user's own
        // `declare(strict_types=1)` had just set. The prelude declares nothing of
        // the sort, so it has no facts of its own to apply.
        let stmts = parser::parse(&src).expect("prelude parses");
        let prog = compiler::compile(&stmts, false).expect("prelude compiles");
        (prog.functions, prog.classes)
    })
}

/// Merge an already-compiled program onto the current host (install the exception
/// prelude, then the program's user functions/classes/try-defs) and return the
/// main chunk for the caller to run.
pub fn load_merged(prog: compiler::Program) -> fusevm::Chunk {
    let compiler::Program {
        main,
        main_locals,
        functions,
        classes,
        try_defs,
        diags: _,
    } = prog;
    let (prelude_fns, prelude_classes) = prelude_defs();
    host::with_host(|h| {
        // Prelude first, then the user program — a user redeclaration wins.
        h.load_program(prelude_fns.clone());
        h.load_classes(prelude_classes.clone());
        h.load_program(functions);
        h.load_classes(classes);
        h.load_try_defs(try_defs);
        // Reserve the global frame's slots before the chunk that numbered them
        // runs. An `include`/`eval` later in the same frame keeps the by-name
        // path, which reaches these same slots.
        h.seed_global_slots(&main_locals);
    });
    main
}

/// Run an already-compiled program on the current host.
///
/// Compile-time notices are flushed first, ahead of the program's own output —
/// the reference engine finishes reading the whole source before it executes a
/// line of it, so `echo "a"; $v = 1; echo "${v}";` prints the deprecation notice
/// BEFORE the `a`, not between the two statements.
pub fn run_compiled(mut prog: compiler::Program) -> Result<Value, String> {
    let diags = std::mem::take(&mut prog.diags);
    let main = load_merged(prog);
    if !diags.is_empty() {
        host::with_host(|h| {
            for d in &diags {
                h.diagnose(d.severity, d.level, d.line, &d.msg);
            }
        });
    }
    host::run_main(main)
}

/// Parse, compile, load, and run a PHP source string on a fresh host; return the
/// value of the last top-level expression.
pub fn eval_str(src: &str) -> Result<Value, String> {
    host::reset_host();
    run_compiled(compile(src)?)
}

/// Compile `src`, displaying a syntax error the way the PHP CLI does: a
/// `Parse error: …` copy on stdout and a `PHP `-prefixed copy on stderr. The
/// error is still returned so the caller can take an exit status from it, with
/// [`host::fatal_reported`] marking that it has already been shown.
///
/// Only the *parser*'s failures are framed this way. A later compile failure has
/// no reference equivalent (it is scaffold-specific), so it falls through to the
/// terse `php: …` form.
fn compile_cli(src: &str) -> Result<compiler::Program, String> {
    if let Err(e) = parser::parse_meta(src) {
        // A `declare(strict_types=…)` violation is a `Fatal error`, not a
        // `Parse error`; both stop the run before it produces any output.
        host::with_host(|h| h.fatal(e.severity, &e.message));
        return Err(e.message);
    }
    compile(src)
}

/// [`eval_str`] for the CLI: identical, except a syntax error is displayed in
/// PHP's shape rather than left to the caller to print tersely, and the trailing
/// command-line arguments reach the program as `$argv`.
pub fn eval_cli(src: &str, args: &[String]) -> Result<Value, String> {
    host::reset_host();
    // `php -r` code names itself `Command line code` in diagnostics and
    // `__FILE__`, but `Standard input code` in `$argv[0]`. The reference really
    // does disagree with itself here, so the two are set from different values.
    host::with_host(|h| h.set_script_args(None, args));
    run_compiled(compile_cli(src)?)
}

/// [`eval_cli`] for a script read from standard input (`php < script.php`).
///
/// The SAME source names itself differently under the three CLI entry points, so
/// the name cannot be inferred from the text: `php -r` code is `Command line
/// code`, a named file is its resolved path, and stdin is `Standard input code`.
/// Every diagnostic quotes it, and so does `__FILE__`.
pub fn eval_stdin_cli(src: &str, args: &[String]) -> Result<Value, String> {
    host::reset_host();
    host::with_host(|h| {
        h.set_script_name("Standard input code");
        h.set_script_args(None, args);
    });
    run_compiled(compile_cli(src)?)
}

/// [`eval_file`] for the CLI — see [`eval_cli`].
pub fn eval_file_cli(path: &str, args: &[String]) -> Result<Value, String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    host::reset_host();
    set_script_name(path);
    // `$argv[0]` is the path AS WRITTEN on the command line, where the diagnostic
    // name just set is the resolved one — `php sub/s.php` reports both.
    host::with_host(|h| h.set_script_args(Some(path), args));
    run_compiled(compile_cli(&src)?)
}

/// Read and run a `.php` file on a fresh host.
pub fn eval_file(path: &str) -> Result<Value, String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    host::reset_host();
    set_script_name(path);
    run_compiled(compile(&src)?)
}

/// Name this run's source for diagnostics. PHP prints the script's *resolved*
/// path in `Warning: … in <file> on line N`, so a relative argument is expanded;
/// a fresh host defaults to `"Command line code"`, which is what `php -r` uses.
fn set_script_name(path: &str) {
    let resolved = std::fs::canonicalize(path)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.to_string());
    host::with_host(|h| h.set_script_name(resolved));
}

/// Read and run a `.php` file under the DAP debugger (per-statement line markers,
/// tracing JIT disabled so the markers fire).
pub fn eval_file_debug(path: &str) -> Result<Value, String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let prog = compile_debug(&src)?;
    host::reset_host();
    set_script_name(path);
    host::set_debug_mode(true);
    let r = run_compiled(prog);
    host::set_debug_mode(false);
    r
}

/// Evaluate `src` with `vars` bound and return the captured program output —
/// [`eval_capture`] for an embedder that has input to hand the program.
///
/// The variables are seeded *after* the host reset that starts every run, which
/// is the whole reason this exists: setting them beforehand through
/// `host::with_host` cannot work, because the reset wipes them. Each is bound as
/// an ordinary PHP variable, so `("stdin", "…")` reads as `$stdin`.
///
/// ```no_run
/// let out = phplang::eval_capture_with("<?php echo strtoupper($stdin);", &[("stdin", "hi")]);
/// assert_eq!(out.unwrap(), "HI");
/// ```
pub fn eval_capture_with(src: &str, vars: &[(&str, &str)]) -> Result<String, String> {
    host::reset_host();
    host::with_host(|h| {
        for (name, text) in vars {
            h.set_var(name, Value::Str(std::sync::Arc::new(text.to_string())));
        }
        h.begin_capture();
    });
    let r = run_compiled(compile(src)?);
    let out = host::with_host(|h| h.end_capture());
    r.map(|_| out)
}

/// Evaluate `src` and return the captured program output. The convenience entry
/// point for tests: installs an output buffer, runs, and returns what `echo`
/// wrote (PHP is output-oriented — its observable result is stdout, not a value).
pub fn eval_capture(src: &str) -> Result<String, String> {
    host::reset_host();
    host::with_host(|h| h.begin_capture());
    let r = run_compiled(compile(src)?);
    let out = host::with_host(|h| h.end_capture());
    r.map(|_| out)
}
