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

/// Merge an already-compiled program onto the current host (install its user
/// functions) and return the main chunk for the caller to run.
pub fn load_merged(prog: compiler::Program) -> fusevm::Chunk {
    let compiler::Program { main, functions } = prog;
    host::with_host(|h| h.load_program(functions));
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
