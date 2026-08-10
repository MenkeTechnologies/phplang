//! End-to-end tests for the `runtime` stdlib category (`src/stdlib/runtime.rs`).
//! These functions are deliberately graceful: assertions never throw,
//! `trigger_error` never halts, and the handler-registration functions are
//! documented no-ops. Behavior cross-checked against PHP 8's semantics where a
//! non-fatal analogue exists.

use phplang::eval_capture;
use std::time::{SystemTime, UNIX_EPOCH};

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

/// Unique temp path under the OS temp dir; caller removes it.
fn temp_path(tag: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "phplang_runtime_{tag}_{}_{nanos}.txt",
        std::process::id()
    ))
}

// ── assert ───────────────────────────────────────────────────────────────────

#[test]
fn assert_true_is_true() {
    // A truthy assertion returns true (assertions "enabled" but non-fatal).
    assert_eq!(run("<?php echo var_export(assert(true), true);"), "true");
    assert_eq!(
        run("<?php echo var_export(assert(true) === true, true);"),
        "true"
    );
}

#[test]
fn assert_false_is_false_and_does_not_halt() {
    // A falsy assertion returns false without throwing; program continues.
    assert_eq!(
        run("<?php $r = assert(false); echo var_export($r, true), '|', 'after';"),
        "false|after"
    );
}

#[test]
fn assert_accepts_description_argument() {
    // The optional description is accepted and ignored; still returns true.
    assert_eq!(
        run("<?php echo var_export(assert(1 === 1, 'must be equal'), true);"),
        "true"
    );
}

#[test]
fn assert_options_returns_zero() {
    assert_eq!(run("<?php echo assert_options(1);"), "0");
}

// ── trigger_error ────────────────────────────────────────────────────────────

#[test]
fn trigger_error_returns_true_and_continues() {
    // The diagnostic is DISPLAYED — on stdout, interleaved with the program's
    // own output, exactly like an engine-raised one — and the call answers true
    // without halting, so the following echo runs.
    //
    // php -r "$r = trigger_error('boom'); echo var_export($r, true), '|', 'still here';"
    assert_eq!(
        run("<?php $r = trigger_error('boom'); echo var_export($r, true), '|', 'still here';"),
        "\nNotice: boom in Command line code on line 1\ntrue|still here"
    );
}

#[test]
fn trigger_error_with_level_does_not_halt() {
    // php -r "trigger_error('warn', E_USER_WARNING); echo 'continued';"
    assert_eq!(
        run("<?php trigger_error('warn', E_USER_WARNING); echo 'continued';"),
        "\nWarning: warn in Command line code on line 1\ncontinued"
    );
}

#[test]
fn trigger_error_obeys_error_reporting_and_suppression() {
    // Being a real diagnostic means both gates reach it. Neither was reachable
    // while it printed its own line straight to stderr.
    //
    // php -r "error_reporting(0); trigger_error('m'); echo 'z';"      → "z"
    // php -r "@trigger_error('m'); echo 'q';"                          → "q"
    assert_eq!(
        run("<?php error_reporting(0); trigger_error('m'); echo 'z';"),
        "z"
    );
    assert_eq!(run("<?php @trigger_error('m'); echo 'q';"), "q");
}

#[test]
fn trigger_error_rejects_a_level_that_is_not_one_of_the_four() {
    // An ENGINE level such as E_WARNING is not a fallback — it is a ValueError.
    //
    // php -r "try { trigger_error('m', E_WARNING); } catch (ValueError \$e) { echo \$e->getMessage(); }"
    assert_eq!(
        run("<?php try { trigger_error('m', E_WARNING); } catch (ValueError $e) { echo $e->getMessage(); }"),
        "trigger_error(): Argument #2 ($error_level) must be one of E_USER_ERROR, \
         E_USER_WARNING, E_USER_NOTICE, or E_USER_DEPRECATED"
    );
}

// ── error_log ────────────────────────────────────────────────────────────────

#[test]
fn error_log_type3_appends_to_file() {
    let path = temp_path("errlog");
    let p = path.to_string_lossy().replace('\\', "\\\\");
    // Two appends must accumulate in the destination file.
    let src =
        format!("<?php error_log('first', 3, '{p}'); error_log('second', 3, '{p}'); echo 'ok';");
    let out = run(&src);
    let contents = std::fs::read_to_string(&path).unwrap_or_default();
    let _ = std::fs::remove_file(&path);
    assert_eq!(out, "ok");
    assert_eq!(contents, "firstsecond");
}

#[test]
fn error_log_type3_returns_true() {
    let path = temp_path("errlogret");
    let p = path.to_string_lossy().replace('\\', "\\\\");
    let src = format!("<?php echo var_export(error_log('x', 3, '{p}'), true);");
    let out = run(&src);
    let _ = std::fs::remove_file(&path);
    assert_eq!(out, "true");
}

#[test]
fn error_log_stderr_default_returns_true() {
    // type 0 (default) goes to stderr and returns true.
    assert_eq!(
        run("<?php echo var_export(error_log('to stderr'), true);"),
        "true"
    );
}

// ── debug_backtrace / debug_print_backtrace ──────────────────────────────────

#[test]
fn debug_backtrace_is_empty_array() {
    assert_eq!(
        run("<?php $b = debug_backtrace(); echo is_array($b) ? 'arr' : 'no', ':', count($b);"),
        "arr:0"
    );
}

#[test]
fn debug_print_backtrace_returns_null() {
    assert_eq!(
        run("<?php echo var_export(debug_print_backtrace(), true);"),
        "NULL"
    );
}

// ── error/exception handler registration (no-ops) ────────────────────────────

#[test]
fn set_error_handler_returns_null() {
    assert_eq!(
        run("<?php echo var_export(set_error_handler(function(){}), true);"),
        "NULL"
    );
}

#[test]
fn set_returns_null_restore_returns_true() {
    // set_* return the previous handler (none → null); restore_* return bool true
    // per the PHP manual.
    assert_eq!(
        run("<?php echo var_export(set_exception_handler(function(){}), true);"),
        "NULL"
    );
    assert_eq!(
        run("<?php echo var_export(restore_error_handler(), true);"),
        "true"
    );
    assert_eq!(
        run("<?php echo var_export(restore_exception_handler(), true);"),
        "true"
    );
}

// ── shutdown / autoload registration ─────────────────────────────────────────

#[test]
fn register_shutdown_function_returns_null() {
    assert_eq!(
        run("<?php echo var_export(register_shutdown_function(function(){}), true);"),
        "NULL"
    );
}

#[test]
fn spl_autoload_register_returns_true() {
    assert_eq!(
        run("<?php echo var_export(spl_autoload_register(function(){}), true);"),
        "true"
    );
}

// ── get_defined_vars ─────────────────────────────────────────────────────────

#[test]
fn get_defined_vars_is_empty_array() {
    assert_eq!(
        run("<?php $x = 1; $v = get_defined_vars(); echo is_array($v) ? 'arr' : 'no', ':', count($v);"),
        "arr:0"
    );
}

// ── class_alias ──────────────────────────────────────────────────────────────

#[test]
fn class_alias_true_for_existing_class() {
    assert_eq!(
        run("<?php class Foo {} echo var_export(class_alias('Foo', 'Bar'), true);"),
        "true"
    );
}

#[test]
fn class_alias_false_for_missing_class() {
    assert_eq!(
        run("<?php echo var_export(class_alias('NoSuchClass', 'Alias'), true);"),
        "false"
    );
}
