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
    std::env::temp_dir().join(format!("phplang_runtime_{tag}_{}_{nanos}.txt", std::process::id()))
}

// ── assert ───────────────────────────────────────────────────────────────────

#[test]
fn assert_true_is_true() {
    // A truthy assertion returns true (assertions "enabled" but non-fatal).
    assert_eq!(run("<?php echo var_export(assert(true), true);"), "true");
    assert_eq!(run("<?php echo var_export(assert(true) === true, true);"), "true");
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
    // Emits to stderr, never halts: the following echo must run.
    assert_eq!(
        run("<?php $r = trigger_error('boom'); echo var_export($r, true), '|', 'still here';"),
        "true|still here"
    );
}

#[test]
fn trigger_error_with_level_does_not_halt() {
    assert_eq!(
        run("<?php trigger_error('warn', E_USER_WARNING); echo 'continued';"),
        "continued"
    );
}

// ── error_log ────────────────────────────────────────────────────────────────

#[test]
fn error_log_type3_appends_to_file() {
    let path = temp_path("errlog");
    let p = path.to_string_lossy().replace('\\', "\\\\");
    // Two appends must accumulate in the destination file.
    let src = format!(
        "<?php error_log('first', 3, '{p}'); error_log('second', 3, '{p}'); echo 'ok';"
    );
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
    assert_eq!(run("<?php echo var_export(error_log('to stderr'), true);"), "true");
}

// ── debug_backtrace / debug_print_backtrace ──────────────────────────────────

#[test]
fn debug_backtrace_is_empty_array() {
    assert_eq!(run("<?php $b = debug_backtrace(); echo is_array($b) ? 'arr' : 'no', ':', count($b);"), "arr:0");
}

#[test]
fn debug_print_backtrace_returns_null() {
    assert_eq!(run("<?php echo var_export(debug_print_backtrace(), true);"), "NULL");
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
fn restore_and_exception_handlers_return_null() {
    assert_eq!(run("<?php echo var_export(restore_error_handler(), true);"), "NULL");
    assert_eq!(
        run("<?php echo var_export(set_exception_handler(function(){}), true);"),
        "NULL"
    );
    assert_eq!(run("<?php echo var_export(restore_exception_handler(), true);"), "NULL");
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
