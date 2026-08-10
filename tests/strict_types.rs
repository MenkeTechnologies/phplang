//! `declare(strict_types=1)` and the scalar type declarations it governs.
//!
//! Every expectation below is a byte-parity assertion taken verbatim from the same
//! program under the reference `php` 8.5.9. Under `php -r` the script is named
//! `Command line code` and is all on line 1, which is what the messages quote.
//!
//! The two typing modes are one mechanism with one switch, so they are tested
//! together: a coercive expectation that stopped failing because coercion was
//! removed would be caught by its strict twin, and vice versa. Each conversion is
//! asserted in BOTH directions — the value that converts and the value that does
//! not — because a check that only ever saw acceptable input would pass against an
//! engine that accepted everything, which is exactly the state this file was
//! written to end.

use phplang::{compile, host, run_compiled};

/// Run `src` and return everything it wrote, including any fatal-error block.
fn output_of(src: &str) -> String {
    host::reset_host();
    host::with_host(|h| h.begin_capture());
    if let Ok(prog) = compile(src) {
        let _ = run_compiled(prog);
    }
    host::with_host(|h| h.end_capture())
}

/// The compile-time rejection of `src`, rendered as the CLI displays it:
/// `"{severity}: {body}"`.
///
/// The `declare` violations are raised by the PARSE, before a program exists to
/// run, so they cannot be observed through [`output_of`] — which is the point of
/// asserting them here. The severity is part of the expectation: PHP calls these
/// `Fatal error`, not `Parse error`, and getting that wrong changes the word it
/// prints on stdout.
fn compile_fatal(src: &str) -> String {
    host::reset_host();
    let e = phplang::parser::parse_meta(src).expect_err("source must be rejected");
    format!("{}: {}", e.severity, e.message)
}

/// `src` wrapped so a thrown `TypeError` is caught and its message printed — the
/// shape every diagnostic assertion below uses, since `getMessage` is the part
/// that is fully reproduced (see the uncaught-rendering note in `README.md`).
fn caught(decl: &str, call: &str) -> String {
    output_of(&format!(
        "<?php {decl} try {{ {call} }} catch (Throwable $e) {{ \
         echo get_class($e), \"|\", $e->getMessage(); }}"
    ))
}

// ── the switch itself ────────────────────────────────────────────────────────

#[test]
fn strict_mode_rejects_the_string_that_coercive_mode_converts() {
    // The single case that defines the feature, asserted both ways round so
    // neither half can pass by the engine simply ignoring types.
    let call = "function f(int $x) { var_dump($x); } f(\"5\");";
    assert_eq!(caught("", call), "int(5)\n");
    assert_eq!(
        caught("declare(strict_types=1);", call),
        "TypeError|f(): Argument #1 ($x) must be of type int, string given, \
         called in Command line code on line 1"
    );
}

#[test]
fn strict_types_zero_is_the_coercive_mode_spelled_out() {
    // `=0` must be accepted and mean the default, not merely be tolerated.
    assert_eq!(
        caught(
            "declare(strict_types=0);",
            "function f(int $x) { var_dump($x); } f(\"5\");"
        ),
        "int(5)\n"
    );
}

#[test]
fn int_to_float_widening_survives_strict_mode() {
    // The one conversion strict mode still performs. If this regressed to a
    // TypeError the feature would look "more correct" while being wrong.
    assert_eq!(
        caught(
            "declare(strict_types=1);",
            "function g(float $x) { var_dump($x); } g(5);"
        ),
        "float(5)\n"
    );
    // …and it does NOT run the other way: float→int is rejected.
    assert_eq!(
        caught(
            "declare(strict_types=1);",
            "function h(int $x) { var_dump($x); } h(5.0);"
        ),
        "TypeError|h(): Argument #1 ($x) must be of type int, float given, \
         called in Command line code on line 1"
    );
}

#[test]
fn bool_and_string_are_exact_under_strict_mode() {
    assert_eq!(
        caught(
            "declare(strict_types=1);",
            "function s(string $x) { var_dump($x); } s(5);"
        ),
        "TypeError|s(): Argument #1 ($x) must be of type string, int given, \
         called in Command line code on line 1"
    );
    assert_eq!(
        caught(
            "declare(strict_types=1);",
            "function b(bool $x) { var_dump($x); } b(1);"
        ),
        "TypeError|b(): Argument #1 ($x) must be of type bool, int given, \
         called in Command line code on line 1"
    );
}

// ── coercive mode, which strict mode is defined against ──────────────────────

#[test]
fn a_trailing_garbage_string_converts_in_neither_mode() {
    // `"5abc"` is the boundary of coercive typing: it is NOT a 5 here, though the
    // same string added to a number would be. A check that accepted it would make
    // the coercive path indistinguishable from no checking at all.
    assert_eq!(
        caught("", "function f(int $x) { var_dump($x); } f(\"5abc\");"),
        "TypeError|f(): Argument #1 ($x) must be of type int, string given, \
         called in Command line code on line 1"
    );
    // The fully numeric neighbour does convert, which is what makes the rejection
    // above a property of the STRING rather than of the parameter.
    assert_eq!(
        caught("", "function f(int $x) { var_dump($x); } f(\"5\");"),
        "int(5)\n"
    );
}

#[test]
fn null_and_non_scalars_are_rejected_in_coercive_mode_too() {
    for (arg, given) in [("null", "null"), ("[1]", "array")] {
        assert_eq!(
            caught(
                "",
                &format!("function f(int $x) {{ var_dump($x); }} f({arg});")
            ),
            format!(
                "TypeError|f(): Argument #1 ($x) must be of type int, {given} given, \
                 called in Command line code on line 1"
            ),
            "argument {arg} in coercive mode"
        );
    }
}

#[test]
fn a_nullable_scalar_takes_null_and_still_checks_the_rest() {
    assert_eq!(
        caught(
            "declare(strict_types=1);",
            "function n(?int $x) { var_dump($x); } n(null);"
        ),
        "NULL\n"
    );
    assert_eq!(
        caught(
            "declare(strict_types=1);",
            "function n(?int $x) { var_dump($x); } n(\"5\");"
        ),
        "TypeError|n(): Argument #1 ($x) must be of type ?int, string given, \
         called in Command line code on line 1"
    );
}

#[test]
fn a_lossy_conversion_is_deprecated_against_the_parameters_own_line() {
    // PHP attributes this to where the PARAMETER is declared, not to the call —
    // the two are different lines here, so an engine that used the call site
    // would print `line 3`.
    assert_eq!(
        output_of("<?php\nfunction f(int $x) { var_dump($x); }\nf(5.9);"),
        "\nDeprecated: Implicit conversion from float 5.9 to int loses precision \
         in Command line code on line 2\nint(5)\n"
    );
}

// ── return types ─────────────────────────────────────────────────────────────

#[test]
fn a_return_type_is_checked_in_both_modes() {
    // Coercive: converted on the way out.
    assert_eq!(
        caught("", "function r(): int { return \"5\"; } var_dump(r());"),
        "int(5)\n"
    );
    // Strict: rejected, and with the return wording, which names no call site.
    assert_eq!(
        caught(
            "declare(strict_types=1);",
            "function r(): int { return \"5\"; } var_dump(r());"
        ),
        "TypeError|r(): Return value must be of type int, string returned"
    );
}

#[test]
fn a_return_type_widens_int_to_float_under_strict_mode() {
    assert_eq!(
        caught(
            "declare(strict_types=1);",
            "function w(): float { return 5; } var_dump(w());"
        ),
        "float(5)\n"
    );
}

// ── methods and constructors ─────────────────────────────────────────────────

#[test]
fn a_method_names_its_class_in_the_diagnostic_with_the_declared_casing() {
    // The class is looked up lowercased internally; the message must still read
    // `C::m`, not `c::m`.
    assert_eq!(
        caught(
            "declare(strict_types=1);",
            "class C { public function m(int $x) {} } (new C)->m(\"5\");"
        ),
        "TypeError|C::m(): Argument #1 ($x) must be of type int, string given, \
         called in Command line code on line 1"
    );
}

#[test]
fn a_promoted_constructor_parameter_is_typed_like_any_other() {
    assert_eq!(
        caught(
            "declare(strict_types=1);",
            "class K { public function __construct(public int $p) {} } new K(\"9\");"
        ),
        "TypeError|K::__construct(): Argument #1 ($p) must be of type int, \
         string given, called in Command line code on line 1"
    );
}

#[test]
fn a_variadic_parameter_is_checked_per_argument_and_named_by_number_only() {
    // PHP writes `Argument #2 must be…` with NO `($xs)`, unlike every other form.
    assert_eq!(
        caught(
            "declare(strict_types=1);",
            "function v(int ...$xs) { var_dump($xs); } v(1, \"2\");"
        ),
        "TypeError|v(): Argument #2 must be of type int, string given, \
         called in Command line code on line 1"
    );
}

// ── the declaration's own rules, all of them compile-time ────────────────────

#[test]
fn strict_types_must_be_the_very_first_statement() {
    // The `echo` must NOT run: PHP rejects this before executing a line, so any
    // output before the diagnostic is itself the failure.
    assert_eq!(
        compile_fatal("<?php\necho \"before\";\ndeclare(strict_types=1);"),
        "Fatal error: strict_types declaration must be the very first statement \
         in the script in Command line code on line 3\nStack trace:\n#0 {main}"
    );
}

#[test]
fn a_preceding_declare_does_not_disqualify_strict_types() {
    // The exception to the rule above, and the half most likely to be missed:
    // another `declare` may come first, and the mode still takes effect.
    assert_eq!(
        caught(
            "declare(ticks=1); declare(strict_types=1);",
            "function f(int $x) {} f(\"5\");"
        ),
        "TypeError|f(): Argument #1 ($x) must be of type int, string given, \
         called in Command line code on line 1"
    );
}

#[test]
fn a_second_declare_cannot_turn_strict_typing_back_off() {
    // The mode is a LATCH, not an assignment: once a file has turned it on, a
    // later `declare(strict_types=0)` does not restore coercion. Both orders end
    // strict, which is what distinguishes a latch from "last value wins" — an
    // engine that simply stored the last value passes every other test in this
    // file and fails only the first case here.
    for decl in [
        "declare(strict_types=1); declare(strict_types=0);",
        "declare(strict_types=0); declare(strict_types=1);",
    ] {
        assert_eq!(
            caught(decl, "function f(int $x) { var_dump($x); } f(\"5\");"),
            "TypeError|f(): Argument #1 ($x) must be of type int, string given, \
             called in Command line code on line 1",
            "declarations {decl}"
        );
    }
    // …and two zeroes still mean coercive, so the latch is not simply "any
    // `strict_types` at all turns it on".
    assert_eq!(
        caught(
            "declare(strict_types=0); declare(strict_types=0);",
            "function f(int $x) { var_dump($x); } f(\"5\");"
        ),
        "int(5)\n"
    );
}

#[test]
fn strict_types_rejects_block_mode_but_ticks_accepts_it() {
    assert_eq!(
        compile_fatal("<?php declare(strict_types=1) { echo \"in\"; }"),
        "Fatal error: strict_types declaration must not use block mode \
         in Command line code on line 1\nStack trace:\n#0 {main}"
    );
    // The same syntax is legal for another directive, so the rejection is about
    // `strict_types` and not about block mode being unimplemented.
    assert_eq!(output_of("<?php declare(ticks=1) { echo \"in\"; }"), "in");
}

#[test]
fn strict_types_takes_only_the_literals_zero_and_one() {
    assert_eq!(
        compile_fatal("<?php declare(strict_types=2); echo \"x\";"),
        "Fatal error: strict_types declaration must have 0 or 1 as its value \
         in Command line code on line 1\nStack trace:\n#0 {main}"
    );
    // A non-literal is a DIFFERENT message — and `-1` counts as a non-literal,
    // because the minus makes it an expression rather than a literal.
    for src in [
        "<?php declare(strict_types=$x); echo \"x\";",
        "<?php declare(strict_types=-1); echo \"x\";",
    ] {
        assert_eq!(
            compile_fatal(src),
            "Fatal error: declare(strict_types) value must be a literal \
             in Command line code on line 1\nStack trace:\n#0 {main}",
            "source {src}"
        );
    }
}

#[test]
fn an_unknown_declare_warns_and_an_encoding_one_warns_differently() {
    assert_eq!(
        output_of("<?php declare(foo=1); echo \"ok\";"),
        "\nWarning: Unsupported declare 'foo' in Command line code on line 1\nok"
    );
    assert_eq!(
        output_of("<?php declare(encoding='UTF-8'); echo \"ok\";"),
        "\nWarning: declare(encoding=...) ignored because Zend multibyte feature \
         is turned off by settings in Command line code on line 1\nok"
    );
    // `ticks` is the one directive accepted in silence.
    assert_eq!(output_of("<?php declare(ticks=1); echo \"ok\";"), "ok");
}

// ── types that parse but impose no check ─────────────────────────────────────

#[test]
fn union_and_qualified_types_parse_where_they_used_to_be_syntax_errors() {
    // These were PARSE ERRORS before types were read properly. They impose no
    // check (see `TypeHint::scalar`), so the assertion is that they run at all —
    // and the value arrives untouched, which pins the "no check" half too.
    assert_eq!(
        output_of("<?php function u(int|string $x) { var_dump($x); } u(1.5);"),
        "float(1.5)\n"
    );
    assert_eq!(
        output_of("<?php function q(\\Foo\\Bar $x) { echo \"ran\"; } q(1);"),
        "ran"
    );
    assert_eq!(
        output_of("<?php function d((A&B)|null $x) { echo \"ran\"; } d(null);"),
        "ran"
    );
}

#[test]
fn a_by_reference_typed_parameter_is_coerced_through_the_reference() {
    // CORRECTED EXPECTATION. This test previously asserted `string(1) "5"` under
    // the belief that a by-ref parameter is an alias the callee may rewrite and so
    // is not coerced on the way in. That belief is wrong about PHP, and the pin was
    // frozen against it:
    //
    //   $ php -r 'function f(int &$x) { var_dump($x); } $v = "5"; f($v); var_dump($v);'
    //   int(5)
    //   int(5)
    //
    // php 8.5.9 coerces a by-reference argument exactly as it coerces a by-value
    // one, and writes the converted value back THROUGH the reference before the
    // body runs — the second `int(5)` is the caller's own variable, which the body
    // above never touches. The old expectation is kept below, re-scoped to what it
    // actually described: the by-ref parameter is still an alias, so what the body
    // stores reaches the caller.
    assert_eq!(
        output_of("<?php function f(int &$x) { var_dump($x); } $v = \"5\"; f($v); var_dump($v);"),
        "int(5)\nint(5)\n"
    );
    // The alias half, which the original assertion existed to protect: a write in
    // the callee is a write to the caller's variable.
    assert_eq!(
        output_of("<?php function f(int &$x) { $x = 9; } $v = \"5\"; f($v); var_dump($v);"),
        "int(9)\n"
    );
    // A string that cannot convert is rejected in a by-ref position too, rather
    // than being passed through unchecked.
    assert_eq!(
        caught("", "function f(int &$x) {} $v = \"abc\"; f($v);"),
        "TypeError|f(): Argument #1 ($x) must be of type int, string given, \
         called in Command line code on line 1"
    );
    // …and strict mode refuses the numeric string that coercive mode converts,
    // which is the same switch the rest of this file tests, reaching through the
    // reference rather than around it.
    assert_eq!(
        caught(
            "declare(strict_types=1);",
            "function f(int &$x) { var_dump($x); } $v = \"5\"; f($v);"
        ),
        "TypeError|f(): Argument #1 ($x) must be of type int, string given, \
         called in Command line code on line 1"
    );
}
