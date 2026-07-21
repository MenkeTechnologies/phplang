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
