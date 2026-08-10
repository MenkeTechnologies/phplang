//! The `php` binary entry point.
//!
//! Dispatch: `-a` starts the interactive REPL; `-r` runs a one-liner; otherwise
//! a `.php` file is run.
//!
//! A fatal or parse error is reported the way the PHP CLI reports it — the
//! *display* copy on stdout (written by the runtime itself, so it interleaves
//! with the program's own output and lands inside an open `ob_start` buffer) and
//! a `PHP `-prefixed *log* copy on stderr — and exits 255. Anything with no PHP
//! equivalent still goes to stderr in terse `php: <reason>` form.

use std::process::ExitCode;

/// What the PHP CLI exits with after a fatal or parse error.
const FATAL_EXIT: u8 = 255;

fn main() -> ExitCode {
    let mut cli = phplang::cli::parse();

    // `-d name=value` must take effect before the source is read: it can change
    // the error level that decides whether a COMPILE-time notice is displayed.
    for d in &cli.define {
        let (name, value) = match d.split_once('=') {
            Some((n, v)) => (n, v),
            None => (d.as_str(), "1"),
        };
        phplang::host::add_initial_ini(name.trim(), value.trim());
    }

    if cli.lsp {
        return match phplang::lsp::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => fail(&e),
        };
    }
    if cli.dap {
        return match phplang::dap::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => fail(&e),
        };
    }

    if let Some(code) = cli.run.take() {
        // `php -r` code carries no opening tag; it is raw PHP. Prepend `<?php`
        // so the lexer starts in PHP mode instead of echoing it as inline HTML.
        let src = format!("<?php {code}");
        // With `-r` there is no script to name, so the FILE positional the parser
        // filled is really the program's first argument: `php -r CODE a b` must
        // reach the program as `$argv = ["Standard input code", "a", "b"]`.
        let mut args = Vec::new();
        args.extend(cli.file.take());
        args.append(&mut cli.argv);
        return match phplang::eval_cli(&src, &args) {
            Ok(_) => ExitCode::SUCCESS,
            Err(e) => fail(&e),
        };
    }

    if let Some(file) = cli.file {
        if cli.dump_bytecode {
            return finish(dump(&file));
        }
        if cli.dump_tokens {
            return finish(dump_tokens(&file));
        }
        if cli.dump_ast {
            return finish(dump_ast(&file));
        }
        if cli.disasm {
            return finish(disasm(&file));
        }
        if cli.tiers {
            return finish(tiers(&file));
        }
        return match phplang::eval_file_cli(&file, &cli.argv) {
            Ok(_) => ExitCode::SUCCESS,
            Err(e) => fail(&e),
        };
    }

    if cli.interactive || atty_stdin() {
        phplang::repl::run();
        return ExitCode::SUCCESS;
    }

    // No file and non-interactive stdin: run stdin as a script. The reference
    // names it `Standard input code` rather than `Command line code`, which is
    // what every diagnostic and `__FILE__` in it quote.
    let src = std::io::read_to_string(std::io::stdin()).unwrap_or_default();
    match phplang::eval_stdin_cli(&src, &cli.argv) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => fail(&e),
    }
}

/// Render a function's parameter list for the `--dump`/`--disasm` headers:
/// `$name`, `...$rest` for a variadic, and a `= …` marker when a default exists.
fn fmt_params(params: &[phplang::host::Param]) -> String {
    params
        .iter()
        .map(|p| {
            let dots = if p.variadic { "..." } else { "" };
            let def = if p.default.is_some() { " = …" } else { "" };
            format!("{dots}${}{def}", p.name)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn dump(file: &str) -> Result<(), String> {
    let src = std::fs::read_to_string(file).map_err(|e| format!("cannot read {file}: {e}"))?;
    let prog = phplang::compile(&src)?;
    // Bytecode dump is explicit user-requested output.
    println!("== main ==\n{:#?}", prog.main.ops);
    for (name, f) in &prog.functions {
        println!(
            "== function {name}({}) ==\n{:#?}",
            fmt_params(&f.params),
            f.chunk.ops
        );
    }
    Ok(())
}

/// `--dump-tokens`: print the lexer token stream (after `rust { }` desugaring),
/// one `line:col  Tok` per line.
fn dump_tokens(file: &str) -> Result<(), String> {
    let src = std::fs::read_to_string(file).map_err(|e| format!("cannot read {file}: {e}"))?;
    let src = phplang::rust_ffi::desugar(&src);
    for t in phplang::lexer::lex(&src)? {
        println!("{}\t{:?}", t.line, t.tok);
    }
    Ok(())
}

/// `--dump-ast`: print the parsed PHP AST.
fn dump_ast(file: &str) -> Result<(), String> {
    let src = std::fs::read_to_string(file).map_err(|e| format!("cannot read {file}: {e}"))?;
    let stmts = phplang::parser::parse(&src)?;
    println!("{stmts:#?}");
    Ok(())
}

/// `--disasm`: print a fusevm bytecode disassembly of the main chunk and every
/// compiled function, via the shared `fusevm::Chunk::disassemble`.
fn disasm(file: &str) -> Result<(), String> {
    let src = std::fs::read_to_string(file).map_err(|e| format!("cannot read {file}: {e}"))?;
    let prog = phplang::compile(&src)?;
    println!("; php fusevm — main\n{}", prog.main.disassemble());
    for (name, f) in &prog.functions {
        println!(
            "; php fusevm — function {name}({})\n{}",
            fmt_params(&f.params),
            f.chunk.disassemble()
        );
    }
    Ok(())
}

/// `--tiers`: run the script, then report which fusevm execution tier took
/// each of its chunks — asked of fusevms own eligibility and cache
/// predicates, so the answer comes from the compiler that would have done the
/// work. The programs own output precedes the report.
fn tiers(file: &str) -> Result<(), String> {
    let src = std::fs::read_to_string(file).map_err(|e| format!("cannot read {file}: {e}"))?;
    println!("{}", phplang::tiers::report(&src)?);
    Ok(())
}

fn finish(r: Result<(), String>) -> ExitCode {
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(&e),
    }
}

fn atty_stdin() -> bool {
    // SAFETY: isatty is a pure query on the stdin fd.
    unsafe { libc::isatty(libc::STDIN_FILENO) == 1 }
}

/// Report a failed run. A diagnostic the runtime already displayed in PHP's own
/// shape (on both streams) is not printed again — only its exit status is taken.
fn fail(msg: &str) -> ExitCode {
    if phplang::host::fatal_reported() {
        return ExitCode::from(FATAL_EXIT);
    }
    eprintln!("php: {msg}");
    ExitCode::FAILURE
}
