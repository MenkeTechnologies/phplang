//! The compile-time checks the reference makes on a class declaration: a
//! modifier repeated or used where its member kind forbids it, a member
//! declared twice, a method whose body contradicts its modifiers, and a
//! parameter named twice. Each is a `Fatal error` before anything runs, so the
//! `echo` that precedes it never prints.
//!
//! Every expectation is the verbatim stdout of `php -r` under the reference
//! `php` 8.5.11 (stderr carries only the `log_errors` copy and is dropped).
//! Grammar-level modifier errors carry no stack trace; the others do.

use std::process::{Command, Stdio};

fn run_r(code: &str) -> (String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_php"))
        .arg("-r")
        .arg(code)
        .stderr(Stdio::null())
        .output()
        .expect("spawn php -r");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn fatal(msg: &str) -> (String, i32) {
    (
        format!("\nFatal error: {msg} in Command line code on line 1\nStack trace:\n#0 {{main}}\n"),
        255,
    )
}

fn bare_fatal(msg: &str) -> (String, i32) {
    (format!("\nFatal error: {msg} in Command line code on line 1\n"), 255)
}

#[test]
fn a_repeated_member_is_diagnosed_per_kind() {
    assert_eq!(
        run_r("echo 1; class A { const X = 1; const X = 2; }"),
        fatal("Cannot redefine class constant A::X")
    );
    assert_eq!(run_r("echo 1; class A { public $a; public $a; }"), fatal("Cannot redeclare A::$a"));
    assert_eq!(
        run_r("echo 1; class A { function f() {} function F() {} }"),
        fatal("Cannot redeclare A::F()")
    );
    assert_eq!(
        run_r("echo 1; enum E { case A; case A; }"),
        fatal("Cannot redefine class constant E::A")
    );
    assert_eq!(
        run_r("echo 1; class A { public $x; function __construct(public $x) {} }"),
        fatal("Cannot redeclare A::$x")
    );
}

#[test]
fn a_method_body_must_match_its_modifiers() {
    assert_eq!(
        run_r("echo 1; class A { abstract function f(); }"),
        fatal("Class A declares abstract method f() and must therefore be declared abstract")
    );
    assert_eq!(
        run_r("echo 1; class A { function f(); }"),
        fatal("Non-abstract method A::f() must contain body")
    );
    assert_eq!(
        run_r("echo 1; interface I { function f() {} }"),
        fatal("Interface function I::f() cannot contain body")
    );
    assert_eq!(
        run_r("echo 1; interface I { private function f(); }"),
        fatal("Access type for interface method I::f() must be public")
    );
}

#[test]
fn a_parameter_named_twice_is_diagnosed() {
    assert_eq!(run_r("echo 1; function f($a, $a) {}"), fatal("Redefinition of parameter $a"));
}

#[test]
fn a_readonly_property_needs_a_type() {
    assert_eq!(
        run_r("echo 1; class A { public readonly $x; }"),
        fatal("Readonly property A::$x must have type")
    );
}

#[test]
fn misplaced_or_repeated_modifiers_are_grammar_errors_without_a_trace() {
    assert_eq!(
        run_r("echo 1; final final class A {}"),
        bare_fatal("Multiple final modifiers are not allowed")
    );
    assert_eq!(
        run_r("echo 1; abstract final class A {}"),
        bare_fatal("Cannot use the final modifier on an abstract class")
    );
    assert_eq!(
        run_r("echo 1; class A { public public $x; }"),
        bare_fatal("Multiple access type modifiers are not allowed")
    );
    assert_eq!(
        run_r("echo 1; class A { static const X = 1; }"),
        bare_fatal("Cannot use the static modifier on a class constant")
    );
    assert_eq!(
        run_r("echo 1; class A { readonly function f() {} }"),
        bare_fatal("Cannot use the readonly modifier on a method")
    );
    assert_eq!(
        run_r("echo 1; class A { abstract final function f(); }"),
        bare_fatal("Cannot use the final modifier on an abstract method")
    );
}

#[test]
fn well_formed_declarations_still_run() {
    let code = "trait T { abstract private function h(); }
interface I { const K = 3; public function f(); }
abstract class A implements I { use T; abstract protected function z(); public function f() { return 1; } private function h() {} }
final class C extends A { public function z() { return 2; } }
readonly final class R { public function __construct(public int $a) {} }
class U { var $v = 1; final const F = 4; }
$c = new C; echo $c->f(), $c->z(), I::K, U::F, (new R(5))->a;";
    assert_eq!(run_r(code), ("12345".to_string(), 0));
}
