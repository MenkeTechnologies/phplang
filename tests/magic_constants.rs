//! PHP's magic constants: `__LINE__`, `__FILE__`, `__DIR__`, `__FUNCTION__`,
//! `__CLASS__`, `__METHOD__`, `__NAMESPACE__` and `__TRAIT__`.
//!
//! They are resolved where they are WRITTEN, not where the call arrives, which is
//! the property most of this file exists to pin: `__CLASS__` in an inherited
//! method names the class that declared it, `__CLASS__` in a trait method names
//! the class that used the trait, and a closure's `__FUNCTION__` is built out of
//! the scope it was written in rather than the one it is called from. An engine
//! that answered any of these from the running frame would pass a naive test and
//! fail every one below.
//!
//! Every expectation is the verbatim stdout of the same program under the
//! reference `php` 8.5.9 invoked as `php -r`, which names the script
//! `Command line code` and puts it all on line 1 — the entry point
//! [`phplang::eval_capture`] reproduces.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

// ── file scope ───────────────────────────────────────────────────────────────

#[test]
fn every_name_constant_is_empty_at_file_scope() {
    // Empty STRINGS, not null and not an undefined-constant `Error`: `var_export`
    // is what distinguishes the three.
    assert_eq!(
        run(
            "<?php echo var_export(__CLASS__, true), var_export(__FUNCTION__, true), \
             var_export(__METHOD__, true), var_export(__TRAIT__, true), \
             var_export(__NAMESPACE__, true);"
        ),
        // Five empty strings, each `var_export`ed as a pair of quotes.
        "''''''''''"
    );
}

#[test]
fn line_is_the_line_the_constant_is_written_on() {
    // Three reads on three lines, so a constant folded once and reused would show.
    assert_eq!(
        run("<?php\necho __LINE__, \"|\";\n\necho __LINE__, \"|\";\necho __LINE__;"),
        "2|4|5"
    );
    // And the line is the WRITE site, not the call site: a function reporting
    // `__LINE__` reports where the constant stands inside it.
    assert_eq!(
        run("<?php\nfunction f() {\n  return __LINE__;\n}\necho f(), \"|\", __LINE__;"),
        "3|5"
    );
}

#[test]
fn file_is_the_script_name_the_entry_point_gave_it() {
    // `php -r` code has no file, and the reference calls it `Command line code` —
    // the same name every diagnostic quotes, so the two cannot drift apart.
    assert_eq!(run("<?php echo __FILE__;"), "Command line code");
    assert_eq!(
        run("<?php echo $undef; echo \"|\", __FILE__;"),
        "\nWarning: Undefined variable $undef in Command line code on line 1\n|Command line code"
    );
}

#[test]
fn dir_is_the_working_directory_when_the_script_has_no_file() {
    // `php -r 'var_dump(__DIR__ === getcwd());'` prints `bool(true)`: with no file
    // to take a directory from, the reference answers the working directory.
    assert_eq!(run("<?php var_dump(__DIR__ === getcwd());"), "bool(true)\n");
}

// ── functions ────────────────────────────────────────────────────────────────

#[test]
fn a_free_functions_method_constant_is_just_its_name() {
    // `__METHOD__` does NOT gain a `::` half outside a class; it repeats
    // `__FUNCTION__`. An engine that always wrote `Class::name` would print a
    // stray `::` here.
    assert_eq!(
        run(
            "<?php function g() { echo __FUNCTION__, \"|\", __METHOD__, \"|\", \
             var_export(__CLASS__, true); } g();"
        ),
        "g|g|''"
    );
}

#[test]
fn a_function_declared_inside_a_method_belongs_to_no_class() {
    // It lands in the global function table, so its `__CLASS__` is empty even
    // though it was WRITTEN inside a class body — the one place the lexical rule
    // and the "innermost enclosing braces" rule disagree.
    assert_eq!(
        run("<?php class C { public function m() { function g() { \
             return var_export(__CLASS__, true) . \"|\" . __METHOD__; } return g(); } } \
             echo (new C)->m();"),
        "''|g"
    );
}

// ── classes ──────────────────────────────────────────────────────────────────

#[test]
fn class_and_method_name_the_declaring_class_not_the_called_one() {
    // The whole point of compile-time resolution: `D` inherits `m` and calling it
    // through `D` still reports `C`. An engine reading the running frame's class
    // would print `D` here and pass every other test in this file.
    assert_eq!(
        run(
            "<?php class C { public function m() { echo __CLASS__, \"|\", __METHOD__; } } \
             class D extends C {} (new D)->m();"
        ),
        "C|C::m"
    );
    // …while a method D declares itself does report D, so the assertion above is
    // not simply "always name the parent".
    assert_eq!(
        run(
            "<?php class C {} class D extends C { public function d() { echo __CLASS__; } } \
             (new D)->d();"
        ),
        "D"
    );
}

#[test]
fn a_static_method_and_a_class_constant_resolve_the_same_way() {
    assert_eq!(
        run(
            "<?php class C { const K = __CLASS__; public $p = __CLASS__; \
             public static function s() { echo __METHOD__; } } \
             C::s(); echo \"|\", C::K, \"|\", (new C)->p;"
        ),
        "C::s|C|C"
    );
}

// ── traits ───────────────────────────────────────────────────────────────────

#[test]
fn a_trait_method_reports_the_using_class_but_its_own_name() {
    // Three different answers from one declaration, and they disagree on purpose:
    // `__CLASS__` is the class that used the trait (a run-time fact — the same
    // trait can be used by many), `__TRAIT__` and `__METHOD__` are the trait.
    let src = "<?php trait T { public function tm() { \
               echo __TRAIT__, \"|\", __CLASS__, \"|\", __FUNCTION__, \"|\", __METHOD__; } } \
               class C { use T; } class D { use T; }";
    assert_eq!(run(&format!("{src} (new C)->tm();")), "T|C|tm|T::tm");
    // The same trait in a second class reports that second class, which is what
    // rules out resolving `__CLASS__` to the trait or to the first user of it.
    assert_eq!(run(&format!("{src} (new D)->tm();")), "T|D|tm|T::tm");
}

#[test]
fn trait_is_empty_outside_a_trait() {
    assert_eq!(
        run(
            "<?php class C { public function m() { echo var_export(__TRAIT__, true); } } \
             (new C)->m();"
        ),
        "''"
    );
}

// ── closures ─────────────────────────────────────────────────────────────────

#[test]
fn a_closure_is_named_after_the_scope_it_was_written_in() {
    // PHP 8.4 gives closures the name `{closure:<scope>:<line>}`, and the scope is
    // the enclosing declaration — the FILE at file scope, `f()` inside a function,
    // `C::m()` inside a method.
    assert_eq!(
        run("<?php $c = function() { return __FUNCTION__; }; echo $c();"),
        "{closure:Command line code:1}"
    );
    assert_eq!(
        run(
            "<?php function outer() { return (function() { return __FUNCTION__; })(); } \
             echo outer();"
        ),
        "{closure:outer():1}"
    );
    assert_eq!(
        run("<?php class K { public static function sm() { \
             return (fn() => __FUNCTION__ . \"|\" . __CLASS__)(); } } echo K::sm();"),
        "{closure:K::sm():1}|K"
    );
}

#[test]
fn closure_names_nest() {
    // The inner closure's scope is the outer closure's NAME, so the two compose
    // rather than the inner one restarting from the enclosing function.
    assert_eq!(
        run("<?php function nested() { return (function() { \
             return (function() { return __FUNCTION__; })(); })(); } echo nested();"),
        "{closure:{closure:nested():1}:1}"
    );
}

#[test]
fn a_closure_in_a_method_keeps_the_class_but_not_the_method_name() {
    // `__CLASS__` carries into the closure; `__METHOD__` does not — it becomes the
    // closure's own name, which is built FROM the method's.
    assert_eq!(
        run("<?php class C { public function w() { \
             return (function() { return __CLASS__ . \"|\" . __METHOD__; })(); } } \
             echo (new C)->w();"),
        "C|{closure:C::w():1}"
    );
}

// ── anonymous classes ────────────────────────────────────────────────────────

#[test]
fn an_anonymous_class_reports_the_generated_name_get_class_reports() {
    // The name is minted at compile time and includes the script and line, so the
    // assertion is that `__CLASS__` and `get_class` agree rather than what the
    // name literally is — and `__METHOD__` is built from the same name.
    assert_eq!(
        run("<?php $a = new class { public function q() { \
             return [__CLASS__, __METHOD__]; } }; \
             [$c, $m] = $a->q(); \
             var_dump($c === get_class($a), $m === get_class($a) . \"::q\");"),
        "bool(true)\nbool(true)\n"
    );
}

// ── namespaces ───────────────────────────────────────────────────────────────

#[test]
fn namespace_is_the_declared_namespace_and_empty_without_one() {
    // The FULL dotted name, not the last segment a class reference folds to.
    assert_eq!(run("<?php namespace A\\B; echo __NAMESPACE__;"), "A\\B");
    assert_eq!(run("<?php echo var_export(__NAMESPACE__, true);"), "''");
}

// ── spelling ─────────────────────────────────────────────────────────────────

#[test]
fn the_names_are_matched_case_insensitively() {
    // PHP's scanner folds them like any keyword, so `__line__` is `__LINE__` and
    // not an undefined constant.
    assert_eq!(
        run("<?php echo __line__, \"|\", __Class__ === \"\" ? \"y\" : \"n\";"),
        "1|y"
    );
}
