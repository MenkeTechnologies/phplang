//! Closures and arrow functions as first-class callables: `use` capture,
//! implicit arrow-fn capture, closures as builtin callbacks, closures returned
//! from and produced inside functions, immediate invocation, default/variadic
//! closure parameters, and truthiness. Source in, captured `echo` out.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn closure_stored_in_variable_and_called() {
    let src = r#"<?php $f = function($x) { return $x + 1; }; echo $f(41);"#;
    assert_eq!(run(src), "42");
}

#[test]
fn closure_captures_use_var_by_value() {
    // `use ($y)` captures the value at creation time; a later reassignment of
    // $y does not change what the closure saw.
    let src = r#"<?php
        $y = 10;
        $add = function($x) use ($y) { return $x + $y; };
        $y = 999;
        echo $add(5);"#;
    assert_eq!(run(src), "15");
}

#[test]
fn closure_multiple_use_vars() {
    let src = r#"<?php
        $a = 2; $b = 3;
        $f = function($x) use ($a, $b) { return $x * $a + $b; };
        echo $f(10);"#;
    assert_eq!(run(src), "23");
}

#[test]
fn arrow_captures_free_var_by_value() {
    let src = r#"<?php
        $base = 100;
        $f = fn($x) => $x + $base;
        echo $f(5);"#;
    assert_eq!(run(src), "105");
}

#[test]
fn arrow_single_expression_body() {
    let src = r#"<?php $sq = fn($x) => $x * $x; echo $sq(9);"#;
    assert_eq!(run(src), "81");
}

#[test]
fn nested_arrow_captures_outer_param() {
    // The outer arrow captures nothing free; the inner captures $x. `$add(3)(4)`.
    let src = r#"<?php $add = fn($x) => fn($y) => $x + $y; echo $add(3)(4);"#;
    assert_eq!(run(src), "7");
}

#[test]
fn closure_as_array_map_callback() {
    let src = r#"<?php
        $double = function($x) { return $x * 2; };
        echo implode(",", array_map($double, [1, 2, 3]));"#;
    assert_eq!(run(src), "2,4,6");
}

#[test]
fn arrow_as_array_map_callback() {
    let src = r#"<?php echo implode(",", array_map(fn($x) => $x * $x, [1, 2, 3]));"#;
    assert_eq!(run(src), "1,4,9");
}

#[test]
fn closure_as_array_filter_callback() {
    let src = r#"<?php
        $even = fn($x) => $x % 2 == 0;
        echo implode(",", array_filter([1, 2, 3, 4, 5, 6], $even));"#;
    assert_eq!(run(src), "2,4,6");
}

#[test]
fn closure_as_array_reduce_callback() {
    let src = r#"<?php
        echo array_reduce([1, 2, 3, 4], fn($c, $x) => $c + $x, 0);"#;
    assert_eq!(run(src), "10");
}

#[test]
fn returning_a_closure_from_a_function() {
    // A factory that closes over its argument and returns the closure.
    let src = r#"<?php
        function adder($n) {
            return function($x) use ($n) { return $x + $n; };
        }
        $add10 = adder(10);
        echo $add10(7);"#;
    assert_eq!(run(src), "17");
}

#[test]
fn immediately_invoked_closure() {
    let src = r#"<?php echo (function($x) { return $x * 3; })(14);"#;
    assert_eq!(run(src), "42");
}

#[test]
fn closure_with_default_parameter() {
    let src = r#"<?php $g = function($x, $y = 5) { return $x + $y; }; echo $g(10);"#;
    assert_eq!(run(src), "15");
}

#[test]
fn closure_with_variadic_parameter() {
    let src = r#"<?php $s = function(...$n) { return array_sum($n); }; echo $s(1, 2, 3);"#;
    assert_eq!(run(src), "6");
}

#[test]
fn closure_is_truthy_in_boolean_contexts() {
    // A closure handle is always truthy; `empty()` on it is false; `gettype` is
    // "object" (verified against PHP 8.5).
    let src = r#"<?php
        $f = function() { return 1; };
        $t = $f ? "T" : "F";
        $e = empty($f) ? "E" : "N";
        echo $t, ":", $e, ":", gettype($f);"#;
    assert_eq!(run(src), "T:N:object");
}

#[test]
fn is_callable_on_closure_and_name() {
    let src = r#"<?php
        $f = function() {};
        echo is_callable($f) ? "Y" : "N", is_callable("strlen") ? "Y" : "N";"#;
    assert_eq!(run(src), "YY");
}

#[test]
fn by_reference_use_capture_shares_the_variable() {
    // `use (&$v)` binds the closure's name to the enclosing variable's reference
    // cell, so writes cross in both directions. Values checked against php 8.5.9.
    let counter = r#"<?php $n = 0; $bump = function() use (&$n) { $n++; };
        $bump(); $bump(); $bump(); echo $n;"#;
    assert_eq!(run(counter), "3");

    // A write made after the closure was created is visible inside it — which is
    // the difference from a by-value capture, checked here in one program.
    let both = r#"<?php $c = 5;
        $byval = function() use ($c) { return $c; };
        $byref = function() use (&$c) { return $c; };
        $c = 9;
        echo $byval(), "/", $byref();"#;
    assert_eq!(run(both), "5/9");

    // The captured variable outlives the scope it came from.
    let closed_over = r#"<?php
        $mk = function() { $n = 0; return function() use (&$n) { return ++$n; }; };
        $f = $mk(); echo $f(), $f(), $f();"#;
    assert_eq!(run(closed_over), "123");

    // An array captured by reference is mutated in place, not copied.
    let arr = r#"<?php $log = []; $add = function($x) use (&$log) { $log[] = $x; };
        $add('a'); $add('b'); echo count($log), implode(',', $log);"#;
    assert_eq!(run(arr), "2a,b");
}
