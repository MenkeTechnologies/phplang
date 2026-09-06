//! Closures and generators as OBJECTS to the reflection surface, plus the
//! offset-type and call-order rules the same round settled.
//!
//! A closure is an instance of `Closure` and a generator an instance of
//! `Generator` in the reference; here they were `PhpObj` variants that every
//! predicate missed, so `get_class($gen)` raised a `TypeError` whose own message
//! read "must be of type object, object given". Byte-verified against PHP 8.5.10.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

/// The three values under test, bound to `$g` (generator), `$c` (closure) and
/// `$f` (arrow function).
const SETUP: &str = r#"<?php
    function gf() { yield 1; }
    $g = gf(); $c = function () { return 1; }; $f = fn() => 1;
"#;

fn with_setup(tail: &str) -> String {
    run(&format!("{SETUP}{tail}"))
}

#[test]
fn a_closure_and_a_generator_are_objects() {
    assert_eq!(
        with_setup(r#"var_dump(is_object($g), is_object($c), is_object($f));"#),
        "bool(true)\nbool(true)\nbool(true)\n"
    );
    assert_eq!(
        with_setup(r#"echo get_class($g), "|", get_class($c), "|", get_class($f);"#),
        "Generator|Closure|Closure"
    );
    assert_eq!(
        with_setup(r#"echo get_debug_type($g), "|", $c::class;"#),
        "Generator|Closure"
    );
}

#[test]
fn instanceof_knows_the_engine_class_hierarchy() {
    // A generator implements Iterator, which extends Traversable — none of it
    // written in any class table.
    assert_eq!(
        with_setup(
            r#"var_dump($g instanceof Generator, $g instanceof Iterator, $g instanceof Traversable);"#
        ),
        "bool(true)\nbool(true)\nbool(true)\n"
    );
    assert_eq!(
        with_setup(
            r#"var_dump($c instanceof Closure, $c instanceof Traversable, $g instanceof Closure);"#
        ),
        "bool(true)\nbool(false)\nbool(false)\n"
    );
    assert_eq!(
        with_setup(r#"var_dump(is_a($c, "Closure"), is_subclass_of($g, "Iterator"));"#),
        "bool(true)\nbool(true)\n"
    );
}

#[test]
fn engine_methods_are_visible_to_method_exists() {
    assert_eq!(
        with_setup(
            r#"var_dump(method_exists($g, "current"), method_exists($c, "bindTo"), method_exists($c, "current"));"#
        ),
        "bool(true)\nbool(true)\nbool(false)\n"
    );
}

#[test]
fn method_exists_rejects_a_subject_that_is_neither_object_nor_string() {
    let out = run(
        r#"<?php try { method_exists(null, "x"); } catch (\TypeError $e) { echo $e->getMessage(); }"#,
    );
    assert_eq!(
        out,
        "method_exists(): Argument #1 ($object_or_class) must be of type object|string, null given"
    );
    let out = run(
        r#"<?php try { method_exists(5, "x"); } catch (\TypeError $e) { echo $e->getMessage(); }"#,
    );
    assert!(out.ends_with("int given"), "got {out:?}");
}

#[test]
fn closures_compare_by_identity_and_generators_do_not() {
    // `Closure` is the one class with a compare handler that answers "not equal"
    // to everything but itself; a `Generator` has no visible properties, so any
    // two of them are `==`.
    let src = "<?php $a = function () { return 1; }; $b = function () { return 1; }; $c = $a;
               var_dump($a == $a, $a == $b, $a == $c);";
    assert_eq!(run(src), "bool(true)\nbool(false)\nbool(true)\n");
    let src = "<?php function p() { yield 1; } function q() { yield 2; }
               var_dump(p() == q(), p() === q());";
    assert_eq!(run(src), "bool(true)\nbool(false)\n");
}

#[test]
fn a_type_error_names_the_engine_class_not_the_word_object() {
    let src = r#"<?php $c = fn() => 1;
        try { strlen($c); } catch (\TypeError $e) { echo $e->getMessage(); }"#;
    assert_eq!(
        run(src),
        "strlen(): Argument #1 ($string) must be of type string, Closure given"
    );
}

#[test]
fn class_implements_reaches_engine_ancestry_and_class_uses_reports_traits() {
    assert_eq!(
        run(r#"<?php echo implode(",", array_keys(class_implements("Generator")));"#),
        "Iterator,Traversable"
    );
    assert_eq!(
        run(
            r#"<?php trait T1 {} trait T2 {} class C { use T1, T2; } echo implode(",", array_keys(class_uses("C")));"#
        ),
        "T1,T2"
    );
}

#[test]
fn an_enum_case_is_a_singleton_and_refuses_to_be_cloned() {
    let src = r#"<?php enum E { case A; }
        try { clone E::A; } catch (\Error $e) { echo $e->getMessage(); }"#;
    assert_eq!(run(src), "Trying to clone an uncloneable object of class E");
}

#[test]
fn isset_refuses_an_operand_that_is_not_a_variable() {
    // A COMPILE-time fatal, so the operand never runs: nothing `f()` echoes
    // reaches the output.
    let err = eval_capture(r#"<?php function f() { echo "F"; return 1; } var_dump(isset(f()));"#)
        .unwrap_or_else(|e| e.to_string());
    assert!(
        err.contains("Cannot use isset() on the result of an expression"),
        "got {err:?}"
    );
    assert!(!err.contains('F'), "the operand must not have run: {err:?}");
    // A dereference of a call is still a variable form and stays legal.
    assert_eq!(
        run(r#"<?php function f() { return [7]; } var_dump(isset(f()[0]), isset(f()[9]));"#),
        "bool(true)\nbool(false)\n"
    );
}

#[test]
fn an_array_or_object_offset_is_a_type_error() {
    let src = r#"<?php $a = [1];
        try { $a[[1]]; } catch (\TypeError $e) { echo $e->getMessage(); }"#;
    assert_eq!(run(src), "Cannot access offset of type array on array");
    // `isset()`/`empty()` word the same rejection differently.
    let src = r#"<?php $a = [1];
        try { isset($a[new stdClass]); } catch (\TypeError $e) { echo $e->getMessage(); }"#;
    assert_eq!(
        run(src),
        "Cannot access offset of type stdClass in isset or empty"
    );
    let src = r#"<?php $a = [1];
        try { empty($a[[1]]); } catch (\TypeError $e) { echo $e->getMessage(); }"#;
    assert_eq!(
        run(src),
        "Cannot access offset of type array in isset or empty"
    );
}

#[test]
fn a_receiver_is_rejected_before_the_arguments_run() {
    let src = r#"<?php function f() { echo "F"; return 1; } $n = null;
        try { $n->m(f()); } catch (\Error $e) { echo $e->getMessage(); }"#;
    // No "F": the reference decides the receiver cannot take a call first.
    assert_eq!(run(src), "Call to a member function m() on null");
    let src = r#"<?php function f() { echo "F"; return 1; } $n = [1];
        try { $n->m(f()); } catch (\Error $e) { echo $e->getMessage(); }"#;
    assert_eq!(run(src), "Call to a member function m() on array");
    // A call that is going to succeed is unaffected.
    let src = r#"<?php class C { function m($x) { return $x * 2; } }
        echo (new C())->m(21);"#;
    assert_eq!(run(src), "42");
}

#[test]
fn a_variable_read_only_through_a_double_colon_survives_a_detached_chunk() {
    // A `try` body is a detached chunk. The promote analysis never walked the
    // left of a `::`, so a variable reached only that way was promoted into a
    // frame slot the chunk cannot see and read back as null.
    let src = r#"<?php class C { const K = 5; static $s = 7; static function m() { return 9; } }
        $o = new C();
        try { echo $o::class, $o::K, $o::$s, $o::m(); } catch (\Throwable $e) { echo "E"; }"#;
    assert_eq!(run(src), "C579");
}
