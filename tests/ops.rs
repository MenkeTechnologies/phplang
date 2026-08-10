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

// ── PHP 8 string-to-number juggling ──────────────────────────────────────────
//
// PHP 8 split what PHP 7 did silently into three outcomes, and every case below
// was byte-diffed against the reference CLI (stdout, which is where it puts its
// diagnostics) before being written down.

/// The one-line preamble the CLI names in a diagnostic raised from `eval_capture`.
fn warn(line: u32) -> String {
    format!("\nWarning: A non-numeric value encountered in Command line code on line {line}\n")
}

#[test]
fn non_numeric_string_is_a_type_error() {
    // The message names both operand types and the operator that joined them.
    for (expr, want) in [
        (r#""g" + 9"#, "string + int"),
        (r#"9 + "g""#, "int + string"),
        (r#""g" - 1"#, "string - int"),
        (r#""g" * 2"#, "string * int"),
        (r#""g" / 2"#, "string / int"),
        (r#""g" % 2"#, "string % int"),
        (r#""g" ** 2"#, "string ** int"),
        (r#""g" + 1.5"#, "string + float"),
        (r#""g" + "h""#, "string + string"),
        (r#""g" + true"#, "string + bool"),
        (r#""g" + null"#, "string + null"),
        (r#""g" + [1]"#, "string + array"),
        (r#"[1] * 2"#, "array * int"),
    ] {
        let src = format!(
            r#"<?php try {{ $x = {expr}; }} catch (TypeError $e) {{ echo $e->getMessage(); }}"#
        );
        assert_eq!(
            run(&src),
            format!("Unsupported operand types: {want}"),
            "{expr}"
        );
    }
}

#[test]
fn empty_and_blank_strings_have_no_numeric_reading() {
    // `""` and `"   "` are not zero in PHP 8 — they have no numeric prefix at
    // all, so they throw exactly like `"g"` does.
    for expr in [r#""" + 1"#, r#""   " + 1"#, r#""INF" + 1"#, r#""NAN" + 1"#] {
        let src = format!(
            r#"<?php try {{ $x = {expr}; echo "no throw"; }} catch (TypeError $e) {{ echo "threw"; }}"#
        );
        assert_eq!(run(&src), "threw", "{expr}");
    }
}

#[test]
fn leading_numeric_string_warns_and_uses_its_prefix() {
    assert_eq!(run(r#"<?php echo "5g" + 1;"#), format!("{}6", warn(1)));
    assert_eq!(run(r#"<?php echo "5g" - 1;"#), format!("{}4", warn(1)));
    assert_eq!(run(r#"<?php echo "5g" * 2;"#), format!("{}10", warn(1)));
    assert_eq!(run(r#"<?php echo "5g" / 2;"#), format!("{}2.5", warn(1)));
    assert_eq!(run(r#"<?php echo "5g" % 2;"#), format!("{}1", warn(1)));
    assert_eq!(run(r#"<?php echo "5g" ** 2;"#), format!("{}25", warn(1)));
    // A prefix that is itself a float, and a signed one.
    assert_eq!(run(r#"<?php echo ".5g" + 0;"#), format!("{}0.5", warn(1)));
    assert_eq!(run(r#"<?php echo "-5g" + 0;"#), format!("{}-5", warn(1)));
    // `"0x1A"` reads as `0`: PHP has not accepted hex in numeric strings since 5.
    assert_eq!(run(r#"<?php echo "0x1A" + 0;"#), format!("{}0", warn(1)));
}

#[test]
fn operands_resolve_left_to_right() {
    // Observable ordering: a leading-numeric left operand warns before a
    // non-numeric right operand throws, and a non-numeric left operand throws
    // before the right is looked at at all.
    assert_eq!(
        run(
            r#"<?php try { $x = "5g" + "g"; } catch (TypeError $e) { echo "|", $e->getMessage(); }"#
        ),
        format!("{}|Unsupported operand types: string + string", warn(1))
    );
    assert_eq!(
        run(
            r#"<?php try { $x = "g" + "5g"; } catch (TypeError $e) { echo "|", $e->getMessage(); }"#
        ),
        "|Unsupported operand types: string + string"
    );
    // Two leading-numeric operands warn twice and still produce a value.
    assert_eq!(
        run(r#"<?php echo "5g" + "5g";"#),
        format!("{}{}10", warn(1), warn(1))
    );
}

#[test]
fn operand_rules_precede_the_operators_own_checks() {
    // `"g" / 0` is a TypeError, not a DivisionByZeroError: the operands are
    // resolved before the divisor is inspected.
    assert_eq!(
        run(r#"<?php try { $x = "g" / 0; } catch (Throwable $e) { echo get_class($e); }"#),
        "TypeError"
    );
    assert_eq!(
        run(r#"<?php try { $x = "5g" / 0; } catch (Throwable $e) { echo "|", get_class($e); }"#),
        format!("{}|DivisionByZeroError", warn(1))
    );
}

#[test]
fn unary_plus_and_minus_report_as_multiplication() {
    // Both lower to a multiplication in the reference, so the type pair they
    // name is `string * int` rather than anything involving `+`/`-`.
    for expr in [r#"-"g""#, r#"+"g""#] {
        let src = format!(
            r#"<?php try {{ $x = {expr}; }} catch (TypeError $e) {{ echo $e->getMessage(); }}"#
        );
        assert_eq!(
            run(&src),
            "Unsupported operand types: string * int",
            "{expr}"
        );
    }
    assert_eq!(run(r#"<?php echo -"5g";"#), format!("{}-5", warn(1)));
    assert_eq!(run(r#"<?php echo +"5g";"#), format!("{}5", warn(1)));
    // Unary plus on an already-numeric operand stays silent and numeric.
    assert_eq!(run(r#"<?php var_dump(+"5");"#), "int(5)\n");
}

#[test]
fn compound_assignment_follows_the_same_rules() {
    assert_eq!(
        run(r#"<?php $x = "g"; try { $x += 9; } catch (TypeError $e) { echo $e->getMessage(); }"#),
        "Unsupported operand types: string + int"
    );
    assert_eq!(
        run(r#"<?php $x = "5g"; $x += 1; echo $x;"#),
        format!("{}6", warn(1))
    );
    // `.=` is concatenation, which the change did not touch.
    assert_eq!(run(r#"<?php $x = "g"; $x .= 9; echo $x;"#), "g9");
}

#[test]
fn bool_null_and_numeric_strings_still_convert_silently() {
    assert_eq!(run(r#"<?php echo null + 1;"#), "1");
    assert_eq!(run(r#"<?php echo true + 1;"#), "2");
    assert_eq!(run(r#"<?php echo " 5 " + 1;"#), "6");
    assert_eq!(run(r#"<?php echo "5." + 1;"#), "6");
    assert_eq!(run(r#"<?php echo ".5" + 1;"#), "1.5");
    assert_eq!(run(r#"<?php echo "5e3" + 0;"#), "5000");
}

#[test]
fn exponent_without_digits_reads_as_the_mantissa() {
    // `"5e"` has no exponent, so the numeric prefix is `5` — a backtrack the
    // scanner has to perform rather than failing at the `e`.
    assert_eq!(run(r#"<?php echo "5e" + 0;"#), format!("{}5", warn(1)));
    assert_eq!(run(r#"<?php echo "5e+" + 0;"#), format!("{}5", warn(1)));
    assert_eq!(run(r#"<?php var_dump(is_numeric("5e"));"#), "bool(false)\n");
    // An exponent that overflows is still a fully numeric string.
    assert_eq!(
        run(r#"<?php var_dump(is_numeric("1e400"));"#),
        "bool(true)\n"
    );
    assert_eq!(run(r#"<?php echo "1e400" + 0;"#), "INF");
}

#[test]
fn type_error_is_catchable_and_ignores_error_reporting() {
    // The warning is maskable; the TypeError is an exception and is not.
    assert_eq!(
        run(r#"<?php error_reporting(E_ALL & ~E_WARNING); echo "5g" + 1;"#),
        "6"
    );
    assert_eq!(
        run(
            r#"<?php error_reporting(0); try { $x = "g" + 1; } catch (TypeError $e) { echo "caught"; }"#
        ),
        "caught"
    );
    // Catchable from inside a function, through the call unwind.
    assert_eq!(
        run(
            r#"<?php function f() { return "g" + 1; } try { f(); } catch (TypeError $e) { echo "caught"; }"#
        ),
        "caught"
    );
}

#[test]
fn array_plus_array_is_union_not_arithmetic() {
    // The left operand's entries win; the right contributes only missing keys.
    assert_eq!(
        run(r#"<?php print_r([1] + [2]);"#),
        "Array\n(\n    [0] => 1\n)\n"
    );
    assert_eq!(
        run(r#"<?php print_r([1] + [9, 8]);"#),
        "Array\n(\n    [0] => 1\n    [1] => 8\n)\n"
    );
    assert_eq!(
        run(r#"<?php print_r(["a" => 1] + ["b" => 2]);"#),
        "Array\n(\n    [a] => 1\n    [b] => 2\n)\n"
    );
}

#[test]
fn bitwise_takes_the_string_path_only_when_both_sides_are_strings() {
    // Two strings combine byte-wise and stay a string: `0x35 | 0x33` is `0x37`.
    assert_eq!(run(r#"<?php var_dump("5" | "3");"#), "string(1) \"7\"\n");
    assert_eq!(run(r#"<?php var_dump("g" | "h");"#), "string(1) \"o\"\n");
    // `&` and `^` stop at the shorter operand; `|` runs to the longer one.
    assert_eq!(run(r#"<?php var_dump(strlen("abc" & "xy"));"#), "int(2)\n");
    assert_eq!(run(r#"<?php var_dump(strlen("abc" ^ "xy"));"#), "int(2)\n");
    assert_eq!(run(r#"<?php var_dump(strlen("abc" | "xy"));"#), "int(3)\n");
    // A string against a number is numeric, so the operand rules apply.
    assert_eq!(run(r#"<?php var_dump("10" | 1);"#), "int(11)\n");
    assert_eq!(
        run(r#"<?php try { $x = "g" | 1; } catch (TypeError $e) { echo $e->getMessage(); }"#),
        "Unsupported operand types: string | int"
    );
    assert_eq!(
        run(r#"<?php try { $x = ~[1]; } catch (TypeError $e) { echo $e->getMessage(); }"#),
        "Cannot perform bitwise not on array"
    );
}

#[test]
fn narrowing_a_float_operand_to_int_is_diagnosed() {
    // `% << >> & | ^` accept only integers, and PHP reports the narrowing.
    assert_eq!(
        run(r#"<?php echo 2.5 % 2;"#),
        "\nDeprecated: Implicit conversion from float 2.5 to int loses precision in Command line code on line 1\n0"
    );
    // An integral float narrows in silence.
    assert_eq!(run(r#"<?php echo 2.0 % 2;"#), "0");
    // A float-string quotes the string it came from, not the parsed number.
    assert_eq!(
        run(r#"<?php echo "2.5" % 2;"#),
        "\nDeprecated: Implicit conversion from float-string \"2.5\" to int loses precision in Command line code on line 1\n0"
    );
    // An integer-format string that merely outgrew i64 narrows silently, while
    // a float-format one of the same magnitude does not.
    assert_eq!(run(r#"<?php echo "9223372036854775808" % 2;"#), "1");
    assert_eq!(
        run(r#"<?php echo "1e20" % 2;"#),
        "\nDeprecated: Implicit conversion from float-string \"1e20\" to int loses precision in Command line code on line 1\n1"
    );
    // An out-of-range float *literal* is a warning instead, and yields 0.
    assert_eq!(
        run(r#"<?php echo 1e20 % 2;"#),
        "\nWarning: The float 1.0E+20 is not representable as an int, cast occurred in Command line code on line 1\n0"
    );
}

#[test]
fn the_left_operand_is_narrowed_before_the_right_is_classified() {
    // `2.5 % "INF"` deprecates the left narrowing and only then throws on the
    // right, so the two steps interleave per operand rather than batching.
    assert_eq!(
        run(r#"<?php try { $x = 2.5 % "INF"; } catch (TypeError $e) { echo "|", $e->getMessage(); }"#),
        "\nDeprecated: Implicit conversion from float 2.5 to int loses precision in Command line code on line 1\n|Unsupported operand types: float % string"
    );
}

#[test]
fn increment_and_decrement_refuse_arrays_and_objects() {
    // Refused by name, not with the `Unsupported operand types` wording.
    for (src, want) in [
        (r#"$x = [1]; $x++;"#, "Cannot increment array"),
        (r#"$x = [1]; $x--;"#, "Cannot decrement array"),
        (r#"$x = new stdClass; $x++;"#, "Cannot increment stdClass"),
        (r#"$x = new stdClass; $x--;"#, "Cannot decrement stdClass"),
    ] {
        let prog =
            format!(r#"<?php try {{ {src} }} catch (TypeError $e) {{ echo $e->getMessage(); }}"#);
        assert_eq!(run(&prog), want, "{src}");
    }
    // The types `++`/`--` merely leave alone still only warn.
    assert_eq!(run(r#"<?php $x = 1.5; $x++; echo $x;"#), "2.5");
}

// ── the `*` operand swap ─────────────────────────────────────────────────────

/// The reference's compiler puts a CONSTANT operand of `*` in the second slot,
/// so `"g" * $t` reports `int * string` rather than the source order. Only `*`
/// does this; `+` commutes on numbers but is also array union, and the reference
/// leaves it alone.
#[test]
fn multiplication_swaps_a_constant_left_operand() {
    let catch = |e: &str| {
        format!(
            r#"<?php $t = 1; $s = "g"; try {{ $x = {e}; }} catch (TypeError $er) {{ echo $er->getMessage(); }}"#
        )
    };
    // Constant left, runtime right — swapped.
    assert_eq!(
        run(&catch(r#""g" * $t"#)),
        "Unsupported operand types: int * string"
    );
    assert_eq!(
        run(&catch(r#""g" * [1][0]"#)),
        "Unsupported operand types: int * string"
    );
    // Not `*` — no swap, source order.
    assert_eq!(
        run(&catch(r#""g" + $t"#)),
        "Unsupported operand types: string + int"
    );
    assert_eq!(
        run(&catch(r#""g" - $t"#)),
        "Unsupported operand types: string - int"
    );
    assert_eq!(
        run(&catch(r#""g" / $t"#)),
        "Unsupported operand types: string / int"
    );
    assert_eq!(
        run(&catch(r#""g" ** $t"#)),
        "Unsupported operand types: string ** int"
    );
    // Left already runtime, or both runtime — no swap either way.
    assert_eq!(
        run(&catch(r#"$t * "g""#)),
        "Unsupported operand types: int * string"
    );
    assert_eq!(
        run(&catch(r#"$s * $t"#)),
        "Unsupported operand types: string * int"
    );
}

/// The swap tracks the reference's constant FOLDING, not just "is it a literal".
#[test]
fn the_swap_follows_what_the_reference_would_fold() {
    let catch = |e: &str| {
        format!(r#"<?php try {{ $x = {e}; }} catch (TypeError $er) {{ echo $er->getMessage(); }}"#)
    };
    // Arithmetic over numeric literals folds to a constant → no swap.
    assert_eq!(
        run(&catch(r#""g" * (1*1)"#)),
        "Unsupported operand types: string * int"
    );
    assert_eq!(
        run(&catch(r#""g" * ("2"+1)"#)),
        "Unsupported operand types: string * int"
    );
    assert_eq!(
        run(&catch(r#""g" * (2.5*2)"#)),
        "Unsupported operand types: string * float"
    );
    // A LEADING-numeric string constant would warn if folded, so the reference
    // does not fold it — the operand stays runtime and the swap happens.
    assert_eq!(
        run(&catch(r#""g" * ("0x1A"*1)"#)),
        format!("{}Unsupported operand types: int * string", warn(1))
    );
    // A call is NOT treated as runtime: the reference folds `strlen()` on a
    // literal, and swapping there would invent a divergence.
    assert_eq!(
        run(&catch(r#""g" * strlen("ab")"#)),
        "Unsupported operand types: string * int"
    );
}

/// The swap is observable in the WARNINGS too, not only the message, because the
/// operands are coerced in slot order: the swapped-in right operand throws
/// before the constant left one is ever coerced, so its warning never fires.
#[test]
fn the_swap_decides_which_operand_warns_first() {
    // Constant left is coerced SECOND, so "5g" never warns — the throw from $g
    // comes first.
    let swapped = r#"<?php $g = "g"; try { $x = "5g" * $g; } catch (TypeError $e) { echo $e->getMessage(); }"#;
    assert_eq!(run(swapped), "Unsupported operand types: string * string");
    // Both runtime: no swap, so the left IS coerced first and does warn.
    let plain = r#"<?php $g = "g"; $n = "5g"; try { $x = $n * $g; } catch (TypeError $e) { echo $e->getMessage(); }"#;
    assert_eq!(
        run(plain),
        format!("{}Unsupported operand types: string * string", warn(1))
    );
    // `+` is not swapped, so the constant left warns there.
    let plus = r#"<?php $g = "g"; try { $x = "5g" + $g; } catch (TypeError $e) { echo $e->getMessage(); }"#;
    assert_eq!(
        run(plus),
        format!("{}Unsupported operand types: string + string", warn(1))
    );
}

// ── PHP_INT_MIN / -1: the four operations that overflow on it ────────────────

/// `PHP_INT_MIN` divided by `-1` is the one case where the exact answer is one
/// past `PHP_INT_MAX`, and each of the four operations that meet it resolves it
/// differently. Every one of them panicked in debug Rust before this was pinned
/// — `attempt to divide with overflow`, `attempt to calculate the remainder with
/// overflow`, and `attempt to negate with overflow` — which is a crash where the
/// reference produces a value or a catchable error.
///
/// Each expectation below was taken from `php 8.5.9`.
#[test]
fn int_min_over_minus_one_does_not_overflow() {
    // `/` widens to float rather than reporting anything.
    assert_eq!(
        run("<?php var_dump(PHP_INT_MIN / -1);"),
        "float(9.223372036854776E+18)\n"
    );
    // `%` is mathematically 0 even though the quotient is unrepresentable.
    assert_eq!(run("<?php var_dump(PHP_INT_MIN % -1);"), "int(0)\n");
    // `abs` widens for the same reason `-PHP_INT_MIN` does.
    assert_eq!(
        run("<?php var_dump(abs(PHP_INT_MIN));"),
        "float(9.223372036854776E+18)\n"
    );
    assert_eq!(
        run("<?php var_dump(-PHP_INT_MIN);"),
        "float(9.223372036854776E+18)\n"
    );
    // `intdiv` must answer an int, so it is the only one that raises.
    assert_eq!(
        run("<?php try { intdiv(PHP_INT_MIN, -1); } \
             catch (Throwable $e) { echo get_class($e), ': ', $e->getMessage(); }"),
        "ArithmeticError: Division of PHP_INT_MIN by -1 is not an integer"
    );
}

/// `intdiv`'s overflow is an `ArithmeticError` and its zero divisor a
/// `DivisionByZeroError`. `DivisionByZeroError` EXTENDS `ArithmeticError`, so the
/// narrow arm must not swallow the overflow — a test that caught only `Throwable`
/// would pass with the two collapsed into one class.
#[test]
fn intdiv_overflow_and_zero_are_different_classes() {
    assert_eq!(
        run("<?php try { intdiv(PHP_INT_MIN, -1); } \
             catch (DivisionByZeroError $e) { echo 'dbz'; } \
             catch (ArithmeticError $e) { echo 'arith'; }"),
        "arith"
    );
    assert_eq!(
        run("<?php try { intdiv(1, 0); } \
             catch (DivisionByZeroError $e) { echo 'dbz:', $e->getMessage(); }"),
        "dbz:Division by zero"
    );
}

/// The neighbouring values must keep the ordinary integer answers — a fix that
/// widened or raised too eagerly would still pass the overflow test above.
#[test]
fn int_min_neighbours_stay_integers() {
    assert_eq!(
        run("<?php var_dump(intdiv(PHP_INT_MIN, 1));"),
        "int(-9223372036854775808)\n"
    );
    assert_eq!(
        run("<?php var_dump(intdiv(PHP_INT_MAX, -1));"),
        "int(-9223372036854775807)\n"
    );
    assert_eq!(run("<?php var_dump(PHP_INT_MAX % -1);"), "int(0)\n");
    assert_eq!(
        run("<?php var_dump(abs(PHP_INT_MIN + 1));"),
        "int(9223372036854775807)\n"
    );
    // Truncation toward zero, not flooring, on every sign combination.
    assert_eq!(
        run("<?php var_dump(intdiv(-7, 2), intdiv(7, -2), intdiv(-7, -2));"),
        "int(-3)\nint(-3)\nint(3)\n"
    );
    assert_eq!(
        run("<?php var_dump(-7 % 3, 7 % -3, -7 % -3);"),
        "int(-1)\nint(1)\nint(-1)\n"
    );
}
