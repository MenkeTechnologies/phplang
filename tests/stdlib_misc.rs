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

/// PHP 8.4 deprecated relying on `str_getcsv`'s default `$escape`, so every call
/// below that omits it is PREFIXED by this notice — the reference emits it
/// before any parsing, even for an empty subject. Each test therefore asserts
/// the notice AND the unchanged parse result, and pairs the call with an
/// explicit-`$escape` form that proves the notice is the only difference.
const CSV_DEPRECATED: &str = "\nDeprecated: str_getcsv(): the $escape parameter must be provided as its default value will change in Command line code on line 1\n";

#[test]
fn str_getcsv_plain_fields() {
    assert_eq!(
        run("<?php echo implode('|', str_getcsv('a,b,c'));"),
        format!("{CSV_DEPRECATED}a|b|c")
    );
    assert_eq!(
        run("<?php echo implode('|', str_getcsv('a,b,c', ',', '\"', '\\\\'));"),
        "a|b|c"
    );
}

#[test]
fn str_getcsv_enclosed_field_with_separator() {
    // A comma inside the enclosure is part of the field, not a delimiter.
    assert_eq!(
        run("<?php $r=str_getcsv('\"a,b\",c'); echo $r[0],'/',$r[1];"),
        format!("{CSV_DEPRECATED}a,b/c")
    );
}

#[test]
fn str_getcsv_doubled_enclosure_is_literal() {
    // "" inside an enclosure collapses to a single literal quote.
    assert_eq!(
        run("<?php $r=str_getcsv('\"a\"\"b\",c'); echo $r[0],'/',$r[1];"),
        format!("{CSV_DEPRECATED}a\"b/c")
    );
}

#[test]
fn str_getcsv_custom_separator() {
    // A custom separator does not satisfy the deprecation: only $escape does.
    assert_eq!(
        run("<?php echo implode('|', str_getcsv('a;b;c', ';'));"),
        format!("{CSV_DEPRECATED}a|b|c")
    );
    assert_eq!(
        run("<?php echo implode('|', str_getcsv('a;b;c', ';', '\"', '\\\\'));"),
        "a|b|c"
    );
}

#[test]
fn str_getcsv_empty_line_is_single_null() {
    // A wholly empty line yields one null field — and still raises the notice,
    // which fires on the argument count before any parsing.
    assert_eq!(
        run("<?php $r=str_getcsv(''); echo count($r),':',var_export($r[0],true);"),
        format!("{CSV_DEPRECATED}1:NULL")
    );
}

#[test]
fn str_getcsv_empty_fields_are_empty_strings() {
    // A bare separator yields two empty-string fields (not null).
    assert_eq!(
        run("<?php $r=str_getcsv(','); echo count($r),':[',$r[0],'][',$r[1],']';"),
        format!("{CSV_DEPRECATED}2:[][]")
    );
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
    // The array has to be a VARIABLE. This test used to pass the literal
    // `[1,[2]]` directly and expect `true`, which no PHP returns: an array
    // literal cannot supply the by-reference binding the first parameter needs,
    // so `php -r` answers the same snippet with
    // `Error: array_walk_recursive(): Argument #1 ($array) could not be passed
    // by reference`. The return value is what this test is about, so it now
    // asks for it through an argument the reference accepts.
    assert_eq!(
        run("<?php $a=[1,[2]]; echo var_export(array_walk_recursive($a, function($v){}), true);"),
        "true"
    );
}

#[test]
fn array_walk_recursive_rejects_an_array_literal() {
    // The behaviour the old form of the test above was actually exercising.
    let err = eval_capture(
        "<?php echo var_export(array_walk_recursive([1,[2]], function($v){}), true);",
    )
    .unwrap_err();
    assert!(
        err.contains(
            "array_walk_recursive(): Argument #1 ($array) could not be passed by reference"
        ),
        "unexpected error text: {err}"
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

// ── strnatcmp: PHP 8.5 regression fixes ──────────────────────────────────────
// The prior implementation used the older Martin Pool variant and diverged from
// PHP 8.5 on these inputs (all cross-checked against the `php` 8.5 CLI).

#[test]
fn strnatcmp_leading_zeros_stripped_once() {
    // PHP strips leading zeros once at the start, so bare zero-runs are equal.
    assert_eq!(run("<?php echo strnatcmp('0','00');"), "0");
    assert_eq!(run("<?php echo strnatcmp('1','01');"), "0");
    assert_eq!(run("<?php echo strnatcmp('00','000');"), "0");
}

#[test]
fn strnatcmp_trailing_char_makes_longer_greater() {
    // A trailing character that has nothing to compare against wins.
    assert_eq!(run("<?php echo strnatcmp('a ','a');"), "1");
    assert_eq!(run("<?php echo strnatcmp('a0','a');"), "1");
    assert_eq!(run("<?php echo strnatcmp('a','ab');"), "-1");
}

#[test]
fn strnatcmp_leading_whitespace_skipped() {
    // Leading whitespace is skipped, so " a" and "a" compare equal.
    assert_eq!(run("<?php echo strnatcmp(' a','a');"), "0");
    assert_eq!(run("<?php echo strnatcmp('10',' 10');"), "0");
}

#[test]
fn strnatcmp_fractional_vs_magnitude() {
    // Leading-zero run → fractional (left-aligned) comparison.
    assert_eq!(run("<?php echo strnatcmp('a01','a1');"), "-1");
    assert_eq!(run("<?php echo strnatcmp('a1','a01');"), "1");
    // No leading zero → magnitude comparison.
    assert_eq!(run("<?php echo strnatcmp('100','20');"), "1");
}

// ── str_word_count ───────────────────────────────────────────────────────────
// NOTE: the `$format`/`$characters` modes of the misc `str_word_count` are
// currently shadowed by a core `builtins::call_library` stub (a whitespace-token
// count) that this module is not permitted to edit, so only the count form is
// reachable end-to-end. See the SHADOWED note on `str_word_count` in
// `src/stdlib/misc.rs` for the full explanation.

#[test]
fn str_word_count_counts_words() {
    assert_eq!(
        run("<?php echo str_word_count('Hello world foo bar');"),
        "4"
    );
    assert_eq!(run("<?php echo str_word_count('');"), "0");
}

// ── metaphone ────────────────────────────────────────────────────────────────

#[test]
fn metaphone_classic_words() {
    assert_eq!(run("<?php echo metaphone('Thompson');"), "0MPSN");
    assert_eq!(run("<?php echo metaphone('phone');"), "FN");
    assert_eq!(run("<?php echo metaphone('Xavier');"), "SFR");
    assert_eq!(run("<?php echo metaphone('Wikipedia');"), "WKPT");
}

#[test]
fn metaphone_digraphs_and_silent() {
    // CH→X (sh), SCH→SX, GH silent/→F, GN→N, KN→N.
    assert_eq!(run("<?php echo metaphone('school');"), "SXL");
    assert_eq!(run("<?php echo metaphone('Christ');"), "XRST");
    assert_eq!(run("<?php echo metaphone('knight');"), "NFT");
    assert_eq!(run("<?php echo metaphone('gnome');"), "NM");
}

#[test]
fn metaphone_phoneme_limit_and_empty() {
    // The 2nd arg caps the number of phonemes; letterless input yields "".
    assert_eq!(run("<?php echo metaphone('Thompson', 4);"), "0MPS");
    assert_eq!(run("<?php echo var_export(metaphone('123'), true);"), "''");
}

// ── uniqid ───────────────────────────────────────────────────────────────────

#[test]
fn uniqid_default_is_13_hex_chars() {
    assert_eq!(run("<?php echo strlen(uniqid());"), "13");
    // The 13 chars are all hexadecimal.
    assert_eq!(
        run("<?php echo (ctype_xdigit(uniqid()) ? 'hex' : 'no');"),
        "hex"
    );
}

#[test]
fn uniqid_prefix_prepended() {
    assert_eq!(
        run("<?php $u=uniqid('pre_'); echo substr($u,0,4),':',strlen($u);"),
        "pre_:17"
    );
}

#[test]
fn uniqid_more_entropy_appends_fraction() {
    // more_entropy adds a '.' and 8 fractional digits (total length 23).
    assert_eq!(
        run("<?php $u=uniqid('', true); echo strlen($u),':',(strpos($u,'.')!==false?'dot':'no');"),
        "23:dot"
    );
}

// ── array_udiff / array_uintersect ───────────────────────────────────────────

#[test]
fn array_udiff_keeps_unmatched_and_preserves_keys() {
    assert_eq!(
        run("<?php $r=array_udiff([1,2,3,4],[2,4], fn($a,$b)=>$a<=>$b); echo implode(',', array_keys($r)),'|',implode(',',$r);"),
        "0,2|1,3"
    );
}

#[test]
fn array_uintersect_keeps_common() {
    assert_eq!(
        run("<?php $r=array_uintersect([1,2,3,4],[2,4,5], fn($a,$b)=>$a<=>$b); echo implode(',', array_keys($r)),'|',implode(',',$r);"),
        "1,3|2,4"
    );
}

#[test]
fn array_uintersect_three_arrays_requires_all() {
    // 2 is in both others, 4 only in the second — only 2 survives.
    assert_eq!(
        run(
            "<?php echo implode(',', array_uintersect([1,2,3,4],[2,4],[2,9], fn($a,$b)=>$a<=>$b));"
        ),
        "2"
    );
}

// ── array_diff_ukey / array_intersect_ukey ───────────────────────────────────

#[test]
fn array_diff_ukey_compares_keys() {
    assert_eq!(
        run("<?php $r=array_diff_ukey(['a'=>1,'b'=>2,'c'=>3],['b'=>9], fn($a,$b)=>strcmp($a,$b)); echo implode(',', array_keys($r)),'|',implode(',',$r);"),
        "a,c|1,3"
    );
}

#[test]
fn array_intersect_ukey_compares_keys() {
    assert_eq!(
        run("<?php $r=array_intersect_ukey(['a'=>1,'b'=>2,'c'=>3],['b'=>9,'c'=>8], fn($a,$b)=>strcmp($a,$b)); echo implode(',', array_keys($r)),'|',implode(',',$r);"),
        "b,c|2,3"
    );
}

// ── array_multisort ──────────────────────────────────────────────────────────

#[test]
fn array_multisort_single_array_ascending() {
    assert_eq!(
        run("<?php $a=[3,1,2]; array_multisort($a); echo implode(',',$a);"),
        "1,2,3"
    );
}

#[test]
fn array_multisort_descending_flag() {
    assert_eq!(
        run("<?php $a=[3,1,2]; array_multisort($a, SORT_DESC); echo implode(',',$a);"),
        "3,2,1"
    );
}

#[test]
fn array_multisort_parallel_columns() {
    // The second array is reordered by the first array's sort permutation.
    assert_eq!(
        run("<?php $a=[3,1,2]; $b=['c','a','b']; array_multisort($a,$b); echo implode(',',$a),'|',implode(',',$b);"),
        "1,2,3|a,b,c"
    );
}

#[test]
fn array_multisort_ties_broken_by_second_column() {
    // Equal primary keys → the second column decides order.
    assert_eq!(
        run("<?php $a=[1,1,2]; $b=[3,1,2]; array_multisort($a,$b); echo implode(',',$a),'|',implode(',',$b);"),
        "1,1,2|1,3,2"
    );
}
