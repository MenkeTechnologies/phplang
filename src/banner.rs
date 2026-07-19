//! The REPL banner (shown only for the interactive `php -a`, never for script
//! execution).

use nu_ansi_term::Color;

/// ANSI-Shadow wordmark, matching the house cyberpunk style.
pub const WORDMARK: &str = r#"
██████╗ ██╗  ██╗██████╗
██╔══██╗██║  ██║██╔══██╗
██████╔╝███████║██████╔╝
██╔═══╝ ██╔══██║██╔═══╝
██║     ██║  ██║██║
╚═╝     ╚═╝  ╚═╝╚═╝
"#;

/// Print the banner + a one-line subtitle for the interactive REPL.
pub fn print_banner() {
    let v = env!("CARGO_PKG_VERSION");
    println!("{}", Color::Cyan.bold().paint(WORDMARK));
    println!(
        "{} {}  {}",
        Color::Purple.bold().paint("phplang"),
        Color::White.paint(format!("v{v}")),
        Color::DarkGray.paint("PHP on fusevm — bytecode VM + Cranelift JIT")
    );
    println!(
        "{}",
        Color::DarkGray.paint("type a statement, or Ctrl-D to exit")
    );
}
