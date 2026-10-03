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
    (
        format!("\nFatal error: {msg} in Command line code on line 1\n"),
        255,
    )
}

#[test]
fn a_repeated_member_is_diagnosed_per_kind() {
    assert_eq!(
        run_r("echo 1; class A { const X = 1; const X = 2; }"),
        fatal("Cannot redefine class constant A::X")
    );
    assert_eq!(
        run_r("echo 1; class A { public $a; public $a; }"),
        fatal("Cannot redeclare A::$a")
    );
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
    assert_eq!(
        run_r("echo 1; function f($a, $a) {}"),
        fatal("Redefinition of parameter $a")
    );
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
#[test]
fn an_enum_refuses_what_it_cannot_have() {
    assert_eq!(
        run_r("echo 1; enum E: float { case A = 1.5; }"),
        fatal("Enum backing type must be int or string, float given")
    );
    assert_eq!(
        run_r(r#"echo 1; enum E: string { case A = "a"; case B; }"#),
        fatal("Case B of backed enum E must have a value")
    );
    assert_eq!(
        run_r("echo 1; enum E { case A = 1; }"),
        fatal("Case A of non-backed enum E must not have a value")
    );
    // A property is refused as it is compiled; the magic methods only once the
    // body is done, so a property declared after one is still reported first.
    assert_eq!(
        run_r("echo 1; enum E { function __get($n) {} function __clone() {} public $x; }"),
        fatal("Enum E cannot include properties")
    );
    // The magic methods are checked in a fixed order, in the engine's spelling.
    assert_eq!(
        run_r("echo 1; enum E { function __tostring() {} function __clone() {} }"),
        fatal("Enum E cannot include magic method __clone")
    );
    assert_eq!(
        run_r("echo 1; enum E { function __tostring() {} }"),
        fatal("Enum E cannot include magic method __toString")
    );
}

#[test]
fn a_void_or_never_body_refuses_what_its_returns_carry() {
    assert_eq!(
        run_r("echo 1; function f(): void { return 1; }"),
        fatal("A void function must not return a value")
    );
    assert_eq!(
        run_r("echo 1; function f(): void { return NULL; }"),
        fatal(
            "A void function must not return a value (did you mean \"return;\" instead of \
             \"return null;\"?)"
        )
    );
    assert_eq!(
        run_r("echo 1; class A { function m(): never { return; } }"),
        fatal("A never-returning method must not return")
    );
    // A closure written in a method is a "method" too.
    assert_eq!(
        run_r("echo 1; class A { function m() { $f = function(): void { return 1; }; } }"),
        fatal("A void method must not return a value")
    );
    assert_eq!(
        run_r("echo 1; $f = fn(): void => 1;"),
        fatal("A void function must not return a value")
    );
    // A nested body has its own rule; `fn(): never` may throw; a generator is exempt.
    assert_eq!(
        run_r(
            "function f(): void { $g = function() { return 1; }; $h = fn() => 2; return; } f(); \
             $n = fn(): never => throw new Exception(\"x\"); \
             function g(): iterable { yield 1; return 2; } foreach (g() as $v) echo $v;"
        ),
        ("1".to_string(), 0)
    );
}

#[test]
fn this_is_never_rebound() {
    for code in [
        "echo 1; $this = 1;",
        "echo 1; foreach ([1] as $k => $this) {}",
        "echo 1; try {} catch (Exception $this) {}",
        "echo 1; $this = &$a;",
        "echo 1; [$a, $this] = [1, 2];",
    ] {
        assert_eq!(run_r(code), fatal("Cannot re-assign $this"), "{code}");
    }
    assert_eq!(
        run_r("echo 1; function f() { global $this; }"),
        fatal("Cannot use $this as global variable")
    );
    assert_eq!(
        run_r("echo 1; function f() { static $this; }"),
        fatal("Cannot use $this as static variable")
    );
    assert_eq!(
        run_r("echo 1; unset($a, $this);"),
        fatal("Cannot unset $this")
    );
    assert_eq!(
        run_r("echo 1; $f = fn($this) => 1;"),
        fatal("Cannot use $this as parameter")
    );
    assert_eq!(
        run_r("echo 1; function f($_GET) {}"),
        fatal("Cannot re-assign auto-global variable _GET")
    );
}

#[test]
fn a_use_clause_refuses_this_superglobals_repeats_and_parameters() {
    assert_eq!(
        run_r("echo 1; $f = function() use ($a, $this) {};"),
        fatal("Cannot use $this as lexical variable")
    );
    assert_eq!(
        run_r("echo 1; $f = function() use ($_GET) {};"),
        fatal("Cannot use auto-global as lexical variable")
    );
    assert_eq!(
        run_r("echo 1; $f = function() use ($a, $a) {};"),
        fatal("Cannot use variable $a twice")
    );
    assert_eq!(
        run_r("echo 1; $f = function($a) use ($a) {};"),
        fatal("Cannot use lexical variable $a as a parameter name")
    );
}

#[test]
fn a_parameter_list_refuses_misplaced_variadics_and_bottom_types() {
    assert_eq!(
        run_r("echo 1; function f(...$a, $b) {}"),
        fatal("Only the last parameter can be variadic")
    );
    assert_eq!(
        run_r("echo 1; function f(...$a = []) {}"),
        fatal("Variadic parameter cannot have a default value")
    );
    assert_eq!(
        run_r("echo 1; function f(void $a) {}"),
        fatal("void cannot be used as a parameter type")
    );
    assert_eq!(
        run_r("echo 1; function f(?void $a) {}"),
        fatal("Void can only be used as a standalone type")
    );
    assert_eq!(
        run_r("echo 1; function f(?never $a) {}"),
        fatal("never can only be used as a standalone type")
    );
}

#[test]
fn reading_this_with_no_object_bound_is_an_error() {
    // php -r 'echo $this;' — no `Undefined variable` warning, an Error.
    let thrown = |what: &str| {
        (
            format!(
                "\nFatal error: Uncaught Error: Using $this when not in object context in Command \
                 line code:1\nStack trace:\n#0 {what}\n"
            ),
            255,
        )
    };
    assert_eq!(
        run_r("echo $this;"),
        thrown("{main}\n  thrown in Command line code on line 1")
    );
    assert_eq!(
        run_r("$this->a = 1;"),
        thrown("{main}\n  thrown in Command line code on line 1")
    );
    assert_eq!(
        run_r("$this++;"),
        thrown("{main}\n  thrown in Command line code on line 1")
    );
    assert_eq!(
        run_r("function f() { return $this; } f();"),
        thrown("Command line code(1): f()\n#1 {main}\n  thrown in Command line code on line 1")
    );
}
