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

/// Inside a function the answer is the frame's own bound variables, in binding
/// order — every expectation here is what php 8.5 prints for the same source.
/// This used to be a stub returning an empty array.
#[test]
fn get_defined_vars_lists_the_frames_bound_variables() {
    // Parameters first, then locals at first mention. An unset name is gone; a
    // name bound to null stays, which is the whole reason unset and null are
    // stored differently.
    assert_eq!(
        run("<?php function f($p) { $a = 1; $b = null; unset($a); print_r(array_keys(get_defined_vars())); } f(7);"),
        "Array\n(\n    [0] => p\n    [1] => b\n)\n"
    );
    assert_eq!(
        run("<?php function f() { $v = null; var_dump(array_key_exists('v', get_defined_vars())); } f();"),
        "bool(true)\n"
    );
    assert_eq!(
        run("<?php function f() { $v = 1; unset($v); var_dump(array_key_exists('v', get_defined_vars())); } f();"),
        "bool(false)\n"
    );
    // A function that binds nothing answers an empty array — the old stub's
    // answer, now for the right reason.
    assert_eq!(
        run("<?php function f() { $v = get_defined_vars(); echo is_array($v) ? 'arr' : 'no', ':', count($v); } f();"),
        "arr:0"
    );
    // `extract` and a closure's `use` bind through the same table.
    assert_eq!(
        run("<?php function f() { extract(['e' => 5]); print_r(get_defined_vars()); } f();"),
        "Array\n(\n    [e] => 5\n)\n"
    );
    assert_eq!(
        run("<?php function f() { $x = 1; $c = function () use ($x) { print_r(get_defined_vars()); }; $c(); } f();"),
        "Array\n(\n    [x] => 1\n)\n"
    );
    // `$this` is not one of the method's variables.
    assert_eq!(
        run("<?php class C { function m() { $z = 1; print_r(array_keys(get_defined_vars())); } } (new C)->m();"),
        "Array\n(\n    [0] => z\n)\n"
    );
}

/// At GLOBAL scope the answer still differs from the reference — it lists the
/// superglobals its `variables_order` populated, in its own fixed order and
/// ahead of the script's variables. What holds either way is that the script's
/// own variable is there and `$GLOBALS` is not, so that is what this pins
/// rather than a count that would freeze the divergence in place.
#[test]
fn get_defined_vars_at_global_scope_holds_the_scripts_own_variables() {
    assert_eq!(
        run("<?php $x = 1; $k = get_defined_vars(); var_dump(array_key_exists('x', $k), array_key_exists('GLOBALS', $k), $k['x']);"),
        "bool(true)\nbool(false)\nint(1)\n"
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

// ── func_get_args / func_num_args / func_get_arg ─────────────────────────────

/// Since PHP 7 the reference reports a declared parameter's CURRENT value, not
/// the value the caller passed. This engine answered from a snapshot taken when
/// the frame was bound, so every one of these read back the original.
#[test]
fn the_func_arg_family_reports_a_parameter_s_current_value() {
    assert_eq!(
        run("<?php function f($a) { $a = 99; return implode(',', func_get_args()); } echo f(1);"),
        "99"
    );
    // Only the positions a declared parameter covers are re-read; an argument
    // past them is the value that was passed, which is why writing into a
    // variadic's array changes nothing here.
    assert_eq!(
        run("<?php function f($a,$b) { $a = 99; return implode(',', func_get_args()); } echo f(1,2,3);"),
        "99,2,3"
    );
    assert_eq!(
        run("<?php function f(...$r) { $r[0] = 99; return implode(',', func_get_args()); } echo f(1,2);"),
        "1,2"
    );
    assert_eq!(
        run("<?php function f($a) { $a = 9; return func_get_arg(0); } echo f(1);"),
        "9"
    );
    // The count is what the CALL supplied and a write cannot change it.
    assert_eq!(
        run("<?php function f($a) { $a = 99; return func_num_args(); } echo f(1,2);"),
        "2"
    );
    // A method frame and a closure frame are built by different paths and must
    // agree with the plain function form.
    assert_eq!(
        run("<?php class C { function m($a) { $a = 9; return implode(',', func_get_args()); } } echo (new C)->m(1);"),
        "9"
    );
    assert_eq!(
        run("<?php $f = function($a) { $a = 9; return implode(',', func_get_args()); }; echo $f(1);"),
        "9"
    );
}

/// A named argument is reported at its PARAMETER's position, not in the order it
/// was written, and a position skipped by one reads back as that parameter's
/// default — both of which follow from reading the parameters rather than the
/// call. Reported in call order, `f(b: 2, a: 1)` came back as `2,1`.
#[test]
fn func_get_args_places_a_named_argument_at_its_parameter() {
    assert_eq!(
        run(
            "<?php function f($a,$b) { return implode(',', func_get_args()); } echo f(b: 2, a: 1);"
        ),
        "1,2"
    );
    assert_eq!(
        run("<?php function f($a=1,$b=2,$c=3) { return implode(',', func_get_args()); } echo f(9, c: 7);"),
        "9,2,7"
    );
    assert_eq!(
        run("<?php function f($a=1,$b=2,$c=3) { return func_num_args(); } echo f(9, c: 7);"),
        "3"
    );
}

/// All three are a fatal at the global scope, and the reference words each of
/// them differently — this engine answered `[]`, `0` and null instead.
#[test]
fn the_func_arg_family_is_a_fatal_at_the_global_scope() {
    let err = |call: &str| {
        run(&format!(
            "<?php try {{ {call}; }} catch (\\Error $e) {{ echo $e->getMessage(); }}"
        ))
    };
    assert_eq!(
        err("func_get_args()"),
        "func_get_args() cannot be called from the global scope"
    );
    assert_eq!(
        err("func_get_arg(0)"),
        "func_get_arg() cannot be called from the global scope"
    );
    // Not the same sentence as the other two.
    assert_eq!(
        err("func_num_args()"),
        "func_num_args() must be called from a function context"
    );
}

/// Out of range in the two directions is two DIFFERENT `ValueError`s.
#[test]
fn func_get_arg_rejects_a_position_out_of_range() {
    let err = |pos: &str| {
        run(&format!(
            "<?php function f() {{ try {{ return func_get_arg({pos}); }} \
             catch (\\Throwable $e) {{ return get_class($e).'|'.$e->getMessage(); }} }} echo f(1);"
        ))
    };
    assert_eq!(
        err("5"),
        "ValueError|func_get_arg(): Argument #1 ($position) must be less than the number of \
         the arguments passed to the currently executed function"
    );
    assert_eq!(
        err("-1"),
        "ValueError|func_get_arg(): Argument #1 ($position) must be greater than or equal to 0"
    );
}

/// The stack trace renders a frame's arguments from the same place
/// `func_get_args()` reads, so a mutated parameter shows its current value there
/// too — `f(99)`, not `f(1)`.
#[test]
fn a_trace_frame_shows_a_parameter_s_current_value() {
    assert_eq!(
        run(
            "<?php function f($a) { $a = 99; throw new Exception('x'); } \
             try { f(1); } catch (Exception $e) { echo $e->getTraceAsString(); }"
        ),
        "#0 Command line code(1): f(99)\n#1 {main}"
    );
}
