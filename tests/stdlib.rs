//! Standard-library end-to-end tests: PHP source in, captured `echo` output out.
//! Covers the string/math/array/util functions added to `builtins::call_library`.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn string_predicates_and_pad() {
    assert_eq!(
        run(r#"<?php echo str_contains("hello world", "o w") ? "y" : "n";"#),
        "y"
    );
    assert_eq!(
        run(r#"<?php echo str_starts_with("phplang", "php") ? "y" : "n";"#),
        "y"
    );
    assert_eq!(
        run(r#"<?php echo str_ends_with("script.php", ".php") ? "y" : "n";"#),
        "y"
    );
    assert_eq!(run(r#"<?php echo str_pad("7", 3, "0", 0);"#), "007"); // STR_PAD_LEFT
    assert_eq!(run(r#"<?php echo str_pad("hi", 6, "*", 2);"#), "**hi**"); // STR_PAD_BOTH
}

#[test]
fn string_case_and_words() {
    assert_eq!(
        run(r#"<?php echo ucwords("the quick brown fox");"#),
        "The Quick Brown Fox"
    );
    assert_eq!(run(r#"<?php echo lcfirst("Hello");"#), "hello");
    assert_eq!(run(r#"<?php echo str_word_count("one two three");"#), "3");
    assert_eq!(run(r#"<?php echo strcmp("a", "b");"#), "-1");
    assert_eq!(run(r#"<?php echo strcasecmp("ABC", "abc");"#), "0");
}

#[test]
fn string_encoding_helpers() {
    assert_eq!(
        run(r#"<?php echo htmlspecialchars("<a href=\"x\">&");"#),
        "&lt;a href=&quot;x&quot;&gt;&amp;"
    );
    assert_eq!(run(r#"<?php echo chr(65), ord("A");"#), "A65");
    assert_eq!(
        run(r#"<?php echo dechex(255), " ", hexdec("ff");"#),
        "ff 255"
    );
    assert_eq!(run(r#"<?php echo bin2hex("AB");"#), "4142");
}

#[test]
fn str_split_builds_array() {
    assert_eq!(
        run(r#"<?php echo implode("-", str_split("abcd", 2));"#),
        "ab-cd"
    );
    assert_eq!(run(r#"<?php echo count(str_split("hello"));"#), "5");
}

#[test]
fn number_format_groups_and_rounds() {
    assert_eq!(
        run(r#"<?php echo number_format(1234567.891, 2);"#),
        "1,234,567.89"
    );
    assert_eq!(run(r#"<?php echo number_format(1000);"#), "1,000");
    assert_eq!(
        run(r#"<?php echo number_format(1234.5678, 2, ".", " ");"#),
        "1 234.57"
    );
}

#[test]
fn math_functions() {
    assert_eq!(run(r#"<?php echo pow(2, 8);"#), "256");
    assert_eq!(run(r#"<?php echo intdiv(17, 5);"#), "3");
    assert_eq!(run(r#"<?php echo round(pi(), 4);"#), "3.1416");
    assert_eq!(run(r#"<?php echo fmod(10, 3);"#), "1");
}

#[test]
fn intdiv_by_zero_errors() {
    // A zero divisor is a DivisionByZeroError specifically — intdiv's OTHER
    // failure (PHP_INT_MIN / -1) is an ArithmeticError, and `is_err()` could not
    // tell the two apart even though a `catch (DivisionByZeroError)` can.
    assert_eq!(
        run(
            "<?php try { intdiv(1, 0); } catch (Throwable $e) { echo get_class($e), ': ', $e->getMessage(); }"
        ),
        "DivisionByZeroError: Division by zero"
    );
}

#[test]
fn array_sum_and_product() {
    assert_eq!(run(r#"<?php echo array_sum([1, 2, 3, 4]);"#), "10");
    assert_eq!(run(r#"<?php echo array_product([1, 2, 3, 4]);"#), "24");
    assert_eq!(run(r#"<?php echo array_sum([1.5, 2.5]);"#), "4");
}

#[test]
fn array_map_and_filter_with_named_callbacks() {
    let src = r#"<?php
        function dbl($x) { return $x * 2; }
        echo implode(",", array_map("dbl", [1, 2, 3]));"#;
    assert_eq!(run(src), "2,4,6");

    let src2 = r#"<?php
        function big($x) { return $x > 2; }
        echo implode(",", array_values(array_filter([1, 2, 3, 4], "big")));"#;
    assert_eq!(run(src2), "3,4");
}

#[test]
fn array_reduce_folds() {
    let src = r#"<?php
        function add($carry, $x) { return $carry + $x; }
        echo array_reduce([1, 2, 3, 4], "add", 0);"#;
    assert_eq!(run(src), "10");
}

#[test]
fn array_slice_and_reverse() {
    assert_eq!(
        run(r#"<?php echo implode(",", array_slice([10, 20, 30, 40, 50], 1, 3));"#),
        "20,30,40"
    );
    assert_eq!(
        run(r#"<?php echo implode(",", array_reverse([1, 2, 3]));"#),
        "3,2,1"
    );
    assert_eq!(
        run(r#"<?php echo implode(",", array_slice([1, 2, 3, 4], -2));"#),
        "3,4"
    );
}

#[test]
fn array_merge_renumbers_and_overwrites() {
    assert_eq!(
        run(r#"<?php echo implode(",", array_merge([1, 2], [3, 4]));"#),
        "1,2,3,4"
    );
    let src = r#"<?php $m = array_merge(["a" => 1], ["a" => 2, "b" => 3]);
        echo $m["a"], $m["b"];"#;
    assert_eq!(run(src), "23");
}

#[test]
fn array_key_and_search() {
    assert_eq!(
        run(r#"<?php echo array_key_exists("x", ["x" => 1]) ? "y" : "n";"#),
        "y"
    );
    assert_eq!(
        run(r#"<?php echo array_key_exists("z", ["x" => 1]) ? "y" : "n";"#),
        "n"
    );
    assert_eq!(run(r#"<?php echo array_search(30, [10, 20, 30]);"#), "2");
}

#[test]
fn array_unique_and_flip() {
    assert_eq!(
        run(r#"<?php echo implode(",", array_values(array_unique([1, 2, 2, 3, 3, 3])));"#),
        "1,2,3"
    );
    let src = r#"<?php $f = array_flip(["a" => 0, "b" => 1]); echo $f[0], $f[1];"#;
    assert_eq!(run(src), "ab");
}

#[test]
fn sort_mutates_in_place() {
    // PHP sort/rsort sort the array in place and return true.
    assert_eq!(
        run(r#"<?php $a = [3, 1, 2]; sort($a); echo implode(",", $a);"#),
        "1,2,3"
    );
    assert_eq!(
        run(r#"<?php $a = [1, 3, 2]; rsort($a); echo implode(",", $a);"#),
        "3,2,1"
    );
    assert_eq!(
        run(r#"<?php $a = [3, 1, 2]; echo sort($a) ? "y" : "n";"#),
        "y"
    );
}

#[test]
fn ksort_and_asort_in_place_preserve_keys() {
    let src = r#"<?php $a = ["c" => 3, "a" => 1, "b" => 2]; asort($a);
        echo implode(",", array_keys($a));"#; // value-sorted: 1,2,3 -> keys a,b,c
    assert_eq!(run(src), "a,b,c");
    let src2 = r#"<?php $k = ["b" => 2, "a" => 1, "c" => 3]; ksort($k);
        echo implode(",", array_keys($k));"#;
    assert_eq!(run(src2), "a,b,c");
}

#[test]
fn array_fill_and_combine() {
    assert_eq!(
        run(r#"<?php echo implode(",", array_fill(0, 3, "x"));"#),
        "x,x,x"
    );
    let src = r#"<?php $c = array_combine(["a", "b"], [1, 2]); echo $c["a"], $c["b"];"#;
    assert_eq!(run(src), "12");
}

#[test]
fn array_diff_and_intersect() {
    assert_eq!(
        run(r#"<?php echo implode(",", array_values(array_diff([1, 2, 3, 4], [2, 4])));"#),
        "1,3"
    );
    assert_eq!(
        run(r#"<?php echo implode(",", array_values(array_intersect([1, 2, 3, 4], [2, 4, 5])));"#),
        "2,4"
    );
}

#[test]
fn json_encode_scalars_lists_and_maps() {
    assert_eq!(run(r#"<?php echo json_encode([1, 2, 3]);"#), "[1,2,3]");
    assert_eq!(
        run(r#"<?php echo json_encode(["a" => 1, "b" => 2]);"#),
        r#"{"a":1,"b":2}"#
    );
    assert_eq!(run(r#"<?php echo json_encode("he\"llo");"#), r#""he\"llo""#);
    assert_eq!(
        run(r#"<?php echo json_encode(true), json_encode(null);"#),
        "truenull"
    );
}

#[test]
fn var_export_and_boolval() {
    assert_eq!(run(r#"<?php echo var_export(42, true);"#), "42");
    assert_eq!(run(r#"<?php echo var_export("hi", true);"#), "'hi'");
    assert_eq!(
        run(r#"<?php echo boolval(0) ? "y" : "n", boolval("x") ? "y" : "n";"#),
        "ny"
    );
}

#[test]
fn is_callable_on_library_name() {
    assert_eq!(run(r#"<?php echo is_callable("strlen") ? "y" : "n";"#), "y");
}
