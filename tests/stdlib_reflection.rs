//! Tests for the `reflection` stdlib category: class/object introspection —
//! `class_exists`, `method_exists`, `property_exists`, `get_class`,
//! `get_parent_class`, `get_object_vars`, `get_class_methods`, `is_a`,
//! `is_subclass_of`, and the always-false `interface_exists`/`trait_exists`/
//! `enum_exists`. Expected results match PHP 8, except where documented in
//! `src/stdlib/reflection.rs` (no interfaces/traits/enums; method names are
//! reported lowercased).

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn class_exists_true_and_false() {
    assert_eq!(
        run(r#"<?php class Widget {} echo class_exists("Widget") ? "y" : "n";"#),
        "y"
    );
    // Case-insensitive class lookup.
    assert_eq!(
        run(r#"<?php class Widget {} echo class_exists("widget") ? "y" : "n";"#),
        "y"
    );
    assert_eq!(run(r#"<?php echo class_exists("Nope") ? "y" : "n";"#), "n");
}

#[test]
fn interface_trait_enum_exist_always_false() {
    assert_eq!(
        run(r#"<?php echo interface_exists("Foo") ? "y" : "n";"#),
        "n"
    );
    assert_eq!(run(r#"<?php echo trait_exists("Foo") ? "y" : "n";"#), "n");
    assert_eq!(run(r#"<?php echo enum_exists("Foo") ? "y" : "n";"#), "n");
}

#[test]
fn method_exists_on_object_and_class_name() {
    let src = r#"<?php
        class C { function go() {} }
        $o = new C();
        echo method_exists($o, "go") ? "y" : "n";
        echo method_exists("C", "go") ? "y" : "n";
        echo method_exists($o, "nope") ? "y" : "n";"#;
    assert_eq!(run(src), "yyn");
}

#[test]
fn method_exists_walks_parent_chain() {
    let src = r#"<?php
        class Base { function inherited() {} }
        class Sub extends Base {}
        echo method_exists("Sub", "inherited") ? "y" : "n";"#;
    assert_eq!(run(src), "y");
}

#[test]
fn property_exists_declared_and_dynamic() {
    let src = r#"<?php
        class C { public $declared = 1; }
        $o = new C();
        echo property_exists("C", "declared") ? "y" : "n";
        echo property_exists($o, "declared") ? "y" : "n";
        echo property_exists("C", "ghost") ? "y" : "n";
        $o->dynamic = 5;
        echo property_exists($o, "dynamic") ? "y" : "n";
        echo property_exists("C", "dynamic") ? "y" : "n";"#;
    // declared via class + object => y y; missing => n; dynamic on object => y;
    // dynamic is not on the class definition => n.
    assert_eq!(run(src), "yynyn");
}

#[test]
fn get_class_returns_original_casing() {
    let src = r#"<?php
        class MyWidget {}
        $o = new MyWidget();
        echo get_class($o);"#;
    assert_eq!(run(src), "MyWidget");
}

#[test]
fn get_class_non_object_is_false() {
    assert_eq!(
        run(r#"<?php echo get_class(5) === false ? "y" : "n";"#),
        "y"
    );
}

#[test]
fn get_parent_class_object_and_name() {
    let src = r#"<?php
        class Base {}
        class Child extends Base {}
        $c = new Child();
        echo get_parent_class($c);
        echo "|";
        echo get_parent_class("Child");"#;
    assert_eq!(run(src), "Base|Base");
}

#[test]
fn get_parent_class_no_parent_is_false() {
    let src = r#"<?php
        class Solo {}
        echo get_parent_class("Solo") === false ? "y" : "n";
        echo get_parent_class(new Solo()) === false ? "y" : "n";"#;
    assert_eq!(run(src), "yy");
}

#[test]
fn get_object_vars_assoc() {
    let src = r#"<?php
        class Point { public $x = 1; public $y = 2; }
        $p = new Point();
        $p->x = 10;
        $vars = get_object_vars($p);
        echo $vars["x"];
        echo ",";
        echo $vars["y"];
        echo ",";
        echo implode("|", array_keys($vars));"#;
    assert_eq!(run(src), "10,2,x|y");
}

#[test]
fn get_object_vars_non_object_false() {
    assert_eq!(
        run(r#"<?php echo get_object_vars(5) === false ? "y" : "n";"#),
        "y"
    );
}

#[test]
fn get_class_methods_lists_names_lowercased() {
    // Host stores method names lowercased, so results are lowercased (documented
    // deviation from PHP's declared-casing behavior).
    let src = r#"<?php
        class C { function Alpha() {} function beta() {} }
        $m = get_class_methods("C");
        sort($m);
        echo implode(",", $m);"#;
    assert_eq!(run(src), "alpha,beta");
}

#[test]
fn get_class_methods_includes_inherited() {
    let src = r#"<?php
        class Base { function base_m() {} }
        class Sub extends Base { function sub_m() {} }
        $m = get_class_methods(new Sub());
        sort($m);
        echo implode(",", $m);"#;
    assert_eq!(run(src), "base_m,sub_m");
}

#[test]
fn is_a_object_instance_and_ancestor() {
    let src = r#"<?php
        class Base {}
        class Child extends Base {}
        $c = new Child();
        echo is_a($c, "Child") ? "y" : "n";
        echo is_a($c, "Base") ? "y" : "n";
        echo is_a($c, "Other") ? "y" : "n";"#;
    assert_eq!(run(src), "yyn");
}

#[test]
fn is_a_string_requires_allow_string() {
    let src = r#"<?php
        class Base {}
        class Child extends Base {}
        echo is_a("Child", "Base") ? "y" : "n";
        echo is_a("Child", "Base", true) ? "y" : "n";"#;
    // Without allow_string a class-name string returns false; with it, true.
    assert_eq!(run(src), "ny");
}

#[test]
fn is_subclass_of_excludes_self() {
    let src = r#"<?php
        class Base {}
        class Child extends Base {}
        $c = new Child();
        echo is_subclass_of($c, "Base") ? "y" : "n";
        echo is_subclass_of($c, "Child") ? "y" : "n";"#;
    // Ancestor => true; own class => false.
    assert_eq!(run(src), "yn");
}

#[test]
fn is_subclass_of_allows_string_by_default() {
    let src = r#"<?php
        class Base {}
        class Child extends Base {}
        echo is_subclass_of("Child", "Base") ? "y" : "n";
        echo is_subclass_of("Base", "Base") ? "y" : "n";"#;
    // String subject honored by default (allow_string defaults true).
    assert_eq!(run(src), "yn");
}

// ── HARDEN: is_a with allow_string must reject a non-declared class name ──────
#[test]
fn is_a_string_nonexistent_class_is_false() {
    // A bare name equals itself, so without the class_exists guard the ancestry
    // walk would report `true` on the first step for a class that was never
    // declared. PHP returns false because the class does not exist.
    let src = r#"<?php
        echo is_a("Ghost", "Ghost", true) ? "y" : "n";
        echo is_a("Ghost", "Other", true) ? "y" : "n";"#;
    assert_eq!(run(src), "nn");
}

#[test]
fn is_subclass_of_string_nonexistent_class_is_false() {
    let src = r#"<?php
        echo is_subclass_of("Ghost", "Base") ? "y" : "n";
        echo is_subclass_of("Ghost", "Ghost") ? "y" : "n";"#;
    assert_eq!(run(src), "nn");
}

// ── EXPAND: class_parents ────────────────────────────────────────────────────
#[test]
fn class_parents_walks_chain_assoc() {
    let src = r#"<?php
        class A {}
        class B extends A {}
        class C extends B {}
        $p = class_parents("C");
        echo implode(",", array_keys($p));
        echo "|";
        echo implode(",", array_values($p));"#;
    // Nearest ancestor first; keys and values are both the class name.
    assert_eq!(run(src), "B,A|B,A");
}

#[test]
fn class_parents_on_object_and_no_parent() {
    let src = r#"<?php
        class Base {}
        class Child extends Base {}
        $p = class_parents(new Child());
        echo implode(",", array_keys($p));
        echo "|";
        // A root class has an empty parents array (not false).
        echo count(class_parents("Base"));"#;
    assert_eq!(run(src), "Base|0");
}

#[test]
fn class_parents_unknown_is_false() {
    assert_eq!(
        run(r#"<?php echo class_parents("Ghost") === false ? "y" : "n";"#),
        "y"
    );
}

// ── EXPAND: class_implements / class_uses (empty; false for unknown) ──────────
#[test]
fn class_implements_empty_for_valid_false_for_unknown() {
    let src = r#"<?php
        class C {}
        echo count(class_implements("C"));
        echo count(class_implements(new C()));
        echo class_implements("Ghost") === false ? "F" : "?";"#;
    assert_eq!(run(src), "00F");
}

#[test]
fn class_uses_empty_for_valid_false_for_unknown() {
    let src = r#"<?php
        class C {}
        echo count(class_uses("C"));
        echo class_uses("Ghost") === false ? "F" : "?";"#;
    assert_eq!(run(src), "0F");
}

// ── EXPAND: get_class_vars (false for unknown; empty for declared) ────────────
#[test]
fn get_class_vars_unknown_false_declared_empty() {
    let src = r#"<?php
        class C { public $x = 1; }
        echo get_class_vars("Ghost") === false ? "F" : "?";
        echo is_array(get_class_vars("C")) ? "A" : "?";"#;
    // Property-default values are not reachable without a host accessor
    // (documented), so a declared class yields an empty array.
    assert_eq!(run(src), "FA");
}

// ── EXPAND: get_declared_classes (best-effort array) ─────────────────────────
#[test]
fn get_declared_classes_returns_array() {
    let src = r#"<?php
        class C {}
        echo is_array(get_declared_classes()) ? "A" : "?";"#;
    assert_eq!(run(src), "A");
}
