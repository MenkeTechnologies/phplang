//! array_map over multiple arrays (+ null-callback zip) and the procedural
//! date/time wrappers (date_create/date_format/date_diff/…).

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn array_map_single_preserves_keys() {
    let src = r#"<?php $r = array_map(fn($x) => $x * 2, ["a" => 5, "b" => 6]);
        echo $r["a"], $r["b"];"#;
    assert_eq!(run(src), "1012");
}

#[test]
fn array_map_multiple_arrays() {
    let src =
        r#"<?php echo implode(",", array_map(fn($a, $b) => $a + $b, [1, 2, 3], [10, 20, 30]));"#;
    assert_eq!(run(src), "11,22,33");
}

#[test]
fn array_map_uneven_lengths_pad_with_null() {
    let src = r#"<?php $r = array_map(fn($a, $b) => $a . "/" . ($b ?? "-"), [1, 2, 3], ["x", "y"]);
        echo implode(",", $r);"#;
    assert_eq!(run(src), "1/x,2/y,3/-");
}

#[test]
fn array_map_null_callback_zips() {
    let src = r#"<?php $z = array_map(null, [1, 2], ["a", "b"]);
        echo $z[0][0], $z[0][1], "|", $z[1][0], $z[1][1];"#;
    assert_eq!(run(src), "1a|2b");
}

#[test]
fn date_create_and_format() {
    let src = r#"<?php $d = date_create("2020-06-15 10:30:00");
        echo date_format($d, "Y-m-d H:i:s");"#;
    assert_eq!(run(src), "2020-06-15 10:30:00");
}

#[test]
fn date_diff_days() {
    let src = r#"<?php $a = date_create("2020-01-01"); $b = date_create("2020-01-15");
        $i = date_diff($a, $b); echo $i->days, "|", $i->format("%d days");"#;
    assert_eq!(run(src), "14|14 days");
}

#[test]
fn date_timestamp_get() {
    let src = r#"<?php $d = date_create("1970-01-02 00:00:00");
        echo date_timestamp_get($d);"#;
    assert_eq!(run(src), "86400");
}
