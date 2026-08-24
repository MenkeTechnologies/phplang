//! Predefined constants, the `define`/`defined`/`constant` functions, and the
//! `const` declaration. All four reach one host constant table; a bare constant
//! reference resolves against it and raises `Error: Undefined constant "…"` when
//! the name is absent — the PHP 7 fallback to the bare name as a string was
//! removed in PHP 8 (see `undefined_constant_is_a_catchable_error`).

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn php_int_max_and_size() {
    assert_eq!(run("<?php echo PHP_INT_MAX;"), "9223372036854775807");
    assert_eq!(run("<?php echo PHP_INT_MIN;"), "-9223372036854775808");
    assert_eq!(run("<?php echo PHP_INT_SIZE;"), "8");
}

#[test]
fn php_eol_is_newline() {
    assert_eq!(run(r#"<?php echo PHP_EOL === "\n" ? "yes" : "no";"#), "yes");
}

#[test]
fn math_constants() {
    assert_eq!(run("<?php printf('%.5f', M_PI);"), "3.14159");
    assert_eq!(run("<?php printf('%.5f', M_E);"), "2.71828");
    assert_eq!(
        run("<?php echo M_SQRT2 > 1.4141 && M_SQRT2 < 1.4143 ? 'y' : 'n';"),
        "y"
    );
}

#[test]
fn flag_constants_are_integers() {
    assert_eq!(run("<?php echo SORT_STRING;"), "2");
    assert_eq!(run("<?php echo STR_PAD_LEFT;"), "0");
    assert_eq!(run("<?php echo FILTER_VALIDATE_EMAIL;"), "274");
    assert_eq!(run("<?php echo JSON_PRETTY_PRINT;"), "128");
    // 30719, not the pre-8.4 32767: PHP 8.4 removed the E_STRICT level and took
    // its bit (2048) out of E_ALL. The constant E_STRICT itself still exists.
    assert_eq!(run("<?php echo E_ALL;"), "30719");
    assert_eq!(run("<?php echo E_STRICT;"), "2048");
    assert_eq!(run("<?php echo E_ALL & E_STRICT;"), "0");
}

#[test]
fn define_defined_constant() {
    let src = r#"<?php
        define("APP_MODE", "prod");
        echo APP_MODE;
        echo defined("APP_MODE") ? "Y" : "N";
        echo defined("NOPE") ? "Y" : "N";
        echo constant("APP_MODE");"#;
    assert_eq!(run(src), "prodYNprod");
}

/// `define()` does not redefine: the second call returns false, the FIRST value
/// survives — and the attempt WARNS.
///
/// This test previously asserted `"bool(true)\nbool(false)\n1"`, i.e. the return
/// values with no warning between them. The reference has always warned here;
/// the expectation described phplang's output rather than the oracle's, so it is
/// replaced by what `php` 8.5.9 actually prints for this exact source:
///
/// ```text
/// bool(true)
///
/// Warning: Constant X already defined, this will be an error in PHP 9 in … on line 3
/// bool(false)
/// 1
/// ```
///
/// The warning is on *stdout* — under the CLI defaults the reference displays
/// diagnostics there, interleaved with the program's own output — so it is part
/// of the captured string rather than something to be filtered out of it.
#[test]
fn define_does_not_redefine() {
    let src = r#"<?php
        var_dump(define("X", 1));
        var_dump(define("X", 2));
        echo X;"#;
    assert_eq!(
        run(src),
        "bool(true)\n\nWarning: Constant X already defined, this will be an error in PHP 9 \
         in Command line code on line 3\nbool(false)\n1"
    );
}

/// An undefined constant is a catchable `Error`, not its own name as a string.
///
/// This test previously asserted the PHP 7 fallback (`echo NOT_DEFINED` echoing
/// `"NOT_DEFINED"`). That leniency was removed in PHP 8 and the reference has
/// raised `Error: Undefined constant "…"` since; the old expectation described
/// phplang's behaviour rather than the oracle's, so it is replaced here by what
/// PHP 8.5.9 actually does. Both ways of reaching a constant are covered — the
/// bareword and `constant()` — because they resolve through the same table and
/// only one of them used to throw.
#[test]
fn undefined_constant_is_a_catchable_error() {
    let bare = r#"<?php try { echo THIS_IS_NOT_DEFINED; }
        catch (Error $e) { echo get_class($e), ": ", $e->getMessage(); }"#;
    assert_eq!(
        run(bare),
        r#"Error: Undefined constant "THIS_IS_NOT_DEFINED""#
    );
    let via_fn = r#"<?php try { echo constant('THIS_IS_NOT_DEFINED'); }
        catch (Error $e) { echo get_class($e), ": ", $e->getMessage(); }"#;
    assert_eq!(
        run(via_fn),
        r#"Error: Undefined constant "THIS_IS_NOT_DEFINED""#
    );
    // A DEFINED constant still resolves through both, and `defined()` answers
    // without throwing for either name.
    let ok = r#"<?php define('IS_HERE', 5); echo IS_HERE, constant('IS_HERE'),
        (int) defined('IS_HERE'), (int) defined('THIS_IS_NOT_DEFINED');"#;
    assert_eq!(run(ok), "5510");
}

#[test]
fn constant_usable_as_array_key_and_in_expr() {
    let src = r#"<?php
        define("LIMIT", 3);
        $a = [];
        for ($i = 0; $i < LIMIT; $i++) { $a[] = $i * 2; }
        echo implode(",", $a);"#;
    assert_eq!(run(src), "0,2,4");
}

// ── the `const` declaration ──────────────────────────────────────────────────
//
// Every expectation below is byte-parity with `php` 8.5.9. `const` and
// `define()` write the same table but are different constructs: one is a
// declaration read at statement level, the other a function call.

#[test]
fn const_declares_a_global_constant() {
    assert_eq!(run("<?php const X = 1; echo X;"), "1");
    // One `const` may declare SEVERAL names, comma separated.
    assert_eq!(run("<?php const A = 3, B = 4; echo A, '|', B;"), "3|4");
    // An array constant, and a subscript of it at the point of use.
    assert_eq!(run("<?php const L = [10, 20]; echo L[1];"), "20");
}

#[test]
fn const_initializer_may_read_an_earlier_constant() {
    // The entries are written in source order, so a later one can read an
    // earlier one — within a single `const` as well as across two.
    assert_eq!(run("<?php const P = 5; const Q = P * 2; echo Q;"), "10");
    assert_eq!(run("<?php const R = 2, S = R + 1; echo S;"), "3");
    // Including a predefined constant as the initializer.
    assert_eq!(
        run("<?php const M = PHP_INT_MAX; echo M;"),
        "9223372036854775807"
    );
}

#[test]
fn const_is_not_hoisted() {
    // The constant comes into being WHERE THE STATEMENT STANDS. A `defined()`
    // earlier in the same script must answer false — which is the whole reason
    // this is a runtime statement and not a load-time table write.
    assert_eq!(
        run("<?php var_dump(defined('LATE')); const LATE = 1; var_dump(defined('LATE'));"),
        "bool(false)\nbool(true)\n"
    );
}

#[test]
fn redefining_a_constant_warns_and_keeps_the_first_value() {
    // PHP does not redefine: the FIRST value survives and the second attempt
    // warns. Verbatim from the reference, whose warning goes to stdout under
    // the CLI defaults.
    let warn = "\nWarning: Constant D already defined, this will be an error in PHP 9 \
                in Command line code on line 1\n";
    assert_eq!(
        run("<?php const D = 1; const D = 2; echo D;"),
        format!("{warn}1")
    );
    // The same warning when the two spellings meet, in EITHER order, because it
    // is raised at the one table write both of them reach.
    assert_eq!(
        run("<?php define('E', 1); const E = 2; echo E;"),
        "\nWarning: Constant E already defined, this will be an error in PHP 9 \
             in Command line code on line 1\n1"
            .to_string()
    );
    // `define()` over an existing `const` warns AND returns false.
    assert_eq!(
        run("<?php const F = 1; var_dump(define('F', 2)); echo F;"),
        "\nWarning: Constant F already defined, this will be an error in PHP 9 \
             in Command line code on line 1\nbool(false)\n1"
            .to_string()
    );
}

#[test]
fn const_is_visible_inside_a_function_body() {
    // A global constant needs no `global` declaration to be read in a function.
    assert_eq!(
        run("<?php const G = 7; function f() { return G; } echo f();"),
        "7"
    );
}

#[test]
fn const_inside_a_namespace_block_is_top_level() {
    // `namespace Name { }` is the one brace-delimited block that does not leave
    // top level, so a `const` is legal directly inside it.
    assert_eq!(run("<?php namespace N { const A = 1; echo A; }"), "1");
    // And under the statement (semicolon) form of the same declaration.
    assert_eq!(run("<?php namespace N; const B = 2; echo B;"), "2");
}
