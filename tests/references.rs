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

#[test]
fn by_reference_parameter_scalar() {
    let src = r#"<?php
        function inc(&$x) { $x = $x + 1; }
        $n = 5; inc($n); inc($n);
        echo $n;"#;
    assert_eq!(run(src), "7");
}

#[test]
fn by_reference_parameter_swap() {
    let src = r#"<?php
        function swap(&$a, &$b) { $t = $a; $a = $b; $b = $t; }
        $x = 1; $y = 2; swap($x, $y);
        echo $x, $y;"#;
    assert_eq!(run(src), "21");
}

#[test]
fn value_parameter_is_not_written_back() {
    let src = r#"<?php
        function noref($x) { $x = 99; }
        $n = 5; noref($n);
        echo $n;"#;
    assert_eq!(run(src), "5");
}

#[test]
fn by_reference_parameter_forward_declared() {
    // The call precedes the definition; the pre-pass still wires the write-back.
    let src = r#"<?php $n = 10; bump($n); echo $n;
        function bump(&$x) { $x = $x * 2; }"#;
    assert_eq!(run(src), "20");
}

// ── references to a container slot (`$r = &$a['x']['y']`) ────────────────────
//
// Each expectation below is the verbatim stdout of the same program under the
// reference `php` 8.5.9.

#[test]
fn reference_to_a_nested_array_element_writes_both_ways() {
    let src = r#"<?php $d = ['x' => ['y' => 1]];
        $r = &$d['x']['y'];
        $r = 99; echo $d['x']['y'], "|";
        $d['x']['y'] = 7; echo $r;"#;
    assert_eq!(run(src), "99|7");
}

#[test]
fn an_array_copy_keeps_a_referenced_element_shared() {
    // PHP's copy is deep in the arrays but a referenced slot is a reference in
    // both copies, so a write through the alias is visible in the copy too.
    let src = r#"<?php $a = [1,2,3]; $q = &$a[1]; $b = $a; $q = 55;
        echo implode(",", $a), "|", implode(",", $b);"#;
    assert_eq!(run(src), "1,55,3|1,55,3");
}

#[test]
fn an_array_element_can_be_bound_to_a_variable() {
    let src = r#"<?php $v = 10; $c = []; $c['k'] = &$v;
        $v = 20; echo $c['k'], "|";
        $c['k'] = 30; echo $v;"#;
    assert_eq!(run(src), "20|30");
}

#[test]
fn an_appended_element_can_be_bound_to_a_variable() {
    let src = r#"<?php $w = 1; $e = []; $e[] = &$w; $w = 2; echo $e[0];"#;
    assert_eq!(run(src), "2");
}

#[test]
fn object_properties_can_be_referenced_in_both_directions() {
    let src = r#"<?php class P { public $p = 1; }
        $o = new P();
        $pr = &$o->p; $pr = 42; echo $o->p, "|";
        $o->p = 43; echo $pr, "|";
        $z = 3; $o->p = &$z; $z = 4; echo $o->p;"#;
    assert_eq!(run(src), "42|43|4");
}

#[test]
fn a_reference_into_a_property_array_reaches_the_property() {
    // The path is rooted on the property's own array, not on a copy of it.
    let src = r#"<?php class P { public $items = []; }
        $o = new P(); $o->items = [1,2];
        $ir = &$o->items[1]; $ir = 9;
        echo implode(",", $o->items);"#;
    assert_eq!(run(src), "1,9");
}

#[test]
fn taking_a_reference_vivifies_a_missing_element_as_null() {
    let src = r#"<?php $m = []; $mr = &$m['new'];
        echo json_encode($m), "|"; $mr = 5; echo json_encode($m);"#;
    assert_eq!(run(src), r#"{"new":null}|{"new":5}"#);
}

#[test]
fn unsetting_an_element_leaves_the_alias_holding_the_value() {
    // `unset` removes the binding, not the cell: the alias keeps the value and
    // writing through it no longer reaches the array.
    let src = r#"<?php $i = [1,2]; $r = &$i[0]; unset($i[0]); $r = 7;
        echo json_encode($i), "|", $r;"#;
    assert_eq!(run(src), r#"{"1":2}|7"#);
}

#[test]
fn an_array_element_binds_to_a_by_reference_parameter() {
    let src = r#"<?php function bump(&$x) { $x++; }
        $n = [1,2]; bump($n[0]); echo implode(",", $n);"#;
    assert_eq!(run(src), "2,2");
}

#[test]
fn var_dump_marks_a_referenced_element() {
    let src = r#"<?php $a = [1,2,3]; $r = &$a[1]; var_dump($a);"#;
    assert_eq!(
        run(src),
        "array(3) {\n  [0]=>\n  int(1)\n  [1]=>\n  &int(2)\n  [2]=>\n  int(3)\n}\n"
    );
}

#[test]
fn a_method_can_return_a_reference_to_a_property_element() {
    let src = r#"<?php
        class Box {
            public $data = ['a' => 1];
            public function &get($k) { return $this->data[$k]; }
        }
        $b = new Box(); $v = &$b->get('a'); $v = 42;
        echo $b->data['a'];"#;
    assert_eq!(run(src), "42");
}

#[test]
fn a_by_value_call_of_a_by_reference_function_still_yields_the_value() {
    let src = r#"<?php
        function &pick(&$arr, $i) { return $arr[$i]; }
        $z = [1,2,3];
        echo pick($z, 1), "|";
        $p = &pick($z, 1); $p = 77;
        echo implode(",", $z);"#;
    assert_eq!(run(src), "2|1,77,3");
}
