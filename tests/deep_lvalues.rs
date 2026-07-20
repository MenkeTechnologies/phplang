//! Deep / nested array lvalues: `$a[b][c] = v`, `$a[b][] = v`, mid-path appends
//! (`$a[][k] = v`), compound ops and `++`/`--` on an element, and
//! auto-vivification of intermediate arrays. Each case runs the compile → lower →
//! run-on-fusevm pipeline and reads results back through `echo`.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn two_dimensional_set_from_unset() {
    // Both levels auto-vivify from an unset variable.
    assert_eq!(run("<?php $a['x']['y'] = 7; echo $a['x']['y'];"), "7");
}

#[test]
fn three_dimensional_set_and_read() {
    assert_eq!(
        run("<?php $a[0][1][2] = 'deep'; echo $a[0][1][2];"),
        "deep"
    );
}

#[test]
fn nested_set_preserves_siblings() {
    assert_eq!(
        run("<?php $a['k']['a'] = 1; $a['k']['b'] = 2; echo $a['k']['a'], $a['k']['b'];"),
        "12"
    );
}

#[test]
fn append_into_nested_array() {
    // `$a['r'][] = ...` appends into the (auto-vivified) nested array.
    assert_eq!(
        run("<?php $a['r'][] = 7; $a['r'][] = 8; echo implode(',', $a['r']);"),
        "7,8"
    );
}

#[test]
fn mid_path_append_creates_element() {
    // `$a[][k] = v` appends a fresh sub-array `[k => v]` (verified vs PHP 8.5).
    assert_eq!(
        run("<?php $a[]['k'] = 'v'; echo $a[0]['k'];"),
        "v"
    );
}

#[test]
fn mid_path_append_with_prefix() {
    // `$a[b][][c] = 5` → $a['b'] gains an appended child `[c => 5]`.
    assert_eq!(
        run("<?php $a['b'][]['c'] = 5; echo $a['b'][0]['c'];"),
        "5"
    );
}

#[test]
fn double_mid_path_append() {
    // `$a[][] = 7` → `[[7]]`.
    assert_eq!(run("<?php $a[][] = 7; echo $a[0][0];"), "7");
}

#[test]
fn compound_add_on_nested_element() {
    assert_eq!(
        run("<?php $a['x']['y'] = 3; $a['x']['y'] += 10; echo $a['x']['y'];"),
        "13"
    );
}

#[test]
fn compound_add_autovivifies_from_null() {
    // A compound op on an unset deep element starts from 0 (matches PHP).
    assert_eq!(run("<?php $a['p']['q'] += 5; echo $a['p']['q'];"), "5");
}

#[test]
fn compound_concat_on_nested_element() {
    assert_eq!(
        run("<?php $a['s']['t'] = 'ab'; $a['s']['t'] .= 'cd'; echo $a['s']['t'];"),
        "abcd"
    );
}

#[test]
fn increment_on_nested_element() {
    // Increment of an unset element yields 1 (matches PHP).
    assert_eq!(run("<?php $a['n'][0]++; echo $a['n'][0];"), "1");
}

#[test]
fn post_increment_returns_old_value_on_element() {
    assert_eq!(
        run("<?php $a['i'] = 5; $old = $a['i']++; echo $old, ':', $a['i'];"),
        "5:6"
    );
}

#[test]
fn decrement_on_unset_element_is_scaffold_negative_one() {
    // SCAFFOLD DEVIATION: `--` on an unset element yields -1 here, consistent with
    // `b_incdec` on a plain `$var`. Real PHP leaves it null (decrement of null is a
    // no-op). This test pins the scaffold behavior, it does not claim PHP parity.
    assert_eq!(run("<?php $a[0]--; echo $a[0];"), "-1");
}

#[test]
fn compound_key_expression_evaluated_once() {
    // The key of a compound element assignment is evaluated exactly once: a
    // side-effecting key (post-increment) must not advance twice.
    assert_eq!(
        run("<?php $i = 0; $a = [10, 20]; $a[$i++] += 5; echo $a[0], ':', $a[1], ':', $i;"),
        "15:20:1"
    );
}

#[test]
fn deep_build_in_loop() {
    assert_eq!(
        run("<?php $g = []; for ($i = 0; $i < 3; $i++) { $g['row'][] = $i * $i; } echo implode(',', $g['row']);"),
        "0,1,4"
    );
}
