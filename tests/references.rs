//! References: `$b = &$a` variable aliasing and `foreach ($a as &$v)` by-reference
//! iteration (element write-back).

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn reference_alias_writes_both_ways() {
    let src = r#"<?php $a = 1; $b = &$a; $b = 99;
        echo $a, "|", $b;"#;
    assert_eq!(run(src), "99|99");
}

#[test]
fn change_to_source_visible_in_alias() {
    let src = r#"<?php $a = 5; $b = &$a; $a = 10; echo $b;"#;
    assert_eq!(run(src), "10");
}

#[test]
fn unset_breaks_the_binding() {
    let src = r#"<?php $a = 1; $b = &$a; unset($b); $b = 50;
        echo $a, "|", $b;"#;
    // Unsetting $b removes only its binding; $a keeps its value.
    assert_eq!(run(src), "1|50");
}

#[test]
fn reference_chain() {
    let src = r#"<?php $a = 1; $b = &$a; $c = &$b; $c = 7; echo $a, $b, $c;"#;
    assert_eq!(run(src), "777");
}

#[test]
fn foreach_by_reference_mutates_elements() {
    let src = r#"<?php $a = [1, 2, 3];
        foreach ($a as &$v) { $v = $v * 10; }
        unset($v);
        echo implode(",", $a);"#;
    assert_eq!(run(src), "10,20,30");
}

#[test]
fn foreach_by_reference_with_key() {
    let src = r#"<?php $a = ["x" => 1, "y" => 2];
        foreach ($a as $k => &$v) { $v = $k . $v; }
        echo implode(",", $a);"#;
    assert_eq!(run(src), "x1,y2");
}

#[test]
fn foreach_by_reference_continue_preserves_element() {
    let src = r#"<?php $a = [1, 2, 3, 4];
        foreach ($a as &$v) { if ($v == 2) continue; $v = 0; }
        echo implode(",", $a);"#;
    assert_eq!(run(src), "0,2,0,0");
}
