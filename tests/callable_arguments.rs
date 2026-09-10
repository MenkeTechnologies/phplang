//! End-to-end tests for the `callable` parameter check — the refusal PHP 8
//! raises when a library function is handed something that does not name
//! anything invocable — and for `Closure::fromCallable`, which builds a real
//! `Closure` over one.
//!
//! Both go through the single decision tree in `stdlib::callable`, so the reason
//! clause is identical either side of the comma; the tests pin one of each
//! shape. Every expectation is the byte output of
//! `/opt/homebrew/Cellar/php/8.5.10/bin/php -d xdebug.mode=off`.

use phplang::eval_capture;

/// Run `expr` against the fixture classes below, catching any refusal.
fn caught(expr: &str) -> String {
    let src = format!(
        "<?php\n\
         class C {{ public function m() {{}} private function p() {{}} public static function s() {{}} }}\n\
         class D {{ public function __call($n, $a) {{ return \"magic\"; }} }}\n\
         class E {{ public function __invoke($v) {{ return $v * 2; }} }}\n\
         $x = [3, 1];\n\
         try {{ $r = {expr}; echo \"OK \"; var_export($r); }}\n\
         catch (Throwable $e) {{ echo get_class($e), \": \", $e->getMessage(); }}"
    );
    eval_capture(&src).unwrap_or_else(|e| panic!("eval error for {expr:?}: {e}"))
}

#[test]
fn a_name_that_resolves_to_nothing_is_a_type_error() {
    // A nullable `callable` parameter says "or null"; a plain one does not.
    assert_eq!(
        caught(r#"array_map("nofunc", [1])"#),
        "TypeError: array_map(): Argument #1 ($callback) must be a valid callback or null, \
         function \"nofunc\" not found or invalid function name"
    );
    assert_eq!(
        caught(r#"usort($x, "nofunc")"#),
        "TypeError: usort(): Argument #2 ($callback) must be a valid callback, \
         function \"nofunc\" not found or invalid function name"
    );
}

#[test]
fn the_callable_parameter_is_judged_in_its_own_position() {
    // #1 is the callback and #2 the array: the callback is reported even though
    // the array is wrong too.
    assert_eq!(
        caught(r#"array_map("nofunc", "notarray")"#),
        "TypeError: array_map(): Argument #1 ($callback) must be a valid callback or null, \
         function \"nofunc\" not found or invalid function name"
    );
    // Reversed for `array_filter`, whose array is #1 — so the array wins.
    assert_eq!(
        caught(r#"array_filter("notarray", "nofunc")"#),
        "TypeError: array_filter(): Argument #1 ($array) must be of type array, string given"
    );
}

#[test]
fn each_shape_of_broken_array_callable_has_its_own_reason() {
    let prefix =
        "TypeError: array_map(): Argument #1 ($callback) must be a valid callback or null, ";
    assert_eq!(
        caught("array_map([\"C\"], [1])"),
        format!("{prefix}array callback must have exactly two members")
    );
    assert_eq!(
        caught("array_map([1 => \"C\", 2 => \"s\"], [1])"),
        format!("{prefix}array callback has to contain indices 0 and 1")
    );
    assert_eq!(
        caught("array_map([1, 2], [1])"),
        format!("{prefix}first array member is not a valid class name or object")
    );
    assert_eq!(
        caught("array_map([\"Nope\", \"m\"], [1])"),
        format!("{prefix}class \"Nope\" not found")
    );
    assert_eq!(
        caught("array_map([\"C\", \"nope\"], [1])"),
        format!("{prefix}class C does not have a method \"nope\"")
    );
    assert_eq!(
        caught("array_map([new C, \"p\"], [1])"),
        format!("{prefix}cannot access private method C::p()")
    );
    // `["C", "m"]` names no instance, and `m` is not static.
    assert_eq!(
        caught("array_map([\"C\", \"m\"], [1])"),
        format!("{prefix}non-static method C::m() cannot be called statically")
    );
    // Neither a string nor an array is not a callable of any kind.
    assert_eq!(
        caught("array_map(1, [1])"),
        format!("{prefix}no array or string given")
    );
    assert_eq!(
        caught("array_map(new C, [1])"),
        format!("{prefix}no array or string given")
    );
}

#[test]
fn magic_and_invoke_make_a_callable_valid() {
    // `__call` answers for a method that was never declared.
    assert_eq!(
        caught(r#"array_map([new D, "zzz"], [1])"#),
        "OK array (\n  0 => 'magic',\n)"
    );
    // An object is callable exactly when its class defines `__invoke`.
    assert_eq!(caught("array_map(new E, [2])"), "OK array (\n  0 => 4,\n)");
    // A null callback is what `array_map` zips with, so `?callable` takes it.
    assert_eq!(
        caught("array_map(null, [1], [2])"),
        "OK array (\n  0 => \n  array (\n    0 => 1,\n    1 => 2,\n  ),\n)"
    );
}

#[test]
fn from_callable_builds_a_real_closure() {
    assert_eq!(
        caught(r#"Closure::fromCallable("strtoupper") instanceof Closure"#),
        "OK true"
    );
    assert_eq!(
        caught(r#"get_class(Closure::fromCallable("strtoupper"))"#),
        "OK 'Closure'"
    );
    assert_eq!(
        caught(r#"Closure::fromCallable("strtoupper")("hi")"#),
        "OK 'HI'"
    );
    // Arguments reach the callable by name as well as by position.
    assert_eq!(
        caught(r#"Closure::fromCallable("str_repeat")(times: 3, string: "a")"#),
        "OK 'aaa'"
    );
    // A method callable keeps its instance.
    assert_eq!(
        caught(r#"Closure::fromCallable([new E, "__invoke"])(5)"#),
        "OK 10"
    );
    // A Closure is handed back rather than wrapped again.
    assert_eq!(caught("Closure::fromCallable(fn($v) => $v * 2)(4)"), "OK 8");
}

#[test]
fn from_callable_refuses_what_is_not_a_callable() {
    assert_eq!(
        caught(r#"Closure::fromCallable("nofunc")"#),
        "TypeError: Failed to create closure from callable: \
         function \"nofunc\" not found or invalid function name"
    );
    assert_eq!(
        caught("Closure::fromCallable([new C, \"nope\"])"),
        "TypeError: Failed to create closure from callable: class C does not have a method \"nope\""
    );
    assert_eq!(
        caught("Closure::fromCallable(1)"),
        "TypeError: Failed to create closure from callable: no array or string given"
    );
    assert_eq!(
        caught("Closure::fromCallable()"),
        "ArgumentCountError: Closure::fromCallable() expects exactly 1 argument, 0 given"
    );
}
