//! Static class properties (`self::$x`, `C::$x`) and function static locals
//! (`static $n = 0;`). Static-property storage is shared per declaring class
//! across every subclass and instance; a static local's value survives across
//! calls. Expected strings match the reference `php` CLI.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn static_property_accumulates_via_self() {
    assert_eq!(
        run(
            r#"<?php class C { public static $n = 0; static function inc() { self::$n++; } }
                C::inc(); C::inc(); C::inc(); echo C::$n;"#
        ),
        "3"
    );
}

#[test]
fn static_property_external_read_and_write() {
    assert_eq!(
        run(r#"<?php class C { public static $n = 0; } C::$n = 42; echo C::$n;"#),
        "42"
    );
}

#[test]
fn static_property_compound_assign() {
    assert_eq!(
        run(r#"<?php class C { public static $n = 10; } C::$n += 5; C::$n *= 2; echo C::$n;"#),
        "30"
    );
}

#[test]
fn static_property_pre_and_post_incdec() {
    assert_eq!(
        run(
            r#"<?php class C { public static $n = 5; } echo C::$n++; echo " "; echo C::$n; echo " "; echo ++C::$n;"#
        ),
        "5 6 7"
    );
}

#[test]
fn static_property_shared_with_subclass() {
    // A subclass that does not redeclare the static shares the parent's cell.
    assert_eq!(
        run(
            r#"<?php class C { public static $n = 0; static function inc() { self::$n++; } }
                class D extends C {}
                C::inc(); D::inc(); echo C::$n . " " . D::$n;"#
        ),
        "2 2"
    );
}

#[test]
fn static_property_initialized_from_const() {
    assert_eq!(
        run(r#"<?php class E { const BASE = 10; public static $d = self::BASE; } echo E::$d;"#),
        "10"
    );
}

#[test]
fn static_property_array_default() {
    assert_eq!(
        run(r#"<?php class E { public static $arr = [1, 2, 3]; } echo count(E::$arr);"#),
        "3"
    );
}

#[test]
fn static_property_not_copied_into_instance() {
    // A static property is class-level: it must not appear among an instance's
    // own properties.
    assert_eq!(
        run(r#"<?php class C { public static $s = 1; public $i = 2; }
                $o = new C(); echo count(get_object_vars($o));"#),
        "1"
    );
}

#[test]
fn static_local_counts_across_calls() {
    assert_eq!(
        run(r#"<?php function f() { static $c = 0; $c++; return $c; } echo f() . f() . f();"#),
        "123"
    );
}

#[test]
fn static_local_default_runs_once() {
    // The initializer is applied only on first entry; later calls keep the value.
    assert_eq!(
        run(
            r#"<?php function f() { static $x = 100; $x -= 1; return $x; } echo f() . " " . f() . " " . f();"#
        ),
        "99 98 97"
    );
}

#[test]
fn static_local_multiple_declarations() {
    assert_eq!(
        run(
            r#"<?php function f() { static $a = 1, $b = 2; $a++; $b += 10; return "$a-$b"; }
                echo f() . " " . f();"#
        ),
        "2-12 3-22"
    );
}

#[test]
fn static_local_independent_per_function() {
    // Each function's `static $c` is a distinct slot.
    assert_eq!(
        run(r#"<?php function a() { static $c = 0; return ++$c; }
                function b() { static $c = 0; return ++$c; }
                echo a() . a() . b();"#),
        "121"
    );
}

#[test]
fn static_local_uninitialized_defaults_null() {
    assert_eq!(
        run(r#"<?php function f() { static $x; $x = ($x ?? 0) + 1; return $x; } echo f() . f();"#),
        "12"
    );
}
