//! Tests for the `stdlib::strings` category. Every expected value mirrors the
//! output of the reference PHP 8 function of the same name. These run headless
//! (no `php` on PATH required).

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn substr_count_and_replace() {
    assert_eq!(run(r#"<?php echo substr_count("hello world", "o");"#), "2");
    assert_eq!(run(r#"<?php echo substr_count("ababab", "ab");"#), "3");
    // Non-overlapping, like PHP.
    assert_eq!(run(r#"<?php echo substr_count("aaa", "aa");"#), "1");
    assert_eq!(
        run(r#"<?php echo substr_count("hello world", "o", 5);"#),
        "1"
    );
    assert_eq!(
        run(r#"<?php echo substr_replace("Hello", "World", 0);"#),
        "World"
    );
    assert_eq!(
        run(r#"<?php echo substr_replace("Hello World", "PHP", 6);"#),
        "Hello PHP"
    );
    assert_eq!(
        run(r#"<?php echo substr_replace("abcdef", "X", 1, 2);"#),
        "aXdef"
    );
    // Negative offset counts from the end.
    assert_eq!(
        run(r#"<?php echo substr_replace("abcdef", "-", -1, 1);"#),
        "abcde-"
    );
}

#[test]
fn strtr_both_forms() {
    assert_eq!(run(r#"<?php echo strtr("Hi All", "ia", "eu");"#), "He All");
    // Truncates to the shorter of from/to.
    assert_eq!(run(r#"<?php echo strtr("abc", "abc", "xy");"#), "xyc");
    assert_eq!(
        run(r#"<?php echo strtr("Hello World", ["Hello" => "Hi", "World" => "Earth"]);"#),
        "Hi Earth"
    );
    // Longest key wins at a given position.
    assert_eq!(
        run(r#"<?php echo strtr("XY", ["X" => "1", "XY" => "2"]);"#),
        "2"
    );
}

#[test]
fn strstr_family() {
    assert_eq!(
        run(r#"<?php echo strstr("user@example.com", "@");"#),
        "@example.com"
    );
    assert_eq!(
        run(r#"<?php echo strstr("user@example.com", "@", true);"#),
        "user"
    );
    assert_eq!(
        run(r#"<?php var_dump(strstr("abc", "x"));"#),
        "bool(false)\n"
    );
    assert_eq!(run(r#"<?php echo stristr("HELLO", "ell");"#), "ELLO");
    assert_eq!(run(r#"<?php echo strrchr("a/b/c", "/");"#), "/c");
    assert_eq!(
        run(r#"<?php echo strpbrk("This is a test", "st");"#),
        "s is a test"
    );
}

#[test]
fn span_functions() {
    assert_eq!(
        run(r#"<?php echo strspn("42 is the answer", "1234567890");"#),
        "2"
    );
    assert_eq!(run(r#"<?php echo strcspn("Hello World", "Wd");"#), "6");
    assert_eq!(run(r#"<?php echo strspn("aaabbb", "a");"#), "3");
}

#[test]
fn case_insensitive_positions() {
    assert_eq!(run(r#"<?php echo stripos("Hello World", "o", 5);"#), "7");
    assert_eq!(
        run(r#"<?php var_dump(stripos("abc", "X"));"#),
        "bool(false)\n"
    );
    assert_eq!(
        run(r#"<?php echo strrpos("hello world hello", "hello");"#),
        "12"
    );
    assert_eq!(run(r#"<?php echo strripos("Hello WORLD", "o");"#), "7");
    assert_eq!(run(r#"<?php echo strncasecmp("Hello", "HELP", 3);"#), "0");
    assert!(run(r#"<?php echo strncasecmp("Hello", "HELP", 4);"#).starts_with('-'));
}

#[test]
fn ireplace_and_escaping() {
    assert_eq!(
        run(r#"<?php echo str_ireplace("WORLD", "PHP", "Hello world");"#),
        "Hello PHP"
    );
    assert_eq!(
        run(r#"<?php echo str_ireplace(["a", "B"], ["1", "2"], "AbAb");"#),
        "1212"
    );
    assert_eq!(run(r#"<?php echo quotemeta("1+1=2");"#), r"1\+1=2");
    assert_eq!(run(r#"<?php echo addslashes("O'Reilly");"#), r"O\'Reilly");
    assert_eq!(run(r#"<?php echo stripslashes("O\\'Reilly");"#), "O'Reilly");
    assert_eq!(run(r#"<?php echo str_rot13("Hello");"#), "Uryyb");
    // rot13 is its own inverse.
    assert_eq!(
        run(r#"<?php echo str_rot13(str_rot13("Sphinx"));"#),
        "Sphinx"
    );
}

#[test]
fn nl2br_chunk_tags() {
    assert_eq!(run("<?php echo nl2br(\"a\nb\");"), "a<br />\nb");
    assert_eq!(
        run(r#"<?php echo chunk_split("abcdefgh", 3, "-");"#),
        "abc-def-gh-"
    );
    assert_eq!(
        run(r#"<?php echo htmlspecialchars_decode("&lt;a&gt; &amp; &quot;b&quot;");"#),
        r#"<a> & "b""#
    );
    assert_eq!(
        run(r#"<?php echo strip_tags("<b>hi</b> <i>there</i>");"#),
        "hi there"
    );
}

#[test]
fn distance_metrics() {
    assert_eq!(run(r#"<?php echo similar_text("World", "word");"#), "3");
    assert_eq!(run(r#"<?php echo similar_text("Hello", "Hello");"#), "5");
    assert_eq!(run(r#"<?php echo levenshtein("kitten", "sitting");"#), "3");
    assert_eq!(run(r#"<?php echo levenshtein("", "abc");"#), "3");
    assert_eq!(run(r#"<?php echo levenshtein("abc", "abc");"#), "0");
}

#[test]
fn sprintf_family_and_scan() {
    assert_eq!(
        run(r#"<?php echo vsprintf("%s has %d apples", ["Bob", 3]);"#),
        "Bob has 3 apples"
    );
    assert_eq!(run(r#"<?php vprintf("%d-%d", [4, 5]);"#), "4-5");
    assert_eq!(
        run(r#"<?php $r = sscanf("age:42", "age:%d"); echo $r[0];"#),
        "42"
    );
    assert_eq!(
        run(r#"<?php $r = sscanf("x 3.5", "%s %f"); echo $r[0], "|", $r[1];"#),
        "x|3.5"
    );
}

// ── regression tests for reviewed PHP-8-semantics bugs ───────────────────────

#[test]
fn multibyte_byte_slices_do_not_panic() {
    // Bug 1: these all sliced a &str at a raw byte offset that landed mid-UTF-8
    // char ("é" is two bytes) and panicked "byte index N is not a char boundary".
    // PHP strings are byte-oriented, so byte-length results are expected.
    // "héllo" bytes: h é(2) l l o. Splicing at byte offset 2 lands inside "é":
    // PHP emits the raw bytes 68 c3 78; phplang stores results in a Rust `String`,
    // so the dangling 0xC3 lead byte surfaces as U+FFFD via from_utf8_lossy. The
    // load-bearing assertion is "no panic" — old code crashed here.
    assert_eq!(
        run(r#"<?php echo substr_replace("héllo", "x", 2);"#),
        "h\u{FFFD}x"
    );
    assert_eq!(run(r#"<?php echo substr_count("héllo héllo", "l");"#), "4");
    // stripos with an offset that falls mid-char must not panic.
    assert_eq!(run(r#"<?php echo stripos("héllo", "L", 2);"#), "3");
    // strspn/strcspn are byte-oriented and count the leading 2-byte "é" as bytes.
    assert_eq!(run(r#"<?php echo strcspn("héllo", "l");"#), "3");
}

#[test]
fn empty_needle_php8_semantics() {
    // Bug 2: empty needle must return the whole haystack (PHP 8), not false.
    assert_eq!(run(r#"<?php echo strstr("hello", "");"#), "hello");
    assert_eq!(run(r#"<?php echo stristr("hello", "");"#), "hello");
    // before_needle => the (empty) portion before position 0.
    assert_eq!(
        run(r#"<?php var_dump(strstr("hello", "", true));"#),
        "string(0) \"\"\n"
    );
    // Bug 3: empty needle must return strlen(haystack), not false.
    assert_eq!(run(r#"<?php echo strrpos("hello", "");"#), "5");
    assert_eq!(run(r#"<?php echo strripos("hello", "");"#), "5");
}

#[test]
fn strrpos_negative_offset_window() {
    // Bug 3: the multi-char-needle negative-offset window must cap the start at
    // len+off (not len+off+needle_len-1). strrpos("ababab","ab",-3) == 2.
    assert_eq!(run(r#"<?php echo strrpos("ababab", "ab", -3);"#), "2");
    assert_eq!(run(r#"<?php echo strrpos("ababab", "ab", -1);"#), "4");
}

#[test]
fn strncasecmp_negative_length_is_valueerror() {
    // Bug 4: a negative $length raises a PHP-8 ValueError, not a coerced compare.
    // Pinned by class AND message: `is_err()` alone would also pass if the call
    // failed for some unrelated reason, or if the message named another argument.
    assert_eq!(
        run(
            r#"<?php try { strncasecmp("a", "b", -1); } catch (Throwable $e) { echo get_class($e), ': ', $e->getMessage(); }"#
        ),
        "ValueError: strncasecmp(): Argument #3 ($length) must be greater than or equal to 0"
    );
    // Zero length still compares equal (no bytes compared).
    assert_eq!(run(r#"<?php echo strncasecmp("abc", "xyz", 0);"#), "0");
}

#[test]
fn multibyte() {
    assert_eq!(run(r#"<?php echo mb_strlen("héllo");"#), "5");
    // Byte strlen counts the 2-byte é as two.
    assert_eq!(run(r#"<?php echo mb_strtoupper("héllo");"#), "HÉLLO");
    assert_eq!(run(r#"<?php echo mb_strtolower("HÉLLO");"#), "héllo");
    assert_eq!(run(r#"<?php echo mb_substr("héllo", 1, 3);"#), "éll");
    assert_eq!(run(r#"<?php echo mb_substr("héllo", -2);"#), "lo");
}

// ── byte-wise case, charlists, and limits ───────────────────────────────────
//
// PHP's case family is ASCII-only, `ucwords`' separator argument *replaces* the
// default set, `trim` takes a charlist with `a..z` ranges, and `explode` honours
// a limit. All four were previously ignored or Unicode-aware by mistake.

#[test]
fn case_functions_are_ascii_only() {
    // The multibyte "é" must survive untouched — mb_* is the Unicode-aware form.
    assert_eq!(
        run(r#"<?php echo strtoupper("héllo"), "|", strtolower("HÉLLO");"#),
        "HéLLO|hÉllo"
    );
    assert_eq!(
        run(r#"<?php echo ucfirst("élan"), "|", lcfirst("ÉLAN");"#),
        "élan|ÉLAN"
    );
    assert_eq!(
        run(r#"<?php echo ucfirst("abc"), "|", lcfirst("ABC");"#),
        "Abc|aBC"
    );
}

#[test]
fn ucwords_separators_replace_the_default_set() {
    // With "-" given, the space is no longer a separator.
    assert_eq!(
        run(r#"<?php echo ucwords("hello world-foo bar", "-");"#),
        "Hello world-Foo bar"
    );
    assert_eq!(
        run(r#"<?php echo ucwords("hello world_foo bar");"#),
        "Hello World_foo Bar"
    );
    // Every byte following a separator is uppercased, runs included.
    assert_eq!(run(r#"<?php echo ucwords("--a-b", "-");"#), "--A-B");
}

#[test]
fn trim_accepts_a_charlist_with_ranges() {
    assert_eq!(
        run(r#"<?php echo rtrim("xayb", "ab"), "|", ltrim("00123", "0");"#),
        "xay|123"
    );
    // "a..z" is the range a-z, not the three characters a, ., z.
    assert_eq!(run(r#"<?php echo trim("a..z", "a..z");"#), "..");
    // The default set includes NUL and \x0B but not the form feed \x0C.
    assert_eq!(run("<?php echo trim(\"\\x00\\x0Bx \\t\\n\\r\");"), "x");
}

#[test]
fn explode_honours_its_limit() {
    assert_eq!(
        run(r#"<?php echo implode("|", explode(",", "a,b,c", 2));"#),
        "a|b,c"
    );
    // A negative limit drops that many trailing parts.
    assert_eq!(
        run(r#"<?php echo implode("|", explode(",", "a,b,c", -1));"#),
        "a|b"
    );
    // Zero behaves as one.
    assert_eq!(run(r#"<?php echo count(explode(",", "a,b,c", 0));"#), "1");
}

#[test]
fn str_replace_handles_arrays_and_reports_a_count() {
    assert_eq!(
        run(r#"<?php echo str_replace(["a","b"], ["1","2"], "aabbc", $n), ":", $n;"#),
        "1122c:4"
    );
    // An array search with a scalar replacement maps every needle to it.
    assert_eq!(
        run(r#"<?php echo str_replace(["a","b"], "X", "aabbc");"#),
        "XXXXc"
    );
    // Searches apply in sequence to the running result, so "aa" finds nothing.
    assert_eq!(
        run(r#"<?php echo str_replace(["a","aa"], ["1","2"], "aaa");"#),
        "111"
    );
    // An array subject is processed element-wise.
    assert_eq!(
        run(r#"<?php echo implode("|", str_replace("a", "X", ["aa", "ba"]));"#),
        "XX|bX"
    );
}

#[test]
fn double_quoted_escapes_cover_hex_octal_and_unicode() {
    assert_eq!(run(r#"<?php echo bin2hex("\x41\102\u{e9}");"#), "4142c3a9");
    // \v and \f are real escapes; an unknown one keeps its backslash.
    assert_eq!(run(r#"<?php echo bin2hex("\v\f\q");"#), "0b0c5c71");
}

// ── argument guards that used to be silent fallbacks ─────────────────────────

/// Three functions accepted an argument the reference rejects and quietly did
/// something plausible instead: `str_pad` substituted the default space for an
/// empty pad, `str_split` clamped a non-positive length to 1, and `explode`
/// treated an empty separator as "one piece". Each produced a believable value,
/// so nothing downstream could notice.
///
/// Re-verified against php 8.5.9.
#[test]
fn empty_or_nonpositive_arguments_are_value_errors() {
    let cases = [
        (
            r#"str_pad("hello", 8, "")"#,
            "str_pad(): Argument #3 ($pad_string) must not be empty",
        ),
        (
            r#"str_split("abc", 0)"#,
            "str_split(): Argument #2 ($length) must be greater than 0",
        ),
        (
            r#"str_split("abc", -1)"#,
            "str_split(): Argument #2 ($length) must be greater than 0",
        ),
        (
            r#"explode("", "abc")"#,
            "explode(): Argument #1 ($separator) must not be empty",
        ),
    ];
    for (call, msg) in cases {
        assert_eq!(
            run(&format!(
                r#"<?php try {{ {call}; }} catch (Throwable $e) {{ echo get_class($e), ': ', $e->getMessage(); }}"#
            )),
            format!("ValueError: {msg}"),
            "{call}"
        );
    }
    // An OMITTED pad string still defaults to a space — only an explicitly empty
    // one is rejected, so the guard must not fire on the default path.
    assert_eq!(run(r#"<?php echo '[', str_pad("hi", 5), ']';"#), "[hi   ]");
    assert_eq!(run(r#"<?php echo str_pad("hi", 5, "-");"#), "hi---");
    assert_eq!(
        run(r#"<?php echo implode("|", str_split("abcde", 2));"#),
        "ab|cd|e"
    );
    assert_eq!(
        run(r#"<?php echo implode("|", explode(",", "a,b"));"#),
        "a|b"
    );
}

/// PHP 8.2 changed `str_split("")` from `[""]` to an empty array. The old answer
/// differs from the new one only in a case most tests never exercise.
#[test]
fn str_split_of_the_empty_string_is_an_empty_array() {
    assert_eq!(run(r#"<?php var_dump(str_split(""));"#), "array(0) {\n}\n");
    assert_eq!(run(r#"<?php var_dump(count(str_split("")));"#), "int(0)\n");
    // A non-empty subject is unaffected.
    assert_eq!(run(r#"<?php var_dump(count(str_split("a")));"#), "int(1)\n");
}

// ── strip_tags ───────────────────────────────────────────────────────────────

/// `$allowed_tags` in both spellings PHP accepts. An allowed tag is re-emitted
/// VERBATIM — attributes included — because the scanner buffers the whole
/// `<…>` span and replays it, rather than reconstructing a bare tag.
#[test]
fn strip_tags_allow_list() {
    assert_eq!(
        run(r#"<?php echo strip_tags("<b>x</b><i>y</i>", "<b>");"#),
        "<b>x</b>y"
    );
    assert_eq!(
        run(r#"<?php echo strip_tags("<b>x</b><i>y</i>", ["b","i"]);"#),
        "<b>x</b><i>y</i>"
    );
    assert_eq!(
        run(r#"<?php echo strip_tags('<p>T <b class="x">bold</b></p>', '<b>');"#),
        "T <b class=\"x\">bold</b>"
    );
    // The allow-list is matched against a NORMALIZED tag: `</b>` and `<b …>`
    // both reduce to `<b>`, and `<br/>` to `<br>`.
    assert_eq!(
        run(r#"<?php echo strip_tags("<br/>a<br />b", "<br>");"#),
        "<br/>a<br />b"
    );
    assert_eq!(
        run(r#"<?php echo strip_tags("<B CLASS=y>x</B>", "<b>");"#),
        "<B CLASS=y>x</B>"
    );
    // Matching is by SUBSTRING of the allow string, so `<b>` and `<body>` are
    // not interchangeable in either direction.
    assert_eq!(run(r#"<?php echo strip_tags("<b>x</b>", "<body>");"#), "x");
    assert_eq!(
        run(r#"<?php echo strip_tags("<body>x</body>", "<b>");"#),
        "x"
    );
    // No allow-list at all: every tag goes.
    assert_eq!(run(r#"<?php echo strip_tags("<b>x</b>", "");"#), "x");
    assert_eq!(run(r#"<?php echo strip_tags("a<b>c", null);"#), "ac");
}

/// The states beyond "inside a tag": a `>` inside a quoted attribute does NOT
/// close the tag, a comment runs to `-->`, `<?…?>` is swallowed whole, and a
/// `<` followed by whitespace is literal text rather than a tag opener.
#[test]
fn strip_tags_scanner_states() {
    // The `>` inside the quoted attribute does not CLOSE the tag, but it is
    // also not buffered — state 1 breaks on it while `in_q` is set — so the
    // replayed tag comes back without it.
    assert_eq!(
        run(r#"<?php echo strip_tags('<a href="x>y">link</a>', "<a>");"#),
        "<a href=\"xy\">link</a>"
    );
    assert_eq!(
        run(r#"<?php echo strip_tags("<!-- c --> visible");"#),
        " visible"
    );
    assert_eq!(
        run(r#"<?php echo strip_tags("<!-- <b>x</b> --> y", "<b>");"#),
        " y"
    );
    assert_eq!(
        run(r#"<?php echo strip_tags("<?php echo 1; ?>after");"#),
        "after"
    );
    assert_eq!(
        run(r#"<?php echo strip_tags('<?xml version="1.0"?><a>x</a>');"#),
        "x"
    );
    assert_eq!(
        run(r#"<?php echo strip_tags("<!DOCTYPE html><p>x</p>");"#),
        "x"
    );
    // `< ` with a space is text; `5 < 6 > 7` keeps both operators.
    assert_eq!(
        run(r#"<?php echo strip_tags("a < b and c > d");"#),
        "a < b and c > d"
    );
    assert_eq!(run(r#"<?php echo strip_tags("5 < 6 > 7");"#), "5 < 6 > 7");
    // A tag that never closes swallows the rest of the input.
    assert_eq!(run(r#"<?php echo strip_tags("unclosed <b");"#), "unclosed ");
    assert_eq!(run(r#"<?php echo strip_tags("<<b>>x");"#), "x");
}

// ── HTML entities ────────────────────────────────────────────────────────────

/// `htmlentities` is `htmlspecialchars` PLUS the named-entity table; the two
/// used to share one arm and so produced identical output for every input.
#[test]
fn htmlentities_maps_named_entities() {
    assert_eq!(
        run(r#"<?php echo htmlentities("<é>");"#),
        "&lt;&eacute;&gt;"
    );
    assert_eq!(run(r#"<?php echo htmlspecialchars("<é>");"#), "&lt;é&gt;");
    // The table is the full HTML 4.01 set, not only the Latin-1 supplement.
    assert_eq!(
        run("<?php echo htmlentities(\"\u{20AC}\u{03B1}\u{2665}\u{00A0}\");"),
        "&euro;&alpha;&hearts;&nbsp;"
    );
    // The math block is in the table too: U+2260 is `&ne;`.
    assert_eq!(run("<?php echo htmlentities(\"\u{2260}\");"), "&ne;");
}

/// `$flags` selects which quotes are escaped. Ignoring it (the previous
/// behavior) silently escaped BOTH quotes under every flag, including
/// `ENT_NOQUOTES`.
#[test]
fn html_encode_honors_quote_flags() {
    let s = r#"<?php echo htmlspecialchars("a'b\"c", %s);"#;
    assert_eq!(run(&s.replace("%s", "ENT_NOQUOTES")), "a'b\"c");
    assert_eq!(run(&s.replace("%s", "ENT_COMPAT")), "a'b&quot;c");
    assert_eq!(run(&s.replace("%s", "ENT_QUOTES")), "a&#039;b&quot;c");
    // Omitted flags default to ENT_QUOTES since PHP 8.1.
    assert_eq!(
        run(r#"<?php echo htmlspecialchars("a'b\"c");"#),
        "a&#039;b&quot;c"
    );
}

/// The two decoders resolve DIFFERENT sets. `htmlspecialchars_decode` knows
/// only what `htmlspecialchars` writes, so a named or numeric reference it
/// could never have produced is left standing.
#[test]
fn html_decoders_differ_in_reach() {
    let input = r#"&lt;&eacute;&#233;&amp;&#039;&#39;&apos;&quot;"#;
    assert_eq!(
        run(&format!(r#"<?php echo html_entity_decode("{input}");"#)),
        "<éé&''&apos;\""
    );
    assert_eq!(
        run(&format!(
            r#"<?php echo htmlspecialchars_decode("{input}");"#
        )),
        "<&eacute;&#233;&''&apos;\""
    );
    // Hex references decode; an unknown entity and an unterminated `&amp` do not.
    assert_eq!(
        run(r#"<?php echo html_entity_decode("&#xE9;|&unknown;|&amp");"#),
        "é|&unknown;|&amp"
    );
    // Decoding is single-pass: `&amp;lt;` yields the literal text `&lt;`.
    assert_eq!(run(r#"<?php echo html_entity_decode("&amp;lt;");"#), "&lt;");
    // `$flags` gates a NUMERIC quote reference just as it gates the named one,
    // and in BOTH decoders — under ENT_COMPAT a `&#039;` survives untouched
    // while the `&#34;` beside it decodes.
    let quotes = r#"&#039;&#34;&#x27;&#x22;&apos;&quot;"#;
    for f in ["html_entity_decode", "htmlspecialchars_decode"] {
        let src = |flag: &str| format!(r#"<?php echo {f}("{quotes}", {flag});"#);
        assert_eq!(run(&src("ENT_NOQUOTES")), quotes);
        assert_eq!(run(&src("ENT_COMPAT")), "&#039;\"&#x27;\"&apos;\"");
        assert_eq!(run(&src("ENT_QUOTES")), "'\"'\"&apos;\"");
    }
    // Round trip through the named table.
    assert_eq!(
        run(r#"<?php echo html_entity_decode(htmlentities("<b>é & 'q'</b>"));"#),
        "<b>é & 'q'</b>"
    );
}
