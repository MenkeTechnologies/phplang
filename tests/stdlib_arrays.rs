//! End-to-end tests for the `arrays` stdlib category (`src/stdlib/arrays.rs`).
//! PHP source in, captured `echo` output out. Expected values were cross-checked
//! against PHP 8.5's reference `php` CLI. Arrays are reference handles here, so
//! the by-reference builtins (`usort`, `shuffle`, the internal-pointer family)
//! are observed through the caller's `$var` after the call.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

// ── array_column ─────────────────────────────────────────────────────────────

#[test]
fn array_column_basic() {
    assert_eq!(
        run("<?php $r=[['id'=>1,'name'=>'A'],['id'=>2,'name'=>'B']]; echo implode(',', array_column($r,'name'));"),
        "A,B"
    );
}

#[test]
fn array_column_with_index_key() {
    // Re-key the plucked column by another column.
    assert_eq!(
        run("<?php $r=[['id'=>7,'name'=>'A'],['id'=>9,'name'=>'B']]; $c=array_column($r,'name','id'); echo $c[7], $c[9];"),
        "AB"
    );
}

#[test]
fn array_column_null_column_keeps_row() {
    // A null column keeps the whole row, re-keyed by index_key.
    assert_eq!(
        run("<?php $r=[['id'=>7,'v'=>'x']]; $c=array_column($r,null,'id'); echo $c[7]['v'];"),
        "x"
    );
}

#[test]
fn array_column_skips_missing() {
    // Rows lacking the requested column are dropped.
    assert_eq!(
        run("<?php $r=[['name'=>'A'],['other'=>'Z'],['name'=>'B']]; echo implode(',', array_column($r,'name'));"),
        "A,B"
    );
}

// ── array_chunk ──────────────────────────────────────────────────────────────

#[test]
fn array_chunk_reindexes() {
    assert_eq!(
        run("<?php $c=array_chunk([1,2,3,4,5],2); echo count($c),'|',implode(',',$c[0]),'|',implode(',',$c[2]);"),
        "3|1,2|5"
    );
}

#[test]
fn array_chunk_preserve_keys() {
    assert_eq!(
        run("<?php $c=array_chunk(['a'=>1,'b'=>2,'c'=>3],2,true); echo implode(',',array_keys($c[0])),'|',implode(',',array_keys($c[1]));"),
        "a,b|c"
    );
}

#[test]
fn array_chunk_bad_size_errors() {
    assert!(eval_capture("<?php array_chunk([1,2],0);").is_err());
}

// ── array_fill_keys / array_pad ──────────────────────────────────────────────

#[test]
fn array_fill_keys_basic() {
    assert_eq!(
        run("<?php $a=array_fill_keys(['x','y'],0); echo $a['x'],$a['y'],'|',count($a);"),
        "00|2"
    );
}

#[test]
fn array_pad_right() {
    assert_eq!(
        run("<?php echo implode(',', array_pad([1,2,3],5,0));"),
        "1,2,3,0,0"
    );
}

#[test]
fn array_pad_left_reindexes() {
    assert_eq!(
        run("<?php echo implode(',', array_pad([1,2,3],-5,9));"),
        "9,9,1,2,3"
    );
}

#[test]
fn array_pad_no_pad_when_large_enough() {
    assert_eq!(
        run("<?php echo implode(',', array_pad([1,2,3],2,0));"),
        "1,2,3"
    );
}

// ── array_key_first / array_key_last / array_is_list ─────────────────────────

#[test]
fn array_key_first_last() {
    assert_eq!(
        run("<?php $a=['x'=>1,'y'=>2,'z'=>3]; echo array_key_first($a),'|',array_key_last($a);"),
        "x|z"
    );
}

#[test]
fn array_key_first_empty_is_null() {
    assert_eq!(run("<?php var_dump(array_key_first([]));"), "NULL\n");
}

#[test]
fn array_is_list_true_and_false() {
    assert_eq!(
        run("<?php var_dump(array_is_list([1,2,3])); var_dump(array_is_list([1=>'a',0=>'b']));"),
        "bool(true)\nbool(false)\n"
    );
}

// ── key/assoc diff & intersect ───────────────────────────────────────────────

#[test]
fn array_diff_key_removes_shared_keys() {
    assert_eq!(
        run("<?php $d=array_diff_key(['a'=>1,'b'=>2,'c'=>3],['a'=>9],['c'=>9]); echo implode(',',array_keys($d)),'=',implode(',',$d);"),
        "b=2"
    );
}

#[test]
fn array_intersect_key_keeps_shared_keys() {
    assert_eq!(
        run("<?php $d=array_intersect_key(['a'=>1,'b'=>2,'c'=>3],['a'=>9,'c'=>9]); echo implode(',',array_keys($d));"),
        "a,c"
    );
}

#[test]
fn array_diff_assoc_checks_key_and_value() {
    assert_eq!(
        run("<?php $d=array_diff_assoc(['a'=>1,'b'=>2,'c'=>3],['a'=>1,'b'=>9]); echo implode(',',array_keys($d));"),
        "b,c"
    );
}

#[test]
fn array_intersect_assoc_checks_key_and_value() {
    assert_eq!(
        run("<?php $d=array_intersect_assoc(['a'=>1,'b'=>2,'c'=>3],['a'=>1,'b'=>9]); echo implode(',',array_keys($d)),'=',implode(',',$d);"),
        "a=1"
    );
}

// ── array_merge_recursive / array_replace ────────────────────────────────────

#[test]
fn merge_recursive_merges_subarrays() {
    assert_eq!(
        run("<?php $m=array_merge_recursive(['a'=>[1]],['a'=>[3]]); echo implode(',',$m['a']);"),
        "1,3"
    );
}

#[test]
fn merge_recursive_wraps_scalars() {
    assert_eq!(
        run("<?php $m=array_merge_recursive(['a'=>1],['a'=>2],['a'=>3]); echo implode(',',$m['a']);"),
        "1,2,3"
    );
}

#[test]
fn merge_recursive_scalar_and_array() {
    assert_eq!(
        run("<?php $m=array_merge_recursive(['a'=>1],['a'=>[2,3]]); echo implode(',',$m['a']);"),
        "1,2,3"
    );
}

#[test]
fn merge_recursive_does_not_mutate_input() {
    // The source subarray must be left intact (deep copy).
    assert_eq!(
        run(
            "<?php $x=['a'=>[1]]; array_merge_recursive($x,['a'=>[2]]); echo implode(',',$x['a']);"
        ),
        "1"
    );
}

#[test]
fn array_replace_overwrites_by_key() {
    assert_eq!(
        run("<?php $r=array_replace(['a'=>1,'b'=>2],['b'=>9,'c'=>3]); echo $r['a'],$r['b'],$r['c'];"),
        "193"
    );
}

// ── array_count_values ───────────────────────────────────────────────────────

#[test]
fn count_values_tallies() {
    // Integer 1 and string "1" fold to the same key (PHP key normalization).
    assert_eq!(
        run("<?php $c=array_count_values([1,'1',1,'hi','hi']); echo $c[1],'|',$c['hi'];"),
        "3|2"
    );
}

// ── usort / uasort / uksort ──────────────────────────────────────────────────

#[test]
fn usort_reindexes() {
    assert_eq!(
        run("<?php $a=[3,1,2]; usort($a, fn($x,$y)=>$x-$y); echo implode(',',$a);"),
        "1,2,3"
    );
}

#[test]
fn uasort_preserves_keys() {
    assert_eq!(
        run("<?php $a=['b'=>2,'a'=>1,'c'=>3]; uasort($a, fn($x,$y)=>$x-$y); echo implode(',',array_keys($a));"),
        "a,b,c"
    );
}

#[test]
fn uksort_sorts_by_key() {
    assert_eq!(
        run("<?php $a=['b'=>1,'a'=>2,'c'=>3]; uksort($a, fn($x,$y)=>strcmp($x,$y)); echo implode(',',array_keys($a));"),
        "a,b,c"
    );
}

#[test]
fn usort_named_callback() {
    assert_eq!(
        run("<?php function cmp($x,$y){return $y-$x;} $a=[1,3,2]; usort($a,'cmp'); echo implode(',',$a);"),
        "3,2,1"
    );
}

// ── natsort / natcasesort ────────────────────────────────────────────────────

#[test]
fn natsort_natural_order() {
    assert_eq!(
        run("<?php $a=['img10','img2','img1']; natsort($a); echo implode(',',$a);"),
        "img1,img2,img10"
    );
}

#[test]
fn natsort_preserves_keys() {
    assert_eq!(
        run("<?php $a=['img10','img2','img1']; natsort($a); echo implode(',',array_keys($a));"),
        "2,1,0"
    );
}

#[test]
fn natcasesort_ignores_case() {
    assert_eq!(
        run("<?php $a=['IMG10','img2','Img1']; natcasesort($a); echo implode(',',$a);"),
        "Img1,img2,IMG10"
    );
}

#[test]
fn natsort_leading_zero_run_orders_by_original_chars() {
    // Regression: PHP strnatcmp("a010","a10") == -1. A digit run with a leading
    // zero compares left-aligned char-by-char ('0' < '1'), so "a010" sorts before
    // "a10" instead of folding both to the value 10 and comparing Equal.
    assert_eq!(
        run("<?php $a=['a10','a010']; natsort($a); echo implode(',',$a);"),
        "a010,a10"
    );
}

#[test]
fn natsort_leading_zero_vs_more_digits() {
    // "007" (leading zero, fractional compare) vs "10" (no leading zero). Element
    // 0: '0' < '1' under the left-aligned rule, so "007" precedes "10".
    assert_eq!(
        run("<?php $a=['10','007','08']; natsort($a); echo implode(',',$a);"),
        "007,08,10"
    );
}

// ── shuffle / array_rand (membership/count only) ─────────────────────────────

#[test]
fn shuffle_preserves_membership() {
    // Order is random, but the multiset of values and the count are stable.
    assert_eq!(
        run("<?php $a=[1,2,3,4,5]; shuffle($a); sort($a); echo implode(',',$a),'|',count($a);"),
        "1,2,3,4,5|5"
    );
}

#[test]
fn shuffle_returns_true_and_makes_list() {
    assert_eq!(
        run(
            "<?php $a=['x'=>1,'y'=>2]; var_dump(shuffle($a)); echo array_is_list($a)?'list':'map';"
        ),
        "bool(true)\nlist"
    );
}

#[test]
fn array_rand_single_is_valid_key() {
    // A single pick returns one of the keys; verify it indexes a real element.
    assert_eq!(
        run("<?php $a=['a','b','c']; $k=array_rand($a); echo ($k>=0 && $k<3)?'ok':'bad';"),
        "ok"
    );
}

#[test]
fn array_rand_multiple_distinct_keys() {
    assert_eq!(
        run("<?php $a=[10,20,30,40]; $k=array_rand($a,3); echo count($k),'|',(count(array_unique($k))==3)?'distinct':'dup';"),
        "3|distinct"
    );
}

#[test]
fn array_rand_out_of_range_errors() {
    assert!(eval_capture("<?php array_rand([1,2],5);").is_err());
}

// ── array_walk ───────────────────────────────────────────────────────────────

#[test]
fn array_walk_visits_each() {
    assert_eq!(
        run("<?php $a=['a'=>1,'b'=>2]; array_walk($a, function($v,$k){ echo $k,':',$v,';'; });"),
        "a:1;b:2;"
    );
}

#[test]
fn array_walk_passes_extra() {
    assert_eq!(
        run("<?php $a=[1,2]; array_walk($a, function($v,$k,$e){ echo $v+$e,';'; }, 10);"),
        "11;12;"
    );
}

#[test]
fn array_walk_scalar_mutation_does_not_propagate() {
    // Documents the VM limitation: phplang has no by-reference parameters, so a
    // scalar reassigned inside the callback does NOT write back into the array
    // (PHP's `function(&$v, $k)` could). The array is left untouched.
    assert_eq!(
        run("<?php $a=[1,2,3]; array_walk($a, function($v,$k){ $v = $v * 10; }); echo implode(',',$a);"),
        "1,2,3"
    );
}

// ── compact / extract ────────────────────────────────────────────────────────

#[test]
fn compact_gathers_bound_vars() {
    assert_eq!(
        run("<?php $name='Bob'; $age=30; $a=compact('name','age','missing'); echo $a['name'],':',$a['age'],'|',count($a);"),
        "Bob:30|2"
    );
}

#[test]
fn compact_nested_names() {
    assert_eq!(
        run("<?php $x=1; $y=2; $a=compact(['x',['y']]); echo $a['x'],$a['y'];"),
        "12"
    );
}

#[test]
fn extract_binds_string_keys() {
    assert_eq!(
        run(
            "<?php $n=extract(['city'=>'NYC','zip'=>10001,5=>'skip']); echo $n,'|',$city,'|',$zip;"
        ),
        "2|NYC|10001"
    );
}

// ── internal pointer (end/reset/current/key/next/prev) ───────────────────────

#[test]
fn pointer_current_starts_at_first() {
    assert_eq!(run("<?php $a=[10,20,30]; echo current($a);"), "10");
}

#[test]
fn pointer_next_and_key() {
    assert_eq!(
        run("<?php $a=['x'=>1,'y'=>2,'z'=>3]; echo current($a),next($a),key($a);"),
        "12y"
    );
}

#[test]
fn pointer_end_and_prev() {
    assert_eq!(run("<?php $a=[10,20,30]; echo end($a),prev($a);"), "3020");
}

#[test]
fn pointer_reset_reanchors() {
    assert_eq!(
        run("<?php $a=[10,20,30]; next($a); next($a); echo current($a),'|',reset($a);"),
        "30|10"
    );
}

#[test]
fn pointer_off_end_is_sticky() {
    // Past the end, current is false and prev stays invalid (does not recover).
    assert_eq!(
        run("<?php $a=[1,2]; next($a); next($a); var_dump(current($a)); var_dump(prev($a));"),
        "bool(false)\nbool(false)\n"
    );
}

#[test]
fn pointer_empty_array() {
    assert_eq!(
        run("<?php $a=[]; var_dump(current($a)); var_dump(key($a));"),
        "bool(false)\nNULL\n"
    );
}

#[test]
fn pointer_append_keeps_cursor() {
    // Regression: appending to the array must NOT rewind the internal pointer.
    // PHP keeps the cursor where it was across `$a[] = x`; here the cursor sits on
    // index 1 (value 20) and stays there after the push, so current() is 20.
    assert_eq!(
        run("<?php $a=[10,20,30]; next($a); $a[]=40; echo current($a);"),
        "20"
    );
}

#[test]
fn pointer_no_cross_program_leak() {
    // A fresh array reusing the same low handle must start at its first element,
    // not inherit a cursor from an earlier run (fingerprint guard).
    run("<?php $a=[7,8,9]; end($a);");
    assert_eq!(run("<?php $b=[100,200]; echo current($b);"), "100");
}

// ── array_walk by reference ─────────────────────────────────────────────────

#[test]
fn array_walk_by_reference_callback_writes_back_scalars() {
    // The element travels through a reference cell, so `&$v` mutates the array
    // even for scalars. The key is still passed by value.
    assert_eq!(
        run("<?php $a=[1,2,3]; array_walk($a, function(&$v,$k){ $v = $v*10 + $k; }); echo implode(',',$a);"),
        "10,21,32"
    );
    // Keys are preserved, not reindexed.
    assert_eq!(
        run("<?php $b=['x'=>1,'y'=>2]; array_walk($b, function(&$v){ $v++; }); echo json_encode($b);"),
        r#"{"x":2,"y":3}"#
    );
}

/// `count($a, COUNT_RECURSIVE)` counts every element at every depth: a nested
/// array is one element AND contributes its own contents.
///
/// Byte-parity with `php` 8.5.9. The `$mode` argument used to be read by
/// nothing, so `COUNT_RECURSIVE` answered the top-level count — the divergence
/// was recorded in the corpus as `// => 2 (PHP 8 prints: 4)`.
#[test]
fn count_recursive_counts_every_depth() {
    assert_eq!(run("<?php echo count([1, [2, 3]], COUNT_RECURSIVE);"), "4");
    assert_eq!(
        run("<?php echo count([1, [2, [3, 4]]], COUNT_RECURSIVE);"),
        "6"
    );
    // An empty nested array still counts as the one element it is.
    assert_eq!(run("<?php echo count([[], []], COUNT_RECURSIVE);"), "2");
    assert_eq!(run("<?php echo count([], COUNT_RECURSIVE);"), "0");
    // COUNT_NORMAL is the default and stays the top-level count.
    assert_eq!(run("<?php echo count([1, [2, 3]], COUNT_NORMAL);"), "2");
    assert_eq!(run("<?php echo count([1, [2, 3]]);"), "2");
}

#[test]
fn count_recursive_does_not_descend_into_objects() {
    // A nested `Countable` is ONE element; its own count() is not consulted,
    // so a class reporting 5 must not add 5 here.
    assert_eq!(
        run(
            "<?php class C implements Countable { public function count(): int { return 5; } }
            echo count([new C], COUNT_RECURSIVE);"
        ),
        "1"
    );
    // A plain object is likewise a single element, whatever its properties.
    assert_eq!(
        run(r#"<?php $o = (object)["a" => 1]; echo count([$o, [1]], COUNT_RECURSIVE);"#),
        "3"
    );
}

#[test]
fn count_rejects_a_mode_that_is_neither_normal_nor_recursive() {
    assert_eq!(
        run(r#"<?php try { echo count([1], 99); } catch (Throwable $e) {
            echo get_class($e), "|", $e->getMessage(); }"#),
        "ValueError|count(): Argument #2 ($mode) must be either COUNT_NORMAL or COUNT_RECURSIVE"
    );
    assert_eq!(
        run(r#"<?php try { echo count([1], -1); } catch (Throwable $e) {
            echo get_class($e), "|", $e->getMessage(); }"#),
        "ValueError|count(): Argument #2 ($mode) must be either COUNT_NORMAL or COUNT_RECURSIVE"
    );
}
