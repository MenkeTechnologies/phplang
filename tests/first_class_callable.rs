//! End-to-end tests for PHP 8.1 first-class callable syntax `f(...)`: a function
//! name, an instance method, a static method, a user function, and a closure-held
//! value each produce a `Closure` that forwards its arguments. Byte-verified
//! against the reference `php`.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn fcc_builtin_function() {
    let src = r#"<?php
        $f = strlen(...);
        echo $f("hello");"#;
    assert_eq!(run(src), "5");
}

#[test]
fn fcc_is_callable() {
    let src = r#"<?php
        $f = strlen(...);
        var_dump(is_callable($f));"#;
    assert_eq!(run(src), "bool(true)\n");
}

#[test]
fn fcc_passed_to_array_map() {
    let src = r#"<?php
        $r = array_map(strtoupper(...), ["a", "b", "c"]);
        echo implode(",", $r);"#;
    assert_eq!(run(src), "A,B,C");
}

#[test]
fn fcc_user_function() {
    let src = r#"<?php
        function sub($a, $b) { return $a - $b; }
        $g = sub(...);
        echo $g(10, 3);"#;
    assert_eq!(run(src), "7");
}

#[test]
fn fcc_instance_method_binds_receiver() {
    let src = r#"<?php
        class Calc {
            public $base;
            function __construct($b) { $this->base = $b; }
            function add($x) { return $this->base + $x; }
        }
        $c = new Calc(10);
        $adder = $c->add(...);
        echo $adder(5);"#;
    assert_eq!(run(src), "15");
}

#[test]
fn fcc_static_method() {
    let src = r#"<?php
        class Calc {
            static function mul($a, $b) { return $a * $b; }
        }
        $m = Calc::mul(...);
        echo $m(3, 4);"#;
    assert_eq!(run(src), "12");
}

#[test]
fn fcc_nested_in_array_map_with_sum() {
    let src = r#"<?php
        echo array_sum(array_map(strlen(...), ["ab", "cde", "f"]));"#;
    assert_eq!(run(src), "6");
}

#[test]
fn fcc_from_closure_value() {
    let src = r#"<?php
        $orig = fn($n) => $n * 2;
        $g = $orig(...);
        echo $g(21);"#;
    assert_eq!(run(src), "42");
}
