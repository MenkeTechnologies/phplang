//! The SHAPE of a failure, not its wording: which Throwable class, which
//! `getCode()`, and — for the failures that are not throws at all — which
//! diagnostic LEVEL and whether execution continues.
//!
//! A right message at the wrong level is a divergence a message audit cannot
//! see. `Warning` and `Deprecated` are printed by the same code path in this
//! engine and differ only in the word, so the assertions below pin the word;
//! the `E_*` bit behind it is pinned separately by masking it off with
//! `error_reporting()` and checking the diagnostic disappears while the
//! program's own output does not.
//!
//! Reference: `php 8.5.9` (Homebrew, NTS) as `php -r`, `LC_ALL=C TZ=UTC`,
//! php.ini `/opt/homebrew/etc/php/8.5/php.ini`, `error_reporting=30719` (E_ALL),
//! `display_errors=1`, `log_errors=1`, `date.timezone` unset (effective `UTC`).

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

/// `class|message` for whatever `expr` throws, or `NOTHROW` prefixed by
/// anything the program printed on the way.
fn shape(expr: &str) -> String {
    run(&format!(
        r#"<?php try {{ {expr} echo "NOTHROW"; }} catch (\Throwable $e) {{
               echo get_class($e), "|", $e->getMessage();
           }}"#
    ))
}

// ── class, not just message ──────────────────────────────────────────────────

/// The four argument-error classes PHP distinguishes, each from a call that
/// actually raises it. Reading the message alone cannot tell them apart.
#[test]
fn the_argument_error_classes_are_distinct() {
    assert_eq!(
        shape(r#"sprintf("%d");"#),
        "ArgumentCountError|2 arguments are required, 1 given"
    );
    assert_eq!(
        shape(r#"vsprintf("%s %s", ["a"]);"#),
        "ValueError|The arguments array must contain 2 items, 1 given"
    );
    assert_eq!(
        shape("range([], []);"),
        "TypeError|range(): Argument #1 ($start) must be of type string|int|float, array given"
    );
    assert_eq!(
        shape(r#"$s = "abc"; $s["x"] = "Z";"#),
        "TypeError|Cannot access offset of type string on string"
    );
    // `ArgumentCountError extends TypeError`, so a `catch (TypeError)` sees it —
    // the hierarchy is part of the contract, not only the leaf name.
    assert_eq!(
        run(
            r#"<?php try { sprintf("%d"); } catch (\TypeError $e) { echo "caught as TypeError"; }"#
        ),
        "caught as TypeError"
    );
    // …and `ValueError` is NOT a TypeError, so the same catch must miss it.
    assert_eq!(
        run(r#"<?php try {
                   try { vsprintf("%s %s", ["a"]); }
                   catch (\TypeError $e) { echo "WRONG"; }
               } catch (\ValueError $e) { echo "fell through to ValueError"; }"#),
        "fell through to ValueError"
    );
}

/// `JsonException` carries the `JSON_ERROR_*` constant as its `getCode()`, which
/// is the only way a caller can tell a syntax error from a depth error once the
/// exception has been thrown.
#[test]
fn json_exception_carries_the_error_code() {
    for (call, code, message) in [
        (
            r#"json_decode("{", false, 512, JSON_THROW_ON_ERROR)"#,
            4,
            "Syntax error",
        ),
        (
            r#"json_decode("[[1]]", true, 1, JSON_THROW_ON_ERROR)"#,
            1,
            "Maximum stack depth exceeded",
        ),
        (
            "json_encode(NAN, JSON_THROW_ON_ERROR)",
            7,
            "Inf and NaN cannot be JSON encoded",
        ),
    ] {
        assert_eq!(
            run(&format!(
                r#"<?php try {{ {call}; }} catch (\Throwable $e) {{
                       printf("%s|%d|%s", get_class($e), $e->getCode(), $e->getMessage());
                   }}"#
            )),
            format!("JsonException|{code}|{message}"),
            "{call}"
        );
    }
    // `JsonException extends Exception`, NOT Error — a `catch (Error)` misses it.
    assert_eq!(
        run(
            r#"<?php try { json_decode("{", false, 512, JSON_THROW_ON_ERROR); }
               catch (\Error $e) { echo "WRONG"; }
               catch (\Exception $e) { echo "Exception"; }"#
        ),
        "Exception"
    );
}

/// Every other Throwable this engine raises has code 0 and no previous — the
/// engine never populates either, and a test that only read `getMessage()`
/// could not have noticed if it had.
#[test]
fn engine_raised_throwables_carry_code_zero_and_no_previous() {
    for expr in [
        "intdiv(1, 0);",
        "1 % 0;",
        "range(1, 5, 0);",
        r#"array_fill(0, -1, "x");"#,
        r#"$x = null; $x();"#,
    ] {
        assert_eq!(
            run(&format!(
                r#"<?php try {{ {expr} }} catch (\Throwable $e) {{
                       printf("%s|%s", var_export($e->getCode(), true),
                              $e->getPrevious() === null ? "null" : "set");
                   }}"#
            )),
            "0|null",
            "{expr}"
        );
    }
}

// ── level, not just text: throw vs warn vs deprecate vs continue ─────────────

/// These emit a diagnostic and KEEP GOING. Getting the level right matters as
/// much as the text: a `Deprecated` that is printed as a `Warning` survives a
/// message audit but breaks `error_reporting(E_ALL & ~E_DEPRECATED)`.
#[test]
fn a_deprecation_prints_deprecated_and_execution_continues() {
    let cases: &[(&str, &str)] = &[
        (
            "chr(-1);",
            "chr(): Providing a value not in-between 0 and 255 is deprecated, this is because \
             a byte value must be in the [0, 255] interval. The value used will be constrained \
             using % 256",
        ),
        (
            "ord(\"\");",
            "ord(): Providing an empty string is deprecated",
        ),
        (
            "bindec('1a0b1');",
            "Invalid characters passed for attempted conversion, these have been ignored",
        ),
        (
            "hexdec('zz');",
            "Invalid characters passed for attempted conversion, these have been ignored",
        ),
        (
            "octdec('9');",
            "Invalid characters passed for attempted conversion, these have been ignored",
        ),
        (
            "pow(0, -1);",
            "Power of base 0 and negative exponent is deprecated",
        ),
    ];
    for (call, msg) in cases {
        assert_eq!(
            run(&format!("<?php {call} echo \"CONTINUED\";")),
            format!("\nDeprecated: {msg} in Command line code on line 1\nCONTINUED"),
            "{call}"
        );
        // The `E_DEPRECATED` bit is what gates it: masked off, the diagnostic
        // goes away and the program still runs to the end.
        assert_eq!(
            run(&format!(
                "<?php error_reporting(E_ALL & ~E_DEPRECATED); {call} echo \"CONTINUED\";"
            )),
            "CONTINUED",
            "{call} under E_ALL & ~E_DEPRECATED"
        );
        // Masking off E_WARNING instead must NOT silence it — that is the check
        // that tells a Deprecated apart from a Warning with the same words.
        assert_eq!(
            run(&format!(
                "<?php error_reporting(E_ALL & ~E_WARNING); {call} echo \"CONTINUED\";"
            )),
            format!("\nDeprecated: {msg} in Command line code on line 1\nCONTINUED"),
            "{call} under E_ALL & ~E_WARNING"
        );
    }
}

/// The same discrimination for the E_WARNING family, including the two the
/// recursion guards added.
#[test]
fn a_warning_prints_warning_and_execution_continues() {
    let cases: &[(&str, &str)] = &[
        (r#"implode(",", [1, [2]]);"#, "Array to string conversion"),
        (
            r#"$s = "abc"; $s[1.7] = "Z";"#,
            "String offset cast occurred",
        ),
        (r#"$s = "abc"; $s[-10] = "Z";"#, "Illegal string offset -10"),
        (
            "$a = [1]; $a[] = &$a; var_export($a);",
            "var_export does not handle circular references",
        ),
        (
            "$a = [1]; $a[] = &$a; count($a, COUNT_RECURSIVE);",
            "count(): Recursion detected",
        ),
        (
            r#"sprintf("%.60f", 1.0);"#,
            "sprintf(): Requested precision of 60 digits was truncated to PHP maximum of \
             53 digits",
        ),
    ];
    for (call, msg) in cases {
        let out = run(&format!("<?php {call} echo \"|CONTINUED\";"));
        // `var_export` prints as well, so the diagnostic is asserted as a prefix
        // and the continuation as a suffix.
        let expected_head = format!("\n{}: {msg} in Command line code on line 1\n", level(msg));
        assert!(
            out.starts_with(&expected_head),
            "{call}\n  expected to start with {expected_head:?}\n  got {out:?}"
        );
        assert!(
            out.ends_with("|CONTINUED"),
            "{call} did not continue: {out:?}"
        );
    }
}

/// `sprintf`'s precision truncation is the one E_NOTICE in the set above; the
/// rest are E_WARNING. The level is what decides which `error_reporting` mask
/// hides it.
fn level(msg: &str) -> &'static str {
    if msg.starts_with("sprintf(): Requested precision") {
        "Notice"
    } else {
        "Warning"
    }
}

/// Masking the exact bit is what proves the level, so each of the three is
/// checked against its OWN mask.
#[test]
fn each_diagnostic_is_gated_by_its_own_error_reporting_bit() {
    // E_NOTICE (8) hides the precision truncation; E_WARNING (2) does not.
    assert_eq!(
        run(
            r#"<?php error_reporting(E_ALL & ~E_NOTICE); var_dump(strlen(sprintf("%.60f", 1.0)));"#
        ),
        "int(55)\n"
    );
    assert!(
        run(r#"<?php error_reporting(E_ALL & ~E_WARNING); sprintf("%.60f", 1.0); echo "x";"#)
            .contains("Notice: sprintf(): Requested precision"),
        "E_WARNING must not gate an E_NOTICE"
    );
    // E_WARNING (2) hides the array-to-string conversion; E_NOTICE does not.
    assert_eq!(
        run(r#"<?php error_reporting(E_ALL & ~E_WARNING); echo implode(",", [1, [2]]);"#),
        "1,Array"
    );
    assert!(
        run(r#"<?php error_reporting(E_ALL & ~E_NOTICE); echo implode(",", [1, [2]]);"#)
            .contains("Warning: Array to string conversion"),
        "E_NOTICE must not gate an E_WARNING"
    );
}

// ── throw where the engine used to continue, continue where it used to throw ──

/// A string offset write edits the string. It previously replaced the whole
/// variable with a one-element ARRAY, which is a silent wrong answer rather than
/// any kind of diagnostic.
#[test]
fn writing_a_string_offset_edits_the_string() {
    assert_eq!(
        run(r#"<?php $s="abc"; $s[1]="Z"; var_dump($s);"#),
        "string(3) \"aZc\"\n"
    );
    // Past the end the gap is padded with SPACES, not left unset.
    assert_eq!(
        run(r#"<?php $s="abc"; $s[5]="Z"; var_dump($s);"#),
        "string(6) \"abc  Z\"\n"
    );
    // A negative offset counts from the end…
    assert_eq!(
        run(r#"<?php $s="abc"; $s[-1]="Z"; var_dump($s);"#),
        "string(3) \"abZ\"\n"
    );
    // …and one that reaches past the start warns and changes nothing.
    assert_eq!(
        run(r#"<?php $s="abc"; $s[-10]="Z"; var_dump($s);"#),
        "\nWarning: Illegal string offset -10 in Command line code on line 1\nstring(3) \"abc\"\n"
    );
    // Only the first byte of the replacement is used, and PHP says so.
    assert_eq!(
        run(r#"<?php $s="abc"; $s[1]="XY"; var_dump($s);"#),
        "\nWarning: Only the first byte will be assigned to the string offset in \
         Command line code on line 1\nstring(3) \"aXc\"\n"
    );
    // An empty replacement has no byte to assign — a catchable Error.
    assert_eq!(
        shape(r#"$s="abc"; $s[1]="";"#),
        "Error|Cannot assign an empty string to a string offset"
    );
    // An empty string is still a STRING here; only null auto-vivifies an array.
    assert_eq!(
        run(r#"<?php $s=""; $s[0]="a"; var_dump($s);"#),
        "string(1) \"a\"\n"
    );
    assert_eq!(
        run(r#"<?php $s=null; $s[0]="a"; var_dump($s);"#),
        "array(1) {\n  [0]=>\n  string(1) \"a\"\n}\n"
    );
}

/// Argument validation that used to be absent entirely: the call returned a
/// plausible value instead of reporting that it could not be answered.
#[test]
fn arguments_outside_their_domain_are_reported_rather_than_guessed() {
    assert_eq!(
        shape(r#"str_word_count("abc", 9);"#),
        "ValueError|str_word_count(): Argument #2 ($format) must be a valid format value"
    );
    assert_eq!(
        shape(r#"wordwrap("abc", 0, "\n", true);"#),
        "ValueError|wordwrap(): Argument #4 ($cut_long_words) cannot be true when \
         argument #2 ($width) is 0"
    );
    assert_eq!(
        shape(r#"mb_convert_encoding("abc", "NOPE");"#),
        r#"ValueError|mb_convert_encoding(): Argument #2 ($to_encoding) must be a valid encoding, "NOPE" given"#
    );
    assert_eq!(
        shape("array_rand([], 1);"),
        "ValueError|array_rand(): Argument #1 ($array) must not be empty"
    );
    // A non-empty array with a bad `$num` names argument #2 instead — the two
    // messages are different and the engine used to give #2 for both.
    assert_eq!(
        shape("array_rand([1, 2], 5);"),
        "ValueError|array_rand(): Argument #2 ($num) must be between 1 and the number of \
         elements in argument #1 ($array)"
    );
    // The valid forms still work.
    assert_eq!(run(r#"<?php echo str_word_count("a b c");"#), "3");
    assert_eq!(
        run(r#"<?php echo wordwrap("abc def", 3, "|", true);"#),
        "abc|def"
    );
    assert_eq!(
        run(r#"<?php echo mb_convert_encoding("abc", "UTF-8");"#),
        "abc"
    );
}

/// `sprintf` rejects a conversion character it has no case for, and reports a
/// missing one differently from an unknown one. `%i` is the trap: it is a C
/// spelling PHP does not accept.
#[test]
fn sprintf_rejects_unknown_and_missing_conversions() {
    assert_eq!(
        shape(r#"sprintf("%z", 1);"#),
        r#"ValueError|Unknown format specifier "z""#
    );
    assert_eq!(
        shape(r#"sprintf("%i", 1);"#),
        r#"ValueError|Unknown format specifier "i""#
    );
    assert_eq!(
        shape(r#"sprintf("%l", 1);"#),
        "ValueError|Missing format specifier at end of string"
    );
    // A `%` with no argument left reports the ARGUMENT, not the specifier: the
    // reference looks for the argument first and skips the switch entirely.
    assert_eq!(
        shape(r#"sprintf("%");"#),
        "ArgumentCountError|2 arguments are required, 1 given"
    );
    // `l` is a length modifier and is swallowed, so `%ld` is `%d`.
    assert_eq!(run(r#"<?php echo sprintf("%ld", 5);"#), "5");
    // `%h`/`%H` are the locale-independent `%g`/`%G`.
    assert_eq!(run(r#"<?php echo sprintf("%h|%H", 65, 65);"#), "65|65");
}

// ── the trace is part of the shape ───────────────────────────────────────────

/// A trace argument is escaped and truncated, and a whole-valued float keeps its
/// `.0` so it stays distinguishable from an int.
#[test]
fn trace_arguments_are_escaped_and_truncated() {
    let trace = |call: &str| {
        run(&format!(
            r#"<?php function f($s) {{ throw new Error("x"); }}
               try {{ {call} }} catch (\Throwable $e) {{ echo $e->getTraceAsString(); }}"#
        ))
    };
    assert_eq!(
        trace(r#"f("a\nb");"#),
        "#0 Command line code(2): f('a\\nb')\n#1 {main}"
    );
    assert_eq!(
        trace(r#"f("a\tb\x01c");"#),
        "#0 Command line code(2): f('a\\tb\\x01c')\n#1 {main}"
    );
    assert_eq!(
        trace(r#"f("a\\b");"#),
        "#0 Command line code(2): f('a\\\\b')\n#1 {main}"
    );
    // A single quote is NOT escaped, even inside the single-quoted rendering.
    assert_eq!(
        trace(r#"f("a'b");"#),
        "#0 Command line code(2): f('a'b')\n#1 {main}"
    );
    // Truncation is at 15 bytes and appends `...` inside the quotes.
    assert_eq!(
        trace(r#"f("0123456789abcdefghij");"#),
        "#0 Command line code(2): f('0123456789abcde...')\n#1 {main}"
    );
    // A float argument keeps a fractional part; an int does not gain one.
    assert_eq!(
        trace("f(1.0);"),
        "#0 Command line code(2): f(1.0)\n#1 {main}"
    );
    assert_eq!(trace("f(1);"), "#0 Command line code(2): f(1)\n#1 {main}");
    assert_eq!(
        trace("f(1e100);"),
        "#0 Command line code(2): f(1.0E+100)\n#1 {main}"
    );
    assert_eq!(
        trace("f(NAN);"),
        "#0 Command line code(2): f(NAN)\n#1 {main}"
    );
}
