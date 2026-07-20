//! Tests for the `types` stdlib category: type predicates, `get_debug_type`,
//! and the `serialize`/`unserialize` round trip. PHP source in, captured `echo`
//! output out; every expected string is cross-checked against reference `php`.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn is_scalar_predicate() {
    assert_eq!(run(r#"<?php echo is_scalar(1) ? "y" : "n";"#), "y");
    assert_eq!(run(r#"<?php echo is_scalar(1.5) ? "y" : "n";"#), "y");
    assert_eq!(run(r#"<?php echo is_scalar("x") ? "y" : "n";"#), "y");
    assert_eq!(run(r#"<?php echo is_scalar(true) ? "y" : "n";"#), "y");
    // null, arrays, and objects are not scalar.
    assert_eq!(run(r#"<?php echo is_scalar(null) ? "y" : "n";"#), "n");
    assert_eq!(run(r#"<?php echo is_scalar([1,2]) ? "y" : "n";"#), "n");
}

#[test]
fn is_object_and_iterable_and_countable() {
    // Arrays are iterable and countable but not objects.
    assert_eq!(run(r#"<?php echo is_iterable([1,2]) ? "y" : "n";"#), "y");
    assert_eq!(run(r#"<?php echo is_countable([1,2]) ? "y" : "n";"#), "y");
    assert_eq!(run(r#"<?php echo is_object([1,2]) ? "y" : "n";"#), "n");
    assert_eq!(run(r#"<?php echo is_iterable(5) ? "y" : "n";"#), "n");
    // A class instance is an object, not iterable.
    let src = r#"<?php class C {} $o = new C(); echo is_object($o) ? "y" : "n"; echo is_iterable($o) ? "y" : "n";"#;
    assert_eq!(run(src), "yn");
    // A closure is an object.
    assert_eq!(run(r#"<?php $f = fn() => 1; echo is_object($f) ? "y" : "n";"#), "y");
}

#[test]
fn get_debug_type_names() {
    assert_eq!(run(r#"<?php echo get_debug_type(1);"#), "int");
    assert_eq!(run(r#"<?php echo get_debug_type(1.5);"#), "float");
    assert_eq!(run(r#"<?php echo get_debug_type("s");"#), "string");
    assert_eq!(run(r#"<?php echo get_debug_type(true);"#), "bool");
    assert_eq!(run(r#"<?php echo get_debug_type(null);"#), "null");
    assert_eq!(run(r#"<?php echo get_debug_type([1]);"#), "array");
    assert_eq!(run(r#"<?php echo get_debug_type(fn() => 1);"#), "Closure");
    // Objects report their class name (original casing).
    assert_eq!(
        run(r#"<?php class Widget {} echo get_debug_type(new Widget());"#),
        "Widget"
    );
}

#[test]
fn serialize_scalars() {
    assert_eq!(run(r#"<?php echo serialize(null);"#), "N;");
    assert_eq!(run(r#"<?php echo serialize(true);"#), "b:1;");
    assert_eq!(run(r#"<?php echo serialize(false);"#), "b:0;");
    assert_eq!(run(r#"<?php echo serialize(42);"#), "i:42;");
    assert_eq!(run(r#"<?php echo serialize(-7);"#), "i:-7;");
    assert_eq!(run(r#"<?php echo serialize(1.5);"#), "d:1.5;");
    // An integer-valued float serializes without a fractional part.
    assert_eq!(run(r#"<?php echo serialize(1.0);"#), "d:1;");
    // Byte length, not character count, and binary-safe quoting.
    assert_eq!(run(r#"<?php echo serialize("hello");"#), r#"s:5:"hello";"#);
    assert_eq!(run(r#"<?php echo serialize("");"#), r#"s:0:"";"#);
}

#[test]
fn serialize_arrays() {
    // List: integer keys 0..n.
    assert_eq!(
        run(r#"<?php echo serialize([1, 2, 3]);"#),
        "a:3:{i:0;i:1;i:1;i:2;i:2;i:3;}"
    );
    // Mixed string/int keys, preserving insertion order (matches reference php).
    assert_eq!(
        run(r#"<?php echo serialize(["a" => 1, 5 => "x", "b" => true]);"#),
        r#"a:3:{s:1:"a";i:1;i:5;s:1:"x";s:1:"b";b:1;}"#
    );
    // Nested array.
    assert_eq!(
        run(r#"<?php echo serialize([[1, 2], "k" => null]);"#),
        r#"a:2:{i:0;a:2:{i:0;i:1;i:1;i:2;}s:1:"k";N;}"#
    );
}

#[test]
fn unserialize_scalars() {
    assert_eq!(run(r#"<?php var_dump(unserialize("N;"));"#), "NULL\n");
    assert_eq!(run(r#"<?php var_dump(unserialize("b:1;"));"#), "bool(true)\n");
    assert_eq!(
        run(r#"<?php var_dump(unserialize("b:0;"));"#),
        "bool(false)\n"
    );
    assert_eq!(run(r#"<?php var_dump(unserialize("i:42;"));"#), "int(42)\n");
    assert_eq!(
        run(r#"<?php var_dump(unserialize("d:1.5;"));"#),
        "float(1.5)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(unserialize('s:5:"hello";'));"#),
        "string(5) \"hello\"\n"
    );
    // A malformed payload yields boolean false, as PHP does.
    assert_eq!(
        run(r#"<?php var_dump(unserialize("garbage"));"#),
        "bool(false)\n"
    );
    // Trailing bytes after a complete value is a failure.
    assert_eq!(
        run(r#"<?php var_dump(unserialize("i:1;X"));"#),
        "bool(false)\n"
    );
}

#[test]
fn unserialize_array_and_roundtrip() {
    // Direct parse of a nested array payload.
    assert_eq!(
        run(r#"<?php print_r(unserialize('a:2:{s:1:"a";i:1;i:0;a:1:{i:0;s:2:"hi";}}'));"#),
        "Array\n(\n    [a] => 1\n    [0] => Array\n        (\n            [0] => hi\n        )\n\n)\n"
    );
    // Round trip: serialize then unserialize reproduces the structure.
    let src = r#"<?php
        $a = ["name" => "Ada", "nums" => [1, 2, 3], "ok" => true, "z" => null];
        $b = unserialize(serialize($a));
        echo $b["name"], "|", $b["nums"][2], "|", ($b["ok"] ? "T" : "F"), "|";
        var_dump($b["z"]);
    "#;
    assert_eq!(run(src), "Ada|3|T|NULL\n");
}

#[test]
fn serialize_float_special_values() {
    // Non-finite floats round-trip through their PHP spellings.
    // Parse the PHP spellings back to floats, then re-serialize: exercises both
    // the parser's special-case branch and `serialize_float` without relying on
    // runtime INF/NAN constants (this runtime treats those barewords as strings).
    let src = r#"<?php
        echo serialize(unserialize("d:INF;")), " ";
        echo serialize(unserialize("d:-INF;")), " ";
        echo serialize(unserialize("d:NAN;"));
    "#;
    assert_eq!(run(src), "d:INF; d:-INF; d:NAN;");
}
