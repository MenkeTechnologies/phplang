//! Which STREAM a diagnostic goes to, and which ini flag decides.
//!
//! Every diagnostic PHP raises is written twice, under two independent flags:
//! `display_errors` puts the copy the program sees on stdout (interleaved with
//! its own output, inside any open `ob_start` buffer), and `log_errors` puts a
//! `PHP `-prefixed copy on stderr. Both default on.
//!
//! No other test in this suite reads stderr — `eval_capture` cannot see it and
//! the CLI tests null it — so without this file the whole `log_errors` half of
//! every diagnostic is unobserved, and a run that emitted nothing there would
//! look identical to a correct one. That was in fact the state: warnings were
//! never logged at all.
//!
//! Every expectation is the verbatim output of the same command under the
//! reference `php` 8.5.9, captured by running it; the command is quoted above
//! each assertion.

use std::process::{Command, Stdio};

/// Run `code` through the crate's binary with `flags` in front of `-r`, and
/// return `(stdout, stderr, exit status)` — all three, which is the point.
fn run(flags: &[&str], code: &str) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_php"))
        .args(flags)
        .arg("-r")
        .arg(code)
        .stdin(Stdio::null())
        .output()
        .expect("spawn php -r");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

const WARN_SRC: &str = r#"echo "12abc" + 0;"#;
const WARN_DISPLAY: &str = "\nWarning: A non-numeric value encountered in Command line code \
                            on line 1\n12";
const WARN_LOG: &str =
    "PHP Warning:  A non-numeric value encountered in Command line code on line 1\n";

const FATAL_SRC: &str = r#"throw new Exception("x");"#;
const FATAL_BODY: &str = "Uncaught Exception: x in Command line code:1\nStack trace:\n\
                          #0 {main}\n  thrown in Command line code on line 1";

// ── both copies by default ───────────────────────────────────────────────────

#[test]
fn a_warning_is_displayed_on_stdout_and_logged_on_stderr() {
    // php -r 'echo "12abc" + 0;'
    let (out, err, code) = run(&[], WARN_SRC);
    assert_eq!(out, WARN_DISPLAY);
    assert_eq!(err, WARN_LOG);
    assert_eq!(code, 0);
}

#[test]
fn a_fatal_is_displayed_on_stdout_and_logged_on_stderr() {
    // php -r 'throw new Exception("x");'
    let (out, err, code) = run(&[], FATAL_SRC);
    assert_eq!(out, format!("\nFatal error: {FATAL_BODY}\n"));
    assert_eq!(err, format!("PHP Fatal error:  {FATAL_BODY}\n"));
    assert_eq!(code, 255);
}

#[test]
fn a_parse_error_is_displayed_on_stdout_and_logged_on_stderr() {
    // php -r 'echo 1+;'
    let body = "syntax error, unexpected token \";\" in Command line code on line 1";
    let (out, err, code) = run(&[], "echo 1+;");
    assert_eq!(out, format!("\nParse error: {body}\n"));
    assert_eq!(err, format!("PHP Parse error:  {body}\n"));
    assert_eq!(code, 255);
}

// ── each flag suppresses its own copy and only its own ───────────────────────

#[test]
fn log_errors_off_leaves_the_stdout_copy_alone() {
    // php -d log_errors=0 -r 'echo "12abc" + 0;'
    let (out, err, _) = run(&["-d", "log_errors=0"], WARN_SRC);
    assert_eq!(out, WARN_DISPLAY);
    assert_eq!(err, "");
}

#[test]
fn display_errors_off_leaves_the_stderr_copy_alone() {
    // php -d display_errors=0 -r 'echo "12abc" + 0;'  →  stdout is the program's
    // own output ALONE; the diagnostic survives on stderr.
    let (out, err, _) = run(&["-d", "display_errors=0"], WARN_SRC);
    assert_eq!(out, "12");
    assert_eq!(err, WARN_LOG);
}

#[test]
fn a_fatal_is_not_exempt_from_either_flag() {
    // A fatal obeys the same two flags a warning does: with display off it is
    // visible on stderr alone, and the status is 255 whether or not anyone was
    // told.
    let (out, err, code) = run(&["-d", "display_errors=0"], FATAL_SRC);
    assert_eq!(out, "");
    assert_eq!(err, format!("PHP Fatal error:  {FATAL_BODY}\n"));
    assert_eq!(code, 255);

    let (out, err, code) = run(&["-d", "log_errors=0"], FATAL_SRC);
    assert_eq!(out, format!("\nFatal error: {FATAL_BODY}\n"));
    assert_eq!(err, "");
    assert_eq!(code, 255);
}

// ── trigger_error rides the same path ────────────────────────────────────────

#[test]
fn a_user_diagnostic_takes_both_streams_too() {
    // php -r 'trigger_error("m");'  — a user-raised Notice is not special-cased
    // onto stderr; it is a diagnostic like any other.
    let (out, err, code) = run(&[], r#"trigger_error("m");"#);
    assert_eq!(out, "\nNotice: m in Command line code on line 1\n");
    assert_eq!(err, "PHP Notice:  m in Command line code on line 1\n");
    assert_eq!(code, 0);
}

#[test]
fn e_user_error_ends_the_request_with_status_255() {
    // php -r 'trigger_error("m", E_USER_ERROR); echo "after";'
    //
    // Three things at once, and the `after` is the load-bearing one: the level
    // is itself deprecated as of 8.4, the fatal that follows carries the
    // library call as frame #0, and the program does NOT continue.
    let (out, _, code) = run(&[], r#"trigger_error("m", E_USER_ERROR); echo "after";"#);
    assert_eq!(
        out,
        "\nDeprecated: Passing E_USER_ERROR to trigger_error() is deprecated since 8.4, throw \
         an exception or call exit with a string message instead in Command line code on line \
         1\n\nFatal error: m in Command line code on line 1\nStack trace:\n\
         #0 Command line code(1): trigger_error('m', 256)\n#1 {main}\n"
    );
    assert_eq!(code, 255);
}

#[test]
fn a_user_fatals_trace_names_the_php_frames_above_it() {
    // php -r 'function g(){ trigger_error("m", E_USER_ERROR); } g(); echo "no";'
    let (out, _, code) = run(
        &[],
        r#"function g(){ trigger_error("m", E_USER_ERROR); } g(); echo "no";"#,
    );
    assert!(
        out.ends_with(
            "\nFatal error: m in Command line code on line 1\nStack trace:\n\
             #0 Command line code(1): trigger_error('m', 256)\n\
             #1 Command line code(1): g()\n#2 {main}\n"
        ),
        "trace was: {out:?}"
    );
    assert_eq!(code, 255);
}
