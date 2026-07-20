//! End-to-end tests for the `callable` stdlib category (`src/stdlib/callable.rs`).
//! PHP source in, captured `echo` output out. Expected values were cross-checked
//! against PHP 8's reference `php` CLI.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

// ── call_user_func ───────────────────────────────────────────────────────────

#[test]
fn call_user_func_builtin_name() {
    assert_eq!(run("<?php echo call_user_func('strtoupper', 'hi');"), "HI");
}

#[test]
fn call_user_func_multiple_args() {
    assert_eq!(
        run("<?php echo call_user_func('str_repeat', 'ab', 3);"),
        "ababab"
    );
}

#[test]
fn call_user_func_user_function() {
    assert_eq!(
        run("<?php function add($a,$b){return $a+$b;} echo call_user_func('add', 2, 5);"),
        "7"
    );
}

#[test]
fn call_user_func_closure() {
    assert_eq!(
        run("<?php $f = function($x){ return $x * 2; }; echo call_user_func($f, 21);"),
        "42"
    );
}

#[test]
fn call_user_func_arrow_fn() {
    assert_eq!(
        run("<?php $f = fn($x) => $x + 1; echo call_user_func($f, 9);"),
        "10"
    );
}

#[test]
fn call_user_func_no_args() {
    assert_eq!(
        run("<?php function greet(){return 'hey';} echo call_user_func('greet');"),
        "hey"
    );
}

// ── call_user_func_array ─────────────────────────────────────────────────────

#[test]
fn call_user_func_array_builtin() {
    assert_eq!(
        run("<?php echo call_user_func_array('str_repeat', ['xy', 2]);"),
        "xyxy"
    );
}

#[test]
fn call_user_func_array_user_function() {
    assert_eq!(
        run("<?php function sum3($a,$b,$c){return $a+$b+$c;} echo call_user_func_array('sum3', [1,2,3]);"),
        "6"
    );
}

#[test]
fn call_user_func_array_closure() {
    assert_eq!(
        run("<?php $f = function($a,$b){ return $a . $b; }; echo call_user_func_array($f, ['foo','bar']);"),
        "foobar"
    );
}

#[test]
fn call_user_func_array_empty_array() {
    assert_eq!(
        run("<?php function pi_ish(){return 'P';} echo call_user_func_array('pi_ish', []);"),
        "P"
    );
}

// ── function_exists ──────────────────────────────────────────────────────────

#[test]
fn function_exists_user_function() {
    assert_eq!(
        run("<?php function myfn(){} echo function_exists('myfn') ? 'y' : 'n';"),
        "y"
    );
}

#[test]
fn function_exists_user_function_case_insensitive() {
    assert_eq!(
        run("<?php function MyFn(){} echo function_exists('myfn') ? 'y' : 'n';"),
        "y"
    );
}

#[test]
fn function_exists_known_builtin() {
    assert_eq!(
        run("<?php echo function_exists('strlen') ? 'y' : 'n';"),
        "y"
    );
}

#[test]
fn function_exists_known_builtin_case_insensitive() {
    assert_eq!(
        run("<?php echo function_exists('STRLEN') ? 'y' : 'n';"),
        "y"
    );
}

#[test]
fn function_exists_unknown() {
    assert_eq!(
        run("<?php echo function_exists('no_such_function_xyz') ? 'y' : 'n';"),
        "n"
    );
}
