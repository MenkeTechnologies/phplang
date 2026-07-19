//! Regression tests for the string-comparison builtins (`strncmp`,
//! `substr_compare`) and for running real Symfony polyfill code — the CI-visible
//! counterpart to the `parity-stdlib` harness (which needs a reference `php` and
//! the fetched polyfill source, so it never runs in CI).

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn strncmp_prefixes() {
    assert_eq!(run(r#"<?php echo strncmp("hello", "help", 3);"#), "0");
    assert_eq!(
        run(r#"<?php echo strncmp("hello", "help", 4) < 0 ? "y" : "n";"#),
        "y"
    );
    assert_eq!(run(r#"<?php echo strncmp("abc", "abc", 5);"#), "0");
}

#[test]
fn substr_compare_offsets() {
    assert_eq!(run(r#"<?php echo substr_compare("hello", "lo", -2);"#), "0");
    assert_eq!(
        run(r#"<?php echo substr_compare("hello", "world", 0) < 0 ? "y" : "n";"#),
        "y"
    );
    assert_eq!(
        run(r#"<?php echo substr_compare("abcdef", "cd", 2, 2);"#),
        "0"
    );
}

/// The exact Symfony polyfill-php80 bodies (lifted verbatim, renamed `pf_*`) must
/// run on phplang. These outputs were confirmed byte-identical to reference PHP
/// by the parity-stdlib harness.
#[test]
fn symfony_str_polyfills_run() {
    let prelude = r#"<?php
        function pf_str_contains($haystack, $needle) {
            return '' === $needle || false !== strpos($haystack, $needle);
        }
        function pf_str_starts_with($haystack, $needle) {
            return 0 === strncmp($haystack, $needle, strlen($needle));
        }
        function pf_str_ends_with($haystack, $needle) {
            if ('' === $needle || $needle === $haystack) { return true; }
            if ('' === $haystack) { return false; }
            $needleLength = strlen($needle);
            return $needleLength <= strlen($haystack) && 0 === substr_compare($haystack, $needle, -$needleLength);
        }"#;
    assert_eq!(
        run(&format!(
            r#"{prelude}
            echo pf_str_contains("hello world", "o w") ? "1" : "0";
            echo pf_str_starts_with("hello", "he") ? "1" : "0";
            echo pf_str_ends_with("hello", "lo") ? "1" : "0";
            echo pf_str_contains("abc", "z") ? "1" : "0";"#
        )),
        "1110"
    );
}
