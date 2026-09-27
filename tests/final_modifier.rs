//! `final` enforced when a class links against what it inherits: a final class
//! extended, and a final method, constant or property redeclared below it.
//!
//! Every expectation is the verbatim stdout and exit status of the same
//! program under the reference `php` 8.5.11 (stderr carries only the
//! `log_errors` copy and is dropped). Whether the `echo` before the
//! declaration prints tells the two link times apart: an early-bound class
//! links before the file runs, anything else where its declaration runs.

use std::io::Write;
use std::process::{Command, Stdio};

fn run(args: &[&str]) -> (String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_php"))
        .args(args)
        .stderr(Stdio::null())
        .output()
        .expect("spawn php");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn run_r(code: &str) -> (String, i32) {
    run(&["-r", code])
}

/// Run `src` as a script file, with its path in the output replaced by `F`.
fn run_file(src: &str) -> (String, i32) {
    let mut f = temp_script();
    f.1.write_all(src.as_bytes()).unwrap();
    let path = f.0.to_str().unwrap().to_string();
    let (out, code) = run(&[&path]);
    // The engine names the script by its real path (`/tmp` may be a link).
    let real = std::fs::canonicalize(&f.0).unwrap();
    let _ = std::fs::remove_file(&f.0);
    (
        out.replace(real.to_str().unwrap(), "F").replace(&path, "F"),
        code,
    )
}

fn temp_script() -> (std::path::PathBuf, std::fs::File) {
    let dir = std::env::temp_dir();
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = dir.join(format!("phplang-final-{}-{n}.php", std::process::id()));
    let file = std::fs::File::create(&path).unwrap();
    (path, file)
}

fn fatal(before: &str, msg: &str) -> (String, i32) {
    (
        format!("{before}\nFatal error: {msg} in Command line code on line 1\nStack trace:\n#0 {{main}}\n"),
        255,
    )
}

#[test]
fn a_final_class_cannot_be_extended() {
    // Early-bound: the program never starts, so the echo never prints.
    assert_eq!(
        run_r(r#"echo "x"; final class A {} class B extends A {}"#),
        fatal("", "Class B cannot extend final class A")
    );
    // Parent declared further down: B links where it stands, after the echo.
    assert_eq!(
        run_r(r#"echo "x"; class B extends A {} final class A {}"#),
        fatal("x", "Class B cannot extend final class A")
    );
    assert_eq!(
        run_r(r#"echo "x"; if (1) { final class A {} } class B extends A {}"#),
        fatal("x", "Class B cannot extend final class A")
    );
    // An anonymous class is named by its parent.
    assert_eq!(
        run_r("final class A {} $c = new class extends A {}; echo 1;"),
        fatal("", "Class A@anonymous cannot extend final class A")
    );
    // Instantiating one is fine.
    assert_eq!(
        run_r(r#"final class A { function f() { echo "ok"; } } (new A)->f();"#),
        ("ok".to_string(), 0)
    );
}

#[test]
fn a_final_method_cannot_be_overridden() {
    assert_eq!(
        run_r("class A { final function f() {} } class B extends A { function f() {} }"),
        fatal("", "Cannot override final method A::f()")
    );
    assert_eq!(
        run_r(
            "class A { final static function f() {} } class B extends A { static function f() {} }"
        ),
        fatal("", "Cannot override final method A::f()")
    );
    // Through an intermediate class, named as the child spells it.
    assert_eq!(
        run_r("class A { final function f() {} } class B extends A {} class C extends B { function F() {} }"),
        fatal("", "Cannot override final method A::F()")
    );
    // The nearest declaration decides, and names its class.
    assert_eq!(
        run_r(
            "abstract class A { abstract function f(); } class B extends A { final function f() {} } \
             class C extends B { function f() {} }"
        ),
        fatal("", "Cannot override final method B::f()")
    );
    // The parent's declaration order decides which is reported.
    assert_eq!(
        run_r(
            "class A { final function g() {} final function f() {} } \
             class B extends A { function f() {} function g() {} }"
        ),
        fatal("", "Cannot override final method A::g()")
    );
    // A final method taken from a trait is the using class's.
    assert_eq!(
        run_r("trait T { final function f() {} } class A { use T; } class B extends A { function f() {} }"),
        fatal("", "Cannot override final method A::f()")
    );
    // Inherited, it is still callable.
    assert_eq!(
        run_r(
            "class A { final function f() { return 7; } } class B extends A {} echo (new B)->f();"
        ),
        ("7".to_string(), 0)
    );
    // A private constructor's `final` binds; the engine's exception getters are final.
    assert_eq!(
        run_r("class A { private final function __construct() {} } class B extends A { function __construct() {} }"),
        fatal("", "Cannot override final method A::__construct()")
    );
    assert_eq!(
        run_r("class E extends Exception { function getMessage() { return 1; } }"),
        fatal("", "Cannot override final method Exception::getMessage()")
    );
}

#[test]
fn an_overriding_method_is_reported_at_its_own_line() {
    let src = "<?php\necho \"x\\n\";\nclass A {\n  final function f() {}\n}\nclass B\n  extends A\n{\n\n  function f() {}\n}\n";
    assert_eq!(
        run_file(src),
        (
            "\nFatal error: Cannot override final method A::f() in F on line 10\nStack trace:\n#0 {main}\n"
                .to_string(),
            255
        )
    );
    // A trait's method, at the trait's line.
    let src = "<?php\ntrait T { function f(){} }\nclass A { final function f(){} }\nclass B extends A { use T; }\n";
    assert_eq!(
        run_file(src),
        (
            "\nFatal error: Cannot override final method A::f() in F on line 2\nStack trace:\n#0 {main}\n"
                .to_string(),
            255
        )
    );
}

#[test]
fn a_class_declared_in_a_function_fails_inside_its_frame() {
    let src =
        "<?php\nfunction g(){ class B extends A {} }\nfinal class A {}\n echo \"x\\n\"; g();\n";
    assert_eq!(
        run_file(src),
        (
            "x\n\nFatal error: Class B cannot extend final class A in F on line 2\nStack trace:\n#0 F(4): g()\n#1 {main}\n"
                .to_string(),
            255
        )
    );
}

#[test]
fn a_final_constant_cannot_be_redefined() {
    assert_eq!(
        run_r("class A { final const X = 1; } class B extends A { const X = 2; } echo 1;"),
        fatal("", "B::X cannot override final constant A::X")
    );
    assert_eq!(
        run_r("class A { final const X = 1; } class B extends A {} class C extends B { const X = 2; }"),
        fatal("", "C::X cannot override final constant A::X")
    );
    // From an interface, however far up it was declared.
    assert_eq!(
        run_r("interface I { final const X = 1; } class A implements I { const X = 2; }"),
        fatal("", "A::X cannot override final constant I::X")
    );
    assert_eq!(
        run_r("interface I { final const X = 1; } interface J extends I {} class A implements J { const X = 3; }"),
        fatal("", "A::X cannot override final constant I::X")
    );
    // Without `final`, an interface constant may be redefined.
    assert_eq!(
        run_r("interface I { const X = 1; } class A implements I { const X = 2; } echo A::X;"),
        ("2".to_string(), 0)
    );
}

#[test]
fn a_final_property_cannot_be_redeclared_and_is_checked_first() {
    assert_eq!(
        run_r("class A { public final int $p = 1; } class B extends A { public int $p = 2; }"),
        fatal("", "Cannot override final property A::$p")
    );
    assert_eq!(
        run_r(
            "class A { final public static $p = 1; } class B extends A { public static $p = 2; }"
        ),
        fatal("", "Cannot override final property A::$p")
    );
    // Properties, then constants, then methods.
    assert_eq!(
        run_r(
            "class A { final const X = 1; final public $p = 1; final function f() {} } \
             class B extends A { function f() {} public $p = 2; const X = 2; }"
        ),
        fatal("", "Cannot override final property A::$p")
    );
    assert_eq!(
        run_r("class A { final const X = 1; final function f() {} } class B extends A { function f() {} const X = 2; }"),
        fatal("", "B::X cannot override final constant A::X")
    );
}
