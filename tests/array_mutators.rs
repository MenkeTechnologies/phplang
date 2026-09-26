//! End-to-end tests for the by-reference array mutators: `array_push`,
//! `array_pop`, `array_shift`, `array_unshift`, `array_splice`. Each lowers
//! through `ops::ARR_MUT`, mutating the bound `$var` in place, so the tests
//! observe the effect through the variable after the call.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn push_returns_count_and_appends() {
    // array_push returns the new element count and appends in order.
    assert_eq!(
        run("<?php $a = [1, 2]; $n = array_push($a, 3, 4); echo $n, ':', implode(',', $a);"),
        "4:1,2,3,4"
    );
}

/// The array is a by-reference parameter that is READ, so an unset variable is
/// not auto-vivified: it binds as null and the reference refuses it. (This
/// test used to assert the vivification, which no PHP 8 does.)
#[test]
fn push_onto_an_unset_var_is_a_type_error() {
    assert_eq!(
        run("<?php try { array_push($a, 'x'); } catch (TypeError $e) { echo $e->getMessage(); }"),
        "array_push(): Argument #1 ($array) must be of type array, null given"
    );
}

#[test]
fn pop_returns_last_and_shrinks() {
    assert_eq!(
        run("<?php $a = [1, 2, 3]; $x = array_pop($a); echo $x, ':', implode(',', $a);"),
        "3:1,2"
    );
}

#[test]
fn pop_on_empty_returns_null() {
    // array_pop of an empty array is null (echoes as "").
    assert_eq!(
        run("<?php $a = []; $x = array_pop($a); echo '[', $x, ']';"),
        "[]"
    );
}

#[test]
fn pop_then_push_reuses_freed_index() {
    // After popping the top of a contiguous 0-based run, the next append reuses
    // the freed index (next_index reset to the popped key).
    assert_eq!(
        run("<?php $a = [10, 20, 30]; array_pop($a); array_push($a, 99); echo implode(',', $a);"),
        "10,20,99"
    );
}

#[test]
fn pop_on_sparse_array_keeps_next_index() {
    // PHP: popping the top of a gapped array does NOT rewind next_index to
    // max(remaining)+1. Here [5=>a, 10=>b]: pop removes 10, and the next append
    // continues at 11, not 6.
    assert_eq!(
        run("<?php $a = [5 => 'a', 10 => 'b']; array_pop($a); $a[] = 'c'; echo implode(',', $a);"),
        "a,c"
    );
    // PHP rewinds next_index because the popped key (10) was the top of the run,
    // so the next append reuses key 10 (verified against PHP 8.5.8).
    assert_eq!(
        run("<?php $a = [5 => 'a', 10 => 'b']; array_pop($a); $a[] = 'c'; $k = array_keys($a); echo $k[1];"),
        "10"
    );
}

#[test]
fn pop_single_nonzero_key_keeps_next_index() {
    // [3=>x]: pop removes key 3 (== next_index-1), so next append reuses key 3.
    assert_eq!(
        run("<?php $a = [3 => 'x']; array_pop($a); $a[] = 'y'; $k = array_keys($a); echo $k[0];"),
        "3"
    );
}

#[test]
fn shift_returns_first_and_reindexes() {
    // array_shift returns the first element and renumbers integer keys from 0.
    assert_eq!(
        run("<?php $a = [10, 20, 30]; $x = array_shift($a); echo $x, ':', $a[0], ',', $a[1];"),
        "10:20,30"
    );
}

#[test]
fn shift_preserves_string_keys() {
    // String keys survive a shift; only integer keys are renumbered (0,1 in
    // iteration order). Verified against PHP 8.5.8: ['b'=>3, 0=>2, 1=>4].
    assert_eq!(
        run("<?php $a = ['a' => 1, 0 => 2, 'b' => 3, 1 => 4]; array_shift($a); echo $a['b'], ':', $a[0], ':', $a[1];"),
        "3:2:4"
    );
}

#[test]
fn shift_on_empty_returns_null() {
    assert_eq!(
        run("<?php $a = []; $x = array_shift($a); echo '[', $x, ']';"),
        "[]"
    );
}

#[test]
fn unshift_prepends_and_returns_count() {
    // array_unshift prepends as a fresh 0-based run and returns the new count.
    assert_eq!(
        run("<?php $a = [3, 4]; $n = array_unshift($a, 1, 2); echo $n, ':', implode(',', $a);"),
        "4:1,2,3,4"
    );
}

#[test]
fn splice_removes_and_replaces_returning_removed() {
    // array_splice removes a range, splices a replacement, returns the removed
    // elements (reindexed), and reindexes the source.
    assert_eq!(
        run("<?php $a = [1, 2, 3, 4]; $out = array_splice($a, 1, 2, ['x', 'y', 'z']); echo implode(',', $a), '|', implode(',', $out);"),
        "1,x,y,z,4|2,3"
    );
}

#[test]
fn splice_negative_offset_and_default_length() {
    // A negative offset counts from the end; an omitted length runs to the end.
    assert_eq!(
        run("<?php $a = [1, 2, 3, 4, 5]; array_splice($a, -2); echo implode(',', $a);"),
        "1,2,3"
    );
}

/// Every library function that takes its array by reference to MUTATE it checks
/// that argument's type, as the reference does: an unset variable binds as null
/// WITHOUT an "Undefined variable" warning and is then refused, and so is a
/// string. Expectations are the reference's output for the same program.
#[test]
fn by_reference_array_arguments_are_type_checked_and_read_quietly() {
    let src = r#"<?php
        try { sort($u1); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
        try { ksort($u2); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
        try { shuffle($u3); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
        try { end($u4); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
        $s = "x";
        try { sort($s); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
        try { usort($n, fn($a, $b) => 0); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
        try { array_pop($u5); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
        $a = [2, 1]; sort($a); echo implode(",", $a);"#;
    assert_eq!(
        run(src),
        "sort(): Argument #1 ($array) must be of type array, null given\n\
         ksort(): Argument #1 ($array) must be of type array, null given\n\
         shuffle(): Argument #1 ($array) must be of type array, null given\n\
         end(): Argument #1 ($array) must be of type array, null given\n\
         sort(): Argument #1 ($array) must be of type array, string given\n\
         usort(): Argument #1 ($array) must be of type array, null given\n\
         array_pop(): Argument #1 ($array) must be of type array, null given\n\
         1,2"
    );
}
