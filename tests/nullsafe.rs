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
