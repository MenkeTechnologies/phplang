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

// ── dynamic properties (PHP 8.2) ─────────────────────────────────────────────

#[test]
fn creating_an_undeclared_property_is_deprecated() {
    assert_eq!(
        run(r#"<?php class C {} $c = new C(); $c->x = 1; echo $c->x;"#),
        format!(
            "{}1",
            deprecated("Creation of dynamic property C::$x is deprecated")
        )
    );
}

#[test]
fn the_notice_names_the_objects_class_not_the_one_that_declared_the_others() {
    // `$d` is inherited and silent; `$x` is nobody's, and the notice says `C`
    // (the instance's class) even though the declarations live on `P`.
    assert_eq!(
        run(
            r#"<?php class P { public $d; } class C extends P {} $c = new C(); $c->d = 1; $c->x = 2; echo "end";"#
        ),
        format!(
            "{}end",
            deprecated("Creation of dynamic property C::$x is deprecated")
        )
    );
}

#[test]
fn it_fires_per_creation_not_per_write() {
    // The second write finds the property already there; `unset` removes it, so
    // the write after that creates it again and is a second creation.
    assert_eq!(
        run(r#"<?php class C {} $c = new C(); $c->x = 1; $c->x = 2; echo "end";"#),
        format!(
            "{}end",
            deprecated("Creation of dynamic property C::$x is deprecated")
        )
    );
}

#[test]
fn a_declared_or_promoted_property_is_silent_and_so_is_stdclass() {
    // Four ways to be silent: declared with a default, declared bare, promoted
    // from a constructor parameter, and being `stdClass` (PHP's property bag,
    // which is also what an `(object)` cast produces).
    assert_eq!(
        run(
            r#"<?php class C { public $a = 0; public $b; } $c = new C(); $c->a = 1; $c->b = 2; echo $c->a, $c->b;"#
        ),
        "12"
    );
    assert_eq!(
        run(
            r#"<?php class C { function __construct(public $p = 0) {} } $c = new C(); $c->p = 7; echo $c->p;"#
        ),
        "7"
    );
    assert_eq!(
        run(r#"<?php $o = new stdClass(); $o->x = 1; echo $o->x;"#),
        "1"
    );
    assert_eq!(
        run(r#"<?php $o = (object) ["a" => 1]; $o->b = 2; echo $o->a, $o->b;"#),
        "12"
    );
}

#[test]
fn allow_dynamic_properties_opts_out_and_is_inherited() {
    assert_eq!(
        run(r#"<?php #[AllowDynamicProperties] class C {} $c = new C(); $c->x = 1; echo $c->x;"#),
        "1"
    );
    assert_eq!(
        run(
            r#"<?php #[AllowDynamicProperties] class P {} class C extends P {} $c = new C(); $c->x = 1; echo $c->x;"#
        ),
        "1"
    );
    // The name is matched exactly: a NAMESPACED attribute of the same last
    // segment is a different, inert attribute and does not opt the class out.
    assert_eq!(
        run(
            r#"<?php #[Ns\AllowDynamicProperties] class C {} $c = new C(); $c->x = 1; echo "end";"#
        ),
        format!(
            "{}end",
            deprecated("Creation of dynamic property C::$x is deprecated")
        )
    );
}

#[test]
fn a_fetch_for_writing_deprecates_before_the_read_warns() {
    // `.=` and `++` fetch the property for writing and then read it, so PHP
    // announces the creation first and the undefined read second. The order is
    // the whole point of the assertion.
    assert_eq!(
        run(r#"<?php class C {} $c = new C(); $c->n .= "x"; echo $c->n;"#),
        format!(
            "{}{}x",
            deprecated("Creation of dynamic property C::$n is deprecated"),
            warning("Undefined property: C::$n")
        )
    );
    assert_eq!(
        run(r#"<?php class C {} $c = new C(); $c->n++; echo $c->n;"#),
        format!(
            "{}{}1",
            deprecated("Creation of dynamic property C::$n is deprecated"),
            warning("Undefined property: C::$n")
        )
    );
    // Appending into an undeclared property vivifies it, which is a creation too
    // — and this path never reads, so there is no warning.
    assert_eq!(
        run(r#"<?php class C {} $c = new C(); $c->v[] = 1; echo count($c->v);"#),
        format!(
            "{}1",
            deprecated("Creation of dynamic property C::$v is deprecated")
        )
    );
}

// ── compile-time notices ─────────────────────────────────────────────────────

#[test]
fn the_dollar_brace_notice_precedes_all_program_output() {
    // Raised while the source is READ, so it lands before the `a` on the same
    // line — not between the two statements where it was written.
    assert_eq!(
        run(r#"<?php echo "a"; $v = 1; echo "${v}";"#),
        format!(
            "{}a1",
            deprecated("Using ${var} in strings is deprecated, use {$var} instead")
        )
    );
}

#[test]
fn a_compile_time_notice_fires_for_code_that_never_runs() {
    // The function is never called. The notice is a property of the source text.
    assert_eq!(
        run(r#"<?php function f() { $v = 1; return "${v}"; } echo "no-call";"#),
        format!(
            "{}no-call",
            deprecated("Using ${var} in strings is deprecated, use {$var} instead")
        )
    );
}

#[test]
fn error_reporting_cannot_retract_a_compile_time_notice() {
    // The mask is written at run time; the notice was decided before any of it
    // ran, so it is already out.
    assert_eq!(
        run(r#"<?php error_reporting(0); $v = 1; echo "${v}";"#),
        format!(
            "{}1",
            deprecated("Using ${var} in strings is deprecated, use {$var} instead")
        )
    );
}

// ── the error_reporting mask ─────────────────────────────────────────────────

#[test]
fn the_mask_gates_each_severity_independently() {
    // E_ALL is 30719 in PHP 8.4+ (E_STRICT was removed and left E_ALL).
    assert_eq!(run(r#"<?php echo error_reporting();"#), "30719");
    // Warnings off, deprecations still on.
    assert_eq!(
        run(
            r#"<?php error_reporting(E_ALL & ~E_WARNING); class C {} $c = new C(); $c->x = 1; echo $undef; echo "end";"#
        ),
        format!(
            "{}end",
            deprecated("Creation of dynamic property C::$x is deprecated")
        )
    );
    // Deprecations off, warnings still on.
    assert_eq!(
        run(
            r#"<?php error_reporting(E_ALL & ~E_DEPRECATED); class C {} $c = new C(); $c->x = 1; echo $undef; echo "end";"#
        ),
        format!("{}end", warning("Undefined variable $undef"))
    );
    // Everything off.
    assert_eq!(
        run(
            r#"<?php error_reporting(0); class C {} $c = new C(); $c->x = 1; echo $undef; echo "end";"#
        ),
        "end"
    );
}

#[test]
fn setting_the_mask_returns_the_previous_one_and_it_is_restorable() {
    assert_eq!(
        run(
            r#"<?php $old = error_reporting(0); echo $undef; error_reporting($old); echo $undef2;"#
        ),
        warning("Undefined variable $undef2")
    );
}

#[test]
fn ini_set_and_error_reporting_write_the_same_state() {
    // `ini_set` reports the previous value as a string and writes the mask.
    assert_eq!(
        run(
            r#"<?php var_dump(ini_set("error_reporting", "8")); echo $undef; var_dump(error_reporting(), ini_get("error_reporting"));"#
        ),
        "string(5) \"30719\"\nint(8)\nstring(1) \"8\"\n"
    );
    // `error_reporting()` writes back through the ini view too.
    assert_eq!(
        run(r#"<?php error_reporting(8); var_dump(ini_get("error_reporting"));"#),
        "string(1) \"8\"\n"
    );
    // `ini_set` does NOT run the php.ini constant-expression scanner: the string
    // reads as an ordinary integer, so a symbolic level mutes everything (0) and
    // `ini_get` still reports the raw text that was written.
    assert_eq!(
        run(
            r#"<?php ini_set("error_reporting", "E_ALL & ~E_WARNING"); var_dump(ini_get("error_reporting"), error_reporting());"#
        ),
        "string(18) \"E_ALL & ~E_WARNING\"\nint(0)\n"
    );
}

// ── `@` is a run-time region, not a compile-time read mode ───────────────────

#[test]
fn suppression_reaches_a_diagnostic_raised_inside_a_library_function() {
    // The warning is raised from Rust, with no opcode of its own to quieten, so
    // only a run-time suppression region can reach it.
    let noisy = r#"<?php range("ab", "c"); echo "|done";"#;
    assert_eq!(
        run(noisy),
        format!(
            "{}|done",
            diag(
                "Warning",
                "range(): Argument #1 ($start) must be a single byte, \
                 subsequent bytes are ignored"
            )
        )
    );
    assert_eq!(run(r#"<?php @range("ab", "c"); echo "|done";"#), "|done");
    assert_eq!(
        run(r#"<?php @preg_match("/[a/", "x"); echo "|done";"#),
        "|done"
    );
}

#[test]
fn suppression_does_not_swallow_an_error() {
    // `@` drops DIAGNOSTICS. An `Error` is not one, so it still propagates —
    // which is the behaviour that separates `@$o->p` from `isset($o->p)`.
    let src = r#"<?php class C { private $p = 1; } $o = new C;
        try { echo @$o->p; } catch (Throwable $e) { echo get_class($e); }"#;
    assert_eq!(run(src), "Error");
}

#[test]
fn suppression_is_restored_when_an_exception_unwinds_out_of_it() {
    // The region the `@` opened is never closed by its own opcode here, so
    // without a restore on unwind everything after the catch would stay silent.
    let src = r#"<?php
        function boom() { throw new Exception("x"); }
        try { @boom(); } catch (Throwable $e) {}
        echo $undef; echo "|done";"#;
    assert_eq!(
        run(src),
        "\nWarning: Undefined variable $undef in Command line code on line 4\n|done".to_string()
    );
}

#[test]
fn suppression_covers_only_its_own_operand() {
    let src = r#"<?php echo @$a, $b; echo "|done";"#;
    assert_eq!(
        run(src),
        format!("{}|done", diag("Warning", "Undefined variable $b"))
    );
}

// ── ini_get / ini_set over the engine's own defaults ─────────────────────────

#[test]
fn ini_get_reports_the_engine_defaults() {
    // Each value is what the reference reports with NO php.ini loaded (`php -n`),
    // which is what makes it an engine default rather than a machine's config.
    for (name, value) in [
        ("memory_limit", r#"string(4) "128M""#),
        ("date.timezone", r#"string(3) "UTC""#),
        ("precision", r#"string(2) "14""#),
        ("serialize_precision", r#"string(2) "-1""#),
        ("max_execution_time", r#"string(1) "0""#),
        ("post_max_size", r#"string(2) "8M""#),
        ("pcre.backtrack_limit", r#"string(7) "1000000""#),
        ("zend.assertions", r#"string(1) "1""#),
        ("unserialize_max_depth", r#"string(4) "4096""#),
        ("default_charset", r#"string(5) "UTF-8""#),
    ] {
        assert_eq!(
            run(&format!(r#"<?php var_dump(ini_get("{name}"));"#)),
            format!("{value}\n"),
            "{name}"
        );
    }
}

#[test]
fn ini_get_refuses_a_name_whose_value_would_be_machine_specific() {
    // An optional extension's setting, and one whose default is the build's
    // install prefix: both would be wrong on another machine, so neither is known.
    for name in ["mysqli.default_host", "extension_dir", "nosuchsetting"] {
        assert_eq!(
            run(&format!(r#"<?php var_dump(ini_get("{name}"));"#)),
            "bool(false)\n",
            "{name}"
        );
    }
}

#[test]
fn ini_set_returns_the_previous_value_and_the_write_is_readable() {
    let src = r#"<?php var_dump(ini_set("memory_limit", "256M"), ini_get("memory_limit"));"#;
    assert_eq!(run(src), "string(4) \"128M\"\nstring(4) \"256M\"\n");
}

#[test]
fn ini_set_refuses_a_setting_that_is_not_runtime_changeable() {
    // PHP's PHP_INI_PERDIR / PHP_INI_SYSTEM set: `ini_get` reads them, `ini_set`
    // reports false and changes nothing.
    for name in ["post_max_size", "output_buffering", "expose_php"] {
        let src = format!(
            r#"<?php $before = ini_get("{name}");
               var_dump(ini_set("{name}", "99"), ini_get("{name}") === $before);"#
        );
        assert_eq!(run(&src), "bool(false)\nbool(true)\n", "{name}");
    }
}
