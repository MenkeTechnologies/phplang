//! Function parameter features: default values, variadic collection, and
//! call-site argument unpacking (`...$arr`). Each case runs the full
//! compile → lower → run-on-fusevm pipeline and checks captured `echo` output.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn default_used_when_arg_omitted() {
    let src = r#"<?php
        function greet($name, $greeting = "Hello") {
            return "$greeting, $name";
        }
        echo greet("Sam");"#;
    assert_eq!(run(src), "Hello, Sam");
}

#[test]
fn default_overridden_when_arg_passed() {
    let src = r#"<?php
        function greet($name, $greeting = "Hello") {
            return "$greeting, $name";
        }
        echo greet("Sam", "Hi");"#;
    assert_eq!(run(src), "Hi, Sam");
}

#[test]
fn multiple_defaults_fill_left_to_right() {
    // Passing only the first arguments leaves the rest at their defaults.
    let src = r#"<?php
        function f($a, $b = 2, $c = 3, $d = 4) {
            return $a + $b + $c + $d;
        }
        echo f(10) . "," . f(10, 20) . "," . f(10, 20, 30);"#;
    assert_eq!(run(src), "19,37,64");
}

#[test]
fn default_expr_is_a_computed_constant() {
    // The default is a constant expression (not a bare literal): it must be
    // compiled and evaluated. `4 * 5 + 2` is a valid PHP constant expression.
    let src = r#"<?php
        function box($w, $area = 4 * 5 + 2) {
            return $area;
        }
        echo box(1);"#;
    assert_eq!(run(src), "22");
}

#[test]
fn default_array_value() {
    // An array default is a valid PHP constant expression.
    let src = r#"<?php
        function g($a = [1, 2, 3]) {
            return array_sum($a);
        }
        echo g();"#;
    assert_eq!(run(src), "6");
}

#[test]
fn default_arbitrary_expr_is_a_scaffold_extension() {
    // SCAFFOLD DEVIATION: phplang compiles defaults as arbitrary expressions and
    // evaluates them in the callee frame, so a function-call default works here.
    // Real PHP rejects this at compile time ("Constant expression contains invalid
    // operations"). This pins phplang's more-permissive behavior, not PHP parity.
    let src = r#"<?php
        function f($x, $len = strlen("abc")) {
            return $x + $len;
        }
        echo f(10);"#;
    assert_eq!(run(src), "13");
}

#[test]
fn variadic_collects_trailing_args() {
    let src = r#"<?php
        function sum(...$nums) {
            $t = 0;
            foreach ($nums as $n) { $t += $n; }
            return $t;
        }
        echo sum(1, 2, 3, 4);"#;
    assert_eq!(run(src), "10");
}

#[test]
fn variadic_after_fixed_params() {
    // The fixed parameter binds the first argument; the rest land in $rest.
    let src = r#"<?php
        function tag($label, ...$rest) {
            $t = 0;
            foreach ($rest as $n) { $t += $n; }
            return "$label:$t:" . count($rest);
        }
        echo tag("x", 5, 6, 7);"#;
    assert_eq!(run(src), "x:18:3");
}

#[test]
fn variadic_collects_zero_args() {
    let src = r#"<?php
        function sum($base, ...$nums) {
            return $base + count($nums);
        }
        echo sum(100);"#;
    assert_eq!(run(src), "100");
}

#[test]
fn spread_expands_array_at_call_site() {
    let src = r#"<?php
        function add($a, $b, $c) {
            return $a + $b + $c;
        }
        $args = [1, 2, 3];
        echo add(...$args);"#;
    assert_eq!(run(src), "6");
}

#[test]
fn spread_mixed_with_positional_args() {
    // A leading positional argument followed by a spread of the rest.
    let src = r#"<?php
        function add($a, $b, $c, $d) {
            return $a . $b . $c . $d;
        }
        $tail = [2, 3, 4];
        echo add(1, ...$tail);"#;
    assert_eq!(run(src), "1234");
}

#[test]
fn spread_into_variadic() {
    // Unpacking an array straight into a variadic parameter round-trips.
    let src = r#"<?php
        function join_all($sep, ...$parts) {
            return implode($sep, $parts);
        }
        $words = ["a", "b", "c"];
        echo join_all("-", ...$words);"#;
    assert_eq!(run(src), "a-b-c");
}

#[test]
fn spread_of_non_array_raises_a_type_error() {
    // Unpacking something that is neither an array nor a Traversable is a
    // catchable `TypeError` in an argument list, message included. It used to
    // contribute no arguments and answer 0 — a silent wrong count.
    let src = r#"<?php
        function cnt(...$xs) {
            return count($xs);
        }
        $n = 5;
        try {
            echo cnt(...$n);
        } catch (TypeError $e) {
            echo get_class($e), ": ", $e->getMessage();
        }"#;
    assert_eq!(
        run(src),
        "TypeError: Only arrays and Traversables can be unpacked, int given"
    );
}

#[test]
fn spread_drives_a_generator_into_a_variadic() {
    // A Generator unpacks by being driven, so its values arrive as arguments
    // rather than as nothing.
    let src = r#"<?php
        function total(...$xs) {
            return array_sum($xs);
        }
        function gen() {
            yield 1;
            yield 2;
            yield 3;
        }
        echo total(...gen());"#;
    assert_eq!(run(src), "6");
}

// ── `...` unpacking away from a literal function name ───────────────────────
//
// Unpacking used to be lowered only for `f(...$a)` where `f` is a literal name.
// Every other call site was the compile-time refusal `'...' argument unpacking
// is only valid in a function call`, so none of these programs ran at all.

#[test]
fn unpacking_at_a_closure_call() {
    let src = r#"<?php
        $f = function ($a, $b) { return "$a/$b"; };
        $g = fn (...$xs) => implode("-", $xs);
        echo $f(...[1, 2]), "|", $g(...[1, 2, 3]);"#;
    assert_eq!(run(src), "1/2|1-2-3");
}

#[test]
fn unpacking_at_a_method_and_static_call() {
    let src = r#"<?php
        class C {
            public function m($a, $b) { return "m:$a$b"; }
            public static function s($a, $b) { return "s:$a$b"; }
        }
        $c = new C();
        echo $c->m(...[1, 2]), "|", C::s(...[3, 4]), "|", $c?->m(...[5, 6]);"#;
    assert_eq!(run(src), "m:12|s:34|m:56");
}

#[test]
fn unpacking_at_a_constructor() {
    let src = r#"<?php
        class C { public function __construct(public $a, public $b) {} }
        $o = new C(...[1, 2]);
        $n = new C(...["b" => 4, "a" => 3]);
        echo $o->a, $o->b, "|", $n->a, $n->b;"#;
    assert_eq!(run(src), "12|34");
}

#[test]
fn a_string_keyed_spread_binds_by_name_at_every_site() {
    // The keys are named arguments, so ORDER in the array does not matter and a
    // key that matches no parameter is an Error.
    let src = r#"<?php
        class C { public function m($a, $b = "d") { return "$a/$b"; } }
        $c = new C();
        echo $c->m(...["b" => 2, "a" => 1]), "|", $c->m(1, ...["b" => 9]), "|";
        try { echo $c->m(...["zz" => 1]); }
        catch (Error $e) { echo $e->getMessage(); }"#;
    assert_eq!(run(src), "1/2|1/9|Unknown named parameter $zz");
}

#[test]
fn a_non_unpackable_operand_is_a_type_error_at_every_site() {
    let src = r#"<?php
        class C { public function m($a) { return $a; } }
        $c = new C();
        try { $c->m(..."str"); }
        catch (TypeError $e) { echo $e->getMessage(); }"#;
    assert_eq!(
        run(src),
        "Only arrays and Traversables can be unpacked, string given"
    );
}

#[test]
fn a_generator_unpacks_at_a_method_call() {
    let src = r#"<?php
        class C { public function m($a, $b) { return $a + $b; } }
        echo (new C())->m(...(function () { yield 5; yield 6; })());"#;
    assert_eq!(run(src), "11");
}
