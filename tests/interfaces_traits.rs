//! Interfaces (`implements`, interface `extends`), the `instanceof` operator,
//! and traits (`use Trait;` member merging).

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn implements_and_instanceof() {
    let src = r#"<?php
        interface Shape { public function area(); }
        interface Named { public function name(); }
        class Circle implements Shape, Named {
            public $r;
            function __construct($r) { $this->r = $r; }
            function area() { return $this->r * $this->r; }
            function name() { return "circle"; }
        }
        $c = new Circle(3);
        echo $c instanceof Shape ? "Y" : "N";
        echo $c instanceof Named ? "Y" : "N";
        echo $c instanceof Circle ? "Y" : "N";
        echo $c instanceof Exception ? "Y" : "N";
        echo "|", $c->name(), $c->area();"#;
    assert_eq!(run(src), "YYYN|circle9");
}

#[test]
fn interface_inheritance() {
    let src = r#"<?php
        interface A {}
        interface B extends A {}
        class C implements B {}
        $c = new C;
        echo $c instanceof A ? "Y" : "N", $c instanceof B ? "Y" : "N", $c instanceof C ? "Y" : "N";"#;
    assert_eq!(run(src), "YYY");
}

#[test]
fn instanceof_on_non_object_is_false() {
    let src = r#"<?php $x = 5; $s = "str";
        echo $x instanceof Exception ? "Y" : "N", $s instanceof Exception ? "Y" : "N";"#;
    assert_eq!(run(src), "NN");
}

#[test]
fn traits_merge_methods() {
    let src = r#"<?php
        trait Greet { public function hello() { return "hi from " . $this->who(); } }
        trait Who { public function who() { return get_class($this); } }
        class Person { use Greet, Who; }
        $p = new Person;
        echo $p->hello();"#;
    assert_eq!(run(src), "hi from Person");
}

#[test]
fn trait_with_properties() {
    let src = r#"<?php
        trait Counter { public $count = 0; public function inc() { $this->count = $this->count + 1; } }
        class Widget { use Counter; }
        $w = new Widget;
        $w->inc(); $w->inc(); $w->inc();
        echo $w->count;"#;
    assert_eq!(run(src), "3");
}

#[test]
fn class_own_method_overrides_trait() {
    let src = r#"<?php
        trait T { public function greet() { return "trait"; } }
        class C { use T; public function greet() { return "class"; } }
        echo (new C)->greet();"#;
    assert_eq!(run(src), "class");
}

#[test]
fn catch_by_interface_via_hierarchy() {
    // A user exception implementing a marker interface is catchable by its base.
    let src = r#"<?php
        class AppError extends RuntimeException {}
        try { throw new AppError("boom"); }
        catch (Exception $e) { echo $e instanceof RuntimeException ? "Y" : "N", $e->getMessage(); }"#;
    assert_eq!(run(src), "Yboom");
}

#[test]
fn insteadof_picks_the_winner_and_as_rebinds_the_loser() {
    // The construct the whole adaptation block exists for: `insteadof` drops
    // B's `hi` from the merge, and `as` still reaches it — so the class ends up
    // with BOTH implementations, under different names.
    let src = r#"<?php
        trait A { public function hi() { return "A::hi"; } public function bye() { return "A::bye"; } }
        trait B { public function hi() { return "B::hi"; } }
        class C { use A, B { A::hi insteadof B; B::hi as bHi; } }
        $c = new C;
        echo $c->hi(), "|", $c->bHi(), "|", $c->bye();"#;
    assert_eq!(run(src), "A::hi|B::hi|A::bye");
}

#[test]
fn alias_adds_a_name_without_removing_the_original() {
    let src = r#"<?php
        trait A { public function hi() { return "A::hi"; } private function sec() { return "A::sec"; } }
        class C { use A { hi as other; sec as public psec; } }
        $c = new C;
        echo $c->other(), "|", $c->psec(), "|", $c->hi();"#;
    assert_eq!(run(src), "A::hi|A::sec|A::hi");
}

#[test]
fn as_without_a_new_name_only_moves_the_visibility() {
    // `hi as protected;` re-marks the binding the class already has; calling it
    // from outside is then the ordinary protected-access Error.
    let src = r#"<?php
        trait A { public function hi() { return "A::hi"; } }
        class C { use A { hi as protected; } }
        try { (new C)->hi(); } catch (Error $e) { echo $e->getMessage(); }"#;
    assert_eq!(
        run(src),
        "Call to protected method C::hi() from global scope"
    );
}

#[test]
fn qualified_alias_binds_the_named_trait_not_the_merged_method() {
    let src = r#"<?php
        trait A { public function hi() { return "A::hi " . static::class; } }
        class D { use A { A::hi as dHi; } }
        echo (new D)->dHi();"#;
    assert_eq!(run(src), "A::hi D");
}

/// Run `src` and return everything it wrote, *including* a fatal-error block.
///
/// A refused `use` is a fatal, and `eval_capture` drops the captured output
/// when the run fails — which is exactly the case under test here.
fn output_of(src: &str) -> String {
    phplang::host::reset_host();
    phplang::host::with_host(|h| h.begin_capture());
    if let Ok(prog) = phplang::compile(src) {
        let _ = phplang::run_compiled(prog);
    }
    phplang::host::with_host(|h| h.end_capture())
}

#[test]
fn an_unresolved_trait_collision_is_a_fatal_after_the_output_so_far() {
    // The reference binds a trait-using class at RUN time, so everything echoed
    // before the declaration is printed and only then does the link fail. Taken
    // verbatim from `php` 8.5.9.
    let src = r#"<?php
echo "before\n";
trait A { public function hi() { return "A"; } }
trait B { public function hi() { return "B"; } }
class K { use A, B; }
echo "after\n";"#;
    assert_eq!(
        output_of(src),
        "before\n\nFatal error: Trait method B::hi has not been applied as K::hi, because of \
         collision with A::hi in Command line code on line 5\nStack trace:\n#0 {main}\n"
    );
}

#[test]
fn excluding_only_one_of_three_colliding_traits_still_collides() {
    // `insteadof` excludes the loser rather than electing the winner, so A and C
    // are both still in the running and the pair reported is (first, second).
    let src = r#"<?php
trait A { public function hi() { return "A"; } }
trait B { public function hi() { return "B"; } }
trait C { public function hi() { return "C"; } }
class K { use A, B, C { A::hi insteadof B; } }"#;
    assert_eq!(
        output_of(src),
        "\nFatal error: Trait method C::hi has not been applied as K::hi, because of collision \
         with A::hi in Command line code on line 5\nStack trace:\n#0 {main}\n"
    );
}

#[test]
fn an_unqualified_alias_of_an_ambiguous_method_is_refused() {
    let src = r#"<?php
trait A { public function hi() { return "A"; } }
trait B { public function hi() { return "B"; } }
class K { use A, B { A::hi insteadof B; hi as x; } }"#;
    assert_eq!(
        output_of(src),
        "\nFatal error: An alias was defined for method hi(), which exists in both A and B. Use \
         A::hi or B::hi to resolve the ambiguity in Command line code on line 4\nStack \
         trace:\n#0 {main}\n"
    );
}

#[test]
fn an_adaptation_may_only_name_a_trait_the_class_actually_uses() {
    let src = r#"<?php
trait A { public function f() {} }
trait B { public function f() {} }
class K { use A { B::f as z; } }"#;
    assert_eq!(
        output_of(src),
        "\nFatal error: Required Trait B wasn't added to K in Command line code on line 4\n\
         Stack trace:\n#0 {main}\n"
    );
}

#[test]
fn an_alias_of_a_method_no_trait_declares_is_refused() {
    let src = r#"<?php
trait A { public function f() {} }
class K { use A { A::nope as z; } }"#;
    assert_eq!(
        output_of(src),
        "\nFatal error: An alias was defined for A::nope but this method does not exist in \
         Command line code on line 3\nStack trace:\n#0 {main}\n"
    );
}

#[test]
fn a_use_of_an_undeclared_trait_is_a_throwable_error() {
    // Unlike every other link failure above, this one goes through the ordinary
    // exception machinery, so it is catchable.
    let src = r#"<?php
        trait A { public function f() {} }
        try { new class { use A, Nope; }; }
        catch (Error $e) { echo get_class($e), ": ", $e->getMessage(); }"#;
    assert_eq!(run(src), "Error: Trait \"Nope\" not found");
}

// ── interface constants ─────────────────────────────────────────────────────
//
// A `const` declared in an interface is inherited by everything that implements
// or extends it. The constant lookup used to walk only the `parent` chain, so
// every one of these was `Error: Undefined constant C::K`.

#[test]
fn a_class_inherits_the_constants_of_the_interface_it_implements() {
    let src = r#"<?php
        interface I { const K = 5; }
        class C implements I {}
        echo C::K;"#;
    assert_eq!(run(src), "5");
}

#[test]
fn self_and_static_reach_an_inherited_interface_constant() {
    let src = r#"<?php
        interface I { const K = 5; }
        class C implements I {
            public function a() { return self::K; }
            public function b() { return static::K; }
        }
        $c = new C();
        echo $c->a(), $c->b();"#;
    assert_eq!(run(src), "55");
}

#[test]
fn an_interface_constant_is_inherited_through_an_extending_interface() {
    let src = r#"<?php
        interface I { const K = 5; }
        interface J extends I {}
        class C implements J {}
        echo C::K, J::K;"#;
    assert_eq!(run(src), "55");
}

#[test]
fn a_class_constant_shadows_the_interface_one() {
    // The parent CHAIN is searched before any interface, so the nearer class
    // wins at every depth — `P` still answers with the interface's value.
    let src = r#"<?php
        interface I { const K = 1; }
        class P implements I {}
        class C extends P { const K = 2; }
        echo C::K, P::K;"#;
    assert_eq!(run(src), "21");
}

#[test]
fn a_class_that_implements_nothing_still_has_no_such_constant() {
    // The widened lookup must not start finding constants of interfaces the
    // class has no relation to.
    let src = r#"<?php
        interface I { const K = 1; }
        class C {}
        try { echo C::K; } catch (Error $e) { echo $e->getMessage(); }"#;
    assert_eq!(run(src), "Undefined constant C::K");
}
