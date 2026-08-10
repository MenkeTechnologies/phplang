//! Property and method visibility enforcement. `public` members are always
//! reachable; `private` is reachable only from the declaring class, `protected`
//! only from the same inheritance line.
//!
//! An out-of-reach PROPERTY or METHOD is a catchable `Error`, not a host-level
//! abort: the reference throws, so `try { $o->priv(); } catch (Error $e)`
//! handles it and an uncaught one renders as an ordinary fatal-error block. Both
//! halves are asserted below — the class and message a `catch` sees, and the
//! verbatim rendering when nothing catches it, taken from the same program under
//! the reference `php` 8.5.9.

use phplang::{compile, eval_capture, host, run_compiled};

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

fn err(src: &str) -> String {
    eval_capture(src).expect_err("expected a visibility violation to surface as an Err")
}

/// Everything `src` wrote, including an uncaught-exception block — which
/// `eval_capture` drops, since it reports that run as a failure.
fn output_of(src: &str) -> String {
    host::reset_host();
    host::with_host(|h| h.begin_capture());
    if let Ok(prog) = compile(src) {
        let _ = run_compiled(prog);
    }
    host::with_host(|h| h.end_capture())
}

/// `get_class($e)|$e->getMessage()` for the `Error` `src` is expected to throw.
fn caught(src: &str) -> String {
    run(&format!(
        "<?php try {{ {src} }} catch (Throwable $e) {{ echo get_class($e), \"|\", $e->getMessage(); }}"
    ))
}

// ── properties ───────────────────────────────────────────────────────────────

#[test]
fn public_property_reachable_externally() {
    assert_eq!(
        run(r#"<?php class C { public $x = 5; } $o = new C(); echo $o->x;"#),
        "5"
    );
}

#[test]
fn private_property_external_read_throws() {
    assert_eq!(
        caught(r#"class C { private $x = 1; } $o = new C(); echo $o->x;"#),
        "Error|Cannot access private property C::$x"
    );
}

#[test]
fn private_property_external_write_throws() {
    assert_eq!(
        caught(r#"class C { private $x = 1; } $o = new C(); $o->x = 2;"#),
        "Error|Cannot access private property C::$x"
    );
}

#[test]
fn private_property_external_unset_throws() {
    assert_eq!(
        caught(r#"class C { private $x = 1; } $o = new C(); unset($o->x);"#),
        "Error|Cannot access private property C::$x"
    );
}

#[test]
fn an_uncaught_property_access_error_renders_as_a_fatal_block() {
    // Verbatim stdout of the same program under the reference `php` 8.5.9.
    assert_eq!(
        output_of(r#"<?php class C { private $x = 1; } $o = new C(); $o->x = 2;"#),
        "\nFatal error: Uncaught Error: Cannot access private property C::$x in \
         Command line code:1\nStack trace:\n#0 {main}\n  thrown in Command line code \
         on line 1\n"
    );
}

#[test]
fn private_property_internal_access_ok() {
    assert_eq!(
        run(
            r#"<?php class C { private $x = 42; function get() { return $this->x; } }
                $o = new C(); echo $o->get();"#
        ),
        "42"
    );
}

#[test]
fn private_property_same_class_other_instance_ok() {
    // A method may reach a private property on another instance of its own class.
    assert_eq!(
        run(r#"<?php class C { private $x;
                    function __construct($v) { $this->x = $v; }
                    function add(C $o) { return $this->x + $o->x; } }
                $a = new C(3); $b = new C(4); echo $a->add($b);"#),
        "7"
    );
}

#[test]
fn protected_property_external_read_throws() {
    assert_eq!(
        caught(r#"class C { protected $x = 1; } $o = new C(); echo $o->x;"#),
        "Error|Cannot access protected property C::$x"
    );
}

#[test]
fn protected_property_subclass_access_ok() {
    assert_eq!(
        run(r#"<?php class P { protected $x = 7; }
                class C extends P { function get() { return $this->x; } }
                $o = new C(); echo $o->get();"#),
        "7"
    );
}

#[test]
fn private_property_of_unrelated_class_throws_naming_the_declaring_class() {
    assert_eq!(
        caught(
            r#"class A { private $x = 1; }
                class B { function peek(A $a) { return $a->x; } }
                $b = new B(); echo $b->peek(new A());"#
        ),
        "Error|Cannot access private property A::$x"
    );
}

// ── methods ──────────────────────────────────────────────────────────────────

#[test]
fn public_method_reachable_externally() {
    assert_eq!(
        run(r#"<?php class C { function m() { return 6; } } $o = new C(); echo $o->m();"#),
        "6"
    );
}

#[test]
fn private_method_external_call_throws() {
    assert_eq!(
        caught(r#"class C { private function m() { return 1; } } $o = new C(); echo $o->m();"#),
        "Error|Call to private method C::m() from global scope"
    );
}

#[test]
fn an_uncaught_method_access_error_renders_as_a_fatal_block() {
    // Verbatim stdout of the same program under the reference `php` 8.5.9.
    assert_eq!(
        output_of(
            r#"<?php class C { private function m() { return 1; } } $o = new C(); echo $o->m();"#
        ),
        "\nFatal error: Uncaught Error: Call to private method C::m() from global scope in \
         Command line code:1\nStack trace:\n#0 {main}\n  thrown in Command line code \
         on line 1\n"
    );
}

/// A class with a `__call` catch-all never reports an access error: the
/// reference routes an unreachable method to the magic method instead, exactly
/// as it does for an undeclared one.
#[test]
fn a_private_method_is_reached_through_the_call_catch_all() {
    assert_eq!(
        run(r#"<?php class C { private function m() { return 1; }
                    public function __call($n, $a) { return "magic:$n"; } }
                $o = new C(); echo $o->m();"#),
        "magic:m"
    );
}

#[test]
fn private_method_internal_call_ok() {
    assert_eq!(
        run(r#"<?php class C { private function m() { return 99; }
                    function pub() { return $this->m(); } }
                $o = new C(); echo $o->pub();"#),
        "99"
    );
}

#[test]
fn protected_method_subclass_call_ok() {
    assert_eq!(
        run(r#"<?php class P { protected function m() { return 8; } }
                class C extends P { function run() { return $this->m(); } }
                $o = new C(); echo $o->run();"#),
        "8"
    );
}

#[test]
fn protected_method_external_call_throws() {
    assert_eq!(
        caught(r#"class C { protected function m() { return 1; } } $o = new C(); echo $o->m();"#),
        "Error|Call to protected method C::m() from global scope"
    );
}

// ── what enumeration and casting expose ──────────────────────────────────────
//
// Verbatim stdout of the same program under the reference `php` 8.5.9.

#[test]
fn get_object_vars_returns_only_what_the_calling_scope_can_see() {
    let src = r#"<?php
        class V { public $pub = 1; protected $prot = 2; private $priv = 3;
            function inside() { return get_object_vars($this); } }
        $v = new V();
        echo json_encode(get_object_vars($v)), "|", json_encode($v->inside());"#;
    assert_eq!(run(src), r#"{"pub":1}|{"pub":1,"prot":2,"priv":3}"#);
}

#[test]
fn casting_to_array_mangles_non_public_property_names() {
    // A private `$p` declared by `C` becomes "\0C\0p" and a protected one
    // "\0*\0p", so two same-named properties of different visibility cannot
    // collide — and neither key is reachable as `$arr['p']`.
    let src = r#"<?php
        class V { public $pub = 1; protected $prot = 2; private $priv = 3; }
        $arr = (array)(new V());
        echo count($arr), "|", implode(",", array_keys($arr));"#;
    assert_eq!(run(src), "3|pub,\0*\0prot,\0V\0priv");
}

#[test]
fn a_mangled_key_carries_the_declaring_class_of_an_inherited_private() {
    let src = r#"<?php
        class Base { private $secret = 1; }
        class Sub extends Base { public $open = 2; }
        echo implode(",", array_keys((array)(new Sub())));"#;
    assert_eq!(run(src), "\0Base\0secret,open");
}

#[test]
fn settype_converts_through_its_by_reference_parameter() {
    let src = r#"<?php
        $s = "123"; settype($s, "integer");
        $f = 1.9;   settype($f, "int");
        $b = 0;     settype($b, "boolean");
        $n = 5;     settype($n, "string");
        $z = "x";   $ok = settype($z, "float");
        var_dump($s, $f, $b, $n, $ok, $z);"#;
    assert_eq!(
        run(src),
        "int(123)\nint(1)\nbool(false)\nstring(1) \"5\"\nbool(true)\nfloat(0)\n"
    );
}

#[test]
fn settype_to_array_wraps_a_scalar() {
    let src = r#"<?php $a = 1; settype($a, "array"); var_dump($a);"#;
    assert_eq!(run(src), "array(1) {\n  [0]=>\n  int(1)\n}\n");
}

/// An UNCAUGHT visibility violation is a failed run, not merely a printed
/// block: `eval_capture` reports it as `Err`, and the message is the reference's
/// own text.
///
/// This is the assertion the `err` helper at the top of the file was written for
/// and never had — it sat unused, which is also why `cargo clippy --all-targets
/// -- -D warnings` had a `dead_code` finding here.
///
/// ```text
/// $ php -r 'class C { private $p = 1; } $o = new C; echo $o->p;'
/// PHP Fatal error:  Uncaught Error: Cannot access private property C::$p in Command line code:1
/// ```
#[test]
fn an_uncaught_visibility_violation_fails_the_run() {
    let cases: &[(&str, &str)] = &[
        (
            "class C { private $p = 1; } $o = new C; echo $o->p;",
            "Cannot access private property C::$p",
        ),
        (
            "class C { protected $p = 1; } $o = new C; echo $o->p;",
            "Cannot access protected property C::$p",
        ),
        (
            "class C { private function m() {} } $o = new C; $o->m();",
            "Call to private method C::m() from global scope",
        ),
    ];
    for (src, msg) in cases {
        let e = err(&format!("<?php {src}"));
        assert!(e.contains(msg), "{src}\n  expected {msg:?} in {e:?}");
    }
    // The same access from INSIDE the class is not a violation and returns Ok.
    assert_eq!(
        run(
            "<?php class C { private $p = 7; function get() { return $this->p; } } \
             $o = new C; echo $o->get();"
        ),
        "7"
    );
}
