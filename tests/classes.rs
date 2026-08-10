//! End-to-end tests for core PHP OOP: class declaration, `new`, property
//! access, instance/static methods, class constants, `$this`, single
//! inheritance, `self::`/`parent::` scope resolution, constructor property
//! promotion, `::class`, and array-valued property mutation.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

/// The error message from a program expected to fail (e.g. instantiating an
/// abstract class), for asserting the PHP diagnostic text.
fn run_err(src: &str) -> String {
    eval_capture(src).expect_err("expected an error")
}

#[test]
fn instantiating_an_abstract_class_is_an_error() {
    // PHP: `new` on an abstract class is a fatal `Error: Cannot instantiate
    // abstract class Shape`.
    let src = r#"<?php
        abstract class Shape { abstract public function area(): float; }
        $s = new Shape();"#;
    assert!(
        run_err(src).contains("Cannot instantiate abstract class Shape"),
        "got: {}",
        run_err(src)
    );
}

#[test]
fn instantiating_an_interface_is_an_error() {
    let src = r#"<?php
        interface Speaker { public function speak(): string; }
        $s = new Speaker();"#;
    assert!(
        run_err(src).contains("Cannot instantiate interface Speaker"),
        "got: {}",
        run_err(src)
    );
}

#[test]
fn a_concrete_subclass_of_an_abstract_class_is_instantiable() {
    let src = r#"<?php
        abstract class Shape {
            abstract public function area(): float;
            public function name() { return "shape"; }
        }
        class Circle extends Shape { public function area(): float { return 3.14; } }
        $c = new Circle();
        echo $c->name(), ":", $c->area();"#;
    assert_eq!(run(src), "shape:3.14");
}

#[test]
fn construct_binds_constructor_arg_to_property() {
    let src = r#"<?php
        class C {
            public $p;
            function __construct($x) { $this->p = $x; }
        }
        $o = new C(5);
        echo $o->p;"#;
    assert_eq!(run(src), "5");
}

#[test]
fn instance_method_reads_this() {
    let src = r#"<?php
        class C {
            public $p;
            function __construct($x) { $this->p = $x; }
            function doubled() { return $this->p * 2; }
        }
        $o = new C(5);
        echo $o->doubled();"#;
    assert_eq!(run(src), "10");
}

#[test]
fn property_default_value() {
    let src = r#"<?php
        class C { public $n = 42; }
        $o = new C();
        echo $o->n;"#;
    assert_eq!(run(src), "42");
}

#[test]
fn property_assignment_and_readback() {
    let src = r#"<?php
        class Box { public $v; }
        $b = new Box();
        $b->v = "hello";
        echo $b->v;"#;
    assert_eq!(run(src), "hello");
}

#[test]
fn class_constant_read() {
    let src = r#"<?php
        class M { const K = 99; }
        echo M::K;"#;
    assert_eq!(run(src), "99");
}

#[test]
fn static_method_reads_self_const() {
    let src = r#"<?php
        class M {
            const V = 7;
            static function s() { return self::V; }
        }
        echo M::s();"#;
    assert_eq!(run(src), "7");
}

#[test]
fn single_inheritance_calls_parent_method() {
    let src = r#"<?php
        class A {
            const K = 10;
            function who() { return "A"; }
        }
        class B extends A {
            function who() { return "B+" . parent::who(); }
        }
        $b = new B();
        echo B::K, ":", $b->who(), ":", A::K;"#;
    assert_eq!(run(src), "10:B+A:10");
}

#[test]
fn inherited_property_default_from_parent() {
    let src = r#"<?php
        class A { public $base = 1; }
        class B extends A { public $extra = 2; }
        $b = new B();
        echo $b->base + $b->extra;"#;
    assert_eq!(run(src), "3");
}

#[test]
fn constructor_property_promotion() {
    // `public int $x` also assigns $this->x; a promoted param may have a default.
    let src = r#"<?php
        class Pt {
            function __construct(public int $x, public int $y = 9) {}
        }
        $p = new Pt(3);
        echo $p->x, ",", $p->y;"#;
    assert_eq!(run(src), "3,9");
}

#[test]
fn class_keyword_yields_class_name_string() {
    let src = r#"<?php
        class Foo {}
        echo Foo::class;"#;
    assert_eq!(run(src), "Foo");
}

#[test]
fn self_class_keyword_inside_method() {
    // `self::class` resolves to the declared class name, preserving its original
    // case (matches PHP: "Widget").
    let src = r#"<?php
        class Widget {
            function name() { return self::class; }
        }
        $w = new Widget();
        echo $w->name();"#;
    assert_eq!(run(src), "Widget");
}

#[test]
fn array_valued_property_append() {
    // `$this->items[] = x` mutates the array held in the property (the P0 case).
    let src = r#"<?php
        class Bag {
            public $items = [];
            function add($x) { $this->items[] = $x; return $this; }
        }
        $b = new Bag();
        $b->add(1);
        $b->add(2);
        $b->add(3);
        echo implode(",", $b->items);"#;
    assert_eq!(run(src), "1,2,3");
}

#[test]
fn array_valued_property_keyed_set() {
    let src = r#"<?php
        class Registry {
            public $map = [];
            function set($k, $v) { $this->map[$k] = $v; }
            function get($k) { return $this->map[$k]; }
        }
        $r = new Registry();
        $r->set("a", 1);
        $r->set("b", 2);
        echo $r->get("a"), $r->get("b");"#;
    assert_eq!(run(src), "12");
}

#[test]
fn property_increment_and_compound_assign() {
    let src = r#"<?php
        class Cnt {
            public $n = 0;
            function inc() { $this->n++; return $this->n; }
            function bump($by) { $this->n += $by; return $this->n; }
        }
        $c = new Cnt();
        echo $c->inc(), $c->inc(), ":", $c->bump(10);"#;
    assert_eq!(run(src), "12:12");
}

#[test]
fn objects_are_reference_handles() {
    // Passing an object shares the same instance; mutating through one handle is
    // visible through the other.
    let src = r#"<?php
        class Ref { public $v = 1; }
        function bump($o) { $o->v = 100; }
        $a = new Ref();
        $b = $a;
        bump($a);
        echo $b->v;"#;
    assert_eq!(run(src), "100");
}

#[test]
fn fluent_method_chaining() {
    let src = r#"<?php
        class Builder {
            public $parts = [];
            function add($p) { $this->parts[] = $p; return $this; }
            function build() { return implode("-", $this->parts); }
        }
        $b = new Builder();
        echo $b->add("a")->add("b")->add("c")->build();"#;
    assert_eq!(run(src), "a-b-c");
}

// ── property overloading: __get / __set / __isset ────────────────────────────
//
// The magic methods fire for a property the object does not carry — undeclared,
// `unset`, or out of reach of the reading scope — and are consulted BEFORE any
// access error. Every expectation is verbatim output of the same program under
// the reference `php` 8.5.9.

/// A class carrying whichever magic methods `magic` declares, plus the private
/// bag they read and write.
fn with_magic(magic: &str, body: &str) -> String {
    run(&format!(
        r#"<?php class C {{ private $bag = [];
            {magic}
            }}
            $o = new C; {body}"#
    ))
}

const GET: &str = r#"function __get($n) { echo "[G$n]"; return $this->bag[$n] ?? "g"; }"#;
const SET: &str = r#"function __set($n, $v) { echo "[S$n]"; $this->bag[$n] = $v; }"#;
const ISSET: &str = r#"function __isset($n) { echo "[I$n]"; return isset($this->bag[$n]); }"#;

#[test]
fn magic_set_stores_without_creating_a_real_property() {
    let src = format!("{SET} {GET}");
    assert_eq!(
        with_magic(
            &src,
            r#"$o->q = 7; echo "|", $o->q, "|", count(get_object_vars($o));"#
        ),
        // `get_object_vars` from outside sees only PUBLIC properties, and the
        // magic write created none — so there is nothing left to count.
        "[Sq]|[Gq]7|0"
    );
}

#[test]
fn magic_get_fires_for_a_property_that_was_unset() {
    // Gone is indistinguishable from never-declared, so a declared PUBLIC
    // property routes through `__get` once it has been removed.
    let src = r#"<?php class C { public $p = 1; function __get($n) { return "g:$n"; } }
        $o = new C; echo $o->p, "|"; unset($o->p); echo $o->p;"#;
    assert_eq!(run(src), "1|g:p");
}

#[test]
fn magic_get_is_consulted_before_the_visibility_error() {
    let src = r#"<?php class C { private $p = 1; function __get($n) { return "g:$n"; } }
        echo (new C)->p;"#;
    assert_eq!(run(src), "g:p");
}

#[test]
fn a_magic_method_is_not_re_entered_for_the_property_it_is_handling() {
    // Without the guard this recurses until the stack gives out. The INNER read
    // of the same name finds no property and takes the ordinary path, so `??`
    // supplies the default.
    let src = r#"<?php class C { function __get($n) { echo "[G$n]"; return $this->q ?? "d"; } }
        var_dump((new C)->q);"#;
    assert_eq!(run(src), "[Gq]string(1) \"d\"\n");
}

#[test]
fn isset_empty_and_coalesce_consult_different_magic_methods() {
    // `isset` asks `__isset` alone — true even though `__get` is never called.
    assert_eq!(
        with_magic(ISSET, r#"var_dump(isset($o->q));"#),
        "[Iq]bool(false)\n"
    );
    // With no `__isset` at all, `isset` is false however `__get` would answer.
    assert_eq!(with_magic(GET, "var_dump(isset($o->q));"), "bool(false)\n");
    // `??` falls back to `__get` when there is no `__isset` …
    assert_eq!(
        with_magic(GET, r#"var_dump($o->q ?? "D");"#),
        "[Gq]string(1) \"g\"\n"
    );
    // … but `empty()` will not read a property no `__isset` vouched for.
    assert_eq!(with_magic(GET, "var_dump(empty($o->q));"), "bool(true)\n");
    // A true `__isset` is what unlocks `__get` for both.
    let both = format!("{GET} {ISSET} {SET}");
    assert_eq!(
        with_magic(&both, r#"$o->q = 1; var_dump(empty($o->q), $o->q ?? "D");"#),
        // Both arguments are evaluated before `var_dump` prints anything, so
        // all four magic calls come first.
        "[Sq][Iq][Gq][Iq][Gq]bool(false)\nint(1)\n"
    );
}

#[test]
fn a_plain_read_asks_magic_get_where_isset_asks_magic_isset() {
    // `@` is not an isset-mode read: it evaluates the operand normally.
    let both = format!("{GET} {ISSET}");
    assert_eq!(
        with_magic(&both, r#"var_dump(@$o->q);"#),
        "[Gq]string(1) \"g\"\n"
    );
    assert_eq!(
        with_magic(&both, r#"var_dump(isset($o->q));"#),
        "[Iq]bool(false)\n"
    );
}

#[test]
fn a_read_modify_write_uses_magic_set_only_when_magic_get_supplied_the_value() {
    // Both halves present: the pair handles it, and no dynamic property is made.
    let both = format!("{GET} {SET}");
    assert_eq!(
        with_magic(&both, r#"$o->n = 1; $o->n += 2; echo "|", $o->n;"#),
        "[Sn][Gn][Sn]|[Gn]3"
    );
    // `__set` WITHOUT `__get` is not enough to divert the write: the reference
    // reads and writes the property directly, so the deprecation and the
    // undefined-property warning both appear and `__set` never runs.
    let src = r#"<?php class C { function __set($n, $v) { echo "[S$n]"; } }
        $o = new C; $o->q += 3; echo "|", $o->q;"#;
    assert_eq!(
        run(src),
        "\nDeprecated: Creation of dynamic property C::$q is deprecated in \
         Command line code on line 2\n\nWarning: Undefined property: C::$q in \
         Command line code on line 2\n|3"
    );
}

#[test]
fn a_write_through_magic_set_never_reports_a_dynamic_property() {
    // The deprecation is about creating a real property, and `__set` creates none.
    assert_eq!(with_magic(SET, "$o->zz = 1; echo \"|ok\";"), "[Szz]|ok");
}

#[test]
fn magic_get_and_set_are_skipped_from_inside_the_class() {
    // A private property is directly reachable there, so nothing magic fires.
    let src =
        format!("{GET} {SET} function go() {{ $this->bag = [1]; return count($this->bag); }}");
    assert_eq!(with_magic(&src, "echo $o->go();"), "1");
}

// ── __call / __callStatic ────────────────────────────────────────────────────

/// A method the class does not declare is handed to `__call` with its arguments
/// packed into ONE array — not spread as parameters.
#[test]
fn call_catch_all_receives_the_name_and_an_argument_array() {
    let src = r#"<?php class C {
            public function __call($n, $a) { return "$n/" . count($a) . "/" . implode(",", $a); }
        }
        echo (new C)->whatever(1, 2, 3);"#;
    assert_eq!(run(src), "whatever/3/1,2,3");
}

/// The static form is a DIFFERENT method: a class with only `__call` does not
/// answer `C::m()`, and one with only `__callStatic` does not answer `$o->m()`.
#[test]
fn call_static_catch_all_is_separate_from_the_instance_one() {
    let both = r#"class C {
            public function __call($n, $a) { return "inst:$n"; }
            public static function __callStatic($n, $a) { return "static:$n"; }
        }"#;
    assert_eq!(run(&format!("<?php {both} echo (new C)->m();")), "inst:m");
    assert_eq!(run(&format!("<?php {both} echo C::m();")), "static:m");
    // Only `__callStatic`: the instance call has no catch-all to fall back to.
    let src = r#"<?php class D { public static function __callStatic($n, $a) { return "s"; } }
        try { (new D)->m(); } catch (Error $e) { echo $e->getMessage(); }"#;
    assert_eq!(run(src), "Call to undefined method D::m()");
}

/// `__call` is reached through every call form, not just `$o->m()`.
#[test]
fn call_catch_all_is_reached_through_call_user_func() {
    let src = r#"<?php class C { public function __call($n, $a) { return "c:$n:" . ($a[0] ?? "-"); } }
        $o = new C;
        echo call_user_func([$o, "a"], 1), "|", call_user_func_array([$o, "b"], [2]);"#;
    assert_eq!(run(src), "c:a:1|c:b:2");
}

/// A `__call`-backed method is callable but does NOT exist: the two predicates
/// disagree on purpose, and the reference is what settles it.
#[test]
fn a_catch_all_method_is_callable_but_does_not_exist() {
    let src = r#"<?php class C { public function __call($n, $a) {} }
        $o = new C;
        var_dump(method_exists($o, "nope"), is_callable([$o, "nope"]));"#;
    assert_eq!(run(src), "bool(false)\nbool(true)\n");
}

/// Calling something that is not there at all is a catchable `Error`, not a
/// host-level abort — the same class the reference throws.
#[test]
fn an_undefined_method_call_throws_a_catchable_error() {
    let src = r#"<?php class C {}
        try { (new C)->m(); } catch (Error $e) { echo get_class($e), "|", $e->getMessage(); }"#;
    assert_eq!(run(src), "Error|Call to undefined method C::m()");
    let stat = r#"<?php class C {}
        try { C::m(); } catch (Error $e) { echo get_class($e), "|", $e->getMessage(); }"#;
    assert_eq!(run(stat), "Error|Call to undefined method C::m()");
}

#[test]
fn an_undefined_function_call_throws_a_catchable_error() {
    let src = r#"<?php try { definitely_not_a_function(); }
        catch (Error $e) { echo get_class($e), "|", $e->getMessage(); }"#;
    assert_eq!(
        run(src),
        "Error|Call to undefined function definitely_not_a_function()"
    );
}

// ── object comparison ────────────────────────────────────────────────────────

/// `==` on objects compares CLASS plus properties, so two distinct instances can
/// be equal; `===` is identity and never is. An array is equal to neither.
#[test]
fn loose_equality_compares_objects_by_class_and_properties() {
    let src = r#"<?php class P { public $a = 1; protected $b = 2; }
        class Q { public $a = 1; protected $b = 2; }
        $x = new P; $y = new P; $z = new Q;
        var_dump($x == $y, $x === $y, $x == $z);
        $y->a = 9;
        var_dump($x == $y);"#;
    assert_eq!(
        run(src),
        "bool(true)\nbool(false)\nbool(false)\nbool(false)\n"
    );
}

/// `Stringable` is implemented automatically by any class with `__toString`,
/// whether or not the class names the interface.
#[test]
fn stringable_is_implied_by_tostring() {
    let src = r#"<?php class S { public function __toString(): string { return "s"; } }
        class T {}
        var_dump(new S instanceof Stringable, new T instanceof Stringable);"#;
    assert_eq!(run(src), "bool(true)\nbool(false)\n");
}

#[test]
fn anonymous_class_extends_implements_and_takes_constructor_args() {
    let src = r#"<?php
        interface I { public function f(); }
        class P { public $a = 1; public function g() { return "P::g"; } }
        $o = new class(42) extends P implements I {
            public $b;
            public function __construct($b) { $this->b = $b; }
            public function f() { return "anon:" . $this->a . ":" . $this->b; }
        };
        echo $o->f(), "|", $o->g(), "|";
        echo $o instanceof I ? "Y" : "N", $o instanceof P ? "Y" : "N";"#;
    assert_eq!(run(src), "anon:1:42|P::g|YY");
}

#[test]
fn one_anonymous_class_declaration_is_one_class_however_often_it_runs() {
    // The declaration is compiled once, so a `new class` in a loop yields three
    // instances of a single class — not three classes.
    let src = r#"<?php
        $seen = [];
        for ($i = 0; $i < 3; $i++) { $seen[] = get_class(new class {}); }
        echo count(array_unique($seen));"#;
    assert_eq!(run(src), "1");
}

#[test]
fn an_anonymous_class_is_named_for_its_base_then_its_site() {
    // `Base@anonymous\0<script>:<line>$<n>` — the parent, else the first
    // interface, else the literal `class`; then a per-compilation counter,
    // numbered in SOURCE order rather than the order the sites run.
    let src = "<?php\n\
        interface I {}\n\
        class P {}\n\
        function late() { return new class extends P {}; }\n\
        $first = new class implements I {};\n\
        echo late() instanceof P ? '' : 'x';\n\
        echo get_class($first), \"\\n\", get_class(late()), \"\\n\", get_class(new class {});";
    assert_eq!(
        run(src),
        "I@anonymous\0Command line code:5$1\n\
         P@anonymous\0Command line code:4$0\n\
         class@anonymous\0Command line code:7$2"
    );
}

#[test]
fn an_anonymous_class_shows_only_the_head_of_its_name_in_a_message() {
    // Everything that renders the name for a human stops at the NUL, the way
    // the reference's C-string formatting does; `get_class` keeps the whole.
    let src = r#"<?php
        class P {}
        $o = new class extends P { public $x = 1; };
        var_dump($o);
        echo get_debug_type($o), "|";
        try { $o->nope(); } catch (Error $e) { echo $e->getMessage(); }"#;
    assert_eq!(
        run(src),
        "object(P@anonymous)#1 (1) {\n  [\"x\"]=>\n  int(1)\n}\n\
         P@anonymous|Call to undefined method P@anonymous::nope()"
    );
}

// ── `$expr::` — the class named by a value rather than a bareword ───────────

#[test]
fn a_value_may_name_the_class_left_of_a_double_colon() {
    // A string names the class (case-insensitively, as every class lookup is),
    // and an object contributes its own — for constants, static methods and
    // static properties alike.
    let src = r#"<?php
        class C {
            const K = 7;
            public static $sp = 5;
            public static function m() { return "w"; }
        }
        $s = "c";
        $o = new C();
        $a = ["k" => "C"];
        echo $s::K, $s::m(), $s::$sp, "|", $o::K, $o::m(), $o::$sp, "|", $a["k"]::K;
        $s::$sp = 8;
        $s::$sp++;
        echo "|", C::$sp;"#;
    assert_eq!(run(src), "7w5|7w5|7|9");
}

#[test]
fn class_on_a_value_answers_only_for_an_object() {
    // `::class` is NOT "resolve the class then name it": the class-name string
    // every other `::` accepts is refused here, and the message names the value
    // the way the reference does — `true`/`false`, not `bool`.
    let src = r#"<?php
        class MyThing {}
        function why($v) {
            try { return $v::class; } catch (Throwable $e) { return $e->getMessage(); }
        }
        echo (new MyThing())::class, "\n";
        foreach (["C", 5, 1.5, true, false, null, []] as $v) { echo why($v), "\n"; }"#;
    assert_eq!(
        run(src),
        "MyThing\n\
         Cannot use \"::class\" on string\n\
         Cannot use \"::class\" on int\n\
         Cannot use \"::class\" on float\n\
         Cannot use \"::class\" on true\n\
         Cannot use \"::class\" on false\n\
         Cannot use \"::class\" on null\n\
         Cannot use \"::class\" on array\n"
    );
}

#[test]
fn a_double_colon_on_a_value_that_names_no_class_is_a_catchable_error() {
    // Three distinct failures, and PHP tells them apart: a value that could
    // never be a class, a string naming a class that does not exist, and a
    // class that exists without the member.
    let src = r#"<?php
        class C { const A = 1; }
        function why($src) {
            try { return $src(); } catch (Throwable $e) { return get_class($e) . ": " . $e->getMessage(); }
        }
        echo why(function() { $v = 5; return $v::A; }), "\n";
        echo why(function() { $v = "NoSuch"; return $v::A; }), "\n";
        echo why(function() { $v = "C"; return $v::NOPE; }), "\n";
        echo why(function() { $v = "C"; return $v::$nope; }), "\n";
        echo why(function() { $v = "NoSuch"; return $v::m(); }), "\n";
        echo why(function() { return NoSuch::m(); }), "\n";"#;
    assert_eq!(
        run(src),
        "Error: Class name must be a valid object or a string\n\
         Error: Class \"NoSuch\" not found\n\
         Error: Undefined constant C::NOPE\n\
         Error: Access to undeclared static property C::$nope\n\
         Error: Class \"NoSuch\" not found\n\
         Error: Class \"NoSuch\" not found\n"
    );
}

#[test]
fn a_dynamic_class_reference_is_a_variable_an_arrow_fn_must_capture() {
    // `$cls::K` reads `$cls`, so an arrow function that says it has to close
    // over it — a walk that skips the class position silently produces null.
    let src = r#"<?php
        class C { const K = 7; public static function m($x) { return $x * 2; } }
        $cls = "C";
        $f = fn() => $cls::K;
        $g = fn($n) => $cls::m($n);
        echo $f(), "|", $g(21), "|", ($cls::m(...))(3);"#;
    assert_eq!(run(src), "7|42|6");
}

// ── clone ───────────────────────────────────────────────────────────────────

#[test]
fn clone_copies_arrays_by_value_and_leaves_objects_shared() {
    // PHP's clone is shallow in exactly one direction: an array property is a
    // value and travels as a copy, an object property is a handle and stays
    // shared with the original.
    let src = r#"<?php
        class Inner { public $v = 1; }
        class Outer {
            public $n = 29;
            public $arr = [1, 2];
            public $in;
            public function __construct() { $this->in = new Inner(); }
        }
        $a = new Outer();
        $b = clone $a;
        $b->n = 30;
        $b->arr[] = 3;
        $b->in->v = 99;
        echo $a->n, "|", count($a->arr), "|", $a->in->v, "|";
        var_dump($a->in === $b->in, $a === $b);"#;
    assert_eq!(run(src), "29|2|99|bool(true)\nbool(false)\n");
}

#[test]
fn clone_runs_the_clone_hook_on_the_copy_only() {
    // `__clone` runs with `$this` bound to the NEW object, so a hook that
    // rewrites a property must leave the original untouched. `clone` also binds
    // tighter than any operator, so it takes the postfix expression beside it.
    let src = r#"<?php
        class W { public $x = 1; public function __clone() { $this->x = 100; } }
        class Holder { public $o; public function __construct() { $this->o = new W(); } }
        $h = new Holder();
        $c = clone $h->o;
        echo $h->o->x, "|", $c->x, "|", (clone new W())->x, "|";
        var_dump(clone $h->o instanceof W);"#;
    assert_eq!(run(src), "1|100|100|bool(true)\n");
}

#[test]
fn cloning_something_that_is_not_a_clonable_object_is_an_error() {
    // A scalar or array never was an object; a live generator IS one but holds
    // a suspended stack, and PHP refuses it by name rather than by type.
    let src = r#"<?php
        function gen() { yield 1; }
        function why($v) {
            try { $c = clone $v; return "ok"; }
            catch (Throwable $e) { return get_class($e) . ": " . $e->getMessage(); }
        }
        foreach ([5, "s", null, []] as $v) { echo why($v), "\n"; }
        echo why(gen()), "\n";"#;
    assert_eq!(
        run(src),
        "TypeError: clone(): Argument #1 ($object) must be of type object, int given\n\
         TypeError: clone(): Argument #1 ($object) must be of type object, string given\n\
         TypeError: clone(): Argument #1 ($object) must be of type object, null given\n\
         TypeError: clone(): Argument #1 ($object) must be of type object, array given\n\
         Error: Trying to clone an uncloneable object of class Generator\n"
    );
}

// ── readonly ────────────────────────────────────────────────────────────────

#[test]
fn a_readonly_property_takes_exactly_one_write() {
    // Every later write is refused however it is spelled, and the class named
    // is the one that DECLARED the property, not the one written through.
    let src = r#"<?php
        class C {
            public readonly int $id;
            public function __construct(int $i) { $this->id = $i; }
            public function bump() { $this->id++; }
            public function reinit() { $this->id = 99; }
        }
        class E extends C { public function set() { $this->id = 5; } }
        $c = new C(9);
        $e = new E(1);
        function why(callable $f) {
            try { $f(); return "ok"; } catch (Throwable $e) { return $e->getMessage(); }
        }
        echo why(function() use ($c) { $c->id = 10; }), "\n";
        echo why(function() use ($c) { $c->id += 1; }), "\n";
        echo why(fn() => $c->bump()), "\n";
        echo why(fn() => $c->reinit()), "\n";
        echo why(fn() => $e->set()), "\n";
        echo $c->id, $e->id, "\n";"#;
    assert_eq!(
        run(src),
        "Cannot modify readonly property C::$id\n\
         Cannot modify readonly property C::$id\n\
         Cannot modify readonly property C::$id\n\
         Cannot modify readonly property C::$id\n\
         Cannot modify readonly property C::$id\n\
         91\n"
    );
}

#[test]
fn an_uninitialized_readonly_property_may_only_be_written_from_its_own_line() {
    // The one write must come from the declaring class or a subclass — PHP 8.4
    // gave the rule `protected(set)` shape, and the refusal names the scope the
    // write came from.
    let src = r#"<?php
        class C { public readonly int $v; }
        class Sub extends C { public function set() { $this->v = 3; } }
        class Other { public function set(C $c) { $c->v = 4; } }
        function why(callable $f) {
            try { $f(); return "ok"; } catch (Throwable $e) { return $e->getMessage(); }
        }
        $s = new Sub();
        echo why(fn() => $s->set()), "|", $s->v, "\n";
        echo why(fn() => (new Other)->set(new C())), "\n";
        echo why(function() { $c = new C(); $c->v = 5; }), "\n";"#;
    assert_eq!(
        run(src),
        "ok|3\n\
         Cannot modify protected(set) readonly property C::$v from scope Other\n\
         Cannot modify protected(set) readonly property C::$v from global scope\n"
    );
}

#[test]
fn a_readonly_property_refuses_unset_and_modification_through_it() {
    // Writing THROUGH the property is refused with its own wording: PHP will
    // not hand out a modifiable reference into a readonly property at all.
    let src = r#"<?php
        class C {
            public readonly array $tags;
            public function __construct() { $this->tags = ["a"]; }
        }
        $c = new C();
        function why(callable $f) {
            try { $f(); return "ok"; } catch (Throwable $e) { return $e->getMessage(); }
        }
        echo why(function() use ($c) { $c->tags[] = "x"; }), "\n";
        echo why(function() use ($c) { unset($c->tags); }), "\n";
        echo count($c->tags), "\n";"#;
    assert_eq!(
        run(src),
        "Cannot indirectly modify readonly property C::$tags\n\
         Cannot unset readonly property C::$tags\n\
         1\n"
    );
}

#[test]
fn readonly_reaches_promoted_parameters_readonly_classes_and_traits() {
    // Three spellings of the same declaration, all of which have to register
    // the property as readonly or the enforcement silently does not apply.
    let src = r#"<?php
        class P { public function __construct(public readonly int $a) {} }
        readonly class R { public function __construct(public int $b) {} }
        trait T { public readonly string $t; }
        class U { use T; public function __construct() { $this->t = "z"; } }
        function why(callable $f) {
            try { $f(); return "ok"; } catch (Throwable $e) { return $e->getMessage(); }
        }
        echo why(function() { (new P(1))->a = 2; }), "\n";
        echo why(function() { (new R(1))->b = 2; }), "\n";
        echo why(function() { (new U())->t = "q"; }), "\n";"#;
    assert_eq!(
        run(src),
        "Cannot modify readonly property P::$a\n\
         Cannot modify readonly property R::$b\n\
         Cannot modify readonly property U::$t\n"
    );
}

#[test]
fn the_clone_hook_is_the_one_place_a_readonly_property_reopens() {
    // PHP 8.3 lets `__clone` give the copy a fresh identity; the reopening ends
    // with the hook, so the copy is locked again on the way out.
    let src = r#"<?php
        class R {
            public function __construct(public readonly int $id) {}
            public function __clone(): void { $this->id = $this->id + 1; }
        }
        $r = new R(1);
        $r2 = clone $r;
        echo $r->id, $r2->id, "|";
        try { $r2->id = 50; } catch (Throwable $e) { echo $e->getMessage(); }"#;
    assert_eq!(run(src), "12|Cannot modify readonly property R::$id");
}
