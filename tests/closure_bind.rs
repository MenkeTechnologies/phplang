//! End-to-end tests for closure rebinding: `Closure::bind($fn, $obj, $scope)`,
//! `$fn->bindTo($obj, $scope)`, and `$fn->call($obj, ...)`. These rebind `$this`
//! and the private-access scope of a closure. Byte-verified against reference
//! PHP 8.5. Source in, captured `echo` output out.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn closure_bind_static_rebinds_this_and_scope() {
    let src = r#"<?php
        class Box { private $v; public function __construct($v) { $this->v = $v; } }
        $read = function() { return $this->v; };
        echo Closure::bind($read, new Box(7), Box::class)();"#;
    assert_eq!(run(src), "7");
}

#[test]
fn bind_to_returns_a_new_closure() {
    let src = r#"<?php
        class Box { private $v; public function __construct($v) { $this->v = $v; } }
        $read = function() { return $this->v; };
        $b = $read->bindTo(new Box(20), Box::class);
        echo $b();"#;
    assert_eq!(run(src), "20");
}

#[test]
fn call_binds_and_invokes_in_one_shot() {
    // `call` scopes to the bound object's class, so it may reach private members.
    let src = r#"<?php
        class Box { private $v; public function __construct($v) { $this->v = $v; } }
        $read = function() { return $this->v; };
        echo $read->call(new Box(33));"#;
    assert_eq!(run(src), "33");
}

#[test]
fn an_arrow_function_can_be_rebound() {
    // An arrow fn captures free variables by value and still rebinds `$this`.
    let src = r#"<?php
        class Box { private $v; public function __construct($v) { $this->v = $v; } }
        $add = fn($n) => $this->v + $n;
        echo Closure::bind($add, new Box(100), Box::class)(5);"#;
    assert_eq!(run(src), "105");
}

#[test]
fn rebinding_targets_a_different_object_each_time() {
    let src = r#"<?php
        class Box { private $v; public function __construct($v) { $this->v = $v; } }
        $read = function() { return $this->v; };
        $f = $read->bindTo(new Box(1), Box::class);
        $g = $f->bindTo(new Box(2), Box::class);
        echo $f(), $g();"#;
    assert_eq!(run(src), "12");
}

#[test]
fn a_bound_closure_can_mutate_the_object() {
    let src = r#"<?php
        class Box { private $v; public function __construct($v) { $this->v = $v; } }
        $inc = function() { return ++$this->v; };
        $bi = $inc->bindTo(new Box(41), Box::class);
        echo $bi(), $bi();"#;
    assert_eq!(run(src), "4243");
}

#[test]
fn a_method_closure_keeps_its_class_scope() {
    // A closure created inside a method automatically binds `$this` and the class
    // scope, so it can read private members without an explicit bind.
    let src = r#"<?php
        class Vault {
            private $secret = 'xyz';
            public function opener() { return function() { return $this->secret; }; }
        }
        $op = (new Vault())->opener();
        echo $op();"#;
    assert_eq!(run(src), "xyz");
}

#[test]
fn a_bound_closure_can_call_a_private_method() {
    let src = r#"<?php
        class Svc {
            private $n = 5;
            private function twice() { return $this->n * 2; }
        }
        $call = function() { return $this->twice(); };
        echo $call->bindTo(new Svc(), Svc::class)();"#;
    assert_eq!(run(src), "10");
}
