//! The `php` binary entry point.
//!
//! Dispatch: `-a` starts the interactive REPL; `-r` runs a one-liner; otherwise
//! a `.php` file is run. Errors go to stderr in terse `php: <reason>` form;
//! nothing else is printed.

use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = phplang::cli::parse();

    if let Some(code) = cli.run {
        // `php -r` code carries no opening tag; it is raw PHP. Prepend `<?php`
        // so the lexer starts in PHP mode instead of echoing it as inline HTML.
        let src = format!("<?php {code}");
        return run_source(&src);
    }

    if let Some(file) = cli.file {
        if cli.dump_bytecode {
            return match dump(&file) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => fail(&e),
            };
        }
        return match phplang::eval_file(&file) {
            Ok(_) => ExitCode::SUCCESS,
            Err(e) => fail(&e),
        };
    }

    if cli.interactive || atty_stdin() {
        phplang::repl::run();
        return ExitCode::SUCCESS;
    }

    // No file and non-interactive stdin: run stdin as a script.
    let src = std::io::read_to_string(std::io::stdin()).unwrap_or_default();
    run_source(&src)
}

fn run_source(src: &str) -> ExitCode {
    match phplang::eval_str(src) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => fail(&e),
    }
}

fn dump(file: &str) -> Result<(), String> {
    let src = std::fs::read_to_string(file).map_err(|e| format!("cannot read {file}: {e}"))?;
    let prog = phplang::compile(&src)?;
    // Bytecode dump is explicit user-requested output.
    println!("== main ==\n{:#?}", prog.main.ops);
    for (name, f) in &prog.functions {
        println!(
            "== function {name}({}) ==\n{:#?}",
            f.params.join(", "),
            f.chunk.ops
        );
    }
    Ok(())
}

fn atty_stdin() -> bool {
    // SAFETY: isatty is a pure query on the stdin fd.
    unsafe { libc::isatty(libc::STDIN_FILENO) == 1 }
}

fn fail(msg: &str) -> ExitCode {
    eprintln!("php: {msg}");
    ExitCode::FAILURE
}
