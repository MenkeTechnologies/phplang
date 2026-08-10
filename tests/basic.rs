//! End-to-end tests: PHP source in, captured `echo` output out. Each case
//! exercises the compile → lower → run-on-fusevm pipeline against `PhpHost`.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn inline_html_passthrough() {
    assert_eq!(run("A<?php echo 1; ?>B"), "A1B");
}

#[test]
fn echo_and_arithmetic() {
    assert_eq!(run("<?php echo 6 * 7;"), "42");
    assert_eq!(run("<?php echo 2 + 3 * 4;"), "14"); // precedence
    assert_eq!(run("<?php echo (2 + 3) * 4;"), "20");
    assert_eq!(run("<?php echo 2 ** 10;"), "1024");
}

#[test]
fn division_is_int_when_exact_else_float() {
    assert_eq!(run("<?php echo 10 / 2;"), "5");
    assert_eq!(run("<?php echo 10 / 4;"), "2.5");
    assert_eq!(run("<?php echo 7 % 3;"), "1");
}

#[test]
fn variables_and_interpolation() {
    assert_eq!(run(r#"<?php $x = 5; echo "x=$x";"#), "x=5");
    assert_eq!(run(r#"<?php $a="foo"; $b="bar"; echo $a . $b;"#), "foobar");
}

#[test]
fn string_number_coercion() {
    // PHP 8 still uses the numeric prefix, but no longer silently: the CLI
    // writes `A non-numeric value encountered` to stdout, so it is part of the
    // output the reference produces for this program.
    assert_eq!(
        run(r#"<?php echo "12abc" + 0;"#),
        "\nWarning: A non-numeric value encountered in Command line code on line 1\n12"
    );
    assert_eq!(run(r#"<?php echo "3" . 4;"#), "34");
}

#[test]
fn comparisons_loose_and_strict() {
    assert_eq!(run(r#"<?php echo (1 == "1") ? "y" : "n";"#), "y");
    assert_eq!(run(r#"<?php echo (1 === "1") ? "y" : "n";"#), "n");
    assert_eq!(run(r#"<?php echo (0 == "a") ? "y" : "n";"#), "n"); // PHP 8
    assert_eq!(run(r#"<?php echo (2 < 10) ? "y" : "n";"#), "y");
}

#[test]
fn if_elseif_else() {
    let src = r#"<?php $n = 5;
        if ($n < 0) { echo "neg"; }
        elseif ($n == 0) { echo "zero"; }
        else { echo "pos"; }"#;
    assert_eq!(run(src), "pos");
}

#[test]
fn while_and_break_continue() {
    let src = r#"<?php $i = 0; $s = "";
        while (true) {
            $i++;
            if ($i == 3) { continue; }
            if ($i > 5) { break; }
            $s .= $i;
        }
        echo $s;"#;
    assert_eq!(run(src), "1245");
}

#[test]
fn for_loop() {
    assert_eq!(
        run(r#"<?php for ($i = 0; $i < 4; $i++) { echo $i; }"#),
        "0123"
    );
}

#[test]
fn arrays_and_foreach() {
    let src = r#"<?php $a = [1, 2, 3]; $a[] = 4;
        $t = 0;
        foreach ($a as $v) { $t += $v; }
        echo $t;"#;
    assert_eq!(run(src), "10");
}

#[test]
fn associative_foreach_with_key() {
    let src = r#"<?php $m = ["a" => 1, "b" => 2];
        $out = "";
        foreach ($m as $k => $v) { $out .= "$k=$v;"; }
        echo $out;"#;
    assert_eq!(run(src), "a=1;b=2;");
}

#[test]
fn functions_and_recursion() {
    let src = r#"<?php
        function fact($n) {
            if ($n <= 1) { return 1; }
            return $n * fact($n - 1);
        }
        echo fact(5);"#;
    assert_eq!(run(src), "120");
}

#[test]
fn library_functions() {
    assert_eq!(run(r#"<?php echo strlen("hello");"#), "5");
    assert_eq!(run(r#"<?php echo strtoupper("abc");"#), "ABC");
    assert_eq!(run(r#"<?php echo implode("-", [1, 2, 3]);"#), "1-2-3");
    assert_eq!(run(r#"<?php echo count([1, 2, 3]);"#), "3");
    assert_eq!(
        run(r#"<?php echo in_array(2, [1, 2, 3]) ? "y" : "n";"#),
        "y"
    );
    assert_eq!(run(r#"<?php echo max(3, 7, 1);"#), "7");
}

#[test]
fn division_by_zero_is_an_error() {
    assert_eq!(
        run(
            "<?php try { echo 1 / 0; } catch (Throwable $e) { echo get_class($e), ': ', $e->getMessage(); }"
        ),
        "DivisionByZeroError: Division by zero"
    );
    // The modulo form reports a different message from the division form.
    assert_eq!(
        run(
            "<?php try { echo 1 % 0; } catch (Throwable $e) { echo get_class($e), ': ', $e->getMessage(); }"
        ),
        "DivisionByZeroError: Modulo by zero"
    );
    // `fdiv` is the one that does NOT raise, so the guard cannot be global.
    assert_eq!(run("<?php echo fdiv(1, 0);"), "INF");
}
