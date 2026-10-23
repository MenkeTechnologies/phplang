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

#[test]
fn group_use_declarations_parse_in_every_shape() {
    // `use A\{B, C};`, mixed `function`/`const` members, aliases and a trailing
    // comma are all accepted; the imports themselves stay discarded.
    let src = r#"<?php
        use A\{B, C};
        use function A\{f, g};
        use A\B\{const X, function y, Z as W,};
        echo "ok";"#;
    assert_eq!(run(src), "ok");
}

#[test]
fn a_namespace_relative_name_resolves_like_the_bare_name() {
    let src = r#"<?php
        namespace A;
        function f() { return 5; }
        class K { const V = 7; }
        echo namespace\f(), namespace\K::V;"#;
    assert_eq!(run(src), "57");
}

// ── namespace resolution ─────────────────────────────────────────────────────
//
// Names resolve the way `zend_compile.c` resolves them: a declaration belongs to
// the enclosing namespace, a class name is the current namespace or an import
// (never a global fallback), and an unqualified function or constant tries the
// namespace first and the global table second, at run time. Every expected
// string below is `php -r` output from php 8.5.11.

/// The text of a compile-time `Fatal error` or `Parse error`, as `eval_capture`
/// reports one.
fn refused(src: &str) -> String {
    eval_capture(src).expect_err("the source must be refused")
}

#[test]
fn declarations_belong_to_the_enclosing_namespace() {
    let src = r#"<?php namespace A\B;
        class P { function m() { return __METHOD__ . "|" . __CLASS__; } }
        function f() { return __FUNCTION__; }
        const K = 3;
        echo get_class(new P), " ", (new P)->m(), " ", f(), " ", \A\B\f(), " ", K, \A\B\K, " ", P::class;"#;
    assert_eq!(run(src), r"A\B\P A\B\P::m|A\B\P A\B\f A\B\f 33 A\B\P");
}

#[test]
fn a_declaration_is_not_visible_unqualified_from_another_namespace() {
    let src = r#"<?php namespace A; class P {} function f() { return 1; }
        namespace B; echo get_class(new \A\P), \A\f(), " ";
        try { new P; } catch (\Error $e) { echo $e->getMessage(); }"#;
    assert_eq!(run(src), r#"A\P1 Class "B\P" not found"#);
}

#[test]
fn a_qualified_name_is_relative_to_the_current_namespace() {
    // `A\f` written inside `A` is `A\A\f`, which does not exist.
    let src = r#"<?php namespace A; function f() { return 7; }
        try { echo A\f(); } catch (\Error $e) { echo $e->getMessage(); }"#;
    assert_eq!(run(src), r"Call to undefined function A\A\f()");
    let src = r#"<?php namespace A\B; class P { const Z = 2; static function s() { return "s"; } }
        namespace C; echo \A\B\P::Z, \A\B\P::s(), get_class(new \A\B\P);"#;
    assert_eq!(run(src), r"2sA\B\P");
}

#[test]
fn use_imports_bind_classes_functions_and_constants() {
    let src = r#"<?php
        namespace Lib { class Box {} function helper() { return "h"; } const V = "v"; }
        namespace App {
            use Lib\Box; use Lib as L; use function Lib\helper; use const Lib\V;
            use function Lib\{helper as h2}; use Lib\{Box as B2};
            echo get_class(new Box), get_class(new L\Box), get_class(new box), get_class(new B2),
                helper(), HELPER(), h2(), V, L\V;
        }"#;
    assert_eq!(run(src), r"Lib\BoxLib\BoxLib\BoxLib\Boxhhhvv");
}

#[test]
fn a_constant_import_is_case_sensitive() {
    let src = r#"<?php namespace Lib { const V = "v"; }
        namespace App { use const Lib\V; try { echo v; } catch (\Error $e) { echo $e->getMessage(); } }"#;
    assert_eq!(run(src), r#"Undefined constant "App\v""#);
}

#[test]
fn an_unqualified_function_falls_back_to_the_global_one() {
    // A namespaced `strlen` shadows the library one; anything the namespace
    // lacks is the global function.
    let src = r#"<?php namespace A; function strlen($s) { return -1; }
        echo strlen("abc"), \strlen("abc"), " ", str_repeat("x", 2), " ", count([1, 2]);"#;
    assert_eq!(run(src), "-13 xx 2");
    // Neither exists: the namespaced name is the one reported.
    let src = r#"<?php namespace A; try { nope(); } catch (\Error $e) { echo $e->getMessage(); }"#;
    assert_eq!(run(src), r"Call to undefined function A\nope()");
}

#[test]
fn an_unqualified_constant_falls_back_to_the_global_one() {
    let src = r#"<?php namespace A; const PHP_EOL = "eol";
        echo PHP_EOL, "|", E_ALL === \E_ALL ? "global" : "ns", "|";
        try { echo NOPE; } catch (\Error $e) { echo $e->getMessage(); }"#;
    assert_eq!(run(src), r#"eol|global|Undefined constant "A\NOPE""#);
}

#[test]
fn a_class_name_never_falls_back_to_the_global_one() {
    let src = r#"<?php namespace A; try { new Exception("x"); } catch (\Error $e) { echo $e->getMessage(); }"#;
    assert_eq!(run(src), r#"Class "A\Exception" not found"#);
    // `catch (Exception)` inside `A` names `A\Exception`, so it does not catch.
    let src = r#"<?php namespace A; try { try { throw new \RuntimeException("x"); }
        catch (Exception $e) { echo "wrong"; } } catch (\RuntimeException $e) { echo "outer"; }"#;
    assert_eq!(run(src), "outer");
}

#[test]
fn a_type_declaration_names_the_resolved_class() {
    let src = r#"<?php namespace A; class P {} function f(P $p, int $i = 0): string { return get_class($p); }
        echo f(new P), "|"; try { f(new \stdClass); } catch (\TypeError $e) { echo $e->getMessage(); }"#;
    assert_eq!(
        run(src),
        r"A\P|A\f(): Argument #1 ($p) must be of type A\P, stdClass given, called in Command line code on line 2"
    );
}

#[test]
fn conflicting_imports_are_compile_errors() {
    for (src, want) in [
        (
            r"<?php namespace A; use B\C; use D\C;",
            r"Cannot use D\C as C because the name is already in use",
        ),
        (
            r"<?php namespace A; class C {} use B\C;",
            r"Cannot use B\C as C because the name is already in use",
        ),
        (
            r"<?php namespace A; use B\C; class C {}",
            r"Cannot redeclare class A\C (previously declared as local import)",
        ),
        (
            r"<?php namespace A; use function B\f; use function C\F;",
            r"Cannot use function C\F as F because the name is already in use",
        ),
        (
            r"<?php namespace A; use const B\K; use const C\K;",
            r"Cannot use const C\K as K because the name is already in use",
        ),
        (
            r"<?php namespace A; use B\C as self;",
            r"Cannot use B\C as self because 'self' is a special class name",
        ),
    ] {
        let e = refused(src);
        assert!(
            e.contains(&format!("{want} in Command line code on line 1")),
            "{src:?} gave {e:?}"
        );
    }
    // The three tables are separate: one alias per kind is no conflict.
    assert_eq!(
        run(r#"<?php namespace A; use B\C; use const D\C; use function E\C; echo "ok";"#),
        "ok"
    );
}

#[test]
fn a_reserved_word_is_not_an_import_alias() {
    assert_eq!(
        refused(r"<?php namespace A; use B\C as STATIC;"),
        r#"syntax error, unexpected token "static", expecting identifier in Command line code on line 1"#
    );
}

#[test]
fn a_name_held_in_a_string_may_carry_the_global_prefix() {
    let src = r#"<?php namespace A; class P { const K = 1; static function s() { return "s"; } }
        interface I {} class Q extends P implements I {} function f() { return "f"; }
        $c = '\A\P';
        var_dump(class_exists($c), interface_exists('\A\I'), get_class(new $c), $c::K, $c::s(),
            constant('\A\P::K'), is_a(new Q, '\A\P'), is_subclass_of('\A\Q', '\A\P'),
            method_exists($c, 's'), get_parent_class('\A\Q'), function_exists('\A\f'),
            call_user_func('\A\f'), call_user_func('\A\P::s'), call_user_func([$c, 's']));"#;
    assert_eq!(
        run(src),
        "bool(true)\nbool(true)\nstring(3) \"A\\P\"\nint(1)\nstring(1) \"s\"\nint(1)\nbool(true)\n\
         bool(true)\nbool(true)\nstring(3) \"A\\P\"\nbool(true)\nstring(1) \"f\"\nstring(1) \"s\"\n\
         string(1) \"s\"\n"
    );
    // An unknown class keeps its `\` in the message.
    let src = r#"<?php $c = '\Nope'; try { $c::m(); } catch (Error $e) { echo $e->getMessage(), "|"; }
        try { array_map(['\Nope', 'm'], [1]); } catch (Error $e) { echo $e->getMessage(); }"#;
    assert_eq!(
        run(src),
        r#"Class "\Nope" not found|array_map(): Argument #1 ($callback) must be a valid callback or null, class "\Nope" not found"#
    );
}

#[test]
fn a_constant_namespace_is_case_insensitive() {
    // The namespace part of a constant's name folds; the constant's own does
    // not. The redefinition warning names the registered (folded) key.
    let src = r#"<?php namespace A; const K = 1; define('X\Y', 2);
        echo \a\K, constant('a\K'), \x\Y, defined('A\k') ? "y" : "n"; define('x\Y', 3);"#;
    assert_eq!(
        run(src),
        "112n\nWarning: Constant x\\Y already defined, this will be an error in PHP 9 in Command line code on line 2\n"
    );
}

#[test]
fn an_undefined_function_is_named_as_the_call_spelled_it() {
    assert_eq!(
        run(r#"<?php try { NoPe(); } catch (Error $e) { echo $e->getMessage(); }"#),
        "Call to undefined function NoPe()"
    );
}

#[test]
fn a_global_namespace_block_has_an_empty_name() {
    assert_eq!(
        run(r#"<?php namespace A { } namespace { echo var_export(__NAMESPACE__, true); }"#),
        "''"
    );
}

#[test]
fn an_interpolated_expression_resolves_in_the_namespace() {
    assert_eq!(
        run(r#"<?php namespace A; const K = 1; $a = [1 => "one"]; echo "{$a[K]}";"#),
        "one"
    );
}

#[test]
fn a_first_class_callable_resolves_like_a_call() {
    assert_eq!(
        run(
            r#"<?php namespace A; $f = strlen(...); function g() { return "g"; } $h = g(...); echo $f("abcd"), $h();"#
        ),
        "4g"
    );
}

#[test]
fn an_attribute_name_resolves_as_a_class_name() {
    // Unqualified inside a namespace, `AllowDynamicProperties` is
    // `A\AllowDynamicProperties` — a different, inert attribute.
    assert_eq!(
        run(r#"<?php namespace A; #[AllowDynamicProperties] class C {} $c = new C; $c->x = 1; echo $c->x;"#),
        "\nDeprecated: Creation of dynamic property A\\C::$x is deprecated in Command line code on line 1\n1"
    );
    // An import reaches the engine's own attribute.
    assert_eq!(
        run(r#"<?php namespace A; use Deprecated as D; #[D] function f() {} f(); echo "|";"#),
        "\nDeprecated: Function A\\f() is deprecated in Command line code on line 1\n|"
    );
}

#[test]
fn a_conditionally_declared_function_is_resolved_when_the_call_runs() {
    let src = r#"<?php namespace A;
        if (false) { function strlen($s) { return 9; } }
        if (true) { function count($a) { return 9; } }
        function g() { function str_repeat($s, $n) { return "!"; } }
        echo strlen("ab"), count([1]), str_repeat("x", 2); g(); echo str_repeat("x", 2);"#;
    assert_eq!(run(src), "29xx!");
}
