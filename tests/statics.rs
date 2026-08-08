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

// ── late static binding ──────────────────────────────────────────────────────
//
// `static::` names the class the call was made *on*, which is why it cannot be
// resolved when the method is compiled: one inherited body sees a different
// class per subclass. Every expectation is the verbatim stdout of the same
// program under the reference `php` 8.5.9.

/// A three-deep hierarchy whose base methods all reach through `static::`.
const H: &str = r#"class A {
        public static $reg = "A";
        const TAG = "A";
        public static function name() { return static::class; }
        public static function create() { return new static(); }
        public static function viaSelf() { return self::name(); }
        public static function viaStatic() { return static::name(); }
        public function tag() { return static::TAG; }
        public function reg() { return static::$reg; }
    }
    class B extends A {
        public static $reg = "B";
        const TAG = "B";
        public static function viaParent() { return parent::name(); }
    }
    class C extends B { const TAG = "C"; public static $reg = "C"; }"#;

#[test]
fn static_class_is_the_called_class_not_the_declaring_one() {
    let src = format!(r#"<?php {H} echo A::name(), "|", B::name(), "|", C::name();"#);
    assert_eq!(run(&src), "A|B|C");
}

#[test]
fn a_forwarding_call_keeps_the_callers_late_static_class() {
    // `self::`, `parent::` and `static::` forward; naming a class does not.
    let src = format!(r#"<?php {H} echo B::viaSelf(), "|", B::viaParent(), "|", C::viaStatic();"#);
    assert_eq!(run(&src), "B|B|C");
}

#[test]
fn new_static_instantiates_the_called_class() {
    let src = format!(r#"<?php {H} echo get_class(C::create()), "|", get_class(A::create());"#);
    assert_eq!(run(&src), "C|A");
}

#[test]
fn static_reaches_constants_and_static_properties_of_the_called_class() {
    let src = format!(
        r#"<?php {H} $c = new C(); $a = new A();
        echo $c->tag(), "|", $a->tag(), "|", $c->reg(), "|", $a->reg();"#
    );
    assert_eq!(run(&src), "C|A|C|A");
}

#[test]
fn static_inside_a_constructor_is_the_instantiated_class() {
    let src = r#"<?php class P { public function __construct() { echo static::class; } }
        class Q extends P {}
        new Q(); echo "|"; new P();"#;
    assert_eq!(run(src), "Q|P");
}

#[test]
fn a_later_call_does_not_inherit_an_earlier_ones_late_static_class() {
    // The binding lives on the frame, so it cannot leak past the call.
    let src = format!(r#"<?php {H} C::name(); echo A::name();"#);
    assert_eq!(run(&src), "A");
}
