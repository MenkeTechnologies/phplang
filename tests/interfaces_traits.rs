//! Interfaces (`implements`, interface `extends`), the `instanceof` operator,
//! and traits (`use Trait;` member merging).

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn implements_and_instanceof() {
    let src = r#"<?php
        interface Shape { public function area(); }
        interface Named { public function name(); }
        class Circle implements Shape, Named {
            public $r;
            function __construct($r) { $this->r = $r; }
            function area() { return $this->r * $this->r; }
            function name() { return "circle"; }
        }
        $c = new Circle(3);
        echo $c instanceof Shape ? "Y" : "N";
        echo $c instanceof Named ? "Y" : "N";
        echo $c instanceof Circle ? "Y" : "N";
        echo $c instanceof Exception ? "Y" : "N";
        echo "|", $c->name(), $c->area();"#;
    assert_eq!(run(src), "YYYN|circle9");
}

#[test]
fn interface_inheritance() {
    let src = r#"<?php
        interface A {}
        interface B extends A {}
        class C implements B {}
        $c = new C;
        echo $c instanceof A ? "Y" : "N", $c instanceof B ? "Y" : "N", $c instanceof C ? "Y" : "N";"#;
    assert_eq!(run(src), "YYY");
}

#[test]
fn instanceof_on_non_object_is_false() {
    let src = r#"<?php $x = 5; $s = "str";
        echo $x instanceof Exception ? "Y" : "N", $s instanceof Exception ? "Y" : "N";"#;
    assert_eq!(run(src), "NN");
}

#[test]
fn traits_merge_methods() {
    let src = r#"<?php
        trait Greet { public function hello() { return "hi from " . $this->who(); } }
        trait Who { public function who() { return get_class($this); } }
        class Person { use Greet, Who; }
        $p = new Person;
        echo $p->hello();"#;
    assert_eq!(run(src), "hi from Person");
}

#[test]
fn trait_with_properties() {
    let src = r#"<?php
        trait Counter { public $count = 0; public function inc() { $this->count = $this->count + 1; } }
        class Widget { use Counter; }
        $w = new Widget;
        $w->inc(); $w->inc(); $w->inc();
        echo $w->count;"#;
    assert_eq!(run(src), "3");
}

#[test]
fn class_own_method_overrides_trait() {
    let src = r#"<?php
        trait T { public function greet() { return "trait"; } }
        class C { use T; public function greet() { return "class"; } }
        echo (new C)->greet();"#;
    assert_eq!(run(src), "class");
}

#[test]
fn catch_by_interface_via_hierarchy() {
    // A user exception implementing a marker interface is catchable by its base.
    let src = r#"<?php
        class AppError extends RuntimeException {}
        try { throw new AppError("boom"); }
        catch (Exception $e) { echo $e instanceof RuntimeException ? "Y" : "N", $e->getMessage(); }"#;
    assert_eq!(run(src), "Yboom");
}
