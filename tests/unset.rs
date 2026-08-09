//! The `unset()` construct: remove variables and array elements (no reindex).

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn unset_array_element_keeps_other_keys() {
    // Removing an element leaves a hole; remaining integer keys are NOT renumbered.
    let src = r#"<?php $a = [1, 2, 3]; unset($a[1]);
        echo implode(",", $a), "|", implode(",", array_keys($a));"#;
    assert_eq!(run(src), "1,3|0,2");
}

#[test]
fn unset_variable() {
    let src = r#"<?php $x = 5; unset($x); echo isset($x) ? "set" : "unset";"#;
    assert_eq!(run(src), "unset");
}

#[test]
fn unset_assoc_key() {
    let src = r#"<?php $m = ["a" => 1, "b" => 2, "c" => 3]; unset($m["b"]);
        echo implode(",", array_keys($m)), "|count:", count($m);"#;
    assert_eq!(run(src), "a,c|count:2");
}

#[test]
fn unset_nested_element() {
    let src = r#"<?php $a = ["x" => ["y" => 1, "z" => 2]]; unset($a["x"]["y"]);
        echo implode(",", array_keys($a["x"]));"#;
    assert_eq!(run(src), "z");
}

#[test]
fn unset_multiple_targets() {
    let src = r#"<?php $a = 1; $b = 2; $arr = [1, 2, 3]; unset($a, $b, $arr[0]);
        echo isset($a) ? "1" : "0", isset($b) ? "1" : "0", "|", implode(",", $arr);"#;
    assert_eq!(run(src), "00|2,3");
}

#[test]
fn unset_missing_key_is_quiet() {
    let src = r#"<?php $a = [1, 2]; unset($a[99]); unset($undef); echo count($a);"#;
    assert_eq!(run(src), "2");
}

// ── object properties ────────────────────────────────────────────────────────
//
// `unset($o->p)` removes the property outright. Every expectation is verbatim
// output of the same program under the reference `php` 8.5.9.

#[test]
fn unset_removes_an_object_property() {
    let src = r#"<?php class C { public $x = 1; public $y = 2; }
        $o = new C; unset($o->x);
        echo implode(",", array_keys(get_object_vars($o)));"#;
    assert_eq!(run(src), "y");
}

#[test]
fn unsetting_a_property_that_is_not_there_is_not_an_error() {
    let src = r#"<?php class C { public $x = 1; }
        $o = new C; unset($o->zz);
        echo implode(",", array_keys(get_object_vars($o))), "|ok";"#;
    assert_eq!(run(src), "x|ok");
}

#[test]
fn unset_shares_the_object_rather_than_copying_it() {
    // Objects are handles, so the removal is visible through every alias.
    let src = r#"<?php class C { public $x = 1; }
        $a = new C; $b = $a; unset($a->x);
        var_dump(isset($b->x));"#;
    assert_eq!(run(src), "bool(false)\n");
}

#[test]
fn an_unset_property_reads_as_undefined_again() {
    // The slot is gone, not nulled: reading it warns exactly as a property that
    // was never declared does.
    let src = r#"<?php class C { public $x = 1; }
        $o = new C; unset($o->x); echo "|", $o->x, "|";"#;
    assert_eq!(
        run(src),
        "|\nWarning: Undefined property: C::$x in Command line code on line 2\n|"
    );
}

#[test]
fn unset_calls_magic_unset_for_a_property_the_object_does_not_carry() {
    let src = r#"<?php
        class C { private $bag = ["a" => 1];
            function __unset($n) { echo "[U$n]"; unset($this->bag[$n]); }
            function __isset($n) { echo "[I$n]"; return isset($this->bag[$n]); } }
        $o = new C;
        var_dump(isset($o->a)); unset($o->a); var_dump(isset($o->a));"#;
    assert_eq!(run(src), "[Ia]bool(true)\n[Ua][Ia]bool(false)\n");
}

#[test]
fn unsetting_an_unreachable_property_throws_unless_magic_unset_takes_it() {
    // No `__unset`: the visibility error the reference throws.
    let with_no_magic = r#"<?php class C { private $p = 1; } $o = new C;
        try { unset($o->p); } catch (Throwable $e) {
            echo get_class($e), "|", $e->getMessage(); }"#;
    assert_eq!(
        run(with_no_magic),
        "Error|Cannot access private property C::$p"
    );
    // With one, it is consulted first and there is no error at all.
    let with_magic = r#"<?php class C { private $p = 1; function __unset($n) { echo "[U$n]"; } }
        $o = new C; unset($o->p); echo "|ok";"#;
    assert_eq!(run(with_magic), "[Up]|ok");
}
