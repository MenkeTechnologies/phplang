//! phplang — PHP as a fusevm frontend.
//!
//! Pipeline: `lexer` → `parser` builds a PHP AST → `compiler` lowers it to a
//! `fusevm::Chunk` (plus a table of function sub-chunks) → fusevm executes it,
//! calling back into the `host` (through registered builtins and the strict
//! numeric hook) for every PHP-specific operation. There is no bespoke VM or
//! JIT here — execution and codegen live in fusevm.

pub mod ast;
pub mod banner;
pub mod builtins;
pub mod cli;
pub mod compiler;
pub mod dap;
pub mod host;
pub mod intercepts;
pub mod lexer;
pub mod lsp;
pub mod parser;
pub mod repl;
pub mod rust_ffi;
pub mod stdlib;

pub use fusevm::Value;

/// Compile a PHP source string to a runnable program.
pub fn compile(src: &str) -> Result<compiler::Program, String> {
    let stmts = parser::parse(src)?;
    compiler::compile(&stmts, false)
}

/// Compile with per-statement DAP line markers enabled (`php --dap`).
pub fn compile_debug(src: &str) -> Result<compiler::Program, String> {
    let stmts = parser::parse(src)?;
    compiler::compile(&stmts, true)
}

/// The built-in exception class hierarchy, written in PHP so it flows through the
/// real class system: `throw`/`catch` resolve these as ordinary classes, and user
/// code can subclass them. `Exception` and `Error` are the two disjoint roots
/// (`catch (Throwable)` — handled in the host — matches either); the rest inherit
/// their `__construct`/`getMessage`/`getCode`/`__toString`.
const EXCEPTION_PRELUDE: &str = r#"<?php
class Exception {
    protected $message = "";
    protected $code = 0;
    public function __construct($message = "", $code = 0) {
        $this->message = $message;
        $this->code = $code;
    }
    public function getMessage() { return $this->message; }
    public function getCode() { return $this->code; }
    public function __toString() { return $this->message; }
}
class Error {
    protected $message = "";
    protected $code = 0;
    public function __construct($message = "", $code = 0) {
        $this->message = $message;
        $this->code = $code;
    }
    public function getMessage() { return $this->message; }
    public function getCode() { return $this->code; }
    public function __toString() { return $this->message; }
}
class RuntimeException extends Exception {}
class LogicException extends Exception {}
class InvalidArgumentException extends LogicException {}
class ArithmeticError extends Error {}
class DivisionByZeroError extends ArithmeticError {}
class TypeError extends Error {}
class ValueError extends Error {}
class UnhandledMatchError extends Error {}
"#;

/// A compiled program's installable definitions: `(functions, classes)`.
type PreludeDefs = (
    Vec<(String, host::FuncDef)>,
    Vec<(String, host::ClassDef)>,
);

/// The compiled prelude's functions and classes, built once and merged onto every
/// fresh host before the user program (so user declarations of the same name win).
fn prelude_defs() -> &'static PreludeDefs {
    use std::sync::OnceLock;
    static CACHE: OnceLock<PreludeDefs> = OnceLock::new();
    CACHE.get_or_init(|| {
        let prog = compile(EXCEPTION_PRELUDE).expect("exception prelude compiles");
        (prog.functions, prog.classes)
    })
}

/// Merge an already-compiled program onto the current host (install the exception
/// prelude, then the program's user functions/classes/try-defs) and return the
/// main chunk for the caller to run.
pub fn load_merged(prog: compiler::Program) -> fusevm::Chunk {
    let compiler::Program {
        main,
        functions,
        classes,
        try_defs,
    } = prog;
    let (prelude_fns, prelude_classes) = prelude_defs();
    host::with_host(|h| {
        // Prelude first, then the user program — a user redeclaration wins.
        h.load_program(prelude_fns.clone());
        h.load_classes(prelude_classes.clone());
        h.load_program(functions);
        h.load_classes(classes);
        h.load_try_defs(try_defs);
    });
    main
}

/// Run an already-compiled program on the current host.
pub fn run_compiled(prog: compiler::Program) -> Result<Value, String> {
    host::run_main(load_merged(prog))
}

/// Parse, compile, load, and run a PHP source string on a fresh host; return the
/// value of the last top-level expression.
pub fn eval_str(src: &str) -> Result<Value, String> {
    host::reset_host();
    run_compiled(compile(src)?)
}

/// Read and run a `.php` file on a fresh host.
pub fn eval_file(path: &str) -> Result<Value, String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    host::reset_host();
    run_compiled(compile(&src)?)
}

/// Read and run a `.php` file under the DAP debugger (per-statement line markers,
/// tracing JIT disabled so the markers fire).
pub fn eval_file_debug(path: &str) -> Result<Value, String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let prog = compile_debug(&src)?;
    host::reset_host();
    host::set_debug_mode(true);
    let r = run_compiled(prog);
    host::set_debug_mode(false);
    r
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
