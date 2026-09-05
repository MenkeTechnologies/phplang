//! End-to-end tests for the added control-flow features: `switch`, `do`/`while`,
//! the `match` expression, and the `?:` / `??` operators. Source in, captured
//! `echo` output out — exercising compile → lower → run-on-fusevm.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

// ── switch ──────────────────────────────────────────────────────────────────

#[test]
fn switch_matches_and_breaks() {
    let src = r#"<?php $x = 2;
        switch ($x) {
            case 1: echo "one"; break;
            case 2: echo "two"; break;
            case 3: echo "three"; break;
        }"#;
    assert_eq!(run(src), "two");
}

#[test]
fn switch_falls_through_without_break() {
    // No break on case 1 → execution falls into case 2's body too.
    let src = r#"<?php $x = 1;
        switch ($x) {
            case 1: echo "a";
            case 2: echo "b"; break;
            case 3: echo "c";
        }"#;
    assert_eq!(run(src), "ab");
}

#[test]
fn switch_default_runs_when_no_case_matches() {
    let src = r#"<?php $x = 99;
        switch ($x) {
            case 1: echo "one"; break;
            default: echo "other"; break;
            case 2: echo "two"; break;
        }"#;
    assert_eq!(run(src), "other");
}

#[test]
fn switch_uses_loose_equality() {
    // "1" == 1 loosely, so the numeric case fires for the string subject.
    let src = r#"<?php $x = "1";
        switch ($x) {
            case 1: echo "num"; break;
            default: echo "no"; break;
        }"#;
    assert_eq!(run(src), "num");
}

#[test]
fn switch_break_exits_only_switch_not_outer_loop() {
    let src = r#"<?php $out = "";
        for ($i = 0; $i < 3; $i++) {
            switch ($i) {
                case 1: $out .= "one"; break;
                default: $out .= $i; break;
            }
            $out .= ".";
        }
        echo $out;"#;
    assert_eq!(run(src), "0.one.2.");
}

// ── do/while ────────────────────────────────────────────────────────────────

#[test]
fn do_while_runs_body_once_even_when_false() {
    let src = r#"<?php $n = 0;
        do { echo "ran"; $n++; } while ($n < 0);
        echo $n;"#;
    assert_eq!(run(src), "ran1");
}

#[test]
fn do_while_loops_until_false() {
    let src = r#"<?php $i = 0; $s = "";
        do { $s .= $i; $i++; } while ($i < 4);
        echo $s;"#;
    assert_eq!(run(src), "0123");
}

#[test]
fn do_while_break_and_continue() {
    let src = r#"<?php $i = 0; $s = "";
        do {
            $i++;
            if ($i == 2) { continue; }
            if ($i > 4) { break; }
            $s .= $i;
        } while ($i < 100);
        echo $s;"#;
    assert_eq!(run(src), "134");
}

// ── match ───────────────────────────────────────────────────────────────────

#[test]
fn match_returns_the_matching_arm_value() {
    let src = r#"<?php $x = 2;
        echo match ($x) {
            1 => "one",
            2 => "two",
            3 => "three",
        };"#;
    assert_eq!(run(src), "two");
}

#[test]
fn match_supports_multiple_conditions_per_arm() {
    let src = r#"<?php $x = 3;
        echo match ($x) {
            1, 2 => "low",
            3, 4 => "high",
        };"#;
    assert_eq!(run(src), "high");
}

#[test]
fn match_default_arm() {
    let src = r#"<?php $x = 42;
        echo match ($x) {
            1 => "one",
            default => "fallback",
        };"#;
    assert_eq!(run(src), "fallback");
}

#[test]
fn match_uses_strict_comparison() {
    // "1" !== 1, so the int arm does NOT fire; default catches it.
    let src = r#"<?php $x = "1";
        echo match ($x) {
            1 => "int",
            default => "str",
        };"#;
    assert_eq!(run(src), "str");
}

#[test]
fn match_no_match_without_default_throws() {
    // PHP 8: a `match` with no matching arm and no `default` throws
    // \UnhandledMatchError (the exceptions wave made this faithful; it used to be
    // a scaffold deviation that yielded null). `echo` emits each argument as it is
    // evaluated, so the leading `"["` is written before the `match` is reached and
    // throws; the `"]"` never runs, and the catch appends the message.
    let src = r#"<?php $x = 9;
        try { echo "[", match ($x) { 1 => "a", 2 => "b", }, "]"; }
        catch (UnhandledMatchError $e) { echo $e->getMessage(); }"#;
    assert_eq!(run(src), "[Unhandled match case 9");
}

#[test]
fn match_result_is_usable_in_an_expression() {
    let src = r#"<?php $x = 2;
        $label = match ($x) { 1 => "a", 2 => "b" };
        echo strtoupper($label);"#;
    assert_eq!(run(src), "B");
}

// ── ?: and ?? ───────────────────────────────────────────────────────────────

#[test]
fn elvis_keeps_truthy_left_else_right() {
    assert_eq!(run(r#"<?php echo "yes" ?: "no";"#), "yes");
    assert_eq!(run(r#"<?php echo "" ?: "fallback";"#), "fallback");
    assert_eq!(run(r#"<?php echo 0 ?: 5;"#), "5");
}

#[test]
fn null_coalesce_only_falls_back_on_null() {
    // "0" is falsy but NOT null, so ?? keeps it (unlike ?:).
    assert_eq!(run(r#"<?php echo "0" ?? "fallback";"#), "0");
    assert_eq!(run(r#"<?php echo null ?? "fallback";"#), "fallback");
    assert_eq!(run(r#"<?php $a = null; echo $a ?? "def";"#), "def");
}

#[test]
fn null_coalesce_is_right_associative_chain() {
    assert_eq!(run(r#"<?php echo null ?? null ?? "third";"#), "third");
}

// ── break/continue levels ───────────────────────────────────────────────────
//
// `break n` / `continue n` used to discard the level and always leave the
// innermost loop. All expectations below are byte-checked against reference
// PHP 8.5.

#[test]
fn continue_with_level_skips_the_outer_loop() {
    let src = r#"<?php
        for ($i = 0; $i < 3; $i++) {
            for ($j = 0; $j < 3; $j++) { if ($j == 1) continue 2; echo "$i$j "; }
        }"#;
    assert_eq!(run(src), "00 10 20 ");
}

#[test]
fn break_with_level_leaves_the_outer_loop() {
    let src = r#"<?php
        for ($i = 0; $i < 3; $i++) {
            for ($j = 0; $j < 3; $j++) { if ($i == 1) break 2; echo "$i$j "; }
        }"#;
    assert_eq!(run(src), "00 01 02 ");
}

#[test]
fn switch_counts_as_a_break_level() {
    // `break` leaves the switch; `break 2` leaves the switch *and* the loop.
    let inner = r#"<?php for ($i = 0; $i < 3; $i++) {
            switch ($i) { case 1: break; default: echo "s$i "; } }"#;
    let outer = r#"<?php for ($i = 0; $i < 3; $i++) {
            switch ($i) { case 1: break 2; default: echo "t$i "; } }"#;
    assert_eq!(run(inner), "s0 s2 ");
    assert_eq!(run(outer), "t0 ");
}

#[test]
fn break_level_crosses_a_try_body() {
    // The `try` body compiles to its own chunk, so the level has to survive the
    // control signal that carries it back out to the enclosing loops.
    let src = r#"<?php
        for ($i = 0; $i < 3; $i++) {
            for ($j = 0; $j < 3; $j++) {
                try { if ($j == 1) continue 2; echo "$i$j "; } catch (Exception $e) {}
            }
        }"#;
    assert_eq!(run(src), "00 10 20 ");
}

#[test]
fn break_level_crosses_nested_try_bodies() {
    let src = r#"<?php
        for ($i = 0; $i < 3; $i++) {
            for ($j = 0; $j < 3; $j++) {
                try { try { if ($j == 1) break 2; echo "$i$j "; } finally { echo "f"; } }
                catch (Exception $e) {}
            }
        }"#;
    assert_eq!(run(src), "00 ff");
}

#[test]
fn return_inside_try_inside_loop_terminates_the_function() {
    // Regression guard: the return signal must halt the enclosing loop, not just
    // the try chunk (which would spin forever).
    let src = r#"<?php
        function f() {
            for ($i = 0; $i < 5; $i++) { try { if ($i == 2) return "r$i"; } catch (Exception $e) {} }
            return "end";
        }
        echo f();"#;
    assert_eq!(run(src), "r2");
}

// ── UnhandledMatchError message ─────────────────────────────────────────────
//
// The reference renders the unmatched subject the way a stack trace renders an
// argument, and replaces a non-scalar with `of type <name>`. Concatenating the
// subject into the message instead — which is what this did — reported `null`
// as nothing at all, `true` as `1`, `'hi'` unquoted, and an array as `Array`
// behind an `Array to string conversion` warning the reference never raises.

fn match_error(subject: &str) -> String {
    let src = format!(
        "<?php try {{ echo match ({subject}) {{ 999999 => 1 }}; }} \
         catch (\\UnhandledMatchError $e) {{ echo $e->getMessage(); }}"
    );
    run(&src)
}

#[test]
fn an_unhandled_match_renders_a_scalar_subject_as_a_trace_does() {
    assert_eq!(match_error("null"), "Unhandled match case NULL");
    assert_eq!(match_error("true"), "Unhandled match case true");
    assert_eq!(match_error("false"), "Unhandled match case false");
    assert_eq!(match_error("5"), "Unhandled match case 5");
    assert_eq!(match_error("1.0"), "Unhandled match case 1.0");
    assert_eq!(match_error("\"hi\""), "Unhandled match case 'hi'");
    assert_eq!(match_error("\"\""), "Unhandled match case ''");
}

#[test]
fn an_unhandled_match_cuts_a_long_string_as_a_trace_does() {
    assert_eq!(
        match_error("str_repeat(\"ab\", 30)"),
        "Unhandled match case 'abababababababa...'"
    );
}

#[test]
fn an_unhandled_match_names_the_type_of_a_non_scalar_subject() {
    // No value rendering at all for these, and in particular no `Array to
    // string conversion` on the way to one.
    assert_eq!(match_error("[1, 2]"), "Unhandled match case of type array");
    assert_eq!(
        match_error("new stdClass"),
        "Unhandled match case of type stdClass"
    );
    assert_eq!(
        match_error("new ArrayObject([])"),
        "Unhandled match case of type ArrayObject"
    );
    assert_eq!(
        match_error("(fn() => 1)"),
        "Unhandled match case of type Closure"
    );
}
