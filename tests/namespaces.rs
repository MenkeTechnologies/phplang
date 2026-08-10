//! Namespaces (flat model): `namespace X;`, block `namespace X { }`, and
//! `use A\B\C [as D];` imports are accepted; qualified CLASS names fold to their
//! last segment so short names resolve.
//!
//! Constants are the exception and do not fold: `Foo\BAR` names the constant
//! `Foo\BAR`, which is what `define('Foo\BAR', …)` creates and a different
//! constant from `BAR`. Only a leading global-namespace `\` is dropped.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn namespace_declaration_and_class() {
    let src = r#"<?php
        namespace App;
        class Foo { function bar() { return 42; } }
        echo (new Foo)->bar();"#;
    assert_eq!(run(src), "42");
}

#[test]
fn namespace_with_use_imports() {
    let src = r#"<?php
        namespace App\Models;
        use App\Lib\Helper;
        use Some\Other\Thing as T;
        class User { public $name = "Ada"; }
        $u = new User;
        echo $u->name;"#;
    assert_eq!(run(src), "Ada");
}

#[test]
fn block_namespace() {
    let src = r#"<?php
        namespace X {
            function greet() { return "hello"; }
            echo greet();
        }"#;
    assert_eq!(run(src), "hello");
}

#[test]
fn use_function_and_const_forms_parse() {
    let src = r#"<?php
        use function Foo\bar;
        use const Foo\BAZ;
        echo "ok";"#;
    assert_eq!(run(src), "ok");
}

#[test]
fn qualified_name_folds_to_short_name() {
    // A fully-qualified reference resolves to the (flat) short class name.
    let src = r#"<?php
        class Widget { public $id = 7; }
        $w = new \Widget;
        echo $w->id;"#;
    assert_eq!(run(src), "7");
}

// ── qualified CONSTANT references ────────────────────────────────────────────
//
// Constants are the one qualified name that does NOT fold. A `\`-qualified
// constant keeps its separators, because `define('Foo\BAR', …)` creates a
// constant literally called `Foo\BAR` — a different one from `BAR`. Only the
// leading global-namespace `\` is dropped. (Calls still fold; see below.)

#[test]
fn qualified_constant_keeps_its_full_name() {
    assert_eq!(run(r#"<?php define('Foo\BAR', 5); echo Foo\BAR;"#), "5");
    assert_eq!(run(r#"<?php define('A\B\C', 9); echo A\B\C;"#), "9");
}

#[test]
fn leading_backslash_is_stripped_from_a_constant() {
    assert_eq!(run(r#"<?php define('NS_C', 7); echo \NS_C;"#), "7");
    assert_eq!(run(r#"<?php define('Foo\BAR', 5); echo \Foo\BAR;"#), "5");
}

#[test]
fn a_qualified_constant_is_not_its_last_segment() {
    // The case a last-segment fold would get wrong while passing every test
    // above: both constants exist and hold different values.
    assert_eq!(
        run(r#"<?php define('BAR', 1); define('Foo\BAR', 2); echo BAR, Foo\BAR;"#),
        "12"
    );
}

#[test]
fn qualified_constant_agrees_with_defined_and_constant() {
    assert_eq!(
        run(
            r#"<?php define('A\B\C', 9); var_dump(defined('A\B\C'), constant('A\B\C'), defined('C'));"#
        ),
        "bool(true)\nint(9)\nbool(false)\n"
    );
}

#[test]
fn undefined_qualified_constant_throws_naming_the_full_name() {
    // PHP 8 throws where PHP 7 fell back to the bare name as a string, and the
    // qualified reference must reach that same throw — carrying the WHOLE name.
    assert_eq!(
        run(r#"<?php try { echo Foo\NOPE; } catch (Error $e) { echo $e->getMessage(); }"#),
        r#"Undefined constant "Foo\NOPE""#
    );
    // With only the global prefix the reported name is bare.
    assert_eq!(
        run(r#"<?php try { echo \NOPE; } catch (Error $e) { echo $e->getMessage(); }"#),
        r#"Undefined constant "NOPE""#
    );
}

#[test]
fn a_bare_leading_backslash_call_is_unaffected() {
    // Only INNER separators are new here; a lone global-namespace prefix
    // consumes no segment, so `\name(…)` still calls `name`.
    assert_eq!(run(r#"<?php echo \strlen('abc');"#), "3");
}

#[test]
fn a_qualified_call_is_still_rejected() {
    // Deliberately NOT resolved. The reference resolves a qualified name
    // relative to the current namespace — `namespace A; A\f()` is `A\A\f()`
    // upstream, and fatals — which this flat model does not track. Folding to
    // the last segment would answer 7 where the reference fatals, turning an
    // error into a silently wrong value, so the call form is left alone.
    //
    // `is_err()` used to be the whole assertion here, and it passed for the
    // WRONG REASON: the reference reaches a runtime `Error: Call to undefined
    // function Foo\strlen()`, while this engine does not parse the call form at
    // all and stops at a Parse error. Both are failures, so the old predicate
    // could not tell the recorded divergence from parity — nor would it have
    // noticed if the parse error moved to a different token or line.
    //
    //   $ php -r 'echo Foo\strlen("abc");'
    //   PHP Fatal error:  Uncaught Error: Call to undefined function Foo\strlen()
    for src in [
        r#"<?php echo Foo\strlen('abc');"#,
        r#"<?php namespace A; function f() { return 7; } echo A\f();"#,
    ] {
        let e = eval_capture(src).unwrap_err();
        assert_eq!(
            e, "syntax error, unexpected token \"(\" in Command line code on line 1",
            "expected the recorded parse-error divergence for {src:?}"
        );
        // The one thing that must never happen is the call silently answering.
        assert!(
            !e.contains('7'),
            "the call resolved instead of failing: {e}"
        );
    }
}
