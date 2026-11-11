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

// ── run_program: a whole program with its own stdin ──────────────────────────

#[test]
fn run_program_captures_stdout() {
    let out = phplang::run_program("<?php echo 'hello, ', 'world', \"\\n\";", "");
    assert_eq!(out.unwrap(), "hello, world\n");
}

/// Every stdin read primitive draws from the same input, in order: `fgets`
/// takes one line, `readline` the next without its newline, and a whole-stream
/// read through a second handle the rest.
#[test]
fn run_program_feeds_stdin_to_every_reader() {
    let src = r#"<?php
        echo strtoupper(fgets(STDIN));
        echo '[', readline('> '), "]\n";
        echo strrev(trim(file_get_contents('php://stdin'))), "\n";
        var_dump(readline());
    "#;
    let out = phplang::run_program(src, "one\ntwo\nthree\nfour\n").unwrap();
    assert_eq!(out, "ONE\n[two]\nruof\neerht\nbool(false)\n");
}

#[test]
fn run_program_errors_are_err() {
    assert!(phplang::run_program("<?php echo 'x' ;; }", "").is_err());
    let e = phplang::run_program("<?php throw new RuntimeException('boom');", "").unwrap_err();
    assert!(e.contains("boom"), "{e}");
    assert_eq!(
        phplang::run_program("<?php echo 'a'; exit(3); echo 'b';", ""),
        Err("exit status 3".into())
    );
    // A successful exit keeps the output written before it.
    assert_eq!(
        phplang::run_program("<?php echo 'a'; exit(0); echo 'b';", ""),
        Ok("a".into())
    );
}

/// Shutdown functions and destructors run, as at the end of a CLI request.
#[test]
fn run_program_runs_the_request_shutdown() {
    let src = r#"<?php
        class D { function __destruct() { echo "d"; } }
        $d = new D;
        register_shutdown_function(function () { echo "s"; });
        echo "m";
    "#;
    assert_eq!(phplang::run_program(src, "").unwrap(), "msd");
}

/// Concurrent runs each see their own input and return their own output.
#[test]
fn run_program_concurrent_runs_do_not_mix() {
    let src = "<?php for ($i = 0; $i < 2000; $i++) echo trim(fgets(STDIN) ?: ''), stream_get_contents(STDIN), $i % 100 ? '' : \"\\n\";";
    let handles: Vec<_> = ["a\n", "b\n", "c\n", "d\n"]
        .into_iter()
        .map(|input| std::thread::spawn(move || (input, phplang::run_program(src, input).unwrap())))
        .collect();
    for h in handles {
        let (input, out) = h.join().unwrap();
        let c = input.trim();
        let expected: String = (0..2000)
            .map(|i| {
                format!(
                    "{}{}",
                    if i == 0 { c } else { "" },
                    if i % 100 == 0 { "\n" } else { "" }
                )
            })
            .collect();
        assert_eq!(out, expected, "run fed {input:?}");
    }
}
