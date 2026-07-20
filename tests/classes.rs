//! End-to-end tests for core PHP OOP: class declaration, `new`, property
//! access, instance/static methods, class constants, `$this`, single
//! inheritance, `self::`/`parent::` scope resolution, constructor property
//! promotion, `::class`, and array-valued property mutation.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn construct_binds_constructor_arg_to_property() {
    let src = r#"<?php
        class C {
            public $p;
            function __construct($x) { $this->p = $x; }
        }
        $o = new C(5);
        echo $o->p;"#;
    assert_eq!(run(src), "5");
}

#[test]
fn instance_method_reads_this() {
    let src = r#"<?php
        class C {
            public $p;
            function __construct($x) { $this->p = $x; }
            function doubled() { return $this->p * 2; }
        }
        $o = new C(5);
        echo $o->doubled();"#;
    assert_eq!(run(src), "10");
}

#[test]
fn property_default_value() {
    let src = r#"<?php
        class C { public $n = 42; }
        $o = new C();
        echo $o->n;"#;
    assert_eq!(run(src), "42");
}

#[test]
fn property_assignment_and_readback() {
    let src = r#"<?php
        class Box { public $v; }
        $b = new Box();
        $b->v = "hello";
        echo $b->v;"#;
    assert_eq!(run(src), "hello");
}

#[test]
fn class_constant_read() {
    let src = r#"<?php
        class M { const K = 99; }
        echo M::K;"#;
    assert_eq!(run(src), "99");
}

#[test]
fn static_method_reads_self_const() {
    let src = r#"<?php
        class M {
            const V = 7;
            static function s() { return self::V; }
        }
        echo M::s();"#;
    assert_eq!(run(src), "7");
}

#[test]
fn single_inheritance_calls_parent_method() {
    let src = r#"<?php
        class A {
            const K = 10;
            function who() { return "A"; }
        }
        class B extends A {
            function who() { return "B+" . parent::who(); }
        }
        $b = new B();
        echo B::K, ":", $b->who(), ":", A::K;"#;
    assert_eq!(run(src), "10:B+A:10");
}

#[test]
fn inherited_property_default_from_parent() {
    let src = r#"<?php
        class A { public $base = 1; }
        class B extends A { public $extra = 2; }
        $b = new B();
        echo $b->base + $b->extra;"#;
    assert_eq!(run(src), "3");
}

#[test]
fn constructor_property_promotion() {
    // `public int $x` also assigns $this->x; a promoted param may have a default.
    let src = r#"<?php
        class Pt {
            function __construct(public int $x, public int $y = 9) {}
        }
        $p = new Pt(3);
        echo $p->x, ",", $p->y;"#;
    assert_eq!(run(src), "3,9");
}

#[test]
fn class_keyword_yields_class_name_string() {
    let src = r#"<?php
        class Foo {}
        echo Foo::class;"#;
    assert_eq!(run(src), "Foo");
}

#[test]
fn self_class_keyword_inside_method() {
    // `self::class` resolves to the declared class name, preserving its original
    // case (matches PHP: "Widget").
    let src = r#"<?php
        class Widget {
            function name() { return self::class; }
        }
        $w = new Widget();
        echo $w->name();"#;
    assert_eq!(run(src), "Widget");
}

#[test]
fn array_valued_property_append() {
    // `$this->items[] = x` mutates the array held in the property (the P0 case).
    let src = r#"<?php
        class Bag {
            public $items = [];
            function add($x) { $this->items[] = $x; return $this; }
        }
        $b = new Bag();
        $b->add(1);
        $b->add(2);
        $b->add(3);
        echo implode(",", $b->items);"#;
    assert_eq!(run(src), "1,2,3");
}

#[test]
fn array_valued_property_keyed_set() {
    let src = r#"<?php
        class Registry {
            public $map = [];
            function set($k, $v) { $this->map[$k] = $v; }
            function get($k) { return $this->map[$k]; }
        }
        $r = new Registry();
        $r->set("a", 1);
        $r->set("b", 2);
        echo $r->get("a"), $r->get("b");"#;
    assert_eq!(run(src), "12");
}

#[test]
fn property_increment_and_compound_assign() {
    let src = r#"<?php
        class Cnt {
            public $n = 0;
            function inc() { $this->n++; return $this->n; }
            function bump($by) { $this->n += $by; return $this->n; }
        }
        $c = new Cnt();
        echo $c->inc(), $c->inc(), ":", $c->bump(10);"#;
    assert_eq!(run(src), "12:12");
}

#[test]
fn objects_are_reference_handles() {
    // Passing an object shares the same instance; mutating through one handle is
    // visible through the other.
    let src = r#"<?php
        class Ref { public $v = 1; }
        function bump($o) { $o->v = 100; }
        $a = new Ref();
        $b = $a;
        bump($a);
        echo $b->v;"#;
    assert_eq!(run(src), "100");
}

#[test]
fn fluent_method_chaining() {
    let src = r#"<?php
        class Builder {
            public $parts = [];
            function add($p) { $this->parts[] = $p; return $this; }
            function build() { return implode("-", $this->parts); }
        }
        $b = new Builder();
        echo $b->add("a")->add("b")->add("c")->build();"#;
    assert_eq!(run(src), "a-b-c");
}
