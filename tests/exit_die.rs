//! `exit` / `die` — the construct that ends the request, and the process exit
//! status it leaves behind.
//!
//! These tests shell out to the binary rather than using `eval_capture`, because
//! the status is the whole point and an in-process run has none: `exit(3)` and
//! `exit(0)` produce identical output and differ only in what the shell sees.
//! That is exactly the axis the rest of the suite cannot reach.
//!
//! Every expectation is the verbatim `(stdout, status)` of the same command
//! under the reference `php` 8.5.9, captured by running it — not recalled. The
//! commands are in the comment above each assertion so they can be re-run.

use std::process::{Command, Stdio};

/// Run `code` through the crate's binary and return `(stdout, exit status)`.
/// stderr is dropped: it carries the `log_errors` copy of a diagnostic, while
/// stdout carries the `display_errors` copy interleaved with the program's own
/// output. `tests/error_streams.rs` is where the stderr half is pinned.
fn run(code: &str) -> (String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_php"))
        .arg("-r")
        .arg(code)
        .stderr(Stdio::null())
        .output()
        .expect("spawn php -r");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

// ── the status reaches the shell ─────────────────────────────────────────────

#[test]
fn an_int_status_becomes_the_process_exit_code() {
    // php -r 'echo "a"; exit(3); echo "b";'  →  "a", status 3
    assert_eq!(run(r#"echo "a"; exit(3); echo "b";"#), ("a".into(), 3));
    // A run that never calls `exit` still exits 0.
    assert_eq!(run(r#"echo "a";"#), ("a".into(), 0));
}

#[test]
fn the_status_is_taken_modulo_256() {
    // Only the low byte survives waitpid, and the reference's arithmetic is the
    // same: 300 → 44, -1 → 255, 256 → 0. A naive `as u8` cast agrees on 300 but
    // not on -1, which is why all three are pinned.
    assert_eq!(run("exit(300);"), (String::new(), 44));
    assert_eq!(run("exit(-1);"), (String::new(), 255));
    assert_eq!(run("exit(256);"), (String::new(), 0));
    assert_eq!(run("exit(255);"), (String::new(), 255));
}

#[test]
fn a_string_status_is_printed_and_the_status_is_zero() {
    // php -r 'exit("msg");'  →  "msg", status 0. The string is NOT parsed as a
    // number: `exit("7")` prints 7 and still exits 0.
    assert_eq!(run(r#"exit("msg");"#), ("msg".into(), 0));
    assert_eq!(run(r#"exit("7");"#), ("7".into(), 0));
    assert_eq!(run(r#"die("bye");"#), ("bye".into(), 0));
}

#[test]
fn parentheses_and_argument_are_both_optional() {
    // `exit;` is a complete expression — the case that fails outright if the
    // construct is left to the bareword path, where it reads as a constant.
    assert_eq!(run(r#"echo "a"; exit; echo "b";"#), ("a".into(), 0));
    assert_eq!(run(r#"echo "a"; exit(); echo "b";"#), ("a".into(), 0));
    assert_eq!(run(r#"echo "a"; die; echo "b";"#), ("a".into(), 0));
    assert_eq!(run(r#"echo "a"; die(); echo "b";"#), ("a".into(), 0));
}

// ── the `string|int` parameter's conversions ─────────────────────────────────

#[test]
fn a_bool_narrows_to_int() {
    assert_eq!(run("exit(true);"), (String::new(), 1));
    assert_eq!(run("exit(false);"), (String::new(), 0));
}

#[test]
fn a_fractional_float_narrows_with_the_precision_deprecation() {
    // php -r 'exit(2.9);' → status 2 plus the same Deprecated any other int-only
    // position raises. An INTEGRAL float narrows in silence.
    assert_eq!(
        run("exit(2.9);"),
        (
            "\nDeprecated: Implicit conversion from float 2.9 to int loses precision \
             in Command line code on line 1\n"
                .into(),
            2
        )
    );
    assert_eq!(run("exit(2.0);"), (String::new(), 2));
}

#[test]
fn an_explicit_null_is_deprecated_and_reads_as_zero() {
    // Distinct from `exit()` with NO argument, which is silent — the deprecation
    // is about passing null, not about defaulting.
    assert_eq!(
        run("exit(null);"),
        (
            "\nDeprecated: exit(): Passing null to parameter #1 ($status) of type string|int \
             is deprecated in Command line code on line 1\n"
                .into(),
            0
        )
    );
    assert_eq!(run("exit();"), (String::new(), 0));
}

#[test]
fn a_stringable_object_satisfies_the_string_half_of_the_union() {
    assert_eq!(
        run(r#"exit(new class { function __toString(){ return "S"; } });"#),
        ("S".into(), 0)
    );
}

#[test]
fn anything_that_is_neither_string_nor_int_is_a_type_error() {
    // The trace names the library call as frame #0 and the diagnostic quotes
    // `exit()` even for the `die` spelling, because the scanner folds the two.
    let expected = "\nFatal error: Uncaught TypeError: exit(): Argument #1 ($status) must be of \
                    type string|int, array given in Command line code:1\nStack trace:\n\
                    #0 Command line code(1): exit(Array)\n#1 {main}\n  thrown in Command line \
                    code on line 1\n";
    assert_eq!(run("exit([1]);"), (expected.into(), 255));
    assert_eq!(run("die([1]);"), (expected.into(), 255));
}

// ── the unwind ───────────────────────────────────────────────────────────────

#[test]
fn exit_unwinds_out_of_a_function_body() {
    assert_eq!(
        run(r#"function f(){ exit(4); } f(); echo "no";"#),
        (String::new(), 4)
    );
}

#[test]
fn exit_unwinds_out_of_a_library_callback() {
    // An `exit` inside an `array_map` callback stops the walk where it stands:
    // the first element's echo has run, the third's has not. A dispatcher that
    // tested only for a pending THROW would keep iterating.
    assert_eq!(
        run(r#"array_map(function($x){ if ($x == 2) exit(9); echo $x; }, [1, 2, 3]);"#),
        ("1".into(), 9)
    );
}

#[test]
fn exit_is_not_catchable_and_skips_finally() {
    // Both halves matter and they fail differently: a catchable exit prints
    // "caught", and one that ran `finally` prints "fin".
    assert_eq!(
        run(r#"try { exit(7); } catch (Throwable $e) { echo "caught"; }"#),
        (String::new(), 7)
    );
    assert_eq!(
        run(r#"try { exit(5); } finally { echo "fin"; }"#),
        (String::new(), 5)
    );
}

#[test]
fn open_output_buffers_are_still_flushed() {
    // The unwind is not a teardown: what `ob_start` collected before the `exit`
    // still reaches stdout, as it does under the reference.
    assert_eq!(
        run(r#"ob_start(); echo "buf"; exit(1);"#),
        ("buf".into(), 1)
    );
}

// ── the PHP 8.4 function registration ────────────────────────────────────────

#[test]
fn exit_and_die_are_also_callable_functions() {
    // PHP 8.4 registered them, so `function_exists` answers true for both and a
    // variable function reaches the construct.
    assert_eq!(
        run(r#"var_dump(function_exists("exit"), function_exists("die"));"#),
        ("bool(true)\nbool(true)\n".into(), 0)
    );
    assert_eq!(run(r#"$f = "exit"; $f(3);"#), (String::new(), 3));
}
