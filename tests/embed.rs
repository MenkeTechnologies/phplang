//! Tests for `eval_capture_with`, the embedder entry point: variables are seeded
//! after the host reset that starts every run, so a program can be handed input
//! without splicing an escaped literal into its source.

/// A seeded variable reads as an ordinary `$name`.
#[test]
fn seeded_vars_survive_the_host_reset() {
    let out = phplang::eval_capture_with("<?php echo strtoupper($stdin);", &[("stdin", "hello")]);
    assert_eq!(out.unwrap(), "HELLO");
}

/// Text that would otherwise be read as syntax is data, not code: no escaping
/// happens anywhere, so quotes, backslashes and `$` come back byte for byte.
#[test]
fn hostile_input_is_data_not_syntax() {
    let hostile = "a \"quoted\" \\ back $var {$x} 'single'";
    let out = phplang::eval_capture_with("<?php echo $stdin;", &[("stdin", hostile)]);
    assert_eq!(out.unwrap(), hostile);
}

/// Several variables can be bound at once, and each keeps its own value.
#[test]
fn multiple_vars_bind_independently() {
    let out =
        phplang::eval_capture_with("<?php echo $a . '/' . $b;", &[("a", "one"), ("b", "two")]);
    assert_eq!(out.unwrap(), "one/two");
}
