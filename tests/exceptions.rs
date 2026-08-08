//! End-to-end tests for PHP exceptions: `throw` (statement and PHP 8 expression),
//! `try`/`catch`/`finally` (multi-catch unions, optional var, finally-always), the
//! built-in exception hierarchy with disjoint `Exception`/`Error` roots,
//! `UnhandledMatchError`, callback-throw propagation, and uncaught-fatal shaping.
//! Source in, captured `echo` output out — exercising compile → lower →
//! orchestrated run-on-fusevm.

use phplang::{eval_capture, eval_str};

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

// ── basic catch + getMessage ─────────────────────────────────────────────────

#[test]
fn basic_catch_get_message() {
    let src = r#"<?php try { throw new Exception("boom"); }
        catch (Exception $e) { echo $e->getMessage(); }"#;
    assert_eq!(run(src), "boom");
}

// ── finally runs on the normal (non-throwing) path ───────────────────────────

#[test]
fn finally_on_normal_path() {
    let src = r#"<?php try { echo "T"; } finally { echo "F"; }"#;
    assert_eq!(run(src), "TF");
}

// ── finally runs even when a catch returns (from inside a function) ──────────

#[test]
fn finally_runs_before_catch_return() {
    let src = r#"<?php
        function g() {
            try { throw new Exception("x"); }
            catch (Exception $e) { return "R"; }
            finally { echo "F"; }
        }
        echo g();"#;
    // finally's echo happens before the returned value is emitted → "F" then "R".
    assert_eq!(run(src), "FR");
}

// ── finally runs when a catch rethrows, before the throw propagates ──────────

#[test]
fn finally_runs_before_catch_rethrow_propagates() {
    let src = r#"<?php
        function g() {
            try { throw new Exception("first"); }
            catch (Exception $e) { throw new Exception("second"); }
            finally { echo "F"; }
        }
        try { g(); } catch (Exception $e) { echo $e->getMessage(); }"#;
    // The rethrow's finally echoes "F"; the "second" exception then reaches the
    // outer catch. If finally were skipped on a catch-throw, "F" would be absent.
    assert_eq!(run(src), "Fsecond");
}

// ── no cross-construct signal leak ───────────────────────────────────────────

#[test]
fn no_signal_leak_across_constructs() {
    let src = r#"<?php
        echo (function () { return "A"; })();
        try { echo "X"; } finally { echo "Y"; }
        echo "Z";"#;
    // The IIFE's return must not bleed into the later try; the try's normal
    // completion must not swallow the trailing echo.
    assert_eq!(run(src), "AXYZ");
}

// ── disjoint Exception/Error roots ───────────────────────────────────────────

#[test]
fn type_error_not_caught_by_exception() {
    // catch(Exception) must NOT catch a TypeError → it reaches the outer catch(Error).
    let src = r#"<?php
        try {
            try { throw new TypeError("bad type"); }
            catch (Exception $e) { echo "wrong"; }
        } catch (Error $e) { echo "err:", $e->getMessage(); }"#;
    assert_eq!(run(src), "err:bad type");
}

#[test]
fn type_error_caught_by_error_and_by_typeerror() {
    let by_error = r#"<?php try { throw new TypeError("t"); }
        catch (Error $e) { echo "E"; }"#;
    assert_eq!(run(by_error), "E");
    let by_self = r#"<?php try { throw new TypeError("t"); }
        catch (TypeError $e) { echo "T"; }"#;
    assert_eq!(run(by_self), "T");
}

// ── subclass caught by a base class ──────────────────────────────────────────

#[test]
fn subclass_caught_by_base() {
    // InvalidArgumentException → LogicException → Exception; catch by LogicException.
    let src = r#"<?php try { throw new InvalidArgumentException("i"); }
        catch (LogicException $e) { echo $e->getMessage(); }"#;
    assert_eq!(run(src), "i");
}

// ── throw as an expression (PHP 8) ───────────────────────────────────────────

#[test]
fn throw_expression_in_coalesce() {
    // LHS null → the `?? throw` fires and is caught.
    let thrown = r#"<?php $v = null;
        try { $r = $v ?? throw new Exception("e"); echo "no"; }
        catch (Exception $e) { echo $e->getMessage(); }"#;
    assert_eq!(run(thrown), "e");
    // LHS non-null → the throw never runs; the value flows through.
    let not_thrown = r#"<?php $v = "kept";
        $r = $v ?? throw new Exception("e");
        echo $r;"#;
    assert_eq!(run(not_thrown), "kept");
}

// ── unmatched match with no default throws UnhandledMatchError ────────────────

#[test]
fn unhandled_match_error() {
    let src = r#"<?php
        try { echo match (99) { 1 => "a", 2 => "b" }; }
        catch (UnhandledMatchError $e) { echo "caught:", $e->getMessage(); }"#;
    assert_eq!(run(src), "caught:Unhandled match case 99");
}

// ── a throwing callback stops iteration and propagates ───────────────────────

#[test]
fn callback_throw_propagates() {
    let src = r#"<?php
        try {
            $r = array_map(fn ($x) => $x == 2 ? throw new Exception("stop") : $x, [1, 2, 3]);
            echo "done";
        } catch (Exception $e) { echo $e->getMessage(); }"#;
    assert_eq!(run(src), "stop");
}

// ── multi-catch union ────────────────────────────────────────────────────────

#[test]
fn multi_catch_union() {
    let type_err = r#"<?php try { throw new TypeError("te"); }
        catch (TypeError | ValueError $e) { echo $e->getMessage(); }"#;
    assert_eq!(run(type_err), "te");
    let value_err = r#"<?php try { throw new ValueError("ve"); }
        catch (TypeError | ValueError $e) { echo $e->getMessage(); }"#;
    assert_eq!(run(value_err), "ve");
}

// ── an uncaught exception is a top-level fatal error ─────────────────────────

#[test]
fn uncaught_is_fatal_error() {
    let src = r#"<?php throw new Exception("unhandled");"#;
    let err = eval_str(src).expect_err("uncaught exception must surface as an Err");
    assert_eq!(err, "PHP Fatal error:  Uncaught Exception: unhandled");
}

#[test]
fn uncaught_error_root_shapes_fatal() {
    let err = eval_str(r#"<?php throw new TypeError("nope");"#)
        .expect_err("uncaught error must surface as an Err");
    assert_eq!(err, "PHP Fatal error:  Uncaught TypeError: nope");
}

// ── extra coverage ───────────────────────────────────────────────────────────

#[test]
fn catch_without_variable_is_allowed() {
    let src = r#"<?php try { throw new Exception("x"); }
        catch (Exception) { echo "caught"; }"#;
    assert_eq!(run(src), "caught");
}

#[test]
fn throwable_catches_both_roots() {
    let ex = r#"<?php try { throw new Exception("a"); } catch (Throwable $e) { echo "1"; }"#;
    assert_eq!(run(ex), "1");
    let er = r#"<?php try { throw new TypeError("a"); } catch (Throwable $e) { echo "2"; }"#;
    assert_eq!(run(er), "2");
}

#[test]
fn finally_runs_when_try_body_returns() {
    // A plain `return` from the try body (no throw) still runs finally first.
    let src = r#"<?php
        function g() {
            try { return "R"; }
            finally { echo "F"; }
        }
        echo g();"#;
    assert_eq!(run(src), "FR");
}

#[test]
fn nested_try_inner_catch_then_outer_finally() {
    let src = r#"<?php
        try {
            try { throw new RuntimeException("inner"); }
            catch (RuntimeException $e) { echo "c", $e->getMessage(); }
        } finally { echo "-fin"; }"#;
    assert_eq!(run(src), "cinner-fin");
}

#[test]
fn getcode_returns_constructor_code() {
    let src = r#"<?php try { throw new Exception("m", 42); }
        catch (Exception $e) { echo $e->getCode(); }"#;
    assert_eq!(run(src), "42");
}

#[test]
fn break_inside_try_in_loop_runs_finally_then_breaks() {
    // A `break` in a try body (no loop in that detached chunk) still leaves the
    // enclosing loop, running finally first.
    let src = r#"<?php
        for ($i = 0; $i < 3; $i++) {
            try { if ($i == 1) { break; } echo $i; }
            finally { echo "f"; }
        }
        echo "end";"#;
    // i=0: echo 0, finally f; i=1: finally f, break → "0ffend".
    assert_eq!(run(src), "0ffend");
}

#[test]
fn user_exception_subclass_is_catchable_by_base() {
    // A user-defined class extending a built-in exception participates in the
    // real class hierarchy (catchable by its base).
    let src = r#"<?php
        class MyError extends RuntimeException {}
        try { throw new MyError("custom"); }
        catch (Exception $e) { echo $e->getMessage(); }"#;
    assert_eq!(run(src), "custom");
}

#[test]
fn leading_backslash_global_namespace_prefix() {
    // The `\` global-namespace prefix on class and function names is accepted
    // (phplang has no namespaces, so `\Exception` == `Exception`).
    let src = r#"<?php
        try { throw new \Exception("boom"); }
        catch (\Throwable $e) { echo \strlen($e->getMessage()); }"#;
    assert_eq!(run(src), "4");
}

// ── engine-raised errors ────────────────────────────────────────────────────
//
// A zero divisor and a method call on a non-object used to abort with an
// uncatchable host error; both are catchable Throwables now.

#[test]
fn zero_divisor_throws_a_catchable_division_by_zero_error() {
    let src = r#"<?php
        foreach ([1, 2, 3] as $k) {
            try {
                if ($k == 1) { $x = 1 / 0; }
                if ($k == 2) { $x = 1 % 0; }
                if ($k == 3) { intdiv(1, 0); }
            } catch (DivisionByZeroError $e) { echo get_class($e), ":", $e->getMessage(), ";"; }
        }"#;
    assert_eq!(
        run(src),
        "DivisionByZeroError:Division by zero;DivisionByZeroError:Modulo by zero;\
         DivisionByZeroError:Division by zero;"
    );
}

#[test]
fn method_call_on_a_non_object_throws_error_naming_the_type() {
    // PHP spells booleans as `true`/`false` in this message, not `bool`.
    let src = r#"<?php
        foreach ([null, 1, "s", 1.5, true, []] as $v) {
            try { $v->foo(); } catch (Error $e) { echo $e->getMessage(), ";"; }
        }"#;
    assert_eq!(
        run(src),
        "Call to a member function foo() on null;Call to a member function foo() on int;\
         Call to a member function foo() on string;Call to a member function foo() on float;\
         Call to a member function foo() on true;Call to a member function foo() on array;"
    );
}

#[test]
fn exceptions_carry_a_previous() {
    let src = r#"<?php
        try { throw new Exception("outer", 7, new RuntimeException("inner")); }
        catch (Exception $e) { echo $e->getCode(), ":", $e->getPrevious()->getMessage(); }"#;
    assert_eq!(run(src), "7:inner");
    // With no previous supplied it is null.
    assert_eq!(
        run(
            r#"<?php try { throw new Exception("x"); } catch (Exception $e) { var_dump($e->getPrevious()); }"#
        ),
        "NULL\n"
    );
}
