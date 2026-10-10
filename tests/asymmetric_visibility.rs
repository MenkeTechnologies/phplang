//! Asymmetric property visibility (PHP 8.4): `public private(set) int $x`,
//! `protected(set)`, `public(set)`, on declared and promoted properties, with the
//! write refused from outside the set scope and the redeclaration rules checked
//! at link time.
//!
//! Every expectation is the stdout of `php -r` under the reference `php` 8.5.11.

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

fn ok(stdout: &str) -> (String, i32) {
    (stdout.to_string(), 0)
}

fn fatal(msg: &str) -> (String, i32) {
    (
        format!("\nFatal error: {msg} in Command line code on line 1\nStack trace:\n#0 {{main}}\n"),
        255,
    )
}

#[test]
fn a_write_from_outside_the_set_scope_is_refused() {
    assert_eq!(
        run_r(
            r#"class A { public private(set) int $x = 1; function inc() { $this->x++; return $this->x; } }
$a = new A; echo $a->x, $a->inc();
try { $a->x = 2; } catch (Error $e) { echo "\n", get_class($e), ": ", $e->getMessage(); }
try { $a->x++; } catch (Error $e) { echo "\n", $e->getMessage(); }
try { unset($a->x); } catch (Error $e) { echo "\n", $e->getMessage(); }"#
        ),
        ok(
            "12\nError: Cannot modify private(set) property A::$x from global scope\n\
            Cannot modify private(set) property A::$x from global scope\n\
            Cannot unset private(set) property A::$x from global scope"
        )
    );
}

#[test]
fn protected_set_admits_subclasses_and_names_the_scope_otherwise() {
    assert_eq!(
        run_r(
            r#"class A { public protected(set) int $x = 1; function set() { $this->x = 5; } }
class B extends A { function s2() { $this->x = 7; } }
$b = new B; $b->set(); echo $b->x; $b->s2(); echo $b->x;
try { $b->x = 1; } catch (Error $e) { echo "\n", $e->getMessage(); }"#
        ),
        ok("57\nCannot modify protected(set) property A::$x from global scope")
    );
    // `private(set)` does not admit a subclass, and the message names its scope.
    assert_eq!(
        run_r(
            r#"class A { public private(set) int $x = 1; }
class B extends A { function f() { $this->x = 2; } }
try { (new B)->f(); } catch (Error $e) { echo $e->getMessage(); }"#
        ),
        ok("Cannot modify private(set) property A::$x from scope B")
    );
}

#[test]
fn indirect_writes_and_reference_bindings_are_refused_too() {
    assert_eq!(
        run_r(
            r#"class A { public private(set) array $x = []; }
$a = new A;
try { $a->x[] = 1; } catch (Error $e) { echo $e->getMessage(), "\n"; }
try { $r = &$a->x; } catch (Error $e) { echo $e->getMessage(), "\n"; }
$y = 5; try { $a->x = &$y; } catch (Error $e) { echo $e->getMessage(); }"#
        ),
        ok(
            "Cannot indirectly modify private(set) property A::$x from global scope\n\
            Cannot indirectly modify private(set) property A::$x from global scope\n\
            Cannot indirectly modify private(set) property A::$x from global scope"
        )
    );
}

#[test]
fn static_and_promoted_properties_take_the_modifier() {
    assert_eq!(
        run_r(
            r#"class A { public private(set) static int $x = 1; static function s() { self::$x = 2; }
  public function __construct(public private(set) int $p = 3) {} }
A::s(); echo A::$x;
try { A::$x = 3; } catch (Error $e) { echo "\n", $e->getMessage(); }
$a = new A(4); echo $a->p;
try { $a->p = 2; } catch (Error $e) { echo "\n", $e->getMessage(); }"#
        ),
        ok(
            "2\nCannot modify private(set) property A::$x from global scope4\n\
            Cannot modify private(set) property A::$p from global scope"
        )
    );
}

#[test]
fn a_set_visibility_beats_a_magic_setter_and_survives_clone_and_closures() {
    assert_eq!(
        run_r(
            r#"class A { public private(set) int $x = 1; function __set($n, $v) { echo "magic"; } }
$a = new A; try { $a->x = 2; } catch (Error $e) { echo $e->getMessage(); }
$c = clone $a; echo json_encode($c), var_export(isset($a->x), true);
$f = function () { $this->x = 9; }; $f->call($a); echo $a->x;"#
        ),
        ok("Cannot modify private(set) property A::$x from global scope{\"x\":1}true9")
    );
    assert_eq!(
        run_r(
            r#"trait T { public private(set) int $x = 1; function s() { $this->x = 3; } }
class A { use T; } $a = new A; $a->s(); echo $a->x;
try { $a->x = 2; } catch (Error $e) { echo "\n", $e->getMessage(); }"#
        ),
        ok("3\nCannot modify private(set) property A::$x from global scope")
    );
}

#[test]
fn declaration_errors() {
    assert_eq!(
        run_r("class A { public private(set) $x = 1; }"),
        fatal("Property with asymmetric visibility A::$x must have type")
    );
    assert_eq!(
        run_r("class A { private public(set) int $x = 1; }"),
        fatal("Visibility of property A::$x must not be weaker than set visibility")
    );
    assert_eq!(
        run_r("class A { private(set) function f() {} }"),
        (
            "\nFatal error: Cannot use the private(set) modifier on a method in Command line \
             code on line 1\n"
                .to_string(),
            255
        )
    );
    assert_eq!(
        run_r("class A { public private(set) public(set) int $x = 1; }"),
        (
            "\nFatal error: Multiple access type modifiers are not allowed in Command line \
             code on line 1\n"
                .to_string(),
            255
        )
    );
}

#[test]
fn redeclaration_may_not_narrow_or_introduce_a_set_visibility() {
    assert_eq!(
        run_r(
            "class A { public int $x = 1; } class B extends A { public private(set) int $x = 5; }"
        ),
        fatal("Set access level of B::$x must be omitted (as in class A)")
    );
    assert_eq!(
        run_r(
            "class A { protected(set) int $x = 1; } class B extends A { private(set) int $x = 2; }"
        ),
        fatal("Set access level of B::$x must be protected(set) (as in class A) or weaker")
    );
    assert_eq!(
        run_r(
            "class A { public private(set) int $x = 1; } class B extends A { public int $x = 5; }"
        ),
        fatal("Cannot override final property A::$x")
    );
    assert_eq!(
        run_r(
            "class A { protected(set) int $x = 1; } class B extends A { public(set) int $x = 5; } \
             $b = new B; $b->x = 2; echo $b->x;"
        ),
        ok("2")
    );
}
