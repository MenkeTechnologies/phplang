//! Predefined constants and the `define`/`defined`/`constant` functions. Bare
//! constant references resolve against the host constant table, falling back to
//! the bare name as a string when undefined.

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

#[test]
fn define_does_not_redefine() {
    let src = r#"<?php
        var_dump(define("X", 1));
        var_dump(define("X", 2));
        echo X;"#;
    assert_eq!(run(src), "bool(true)\nbool(false)\n1");
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
