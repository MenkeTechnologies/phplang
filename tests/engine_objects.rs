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
    // `?->` short-circuits on NULL, so a `false` receiver still reaches the
    // call and must reject it before the argument runs.
    let src = r#"<?php function f() { echo "F"; return 1; } $n = false;
        try { $n?->m(f()); } catch (\Error $e) { echo $e->getMessage(); }"#;
    assert_eq!(run(src), "Call to a member function m() on false");
    // A null receiver skips the whole link, argument included.
    let src = r#"<?php function f() { echo "F"; return 1; } $n = null;
        var_dump($n?->m(f()));"#;
    assert_eq!(run(src), "NULL\n");
    // A call that is going to succeed is unaffected, in either spelling.
    let src = r#"<?php class C { function m($x) { return $x * 2; } }
        echo (new C())->m(21), "|", (new C())?->m(10);"#;
    assert_eq!(run(src), "42|20");
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

/// The reference settles the CALLEE before it evaluates a single argument, so a
/// call that cannot proceed prints nothing at all first. The method-receiver form
/// is covered above; these are the other spellings, each of which used to run its
/// argument and print `F` before raising.
#[test]
fn a_callee_is_rejected_before_the_arguments_run() {
    let case = |src: &str| {
        run(&format!(
            "<?php function f() {{ echo \"F\"; return 1; }} {src}"
        ))
    };
    let caught = |setup: &str, call: &str| {
        case(&format!(
            "{setup} try {{ {call}; }} catch (\\Error $e) {{ echo $e->getMessage(); }}"
        ))
    };
    assert_eq!(
        caught("", "undefinedfn(f())"),
        "Call to undefined function undefinedfn()"
    );
    assert_eq!(caught("", "Nope::m(f())"), "Class \"Nope\" not found");
    assert_eq!(caught("", "new Nope(f())"), "Class \"Nope\" not found");
    // The named-argument spelling of each lowers separately and must agree.
    assert_eq!(
        caught("", "undefinedfn(x: f())"),
        "Call to undefined function undefinedfn()"
    );
    assert_eq!(caught("", "new Nope(x: f())"), "Class \"Nope\" not found");
    // A value that is not callable at all, in each of the shapes the reference
    // words differently.
    assert_eq!(
        caught("$n = [1];", "$n(f())"),
        "Array callback must have exactly two elements"
    );
    assert_eq!(
        caught("$n = \"nope\";", "$n(f())"),
        "Call to undefined function nope()"
    );
    assert_eq!(
        caught("$n = 5;", "$n(f())"),
        "Value of type int is not callable"
    );
    assert_eq!(
        caught("$n = new stdClass;", "$n(f())"),
        "Object of type stdClass is not callable"
    );
}

/// The check exists only to move a diagnostic earlier, so a call that was going
/// to succeed must be untouched by it — including every form that now carries
/// one.
#[test]
fn the_callee_check_leaves_a_working_call_alone() {
    assert_eq!(
        run("<?php class C { function __construct(public $x = 0) {} \
             function m($z) { return $z + 1; } static function s($y) { return $y * 2; } } \
             echo (new C(5))->x, C::s(3), (new C)->m(1);"),
        "562"
    );
    // A library function, a user function, and a dynamic callee.
    assert_eq!(
        run(
            "<?php function u($a,$b) { return $a + $b; } $f = 'strtoupper'; \
             echo strtoupper('hi'), u(1,2), $f('x'), bcadd('1','2');"
        ),
        "HI3X3"
    );
    // A cast lowers to an internal call that no name predicate can see; it must
    // not be refused.
    assert_eq!(
        run("<?php $o = (object)['a'=>1]; $a = (array)$o; echo $o->a, $a['a'];"),
        "11"
    );
}

// ── the callee is settled before the arguments ───────────────────────────────

/// PHP decides a method call is impossible before it evaluates a single
/// argument, so an argument that prints must not print. Round 2 closed the
/// receiver half of this; these two forms need the METHOD TABLE walked first,
/// and each used to run its argument and print `A` ahead of the fatal the
/// reference reaches having printed nothing.
///
/// The screen is affordable because it asks `PhpHost::method_declared`, a
/// `contains_key` per class in the chain — the existence question used to be
/// answered by cloning the whole `FuncDef`, method body included.
#[test]
fn an_undefined_or_unreachable_method_is_refused_before_its_arguments() {
    for (src, want) in [
        (
            "class C { function m() {} } $o = new C; $o->nope(print(\"A\"));",
            "Call to undefined method C::nope()",
        ),
        (
            "class C { function m() {} } C::nope(print(\"A\"));",
            "Call to undefined method C::nope()",
        ),
        (
            "class C { private function p() {} } $o = new C; $o->p(print(\"A\"));",
            "Call to private method C::p() from global scope",
        ),
        (
            "class C { protected function p() {} } $o = new C; $o->p(print(\"A\"));",
            "Call to protected method C::p() from global scope",
        ),
        (
            "class C { private static function p() {} } C::p(print(\"A\"));",
            "Call to private method C::p() from global scope",
        ),
        (
            "$c = function ($x) { return $x; }; $c->nope(print(\"A\"));",
            "Call to undefined method Closure::nope()",
        ),
    ] {
        assert_eq!(
            run(&format!(
                "<?php try {{ {src} }} catch (Throwable $e) {{ echo $e->getMessage(); }}"
            )),
            want,
            "{src} must print nothing before the diagnostic"
        );
    }
}

/// A call the magic catch-all answers is NOT refused by that screen, and its
/// argument runs — the check may only refuse what the call would have refused.
#[test]
fn the_early_screen_lets_a_magic_call_through() {
    assert_eq!(
        run(
            "<?php class C { function __call($n, $a) { echo \"|$n:\", $a[0]; } } \
             $o = new C; $o->nope(print(\"A\"));"
        ),
        "A|nope:1"
    );
    assert_eq!(
        run(
            "<?php class C { static function __callStatic($n, $a) { echo \"|$n:\", $a[0]; } } \
             C::nope(print(\"A\"));"
        ),
        "A|nope:1"
    );
    // A private method plus a `__call` is the catch-all's business, not an
    // access error — the screen must reach the same verdict the call does.
    assert_eq!(
        run(
            "<?php class C { private function p() {} function __call($n, $a) { echo \"|$n\"; } } \
             $o = new C; $o->p(print(\"A\"));"
        ),
        "A|p"
    );
}

/// An undefined method on a `Closure` or a `Generator` is a catchable `Error`.
/// Neither has a PHP class for the method table to answer for, and both used to
/// stop the program with the uncatchable `php: call to undefined method …`.
#[test]
fn an_undefined_engine_method_is_catchable() {
    assert_eq!(
        run("<?php $c = function () {}; \
             try { $c->m(); } catch (Throwable $e) { echo get_class($e), '|', $e->getMessage(); }"),
        "Error|Call to undefined method Closure::m()"
    );
    assert_eq!(
        run("<?php function g() { yield 1; } $x = g(); \
             try { $x->m(); } catch (Throwable $e) { echo get_class($e), '|', $e->getMessage(); }"),
        "Error|Call to undefined method Generator::m()"
    );
    // The methods each of them DOES answer still dispatch.
    assert_eq!(
        run("<?php $c = function () { return 3; }; echo $c->call(new stdClass);"),
        "3"
    );
    assert_eq!(
        run("<?php function g() { yield 7; } $x = g(); echo $x->current();"),
        "7"
    );
}
