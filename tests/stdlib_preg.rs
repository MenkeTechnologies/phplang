//! `preg` (PCRE) standard-library tests: PHP source in, captured `echo` output
//! out. Every expectation was cross-checked against reference PHP 8. Backed by
//! the Rust `regex` crate — a PCRE subset (no backreferences / look-around); the
//! two tests at the end pin those documented limitations.

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
fn unsupported_pcre_features_return_error_sentinel() {
    // The Rust engine rejects backreferences and look-around; preg_match then
    // returns PHP's `false` (echoes as the empty string). This pins the
    // documented PCRE-subset limitation rather than asserting it "works".
    assert_eq!(
        run(r#"<?php echo preg_match('/(?<=x)y/', 'xy') === false ? 'err' : 'ok';"#),
        "err"
    );
    assert_eq!(
        run(r#"<?php echo preg_match('/(a)\1/', 'aa') === false ? 'err' : 'ok';"#),
        "err"
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
fn a_pattern_the_rust_engine_alone_rejects_stays_silent() {
    // Back-references and look-around compile in the reference, so there is no
    // diagnostic to copy: the sentinel is returned with NO warning and the error
    // state untouched. This is the documented engine-subset divergence.
    let src = r#"<?php $r = preg_match('/(a)\1/', 'aa');
        echo var_export($r, true), "|", preg_last_error();"#;
    assert_eq!(run(src), "false|0");
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
