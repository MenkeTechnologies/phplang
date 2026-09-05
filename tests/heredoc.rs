//! Heredoc (`<<<EOT`) and nowdoc (`<<<'EOT'`), including PHP 7.3's flexible
//! closing-delimiter indentation. Outputs are byte-verified against the
//! reference `php`.
//!
//! Before this the lexer had no `<<<` at all: every one of these programs was
//! `Parse error: syntax error, unexpected token "<<"`, so a construct that
//! appears in most real PHP was a hard stop rather than a divergence.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

fn err(src: &str) -> String {
    eval_capture(src).expect_err("expected a compile error")
}

#[test]
fn heredoc_interpolates_like_a_double_quoted_string() {
    let src = "<?php\n$n = 3;\n$a = [\"k\" => 5];\n$o = new stdClass; $o->p = \"P\";\necho <<<EOT\nsimple $n, sub $a[k], prop $o->p, braced {$a[\"k\"]}\nEOT;\n";
    assert_eq!(run(src), "simple 3, sub 5, prop P, braced 5");
}

#[test]
fn heredoc_processes_escapes_but_leaves_a_quote_escape_alone() {
    // `\"` is NOT an escape in a heredoc — the backslash stays, because a `"`
    // there never needed escaping. Every other escape behaves as in `"…"`.
    let src = "<?php\n$n = 1;\necho <<<EOT\n\\t|\\\\|\\$n|\\x41|\\101|\\\"|\\'\nEOT;\n";
    assert_eq!(run(src), "\t|\\|$n|A|A|\\\"|\\'");
}

#[test]
fn nowdoc_is_verbatim() {
    // Single-quoted label: no interpolation and no escape processing at all,
    // including for `\\`.
    let src = "<?php\n$n = 1;\necho <<<'EOT'\n$n {$n} \\n \\\\ \\t\nEOT;\n";
    assert_eq!(run(src), "$n {$n} \\n \\\\ \\t");
}

#[test]
fn closing_delimiter_indentation_is_stripped_from_every_line() {
    // PHP 7.3: the closing marker's indentation is removed from the body, and a
    // wholly empty line is legal at any level.
    let src = "<?php\n$x = <<<EOT\n    line1\n      line2\n\n    EOT;\nvar_dump($x);\n";
    assert_eq!(run(src), "string(14) \"line1\n  line2\n\"\n");
}

#[test]
fn a_body_line_indented_less_than_the_delimiter_is_a_parse_error() {
    let src = "<?php\n$x = <<<EOT\n  line1\n    EOT;\n";
    assert!(
        err(src).contains(
            "Invalid body indentation level (expecting an indentation level of at least 4)"
        ),
        "unexpected message: {}",
        err(src)
    );
}

#[test]
fn the_final_newline_belongs_to_the_delimiter() {
    assert_eq!(
        run("<?php\n$x = <<<EOT\na\nEOT;\nvar_dump($x);\n"),
        "string(1) \"a\"\n"
    );
    // An empty body is the empty string, not a newline.
    assert_eq!(
        run("<?php\n$x = <<<EOT\nEOT;\nvar_dump($x);\n"),
        "string(0) \"\"\n"
    );
    // A blank body line survives; only the last newline is the delimiter's.
    assert_eq!(
        run("<?php\n$x = <<<EOT\n\nEOT;\nvar_dump($x);\n"),
        "string(0) \"\"\n"
    );
}

#[test]
fn only_the_exact_label_closes_the_body() {
    // `EOTX` is a longer identifier, not the delimiter, so it stays text.
    let src = "<?php\n$x = <<<EOT\nabc\nEOTX\nEOT;\nvar_dump($x);\n";
    assert_eq!(run(src), "string(8) \"abc\nEOTX\"\n");
}

#[test]
fn a_heredoc_is_an_expression_wherever_one_is_allowed() {
    // In an argument list, in an array literal, and as an operand — the
    // delimiter ends the string, not the statement.
    let src = "<?php\necho strlen(<<<A\nabcd\nA), \"|\";\n$r = [<<<B\none\nB, <<<C\ntwo\nC];\necho implode(\",\", $r), \"|\";\necho <<<D\nx\nD . \"y\";\n";
    assert_eq!(run(src), "4|one,two|xy");
}

#[test]
fn a_quoted_label_is_still_a_heredoc() {
    let src = "<?php\n$n = 2;\necho <<<\"EOT\"\nv$n\nEOT;\n";
    assert_eq!(run(src), "v2");
}

#[test]
fn an_unterminated_body_names_the_line_the_file_ran_out_on() {
    let e = err("<?php\n$x = <<<EOT\nabc\n");
    assert!(
        e.contains("unexpected end of file") && e.contains("on line 4"),
        "unexpected message: {e}"
    );
}

#[test]
fn a_later_statement_keeps_its_own_line_number() {
    // The construct spans lines; the diagnostic below it must not be reported
    // against the line the `<<<` opened on.
    let src = "<?php\n$x = <<<EOT\na\nb\nc\nEOT;\necho $undefinedVariable;\n";
    assert!(run(src).contains("on line 7"), "{}", run(src));
}
