//! PHP 8 attributes (`#[Attr]`) — the parse side.
//!
//! `#[` and `#` are DIFFERENT tokens: `#` opens a line comment, `#[` opens an
//! attribute group. Getting that wrong is not a subtle metadata bug, it silently
//! deletes the attributed declaration and everything after it on the line, which
//! for `php -r` is usually the whole program. Every test here therefore checks
//! that the declaration still exists and still behaves, in each position PHP
//! allows an attribute.
//!
//! Only `#[AllowDynamicProperties]` changes behaviour (covered in
//! tests/diagnostics.rs); the rest must parse and be inert.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn a_hash_bracket_is_an_attribute_and_a_bare_hash_is_still_a_comment() {
    // The distinguishing pair. If `#[` lexed as a comment the first would print
    // nothing at all, because the class and both statements share the line.
    assert_eq!(
        run(r#"<?php #[Attr] class C { const K = 7; } echo C::K;"#),
        "7"
    );
    assert_eq!(run("<?php echo 1; # echo 2;\necho 3;"), "13");
}

#[test]
fn every_declaration_position_accepts_one() {
    let src = r#"<?php
        #[Attr]
        class C {
            #[Attr] const K = 1;
            #[Attr] public $p = 2;
            #[Attr] public static $s = 3;
            #[Attr]
            public function m(#[Attr] $x, #[Attr] int $y = 4) { return $x + $y; }
        }
        #[Attr] function f(#[Attr] $n) { return $n * 2; }
        #[Attr] interface I {}
        #[Attr] trait T { public function t() { return 5; } }
        #[Attr] enum E { #[Attr] case A; #[Attr] case B; }
        $c = new C();
        echo C::K, $c->p, C::$s, $c->m(6), f(7), count(E::cases()), E::A->name;"#;
    assert_eq!(run(src), "12310142A");
}

#[test]
fn arguments_and_grouping_are_scanned_for_balance_and_discarded() {
    // Nested brackets, nested parens, a nested array, several attributes in one
    // group, and several groups in a row all have to end at the right `]`.
    let src = r#"<?php
        #[A(1, [2, 3]), B("x")]
        #[C]
        #[D(E::class, ["k" => [1, 2]])]
        class K { const V = 9; }
        echo K::V;"#;
    assert_eq!(run(src), "9");
}

#[test]
fn a_qualified_name_keeps_its_qualification() {
    // Both forms must parse. They are not the same attribute — `\Ns\Attr` is a
    // user attribute, `\Attr` is the global one — which is what stops a
    // namespaced `AllowDynamicProperties` from opting a class out.
    assert_eq!(
        run(r#"<?php #[\Ns\Sub\Attr] #[\Other] class C { const K = 4; } echo C::K;"#),
        "4"
    );
}

#[test]
fn an_attribute_does_not_leak_onto_the_next_declaration() {
    // `#[AllowDynamicProperties]` applies to `A` only; `B` is a separate
    // declaration and still raises the notice.
    let src = r#"<?php #[AllowDynamicProperties] class A {} class B {} $a = new A(); $a->x = 1; echo $a->x; $b = new B(); $b->y = 2; echo $b->y;"#;
    assert_eq!(
        run(src),
        "1\nDeprecated: Creation of dynamic property B::$y is deprecated \
         in Command line code on line 1\n2"
    );
}
