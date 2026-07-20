//! The `unset()` construct: remove variables and array elements (no reindex).

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn unset_array_element_keeps_other_keys() {
    // Removing an element leaves a hole; remaining integer keys are NOT renumbered.
    let src = r#"<?php $a = [1, 2, 3]; unset($a[1]);
        echo implode(",", $a), "|", implode(",", array_keys($a));"#;
    assert_eq!(run(src), "1,3|0,2");
}

#[test]
fn unset_variable() {
    let src = r#"<?php $x = 5; unset($x); echo isset($x) ? "set" : "unset";"#;
    assert_eq!(run(src), "unset");
}

#[test]
fn unset_assoc_key() {
    let src = r#"<?php $m = ["a" => 1, "b" => 2, "c" => 3]; unset($m["b"]);
        echo implode(",", array_keys($m)), "|count:", count($m);"#;
    assert_eq!(run(src), "a,c|count:2");
}

#[test]
fn unset_nested_element() {
    let src = r#"<?php $a = ["x" => ["y" => 1, "z" => 2]]; unset($a["x"]["y"]);
        echo implode(",", array_keys($a["x"]));"#;
    assert_eq!(run(src), "z");
}

#[test]
fn unset_multiple_targets() {
    let src = r#"<?php $a = 1; $b = 2; $arr = [1, 2, 3]; unset($a, $b, $arr[0]);
        echo isset($a) ? "1" : "0", isset($b) ? "1" : "0", "|", implode(",", $arr);"#;
    assert_eq!(run(src), "00|2,3");
}

#[test]
fn unset_missing_key_is_quiet() {
    let src = r#"<?php $a = [1, 2]; unset($a[99]); unset($undef); echo count($a);"#;
    assert_eq!(run(src), "2");
}
