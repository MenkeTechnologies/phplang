//! End-to-end tests for the `json` stdlib category: `json_decode`,
//! `json_last_error`, `json_last_error_msg`. Expected values were cross-checked
//! against PHP 8; the assertions here run headless without `php`. Note phplang
//! has no `stdClass`, so JSON objects decode to PHP arrays (associative always).

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn decode_scalars_and_types() {
    // Integral numbers -> int, fractional/exponent -> float.
    assert_eq!(run(r#"<?php var_dump(json_decode("42"));"#), "int(42)\n");
    assert_eq!(run(r#"<?php var_dump(json_decode("-7"));"#), "int(-7)\n");
    assert_eq!(run(r#"<?php var_dump(json_decode("1.5"));"#), "float(1.5)\n");
    assert_eq!(
        run(r#"<?php var_dump(is_float(json_decode("1e3")));"#),
        "bool(true)\n"
    );
    // Booleans and null.
    assert_eq!(run(r#"<?php var_dump(json_decode("true"));"#), "bool(true)\n");
    assert_eq!(
        run(r#"<?php var_dump(json_decode("false"));"#),
        "bool(false)\n"
    );
    assert_eq!(run(r#"<?php var_dump(json_decode("null"));"#), "NULL\n");
    // Bare string.
    assert_eq!(
        run(r#"<?php var_dump(json_decode("\"hi\""));"#),
        "string(2) \"hi\"\n"
    );
}

#[test]
fn decode_big_integer_becomes_float() {
    // Beyond i64 range PHP returns a float (no JSON_BIGINT_AS_STRING).
    assert_eq!(
        run(r#"<?php var_dump(is_float(json_decode("99999999999999999999")));"#),
        "bool(true)\n"
    );
    // In range stays int.
    assert_eq!(
        run(r#"<?php var_dump(json_decode("9223372036854775807"));"#),
        "int(9223372036854775807)\n"
    );
}

#[test]
fn decode_array() {
    assert_eq!(
        run(r#"<?php $a = json_decode("[1,2,3]"); echo $a[0]+$a[1]+$a[2], "|", count($a);"#),
        "6|3"
    );
    // Empty array.
    assert_eq!(
        run(r#"<?php var_dump(count(json_decode("[]")));"#),
        "int(0)\n"
    );
    // Mixed element types, whitespace tolerated.
    assert_eq!(
        run(r#"<?php $a = json_decode(" [ 1 , \"x\" , true , null ] "); echo $a[0], $a[1], ($a[2]?"T":"F"); var_dump($a[3]);"#),
        "1xTNULL\n"
    );
}

#[test]
fn decode_object_to_assoc_array() {
    let src = r#"<?php $o = json_decode('{"name":"bob","age":30,"ok":true}');
        echo $o["name"], "|", $o["age"], "|", ($o["ok"]?"T":"F");"#;
    assert_eq!(run(src), "bob|30|T");
    // Numeric string object keys normalize to int keys (PHP array coercion).
    assert_eq!(
        run(r#"<?php $o = json_decode('{"0":"a","1":"b"}'); echo $o[0], $o[1], "|", array_is_list($o)?"L":"M";"#),
        "ab|L"
    );
}

#[test]
fn decode_nested() {
    let src = r#"<?php $o = json_decode('{"a":[1,{"b":2}],"c":{"d":[3,4]}}');
        echo $o["a"][0], $o["a"][1]["b"], $o["c"]["d"][1];"#;
    // $o["a"][0]=1, $o["a"][1]["b"]=2, $o["c"]["d"][1]=4.
    assert_eq!(run(src), "124");
}

#[test]
fn decode_string_escapes() {
    // Standard two-char escapes: "a\nb\tc" -> 5 chars (a, LF, b, TAB, c).
    assert_eq!(
        run(r#"<?php echo strlen(json_decode('"a\nb\tc"'));"#),
        "5"
    );
    // Escaped solidus \/ decodes to '/'. (PHP single quotes keep `\/` verbatim.)
    assert_eq!(run(r#"<?php echo json_decode('"a\/b"');"#), "a/b");
    // Escaped quote \" decodes to '"'.
    assert_eq!(run(r#"<?php echo json_decode('"q\"q"');"#), "q\"q");
    // Escaped backslash \\ decodes to one backslash. The Rust raw string passes
    // `'"\\\\"'` to PHP; PHP single quotes collapse `\\\\` -> `\\`, so the JSON
    // input is `"\\"`, which decodes to a single backslash.
    assert_eq!(run(r#"<?php echo json_decode('"\\\\"');"#), "\\");
    // \uXXXX for a BMP code point.
    assert_eq!(
        run(r#"<?php echo json_decode('"café"');"#),
        "caf\u{00e9}"
    );
    // Surrogate pair -> astral code point (U+1F600 grinning face).
    assert_eq!(
        run(r#"<?php echo json_decode('"😀"');"#),
        "\u{1F600}"
    );
}

#[test]
fn decode_errors_set_last_error() {
    // Valid decode: error resets to none (0).
    assert_eq!(
        run(r#"<?php json_decode("[1,2]"); echo json_last_error(), "|", json_last_error_msg();"#),
        "0|No error"
    );
    // Syntax error -> null result, code 4.
    assert_eq!(
        run(r#"<?php var_dump(json_decode("[1,")); echo json_last_error(), "|", json_last_error_msg();"#),
        "NULL\n4|Syntax error"
    );
    // Empty string is a syntax error in PHP.
    assert_eq!(
        run(r#"<?php json_decode(""); echo json_last_error();"#),
        "4"
    );
    // Trailing garbage after a complete value.
    assert_eq!(
        run(r#"<?php json_decode("1 2"); echo json_last_error();"#),
        "4"
    );
    // Leading-zero and bare-fraction numbers are invalid JSON.
    assert_eq!(
        run(r#"<?php json_decode("01"); echo json_last_error();"#),
        "4"
    );
}

#[test]
fn decode_depth_limit() {
    // depth=1 allows a single container level.
    assert_eq!(
        run(r#"<?php var_dump(json_decode("[1]", true, 1) === null ? 0 : 1); echo json_last_error();"#),
        "int(1)\n0"
    );
    // A nested container at depth=1 exceeds the limit -> JSON_ERROR_DEPTH (1).
    assert_eq!(
        run(r#"<?php json_decode("[[1]]", true, 1); echo json_last_error();"#),
        "1"
    );
}

#[test]
fn round_trip_encode_decode() {
    // json_decode(json_encode($x)) preserves the structure.
    let src = r#"<?php
        $x = ["a" => 1, "b" => [2, 3], "c" => "text", "d" => true, "e" => null];
        $y = json_decode(json_encode($x));
        echo $y["a"], $y["b"][0], $y["b"][1], $y["c"], ($y["d"]?"T":"F");
        var_dump($y["e"]);
    "#;
    assert_eq!(run(src), "123textTNULL\n");

    // A list round-trips as a JSON array and stays a list.
    assert_eq!(
        run(r#"<?php $y = json_decode(json_encode([10,20,30])); echo $y[0]+$y[1]+$y[2], "|", array_is_list($y)?"L":"M";"#),
        "60|L"
    );

    // Unicode survives the round trip (encode emits raw UTF-8, decode reads it).
    assert_eq!(
        run(r#"<?php echo json_decode(json_encode("café 😀"));"#),
        "caf\u{00e9} \u{1F600}"
    );
}
