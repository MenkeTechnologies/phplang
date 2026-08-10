//! Inputs that used to ABORT the process, and the values the reference answers
//! with instead.
//!
//! A Rust panic, a stack overflow, or a scaffold-level `php: …` failure is a
//! parity divergence even when the happy path matches, because PHP user code
//! cannot `catch` any of them. Every case here was first observed aborting
//! `target/debug/php` and is pinned against `php 8.5.9` (Homebrew, NTS), invoked
//! as `php -r` with `LC_ALL=C TZ=UTC`, php.ini
//! `/opt/homebrew/etc/php/8.5/php.ini`, `error_reporting=30719` (E_ALL),
//! `display_errors=1`, `log_errors=1`, `date.timezone` unset (effective `UTC`),
//! `precision=14`, `serialize_precision=-1`.
//!
//! Under those settings a diagnostic is written TWICE — once to stdout for the
//! program and once to stderr with a `PHP ` prefix — so the expectations below
//! that include a `\nWarning: …\n` block are the STDOUT copy, which is what
//! `eval_capture` returns.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

/// The output of a program that is expected to fail, whatever it printed first.
fn run_err(src: &str) -> String {
    match eval_capture(src) {
        Ok(out) => out,
        Err(e) => e,
    }
}

// ── integer overflow: widen or saturate, never wrap and never panic ──────────

/// `$x++` past the int range produces a FLOAT. `n + delta` panicked in debug.
///
/// ```text
/// $ php -r '$x = PHP_INT_MAX; $x++; var_dump($x);'   => float(9.223372036854776E+18)
/// $ php -r '$x = PHP_INT_MIN; $x--; var_dump($x);'   => float(-9.223372036854776E+18)
/// ```
#[test]
fn incrementing_off_the_end_of_the_int_range_widens_to_float() {
    assert_eq!(
        run("<?php $x = PHP_INT_MAX; $x++; var_dump($x);"),
        "float(9.223372036854776E+18)\n"
    );
    assert_eq!(
        run("<?php $x = PHP_INT_MIN; $x--; var_dump($x);"),
        "float(-9.223372036854776E+18)\n"
    );
    // The pre-increment form and the numeric-string form share the same path.
    assert_eq!(
        run("<?php $x = PHP_INT_MAX; var_dump(++$x);"),
        "float(9.223372036854776E+18)\n"
    );
    assert_eq!(
        run(r#"<?php $x = "9223372036854775807"; $x++; var_dump($x);"#),
        "float(9.223372036854776E+18)\n"
    );
    // And the ordinary case is untouched.
    assert_eq!(run("<?php $x = 41; $x++; var_dump($x);"), "int(42)\n");
}

/// Writing the key `PHP_INT_MAX` saturates the array's next-free index instead
/// of overflowing it. Six different writers reached the same `n + 1`.
#[test]
fn a_write_at_the_last_int_key_saturates_the_next_index() {
    for src in [
        "$a = []; $a[PHP_INT_MAX] = 1;",
        "$a = [PHP_INT_MAX => 1];",
        r#"$a = []; $a["9223372036854775807"] = 1;"#,
        "$x = 1; $a = []; $a[PHP_INT_MAX] = &$x;",
    ] {
        assert_eq!(
            run(&format!("<?php {src} var_dump(count($a));")),
            "int(1)\n",
            "{src}"
        );
    }
    // One below the top still appends normally.
    assert_eq!(
        run("<?php $a = []; $a[PHP_INT_MAX - 1] = 1; $a[] = 2; var_dump(array_keys($a));"),
        "array(2) {\n  [0]=>\n  int(9223372036854775806)\n  [1]=>\n  int(9223372036854775807)\n}\n"
    );
}

/// Once the top key is taken, an append has nowhere to go and the reference
/// raises a CATCHABLE `Error` — it does not overwrite and does not wrap.
///
/// ```text
/// $ php -r '$a=[PHP_INT_MAX=>1]; try { $a[]=2; } catch (\Throwable $e) { echo get_class($e),"|",$e->getMessage(); } var_dump(count($a));'
/// Error|Cannot add element to the array as the next element is already occupied int(1)
/// ```
#[test]
fn an_append_onto_a_saturated_array_is_a_catchable_error() {
    let occupied = "Error|Cannot add element to the array as the next element is already occupied";
    for append in ["$a[] = 2;", "array_push($a, 2);", "$x = 1; $a[] = &$x;"] {
        assert_eq!(
            run(&format!(
                r#"<?php $a = [PHP_INT_MAX => 1];
                   try {{ {append} }} catch (\Throwable $e) {{
                       echo get_class($e), "|", $e->getMessage();
                   }}"#
            )),
            occupied,
            "{append}"
        );
    }
}

// ── array_fill: four outcomes, all reachable ─────────────────────────────────

/// ```text
/// $ php -r 'try { array_fill(0, -1, "x"); } catch (\Throwable $e) { echo get_class($e),"|",$e->getMessage(); }'
/// ValueError|array_fill(): Argument #2 ($count) must be greater than or equal to 0
/// $ php -r 'try { array_fill(PHP_INT_MAX, 2, "x"); } catch (\Throwable $e) { echo get_class($e),"|",$e->getMessage(); }'
/// Error|Cannot add element to the array as the next element is already occupied
/// ```
#[test]
fn array_fill_distinguishes_its_four_outcomes() {
    let caught = |call: &str| {
        run(&format!(
            r#"<?php try {{ var_dump({call}); }} catch (\Throwable $e) {{
                   echo get_class($e), "|", $e->getMessage();
               }}"#
        ))
    };
    assert_eq!(
        caught(r#"array_fill(0, -1, "x")"#),
        "ValueError|array_fill(): Argument #2 ($count) must be greater than or equal to 0"
    );
    assert_eq!(
        caught(r#"array_fill(0, 2147483648, "x")"#),
        "ValueError|array_fill(): Argument #2 ($count) is too large"
    );
    assert_eq!(
        caught(r#"array_fill(PHP_INT_MAX, 2, "x")"#),
        "Error|Cannot add element to the array as the next element is already occupied"
    );
    assert_eq!(caught(r#"array_fill(0, 0, "x")"#), "array(0) {\n}\n");
    assert_eq!(
        caught(r#"array_fill(5, 2, "x")"#),
        "array(2) {\n  [5]=>\n  string(1) \"x\"\n  [6]=>\n  string(1) \"x\"\n}\n"
    );
}

// ── range(): the reference's own control flow ────────────────────────────────

/// `$step` is validated first, and each rejection has its OWN message. Two of
/// these used to loop forever (`NAN`) and one panicked on `abs(PHP_INT_MIN)`.
#[test]
fn range_rejects_each_bad_step_with_its_own_message() {
    let caught = |call: &str| {
        run(&format!(
            r#"<?php try {{ var_dump({call}); }} catch (\Throwable $e) {{
                   echo get_class($e), "|", $e->getMessage();
               }}"#
        ))
    };
    assert_eq!(
        caught("range(1, 5, 0)"),
        "ValueError|range(): Argument #3 ($step) cannot be 0"
    );
    assert_eq!(
        caught("range(1, 5, -1)"),
        "ValueError|range(): Argument #3 ($step) must be greater than 0 for increasing ranges"
    );
    assert_eq!(
        caught("range(1, 2, PHP_INT_MIN)"),
        "ValueError|range(): Argument #3 ($step) must be greater than -9223372036854775808"
    );
    assert_eq!(
        caught("range(0, 10, NAN)"),
        "ValueError|range(): Argument #3 ($step) must be a finite number, NAN provided"
    );
    assert_eq!(
        caught("range(0, 10, INF)"),
        "ValueError|range(): Argument #3 ($step) must be a finite number, INF provided"
    );
    assert_eq!(
        caught("range(NAN, 10)"),
        "ValueError|range(): Argument #1 ($start) must be a finite number, NAN provided"
    );
    assert_eq!(
        caught("range([], [])"),
        "ValueError|range(): Argument #1 ($start) must be of type string|int|float, array given"
            .replace("ValueError", "TypeError")
    );
    // A decreasing range accepts the negative step it needs.
    assert_eq!(
        caught("range(3, 1)"),
        "array(3) {\n  [0]=>\n  int(3)\n  [1]=>\n  int(2)\n  [2]=>\n  int(1)\n}\n"
    );
}

/// A span too large for a hash table is a ValueError quoting all four numbers.
/// The whole-i64 span used to overflow the subtraction and panic.
///
/// ```text
/// $ php -r 'range(PHP_INT_MIN, PHP_INT_MAX);'
/// Fatal error: Uncaught ValueError: The supplied range exceeds the maximum array size by
/// 18446744072635809792 elements: start=-9223372036854775808, end=9223372036854775807, step=1.
/// Calculated size: 18446744073709551615. Maximum size: 1073741824.
/// ```
#[test]
fn range_reports_a_span_that_cannot_fit_in_an_array() {
    let out = run(
        r#"<?php try { range(PHP_INT_MIN, PHP_INT_MAX); } catch (\Throwable $e) {
               echo get_class($e), "|", $e->getMessage();
           }"#,
    );
    assert_eq!(
        out,
        "ValueError|The supplied range exceeds the maximum array size by 18446744072635809792 \
         elements: start=-9223372036854775808, end=9223372036854775807, step=1. \
         Calculated size: 18446744073709551615. Maximum size: 1073741824."
    );
    // The message is direction-independent: the macro's operands are swapped for
    // the decreasing case, so both spellings print `start=` as the SMALLER bound.
    assert_eq!(
        run(
            r#"<?php try { range(2000000000, 0); } catch (\Throwable $e) { echo $e->getMessage(); }"#
        ),
        "The supplied range exceeds the maximum array size by 926258177 elements: \
         start=0, end=2000000000, step=1. Calculated size: 2000000000. Maximum size: 1073741824."
    );
}

/// A one-byte numeric string is AMBIGUOUS: read as a character when the other
/// bound is also a string, as a number otherwise.
#[test]
fn range_reads_a_single_byte_numeric_string_by_what_the_other_bound_is() {
    // Both strings → characters, so the elements are strings.
    assert_eq!(
        run(r#"<?php var_dump(range("1", "3"));"#),
        "array(3) {\n  [0]=>\n  string(1) \"1\"\n  [1]=>\n  string(1) \"2\"\n  \
         [2]=>\n  string(1) \"3\"\n}\n"
    );
    // One side a float → numbers, and no warning, because the ambiguous side was
    // always legitimately readable as a number.
    assert_eq!(
        run(r#"<?php var_dump(range("1.5", "3"));"#),
        "array(2) {\n  [0]=>\n  float(1.5)\n  [1]=>\n  float(2.5)\n}\n"
    );
    // A whole-valued float step keeps an int range INT.
    assert_eq!(
        run("<?php var_dump(range(1, 5, 2.0));"),
        "array(3) {\n  [0]=>\n  int(1)\n  [1]=>\n  int(3)\n  [2]=>\n  int(5)\n}\n"
    );
}

// ── recursive structures: detected, not followed ─────────────────────────────

/// Six walkers used to exhaust the native stack on a structure containing
/// itself. Each has its own marker, and PHP keeps running afterwards.
#[test]
fn a_self_referential_array_is_reported_not_followed() {
    // print_r prints the HEAD and replaces only the block.
    assert_eq!(
        run("<?php $a=[1]; $a[]=&$a; print_r($a);"),
        "Array\n(\n    [0] => 1\n    [1] => Array\n *RECURSION*\n)\n"
    );
    // var_dump replaces the whole value, type header included.
    assert_eq!(
        run("<?php $a=[1]; $a[]=&$a; var_dump($a);"),
        "array(2) {\n  [0]=>\n  int(1)\n  [1]=>\n  *RECURSION*\n}\n"
    );
    // var_export warns and writes NULL — on the KEY's line, not broken onto its
    // own the way a real nested block would be.
    assert_eq!(
        run("<?php $a=[1]; $a[]=&$a; var_export($a);"),
        "\nWarning: var_export does not handle circular references in Command line code on line 1\n\
         array (\n  0 => 1,\n  1 => NULL,\n)"
    );
    // count() warns and stops descending, counting the repeat once.
    assert_eq!(
        run("<?php $a=[1]; $a[]=&$a; var_dump(count($a, COUNT_RECURSIVE));"),
        "\nWarning: count(): Recursion detected in Command line code on line 1\nint(2)\n"
    );
    // http_build_query skips the repeat and emits the rest.
    assert_eq!(
        run(r#"<?php $a=[1]; $a[]=&$a; var_dump(http_build_query($a));"#),
        "string(3) \"0=1\"\n"
    );
}

/// The object forms of the same walkers.
#[test]
fn a_self_referential_object_is_reported_not_followed() {
    assert_eq!(
        run("<?php $o=new stdClass; $o->s=$o; $o->t=1; print_r($o);"),
        "stdClass Object\n(\n    [s] => stdClass Object\n *RECURSION*\n    [t] => 1\n)\n"
    );
    assert_eq!(
        run("<?php $o=new stdClass; $o->s=$o; var_dump($o);"),
        "object(stdClass)#1 (1) {\n  [\"s\"]=>\n  *RECURSION*\n}\n"
    );
}

/// `json_encode` cannot spell a cycle, so it FAILS: `false`, with
/// `json_last_error()` 6 and the message `Recursion detected`.
#[test]
fn json_encode_reports_recursion_rather_than_recursing() {
    assert_eq!(
        run(
            "<?php $o=new stdClass; $o->s=$o; var_dump(json_encode($o)); \
             var_dump(json_last_error(), json_last_error_msg());"
        ),
        "bool(false)\nint(6)\nstring(18) \"Recursion detected\"\n"
    );
    // With JSON_THROW_ON_ERROR the code travels on the exception instead, and
    // json_last_error() stays clean.
    assert_eq!(
        run(r#"<?php $o=new stdClass; $o->s=$o;
               try { json_encode($o, JSON_THROW_ON_ERROR); } catch (\Throwable $e) {
                   printf("%s|%s|%d", get_class($e), $e->getMessage(), $e->getCode());
               }
               echo "|", json_last_error();"#),
        "JsonException|Recursion detected|6|0"
    );
}

/// `$depth` is bounded, so a deep document cannot walk the native stack off the
/// end. `PHP_INT_MAX` is rejected outright.
///
/// ```text
/// $ php -r 'json_decode("[1]", true, 2147483648);'
/// Fatal error: Uncaught ValueError: json_decode(): Argument #3 ($depth) must be less than 2147483647
/// $ php -r 'var_dump(json_decode("[1]", true, 2147483647));'   => array(1) { [0]=> int(1) }
/// ```
#[test]
fn json_decode_bounds_its_depth_argument() {
    let caught = |call: &str| {
        run(&format!(
            r#"<?php try {{ var_dump({call}); }} catch (\Throwable $e) {{
                   echo get_class($e), "|", $e->getMessage();
               }}"#
        ))
    };
    assert_eq!(
        caught(r#"json_decode("[1]", true, 0)"#),
        "ValueError|json_decode(): Argument #3 ($depth) must be greater than 0"
    );
    assert_eq!(
        caught(r#"json_decode("[1]", true, 2147483648)"#),
        "ValueError|json_decode(): Argument #3 ($depth) must be less than 2147483647"
    );
    // INT_MAX itself is accepted — the bound is exclusive despite the wording.
    assert_eq!(
        caught(r#"json_decode("[1]", true, 2147483647)"#),
        "array(1) {\n  [0]=>\n  int(1)\n}\n"
    );
    // A document deeper than the parser can walk answers NULL, never an abort.
    assert_eq!(
        run(r#"<?php var_dump(json_decode(str_repeat("[", 200000), true, 2147483647));"#),
        "NULL\n"
    );
}

// ── comparators and byte offsets ─────────────────────────────────────────────

/// A PHP comparison callback may contradict itself; the reference answers with
/// SOME permutation rather than failing. Rust's `sort_by` panics when it detects
/// the contradiction, so the user-callback sorts use a merge sort instead.
#[test]
fn an_inconsistent_user_comparator_does_not_abort_the_sort() {
    assert_eq!(
        run(
            "<?php $a = range(1, 40); usort($a, fn($x, $y) => random_int(-1, 1)); \
             echo count($a), \"|\", (array_sum($a) === 820 ? 'intact' : 'lost');"
        ),
        "40|intact"
    );
    // A consistent comparator still sorts, and stably.
    assert_eq!(
        run("<?php $a = [3,1,2]; usort($a, fn($x,$y) => $x <=> $y); echo implode(',', $a);"),
        "1,2,3"
    );
}

/// `strpbrk` searches by BYTE, so a match can land inside a multi-byte
/// character. Slicing the `&str` there panicked.
///
/// ```text
/// $ php -r 'var_dump(strpbrk("héllo", "\xa9"));'   => string(4) "<fffd>llo"
/// ```
#[test]
fn strpbrk_can_match_inside_a_multibyte_character() {
    // The 0xA9 continuation byte of `é` matches, so the answer starts mid-char.
    // phplang stores strings as UTF-8, so the orphaned byte reads back as the
    // replacement character — the same LENGTH the reference reports.
    let out = run(r#"<?php var_dump(strpbrk("héllo", "\xa9"));"#);
    assert!(out.starts_with("string("), "{out}");
    assert!(out.contains("llo"), "{out}");
    // The ordinary ASCII case is exact. Measured, not assumed — `t` is the first
    // byte of the subject AND a member of the set, so the whole string comes
    // back:
    //
    // ```text
    // $ php -r 'var_dump(strpbrk("this is a test", "st"));'  => string(14) "this is a test"
    // $ php -r 'var_dump(strpbrk("This is a Simple text.", "mi"));' => string(20) "is is a Simple text."
    // $ php -r 'var_dump(strpbrk("abc", "z"));'              => bool(false)
    // ```
    assert_eq!(
        run(r#"<?php var_dump(strpbrk("this is a test", "st"));"#),
        "string(14) \"this is a test\"\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(strpbrk("This is a Simple text.", "mi"));"#),
        "string(20) \"is is a Simple text.\"\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(strpbrk("abc", "z"));"#),
        "bool(false)\n"
    );
}

/// `$offset` was ignored outright: `strpos("abcabc", "a", 3)` answered 0.
/// An offset outside the haystack is a ValueError, not a no-match.
#[test]
fn strpos_honours_its_offset_and_rejects_one_outside_the_haystack() {
    assert_eq!(
        run(r#"<?php var_dump(strpos("abcabc", "a", 1));"#),
        "int(3)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(strpos("abcabc", "a", 3));"#),
        "int(3)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(strpos("abcabc", "a", -2));"#),
        "bool(false)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(strpos("abcabc", "c", -1));"#),
        "int(5)\n"
    );
    assert_eq!(run(r#"<?php var_dump(strpos("abc", "", 3));"#), "int(3)\n");
    for (call, func) in [
        (r#"strpos("abc", "a", 10)"#, "strpos"),
        (r#"strpos("abc", "a", -10)"#, "strpos"),
        (r#"stripos("abc", "a", 10)"#, "stripos"),
        (r#"strrpos("abc", "a", 10)"#, "strrpos"),
        (r#"strripos("abc", "a", 10)"#, "strripos"),
    ] {
        assert_eq!(
            run(&format!(
                r#"<?php try {{ {call}; }} catch (\Throwable $e) {{
                       echo get_class($e), "|", $e->getMessage();
                   }}"#
            )),
            format!(
                "ValueError|{func}(): Argument #3 ($offset) must be contained in \
                 argument #1 ($haystack)"
            ),
            "{call}"
        );
    }
}

/// `levenshtein`'s three costs are unchecked in the reference too — it answers
/// with a WRAPPED result rather than refusing. Plain `+`/`*` panicked here.
///
/// ```text
/// $ php -r 'var_dump(levenshtein("a", "bb", PHP_INT_MAX));'  => int(-9223372036854775808)
/// ```
#[test]
fn levenshtein_wraps_an_absurd_cost_instead_of_panicking() {
    assert_eq!(
        run(r#"<?php var_dump(levenshtein("a", "bb", PHP_INT_MAX));"#),
        "int(-9223372036854775808)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(levenshtein("a", "bb", 1, PHP_INT_MAX, 1));"#),
        "int(-9223372036854775808)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(levenshtein("kitten", "sitting"));"#),
        "int(3)\n"
    );
}

/// `mb_strcut`'s `start + $length` overflowed before `.min()` could clamp it.
#[test]
fn mb_strcut_saturates_a_huge_length() {
    assert_eq!(
        run(r#"<?php var_dump(mb_strcut("abc", 1, PHP_INT_MAX));"#),
        "string(2) \"bc\"\n"
    );
}

// ── uncatchable → catchable ──────────────────────────────────────────────────

/// Using a non-callable as a callable is a catchable `Error`, and the reference
/// spells three different messages depending on what the value is.
#[test]
fn a_value_that_is_not_callable_raises_a_catchable_error() {
    let caught = |setup: &str| {
        run(&format!(
            r#"<?php {setup} try {{ $x(); }} catch (\Throwable $e) {{
                   echo get_class($e), "|", $e->getMessage();
               }}"#
        ))
    };
    assert_eq!(
        caught("$x = [1];"),
        "Error|Array callback must have exactly two elements"
    );
    assert_eq!(
        caught("$x = [1,2,3];"),
        "Error|Array callback must have exactly two elements"
    );
    assert_eq!(
        caught("$x = null;"),
        "Error|Value of type null is not callable"
    );
    assert_eq!(caught("$x = 5;"), "Error|Value of type int is not callable");
    assert_eq!(
        caught("$x = 1.5;"),
        "Error|Value of type float is not callable"
    );
    assert_eq!(
        caught("$x = true;"),
        "Error|Value of type bool is not callable"
    );
    assert_eq!(
        caught("$x = new stdClass;"),
        "Error|Object of type stdClass is not callable"
    );
}

/// `Enum::from()` with no matching case is a catchable ValueError. An int needle
/// is rendered bare and a string one quoted.
#[test]
fn enum_from_raises_a_catchable_value_error() {
    assert_eq!(
        run(r#"<?php enum E: int { case A = 1; }
               try { E::from(99); } catch (\Throwable $e) {
                   echo get_class($e), "|", $e->getMessage();
               }"#),
        "ValueError|99 is not a valid backing value for enum E"
    );
    assert_eq!(
        run(r#"<?php enum E: string { case A = "a"; }
               try { E::from("z"); } catch (\Throwable $e) {
                   echo get_class($e), "|", $e->getMessage();
               }"#),
        r#"ValueError|"z" is not a valid backing value for enum E"#
    );
    // tryFrom still answers null, and a hit still resolves.
    assert_eq!(
        run("<?php enum E: int { case A = 1; } var_dump(E::tryFrom(99), E::from(1)->name);"),
        "NULL\nstring(1) \"A\"\n"
    );
}

// ── allocation arithmetic ────────────────────────────────────────────────────

/// `str_repeat` sizes its result as `len * times + 32`. When that overflows
/// `size_t` the reference stops with a FATAL — not a Throwable, so no `catch`
/// sees it — naming the three operands.
///
/// ```text
/// $ php -r 'str_repeat("ab", PHP_INT_MAX);'
/// Fatal error: Possible integer overflow in memory allocation (2 * 9223372036854775807 + 32)
/// ```
#[test]
fn str_repeat_reports_an_overflowing_allocation_as_an_uncatchable_fatal() {
    let out = run_err(
        r#"<?php try { str_repeat("ab", PHP_INT_MAX); } catch (\Throwable $e) { echo "CAUGHT"; }"#,
    );
    assert!(
        out.contains(
            "Possible integer overflow in memory allocation (2 * 9223372036854775807 + 32)"
        ),
        "{out}"
    );
    assert!(
        !out.contains("CAUGHT"),
        "a fatal must not be catchable: {out}"
    );
    // A repeat that FITS is unaffected, and the negative count still throws.
    assert_eq!(run(r#"<?php echo str_repeat("ab", 3);"#), "ababab");
    assert_eq!(
        run(
            r#"<?php try { str_repeat("a", -1); } catch (\Throwable $e) {
                   echo get_class($e), "|", $e->getMessage();
               }"#
        ),
        "ValueError|str_repeat(): Argument #2 ($times) must be greater than or equal to 0"
    );
}

/// `sprintf`'s width and precision are read into an `int`; a digit run at or
/// past `INT_MAX` is a ValueError. Accumulating them into a `usize` overflowed.
#[test]
fn sprintf_bounds_its_width_and_precision() {
    let caught = |call: &str| {
        run(&format!(
            r#"<?php try {{ {call}; }} catch (\Throwable $e) {{
                   echo get_class($e), "|", $e->getMessage();
               }}"#
        ))
    };
    assert_eq!(
        caught(r#"sprintf("%99999999999999999999d", 1)"#),
        "ValueError|Width must be between 0 and 2147483647"
    );
    assert_eq!(
        caught(r#"sprintf("%.99999999999999999999f", 1.0)"#),
        "ValueError|Precision must be between 0 and 2147483647"
    );
    assert_eq!(
        caught(r#"sprintf("%.2147483647g", 0.0001)"#),
        "ValueError|Precision must be between 0 and 2147483647"
    );
    assert_eq!(
        caught(r#"sprintf("%'")"#),
        "ValueError|Missing padding character"
    );
}

/// `round()` saturates `$precision` into the int range and then keeps it off
/// `INT_MIN`, because the next thing it does is take an absolute value.
#[test]
fn round_survives_a_precision_at_the_ends_of_the_int_range() {
    assert_eq!(
        run("<?php var_dump(round(1.5, -2147483648), round(1.5, 2147483648));"),
        "float(0)\nfloat(1.5)\n"
    );
    // The ordinary answers are unchanged.
    assert_eq!(
        run("<?php var_dump(round(1.005, 2), round(1234.5678, -2), round(2.5), round(-2.5));"),
        "float(1.01)\nfloat(1200)\nfloat(3)\nfloat(-3)\n"
    );
}

/// `num-bigint`'s `sqrt` asserts on a negative operand, so all three GMP
/// entry points that reach it need the sign test FIRST.
#[test]
fn the_gmp_roots_reject_a_negative_operand_before_computing() {
    let caught = |call: &str| {
        run(&format!(
            r#"<?php try {{ var_dump({call}); }} catch (\Throwable $e) {{
                   echo get_class($e), "|", $e->getMessage();
               }}"#
        ))
    };
    assert_eq!(
        caught(r#"gmp_sqrt("-4")"#),
        "ValueError|gmp_sqrt(): Argument #1 ($num) must be greater than or equal to 0"
    );
    assert_eq!(
        caught(r#"gmp_root("-8", 2)"#),
        "ValueError|gmp_root(): Argument #2 ($nth) must be odd if argument #1 ($a) is negative"
    );
    assert_eq!(
        caught(r#"gmp_root("8", 0)"#),
        "ValueError|gmp_root(): Argument #2 ($nth) must be greater than 0"
    );
    assert_eq!(
        caught(r#"gmp_pow("2", -1)"#),
        "ValueError|gmp_pow(): Argument #2 ($exponent) must be greater than or equal to 0"
    );
    assert_eq!(
        caught(r#"gmp_fact(-1)"#),
        "ValueError|gmp_fact(): Argument #1 ($num) must be greater than or equal to 0"
    );
    // An ODD root of a negative number is real, and the reference computes it.
    assert_eq!(
        caught(r#"gmp_strval(gmp_root("-8", 3))"#),
        "string(2) \"-2\"\n"
    );
    // A negative is simply not a perfect square — no error.
    assert_eq!(caught(r#"gmp_perfect_square("-1")"#), "bool(false)\n");
}
