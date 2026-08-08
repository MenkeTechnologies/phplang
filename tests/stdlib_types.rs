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
    assert_eq!(
        run(r#"<?php $f = fn() => 1; echo is_object($f) ? "y" : "n";"#),
        "y"
    );
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
    assert_eq!(
        run(r#"<?php var_dump(unserialize("b:1;"));"#),
        "bool(true)\n"
    );
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
fn serialize_float_e_notation() {
    // serialize_precision = -1: shortest round-trip digits with %G-style switching
    // to E-notation for large/small magnitudes. Expected strings cross-checked
    // against reference php (`serialize(1e17)` = `d:1.0E+17;`, `1e16` stays fixed).
    assert_eq!(run(r#"<?php echo serialize(1e20);"#), "d:1.0E+20;");
    assert_eq!(run(r#"<?php echo serialize(1e-10);"#), "d:1.0E-10;");
    assert_eq!(run(r#"<?php echo serialize(1.5);"#), "d:1.5;");
    assert_eq!(run(r#"<?php echo serialize(0.1);"#), "d:0.1;");
    // Threshold: decpt > 17 flips to E-notation; 1e16 is right below it.
    assert_eq!(
        run(r#"<?php echo serialize(1e16);"#),
        "d:10000000000000000;"
    );
    assert_eq!(run(r#"<?php echo serialize(1e17);"#), "d:1.0E+17;");
    assert_eq!(run(r#"<?php echo serialize(0.0001);"#), "d:0.0001;");
    assert_eq!(run(r#"<?php echo serialize(0.00001);"#), "d:1.0E-5;");
    // Full-precision mantissa in E-notation.
    assert_eq!(
        run(r#"<?php echo serialize(1.844674407371E19);"#),
        "d:1.844674407371E+19;"
    );
    // Round-trips through unserialize.
    assert_eq!(
        run(r#"<?php var_dump(unserialize(serialize(1e20)) === 1e20 ? 1 : 0);"#),
        "int(1)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(unserialize(serialize(1e-10)) === 1e-10 ? 1 : 0);"#),
        "int(1)\n"
    );
}

#[test]
fn unserialize_integer_overflow_saturates() {
    // PHP clamps an out-of-range `i:` literal to PHP_INT_MAX / PHP_INT_MIN
    // (with a warning) instead of returning false.
    assert_eq!(
        run(r#"<?php var_dump(unserialize("i:99999999999999999999;"));"#),
        "int(9223372036854775807)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(unserialize("i:-99999999999999999999;"));"#),
        "int(-9223372036854775808)\n"
    );
    // A non-numeric integer body still fails.
    assert_eq!(
        run(r#"<?php var_dump(unserialize("i:12abc;"));"#),
        "bool(false)\n"
    );
}

#[test]
fn unserialize_negative_array_count_fails() {
    // A negative element count is malformed and yields false, not an empty array.
    assert_eq!(
        run(r#"<?php var_dump(unserialize("a:-1:{}"));"#),
        "bool(false)\n"
    );
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

// ── float representation (serialize_precision vs precision) ─────────────────
//
// `echo` renders floats at precision=14, but `var_dump`, `var_export`,
// `serialize` and `json_encode` all use the shortest round-tripping form. These
// used to share the precision=14 path and silently lost digits.

#[test]
fn var_dump_uses_shortest_roundtrip_float() {
    assert_eq!(
        run(r#"<?php var_dump(1/3);"#),
        "float(0.3333333333333333)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(PHP_INT_MAX + 1);"#),
        "float(9.223372036854776E+18)\n"
    );
    // echo keeps the lower precision=14 rendering.
    assert_eq!(run(r#"<?php echo 1/3;"#), "0.33333333333333");
}

#[test]
fn var_export_float_always_reads_back_as_float() {
    // A whole-numbered float gains a ".0" tail so the output re-parses as float.
    assert_eq!(run(r#"<?php echo var_export(1.0, true);"#), "1.0");
    assert_eq!(run(r#"<?php echo var_export(100.0, true);"#), "100.0");
    assert_eq!(run(r#"<?php echo var_export(-0.0, true);"#), "-0.0");
    // Exponential and non-finite forms already contain a "." or are special.
    assert_eq!(run(r#"<?php echo var_export(1e17, true);"#), "1.0E+17");
    assert_eq!(run(r#"<?php echo var_export(NAN, true);"#), "NAN");
}

#[test]
fn json_encode_float_and_escaping_match_reference() {
    // JSON spells the exponent lowercase and escapes "/" and non-ASCII.
    assert_eq!(
        run(r#"<?php echo json_encode([1.0, 0.1, 1e100]);"#),
        "[1,0.1,1.0e+100]"
    );
    assert_eq!(run(r#"<?php echo json_encode("a/b");"#), r#""a\/b""#);
    assert_eq!(run(r#"<?php echo json_encode("é");"#), r#""\u00e9""#);
    // Above the BMP JSON needs a surrogate pair.
    assert_eq!(
        run("<?php echo json_encode(\"\u{1F600}\");"),
        r#""\ud83d\ude00""#
    );
    assert_eq!(
        run(
            r#"<?php echo json_encode("é", JSON_UNESCAPED_UNICODE), json_encode("a/b", JSON_UNESCAPED_SLASHES);"#
        ),
        r#""é""a/b""#
    );
}

#[test]
fn json_encode_rejects_non_finite_floats() {
    assert_eq!(
        run(r#"<?php var_dump(json_encode([1, INF])); echo json_last_error_msg();"#),
        "bool(false)\nInf and NaN cannot be JSON encoded"
    );
}

#[test]
fn json_encode_distinguishes_objects_from_lists() {
    // An empty object is `{}`; an empty array is `[]`.
    assert_eq!(
        run(r#"<?php echo json_encode(new stdClass()), json_encode([]);"#),
        "{}[]"
    );
    assert_eq!(
        run("<?php echo json_encode([\"a\"=>1], JSON_PRETTY_PRINT);"),
        "{\n    \"a\": 1\n}"
    );
}

// ── (array) / (object) casts ────────────────────────────────────────────────

#[test]
fn array_cast_wraps_scalars_and_unwraps_objects() {
    assert_eq!(run(r#"<?php echo json_encode((array)"a");"#), r#"["a"]"#);
    assert_eq!(run(r#"<?php echo json_encode((array)null);"#), "[]");
    assert_eq!(run(r#"<?php echo json_encode((array)1);"#), "[1]");
    assert_eq!(
        run(r#"<?php echo json_encode((array)(object)["a"=>1]);"#),
        r#"{"a":1}"#
    );
}

#[test]
fn object_cast_builds_a_stdclass() {
    assert_eq!(
        run(r#"<?php $o = (object)["a"=>1]; echo get_class($o), ":", $o->a;"#),
        "stdClass:1"
    );
    // A non-array scalar lands in a `scalar` property; null gives an empty one.
    assert_eq!(run(r#"<?php echo ((object)"s")->scalar;"#), "s");
    assert_eq!(run(r#"<?php echo json_encode((object)null);"#), "{}");
}

#[test]
fn var_dump_prints_objects_with_class_and_properties() {
    assert_eq!(
        run(r#"<?php $p = new stdClass; $p->z = 9; var_dump($p);"#),
        "object(stdClass)#1 (1) {\n  [\"z\"]=>\n  int(9)\n}\n"
    );
}

#[test]
fn intval_honours_an_explicit_base() {
    assert_eq!(
        run(r#"<?php echo intval("0x1A", 16), ",", intval("1A", 16);"#),
        "26,26"
    );
    assert_eq!(
        run(r#"<?php echo intval("012", 8), ",", intval("z", 36);"#),
        "10,35"
    );
    // Base 0 auto-detects from the prefix.
    assert_eq!(
        run(r#"<?php echo intval("0x1A", 0), ",", intval("012", 0), ",", intval("12", 0);"#),
        "26,10,12"
    );
    // Base 10 keeps the ordinary numeric-string reading, exponents included.
    assert_eq!(run(r#"<?php echo intval("1e3", 10);"#), "1000");
}
