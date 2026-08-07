//! Standard-library functions with a by-reference OUT parameter: `preg_match`'s
//! `$matches`, `parse_str`'s result array, `similar_text`'s percentage and
//! `str_replace`'s count. Each publishes its value at the parameter's position
//! and the call site stores it into the caller's variable — the same path a user
//! function's `&$x` parameter takes — so the variable need not exist beforehand.
//!
//! Every expected value here was taken from php 8.5.9 running the same program.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn preg_match_defines_the_matches_variable() {
    let src = r#"<?php
        if (preg_match('/(\d+)-(\d+)/', 'ab 12-34 cd', $m)) {
            echo $m[1], "/", $m[2], "/", count($m);
        }"#;
    assert_eq!(run(src), "12/34/3");
}

#[test]
fn a_failed_match_resets_the_matches_variable() {
    let src = r#"<?php $m = ['stale'];
        echo preg_match('/zzz/', 'abc', $m), count($m);"#;
    assert_eq!(run(src), "00");
}

#[test]
fn preg_match_all_collects_every_match() {
    let src = r#"<?php preg_match_all('/\d/', 'a1b2c3', $all);
        echo count($all), count($all[0]), $all[0][2];"#;
    assert_eq!(run(src), "133");
}

#[test]
fn parse_str_writes_its_second_parameter_and_returns_null() {
    let src = r#"<?php $r = parse_str('a=1&b[]=2&b[]=3', $out);
        echo var_export($r, true), "|", json_encode($out);"#;
    assert_eq!(run(src), r#"NULL|{"a":"1","b":["2","3"]}"#);
}

#[test]
fn parse_str_replaces_what_the_variable_held() {
    let src = r#"<?php $pre = ['stale' => 1]; parse_str('x=9', $pre);
        echo json_encode($pre);"#;
    assert_eq!(run(src), r#"{"x":"9"}"#);
}

#[test]
fn similar_text_reports_its_percentage() {
    let src = r#"<?php $n = similar_text("World", "word", $pct);
        echo $n, "|", round($pct, 2);"#;
    assert_eq!(run(src), "3|66.67");
}

#[test]
fn str_replace_reports_its_replacement_count() {
    let src = r#"<?php $r = str_replace("a", "b", "banana", $cnt); echo $r, "|", $cnt;"#;
    assert_eq!(run(src), "bbnbnb|3");
}

#[test]
fn a_call_that_writes_no_out_value_does_not_see_the_last_one() {
    // The OUT slots are cleared per call, so `$b` cannot pick up `$a`'s captures.
    let src = r#"<?php preg_match('/(x)/', 'x', $a);
        strlen("hi");
        echo count($a), "|", preg_match('/(y)/', 'y', $b), count($b);"#;
    assert_eq!(run(src), "2|12");
}
