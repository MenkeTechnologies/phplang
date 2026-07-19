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
fn match_no_match_without_default_yields_null() {
    // Scaffold deviation: real PHP throws UnhandledMatchError; here it is null,
    // which echoes as the empty string.
    let src = r#"<?php $x = 9;
        echo "[", match ($x) { 1 => "a", 2 => "b", }, "]";"#;
    assert_eq!(run(src), "[]");
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
