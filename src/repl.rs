//! The interactive REPL (`php -a`).
//!
//! Each entered line is executed as PHP against a persistent host, so variables
//! and functions defined on one line are visible on the next. Program output is
//! written straight to stdout; a compile/run error prints to stderr and the loop
//! continues.

use reedline::{DefaultPrompt, DefaultPromptSegment, Reedline, Signal};

pub fn run() {
    crate::banner::print_banner();
    crate::host::reset_host();

    let mut editor = Reedline::create();
    let prompt = DefaultPrompt::new(
        DefaultPromptSegment::Basic("php".to_string()),
        DefaultPromptSegment::Empty,
    );

    loop {
        match editor.read_line(&prompt) {
            Ok(Signal::Success(line)) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if matches!(trimmed, "exit" | "quit") {
                    break;
                }
                run_line(&line);
            }
            Ok(Signal::CtrlC) | Ok(Signal::CtrlD) => break,
            _ => break,
        }
    }
}

/// Compile and run one REPL line against the (persistent) host.
fn run_line(line: &str) {
    // Accept a line with or without an opening tag; default to PHP mode.
    let src = if line.trim_start().starts_with("<?") {
        line.to_string()
    } else {
        format!("<?php {line}")
    };
    match crate::compile(&src) {
        Ok(prog) => {
            if let Err(e) = crate::run_compiled(prog) {
                eprintln!("php: {e}");
            }
        }
        Err(e) => eprintln!("php: {e}"),
    }
}
