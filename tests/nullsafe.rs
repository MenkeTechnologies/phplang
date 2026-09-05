//! End-to-end tests for the PHP 8.0 nullsafe operator `?->`: property reads and
//! method calls short-circuit to null on a null receiver, and behave like `->`
//! otherwise. Outputs are byte-verified against the reference `php`.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn nullsafe_prop_on_object_reads_property() {
    let src = r#"<?php
        class U { public $name = "Alice"; }
        $u = new U();
        echo $u?->name;"#;
    assert_eq!(run(src), "Alice");
}

#[test]
fn nullsafe_method_on_object_calls_it() {
    let src = r#"<?php
        class U { public $name = "Al"; function greet() { return "hi ".$this->name; } }
        $u = new U();
        echo $u?->greet();"#;
    assert_eq!(run(src), "hi Al");
}

#[test]
fn nullsafe_prop_on_null_is_null() {
    let src = r#"<?php
        $n = null;
        var_dump($n?->name);"#;
    assert_eq!(run(src), "NULL\n");
}

#[test]
fn nullsafe_method_on_null_is_null() {
    let src = r#"<?php
        $n = null;
        var_dump($n?->greet());"#;
    assert_eq!(run(src), "NULL\n");
}

#[test]
fn nullsafe_method_args_not_evaluated_on_null() {
    // If the receiver is null the argument expression must not run, so the
    // side-effecting `noisy()` never prints its "X".
    let src = r#"<?php
        $n = null;
        function noisy() { echo "X"; return 1; }
        $r = $n?->doThing(noisy());
        var_dump($r);
        echo "done";"#;
    assert_eq!(run(src), "NULL\ndone");
}

#[test]
fn nullsafe_chain_short_circuits() {
    let src = r#"<?php
        class U { public $address = null; }
        $u = new U();
        var_dump($u?->address?->city);"#;
    assert_eq!(run(src), "NULL\n");
}

#[test]
fn nullsafe_with_coalesce_default() {
    let src = r#"<?php
        class U { public $name = "Bea"; }
        $u = new U();
        $n = null;
        echo ($u?->name ?? "none");
        echo "|";
        echo ($n?->name ?? "none");"#;
    assert_eq!(run(src), "Bea|none");
}

// ── chain-wide short-circuit (PHP 8.0) ──────────────────────────────────────
//
// `?->` stops evaluating the WHOLE remaining chain, not just the one link that
// spelled it. Each of these used to read a property off the null the first link
// produced: the property reads warned `Attempt to read property … on null`, and
// the method call was an uncaught `Call to a member function on null`.

#[test]
fn nullsafe_short_circuits_the_rest_of_the_chain() {
    // Every diagnostic these could raise lands on stdout under the CLI, so an
    // engine that warned would fail on the captured output, not only on the
    // value.
    assert_eq!(run("<?php $n = null; var_dump($n?->a->b);"), "NULL\n");
    assert_eq!(run("<?php $n = null; var_dump($n?->a->b->c);"), "NULL\n");
    assert_eq!(run("<?php $n = null; var_dump($n?->a[\"k\"]);"), "NULL\n");
    assert_eq!(run("<?php $n = null; var_dump($n?->m()->x);"), "NULL\n");
}

#[test]
fn nullsafe_short_circuit_skips_a_later_method_call() {
    // A `->c()` on the short-circuited null is a FATAL in an engine that keeps
    // walking, so this one fails as an error rather than as wrong output.
    assert_eq!(run("<?php $n = null; var_dump($n?->a->b->c());"), "NULL\n");
}

#[test]
fn nullsafe_short_circuit_skips_later_argument_evaluation() {
    // The skipped links' arguments must not run: `f()` would print.
    let src = r#"<?php
        function f() { echo "F"; return 1; }
        $n = null;
        var_dump($n?->a->b(f()));"#;
    assert_eq!(run(src), "NULL\n");
}

#[test]
fn nullsafe_short_circuit_ends_at_the_chain_and_no_further() {
    // The enclosing expression still runs — the short-circuit is the chain's
    // extent, not the statement's.
    assert_eq!(
        run("<?php $n = null; var_dump($n?->a->b . \"x\");"),
        "string(1) \"x\"\n"
    );
    assert_eq!(
        run("<?php $n = null; echo \"a\"; var_dump($n?->a->b); echo \"z\";"),
        "aNULL\nz"
    );
}

#[test]
fn a_nullsafe_chain_in_an_argument_is_its_own_extent() {
    // Two chains in one expression: the inner one short-circuits to null and the
    // OUTER call still happens. Sharing one exit would skip `m()` as well.
    let src = r#"<?php
        class A { function m($x) { return "m:" . var_export($x, true); } }
        $a = new A(); $n = null;
        echo $a->m($n?->x->y);"#;
    assert_eq!(run(src), "m:NULL");
}

#[test]
fn nullsafe_chain_under_isset_and_coalesce() {
    assert_eq!(
        run("<?php $n = null; var_dump(isset($n?->a->b));"),
        "bool(false)\n"
    );
    assert_eq!(run("<?php $n = null; echo $n?->a->b ?? \"D\";"), "D");
    assert_eq!(
        run("<?php $n = null; var_dump(empty($n?->a->b));"),
        "bool(true)\n"
    );
}

#[test]
fn a_non_null_receiver_still_walks_the_whole_chain() {
    let src = r#"<?php
        class B { public $v = 7; function twice() { return $this->v * 2; } }
        class A { public $b; }
        $a = new A(); $a->b = new B();
        echo $a?->b->v, "|", $a->b?->twice(), "|", $a?->b?->twice();"#;
    assert_eq!(run(src), "7|14|14");
}
