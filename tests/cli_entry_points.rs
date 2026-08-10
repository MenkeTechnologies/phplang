//! The three ways the CLI is handed a script — a named file, `-r`, and standard
//! input — and the observables that differ between them.
//!
//! This is the one property no in-process test can reach: the entry point is
//! chosen by the command line, and it decides what the script is CALLED. That
//! name is not cosmetic. Every diagnostic quotes it, `__FILE__` returns it, and
//! `$argv[0]` returns a THIRD thing that agrees with neither in the `-r` case —
//! so a test that only ever ran one entry point would pin the wrong string
//! everywhere and still pass.
//!
//! Every expectation is the verbatim output of the same command under the
//! reference `php` 8.5.9. The tests shell out to the binary this crate builds
//! (`CARGO_BIN_EXE_php`) and need nothing installed.

use std::path::PathBuf;
use std::process::{Command, Stdio};

fn php() -> Command {
    Command::new(env!("CARGO_BIN_EXE_php"))
}

/// Run with `-r` and return stdout. stderr is dropped: it carries only the
/// `log_errors` copy of a diagnostic, and stdout carries the `display_errors`
/// one, which is the copy interleaved with the program's own output.
fn run_r(code: &str, args: &[&str]) -> String {
    let out = php()
        .arg("-r")
        .arg(code)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .expect("spawn php -r");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn run_stdin(src: &str) -> String {
    use std::io::Write;
    let mut child = php()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn php with stdin");
    child
        .stdin
        .take()
        .expect("stdin pipe")
        .write_all(src.as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait for php");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Write `src` to a uniquely named file under the temp directory and run it.
/// Returns `(stdout, the path as passed on the command line, its resolved form)`.
/// The resolved form is taken while the file still EXISTS — on macOS the temp
/// directory is a symlink, so the two differ, which is the whole point of the
/// assertions that use it.
fn run_file(tag: &str, src: &str, args: &[&str]) -> (String, PathBuf, PathBuf) {
    let path = std::env::temp_dir().join(format!("phplang_entry_{tag}_{}.php", std::process::id()));
    std::fs::write(&path, src).expect("write script");
    let out = php()
        .arg(&path)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .expect("spawn php FILE");
    let resolved = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        path,
        resolved,
    )
}

#[test]
fn each_entry_point_names_the_script_differently() {
    // The name reaches the program through `__FILE__` and every diagnostic, and
    // the three answers are three different strings.
    assert_eq!(run_r("echo __FILE__;", &[]), "Command line code");
    assert_eq!(run_stdin("<?php echo __FILE__;"), "Standard input code");
    // A file reports its RESOLVED path, so a relative argument is expanded — the
    // temp directory is a symlink on macOS, which is exactly why the assertion is
    // against the canonical form rather than against the path as written.
    let (out, _, resolved) = run_file("file", "<?php echo __FILE__;", &[]);
    assert_eq!(out, resolved.display().to_string());
}

#[test]
fn a_diagnostic_quotes_the_same_name_file_does() {
    // One program, two entry points, two different diagnostics — which is the
    // trap this file exists for: a warning pinned under `-r` does not describe
    // the same program run from stdin.
    assert_eq!(
        run_r("echo $undef;", &[]),
        "\nWarning: Undefined variable $undef in Command line code on line 1\n"
    );
    assert_eq!(
        run_stdin("<?php echo $undef;"),
        "\nWarning: Undefined variable $undef in Standard input code on line 1\n"
    );
}

#[test]
fn argv_zero_disagrees_with_file_under_dash_r() {
    // `php -r` calls itself `Command line code` in `__FILE__` and
    // `Standard input code` in `$argv[0]`. The reference disagrees with itself
    // here; an engine that derived one from the other would be wrong on one side.
    assert_eq!(
        run_r("echo $argv[0], \"|\", __FILE__;", &[]),
        "Standard input code|Command line code"
    );
    assert_eq!(
        run_stdin("<?php echo $argv[0], \"|\", __FILE__;"),
        "Standard input code|Standard input code"
    );
}

#[test]
fn trailing_arguments_reach_argv_under_every_entry_point() {
    assert_eq!(
        run_r(
            "echo $argc, \":\", implode(\",\", array_slice($argv, 1));",
            &["a", "b"]
        ),
        "3:a,b"
    );
    let (out, ..) = run_file(
        "args",
        "<?php echo $argc, \":\", implode(\",\", array_slice($argv, 1));",
        &["a", "b"],
    );
    assert_eq!(out, "3:a,b");
    // No arguments still means one entry — the script itself.
    assert_eq!(run_r("echo $argc;", &[]), "1");
    assert_eq!(run_stdin("<?php echo $argc;"), "1");
}

#[test]
fn server_mirrors_argv_and_names_the_script() {
    // `$_SERVER['argv']`/`argc` are copies of the globals, and `PHP_SELF` /
    // `SCRIPT_NAME` repeat `$argv[0]`. `SCRIPT_FILENAME` is the one that does not
    // substitute a stand-in name: with no file it is empty.
    assert_eq!(
        run_r(
            "echo $_SERVER['argc'], \"|\", $_SERVER['argv'][1], \"|\", \
             $_SERVER['PHP_SELF'], \"|\", $_SERVER['SCRIPT_NAME'], \"|\", \
             var_export($_SERVER['SCRIPT_FILENAME'], true);",
            &["one"]
        ),
        "2|one|Standard input code|Standard input code|''"
    );
    // A file names itself in all three — and as WRITTEN on the command line, not
    // resolved the way `__FILE__` is.
    let (out, path, resolved) = run_file(
        "server",
        "<?php echo $_SERVER['PHP_SELF'] === $_SERVER['SCRIPT_FILENAME'] ? 'same' : 'differ', \
         \"|\", $_SERVER['SCRIPT_NAME'] === $argv[0] ? 'same' : 'differ', \
         \"|\", var_export($argv[0] === __FILE__, true);",
        &[],
    );
    // `$argv[0]` is the path as written and `__FILE__` the resolved one, so they
    // agree only where the two happen to be the same string.
    let file_matches = if resolved == path { "true" } else { "false" };
    assert_eq!(out, format!("same|same|{file_matches}"));
}

#[test]
fn dir_is_the_working_directory_when_there_is_no_file() {
    let cwd = std::env::current_dir().expect("cwd");
    assert_eq!(run_r("echo __DIR__;", &[]), cwd.display().to_string());
    // With a file it is that file's directory instead, whatever the cwd is.
    let (out, _, resolved) = run_file("dir", "<?php echo __DIR__;", &[]);
    assert_eq!(
        out,
        resolved
            .parent()
            .expect("script has a parent")
            .display()
            .to_string()
    );
}
