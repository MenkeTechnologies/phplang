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

/// A static property is guarded like an instance one, but the message names the
/// class WRITTEN at the access site (`zend_bad_property_access` is handed the
/// class being fetched from), so `B::$q` for a `protected` declared on `A` says
/// `B::$q`. Read, write and `++` are all checked; access from inside the line
/// is not. Expected output recorded from `php -r` 8.5.10.
#[test]
fn static_property_visibility_is_enforced_and_names_the_accessed_class() {
    let src = r#"<?php
        class A {
            private static $p = 1;
            protected static $q = 2;
            public static function get() { return self::$p + static::$q; }
        }
        class B extends A {
            public static function q() { return static::$q; }
            public static function p() { return parent::$p; }
        }
        foreach ([fn() => A::$p, fn() => A::$q, fn() => B::$q, fn() => B::p(),
                  fn() => A::$p = 5, fn() => A::$q++, fn() => b::$q] as $f) {
            try { var_dump($f()); }
            catch (\Error $e) { echo get_class($e), "|", $e->getMessage(), "\n"; }
        }
        echo A::get(), B::q(), "\n";"#;
    assert_eq!(
        run(src),
        "Error|Cannot access private property A::$p\n\
         Error|Cannot access protected property A::$q\n\
         Error|Cannot access protected property B::$q\n\
         Error|Cannot access private property A::$p\n\
         Error|Cannot access private property A::$p\n\
         Error|Cannot access protected property A::$q\n\
         Error|Cannot access protected property B::$q\n\
         32\n"
    );
}

/// `isset`/`empty`/`??` on a static property is a quiet read: an undeclared or
/// out-of-reach property answers "not set" rather than throwing, while an
/// unknown CLASS still throws. From inside the class the private one is set.
#[test]
fn isset_on_a_static_property_is_quiet_about_undeclared_and_unreachable() {
    let src = r#"<?php
        class A {
            public static $n = null;
            private static $p = 1;
            static function inside() { return isset(self::$p) && !empty(static::$p); }
        }
        var_dump(isset(A::$nope), A::$nope ?? "d", isset(A::$n), empty(A::$nope),
                 isset(A::$p), empty(A::$p), A::inside());
        try { isset(Nope::$x); } catch (\Error $e) { echo $e->getMessage(); }"#;
    assert_eq!(
        run(src),
        "bool(false)\nstring(1) \"d\"\nbool(false)\nbool(true)\nbool(false)\nbool(true)\nbool(true)\nClass \"Nope\" not found"
    );
}

/// `private`/`protected` class constants are enforced on every read path —
/// `C::K`, `constant("C::K")` and `defined("C::K")` — naming the class written
/// at the access site. A constant's own initializer, and a static or instance
/// property default, runs in the DECLARING class's scope, so `const R = self::P
/// + 1` may read a private `P` even when the first read of `R` is from outside.
/// A trait's private constant belongs to the composing class. Expected output
/// recorded from `php` 8.5.10.
#[test]
fn class_constant_visibility_is_enforced_and_initializers_run_in_class_scope() {
    let src = r#"<?php
        class A {
            private const P = 1;
            protected const Q = 2;
            const R = self::P + 1;
            static function f() { return static::Q . self::P; }
        }
        class B extends A {
            static function g() { return self::Q . parent::Q; }
            static function h() { return parent::P; }
        }
        foreach ([fn() => A::P, fn() => A::Q, fn() => B::Q, fn() => b::Q, fn() => A::R,
                  fn() => A::f(), fn() => B::g(), fn() => B::h(),
                  fn() => constant("A::P"), fn() => defined("A::P")] as $f) {
            try { var_dump($f()); }
            catch (\Error $e) { echo get_class($e), "|", $e->getMessage(), "\n"; }
        }
        trait T { private const TP = 7; public function tp() { return self::TP; } }
        class U { use T; }
        try { echo U::TP; } catch (\Error $e) { echo $e->getMessage(), "\n"; }
        echo (new U)->tp(), "\n";
        class S {
            private const K = 3;
            private static $s = self::K * 2;
            public $i = self::K;
            public static function s() { return self::$s; }
        }
        echo S::s(), (new S)->i, "\n";"#;
    assert_eq!(
        run(src),
        "Error|Cannot access private constant A::P\n\
         Error|Cannot access protected constant A::Q\n\
         Error|Cannot access protected constant B::Q\n\
         Error|Cannot access protected constant B::Q\n\
         int(2)\n\
         string(2) \"21\"\n\
         string(2) \"22\"\n\
         Error|Cannot access private constant A::P\n\
         Error|Cannot access private constant A::P\n\
         bool(false)\n\
         Cannot access private constant U::TP\n\
         7\n\
         63\n"
    );
}

#[test]
fn foreach_over_an_object_visits_only_what_the_scope_may_see() {
    // From outside only public properties; from a method of the class all of
    // them; from a subclass, not the parent's privates.
    assert_eq!(
        run(
            r#"<?php class E { private $a = 1; protected $p = 0; public $b = 2; function it() { foreach ($this as $k => $v) echo "in:$k "; } }
class F extends E { function it2() { foreach ($this as $k => $v) echo "f:$k "; } }
$e = new E; foreach ($e as $k => $v) echo "$k=$v "; $e->it(); (new F)->it2();"#
        ),
        "b=2 in:a in:p in:b f:p f:b "
    );
}

// ── constructors ─────────────────────────────────────────────────────────────
//
// `ZEND_NEW` asks `zend_std_get_constructor` for the constructor the calling
// scope may see, BEFORE any constructor argument is evaluated. The message has
// no `method` word and names the class that DECLARED the constructor.
// Transcripts from the reference `php` 8.5.12.

#[test]
fn private_constructor_refused_from_global_scope() {
    assert_eq!(
        caught(r#"class K { private function __construct() {} } new K;"#),
        "Error|Call to private K::__construct() from global scope"
    );
    // The class operand may be dynamic; the check is the same.
    assert_eq!(
        caught(r#"class K { private function __construct() {} } $n = "K"; new $n;"#),
        "Error|Call to private K::__construct() from global scope"
    );
}

#[test]
fn inherited_protected_constructor_names_the_declaring_class() {
    let src = r#"<?php class P { protected function __construct() { echo "P ctor\n"; } }
        class C extends P {}
        class D { private function __construct() {} }
        class E extends D {}
        foreach ([fn() => new P, fn() => new C, fn() => new E] as $f) {
            try { $f(); } catch (Error $e) { echo $e->getMessage(), "\n"; }
        }"#;
    assert_eq!(
        run(src),
        "Call to protected P::__construct() from global scope\n\
         Call to protected P::__construct() from global scope\n\
         Call to private D::__construct() from global scope\n"
    );
}

#[test]
fn constructor_refusal_names_the_calling_scope() {
    // A private constructor is callable only from the class that declared it:
    // a subclass is refused even for `new` of ITSELF, and is named as the scope.
    let src = r#"<?php class K { private function __construct() {} }
        class S extends K { private function __construct() {} static function mk() { return new K; } static function mk2() { return new S; } }
        class T extends K { static function mk() { return new K; } static function mk2() { return new T; } }
        class O { function f() { return new K; } }
        foreach ([fn() => S::mk(), fn() => S::mk2(), fn() => T::mk(), fn() => T::mk2(), fn() => (new O)->f()] as $f) {
            try { echo get_class($f()), "\n"; } catch (Error $e) { echo $e->getMessage(), "\n"; }
        }"#;
    assert_eq!(
        run(src),
        "Call to private K::__construct() from scope S\n\
         S\n\
         Call to private K::__construct() from scope T\n\
         Call to private K::__construct() from scope T\n\
         Call to private K::__construct() from scope O\n"
    );
}

#[test]
fn constructor_visibility_allows_the_scopes_the_reference_allows() {
    // The static-factory idiom: `new static` / `new self` / `new K` inside the
    // declaring class; a protected constructor from anywhere on its line; a
    // closure bound into the class's scope.
    let src = r#"<?php class K { private function __construct() {} static function make() { return new static; } static function mk2() { return new self; } static function mk3() { return new K; } }
        class P { protected function __construct() { echo "P ctor\n"; } }
        class C extends P { static function mk() { return new P; } static function mk4() { return new static; } }
        var_dump(K::make() instanceof K, K::mk2() instanceof K, K::mk3() instanceof K);
        var_dump(C::mk() instanceof P, C::mk4() instanceof C);
        $b = Closure::bind(function() { return new K; }, null, K::class);
        var_dump($b() instanceof K);"#;
    assert_eq!(
        run(src),
        "bool(true)\nbool(true)\nbool(true)\nP ctor\nP ctor\nbool(true)\nbool(true)\nbool(true)\n"
    );
}

#[test]
fn constructor_visibility_is_checked_before_the_arguments_run() {
    let src = r#"<?php class K { private function __construct($x) {} }
        function arg() { echo "ARG RAN\n"; return 1; }
        try { new K(arg()); } catch (Error $e) { echo $e->getMessage(), "\n"; }
        try { new K(x: arg()); } catch (Error $e) { echo $e->getMessage(), "\n"; }"#;
    assert_eq!(
        run(src),
        "Call to private K::__construct() from global scope\n\
         Call to private K::__construct() from global scope\n"
    );
}

#[test]
fn instantiability_is_refused_before_constructor_visibility() {
    // `object_init_ex` runs before `get_constructor`: an abstract class with a
    // private constructor reports the abstract refusal.
    assert_eq!(
        caught(r#"abstract class A { private function __construct() {} } new A;"#),
        "Error|Cannot instantiate abstract class A"
    );
    // A direct call to the constructor is an ordinary method call, with the
    // ordinary wording.
    assert_eq!(
        caught(
            r#"class K { private function __construct() {} static function mk() { return new static; } } K::mk()->__construct();"#
        ),
        "Error|Call to private method K::__construct() from global scope"
    );
}

#[test]
fn an_uncaught_constructor_access_error_renders_as_a_fatal_block() {
    assert_eq!(
        output_of(r#"<?php class K { private function __construct() {} } echo "pre\n"; new K; echo "post\n";"#),
        "pre\n\nFatal error: Uncaught Error: Call to private K::__construct() from global scope in \
         Command line code:1\nStack trace:\n#0 {main}\n  thrown in Command line code \
         on line 1\n"
    );
}

#[test]
fn method_access_error_spells_the_scope_as_declared() {
    // A closure inside a method runs in its class's scope, and the message
    // spells that class as it was declared (it used to come out lowercased).
    assert_eq!(
        caught(
            r#"class K { private static function s() { return 2; } } class Zed { function f() { return (function() { return K::s(); })(); } } (new Zed)->f();"#
        ),
        "Error|Call to private method K::s() from scope Zed"
    );
}
