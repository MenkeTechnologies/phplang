//! Operator tests for bitwise (`& | ^ << >> ~`), spaceship (`<=>`), null-coalesce
//! assignment (`??=`), and negative string offsets — all confirmed against
//! reference PHP 8 by the parity fuzzer (modes bitwise/spaceship/stroffset/
//! coalesce).

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn bitwise_operators() {
    assert_eq!(run(r#"<?php echo 5 & 3;"#), "1");
    assert_eq!(run(r#"<?php echo 5 | 2;"#), "7");
    assert_eq!(run(r#"<?php echo 6 ^ 3;"#), "5");
    assert_eq!(run(r#"<?php echo 1 << 4;"#), "16");
    assert_eq!(run(r#"<?php echo 255 >> 2;"#), "63");
    assert_eq!(run(r#"<?php echo ~5;"#), "-6");
}

#[test]
fn bitwise_precedence_and_compound() {
    // `+` binds tighter than `<<`, which binds tighter than `^`/`&`/`|`.
    assert_eq!(run(r#"<?php echo 2 + 3 & 4;"#), "4"); // (2+3) & 4
    assert_eq!(run(r#"<?php echo 1 | 2 ^ 3;"#), "1"); // 1 | (2^3)
    assert_eq!(run(r#"<?php echo 8 >> 1 + 1;"#), "2"); // 8 >> (1+1)
    assert_eq!(
        run(r#"<?php $n = 12; $n &= 10; $n |= 1; $n <<= 2; echo $n;"#),
        "36"
    );
}

#[test]
fn spaceship_operator() {
    assert_eq!(run(r#"<?php echo 3 <=> 7;"#), "-1");
    assert_eq!(run(r#"<?php echo 5 <=> 5;"#), "0");
    assert_eq!(run(r#"<?php echo 9 <=> 2;"#), "1");
    assert_eq!(run(r#"<?php echo "a" <=> "b";"#), "-1");
}

#[test]
fn null_coalesce_assignment() {
    assert_eq!(run(r#"<?php $x = null; $x ??= 5; echo $x;"#), "5");
    assert_eq!(run(r#"<?php $y = 10; $y ??= 99; echo $y;"#), "10");
    assert_eq!(
        run(r#"<?php $a = ['k' => 1]; $a['m'] ??= 8; echo $a['m'], $a['k'];"#),
        "81"
    );
}

#[test]
fn negative_string_offsets() {
    assert_eq!(run(r#"<?php echo "abc"[-1];"#), "c");
    assert_eq!(run(r#"<?php $s = "hello"; echo $s[-2];"#), "l");
    assert_eq!(run(r#"<?php $s = "hi"; echo $s[0], $s[-1];"#), "hi");
}

// ── comparison table (zend_compare) ─────────────────────────────────────────
//
// Relational comparison used to ignore PHP's operand-type table: bool/null
// operands must drag both sides to bool, `null` vs string compares as `""`, and
// arrays order by size then element-wise. Every expectation below is taken from
// reference PHP 8.5.

#[test]
fn null_and_bool_operands_compare_as_bool() {
    // false < true, so `null < -1` — a numeric comparison would say otherwise.
    assert_eq!(run(r#"<?php var_dump(null < -1);"#), "bool(true)\n");
    assert_eq!(run(r#"<?php echo null <=> -1, ",", -1 <=> null;"#), "-1,1");
    // A bool on either side wins over the string/number rules.
    assert_eq!(run(r#"<?php echo true <=> "a", ",", true <=> -1;"#), "0,0");
    assert_eq!(
        run(r#"<?php echo false <=> "0", ",", false <=> "a";"#),
        "0,-1"
    );
}

#[test]
fn null_versus_string_compares_as_empty_string() {
    // The bool rule would call these equal (bool("0") is false); PHP compares
    // "" against the string instead.
    assert_eq!(
        run(r#"<?php echo null <=> "0", ",", null <=> "a";"#),
        "-1,-1"
    );
    assert_eq!(run(r#"<?php echo null <=> "", ",", "a" <=> null;"#), "0,1");
}

#[test]
fn arrays_compare_by_size_then_elementwise() {
    assert_eq!(run(r#"<?php echo [1,2] <=> [1,3];"#), "-1");
    assert_eq!(run(r#"<?php echo [1,2,3] <=> [1,2];"#), "1");
    // Same size but a key missing on the right is "uncomparable" → greater.
    assert_eq!(run(r#"<?php echo ['a'=>1] <=> ['b'=>1];"#), "1");
    // An array outranks every non-array, non-bool, non-null operand.
    assert_eq!(run(r#"<?php echo [] <=> 0, ",", 0 <=> [];"#), "1,-1");
}

#[test]
fn sorts_are_stable_and_honour_flags() {
    // rsort must invert the comparator, not reverse the sorted result, or equal
    // elements come back in the wrong order.
    assert_eq!(
        run(r#"<?php $a=[1,"1",1.0]; rsort($a); echo implode(",", array_map("gettype", $a));"#),
        "integer,string,double"
    );
    assert_eq!(
        run(r#"<?php $b=['p'=>2,'q'=>1,'r'=>2]; arsort($b); echo implode(",", array_keys($b));"#),
        "p,r,q"
    );
    // SORT_STRING orders "10" before "9"; the default comparison would not.
    assert_eq!(
        run(r#"<?php $c=["10","9","1"]; sort($c, SORT_STRING); echo implode(",", $c);"#),
        "1,10,9"
    );
    assert_eq!(
        run(r#"<?php $d=["x10","x9"]; sort($d, SORT_NATURAL); echo implode(",", $d);"#),
        "x9,x10"
    );
}
