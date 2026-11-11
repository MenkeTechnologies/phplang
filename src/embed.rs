//! The embedding entry point: run a whole PHP program with a given standard
//! input and get back what it wrote to standard output.

use crate::{compile, host, run_request};

/// The stack each run gets: the 8 MiB a CLI run has on its main thread, so a
/// program recurses as deep embedded as it does under `php`.
const STACK: usize = 8 * 1024 * 1024;

/// Run `src` as a complete program with `input` as its stdin and return
/// everything it wrote to stdout.
///
/// Each call runs on its own thread with a fresh host (the host and every other
/// piece of interpreter state are thread-local), so nothing leaks between calls
/// and concurrent calls never share an output buffer. `src` is a PHP file's
/// text, so it starts in HTML mode: PHP code needs its `<?php` tag.
///
/// - `input` is what `STDIN`, `php://stdin` and `readline()` read.
/// - Output is captured below the output-buffering stack; `fwrite(STDOUT, …)`
///   lands there too. Diagnostics are displayed into the output as the CLI
///   displays them on stdout; their log copies still go to the process stderr.
/// - The request shutdown runs as under the CLI: shutdown functions,
///   destructors, then the open output buffers.
/// - `Err` for a parse or compile error, an uncaught exception or fatal error
///   (its message), a nonzero `exit`/`die` status (`exit status N`), or a panic
///   in the runtime. `exit(0)`, `exit("text")` and `die` with no status are `Ok`
///   with the output up to that point.
pub fn run_program(src: &str, input: &str) -> Result<String, String> {
    let (src, input) = (src.to_owned(), input.to_owned());
    let run = std::thread::Builder::new()
        .name("phplang-embed".into())
        .stack_size(STACK)
        .spawn(move || run_here(&src, &input))
        .map_err(|e| format!("cannot start the interpreter thread: {e}"))?;
    run.join().unwrap_or_else(|panic| {
        let msg = panic
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".into());
        Err(format!("interpreter panicked: {msg}"))
    })
}

/// [`run_program`] on the calling thread.
fn run_here(src: &str, input: &str) -> Result<String, String> {
    host::reset_host();
    host::with_host(|h| {
        h.set_stdin(input);
        h.begin_capture();
    });
    let r = compile(src).and_then(run_request);
    let out = host::with_host(|h| h.end_capture());
    r?;
    match host::pending_exit() {
        // The low byte, as the CLI hands the shell: `exit(256)` succeeds.
        Some(status) if status as u8 != 0 => Err(format!("exit status {}", status as u8)),
        _ => Ok(out),
    }
}
