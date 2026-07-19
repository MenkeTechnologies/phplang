//! Command-line interface for the `php` binary.

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "php",
    version,
    about = "PHP on fusevm — a compiled PHP runtime (bytecode VM + Cranelift JIT)",
    long_about = None,
)]
pub struct Cli {
    /// Run PHP code directly from the argument (`php -r 'echo 1+1;'`). The code
    /// runs as if wrapped in `<?php ... ?>` (no opening tag required), matching
    /// the reference `php -r`.
    #[arg(short = 'r', long = "run", value_name = "CODE")]
    pub run: Option<String>,

    /// Start the interactive REPL (`php -a`).
    #[arg(short = 'a', long = "interactive")]
    pub interactive: bool,

    /// Print the compiled fusevm bytecode for the script and exit.
    #[arg(long = "dump-bytecode")]
    pub dump_bytecode: bool,

    /// The `.php` script to run (omit with -a / -r).
    #[arg(value_name = "FILE")]
    pub file: Option<String>,

    /// Arguments passed through to the PHP program as `$argv`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub argv: Vec<String>,
}

/// Parse the process arguments.
pub fn parse() -> Cli {
    Cli::parse()
}
