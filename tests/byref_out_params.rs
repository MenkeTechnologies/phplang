//! Standard-library functions with a by-reference OUT parameter: `preg_match`'s
//! `$matches`, `parse_str`'s result array, `similar_text`'s percentage and
//! `str_replace`'s count. Each publishes its value at the parameter's position
//! and the call site stores it into the caller's variable — the same path a user
//! function's `&$x` parameter takes — so the variable need not exist beforehand.
//!
//! Every expected value here was taken from php 8.5.9 running the same program.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn preg_match_defines_the_matches_variable() {
    let src = r#"<?php
        if (preg_match('/(\d+)-(\d+)/', 'ab 12-34 cd', $m)) {
            echo $m[1], "/", $m[2], "/", count($m);
        }"#;
    assert_eq!(run(src), "12/34/3");
}

#[test]
fn a_failed_match_resets_the_matches_variable() {
    let src = r#"<?php $m = ['stale'];
        echo preg_match('/zzz/', 'abc', $m), count($m);"#;
    assert_eq!(run(src), "00");
}

#[test]
fn preg_match_all_collects_every_match() {
    let src = r#"<?php preg_match_all('/\d/', 'a1b2c3', $all);
        echo count($all), count($all[0]), $all[0][2];"#;
    assert_eq!(run(src), "133");
}

#[test]
fn parse_str_writes_its_second_parameter_and_returns_null() {
    let src = r#"<?php $r = parse_str('a=1&b[]=2&b[]=3', $out);
        echo var_export($r, true), "|", json_encode($out);"#;
    assert_eq!(run(src), r#"NULL|{"a":"1","b":["2","3"]}"#);
}

#[test]
fn parse_str_replaces_what_the_variable_held() {
    let src = r#"<?php $pre = ['stale' => 1]; parse_str('x=9', $pre);
        echo json_encode($pre);"#;
    assert_eq!(run(src), r#"{"x":"9"}"#);
}

#[test]
fn similar_text_reports_its_percentage() {
    let src = r#"<?php $n = similar_text("World", "word", $pct);
        echo $n, "|", round($pct, 2);"#;
    assert_eq!(run(src), "3|66.67");
}

#[test]
fn str_replace_reports_its_replacement_count() {
    let src = r#"<?php $r = str_replace("a", "b", "banana", $cnt); echo $r, "|", $cnt;"#;
    assert_eq!(run(src), "bbnbnb|3");
}

#[test]
fn a_call_that_writes_no_out_value_does_not_see_the_last_one() {
    // The OUT slots are cleared per call, so `$b` cannot pick up `$a`'s captures.
    let src = r#"<?php preg_match('/(x)/', 'x', $a);
        strlen("hi");
        echo count($a), "|", preg_match('/(y)/', 'y', $b), count($b);"#;
    assert_eq!(run(src), "2|12");
}

// ---------------------------------------------------------------------------
// A USER function's `&$x` parameter refuses an argument that is not a location,
// the same way a library out-parameter does. The three groups are the
// reference's: a location binds silently, a call's temporary binds with a
// notice, and anything else — a literal, a constant, an operator's result — is
// a catchable `Error` raised before the call happens.
// ---------------------------------------------------------------------------

/// Run `src`, reporting a refusal as `Class: message`.
fn caught(src: &str) -> String {
    let wrapped = format!(
        "<?php try {{ {src} }} catch (Throwable $e) {{ echo get_class($e), \": \", $e->getMessage(); }}"
    );
    eval_capture(&wrapped).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn a_literal_in_a_user_by_reference_parameter_is_refused() {
    assert_eq!(
        caught("function f(&$a) { $a = 9; } f(1);"),
        "Error: f(): Argument #1 ($a) could not be passed by reference"
    );
    // A constant and an operator's result are the same group as the literal.
    assert_eq!(
        caught("function f(&$a) {} class C { const K = 1; } f(C::K);"),
        "Error: f(): Argument #1 ($a) could not be passed by reference"
    );
    assert_eq!(
        caught(r#"function f(&$a) {} f("a" . "b");"#),
        "Error: f(): Argument #1 ($a) could not be passed by reference"
    );
}

#[test]
fn the_refusal_names_the_function_as_declared() {
    // The message carries the DECLARED spelling, not the call's.
    assert_eq!(
        caught("function Foo(&$a) {} FOO(1);"),
        "Error: Foo(): Argument #1 ($a) could not be passed by reference"
    );
}

#[test]
fn a_named_argument_reaches_the_by_reference_parameter_it_spells() {
    // Bound by NAME, so the refusal is still argument #1 — and it precedes the
    // unknown-name error for a name that binds nowhere.
    assert_eq!(
        caught("function f(&$a) {} f(a: 9);"),
        "Error: f(): Argument #1 ($a) could not be passed by reference"
    );
    assert_eq!(
        caught("function f(&$a) {} f(9, c: 7);"),
        "Error: f(): Argument #1 ($a) could not be passed by reference"
    );
}

#[test]
fn a_variadic_by_reference_parameter_names_no_parameter() {
    // There is no `$name` to print for a variadic position, and the reference
    // leaves the parentheses out rather than writing `($)`.
    assert_eq!(
        caught("function v(&...$a) {} v(1, 2);"),
        "Error: v(): Argument #1 could not be passed by reference"
    );
    assert_eq!(
        caught("function v(&...$a) {} $x = 1; v($x, 2);"),
        "Error: v(): Argument #2 could not be passed by reference"
    );
}

#[test]
fn a_location_binds_to_a_user_by_reference_parameter_silently() {
    // Every one of these is writable, so none of them is a diagnostic — the
    // check must not fire on an array element, a property, a static property or
    // an undefined variable.
    let src = r#"<?php
        function f(&$a) { $a = 9; }
        class C { public $p = 1; public static $s = 1; }
        $arr = [1]; $o = new C;
        f($arr[0]); f($o->p); f(C::$s); f($fresh);
        echo $arr[0], $o->p, C::$s, $fresh;"#;
    assert_eq!(run(src), "9999");
}

#[test]
fn a_calls_temporary_binds_with_a_notice() {
    let src = r#"<?php
        function f(&$a) { $a = 9; return "ok"; }
        function g() { return 1; }
        echo f(g());"#;
    assert_eq!(
        run(src),
        "\nNotice: Only variables should be passed by reference in Command line code on line 4\nok"
    );
}

#[test]
fn an_unknown_name_earlier_in_the_call_is_reported_first() {
    // Arguments are sent in WRITTEN order, so the name that binds nowhere fails
    // before the by-reference parameter is judged at all.
    assert_eq!(
        caught("function f(&$a) {} f(b: 2, a: 1);"),
        "Error: Unknown named parameter $b"
    );
    // The other order is the by-reference refusal, because `$a` is sent first.
    assert_eq!(
        caught("function f(&$a) {} f(a: 1, b: 2);"),
        "Error: f(): Argument #1 ($a) could not be passed by reference"
    );
    // A name that DOES bind does not block it.
    assert_eq!(
        caught("function f(&$a, $z = 0) {} f(z: 2, a: 1);"),
        "Error: f(): Argument #1 ($a) could not be passed by reference"
    );
}

#[test]
fn a_named_argument_in_a_variadic_by_reference_tail_is_refused_as_the_first() {
    // The reference numbers the whole named group as argument #1, whatever
    // precedes it and whichever of the names is the offender.
    assert_eq!(
        caught("function v(&...$a) {} v(x: 1);"),
        "Error: v(): Argument #1 could not be passed by reference"
    );
    assert_eq!(
        caught("function v(&...$a) {} $q = 1; v($q, $q, x: 1);"),
        "Error: v(): Argument #1 could not be passed by reference"
    );
    // A location in that tail binds silently, by value — the call runs.
    assert_eq!(
        run(r#"<?php function v(&...$a) { return "v"; } $q = 1; echo v(x: $q);"#),
        "v"
    );
}
