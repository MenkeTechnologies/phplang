//! stdlib `textx`: stream printf family (fprintf/vfprintf/fscanf) plus
//! array_change_key_case and get_html_translation_table.

use phplang::eval_capture;
use std::path::PathBuf;

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("phplang_textx_{}_{name}", std::process::id()))
}

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn fprintf_writes_and_returns_bytes() {
    let p = tmp("fprintf.txt");
    let ps = p.display();
    let src = format!(
        r#"<?php
        $h = fopen("{ps}", "w");
        $n = fprintf($h, "%s=%03d;", "x", 5);
        fclose($h);
        echo $n, "|", file_get_contents("{ps}");"#
    );
    // "x=005;" is 6 bytes.
    assert_eq!(run(&src), "6|x=005;");
    let _ = std::fs::remove_file(&p);
}

#[test]
fn vfprintf_takes_array_args() {
    let p = tmp("vfprintf.txt");
    let ps = p.display();
    let src = format!(
        r#"<?php
        $h = fopen("{ps}", "w");
        $n = vfprintf($h, "%d-%d-%s", [3, 9, "z"]);
        fclose($h);
        echo $n, "|", file_get_contents("{ps}");"#
    );
    assert_eq!(run(&src), "5|3-9-z");
    let _ = std::fs::remove_file(&p);
}

#[test]
fn fscanf_parses_a_line() {
    let p = tmp("fscanf.txt");
    let ps = p.display();
    std::fs::write(&p, "age 42 pi 3.14\n").unwrap();
    let src = format!(
        r#"<?php
        $h = fopen("{ps}", "r");
        $r = fscanf($h, "%s %d %s %f");
        fclose($h);
        echo $r[0], "|", $r[1], "|", $r[2], "|", $r[3];"#
    );
    assert_eq!(run(&src), "age|42|pi|3.14");
    let _ = std::fs::remove_file(&p);
}

#[test]
fn fscanf_returns_false_at_eof() {
    // PHP 8: the 2-arg fscanf returns bool(false) at EOF (not -1), so the
    // idiomatic `while ($r = fscanf(...))` loop terminates.
    let p = tmp("fscanf_eof.txt");
    let ps = p.display();
    std::fs::write(&p, "").unwrap();
    let src = format!(
        r#"<?php
        $h = fopen("{ps}", "r");
        $r = fscanf($h, "%d");
        fclose($h);
        echo var_export($r, true);"#
    );
    assert_eq!(run(&src), "false");
    let _ = std::fs::remove_file(&p);
}

#[test]
fn array_change_key_case_upper() {
    // Task-mandated: ["Foo"=>1] with CASE_UPPER must have key "FOO".
    let src = r#"<?php
        $r = array_change_key_case(["Foo"=>1, "bAr"=>2], CASE_UPPER);
        echo array_key_exists("FOO", $r) ? "y" : "n";
        echo array_key_exists("BAR", $r) ? "y" : "n";
        echo $r["FOO"], $r["BAR"];"#;
    assert_eq!(run(src), "yy12");
}

#[test]
fn array_change_key_case_lower_default_and_int_keys() {
    // Default is CASE_LOWER; integer keys are untouched.
    let src = r#"<?php
        $r = array_change_key_case(["Foo"=>1, 7=>2]);
        echo array_key_exists("foo", $r) ? "y" : "n";
        echo array_key_exists(7, $r) ? "y" : "n";
        echo $r["foo"], $r[7];"#;
    assert_eq!(run(src), "yy12");
}

#[test]
fn array_change_key_case_accepts_int_case() {
    // 0 == CASE_LOWER, 1 == CASE_UPPER.
    let src = r#"<?php
        $u = array_change_key_case(["ab"=>1], 1);
        $l = array_change_key_case(["AB"=>1], 0);
        echo array_key_exists("AB", $u) ? "y" : "n";
        echo array_key_exists("ab", $l) ? "y" : "n";"#;
    assert_eq!(run(src), "yy");
}

#[test]
fn html_translation_table_specialchars() {
    let src = r#"<?php
        $t = get_html_translation_table();
        echo $t["<"], $t[">"], $t["&"], $t["\""], $t["'"];"#;
    assert_eq!(run(src), "&lt;&gt;&amp;&quot;&#039;");
}

#[test]
fn html_translation_table_noquotes() {
    // ENT_NOQUOTES (0) omits both quote characters.
    let src = r#"<?php
        $t = get_html_translation_table(HTML_SPECIALCHARS, 0);
        echo isset($t["\""]) ? "dq" : "-";
        echo isset($t["'"]) ? "sq" : "-";
        echo $t["<"];"#;
    assert_eq!(run(src), "--&lt;");
}

#[test]
fn html_translation_table_entities_has_latin1() {
    // HTML_ENTITIES adds the ISO-8859-1 supplement; U+00A9 -> &copy;.
    let src = r#"<?php
        $e = get_html_translation_table(HTML_ENTITIES);
        $copy = chr(0xA9);   // phplang chr() UTF-8-encodes the code point (U+00A9)
        $nbsp = chr(0xA0);   // U+00A0
        echo $e[$copy], "|", $e[$nbsp], "|", $e["&"];"#;
    assert_eq!(run(src), "&copy;|&nbsp;|&amp;");
}
