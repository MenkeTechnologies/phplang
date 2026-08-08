//! PHP diagnostics: the `Warning` and `Deprecated` messages a read of something
//! that is not there produces, and the contexts that suppress them.
//!
//! These belong to *stdout*, not stderr. Under the CLI defaults PHP displays a
//! diagnostic on the standard output stream, interleaved with the program's own
//! output, so a program that triggers one has different output — not just a
//! different log. That makes every expectation here a byte-parity assertion, and
//! each is the verbatim stdout of the same program under the reference `php`
//! 8.5.9 invoked as `php -r`, whose script name is `Command line code`.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

/// The diagnostic line PHP emits for `msg`, on line 1 of `php -r` input.
fn diag(kind: &str, msg: &str) -> String {
    format!("\n{kind}: {msg} in Command line code on line 1\n")
}

fn warning(msg: &str) -> String {
    diag("Warning", msg)
}

fn deprecated(msg: &str) -> String {
    diag("Deprecated", msg)
}

// ── missing array keys ───────────────────────────────────────────────────────

#[test]
fn a_missing_string_key_is_quoted_and_a_missing_int_key_is_not() {
    assert_eq!(
        run(r#"<?php $a = ['k' => 1]; echo $a['nope']; echo $a[7]; echo "end";"#),
        format!(
            "{}{}end",
            warning(r#"Undefined array key "nope""#),
            warning("Undefined array key 7")
        )
    );
}

#[test]
fn a_read_through_a_missing_key_reports_the_null_it_then_indexes() {
    // Two diagnostics, not one: the missing key, then the null the rest of the
    // path is indexing. A read is fetched in read mode and vivifies nothing.
    assert_eq!(
        run(r#"<?php $a = [1]; echo $a['x']['y']; echo "end";"#),
        format!(
            "{}{}end",
            warning(r#"Undefined array key "x""#),
            warning("Trying to access array offset on null")
        )
    );
}

#[test]
fn an_offset_on_a_non_array_names_the_type_and_spells_bools_as_literals() {
    assert_eq!(
        run(r#"<?php $f = false; echo $f['k']; echo "end";"#),
        format!("{}end", warning("Trying to access array offset on false"))
    );
    assert_eq!(
        run(r#"<?php $i = 5; echo $i['k']; echo "end";"#),
        format!("{}end", warning("Trying to access array offset on int"))
    );
}

#[test]
fn a_string_offset_past_the_end_is_reported_but_a_valid_one_is_not() {
    assert_eq!(
        run(r#"<?php $s = "abc"; echo $s[1]; echo $s[10]; echo "end";"#),
        format!("b{}end", warning("Uninitialized string offset 10"))
    );
}

// ── missing variables and properties ─────────────────────────────────────────

#[test]
fn an_unset_variable_read_is_reported() {
    assert_eq!(
        run(r#"<?php echo $nope; echo "end";"#),
        format!("{}end", warning("Undefined variable $nope"))
    );
}

#[test]
fn a_property_read_on_a_non_object_names_the_type() {
    assert_eq!(
        run(r#"<?php $n = null; echo $n->prop; echo "end";"#),
        format!(
            "{}end",
            warning(r#"Attempt to read property "prop" on null"#)
        )
    );
}

#[test]
fn a_missing_property_names_the_class_that_lacks_it() {
    assert_eq!(
        run(r#"<?php class C { public $ok = 1; } $o = new C(); echo $o->missing; echo "end";"#),
        format!("{}end", warning("Undefined property: C::$missing"))
    );
}

#[test]
fn a_nullsafe_read_reports_a_missing_property_but_not_a_null_receiver() {
    // `?->` short-circuits on null — there is nothing to complain about — but on
    // a real object a missing property is still a missing property.
    assert_eq!(run(r#"<?php $n = null; echo $n?->p; echo "end";"#), "end");
    assert_eq!(
        run(r#"<?php class C {} $o = new C(); echo $o?->p; echo "end";"#),
        format!("{}end", warning("Undefined property: C::$p"))
    );
}

// ── the contexts that suppress a diagnostic ──────────────────────────────────

#[test]
fn isset_empty_coalesce_and_at_are_silent() {
    // Each of these asks whether the thing is there, so its absence is the
    // answer rather than a mistake.
    let src = r#"<?php $a = ['k' => 1];
        echo isset($a['nope']) ? "y" : "n";
        echo empty($a['nope']) ? "y" : "n";
        echo $a['nope'] ?? "d";
        echo @$a['nope'];
        echo @$undefined;
        echo isset($undefined) ? "y" : "n";
        echo isset($a['x']['y']) ? "y" : "n";
        echo "end";"#;
    assert_eq!(run(src), "nydnnend");
}

#[test]
fn writing_and_vivifying_are_silent() {
    let src = r#"<?php $a = [];
        $a['new'] = 1; $a['deep']['er'] = 2; $a['list'][] = 3;
        echo count($a), "end";"#;
    assert_eq!(run(src), "3end");
}

#[test]
fn a_by_reference_argument_is_an_output_and_is_not_reported() {
    // `preg_match($re, $s, $m)` with a fresh `$m` is the normal way to call it.
    let src = r#"<?php preg_match('/a(b)/', 'ab', $m); echo count($m), "end";"#;
    assert_eq!(run(src), "2end");
}

#[test]
fn a_key_expression_inside_isset_still_reports_its_own_diagnostics() {
    // Suppression covers the read being tested, not everything evaluated for it:
    // `$k` here is an ordinary read in an ordinary position.
    assert_eq!(
        run(r#"<?php $a = []; echo isset($a[$k]) ? "y" : "n"; echo "end";"#),
        format!("{}nend", warning("Undefined variable $k"))
    );
}

// ── read-modify-write fetches ────────────────────────────────────────────────

#[test]
fn a_compound_assignment_reports_each_missing_step_of_the_path() {
    // A read-modify-write path is fetched in write mode: the unset container is
    // reported and then vivified, so the next segment reports its own missing
    // key instead of an offset-on-null.
    assert_eq!(
        run(r#"<?php $a['p']['q'] += 5; echo $a['p']['q'];"#),
        format!(
            "{}{}{}5",
            warning("Undefined variable $a"),
            warning(r#"Undefined array key "p""#),
            warning(r#"Undefined array key "q""#)
        )
    );
}

// ── `++` / `--`, which are not `+ 1` / `- 1` ─────────────────────────────────

#[test]
fn increment_and_decrement_of_null_are_not_symmetric() {
    assert_eq!(run(r#"<?php $c = null; $c++; var_dump($c);"#), "int(1)\n");
    assert_eq!(
        run(r#"<?php $b = null; $b--; var_dump($b);"#),
        format!(
            "{}NULL\n",
            warning(
                "Decrement on type null has no effect, this will change in the next major \
                 version of PHP"
            )
        )
    );
}

#[test]
fn neither_operator_changes_a_bool() {
    assert_eq!(
        run(r#"<?php $t = true; $t++; var_dump($t);"#),
        format!(
            "{}bool(true)\n",
            warning(
                "Increment on type bool has no effect, this will change in the next major \
                 version of PHP"
            )
        )
    );
}

#[test]
fn incrementing_a_non_numeric_string_is_alphanumeric_succession() {
    // Perl-style: carry propagates right to left through [a-zA-Z0-9] and a carry
    // off the front prepends a character of the same class.
    let note =
        deprecated("Increment on non-numeric string is deprecated, use str_increment() instead");
    for (input, want) in [
        ("a", "b"),
        ("z", "aa"),
        ("Az", "Ba"),
        ("zz", "aaa"),
        ("a9", "b0"),
        ("9z", "10a"),
        ("Zz", "AAa"),
        // A non-alphanumeric character stops the succession outright.
        ("a-z", "a-a"),
        ("a_", "a_"),
    ] {
        let src = format!(r#"<?php $s = "{input}"; $s++; echo $s;"#);
        assert_eq!(run(&src), format!("{note}{want}"), "incrementing {input:?}");
    }
}

#[test]
fn a_numeric_string_increments_as_a_number() {
    assert_eq!(run(r#"<?php $u = "5"; $u++; var_dump($u);"#), "int(6)\n");
    assert_eq!(
        run(r#"<?php $u = "5.5"; $u++; var_dump($u);"#),
        "float(6.5)\n"
    );
}

#[test]
fn decrementing_a_non_numeric_string_does_nothing() {
    assert_eq!(
        run(r#"<?php $s = "abc"; $s--; echo $s;"#),
        format!(
            "{}abc",
            deprecated("Decrement on non-numeric string has no effect and is deprecated")
        )
    );
}

#[test]
fn the_empty_string_has_its_own_pair_of_rules() {
    assert_eq!(
        run(r#"<?php $s = ""; $s++; var_dump($s);"#),
        format!(
            "{}string(1) \"1\"\n",
            deprecated(
                "Increment on non-numeric string is deprecated, use str_increment() instead"
            )
        )
    );
    assert_eq!(
        run(r#"<?php $s = ""; $s--; var_dump($s);"#),
        format!(
            "{}int(-1)\n",
            deprecated("Decrement on empty string is deprecated as non-numeric")
        )
    );
}
