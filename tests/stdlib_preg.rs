//! `preg` (PCRE) standard-library tests: PHP source in, captured `echo` output
//! out. Every expectation was cross-checked against reference PHP 8.5. Two
//! engines back these functions — the `regex` crate, and `fancy-regex` for the
//! look-around / backreference / atomic-group patterns the first one will not
//! compile; the tests at the end drive the second engine specifically.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn preg_match_returns_bool_int() {
    assert_eq!(run(r#"<?php echo preg_match('/\d+/', 'ab123');"#), "1");
    assert_eq!(run(r#"<?php echo preg_match('/\d+/', 'abc');"#), "0");
    // `i` flag, `#` delimiter, `~` delimiter, `{}` bracket delimiter.
    assert_eq!(run(r#"<?php echo preg_match('/ABC/i', 'xabcx');"#), "1");
    assert_eq!(run(r#"<?php echo preg_match('#/usr/#', 'a/usr/b');"#), "1");
    assert_eq!(run(r#"<?php echo preg_match('~a.b~', 'axb');"#), "1");
    assert_eq!(run(r#"<?php echo preg_match('{^\w+$}', 'word');"#), "1");
}

#[test]
fn preg_match_binds_matches_out_array() {
    // The out array must be pre-initialised (`$m = []`) — see the module docs on
    // the by-value dispatch path. Groups: [0] whole match, 1.. captures.
    let src = r#"<?php
        $m = [];
        preg_match('/(\d+)-(\d+)/', 'x 12-34 y', $m);
        echo $m[0], "|", $m[1], "|", $m[2];"#;
    assert_eq!(run(src), "12-34|12|34");
}

#[test]
fn preg_match_truncates_trailing_unmatched_groups() {
    // PHP drops trailing groups that did not participate; a middle unmatched
    // group stays as "".
    let src = r#"<?php
        $m = [];
        preg_match('/(a)(b)?/', 'a', $m);
        echo count($m), ":", $m[0], $m[1];"#;
    assert_eq!(run(src), "2:aa");
    let mid = r#"<?php
        $m = [];
        preg_match('/(a)(x)?(c)/', 'ac', $m);
        echo count($m), ":", $m[1], "|", $m[2], "|", $m[3];"#;
    assert_eq!(run(mid), "4:a||c");
}

#[test]
fn preg_match_all_pattern_and_set_order() {
    // PREG_PATTERN_ORDER (default): matches[group][occurrence].
    let pat = r#"<?php
        $m = [];
        $c = preg_match_all('/(\w)(\d)/', 'a1 b2 c3', $m);
        echo $c, ":", implode(",", $m[0]), ":", implode(",", $m[1]), ":", implode(",", $m[2]);"#;
    assert_eq!(run(pat), "3:a1,b2,c3:a,b,c:1,2,3");
    // PREG_SET_ORDER (2): matches[occurrence][group].
    let set = r#"<?php
        $m = [];
        preg_match_all('/(\w)(\d)/', 'a1 b2', $m, 2);
        echo implode(",", $m[0]), ";", implode(",", $m[1]);"#;
    assert_eq!(run(set), "a1,a,1;b2,b,2");
    assert_eq!(run(r#"<?php echo preg_match_all('/x/', 'abc');"#), "0");
}

#[test]
fn preg_replace_scalar_and_backrefs() {
    assert_eq!(
        run(r#"<?php echo preg_replace('/\d/', '#', 'a1b2c3');"#),
        "a#b#c#"
    );
    // Swap two captured words via `$1`/`$2`.
    assert_eq!(
        run(r#"<?php echo preg_replace('/(\w+)\s(\w+)/', '$2 $1', 'hello world');"#),
        "world hello"
    );
    // `\1` backref form and `${1}` braced form.
    assert_eq!(
        run(r#"<?php echo preg_replace('/(\d)/', '[\1]', 'a5');"#),
        "a[5]"
    );
    assert_eq!(
        run(r#"<?php echo preg_replace('/(\d)/', '${1}${1}', 'a5');"#),
        "a55"
    );
    // Literal `$` in the replacement (not a backref).
    assert_eq!(run(r#"<?php echo preg_replace('/x/', '$k', 'x');"#), "$k");
    // Limit argument caps the number of replacements.
    assert_eq!(
        run(r#"<?php echo preg_replace('/\d/', '#', 'a1b2c3', 2);"#),
        "a#b#c3"
    );
}

#[test]
fn preg_replace_array_patterns() {
    // Array of patterns with a parallel array of replacements.
    assert_eq!(
        run(r#"<?php echo preg_replace(['/a/', '/b/'], ['X', 'Y'], 'abc');"#),
        "XYc"
    );
    // Array of patterns, single scalar replacement applied to all.
    assert_eq!(
        run(r#"<?php echo preg_replace(['/a/', '/b/'], '_', 'abc');"#),
        "__c"
    );
    // Array subject → array result, keyed as input.
    let src = r#"<?php
        $r = preg_replace('/\d/', '#', ['a1', 'b2']);
        echo $r[0], ",", $r[1];"#;
    assert_eq!(run(src), "a#,b#");
}

#[test]
fn preg_replace_callback_receives_match_array() {
    let src = r#"<?php
        echo preg_replace_callback('/\d+/', function ($m) {
            return $m[0] * 2;
        }, 'x5 y10');"#;
    assert_eq!(run(src), "x10 y20");
    // Callback sees capture groups too.
    let grp = r#"<?php
        echo preg_replace_callback('/(\w)(\d)/', function ($m) {
            return $m[2] . $m[1];
        }, 'a1b2');"#;
    assert_eq!(run(grp), "1a2b");
}

#[test]
fn preg_split_basic_limit_and_flags() {
    assert_eq!(
        run(r#"<?php echo implode("|", preg_split('/,/', 'a,b,c'));"#),
        "a|b|c"
    );
    // Empty pattern splits between every character, with leading/trailing "".
    assert_eq!(
        run(r#"<?php echo implode("|", preg_split('//', 'ab'));"#),
        "|a|b|"
    );
    // PREG_SPLIT_NO_EMPTY (1) drops the empties.
    assert_eq!(
        run(r#"<?php echo implode("|", preg_split('//', 'ab', -1, 1));"#),
        "a|b"
    );
    // limit=2 keeps the remainder in the final piece.
    assert_eq!(
        run(r#"<?php echo implode("|", preg_split('/,/', 'a,b,c,d', 2));"#),
        "a|b,c,d"
    );
    // PREG_SPLIT_DELIM_CAPTURE (2) interleaves captured delimiters.
    assert_eq!(
        run(r#"<?php echo implode("|", preg_split('/(,)/', 'a,b', -1, 2));"#),
        "a|,|b"
    );
}

#[test]
fn preg_quote_escapes_metacharacters() {
    assert_eq!(run(r#"<?php echo preg_quote('a.b*c');"#), r"a\.b\*c");
    assert_eq!(run(r#"<?php echo preg_quote('1+1=2');"#), r"1\+1\=2");
    // The optional delimiter argument is escaped too.
    assert_eq!(run(r#"<?php echo preg_quote('a/b', '/');"#), r"a\/b");
}

#[test]
fn preg_grep_filters_preserving_keys() {
    // Only all-digit entries survive; original keys are preserved.
    let src = r#"<?php
        $r = preg_grep('/^\d+$/', ['12', 'ab', '34']);
        echo implode(",", array_keys($r)), ":", implode(",", array_values($r));"#;
    assert_eq!(run(src), "0,2:12,34");
    // PREG_GREP_INVERT (1) keeps the non-matching entries.
    let inv = r#"<?php
        $r = preg_grep('/^\d+$/', ['12', 'ab', '34'], 1);
        echo implode(",", array_values($r));"#;
    assert_eq!(run(inv), "ab");
}

#[test]
fn multiline_and_dotall_flags() {
    // `m`: `^`/`$` match at line boundaries.
    assert_eq!(
        run("<?php echo preg_match_all('/^\\w+/m', \"a\\nb\\nc\");"),
        "3"
    );
    // `s`: `.` matches newline.
    assert_eq!(run("<?php echo preg_match('/a.b/s', \"a\\nb\");"), "1");
    // Without `s`, `.` does not cross a newline.
    assert_eq!(run("<?php echo preg_match('/a.b/', \"a\\nb\");"), "0");
}

#[test]
fn byte_matching_is_default_unicode_needs_u_flag() {
    // Bug 1: PCRE without `/u` matches BYTES. `é` is 2 bytes (0xC3 0xA9), so a
    // single-char pattern must NOT match, but a two-char one must.
    assert_eq!(run(r#"<?php echo preg_match('/^.$/', 'é');"#), "0");
    assert_eq!(run(r#"<?php echo preg_match('/^.{2}$/', 'é');"#), "1");
    // The `/u` flag switches to Unicode: `é` is a single codepoint, so the
    // reverse holds.
    assert_eq!(run(r#"<?php echo preg_match('/^.$/u', 'é');"#), "1");
    assert_eq!(run(r#"<?php echo preg_match('/^.{2}$/u', 'é');"#), "0");
}

#[test]
fn split_delim_capture_emits_empty_for_leading_nonparticipating_group() {
    // Bug 2: a non-participating capture group that precedes a participating one
    // in the same match must emit "" (trailing non-participating groups drop).
    // Flag 2 == PREG_SPLIT_DELIM_CAPTURE (literal, matching the other tests here).
    let src = r#"<?php
        echo implode("|", preg_split('/(a)|(b)/', 'xaybz', -1, 2));"#;
    // xaybz → [x, a, y, "", b, z]
    assert_eq!(run(src), "x|a|y||b|z");
}

#[test]
fn split_limit_counts_pieces_not_captured_delimiters() {
    // Bug 3: with DELIM_CAPTURE the limit counts only real split pieces, so the
    // interleaved delimiters do not consume the budget.
    let src = r#"<?php
        $r = preg_split('/(,)/', 'a,b,c,d', 3, 2);
        echo count($r), ":", implode("|", $r);"#;
    // 3 real pieces (a, b, c,d) with two captured commas interleaved → 5 total.
    assert_eq!(run(src), "5:a|,|b|,|c,d");
}

#[test]
fn trailing_whitespace_in_flags_region_is_tolerated() {
    // Bug 4: PHP allows whitespace after the closing delimiter / in the flags.
    assert_eq!(run(r#"<?php echo preg_match('/a/ ', 'a');"#), "1");
    assert_eq!(run("<?php echo preg_match(\"/a/\\n\", 'a');"), "1");
}

#[test]
fn no_match_resets_out_array_to_empty() {
    // Bug 5: a subsequent no-match must reset $matches to [], not leave stale
    // captures from the prior successful match.
    let src = r#"<?php
        $m = [];
        preg_match('/(\d+)/', 'x99y', $m);
        echo count($m), ";";
        preg_match('/(\d+)/', 'nodigits', $m);
        echo count($m);"#;
    assert_eq!(run(src), "2;0");
}

#[test]
fn lookaround_and_backreferences_match_rather_than_returning_the_sentinel() {
    // These are the two constructs the `regex` crate has no support for, so each
    // one lands on the second engine. Both used to come back as PHP's `false`
    // (the error sentinel); the reference matches them, so this asserts the
    // reference's answer instead of the old miss.
    assert_eq!(
        run(r#"<?php echo preg_match('/(?<=x)y/', 'xy') === false ? 'err' : 'ok';"#),
        "ok"
    );
    assert_eq!(
        run(r#"<?php echo preg_match('/(a)\1/', 'aa') === false ? 'err' : 'ok';"#),
        "ok"
    );
}

// ── pattern faults: the WARNING shape, not the throw shape ────────────────────
//
// A pattern the reference rejects is a `Warning` naming the calling function,
// the function's own error sentinel, and `preg_last_error()` left at
// `PREG_INTERNAL_ERROR`. Every expectation below is verbatim stdout of the same
// program under the reference `php` 8.5.9.

/// The diagnostic body of the warning `src` raises, with the position clause
/// stripped so the assertion is about the message and not the line.
fn warning(src: &str) -> String {
    run(src)
        .trim()
        .strip_prefix("Warning: ")
        .unwrap_or("<no warning>")
        .split(" in Command line code")
        .next()
        .unwrap_or_default()
        .to_string()
}

#[test]
fn a_bad_delimiter_or_modifier_warns_and_returns_false() {
    for (pattern, message) in [
        ("''", "Empty regular expression"),
        ("'   '", "Empty regular expression"),
        (
            "'abc'",
            "Delimiter must not be alphanumeric, backslash, or NUL byte",
        ),
        (
            "'1a1'",
            "Delimiter must not be alphanumeric, backslash, or NUL byte",
        ),
        ("'/a/Z'", "Unknown modifier 'Z'"),
        // Only the FIRST unknown modifier is reported.
        ("'/a/gg'", "Unknown modifier 'g'"),
        ("'/abc'", "No ending delimiter '/' found"),
        // A bracket delimiter names itself differently, and counts nesting, so
        // this one is unterminated despite ending in `}`.
        ("'{a{b}'", "No ending matching delimiter '}' found"),
    ] {
        let src = format!("<?php preg_match({pattern}, 'x');");
        assert_eq!(
            warning(&src),
            format!("preg_match(): {message}"),
            "{pattern}"
        );
        assert_eq!(
            run(&format!("<?php var_dump(@preg_match({pattern}, 'x'));")),
            "bool(false)\n",
            "{pattern}"
        );
    }
}

#[test]
fn a_malformed_body_reports_the_pcre_fault_and_its_offset() {
    for (pattern, message) in [
        (
            "'/[a/'",
            "missing terminating ] for character class at offset 2",
        ),
        (
            "'/ab[cd/'",
            "missing terminating ] for character class at offset 5",
        ),
        ("'/(a/'", "missing closing parenthesis at offset 2"),
        // `(?` opens a group, so the `?` is not read as a quantifier.
        ("'/(?/'", "missing closing parenthesis at offset 2"),
        ("'/a)/'", "unmatched closing parenthesis at offset 2"),
        (
            "'/{2}/'",
            "quantifier does not follow a repeatable item at offset 3",
        ),
        (
            "'/x{1,2}{3}/'",
            "quantifier does not follow a repeatable item at offset 9",
        ),
        (
            "'/a{2,1}/'",
            "numbers out of order in {} quantifier at offset 5",
        ),
    ] {
        let src = format!("<?php preg_match({pattern}, 'x');");
        assert_eq!(
            warning(&src),
            format!("preg_match(): Compilation failed: {message}"),
            "{pattern}"
        );
    }
}

#[test]
fn a_pattern_the_reference_accepts_still_compiles() {
    // The delimiter and quantifier rules that a careless scan gets wrong. None of
    // these may warn, and each answer is the reference's.
    for (pattern, expected) in [
        // Backslash escapes the delimiter, so the body is `a\/` and closes later.
        (r#"'/a\//'"#, "0"),
        // Bracket delimiters, nested and not.
        ("'{a}'", "1"),
        ("'(a)'", "1"),
        ("'<a>'", "1"),
        // `{` that does not open a quantifier is a literal.
        ("'/a{x}/'", "0"),
        // PCRE2 takes an open lower bound.
        ("'/a{,3}/'", "1"),
        ("'/a{2,}/'", "0"),
        // `)` and `]` inside a character class are literal.
        ("'/[a)]/'", "1"),
        ("'/[]a]/'", "1"),
        // A lazy or possessive suffix is part of the quantifier, not a second one.
        ("'/a*?/'", "1"),
        ("'/a*+/'", "1"),
        // Every modifier letter the reference accepts.
        ("'/a/imsxuADJSUX'", "1"),
    ] {
        assert_eq!(
            run(&format!("<?php echo preg_match({pattern}, 'ab');")),
            expected,
            "{pattern}"
        );
    }
}

#[test]
fn every_compiling_function_reports_the_fault_under_its_own_name() {
    for (call, sentinel) in [
        ("preg_match('/[a/', 'x')", "bool(false)"),
        ("preg_match_all('/[a/', 'x', $m)", "bool(false)"),
        ("preg_replace('/[a/', 'z', 'x')", "NULL"),
        ("preg_replace_callback('/[a/', fn($m) => 'z', 'x')", "NULL"),
        ("preg_split('/[a/', 'x')", "bool(false)"),
        ("preg_grep('/[a/', ['x'])", "bool(false)"),
    ] {
        let name = call.split('(').next().unwrap();
        assert_eq!(
            warning(&format!("<?php {call};")),
            format!("{name}(): Compilation failed: missing terminating ] for character class at offset 2"),
            "{call}"
        );
        assert_eq!(
            run(&format!("<?php var_dump(@{call});")),
            format!("{sentinel}\n"),
            "{call}"
        );
    }
}

#[test]
fn preg_last_error_persists_until_the_next_pattern_compiles() {
    // The state is what makes this observable across CALLS: reading it does not
    // clear it, `preg_quote` does not touch it, and a pattern that compiles
    // clears it even when the match then finds nothing.
    let src = r#"<?php
        @preg_match('/[a/', 'x');           echo preg_last_error(), preg_last_error_msg(), "|";
        echo preg_last_error(), "|";       // reading does not clear
        preg_quote('a');                   echo preg_last_error(), "|";
        preg_match('/zzz/', 'abc');        echo preg_last_error(), "|";  // no match, but compiled
        @preg_grep('/(', ['a']);           echo preg_last_error(), "|";
        preg_split('/,/', 'a,b');          echo preg_last_error(), preg_last_error_msg();"#;
    assert_eq!(run(src), "1Internal error|1|1|0|1|0No error");
}

#[test]
fn a_pattern_only_the_second_engine_takes_still_clears_the_error_state() {
    // The reference compiles this one, so there is no warning to copy and no
    // error to record. It now also MATCHES, which is what the reference does:
    // `php -r '$r = preg_match("/(a)\\1/", "aa"); echo var_export($r, true), "|",
    // preg_last_error();'` prints `1|0`.
    let src = r#"<?php $r = preg_match('/(a)\1/', 'aa');
        echo var_export($r, true), "|", preg_last_error();"#;
    assert_eq!(run(src), "1|0");
}

#[test]
fn suppression_and_the_error_reporting_mask_both_hide_the_warning() {
    // Neither touches the error STATE, only the display.
    assert_eq!(
        run(r#"<?php var_dump(@preg_match('/[a/', 'x')); echo preg_last_error();"#),
        "bool(false)\n1"
    );
    assert_eq!(
        run(r#"<?php error_reporting(E_ALL & ~E_WARNING);
               var_dump(preg_match('/[a/', 'x')); echo preg_last_error();"#),
        "bool(false)\n1"
    );
}

// ── the `A` (anchored) modifier ──────────────────────────────────────────────

#[test]
fn anchored_modifier_matches_only_at_the_search_offset() {
    // Anchored at offset 0, so a match further in does not count.
    assert_eq!(
        run(r#"<?php var_dump(preg_match("/a/A", "bar"));"#),
        "int(0)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(preg_match("/a/A", "abr"));"#),
        "int(1)\n"
    );
}

#[test]
fn anchored_match_all_retries_at_each_successive_offset() {
    // Two leading `a`s match at offsets 0 and 1; the `b` at 2 ends the walk.
    assert_eq!(
        run(r#"<?php var_dump(preg_match_all("/a/A", "aab"));"#),
        "int(2)\n"
    );
    // A subject that does not match at offset 0 stops immediately.
    assert_eq!(
        run(r#"<?php var_dump(preg_match_all("/a/A", "bab"));"#),
        "int(0)\n"
    );
    // A pattern that can match empty keeps going past the last `a`: offsets
    // 0 ("aa"), 2 ("") and 3 ("").
    assert_eq!(
        run(r#"<?php var_dump(preg_match_all("/a*/A", "aab"));"#),
        "int(3)\n"
    );
}

#[test]
fn anchored_replace_split_and_grep() {
    assert_eq!(
        run(r#"<?php var_dump(preg_replace("/a/A", "X", "aab"));"#),
        "string(3) \"XXb\"\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(preg_replace_callback("/a/A", fn($m) => "X", "aab"));"#),
        "string(3) \"XXb\"\n"
    );
    // Nothing matches at offset 0, so the subject comes back unsplit.
    assert_eq!(
        run(r#"<?php print_r(preg_split("/,/A", "a,b,c"));"#),
        "Array\n(\n    [0] => a,b,c\n)\n"
    );
    assert_eq!(
        run(r#"<?php print_r(preg_grep("/a/A", ["ab", "ba"]));"#),
        "Array\n(\n    [0] => ab\n)\n"
    );
}

#[test]
fn unanchored_patterns_are_unaffected_by_the_anchoring_path() {
    assert_eq!(
        run(r#"<?php var_dump(preg_match("/a/", "bar"));"#),
        "int(1)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(preg_match_all("/a/", "bab"));"#),
        "int(1)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(preg_replace("/a/", "X", "aab"));"#),
        "string(3) \"XXb\"\n"
    );
}

// ── the second engine: look-around, backreferences, atomic groups ─────────────
//
// Every expectation below was read back off `php` 8.5.9 before it was written
// here. These constructs are the ones the `regex` crate cannot compile, so each
// case also proves the fallback engine is reached at all.

#[test]
fn look_ahead_and_look_behind_match_in_both_polarities() {
    assert_eq!(
        run(r#"<?php var_dump(preg_match('/foo(?=bar)/', 'foobar', $m), $m);"#),
        "int(1)\narray(1) {\n  [0]=>\n  string(3) \"foo\"\n}\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(preg_match('/foo(?!bar)/', 'foobar'));"#),
        "int(0)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(preg_match('/(?<=\$)\d+/', 'cost $21', $m), $m);"#),
        "int(1)\narray(1) {\n  [0]=>\n  string(2) \"21\"\n}\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(preg_match('/(?<!a)b/', 'ab'));"#),
        "int(0)\n"
    );
    // Two look-aheads stacked at one position — the shape every password rule
    // in the wild is written in.
    assert_eq!(
        run(r#"<?php var_dump(preg_match('/^(?=.*\d)(?=.*[a-z]).{6,}$/', 'abc123'));"#),
        "int(1)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(preg_match('/^(?=.*\d)(?=.*[a-z]).{6,}$/', 'abcdef'));"#),
        "int(0)\n"
    );
}

#[test]
fn backreferences_match_and_substitute() {
    assert_eq!(
        run(r#"<?php var_dump(preg_match('/(\w)\1/', 'abbc', $m), $m);"#),
        "int(1)\narray(2) {\n  [0]=>\n  string(2) \"bb\"\n  [1]=>\n  string(1) \"b\"\n}\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(preg_replace('/(\w+) \1/', '$1', 'the the cat cat'));"#),
        "string(7) \"the cat\"\n"
    );
    // `(?P=name)` — the named form of the same thing.
    assert_eq!(
        run(r#"<?php var_dump(preg_match('/(?<w>\w+)-(?P=w)/', 'hi-hi'));"#),
        "int(1)\n"
    );
}

#[test]
fn atomic_groups_and_possessive_quantifiers_refuse_to_give_back() {
    // `(?>a+)` takes all three `a`s and will not release one for the `a` that
    // follows, so this fails where a plain `a+ab` would match.
    assert_eq!(
        run(r#"<?php var_dump(preg_match('/(?>a+)ab/', 'aaab'));"#),
        "int(0)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(preg_match('/a+ab/', 'aaab'));"#),
        "int(1)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(preg_match('/a++b/', 'aaab'));"#),
        "int(1)\n"
    );
}

#[test]
fn the_modifiers_still_apply_on_the_second_engine() {
    assert_eq!(
        run(r#"<?php var_dump(preg_match('/A(?=b)/i', 'ab'));"#),
        "int(1)\n"
    );
    assert_eq!(
        run("<?php var_dump(preg_match_all('/^(?=\\w)/m', \"ab\\ncd\"));"),
        "int(2)\n"
    );
    assert_eq!(
        run("<?php var_dump(preg_match('/a.(?=c)/s', \"a\\nc\"));"),
        "int(1)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(preg_match('/a (?= b) c/x', 'abc'));"#),
        "int(0)\n"
    );
    // `U` has no builder switch on the second engine and is carried inline; the
    // ungreedy `.+` must still stop at the first `>`.
    assert_eq!(
        run(r#"<?php var_dump(preg_match('/<(.+)>(?=<)/U', '<a><b>', $m), $m);"#),
        "int(1)\narray(2) {\n  [0]=>\n  string(3) \"<a>\"\n  [1]=>\n  string(1) \"a\"\n}\n"
    );
    // `A` retries at each successive offset and stops at the first miss.
    assert_eq!(
        run(r#"<?php var_dump(preg_match_all('/a(?=a)/A', 'aaab'));"#),
        "int(2)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(preg_match_all('/a(?=a)/A', 'baaa'));"#),
        "int(0)\n"
    );
}

#[test]
fn a_second_engine_pattern_leaves_the_error_state_clear() {
    // The reference COMPILES these, so nothing may be recorded for them — the
    // old behaviour returned the sentinel here without touching the state, and
    // the state half must not regress now that the match half works.
    assert_eq!(
        run(
            r#"<?php preg_match('/(?<=a)b/', 'ab'); echo preg_last_error(), '|', preg_last_error_msg();"#
        ),
        "0|No error"
    );
}

// ── named groups in $matches ─────────────────────────────────────────────────

#[test]
fn a_named_group_is_published_under_its_name_and_its_index() {
    // PHP emits the NAME key immediately before the numeric one, in group order.
    assert_eq!(
        run(r#"<?php preg_match('/(?<y>\d+)-(?<m>\d+)/', '2024-05', $m); echo json_encode($m);"#),
        r#"{"0":"2024-05","y":"2024","1":"2024","m":"05","2":"05"}"#
    );
    // An unnamed group in among the named ones keeps its index only.
    assert_eq!(
        run(r#"<?php preg_match('/(?P<x>a)(b)/', 'ab', $m); echo json_encode($m);"#),
        r#"{"0":"ab","x":"a","1":"a","2":"b"}"#
    );
    // A pattern with no names is unchanged by the naming path.
    assert_eq!(
        run(r#"<?php preg_match('/(\d)(\d)/', '12', $m); echo json_encode($m);"#),
        r#"["12","1","2"]"#
    );
}

#[test]
fn a_trailing_unmatched_named_group_loses_its_name_key_too() {
    // The trailing-group truncation runs before the keying, so `b` disappears
    // entirely rather than surviving as a bare name.
    assert_eq!(
        run(r#"<?php preg_match('/(?<a>x)(?<b>y)?/', 'x', $m); echo json_encode($m);"#),
        r#"{"0":"x","a":"x","1":"x"}"#
    );
    // A non-participating group FOLLOWED by one that participates stays, as "".
    assert_eq!(
        run(r#"<?php preg_match('/(?<a>x)?(?<b>y)/', 'y', $m); echo json_encode($m);"#),
        r#"{"0":"y","a":"","1":"","b":"y","2":"y"}"#
    );
}

#[test]
fn preg_match_all_names_the_outer_keys_in_pattern_order_and_the_inner_ones_in_set_order() {
    assert_eq!(
        run(r#"<?php preg_match_all('/(?<c>[ab])/', 'ab', $m); echo json_encode($m);"#),
        r#"{"0":["a","b"],"c":["a","b"],"1":["a","b"]}"#
    );
    assert_eq!(
        run(
            r#"<?php preg_match_all('/(?<c>[ab])/', 'ab', $m, PREG_SET_ORDER); echo json_encode($m);"#
        ),
        r#"[{"0":"a","c":"a","1":"a"},{"0":"b","c":"b","1":"b"}]"#
    );
}

#[test]
fn the_name_slot_and_the_index_slot_hold_independent_values() {
    // PHP stores two copies, not one shared array: writing through the name must
    // not be visible through the index.
    assert_eq!(
        run(
            r#"<?php preg_match_all('/(?<g>\d)/', '12', $m); $m['g'][0] = 'Z'; echo json_encode($m);"#
        ),
        r#"{"0":["1","2"],"g":["Z","2"],"1":["1","2"]}"#
    );
}

#[test]
fn preg_replace_callback_hands_the_named_keys_to_the_callback() {
    assert_eq!(
        run(
            r#"<?php echo preg_replace_callback('/(?<n>\d)(x)?/', function($m){ return json_encode($m); }, '7');"#
        ),
        r#"{"0":"7","n":"7","1":"7"}"#
    );
}

#[test]
fn preg_split_delim_capture_stays_a_plain_list() {
    // The delimiter groups are appended positionally; PHP does NOT key them by
    // name here, unlike every `$matches` above.
    assert_eq!(
        run(
            r#"<?php echo json_encode(preg_split('/(?<d>,)/', 'a,b', -1, PREG_SPLIT_DELIM_CAPTURE));"#
        ),
        r#"["a",",","b"]"#
    );
}

#[test]
fn preg_match_all_set_order_truncates_each_set_on_its_own() {
    // The rows are RAGGED: each set drops its own trailing non-participating
    // groups, so two sets of the same pattern can have different widths.
    assert_eq!(
        run(
            r#"<?php preg_match_all('/(a)(b)?/', 'a ab', $m, PREG_SET_ORDER); echo json_encode($m);"#
        ),
        r#"[["a","a"],["ab","a","b"]]"#
    );
    assert_eq!(
        run(
            r#"<?php preg_match_all('/(a)(b)?/', 'ab a', $m, PREG_SET_ORDER); echo json_encode($m);"#
        ),
        r#"[["ab","a","b"],["a","a"]]"#
    );
    // A gap in the MIDDLE is not a truncation point.
    assert_eq!(
        run(
            r#"<?php preg_match_all('/(a)(x)?(b)/', 'ab', $m, PREG_SET_ORDER); echo json_encode($m);"#
        ),
        r#"[["ab","a","","b"]]"#
    );
    // PREG_PATTERN_ORDER stays full-width — every column is present.
    assert_eq!(
        run(r#"<?php preg_match_all('/(a)(b)?/', 'a ab', $m); echo json_encode($m);"#),
        r#"[["a","ab"],["a","a"],["","b"]]"#
    );
}

#[test]
fn the_preg_flag_and_error_constants_are_defined() {
    // `PREG_GREP_INVERT` was missing, so `preg_grep($p, $a, PREG_GREP_INVERT)`
    // read the undefined name and inverted nothing.
    assert_eq!(
        run(r#"<?php echo json_encode(preg_grep('/a/', ['ab', 'cd'], PREG_GREP_INVERT));"#),
        r#"{"1":"cd"}"#
    );
    let src = r#"<?php echo PREG_GREP_INVERT, ',', PREG_NO_ERROR, ',', PREG_INTERNAL_ERROR, ',',
        PREG_BACKTRACK_LIMIT_ERROR, ',', PREG_RECURSION_LIMIT_ERROR, ',', PREG_BAD_UTF8_ERROR,
        ',', PREG_BAD_UTF8_OFFSET_ERROR, ',', PREG_JIT_STACKLIMIT_ERROR;"#;
    assert_eq!(run(src), "1,0,1,2,3,4,5,6");
}

#[test]
fn the_replacement_expander_handles_every_template_form() {
    // The substitution is hand-rolled so both engines share one implementation;
    // these pin it against the reference for each form a template can take.
    assert_eq!(
        run(r#"<?php var_dump(preg_replace('/(b)/', '[\1]', 'ab'));"#),
        "string(4) \"a[b]\"\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(preg_replace('/(b)/', '${1}${1}', 'ab'));"#),
        "string(3) \"abb\"\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(preg_replace('/(a)(b)/', '$2$1', 'ab'));"#),
        "string(2) \"ba\"\n"
    );
    // A reference to a group that does not exist expands to nothing.
    assert_eq!(
        run(r#"<?php var_dump(preg_replace('/(b)/', '$9', 'ab'));"#),
        "string(1) \"a\"\n"
    );
    // A `$` that is not a group reference is literal.
    assert_eq!(
        run(r#"<?php var_dump(preg_replace('/b/', 'p$q', 'ab'));"#),
        "string(4) \"ap$q\"\n"
    );
    // PHP has no `$$` escape in a replacement — both dollars are literal.
    assert_eq!(
        run(r#"<?php var_dump(preg_replace('/b/', '$$', 'ab'));"#),
        "string(3) \"a$$\"\n"
    );
    // `${1}0` must not be read as group 10.
    assert_eq!(
        run(r#"<?php var_dump(preg_replace('/(a)/', '${1}0', 'a'));"#),
        "string(2) \"a0\"\n"
    );
    // The same forms through the SECOND engine, which shares the expander.
    assert_eq!(
        run(r#"<?php var_dump(preg_replace('/(?<=a)(b)/', '[\1]', 'ab'));"#),
        "string(4) \"a[b]\"\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(preg_replace('/(?<=a)(b)/', '${1}${1}', 'ab'));"#),
        "string(3) \"abb\"\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(preg_replace('/(?<=a)b/', '$$', 'ab'));"#),
        "string(3) \"a$$\"\n"
    );
}

#[test]
fn zero_width_matches_after_a_non_empty_one_are_emitted() {
    // PCRE emits the empty match sitting immediately after a non-empty one; the
    // `regex` crate's own iterator suppresses it, so the match walk is driven by
    // hand. This is what `/a*/` and friends turn on.
    assert_eq!(
        run(r#"<?php echo json_encode(preg_split('/a*/', 'xaby'));"#),
        r#"["","x","","b","y",""]"#
    );
    assert_eq!(
        run(r#"<?php echo json_encode(preg_split('/\d*/', 'a1b'));"#),
        r#"["","a","","b",""]"#
    );
    assert_eq!(
        run(r#"<?php var_dump(preg_match_all('/a*/', 'abc'));"#),
        "int(4)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(preg_replace('/a*/', '-', 'abc'));"#),
        "string(6) \"--b-c-\"\n"
    );
    // The fully-empty pattern is unchanged by the hand-driven walk.
    assert_eq!(
        run(r#"<?php echo json_encode(preg_split('//', 'abc'));"#),
        r#"["","a","b","c",""]"#
    );
}

#[test]
fn an_empty_match_is_retried_for_a_non_empty_one_at_the_same_offset() {
    // PCRE records the empty match and then asks the SAME offset for a non-empty
    // one before stepping on. The lazy `/a*?/` is where that shows: without the
    // retry the `"a"` at offset 0 is never found at all.
    assert_eq!(
        run(r#"<?php var_dump(preg_match_all('/a*?/', 'abc'));"#),
        "int(5)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(preg_replace('/a*?/', 'z', 'abc'));"#),
        "string(7) \"zzzbzcz\"\n"
    );
    assert_eq!(
        run(r#"<?php echo json_encode(preg_split('/a*?/', 'bar'));"#),
        r#"["","b","","","r",""]"#
    );
    assert_eq!(
        run(r#"<?php var_dump(preg_replace('/b*?/', '-', 'abb'));"#),
        "string(7) \"-a-----\"\n"
    );
    // The retry runs under `A` too — a failed one still steps a character on.
    assert_eq!(
        run(r#"<?php var_dump(preg_match_all('/a*/A', 'abc'));"#),
        "int(4)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(preg_match_all('/a*/A', 'baa'));"#),
        "int(3)\n"
    );
    // An alternation whose first branch is zero-width: the non-empty branch must
    // still be reachable at the offset the empty one matched.
    assert_eq!(
        run(r#"<?php preg_match_all('/(?=b)|a/', 'ab', $m); echo json_encode($m);"#),
        r#"[["a",""]]"#
    );
}

#[test]
fn filter_validate_regexp_uses_the_same_compiler_as_preg() {
    // Look-around, which the private lookalike compiler could not take at all.
    assert_eq!(
        run(
            r#"<?php var_dump(filter_var('abc', FILTER_VALIDATE_REGEXP, ['options' => ['regexp' => '/(?<=a)b/']]));"#
        ),
        "string(3) \"abc\"\n"
    );
    // A forward delimiter scan: the body is `a\/b`, not `a\`.
    assert_eq!(
        run(
            r#"<?php var_dump(filter_var('a/b', FILTER_VALIDATE_REGEXP, ['options' => ['regexp' => '/a\/b/']]));"#
        ),
        "string(3) \"a/b\"\n"
    );
    // A pattern fault is diagnosed under `filter_var`'s own name and recorded in
    // the error state, rather than returning false in silence.
    let bad = r#"<?php var_dump(filter_var('a', FILTER_VALIDATE_REGEXP, ['options' => ['regexp' => '/[a/']]));"#;
    assert_eq!(
        warning(bad),
        "filter_var(): Compilation failed: missing terminating ] for character class at offset 2"
    );
    let state = r#"<?php @filter_var('a', FILTER_VALIDATE_REGEXP, ['options' => ['regexp' => '/[a/']]);
        echo preg_last_error();"#;
    assert_eq!(run(state), "1");
}

// ── PCRE's end-of-subject anchors ────────────────────────────────────────────
//
// `$` (unmodified) and `\Z` both match at the end of the subject AND just before
// a newline that ends it. Neither engine's `$` means that, and `fancy-regex`'s
// `\Z` means something else again, so both are rewritten to a look-ahead. Every
// expectation below was read off reference PHP 8.5.

#[test]
fn dollar_matches_before_a_newline_that_ends_the_subject() {
    assert_eq!(run(r#"<?php echo preg_match('/a$/', "a\n");"#), "1");
    assert_eq!(run(r#"<?php echo preg_match('/a$/', "a");"#), "1");
    // ONE final newline, not any run of them, and not a `\r` before it.
    assert_eq!(run(r#"<?php echo preg_match('/a$/', "a\n\n");"#), "0");
    assert_eq!(run(r#"<?php echo preg_match('/a$/', "a\r\n");"#), "0");
    // The anchor is zero-width: the newline is not part of the match.
    let m = r#"<?php $m = []; preg_match('/(a)$/', "xa\n", $m); echo json_encode($m);"#;
    assert_eq!(run(m), r#"["a","a"]"#);
    // Both positions are real, so the walk finds two of them.
    assert_eq!(run(r#"<?php echo preg_match_all('/$/', "a\n");"#), "2");
    assert_eq!(
        run(r#"<?php echo json_encode(preg_replace('/$/', 'X', "a\n"));"#),
        r#""aX\nX""#
    );
    assert_eq!(
        run(r#"<?php echo json_encode(preg_split('/a$/', "a\n"));"#),
        r#"["","\n"]"#
    );
    // The empty-match retry runs against the rewritten body too: `x*$` matches
    // empty at 1, then non-empty, then empty again at each `$` position.
    assert_eq!(
        run(r#"<?php echo json_encode(preg_replace('/x*$/', '-', "ax\n"));"#),
        r#""a--\n-""#
    );
}

#[test]
fn d_and_m_give_dollar_their_own_meaning_and_keep_it() {
    // `D` (PCRE2_DOLLAR_ENDONLY) drops the second position — which is what an
    // un-rewritten `$` already does, so this is the case that must NOT change.
    assert_eq!(run(r#"<?php echo preg_match('/a$/D', "a\n");"#), "0");
    // `/m` is end-of-LINE, which both engines already have.
    assert_eq!(run(r#"<?php echo preg_match('/a$/m', "a\nb");"#), "1");
    assert_eq!(run(r#"<?php echo preg_match_all('/a$/m', "a\na\n");"#), "2");
    // Inline, where the modifier letter is in the body rather than the flags.
    assert_eq!(run(r#"<?php echo preg_match('/(?m)a$/', "a\n");"#), "1");
    // `/m` overrides `/D` in PCRE, so `Dm` is the `/m` answer.
    assert_eq!(run(r#"<?php echo preg_match_all('/$/Dm', "a\n");"#), "2");
    // `\z` is the absolute end and is not touched by any of this.
    assert_eq!(run(r#"<?php echo preg_match('/a\z/', "a\n");"#), "0");
}

#[test]
fn backslash_z_is_the_end_or_just_before_a_final_newline() {
    // `fancy-regex` HAS a `\Z`, and it is not PCRE's: it also matches before a
    // newline that is not final, which makes these two the ones that catch a
    // rewrite done with `\Z` instead of the look-ahead.
    assert_eq!(run(r#"<?php echo preg_match('/a\Z/', "a\n\n");"#), "0");
    assert_eq!(run(r#"<?php echo preg_match_all('/\Z/', "a\n\n");"#), "2");
    assert_eq!(run(r#"<?php echo preg_match('/a\Z/', "a\n");"#), "1");
    assert_eq!(run(r#"<?php echo preg_match('/a\Z/', "a");"#), "1");
}

#[test]
fn the_rewrite_does_not_change_byte_semantics_or_a_literal_dollar() {
    // Without `/u` PCRE reads a BYTE, so `.` cannot span the two bytes of `é`
    // and this is 0. The rewritten body runs on the codepoint-oriented engine,
    // where `.` would span them, so a subject that is not ASCII is left to the
    // compiled engine unless `/u` put PCRE in UTF mode as well.
    assert_eq!(run("<?php echo preg_match('/^.$/', \"\u{e9}\\n\");"), "0");
    assert_eq!(run("<?php echo preg_match('/^.$/u', \"\u{e9}\\n\");"), "1");
    assert_eq!(
        run("<?php echo preg_match('/^.a$/u', \"\u{e9}a\\n\");"),
        "1"
    );
    // An escaped or classed `$` is a literal and is not an anchor.
    assert_eq!(run(r#"<?php echo preg_match('/[$]/', 'a$');"#), "1");
    assert_eq!(run(r#"<?php echo preg_match('/a\$/', 'a$');"#), "1");
    // `A` (anchored) still applies on the rewritten body.
    assert_eq!(run(r#"<?php echo preg_match('/a$/A', "a\n");"#), "1");
    assert_eq!(run(r#"<?php echo preg_match('/b$/A', "ab\n");"#), "0");
}
