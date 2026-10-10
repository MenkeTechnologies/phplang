//! End-to-end tests for the arity and named-parameter checks PHP 8 runs against
//! a BUILTIN before it sees its arguments — the table in `src/argsig.rs`.
//!
//! Every expectation here is the byte output of
//! `/opt/homebrew/Cellar/php/8.5.10/bin/php -d xdebug.mode=off` on the same
//! source. The four shapes worth pinning are the count refusal, the name that
//! binds out of order, the slot a name jumped over, and the order the checks run
//! in relative to each other and to the type check.

use phplang::eval_capture;

/// Run `src` and return what it printed, with any refusal caught and rendered as
/// `Class: message` the way the reference's `getMessage` reads.
fn caught(expr: &str) -> String {
    let src = format!(
        "<?php $x = [3, 1];\n\
         try {{ $r = {expr}; echo \"OK \"; var_export($r); }}\n\
         catch (Throwable $e) {{ echo get_class($e), \": \", $e->getMessage(); }}"
    );
    eval_capture(&src).unwrap_or_else(|e| panic!("eval error for {expr:?}: {e}"))
}

#[test]
fn too_many_arguments_is_refused() {
    // Every parameter required, so the reference says "exactly".
    assert_eq!(
        caught(r#"strtolower("a", "b")"#),
        "ArgumentCountError: strtolower() expects exactly 1 argument, 2 given"
    );
    // Some optional, so it says "at most" — and the count is the DECLARED
    // maximum, not the number required.
    assert_eq!(
        caught("count([1], 1, 2)"),
        "ArgumentCountError: count() expects at most 2 arguments, 3 given"
    );
}

#[test]
fn too_few_arguments_is_refused() {
    assert_eq!(
        caught("str_repeat(\"a\")"),
        "ArgumentCountError: str_repeat() expects exactly 2 arguments, 1 given"
    );
    assert_eq!(
        caught("array_slice([1, 2])"),
        "ArgumentCountError: array_slice() expects at least 2 arguments, 1 given"
    );
    // The singular is spelled "argument".
    assert_eq!(
        caught("implode()"),
        "ArgumentCountError: implode() expects at least 1 argument, 0 given"
    );
}

#[test]
fn argument_count_is_checked_before_argument_types() {
    // The array in #1 would be a TypeError on its own; the count wins.
    assert_eq!(
        caught(r#"strtolower([1], "x")"#),
        "ArgumentCountError: strtolower() expects exactly 1 argument, 2 given"
    );
}

#[test]
fn named_arguments_bind_by_name_not_by_position() {
    // Reversed: binding positionally would put the int in $string.
    assert_eq!(caught(r#"str_repeat(times: 3, string: "a")"#), "OK 'aaa'");
    assert_eq!(
        caught(r#"array_search(needle: 1, haystack: [1, 2])"#),
        "OK 0"
    );
}

#[test]
fn unknown_named_parameter_is_refused() {
    assert_eq!(
        caught(r#"strtolower(a: "AB")"#),
        "Error: Unknown named parameter $a"
    );
    // The unknown name wins over the count, which is also wrong here.
    assert_eq!(
        caught(r#"strtolower("a", "b", c: 1)"#),
        "Error: Unknown named parameter $c"
    );
}

#[test]
fn named_parameter_may_not_overwrite_a_positional() {
    assert_eq!(
        caught(r#"str_repeat("a", 3, times: 3)"#),
        "Error: Named parameter $times overwrites previous argument"
    );
}

#[test]
fn a_slot_a_name_jumped_over_takes_its_default() {
    // $length is skipped and defaults to null, $preserve_keys is named.
    assert_eq!(
        caught("array_slice([1, 2, 3], 1, preserve_keys: true)"),
        "OK array (\n  1 => 2,\n  2 => 3,\n)"
    );
    // $pad_string is skipped and defaults to a single space.
    assert_eq!(
        caught(r#"str_pad("a", 5, pad_type: STR_PAD_LEFT)"#),
        "OK '    a'"
    );
}

#[test]
fn a_required_slot_a_name_jumped_over_is_refused() {
    assert_eq!(
        caught("array_slice(offset: 1)"),
        "ArgumentCountError: array_slice(): Argument #1 ($array) not passed"
    );
    assert_eq!(
        caught("implode(array: [1, 2])"),
        "ArgumentCountError: implode(): Argument #1 ($separator) not passed"
    );
}

#[test]
fn a_skipped_slot_with_no_published_default_is_refused() {
    assert_eq!(
        caught("mt_rand(max: 5)"),
        "ArgumentCountError: mt_rand(): Argument #1 ($min) must be passed explicitly, \
         because the default value is not known"
    );
    assert_eq!(
        caught("array_keys([1, 2], strict: true)"),
        "ArgumentCountError: array_keys(): Argument #2 ($filter_value) must be passed explicitly, \
         because the default value is not known"
    );
}

#[test]
fn a_variadic_defers_its_unknown_name_until_after_the_count() {
    // The count is checked first and $format is unfilled, so the count wins.
    assert_eq!(
        caught("sprintf(x: 1)"),
        "ArgumentCountError: sprintf() expects at least 1 argument, 0 given"
    );
    // With the count satisfied, the unplaceable name is what is left. The
    // variadic's OWN name is unplaceable too.
    assert_eq!(
        caught(r#"sprintf("%s", unknown: 1)"#),
        "ArgumentCountError: sprintf() does not accept unknown named parameters"
    );
    assert_eq!(
        caught(r#"sprintf(format: "%d", values: 1)"#),
        "ArgumentCountError: sprintf() does not accept unknown named parameters"
    );
}

#[test]
fn rand_accepts_zero_or_two_arguments_and_nothing_between() {
    // Reflection declares both parameters optional; the reference still refuses
    // one, and spells the refusal "exactly 2".
    assert_eq!(
        caught("rand(5)"),
        "ArgumentCountError: rand() expects exactly 2 arguments, 1 given"
    );
    assert_eq!(
        caught("rand(1, 2, 3)"),
        "ArgumentCountError: rand() expects exactly 2 arguments, 3 given"
    );
}

#[test]
fn a_hole_is_refused_before_the_count() {
    // Two slots reached of a function that requires three: the reference reports
    // the empty first slot, not the shortfall.
    assert_eq!(
        caught("array_fill(count: 2)"),
        "ArgumentCountError: array_fill(): Argument #1 ($start_index) not passed"
    );
}

#[test]
fn a_refused_call_traces_the_arguments_it_bound() {
    // Every slot the call reached, a hole as NULL, and a name the variadic could
    // not place spelled `name: value` — which is how the reference prints them.
    let src = r#"<?php
        function f() { count(mode: "ab"); }
        try { f(); } catch (Throwable $e) { echo $e->getTraceAsString(); }"#;
    let out = eval_capture(src).expect("eval");
    assert!(
        out.contains("count(NULL, 'ab')"),
        "trace should render the bound arguments, got: {out}"
    );
    let src = r#"<?php
        function f() { sprintf("%s", 1, nosuch: true); }
        try { f(); } catch (Throwable $e) { echo $e->getTraceAsString(); }"#;
    let out = eval_capture(src).expect("eval");
    assert!(
        out.contains("sprintf('%s', 1, nosuch: true)"),
        "trace should name an unplaced argument, got: {out}"
    );
}

#[test]
fn an_unplaceable_name_is_reported_after_the_types() {
    // The array in #1 is the reference's answer; the unplaceable name waits.
    assert_eq!(
        caught(r#"sprintf([], 0, nosuch: 1)"#),
        "TypeError: sprintf(): Argument #1 ($format) must be of type string, array given"
    );
}

#[test]
fn null_in_a_union_that_offers_a_scalar_is_only_deprecated() {
    // `int|float` and `array|string` both have a member null coerces to, so the
    // reference deprecates and runs; a bare `array` has none and is a TypeError.
    // `number_format`'s deprecation names only `float` (php 8.5.11), while its
    // TypeError for a string still says `int|float`.
    let src = r#"<?php var_dump(number_format(null));"#;
    assert_eq!(
        eval_capture(src).expect("eval"),
        "\nDeprecated: number_format(): Passing null to parameter #1 ($num) of type float \
         is deprecated in Command line code on line 1\nstring(1) \"0\"\n"
    );
}

/// The trace a failure renders, with the frame's arguments as they stand when
/// the argument parser stopped.
fn trace(expr: &str) -> String {
    let src =
        format!("<?php try {{ {expr}; }} catch (Throwable $e) {{ echo $e->getTraceAsString(); }}");
    eval_capture(&src).unwrap_or_else(|e| panic!("eval error for {expr:?}: {e}"))
}

#[test]
fn a_string_parameter_converts_its_argument_in_the_trace() {
    // `zend_parse_arg_str` writes the converted string back into the argument
    // slot, so the frame shows `'5'` for the 5 that was written.
    assert!(
        trace("str_pad(5, [])").contains("str_pad('5', Array)"),
        "got: {}",
        trace("str_pad(5, [])")
    );
    // Including the null a scalar parameter merely deprecates.
    let t = trace("explode(null, [], [])");
    assert!(t.contains("explode('', Array, Array)"), "got: {t}");
}

#[test]
fn an_int_parameter_leaves_its_argument_alone_in_the_trace() {
    // `zend_parse_arg_long` converts into a local, not into the slot, so the
    // numeric STRING is still a string in the frame — the opposite of the
    // `string` parameter above, and the reason this is a table of two types
    // rather than a blanket coercion.
    let t = trace(r#"substr("abc", "1", [])"#);
    assert!(t.contains("substr('abc', '1', Array)"), "got: {t}");
    // A `bool` parameter likewise: the null stays a null.
    let t = trace("array_chunk([1], 0, null)");
    assert!(t.contains("array_chunk(Array, 0, NULL)"), "got: {t}");
}

#[test]
fn an_int_float_union_converts_its_argument_in_the_trace() {
    // `zend_parse_arg_number` DOES write back, and picks the member the string
    // spells: `"1.5"` becomes a float, `null` becomes an int 0.
    let t = trace(r#"number_format("1.5", [])"#);
    assert!(t.contains("number_format(1.5, Array)"), "got: {t}");
    let t = trace("number_format(null, [])");
    assert!(t.contains("number_format(0, Array)"), "got: {t}");
}

#[test]
fn an_arity_refusal_reports_its_arguments_as_written() {
    // The count is checked before any argument is parsed, so nothing has been
    // converted yet — `strtolower(5, 6)`, not `strtolower('5', 6)`.
    let t = trace("strtolower(5, 6)");
    assert!(t.contains("strtolower(5, 6)"), "got: {t}");
}

#[test]
fn a_type_refusal_names_a_boolean_by_its_value() {
    // `zend_zval_value_name`: a diagnostic spells a bool `true`/`false`, which
    // is the one place it disagrees with `get_debug_type`.
    assert_eq!(
        caught("count(true)"),
        "TypeError: count(): Argument #1 ($value) must be of type Countable|array, true given"
    );
    assert_eq!(
        caught("count(false)"),
        "TypeError: count(): Argument #1 ($value) must be of type Countable|array, false given"
    );
}
