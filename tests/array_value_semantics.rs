//! A PHP array is a **value**, not a handle: assigning one, passing one to a
//! function, storing one in a property or an element, and binding one in a
//! by-value `foreach` all hand over a copy, so a write through the new name is
//! invisible through the old one. An object stored *inside* an array stays a
//! handle, which is the line PHP draws and the line these tests pin.
//!
//! Every expected value here was taken from php 8.5.9 running the same program.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn assignment_copies_the_array() {
    let src = r#"<?php $a = [1, 2, 3]; $b = $a; $b[] = 4; $b[0] = 99;
        echo count($a), $a[0], "|", count($b), $b[0];"#;
    assert_eq!(run(src), "31|499");
}

#[test]
fn the_copy_is_deep_through_nested_arrays() {
    let src = r#"<?php $s1 = ['a' => ['b' => ['c' => 1]]]; $s2 = $s1;
        $s2['a']['b']['c'] = 2;
        echo $s1['a']['b']['c'], $s2['a']['b']['c'];"#;
    assert_eq!(run(src), "12");
}

#[test]
fn an_object_inside_a_copied_array_is_still_shared() {
    // The array is copied; the object it holds is a handle, so both copies see
    // the same instance. This is the one thing a deep copy must NOT do.
    let src = r#"<?php class C { public $p = 0; }
        $holder = ['o' => new C()];
        $copy = $holder;
        $copy['o']->p = 7;
        echo $holder['o']->p;"#;
    assert_eq!(run(src), "7");
}

#[test]
fn a_by_value_parameter_takes_a_copy() {
    let src = r#"<?php function f($arr) { $arr[] = 'x'; return count($arr); }
        $c = [1]; echo f($c), count($c);"#;
    assert_eq!(run(src), "21");
}

#[test]
fn a_by_reference_parameter_does_not() {
    let src = r#"<?php function f(array &$q) { $q[] = 5; }
        $w = [1]; f($w); echo count($w);"#;
    assert_eq!(run(src), "2");
}

#[test]
fn a_property_and_an_element_each_store_a_copy() {
    let src = r#"<?php class C { public $p = []; }
        $a = [1, 2];
        $o = new C(); $o->p = $a; $o->p[] = 3;
        $box = []; $box['k'] = $a; $box['k'][] = 9;
        echo count($a), count($o->p), count($box['k']);"#;
    assert_eq!(run(src), "233");
}

#[test]
fn foreach_binds_a_copy_by_value_and_the_element_by_reference() {
    let by_value = r#"<?php $m = [[1], [2]];
        foreach ($m as $row) { $row[] = 'z'; }
        echo count($m[0]);"#;
    assert_eq!(run(by_value), "1");

    let by_ref = r#"<?php $n = [[1], [2]];
        foreach ($n as &$row) { $row[] = 'z'; }
        unset($row);
        echo count($n[0]), count($n[1]);"#;
    assert_eq!(run(by_ref), "22");
}

#[test]
fn a_reference_assignment_still_shares() {
    // `$i = &$h` is not an assignment of the array's value; the two names are
    // one variable, so an append through either is visible through both.
    let src = r#"<?php $h = [1, 2, 3]; $i = &$h; $i[] = 4; echo count($h);"#;
    assert_eq!(run(src), "4");
}

#[test]
fn a_returned_array_is_independent_of_the_next_call() {
    let src = r#"<?php function h() { $t = [1, 2]; return $t; }
        $x = h(); $x[] = 3; $y = h(); echo count($x), count($y);"#;
    assert_eq!(run(src), "32");
}

#[test]
fn appending_inside_a_foreach_does_not_extend_the_iteration() {
    // The loop iterates the array as it was when the loop began.
    let src = r#"<?php $g = [1, 2];
        foreach ($g as $v) { $g[] = $v; if (count($g) > 10) break; }
        echo count($g);"#;
    assert_eq!(run(src), "4");
}
