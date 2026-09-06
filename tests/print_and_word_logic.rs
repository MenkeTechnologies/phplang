//! `print` as an expression, and the three WORD logical operators around it.
//!
//! `print` used to be readable only at the head of a statement, so `$r = print
//! "x"` was a parse error; `and`/`or` were folded onto `&&`/`||`, which binds
//! them tighter than `=` instead of looser; and `xor` was not a token at all.
//! All three live in the same precedence neighbourhood. Byte-verified against
//! PHP 8.5.10.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn print_is_an_expression_worth_one() {
    assert_eq!(run(r#"<?php $r = print "p"; var_dump($r);"#), "pint(1)\n");
    assert_eq!(run(r#"<?php var_dump(print "p");"#), "pint(1)\n");
    assert_eq!(run(r#"<?php echo print("p"), "|";"#), "p1|");
    // Nesting works because the inner `print` leaves a value behind.
    assert_eq!(run(r#"<?php print print "n";"#), "n1");
}

#[test]
fn print_binds_looser_than_assignment() {
    // `print $a = 7` prints the ASSIGNMENT, so `$a` really is 7 afterwards.
    assert_eq!(run(r#"<?php $a = 5; print $a = 7; echo "|$a";"#), "7|7");
    assert_eq!(run(r#"<?php print 1 + 2;"#), "3");
}

#[test]
fn word_logic_binds_looser_than_assignment() {
    // The whole reason `and`/`or` exist next to `&&`/`||`: `$x = a and b`
    // assigns `a` and only then ands the result.
    assert_eq!(
        run(r#"<?php $x = true and false; var_dump($x);"#),
        "bool(true)\n"
    );
    assert_eq!(
        run(r#"<?php $x = false or true; var_dump($x);"#),
        "bool(false)\n"
    );
    assert_eq!(run(r#"<?php $a = 1 and 2; var_dump($a);"#), "int(1)\n");
    // `&&` does not: it binds tighter than `=`.
    assert_eq!(
        run(r#"<?php $x = true && false; var_dump($x);"#),
        "bool(false)\n"
    );
}

#[test]
fn xor_is_a_bool_operator_with_no_short_circuit() {
    assert_eq!(
        run(r#"<?php var_dump(true xor false, true xor true, false xor false);"#),
        "bool(true)\nbool(false)\nbool(false)\n"
    );
    // Both sides always run, unlike `and`/`or`.
    let src = r#"<?php function f($v) { echo "f"; return $v; } var_dump(f(1) xor f(0));"#;
    assert_eq!(run(src), "ffbool(true)\n");
    // `or` < `xor` < `and`.
    assert_eq!(run(r#"<?php var_dump(true xor 1 and 0);"#), "bool(true)\n");
}

#[test]
fn word_logic_still_short_circuits() {
    let src = r#"<?php function f($v) { echo "f$v"; return $v; } var_dump(f(0) and f(1));"#;
    assert_eq!(run(src), "f0bool(false)\n");
    let src = r#"<?php function f($v) { echo "f$v"; return $v; } var_dump(f(1) or f(2));"#;
    assert_eq!(run(src), "f1bool(true)\n");
}

#[test]
fn print_reads_as_the_right_operand_of_word_logic() {
    assert_eq!(run(r#"<?php false or print "o";"#), "o");
    assert_eq!(
        run(r#"<?php $x = true and print "a"; var_dump($x);"#),
        "abool(true)\n"
    );
}
