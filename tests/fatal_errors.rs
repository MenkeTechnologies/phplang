//! Fatal- and parse-error *rendering*: the block PHP prints when an exception
//! reaches the top uncaught, and the `syntax error, …` text it prints when the
//! source will not parse.
//!
//! Both belong to *stdout*. Under the CLI defaults the reference displays them on
//! the standard output stream — inside any open `ob_start` buffer, interleaved
//! with whatever the program already echoed — and only *additionally* logs a
//! `PHP `-prefixed copy to stderr. So the rendering is part of a program's
//! output, and every expectation below is a byte-parity assertion taken verbatim
//! from the same program under the reference `php` 8.5.9.
//!
//! `php -r` input is named `Command line code` and is all on line 1; the
//! multi-frame cases below therefore describe a `.php` file, whose name is
//! substituted so the assertions stay independent of the temp path.
//!
//! One qualification on the parse-error assertions. PHP's message often ends in
//! a `, expecting "X" or "Y"` clause naming the tokens its LALR state would have
//! accepted; that set comes out of PHP's generated parser tables and is not
//! reproducible from the grammar as written here, so it is omitted rather than
//! guessed at. The assertions below are therefore exact where the reference
//! emits no such clause (every `++`/`--`-on-a-non-variable case, which is what
//! the parity fuzzer exercises) and cover only the `unexpected <token>` half
//! where it does — that half is verified verbatim against the reference either
//! way, and each test says which case it is in.

use phplang::{compile, host, run_compiled};

/// Run `src` and return everything it wrote, *including* a fatal-error block.
///
/// `eval_capture` drops the captured output when the run fails, which is exactly
/// the case under test here, so the capture is driven directly.
fn output_of(src: &str) -> String {
    host::reset_host();
    host::with_host(|h| h.begin_capture());
    if let Ok(prog) = compile(src) {
        let _ = run_compiled(prog);
    }
    host::with_host(|h| h.end_capture())
}

/// The parser's message for `src`, which is the body of PHP's `Parse error:`
/// display line.
fn parse_error(src: &str) -> String {
    host::reset_host();
    phplang::parser::parse(src).expect_err("source must not parse")
}

// ── uncaught exceptions ──────────────────────────────────────────────────────

#[test]
fn a_top_level_uncaught_throw_renders_the_full_reference_block() {
    // php -r 'throw new Exception("boom");'
    assert_eq!(
        output_of(r#"<?php throw new Exception("boom");"#),
        "\nFatal error: Uncaught Exception: boom in Command line code:1\n\
         Stack trace:\n#0 {main}\n  thrown in Command line code on line 1\n"
    );
}

#[test]
fn an_engine_raised_error_renders_the_same_block_as_a_user_throw() {
    // php -r 'echo 1 % 0;' and php -r 'echo 1 << -1;' — both catchable Errors
    // that reach the top, so both take the ordinary uncaught path.
    assert_eq!(
        output_of("<?php echo 1 % 0;"),
        "\nFatal error: Uncaught DivisionByZeroError: Modulo by zero in Command line code:1\n\
         Stack trace:\n#0 {main}\n  thrown in Command line code on line 1\n"
    );
    assert_eq!(
        output_of("<?php echo 1 << -1;"),
        "\nFatal error: Uncaught ArithmeticError: Bit shift by negative number \
         in Command line code:1\n\
         Stack trace:\n#0 {main}\n  thrown in Command line code on line 1\n"
    );
}

#[test]
fn the_fatal_follows_output_the_program_already_produced() {
    // The block is written through the output stream, not straight to the fd, so
    // it lands after `hi` rather than racing it.
    assert_eq!(
        output_of(r#"<?php echo "hi"; throw new Exception("e");"#),
        "hi\nFatal error: Uncaught Exception: e in Command line code:1\n\
         Stack trace:\n#0 {main}\n  thrown in Command line code on line 1\n"
    );
}

#[test]
fn an_unclosed_output_buffer_is_flushed_at_shutdown_and_contains_the_fatal() {
    // php -r 'ob_start(); echo "buf"; throw new Exception("e");' — the reference
    // appends the fatal INSIDE the open buffer (an ob callback wraps it too),
    // then flushes the buffer as the request shuts down.
    assert_eq!(
        output_of(r#"<?php ob_start(); echo "buf"; throw new Exception("e");"#),
        "buf\nFatal error: Uncaught Exception: e in Command line code:1\n\
         Stack trace:\n#0 {main}\n  thrown in Command line code on line 1\n"
    );
    // The same flush happens without any fatal at all.
    assert_eq!(output_of(r#"<?php ob_start(); echo "x";"#), "x");
}

// ── the exception's own record of where it was raised ────────────────────────

#[test]
fn file_and_line_are_recorded_at_construction_not_at_the_throw() {
    // php -r '$e = new Exception("m"); throw $e;' reports line 1 for both, so the
    // distinction needs a multi-line program: PHP's getLine() is the `new` site.
    let src = "<?php\n$e = new Exception(\"m\");\n\nthrow $e;\n";
    assert_eq!(
        output_of(src),
        "\nFatal error: Uncaught Exception: m in Command line code:2\n\
         Stack trace:\n#0 {main}\n  thrown in Command line code on line 2\n"
    );
}

#[test]
fn get_line_get_file_and_get_trace_as_string_read_the_recorded_site_back() {
    assert_eq!(
        output_of(
            r#"<?php try { throw new Exception("m"); }
catch (Exception $e) { echo $e->getLine(), "|", $e->getFile(), "|", $e->getTraceAsString(); }"#
        ),
        "1|Command line code|#0 {main}"
    );
}

// ── multi-frame stack traces ─────────────────────────────────────────────────

/// Run `src` as a named script and return its output with the script path
/// replaced by `FILE`, so the assertion does not depend on the temp directory.
fn traced(src: &str) -> String {
    // Tests run in parallel, so the directory has to be unique per call — two
    // tests sharing one would race on the write and the cleanup.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("phplang-trace-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("t.php");
    std::fs::write(&path, src).expect("write script");
    let resolved = std::fs::canonicalize(&path).expect("canonicalize");

    host::reset_host();
    host::with_host(|h| {
        h.set_script_name(resolved.display().to_string());
        h.begin_capture();
    });
    if let Ok(prog) = compile(src) {
        let _ = run_compiled(prog);
    }
    let out = host::with_host(|h| h.end_capture());
    let _ = std::fs::remove_dir_all(&dir);
    out.replace(&resolved.display().to_string(), "FILE")
}

#[test]
fn each_frame_names_its_callee_and_the_line_that_called_it() {
    // Reference output for the same file, with the path masked:
    //   #0 FILE(6): inner(6)
    //   #1 FILE(9): outer(5)
    //   #2 {main}
    let src = "<?php\n\
               function inner($x) {\n\
               \x20   throw new RuntimeException(\"boom\");\n\
               }\n\
               function outer($y) {\n\
               \x20   return inner($y + 1);\n\
               }\n\
               echo \"before\\n\";\n\
               outer(5);\n";
    assert_eq!(
        traced(src),
        "before\n\
         \nFatal error: Uncaught RuntimeException: boom in FILE:3\n\
         Stack trace:\n\
         #0 FILE(6): inner(6)\n\
         #1 FILE(9): outer(5)\n\
         #2 {main}\n\
         \x20 thrown in FILE on line 3\n"
    );
}

#[test]
fn trace_arguments_use_phps_abbreviated_forms() {
    // Reference: f(1, 'short', 'a-very-long-str...', Array, NULL, 1.5) — strings
    // are single-quoted and cut to 15 characters, an array collapses to `Array`,
    // and null is spelled `NULL`.
    let src = "<?php\n\
               function f($a, $b, $c, $d, $e, $g) { throw new Exception(\"x\"); }\n\
               f(1, \"short\", \"a-very-long-string-over-15-chars\", [1,2], null, 1.5);\n";
    assert!(
        traced(src).contains("#0 FILE(3): f(1, 'short', 'a-very-long-str...', Array, NULL, 1.5)"),
        "got: {}",
        traced(src)
    );
}

#[test]
fn methods_print_arrow_or_scope_and_name_the_defining_class() {
    // Reference: `Base->m()` — the class that DEFINED the method, in its declared
    // spelling, even though the instance is a `Derived`.
    let src = "<?php\n\
               class Base { public function m() { throw new Exception(\"i\"); } }\n\
               class Derived extends Base {}\n\
               (new Derived)->m();\n";
    assert!(
        traced(src).contains("#0 FILE(4): Base->m()"),
        "got: {}",
        traced(src)
    );

    // A static call keeps `::`.
    let stat = "<?php\n\
                class A {\n\
                \x20   public static function s() { throw new Exception(\"z\"); }\n\
                }\n\
                A::s();\n";
    assert!(
        traced(stat).contains("#0 FILE(5): A::s()"),
        "got: {}",
        traced(stat)
    );
}

// ── parse errors ─────────────────────────────────────────────────────────────

#[test]
fn a_prefix_incdec_on_a_number_is_rejected_at_the_number() {
    // php -r 'echo --2;' → unexpected integer "2". A number can never begin the
    // `variable` PHP's grammar requires, so the number is the offending token.
    assert_eq!(
        parse_error("<?php echo --2;"),
        r#"syntax error, unexpected integer "2" in Command line code on line 1"#
    );
    assert_eq!(
        parse_error("<?php echo ++2;"),
        r#"syntax error, unexpected integer "2" in Command line code on line 1"#
    );
    assert_eq!(
        parse_error("<?php echo --2.5;"),
        r#"syntax error, unexpected floating-point number "2.5" in Command line code on line 1"#
    );
    // Reached through ordinary arithmetic, which is how the parity fuzzer finds
    // it: `1 - --1` is a subtraction whose right operand is a prefix decrement.
    assert_eq!(
        parse_error("<?php echo 1 - --1;"),
        r#"syntax error, unexpected integer "1" in Command line code on line 1"#
    );
}

#[test]
fn a_postfix_incdec_on_a_non_variable_is_rejected_at_the_operator() {
    // php -r 'echo 2++;' → `unexpected token "++", expecting "," or ";"`: the
    // operand is already parsed, so the operator is what the reference reports.
    // The expecting clause is the documented omission (see the module header).
    assert_eq!(
        parse_error("<?php echo 2++;"),
        r#"syntax error, unexpected token "++" in Command line code on line 1"#
    );
    assert_eq!(
        parse_error("<?php echo 2.5--;"),
        r#"syntax error, unexpected token "--" in Command line code on line 1"#
    );
}

#[test]
fn a_number_keeps_its_source_spelling_in_the_message() {
    // The value is not the token: PHP echoes back what was written, so a hex,
    // octal or exponent literal must not be re-rendered as its decimal value.
    assert_eq!(
        parse_error("<?php echo --0x1f;"),
        r#"syntax error, unexpected integer "0x1f" in Command line code on line 1"#
    );
    assert_eq!(
        parse_error("<?php echo --0755;"),
        r#"syntax error, unexpected integer "0755" in Command line code on line 1"#
    );
    assert_eq!(
        parse_error("<?php echo --2.0;"),
        r#"syntax error, unexpected floating-point number "2.0" in Command line code on line 1"#
    );
}

#[test]
fn token_kinds_are_named_the_way_the_reference_names_them() {
    // A reserved word is a `token` in its canonical spelling, whatever case it
    // was written in; a name the scanner leaves alone is an `identifier`. Every
    // case here is one the reference follows with `, expecting "," or ";"` — the
    // documented omission — so these assert the `unexpected <token>` half.
    assert_eq!(
        parse_error("<?php echo 1 RETURN;"),
        r#"syntax error, unexpected token "return" in Command line code on line 1"#
    );
    assert_eq!(
        parse_error("<?php echo 1 foo;"),
        r#"syntax error, unexpected identifier "foo" in Command line code on line 1"#
    );
    // `true`/`false`/`null` are constants, not keywords — the reference reports
    // them as identifiers.
    assert_eq!(
        parse_error("<?php echo 1 true;"),
        r#"syntax error, unexpected identifier "true" in Command line code on line 1"#
    );
    assert_eq!(
        parse_error("<?php echo 1 $v;"),
        r#"syntax error, unexpected variable "$v" in Command line code on line 1"#
    );
    assert_eq!(
        parse_error("<?php echo 1 'sq';"),
        r#"syntax error, unexpected single-quoted string "sq" in Command line code on line 1"#
    );
    assert_eq!(
        parse_error(r#"<?php echo 1 "dq";"#),
        r#"syntax error, unexpected double-quoted string "dq" in Command line code on line 1"#
    );
    // `die` is an alias the scanner folds onto the `exit` token.
    assert_eq!(
        parse_error("<?php echo 1 die;"),
        r#"syntax error, unexpected token "exit" in Command line code on line 1"#
    );
    // A magic constant keeps its uppercase canonical spelling.
    assert_eq!(
        parse_error("<?php echo 1 __class__;"),
        r#"syntax error, unexpected token "__CLASS__" in Command line code on line 1"#
    );
}

#[test]
fn running_out_of_tokens_is_reported_against_the_last_line_not_line_zero() {
    // Reference, for the same three-line file: `syntax error, unexpected end of
    // file, expecting "," or ";" … on line 3` — the line is the point here.
    assert_eq!(
        parse_error("<?php\n\necho 1"),
        "syntax error, unexpected end of file in Command line code on line 3"
    );
}
