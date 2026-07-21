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
