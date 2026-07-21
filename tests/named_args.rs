//! End-to-end tests for PHP 8.0 named arguments: binding by parameter name,
//! mixing with positional arguments, order independence, defaults for omitted
//! parameters, constructors, static/instance methods, and collection of extra
//! named arguments into a variadic. Byte-verified against the reference `php`.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn named_args_reordered() {
    let src = r#"<?php
        function greet($greeting, $name) { return "$greeting, $name"; }
        echo greet(name: "Bob", greeting: "Hello");"#;
    assert_eq!(run(src), "Hello, Bob");
}

#[test]
fn named_after_positional() {
    let src = r#"<?php
        function greet($greeting, $name, $punct) { return "$greeting, $name$punct"; }
        echo greet("Hi", punct: "!", name: "Sue");"#;
    assert_eq!(run(src), "Hi, Sue!");
}

#[test]
fn named_arg_skips_to_default() {
    let src = r#"<?php
        function fmt($x, $base = 10, $prefix = "0x") { return "$prefix$x/$base"; }
        echo fmt(8, prefix: "b");"#;
    assert_eq!(run(src), "b8/10");
}

#[test]
fn named_args_all_defaults_but_one() {
    let src = r#"<?php
        function box($w = 1, $h = 1, $d = 1) { return $w * $h * $d; }
        echo box(h: 5);"#;
    assert_eq!(run(src), "5");
}

#[test]
fn named_constructor_args() {
    let src = r#"<?php
        class Box {
            public $w; public $h; public $d;
            function __construct($w, $h, $d = 1) { $this->w=$w; $this->h=$h; $this->d=$d; }
            function vol() { return $this->w * $this->h * $this->d; }
        }
        $b = new Box(h: 3, w: 2);
        echo $b->vol();
        echo "|";
        $b2 = new Box(w: 4, h: 5, d: 2);
        echo $b2->vol();"#;
    assert_eq!(run(src), "6|40");
}

#[test]
fn named_method_args() {
    let src = r#"<?php
        class C {
            function span($from, $to) { return "$from-$to"; }
        }
        $c = new C();
        echo $c->span(to: 9, from: 1);"#;
    assert_eq!(run(src), "1-9");
}

#[test]
fn named_static_method_args() {
    let src = r#"<?php
        class C {
            static function pair($a, $b) { return "$a:$b"; }
        }
        echo C::pair(b: "y", a: "x");"#;
    assert_eq!(run(src), "x:y");
}

#[test]
fn named_promoted_constructor_property() {
    let src = r#"<?php
        class Pt {
            function __construct(public $x, public $y) {}
        }
        $p = new Pt(y: 7, x: 3);
        echo $p->x, ",", $p->y;"#;
    assert_eq!(run(src), "3,7");
}

#[test]
fn extra_named_args_go_to_variadic() {
    let src = r#"<?php
        function tag($name, ...$attrs) {
            $s = "<$name";
            foreach ($attrs as $k => $v) { $s .= " $k=$v"; }
            return $s.">";
        }
        echo tag("a", id: "1", class: "x");"#;
    assert_eq!(run(src), "<a id=1 class=x>");
}

#[test]
fn named_arg_on_closure() {
    let src = r#"<?php
        $f = function ($a, $b) { return $a - $b; };
        echo $f(b: 3, a: 10);"#;
    assert_eq!(run(src), "7");
}
