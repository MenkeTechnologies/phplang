//! Array/`list()` destructuring assignment tests. Both the `list($a,$b) = …`
//! long form and the `[$a,$b] = …` short form lower to `Expr::Array` as an
//! assignment target; the keyed (`['k'=>$v] = …`), skipped-element (`[,$b] = …`),
//! and nested (`[[$a,$b],$c] = …`) forms are covered here. Expected strings are
//! byte-for-byte identical to the reference `php` CLI.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn short_form_positional() {
    assert_eq!(run(r#"<?php [$a, $b] = [1, 2]; echo "$a $b";"#), "1 2");
}

#[test]
fn list_long_form_positional() {
    assert_eq!(run(r#"<?php list($a, $b) = [3, 4]; echo "$a $b";"#), "3 4");
}

#[test]
fn skipped_leading_element() {
    // A hole consumes its positional index but binds nothing.
    assert_eq!(run(r#"<?php [, $b] = [5, 6]; echo $b;"#), "6");
}

#[test]
fn skipped_middle_element() {
    assert_eq!(
        run(r#"<?php [$a, , $c] = [10, 20, 30]; echo "$a $c";"#),
        "10 30"
    );
}

#[test]
fn keyed_short_form() {
    assert_eq!(
        run(r#"<?php ['x' => $vx, 'y' => $vy] = ['x' => 10, 'y' => 20]; echo "$vx $vy";"#),
        "10 20"
    );
}

#[test]
fn keyed_list_long_form() {
    assert_eq!(
        run(r#"<?php list('a' => $a, 'b' => $b) = ['a' => 'AA', 'b' => 'BB']; echo "$a $b";"#),
        "AA BB"
    );
}

#[test]
fn nested_destructuring() {
    assert_eq!(
        run(r#"<?php [[$m, $n], $o] = [[7, 8], 9]; echo "$m $n $o";"#),
        "7 8 9"
    );
}

#[test]
fn nested_keyed_destructuring() {
    assert_eq!(
        run(r#"<?php ['p' => [$a, $b]] = ['p' => [1, 2]]; echo "$a $b";"#),
        "1 2"
    );
}

#[test]
fn assignment_expression_yields_rhs() {
    // `[...] = rhs` evaluates to the whole RHS array (PHP semantics).
    assert_eq!(
        run(r#"<?php $r = ([$p, $q] = [100, 200]); echo count($r) . " " . $r[0] . " " . $r[1];"#),
        "2 100 200"
    );
}

#[test]
fn destructure_into_array_elements() {
    // Targets need not be bare variables — any lvalue works.
    assert_eq!(
        run(r#"<?php $o = []; [$o['a'], $o['b']] = [1, 2]; echo $o['a'] . " " . $o['b'];"#),
        "1 2"
    );
}

#[test]
fn foreach_with_destructuring_source() {
    assert_eq!(
        run(
            r#"<?php $pairs = [[1, 2], [3, 4]]; $out = ''; foreach ($pairs as $p) { [$x, $y] = $p; $out .= "$x-$y "; } echo trim($out);"#
        ),
        "1-2 3-4"
    );
}

#[test]
fn swap_via_destructuring() {
    assert_eq!(
        run(r#"<?php $a = 1; $b = 2; [$a, $b] = [$b, $a]; echo "$a $b";"#),
        "2 1"
    );
}

// ── `foreach` with a destructuring target ────────────────────────────────────
//
// These lower to the same `Expr::Array` target as the standalone forms above:
// the element is bound to a hidden temporary and the pattern is assigned from
// it at the head of each iteration, so keys, holes and nesting cannot drift
// apart from the `[$a, $b] = …` behaviour they share an implementation with.

#[test]
fn foreach_short_form_pattern() {
    assert_eq!(
        run(r#"<?php foreach ([[1,2],[3,4]] as [$x, $y]) { echo "$x-$y;"; }"#),
        "1-2;3-4;"
    );
}

#[test]
fn foreach_list_long_form_pattern() {
    // The two spellings must agree; `list(…)` is not a function call here.
    assert_eq!(
        run(r#"<?php foreach ([[1,2],[3,4]] as list($x, $y)) { echo "$x=$y;"; }"#),
        "1=2;3=4;"
    );
}

#[test]
fn foreach_key_with_pattern_value() {
    assert_eq!(
        run(r#"<?php foreach (['p'=>[1,2],'q'=>[3,4]] as $k => [$x, $y]) { echo "$k:$x$y;"; }"#),
        "p:12;q:34;"
    );
}

#[test]
fn foreach_keyed_pattern() {
    assert_eq!(
        run(r#"<?php foreach ([['k'=>1,'j'=>2]] as ['j'=>$v, 'k'=>$u]) { echo "$u/$v"; }"#),
        "1/2"
    );
}

#[test]
fn foreach_nested_pattern() {
    assert_eq!(
        run(r#"<?php foreach ([[1,[2,3]]] as [$p, [$q, $r]]) { echo $p, $q, $r; }"#),
        "123"
    );
}

#[test]
fn foreach_pattern_hole() {
    assert_eq!(
        run(r#"<?php foreach ([[1,2,3]] as [, $second]) { echo $second; }"#),
        "2"
    );
}

#[test]
fn foreach_pattern_over_generator() {
    // The generator loop is a separate code path from the array loop and has to
    // destructure too.
    assert_eq!(
        run(r#"<?php function g(){ yield [1,2]; yield [3,4]; } foreach (g() as [$x,$y]) { echo "$x$y;"; }"#),
        "12;34;"
    );
}

#[test]
fn foreach_pattern_short_element_warns_then_binds_null() {
    // A silent null would be a wrong answer: the reference warns FIRST and then
    // assigns null, so the diagnostic is part of the observable behaviour.
    assert_eq!(
        run(r#"<?php foreach ([[1]] as [$x, $y]) { var_dump($x, $y); }"#),
        // The warning precedes the body's output: the pattern is assigned at the
        // head of the iteration, before the first statement runs.
        "\nWarning: Undefined array key 1 in Command line code on line 1\nint(1)\nNULL\n"
    );
}

// ── the list read is not an index read ───────────────────────────────────────

#[test]
fn destructuring_a_string_is_not_a_character_read() {
    // `"ab"[0]` is 'a', but destructuring refuses to walk into a string at all.
    assert_eq!(
        run(r#"<?php [$x] = "ab"; var_dump($x);"#),
        "\nWarning: Cannot use string as array in Command line code on line 1\nNULL\n"
    );
}

#[test]
fn destructuring_null_is_silent() {
    // null is the one non-array subject the reference does NOT warn about.
    assert_eq!(run(r#"<?php [$x] = null; var_dump($x);"#), "NULL\n");
}

#[test]
fn destructuring_a_scalar_warns_with_its_type() {
    assert_eq!(
        run(r#"<?php [$x] = 5; var_dump($x);"#),
        "\nWarning: Cannot use int as array in Command line code on line 1\nNULL\n"
    );
    assert_eq!(
        run(r#"<?php [$x] = 1.5; var_dump($x);"#),
        "\nWarning: Cannot use float as array in Command line code on line 1\nNULL\n"
    );
    assert_eq!(
        run(r#"<?php [$x] = true; var_dump($x);"#),
        "\nWarning: Cannot use bool as array in Command line code on line 1\nNULL\n"
    );
}
