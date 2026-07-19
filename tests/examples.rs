//! Runs every `examples/*.php` file through `phplang::eval_capture` and checks
//! its output. Reads the file to a string (eval_capture takes source text, not
//! a path) so no subprocess/binary is spawned — deterministic under CI.

use phplang::eval_capture;

fn run_example(name: &str) -> String {
    let path = format!("{}/examples/{name}", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    eval_capture(&src).unwrap_or_else(|e| panic!("eval error running {name}: {e}"))
}

#[test]
fn hello_example_runs_and_prints_expected() {
    let out = run_example("hello.php");
    assert!(out.contains("<h1>phplang</h1>"));
    assert!(out.contains("Hello, world!"));
    assert!(out.contains("42")); // 6 * 7
    assert!(out.contains("2.5")); // 10 / 4
    assert!(out.contains("1024")); // 2 ** 10
    assert!(out.contains("ada is 36"));
    assert!(out.contains("sum = 10"));
    assert!(out.contains("i=0 i=1 i=2"));
    assert!(out.contains("5! = 120"));
    assert!(out.contains("DONE 4 1,2,3,4"));
    assert!(out.contains("<footer>bye</footer>"));
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
