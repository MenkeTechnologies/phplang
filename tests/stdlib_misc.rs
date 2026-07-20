//! End-to-end tests for the `misc` stdlib category (`src/stdlib/misc.rs`). PHP
//! source in, captured `echo` output out. Expected values were cross-checked
//! against PHP 8.5's reference `php` CLI.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

// ── strnatcmp / strnatcasecmp ────────────────────────────────────────────────

#[test]
fn strnatcmp_leading_zero_orders_before() {
    // PHP's fractional comparison: "a010" sorts before "a10".
    assert_eq!(run("<?php echo strnatcmp('a010','a10');"), "-1");
}

#[test]
fn strnatcmp_equal() {
    assert_eq!(run("<?php echo strnatcmp('a10','a10');"), "0");
}

#[test]
fn strnatcmp_numeric_magnitude() {
    // "img12" > "img10", and 10 > 9 by magnitude (not lexical).
    assert_eq!(run("<?php echo strnatcmp('img12','img10');"), "1");
    assert_eq!(run("<?php echo strnatcmp('10','9');"), "1");
}

#[test]
fn strnatcmp_sorts_naturally() {
    assert_eq!(
        run("<?php $a=['img10','img12','img2','img1']; usort($a,'strnatcmp'); echo implode(',',$a);"),
        "img1,img2,img10,img12"
    );
}

#[test]
fn strnatcasecmp_folds_case() {
    // Case-insensitive: "IMG10" and "img10" compare equal.
    assert_eq!(run("<?php echo strnatcasecmp('IMG10','img10');"), "0");
    assert_eq!(run("<?php echo strnatcmp('IMG10','img10');"), "-1");
}

// ── soundex ──────────────────────────────────────────────────────────────────

#[test]
fn soundex_classic_names() {
    assert_eq!(run("<?php echo soundex('Robert');"), "R163");
    assert_eq!(run("<?php echo soundex('Rupert');"), "R163");
    assert_eq!(run("<?php echo soundex('Euler');"), "E460");
    assert_eq!(run("<?php echo soundex('Gauss');"), "G200");
}

#[test]
fn soundex_repeated_and_vowels() {
    // Vowel between same-code consonants re-codes; PHP -> A226 for Ashcraft.
    assert_eq!(run("<?php echo soundex('Ashcraft');"), "A226");
    assert_eq!(run("<?php echo soundex('Tymczak');"), "T522");
}

#[test]
fn soundex_no_letters_pads_zeros() {
    // PHP 8.5 pads to four zeros for empty / letterless input.
    assert_eq!(run("<?php echo soundex('123');"), "0000");
    assert_eq!(run("<?php echo soundex('');"), "0000");
}

// ── str_getcsv ───────────────────────────────────────────────────────────────

#[test]
fn str_getcsv_plain_fields() {
    assert_eq!(
        run("<?php echo implode('|', str_getcsv('a,b,c'));"),
        "a|b|c"
    );
}

#[test]
fn str_getcsv_enclosed_field_with_separator() {
    // A comma inside the enclosure is part of the field, not a delimiter.
    assert_eq!(
        run("<?php $r=str_getcsv('\"a,b\",c'); echo $r[0],'/',$r[1];"),
        "a,b/c"
    );
}

#[test]
fn str_getcsv_doubled_enclosure_is_literal() {
    // "" inside an enclosure collapses to a single literal quote.
    assert_eq!(
        run("<?php $r=str_getcsv('\"a\"\"b\",c'); echo $r[0],'/',$r[1];"),
        "a\"b/c"
    );
}

#[test]
fn str_getcsv_custom_separator() {
    assert_eq!(
        run("<?php echo implode('|', str_getcsv('a;b;c', ';'));"),
        "a|b|c"
    );
}

#[test]
fn str_getcsv_empty_line_is_single_null() {
    // A wholly empty line yields one null field.
    assert_eq!(run("<?php $r=str_getcsv(''); echo count($r),':',var_export($r[0],true);"), "1:NULL");
}

#[test]
fn str_getcsv_empty_fields_are_empty_strings() {
    // A bare separator yields two empty-string fields (not null).
    assert_eq!(run("<?php $r=str_getcsv(','); echo count($r),':[',$r[0],'][',$r[1],']';"), "2:[][]");
}

// ── array_walk_recursive ─────────────────────────────────────────────────────

#[test]
fn array_walk_recursive_visits_all_leaves() {
    // phplang has no by-reference closure capture, so accumulate through an
    // object handle passed as the (by-value, but shared-handle) extra argument.
    assert_eq!(
        run(
            "<?php class Acc { public $s = 0; } $acc = new Acc(); $a=[1,[2,3],[4,[5,6]]]; \
             array_walk_recursive($a, function($v,$k,$acc){ $acc->s += $v; }, $acc); echo $acc->s;"
        ),
        "21"
    );
}

#[test]
fn array_walk_recursive_returns_true() {
    assert_eq!(
        run("<?php echo var_export(array_walk_recursive([1,[2]], function($v){}), true);"),
        "true"
    );
}

#[test]
fn array_walk_recursive_receives_keys() {
    assert_eq!(
        run(
            "<?php class Buf { public $out = ''; } $b = new Buf(); $a=['x'=>['y'=>5]]; \
             array_walk_recursive($a, function($v,$k,$b){ $b->out .= \"$k=$v\"; }, $b); echo $b->out;"
        ),
        "y=5"
    );
}

// ── array_find / array_find_key ──────────────────────────────────────────────

#[test]
fn array_find_returns_first_match() {
    assert_eq!(run("<?php echo array_find([1,2,3,4], fn($v)=>$v>2);"), "3");
}

#[test]
fn array_find_no_match_is_null() {
    assert_eq!(
        run("<?php echo var_export(array_find([1,2], fn($v)=>$v>5), true);"),
        "NULL"
    );
}

#[test]
fn array_find_callback_gets_key() {
    assert_eq!(
        run("<?php echo array_find(['a'=>1,'b'=>5,'c'=>3], fn($v,$k)=>$k==='b');"),
        "5"
    );
}

#[test]
fn array_find_key_returns_key() {
    assert_eq!(
        run("<?php echo array_find_key([1,2,3,4], fn($v)=>$v>2);"),
        "2"
    );
}

// ── array_any / array_all ────────────────────────────────────────────────────

#[test]
fn array_any_short_circuits() {
    assert_eq!(
        run("<?php echo var_export(array_any([1,2,3], fn($v)=>$v>2), true);"),
        "true"
    );
    assert_eq!(
        run("<?php echo var_export(array_any([1,2,3], fn($v)=>$v>9), true);"),
        "false"
    );
}

#[test]
fn array_all_requires_every_element() {
    assert_eq!(
        run("<?php echo var_export(array_all([2,4,6], fn($v)=>$v%2===0), true);"),
        "true"
    );
    assert_eq!(
        run("<?php echo var_export(array_all([2,4,5], fn($v)=>$v%2===0), true);"),
        "false"
    );
}

#[test]
fn array_any_all_empty_edge_cases() {
    // Vacuous: array_all([]) is true, array_any([]) is false.
    assert_eq!(
        run("<?php echo var_export(array_all([], fn($v)=>false), true);"),
        "true"
    );
    assert_eq!(
        run("<?php echo var_export(array_any([], fn($v)=>true), true);"),
        "false"
    );
}
