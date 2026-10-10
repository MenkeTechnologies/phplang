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
    // case here is a stray token after an `echo` operand, which the reference
    // follows with `, expecting "," or ";"`.
    assert_eq!(
        parse_error("<?php echo 1 RETURN;"),
        r#"syntax error, unexpected token "return", expecting "," or ";" in Command line code on line 1"#
    );
    assert_eq!(
        parse_error("<?php echo 1 foo;"),
        r#"syntax error, unexpected identifier "foo", expecting "," or ";" in Command line code on line 1"#
    );
    // `true`/`false`/`null` are constants, not keywords — the reference reports
    // them as identifiers.
    assert_eq!(
        parse_error("<?php echo 1 true;"),
        r#"syntax error, unexpected identifier "true", expecting "," or ";" in Command line code on line 1"#
    );
    assert_eq!(
        parse_error("<?php echo 1 $v;"),
        r#"syntax error, unexpected variable "$v", expecting "," or ";" in Command line code on line 1"#
    );
    assert_eq!(
        parse_error("<?php echo 1 'sq';"),
        r#"syntax error, unexpected single-quoted string "sq", expecting "," or ";" in Command line code on line 1"#
    );
    assert_eq!(
        parse_error(r#"<?php echo 1 "dq";"#),
        r#"syntax error, unexpected double-quoted string "dq", expecting "," or ";" in Command line code on line 1"#
    );
    // `die` is an alias the scanner folds onto the `exit` token.
    assert_eq!(
        parse_error("<?php echo 1 die;"),
        r#"syntax error, unexpected token "exit", expecting "," or ";" in Command line code on line 1"#
    );
    // A magic constant keeps its uppercase canonical spelling.
    assert_eq!(
        parse_error("<?php echo 1 __class__;"),
        r#"syntax error, unexpected token "__CLASS__", expecting "," or ";" in Command line code on line 1"#
    );
}

#[test]
fn running_out_of_tokens_is_reported_against_the_last_line_not_line_zero() {
    // Reference, for the same three-line file: `syntax error, unexpected end of
    // file, expecting "," or ";" … on line 3` — the line is the point here.
    assert_eq!(
        parse_error("<?php\n\necho 1"),
        "syntax error, unexpected end of file, expecting \",\" or \";\" in Command line code on line 3"
    );
}

// ── library argument errors ──────────────────────────────────────────────────

#[test]
fn a_library_argument_error_throws_with_the_library_call_as_frame_zero() {
    // php -r 'echo implode(",", range(9, 10, 2));' — a step wider than the span
    // is a ValueError, and the trace names the internal call with its arguments.
    assert_eq!(
        output_of(r#"<?php echo implode(",", range(9, 10, 2));"#),
        "\nFatal error: Uncaught ValueError: range(): Argument #3 ($step) must be less than \
         the range spanned by argument #1 ($start) and argument #2 ($end) in Command line code:1\n\
         Stack trace:\n#0 Command line code(1): range(9, 10, 2)\n#1 {main}\n  \
         thrown in Command line code on line 1\n"
    );
}

#[test]
fn the_internal_frame_stacks_under_the_user_frames_that_reached_it() {
    // php -r 'function f() { return range(9, 10, 2); } f();' — the internal frame
    // is #0 and the PHP function that called it is #1.
    assert_eq!(
        output_of(r#"<?php function f() { return range(9, 10, 2); } f();"#),
        "\nFatal error: Uncaught ValueError: range(): Argument #3 ($step) must be less than \
         the range spanned by argument #1 ($start) and argument #2 ($end) in Command line code:1\n\
         Stack trace:\n#0 Command line code(1): range(9, 10, 2)\n#1 Command line code(1): f()\n\
         #2 {main}\n  thrown in Command line code on line 1\n"
    );
}

#[test]
fn the_internal_frame_renders_its_arguments_the_way_every_other_frame_does() {
    // Long strings are cut to 15 characters inside the quotes and an array
    // collapses to `Array` — the same `trace_arg` rendering a user frame uses.
    assert!(
        output_of(r#"<?php range("aaaaaaaaaaaaaaaaaaaaaaa", "b", 99);"#)
            .contains("#0 Command line code(1): range('aaaaaaaaaaaaaaa...', 'b', 99)")
    );
    assert!(output_of(r#"<?php array_combine([1, 2], [1]);"#)
        .contains("#0 Command line code(1): array_combine(Array, Array)"));
}

#[test]
fn a_sensitive_parameter_never_reaches_the_trace() {
    // PHP marks `hash_hmac`'s `$key` `#[\SensitiveParameter]` so a key cannot
    // leak into an error log; the trace shows the wrapper object instead.
    let out = output_of(r#"<?php hash_hmac("nope", "data", "SECRET-KEY");"#);
    assert!(
        out.contains(
            "#0 Command line code(1): hash_hmac('nope', 'data', Object(SensitiveParameterValue))"
        ),
        "trace was: {out}"
    );
    assert!(!out.contains("SECRET-KEY"), "the key leaked: {out}");
}

#[test]
fn a_library_argument_error_is_catchable_and_carries_its_own_site() {
    // The whole point of throwing rather than aborting: a `catch` sees it, and
    // `getLine`/`getFile` report the CALL, not something internal.
    assert_eq!(
        output_of(
            r#"<?php try { range(9, 10, 2); } catch (ValueError $e) { echo get_class($e), "|", $e->getLine(), "|", $e->getFile(); }"#
        ),
        "ValueError|1|Command line code"
    );
    // A `DivisionByZeroError` from bcmath carries PHP's unprefixed message.
    assert_eq!(
        output_of(
            r#"<?php try { bcdiv("1", "0"); } catch (Throwable $e) { echo get_class($e), "|", $e->getMessage(); }"#
        ),
        "DivisionByZeroError|Division by zero"
    );
}

#[test]
fn a_const_declaration_outside_top_level_is_rejected_at_the_const() {
    // `const` is a TOP-LEVEL statement in PHP's grammar. Inside a function body
    // or any other block the reference reports the `const` token itself as
    // unexpected, with no `expecting …` clause, so these are exact:
    //   php -r 'if(true){ const A=1; }'      → unexpected token "const"
    //   php -r 'function f(){ const A=1; }'  → unexpected token "const"
    let want = r#"syntax error, unexpected token "const" in Command line code on line 1"#;
    assert_eq!(parse_error("<?php if (true) { const A = 1; }"), want);
    assert_eq!(parse_error("<?php function f() { const A = 1; }"), want);
    assert_eq!(parse_error("<?php while (false) { const A = 1; }"), want);
    assert_eq!(
        parse_error("<?php foreach ([1] as $x) { const A = 1; }"),
        want
    );
    assert_eq!(
        parse_error("<?php try { const A = 1; } catch (Exception $e) {}"),
        want
    );
    // The restriction is on PLACEMENT, not on the keyword: the same declaration
    // parses at file scope, and `namespace Name { }` does not leave top level.
    // (A class body's `const` is a different production entirely.)
    // Asserted by RUNNING them, not by `parse(...).is_ok()`: a parse that
    // succeeded but dropped the declaration would satisfy `is_ok()` while
    // leaving the constant undefined, which is the failure this negative control
    // exists to rule out.
    assert_eq!(
        phplang::eval_capture("<?php const A = 1; echo A;").unwrap(),
        "1"
    );
    assert_eq!(
        phplang::eval_capture("<?php namespace N { const A = 1; echo A; }").unwrap(),
        "1"
    );
    assert_eq!(
        phplang::eval_capture("<?php class K { const A = 1; } echo K::A;").unwrap(),
        "1"
    );
}

// ── syntax-error expectation lists ───────────────────────────────────────────

#[test]
fn syntax_errors_carry_the_reference_expecting_clause_and_scanner_nesting_errors() {
    // Every row was read off reference php 8.5: a stray token where the grammar
    // leaves a closer or terminator open (`expecting "," or ";"`), and the
    // scanner-level bracket diagnostics, which no parser table decides.
    let rows: &[(&str, &str)] = &[
        ("<?php echo 1 2;", "syntax error, unexpected integer \"2\", expecting \",\" or \";\" in Command line code on line 1"),
        ("<?php echo 1, 2 3;", "syntax error, unexpected integer \"3\", expecting \",\" or \";\" in Command line code on line 1"),
        ("<?php return 1 2;", "syntax error, unexpected integer \"2\", expecting \";\" in Command line code on line 1"),
        ("<?php break 1 2;", "syntax error, unexpected integer \"2\", expecting \";\" in Command line code on line 1"),
        ("<?php continue 1 2;", "syntax error, unexpected integer \"2\", expecting \";\" in Command line code on line 1"),
        ("<?php global $a $b;", "syntax error, unexpected variable \"$b\", expecting \",\" or \";\" in Command line code on line 1"),
        ("<?php static $a $b;", "syntax error, unexpected variable \"$b\", expecting \",\" or \";\" in Command line code on line 1"),
        ("<?php static $a = 1 $b;", "syntax error, unexpected variable \"$b\", expecting \",\" or \";\" in Command line code on line 1"),
        ("<?php const A = 1 2;", "syntax error, unexpected integer \"2\", expecting \",\" or \";\" in Command line code on line 1"),
        ("<?php const A = 1, B = 2 3;", "syntax error, unexpected integer \"3\", expecting \",\" or \";\" in Command line code on line 1"),
        ("<?php use A\\B C;", "syntax error, unexpected identifier \"C\", expecting \",\" or \";\" in Command line code on line 1"),
        ("<?php namespace A B;", "syntax error, unexpected identifier \"B\", expecting \"{\" in Command line code on line 1"),
        ("<?php goto a b;", "syntax error, unexpected identifier \"b\", expecting \";\" in Command line code on line 1"),
        ("<?php class A { public $a $b; }", "syntax error, unexpected variable \"$b\", expecting \",\" or \";\" in Command line code on line 1"),
        ("<?php class A { public $a = 1 $b; }", "syntax error, unexpected variable \"$b\", expecting \",\" or \";\" in Command line code on line 1"),
        ("<?php class A { const X = 1 2; }", "syntax error, unexpected integer \"2\", expecting \",\" or \";\" in Command line code on line 1"),
        ("<?php class A { use B C; }", "syntax error, unexpected identifier \"C\", expecting \",\" or \";\" or \"{\" in Command line code on line 1"),
        ("<?php class A { function f() {} 1 }", "syntax error, unexpected integer \"1\", expecting \"function\" in Command line code on line 1"),
        ("<?php class A extends B C {}", "syntax error, unexpected identifier \"C\", expecting \"{\" in Command line code on line 1"),
        ("<?php class A implements B C {}", "syntax error, unexpected identifier \"C\", expecting \"{\" in Command line code on line 1"),
        ("<?php interface I extends J K {}", "syntax error, unexpected identifier \"K\", expecting \"{\" in Command line code on line 1"),
        ("<?php enum E { case A case B; }", "syntax error, unexpected token \"case\", expecting \";\" in Command line code on line 1"),
        ("<?php enum E: int { case A = 1 2; }", "syntax error, unexpected integer \"2\", expecting \";\" in Command line code on line 1"),
        ("<?php function f(): int string {}", "syntax error, unexpected identifier \"string\", expecting \"{\" in Command line code on line 1"),
        ("<?php function f($a = 1 2) {}", "syntax error, unexpected integer \"2\", expecting \")\" in Command line code on line 1"),
        ("<?php function f(int $a 2) {}", "syntax error, unexpected integer \"2\", expecting \")\" in Command line code on line 1"),
        ("<?php fn($x 2) => 1;", "syntax error, unexpected integer \"2\", expecting \")\" in Command line code on line 1"),
        ("<?php $x = fn($a) 1;", "syntax error, unexpected integer \"1\", expecting \"=>\" in Command line code on line 1"),
        ("<?php $x = function() 1;", "syntax error, unexpected integer \"1\", expecting \"{\" in Command line code on line 1"),
        ("<?php $x = list($a, $b) 1;", "syntax error, unexpected integer \"1\", expecting \"=\" in Command line code on line 1"),
        ("<?php $x = f(a: 1 2);", "syntax error, unexpected integer \"2\", expecting \")\" in Command line code on line 1"),
        ("<?php $x = f(...$a 2);", "syntax error, unexpected integer \"2\", expecting \")\" in Command line code on line 1"),
        ("<?php $x = [...$a 2];", "syntax error, unexpected integer \"2\", expecting \"]\" in Command line code on line 1"),
        ("<?php $x = [1 => 2 3];", "syntax error, unexpected integer \"3\", expecting \"]\" in Command line code on line 1"),
        ("<?php $x = [1 => 2, 3 => 4 5];", "syntax error, unexpected integer \"5\", expecting \"]\" in Command line code on line 1"),
        ("<?php $x = $a[1 2];", "syntax error, unexpected integer \"2\", expecting \"]\" in Command line code on line 1"),
        ("<?php $x = $a[1][2 3];", "syntax error, unexpected integer \"3\", expecting \"]\" in Command line code on line 1"),
        ("<?php $x = \"{$a[1 2]}\";", "syntax error, unexpected integer \"2\", expecting \"]\" in Command line code on line 1"),
        ("<?php foreach ($a as $b $c) {}", "syntax error, unexpected variable \"$c\", expecting \"->\" or \"?->\" or \"[\" in Command line code on line 1"),
        ("<?php foreach ($a as $b => $c $d) {}", "syntax error, unexpected variable \"$d\", expecting \"->\" or \"?->\" or \"[\" in Command line code on line 1"),
        ("<?php for ($i = 0 $i < 3; $i++) {}", "syntax error, unexpected variable \"$i\", expecting \";\" in Command line code on line 1"),
        ("<?php for ($i = 0; $i < 3; $i++ $j) {}", "syntax error, unexpected variable \"$j\", expecting \")\" in Command line code on line 1"),
        ("<?php try {} catch (A B $c) {}", "syntax error, unexpected identifier \"B\", expecting \")\" in Command line code on line 1"),
        ("<?php try {} catch (A $c $d) {}", "syntax error, unexpected variable \"$d\", expecting \")\" in Command line code on line 1"),
        ("<?php try {} finally 1", "syntax error, unexpected integer \"1\", expecting \"{\" in Command line code on line 1"),
        ("<?php declare(strict_types=1 2);", "syntax error, unexpected integer \"2\", expecting \",\" or \")\" in Command line code on line 1"),
        ("<?php unset($a $b);", "syntax error, unexpected variable \"$b\", expecting \"->\" or \"?->\" or \"[\" in Command line code on line 1"),
        ("<?php unset($a, $b $c);", "syntax error, unexpected variable \"$c\", expecting \"->\" or \"?->\" or \"[\" in Command line code on line 1"),
        ("<?php isset($a, $b $c);", "syntax error, unexpected variable \"$c\", expecting \")\" in Command line code on line 1"),
        ("<?php $x = match($a) { 1 => 2 3 => 4 };", "syntax error, unexpected integer \"3\", expecting \"}\" in Command line code on line 1"),
        ("<?php $x = match($a) { 1, 2 => 3 4 };", "syntax error, unexpected integer \"4\", expecting \"}\" in Command line code on line 1"),
        ("<?php $x = new;", "syntax error, unexpected token \";\", expecting \"class\" in Command line code on line 1"),
        ("<?php class A { function f() { return 1 2; } }", "syntax error, unexpected integer \"2\", expecting \";\" in Command line code on line 1"),
        ("<?php abstract class { }", "syntax error, unexpected token \"{\", expecting identifier in Command line code on line 1"),
        ("<?php class A { public }", "syntax error, unexpected token \"}\", expecting variable in Command line code on line 1"),
        ("<?php class A { function f( }", "Unclosed '(' does not match '}' in Command line code on line 1"),
        ("<?php function ( {}", "syntax error, unexpected token \"{\", expecting variable in Command line code on line 1"),
        ("<?php class A { function f() 1 }", "syntax error, unexpected integer \"1\", expecting \";\" or \"{\" in Command line code on line 1"),
        ("<?php class A { function f(): int 1 }", "syntax error, unexpected integer \"1\", expecting \";\" or \"{\" in Command line code on line 1"),
        ("<?php class A { abstract function f() 1 }", "syntax error, unexpected integer \"1\", expecting \";\" or \"{\" in Command line code on line 1"),
        ("<?php interface I { function f() 1 }", "syntax error, unexpected integer \"1\", expecting \";\" or \"{\" in Command line code on line 1"),
        ("<?php class A { function f }", "syntax error, unexpected token \"}\", expecting \"(\" in Command line code on line 1"),
        ("<?php trait T { function f() 1 }", "syntax error, unexpected integer \"1\", expecting \";\" or \"{\" in Command line code on line 1"),
        ("<?php $x = function() use ($a) 1;", "syntax error, unexpected integer \"1\", expecting \"{\" in Command line code on line 1"),
        ("<?php $x = function() use 1 {};", "syntax error, unexpected integer \"1\", expecting \"(\" in Command line code on line 1"),
        ("<?php function f() 1", "syntax error, unexpected integer \"1\", expecting \"{\" in Command line code on line 1"),
        ("<?php function f", "syntax error, unexpected end of file, expecting \"(\" in Command line code on line 1"),
        ("<?php function", "syntax error, unexpected end of file, expecting \"(\" in Command line code on line 1"),
        ("<?php class", "syntax error, unexpected end of file, expecting identifier in Command line code on line 1"),
        ("<?php interface", "syntax error, unexpected end of file, expecting identifier in Command line code on line 1"),
        ("<?php trait", "syntax error, unexpected end of file, expecting identifier in Command line code on line 1"),
        ("<?php namespace;", "syntax error, unexpected token \";\", expecting \"{\" in Command line code on line 1"),
        ("<?php namespace A\\B C;", "syntax error, unexpected identifier \"C\", expecting \"{\" in Command line code on line 1"),
        ("<?php use function A\\f B;", "syntax error, unexpected identifier \"B\", expecting \",\" or \";\" in Command line code on line 1"),
        ("<?php use const A\\B C;", "syntax error, unexpected identifier \"C\", expecting \",\" or \";\" in Command line code on line 1"),
        ("<?php try 1", "syntax error, unexpected integer \"1\", expecting \"{\" in Command line code on line 1"),
        ("<?php try {} catch 1 {}", "syntax error, unexpected integer \"1\", expecting \"(\" in Command line code on line 1"),
        ("<?php try {} catch (A) 1", "syntax error, unexpected integer \"1\", expecting \"{\" in Command line code on line 1"),
        ("<?php do {} while 1;", "syntax error, unexpected integer \"1\", expecting \"(\" in Command line code on line 1"),
        ("<?php switch (1) 2", "syntax error, unexpected integer \"2\", expecting \":\" or \"{\" in Command line code on line 1"),
        ("<?php switch (1) { 2 }", "syntax error, unexpected integer \"2\", expecting \"case\" or \"default\" or \"}\" in Command line code on line 1"),
        ("<?php switch (1) { default 2: }", "syntax error, unexpected integer \"2\", expecting \":\" or \";\" in Command line code on line 1"),
        ("<?php declare(ticks);", "syntax error, unexpected token \")\", expecting \"=\" in Command line code on line 1"),
        ("<?php $a = [1, 2 3, 4];", "syntax error, unexpected integer \"3\", expecting \"]\" in Command line code on line 1"),
        ("<?php $a = [1, 2, 3 4];", "syntax error, unexpected integer \"4\", expecting \"]\" in Command line code on line 1"),
        ("<?php $a = ['a' => 1 'b' => 2];", "syntax error, unexpected single-quoted string \"b\", expecting \"]\" in Command line code on line 1"),
        ("<?php $a = [[1 2]];", "syntax error, unexpected integer \"2\", expecting \"]\" in Command line code on line 1"),
        ("<?php f(f(1 2));", "syntax error, unexpected integer \"2\", expecting \")\" in Command line code on line 1"),
        ("<?php f([1 2]);", "syntax error, unexpected integer \"2\", expecting \"]\" in Command line code on line 1"),
        ("<?php f(1, g(2 3));", "syntax error, unexpected integer \"3\", expecting \")\" in Command line code on line 1"),
        ("<?php $a = f(1)[2 3];", "syntax error, unexpected integer \"3\", expecting \"]\" in Command line code on line 1"),
        ("<?php $a->b(1 2)->c;", "syntax error, unexpected integer \"2\", expecting \")\" in Command line code on line 1"),
        ("<?php $a = new A(1, 2 3);", "syntax error, unexpected integer \"3\", expecting \")\" in Command line code on line 1"),
        ("<?php foo(1 {", "syntax error, unexpected token \"{\", expecting \")\" in Command line code on line 1"),
        ("<?php foo(1 { }", "syntax error, unexpected token \"{\", expecting \")\" in Command line code on line 1"),
        ("<?php foo(1 ;", "syntax error, unexpected token \";\", expecting \")\" in Command line code on line 1"),
        ("<?php foo(1 ; }", "syntax error, unexpected token \";\", expecting \")\" in Command line code on line 1"),
        ("<?php foo(1 ; ]", "syntax error, unexpected token \";\", expecting \")\" in Command line code on line 1"),
        ("<?php $x = [1 ;", "syntax error, unexpected token \";\", expecting \"]\" in Command line code on line 1"),
        ("<?php $x = [1 ; ]", "syntax error, unexpected token \";\", expecting \"]\" in Command line code on line 1"),
        ("<?php foo(1 2", "syntax error, unexpected integer \"2\", expecting \")\" in Command line code on line 1"),
        ("<?php foo(1 2 }", "syntax error, unexpected integer \"2\", expecting \")\" in Command line code on line 1"),
        ("<?php foo(1 2 ) )", "syntax error, unexpected integer \"2\", expecting \")\" in Command line code on line 1"),
        ("<?php if ($a) { foo(1 2 }", "syntax error, unexpected integer \"2\", expecting \")\" in Command line code on line 1"),
        ("<?php if ($a) { foo(1 2 ", "syntax error, unexpected integer \"2\", expecting \")\" in Command line code on line 1"),
        ("<?php class A { foo }", "syntax error, unexpected identifier \"foo\", expecting \"function\" in Command line code on line 1"),
        ("<?php class A { $a }", "syntax error, unexpected variable \"$a\", expecting \"function\" in Command line code on line 1"),
        ("<?php class A { ; }", "syntax error, unexpected token \";\", expecting \"function\" in Command line code on line 1"),
        ("<?php class A { 1 }", "syntax error, unexpected integer \"1\", expecting \"function\" in Command line code on line 1"),
        ("<?php class A { 'x' }", "syntax error, unexpected single-quoted string \"x\", expecting \"function\" in Command line code on line 1"),
        ("<?php class A { int $a; }", "syntax error, unexpected identifier \"int\", expecting \"function\" in Command line code on line 1"),
        ("<?php class A { ?int $a; }", "syntax error, unexpected token \"?\", expecting \"function\" in Command line code on line 1"),
        ("<?php class A { static }", "syntax error, unexpected token \"}\", expecting variable in Command line code on line 1"),
        ("<?php class A { public 1 }", "syntax error, unexpected integer \"1\", expecting variable in Command line code on line 1"),
        ("<?php class A { public static 1 }", "syntax error, unexpected integer \"1\", expecting variable in Command line code on line 1"),
        ("<?php class A { function f() {} $z }", "syntax error, unexpected variable \"$z\", expecting \"function\" in Command line code on line 1"),
        ("<?php interface I { 1 }", "syntax error, unexpected integer \"1\", expecting \"function\" in Command line code on line 1"),
        ("<?php enum E { 1 }", "syntax error, unexpected integer \"1\", expecting \"function\" in Command line code on line 1"),
        ("<?php trait T { 1 }", "syntax error, unexpected integer \"1\", expecting \"function\" in Command line code on line 1"),
        ("<?php if $z ($a) {}", "syntax error, unexpected variable \"$z\", expecting \"(\" in Command line code on line 1"),
        ("<?php for $z (;;) {}", "syntax error, unexpected variable \"$z\", expecting \"(\" in Command line code on line 1"),
        ("<?php foreach $z ($a as $b) {}", "syntax error, unexpected variable \"$z\", expecting \"(\" in Command line code on line 1"),
        ("<?php function $z () {}", "syntax error, unexpected variable \"$z\", expecting \"(\" in Command line code on line 1"),
        ("<?php declare $z (ticks=1);", "syntax error, unexpected variable \"$z\", expecting \"(\" in Command line code on line 1"),
        ("<?php $x = fn $z () => 1;", "syntax error, unexpected variable \"$z\", expecting \"(\" in Command line code on line 1"),
        ("<?php $x = function $z () {};", "syntax error, unexpected variable \"$z\", expecting \"(\" in Command line code on line 1"),
        ("<?php goto 1;", "syntax error, unexpected integer \"1\", expecting identifier in Command line code on line 1"),
        ("<?php global 1;", "syntax error, unexpected integer \"1\", expecting variable or \"$\" in Command line code on line 1"),
        ("<?php global $a 1;", "syntax error, unexpected integer \"1\", expecting \",\" or \";\" in Command line code on line 1"),
        ("<?php unset ($a) 1;", "syntax error, unexpected integer \"1\", expecting \";\" in Command line code on line 1"),
        ("<?php unset ($a) $z;", "syntax error, unexpected variable \"$z\", expecting \";\" in Command line code on line 1"),
        ("<?php list($a) 1;", "syntax error, unexpected integer \"1\", expecting \"=\" in Command line code on line 1"),
        ("<?php do {} 1 while (1);", "syntax error, unexpected integer \"1\", expecting \"while\" in Command line code on line 1"),
        ("<?php use A\\B 1;", "syntax error, unexpected integer \"1\", expecting \",\" or \";\" in Command line code on line 1"),
        ("<?php const 1 = 2;", "syntax error, unexpected integer \"1\", expecting identifier in Command line code on line 1"),
        ("<?php const A 1;", "syntax error, unexpected integer \"1\", expecting \"=\" in Command line code on line 1"),
        ("<?php static $a 1;", "syntax error, unexpected integer \"1\", expecting \",\" or \";\" in Command line code on line 1"),
        ("<?php new 1;", "syntax error, unexpected integer \"1\", expecting \"class\" in Command line code on line 1"),
        ("<?php return 1 2 3;", "syntax error, unexpected integer \"2\", expecting \";\" in Command line code on line 1"),
        ("<?php while (1) { break 2 3; }", "syntax error, unexpected integer \"3\", expecting \";\" in Command line code on line 1"),
        ("<?php isset 1 ($a);", "syntax error, unexpected integer \"1\", expecting \"(\" in Command line code on line 1"),
        ("<?php $x = new 1;", "syntax error, unexpected integer \"1\", expecting \"class\" in Command line code on line 1"),
    ];
    for (src, expected) in rows {
        assert_eq!(parse_error(src), *expected, "{src}");
    }
}
