//! Namespaces (flat model): `namespace X;`, block `namespace X { }`, and
//! `use A\B\C [as D];` imports are accepted; qualified names fold to their last
//! segment so short names resolve.

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
