//! Runs every `examples/*.php` file through `phplang::eval_capture` and checks
//! its output. Reads the file to a string (eval_capture takes source text, not
//! a path) so no subprocess/binary is spawned — deterministic under CI.

use phplang::eval_capture;

fn run_example(name: &str) -> String {
    let path = format!("{}/examples/{name}", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    eval_capture(&src).unwrap_or_else(|e| panic!("eval error running {name}: {e}"))
}

/// Asserted as the WHOLE output rather than as eleven `contains` checks.
///
/// The substring form could not see ordering, could not see anything extra the
/// example printed, and `contains("42")` in particular matched the `42` inside
/// any other number on the page. The exact text below is byte-identical to what
/// `php 8.5.9` prints for the same file:
///
/// ```text
/// $ diff <(php examples/hello.php) <(target/debug/php examples/hello.php)
/// ```
///
/// Note the trailing space after `i=2` — the loop separator, which no `contains`
/// assertion could have pinned.
#[test]
fn hello_example_runs_and_prints_expected() {
    assert_eq!(
        run_example("hello.php"),
        "<h1>phplang</h1>\n\
         Hello, world!\n\
         42\n\
         2.5\n\
         1024\n\
         ada is 36\n\
         sum = 10\n\
         i=0 i=1 i=2 \n\
         5! = 120\n\
         DONE 4 1,2,3,4\n\
         <footer>bye</footer>\n"
    );
}

#[test]
fn arrays_example_runs_and_prints_expected() {
    let out = run_example("arrays.php");
    assert_eq!(
        out,
        "nums: 10,20,30,40\n\
         count: 4\n\
         sum: 100\n\
         name=ada age=37 \n\
         keys: name,age\n\
         has 30? yes\n\
         range: 1,2,3,4,5\n\
         max/min: 40/10\n"
    );
}

#[test]
fn functions_example_runs_and_prints_expected() {
    let out = run_example("functions.php");
    assert_eq!(
        out,
        "120\n\
         55\n\
         even\n\
         Hi, Ada!\n\
         HI, WORLD!\n\
         6\n"
    );
}

#[test]
fn control_example_runs_and_prints_expected() {
    let out = run_example("control.php");
    assert_eq!(
        out,
        "b\n\
         odd\n\
         124\n\
         012\n\
         yn\n"
    );
}

#[test]
fn classes_example_runs_and_prints_expected() {
    let out = run_example("classes.php");
    assert_eq!(
        out,
        "Camel has 4 legs\n\
         Rex says woof\n\
         Rex has 4 legs\n\
         woof\n\
         Dog\n\
         7\n"
    );
}

#[test]
fn closures_example_runs_and_prints_expected() {
    let out = run_example("closures.php");
    assert_eq!(
        out,
        "105\n\
         12\n\
         2,4,6\n\
         17\n\
         iife\n"
    );
}

#[test]
fn exceptions_example_runs_and_prints_expected() {
    let out = run_example("exceptions.php");
    assert_eq!(
        out,
        "caught: boom\n\
         work + cleanup\n\
         as error: not an int\n\
         logic/runtime: bad arg\n\
         required: missing\n\
         bad config (7)\n"
    );
}

#[test]
fn strings_example_runs_and_prints_expected() {
    let out = run_example("strings.php");
    assert_eq!(
        out,
        "Hello, world!\n\
         Hello, world!\n\
         hello world\n\
         HELLO WORLD\n\
         Php\n\
         padded|\n\
         ababab\n\
         cba\n\
         World\n\
         Hello\n\
         6\n\
         Hello PHP\n\
         11\n"
    );
}
