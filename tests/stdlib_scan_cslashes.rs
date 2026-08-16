//! Round-2 parity tests for the string/array surface the fuzz corpus had never
//! generated: `sscanf` in both its shapes, the `php_charmask` consumers
//! (`addcslashes`/`stripcslashes`/`trim`/`str_word_count`), `count_chars`,
//! `strtok`, the array forms of `substr_replace`, `substr_compare`'s raw
//! comparison result, `array_replace_recursive`, `array_walk_recursive`'s
//! by-reference leaf, and the `array_sum`/`array_product` fold diagnostics.
//!
//! Every expected value below was measured against the reference (`php` 8.5.9)
//! and is the reference's verbatim output. These run headless.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

/// `%x`, `%o` and `%i` were silently unsupported: every one of these produced an
/// EMPTY array before, because the conversion fell through the specifier match
/// and aborted the scan.
#[test]
fn sscanf_integer_bases() {
    assert_eq!(
        run(r#"<?php print_r(sscanf("ff 10 0x1F", "%x %o %i"));"#),
        "Array\n(\n    [0] => 255\n    [1] => 8\n    [2] => 31\n)\n"
    );
    // `%i` auto-detects: a leading 0 is octal, `0x` is hex, otherwise decimal.
    assert_eq!(run(r#"<?php print_r(sscanf("017", "%i"));"#), "Array\n(\n    [0] => 15\n)\n");
    assert_eq!(run(r#"<?php print_r(sscanf("19", "%i"));"#), "Array\n(\n    [0] => 19\n)\n");
    // A `0x` with no hex digit after it gives the `x` back to the input.
    assert_eq!(run(r#"<?php print_r(sscanf("00x10", "%i"));"#), "Array\n(\n    [0] => 0\n)\n");
    assert_eq!(run(r#"<?php print_r(sscanf("-17", "%o"));"#), "Array\n(\n    [0] => -15\n)\n");
}

/// `%[…]` scan sets, including the two placement quirks `BuildCharSet` has: a
/// `]` in first position is a member, and a trailing `-` is a literal.
#[test]
fn sscanf_scan_sets() {
    assert_eq!(run(r#"<?php print_r(sscanf("abc123", "%[a-c]"));"#), "Array\n(\n    [0] => abc\n)\n");
    assert_eq!(run(r#"<?php print_r(sscanf("abc123", "%[^0-9]"));"#), "Array\n(\n    [0] => abc\n)\n");
    assert_eq!(run(r#"<?php print_r(sscanf("]ab", "%[]a]"));"#), "Array\n(\n    [0] => ]a\n)\n");
    assert_eq!(run(r#"<?php print_r(sscanf("a-b", "%[a-]"));"#), "Array\n(\n    [0] => a-\n)\n");
    // A scan set does NOT skip leading whitespace, so this matches nothing and
    // the scan stops with the pre-filled null still in place.
    assert_eq!(run(r#"<?php var_dump(sscanf(" abc", "%[a-z]"));"#), "array(1) {\n  [0]=>\n  NULL\n}\n");
    // Literal text around a scan set still has to match.
    assert_eq!(run(r#"<?php print_r(sscanf("[abc]", "[%[a-c]]"));"#), "Array\n(\n    [0] => abc\n)\n");
}

/// The two-argument form pre-fills one slot per non-suppressed specifier, so a
/// format that outruns its input answers nulls rather than a short array.
#[test]
fn sscanf_pads_unreached_slots_with_null() {
    assert_eq!(
        run(r#"<?php var_dump(sscanf("a b", "%s %s %s"));"#),
        "array(3) {\n  [0]=>\n  string(1) \"a\"\n  [1]=>\n  string(1) \"b\"\n  [2]=>\n  NULL\n}\n"
    );
    // Nothing converted AND the input ran out: the whole answer is null.
    assert_eq!(run(r#"<?php var_dump(sscanf("", "%d"));"#), "NULL\n");
    // A MISMATCH is not an underflow — the partial list survives.
    assert_eq!(run(r#"<?php var_dump(sscanf("abc", "%d"));"#), "array(1) {\n  [0]=>\n  NULL\n}\n");
}

/// The by-reference form. Before this round the extra arguments were compiled as
/// ordinary reads (warning `Undefined variable`) and never written back, and the
/// return was the array rather than the conversion count.
#[test]
fn sscanf_by_reference_form() {
    assert_eq!(
        run(r#"<?php $r = sscanf("42 foo", "%d %s", $a, $b); var_dump($r, $a, $b);"#),
        "int(2)\nint(42)\nstring(3) \"foo\"\n"
    );
    // A variable no conversion reached keeps its previous value — it is NOT
    // nulled, which is why the write-back has to be guarded.
    assert_eq!(
        run(r#"<?php $a = $b = 'K'; var_dump(sscanf("12", "%d %d", $a, $b), $a, $b);"#),
        "int(1)\nint(12)\nstring(1) \"K\"\n"
    );
    // Underflow with zero conversions is -1 here, where the array form is null.
    assert_eq!(
        run(r#"<?php $x = 'K'; var_dump(sscanf("", "%d", $x), $x);"#),
        "int(-1)\nstring(1) \"K\"\n"
    );
    // `%*d` assigns nothing, so it does not consume a by-reference slot.
    assert_eq!(
        run(r#"<?php var_dump(sscanf("hi 5", "%*s %d", $p), $p);"#),
        "int(2)\nint(5)\n"
    );
}

/// Arity is checked against the FORMAT before any input is read, and the two
/// directions raise different messages.
#[test]
fn sscanf_by_reference_arity_is_validated() {
    assert_eq!(
        run(
            r#"<?php try { sscanf("1", "%d", $a, $b); } catch (Throwable $e) { echo get_class($e), ': ', $e->getMessage(); }"#
        ),
        "ValueError: Variable is not assigned by any conversion specifiers"
    );
    assert_eq!(
        run(
            r#"<?php try { sscanf("1 2", "%d %d", $a); } catch (Throwable $e) { echo get_class($e), ': ', $e->getMessage(); }"#
        ),
        "ValueError: Different numbers of variable names and field specifiers"
    );
}

/// `%c` does not skip whitespace and defaults to a width of 1; `%n` reports the
/// byte offset without consuming anything.
#[test]
fn sscanf_char_and_offset_conversions() {
    // `%c` does not skip whitespace, but it does stop AT whitespace, so a
    // leading blank yields the empty string rather than the blank itself.
    assert_eq!(run(r#"<?php var_dump(sscanf(" x", "%c"));"#), "array(1) {\n  [0]=>\n  string(0) \"\"\n}\n");
    assert_eq!(run(r#"<?php print_r(sscanf("abc", "%c%c%c"));"#), "Array\n(\n    [0] => a\n    [1] => b\n    [2] => c\n)\n");
    assert_eq!(run(r#"<?php print_r(sscanf("hello", "%s%n"));"#), "Array\n(\n    [0] => hello\n    [1] => 5\n)\n");
    // The size modifiers are accepted and ignored.
    assert_eq!(run(r#"<?php print_r(sscanf("1 2 3.5", "%ld %hd %Lf"));"#), "Array\n(\n    [0] => 1\n    [1] => 2\n    [2] => 3.5\n)\n");
}

/// `addcslashes` did not exist. Outside printable ASCII the escape is the C
/// mnemonic, and a charlist range is inclusive.
#[test]
fn addcslashes_escapes_the_selected_bytes() {
    assert_eq!(run(r#"<?php echo addcslashes("foo[bar]", "A..z");"#), r"\f\o\o\[\b\a\r\]");
    // Only the bytes actually in the list are touched.
    assert_eq!(run(r#"<?php echo addcslashes("foo[bar]", "A..Z");"#), "foo[bar]");
    assert_eq!(run(r#"<?php echo addcslashes("\n\t\x07", "\n\t\x07");"#), r"\n\t\a");
    // Both short-circuits sit above the charmask, so neither warns.
    assert_eq!(run(r#"<?php echo addcslashes("", "z..A"), "|", addcslashes("hi", "");"#), "|hi");
}

/// A malformed `..` range is a warning shared by every `php_charmask` consumer,
/// and the range contributes nothing to the mask.
#[test]
fn charmask_range_diagnostics_are_shared() {
    assert_eq!(
        run(r#"<?php echo addcslashes("a..b", "..z");"#),
        "\nWarning: addcslashes(): Invalid '..'-range, no character to the left of '..' in Command line code on line 1\na\\.\\.b"
    );
    assert_eq!(
        run(r#"<?php echo addcslashes("a..b", "a..");"#),
        "\nWarning: addcslashes(): Invalid '..'-range, no character to the right of '..' in Command line code on line 1\n\\a\\.\\.b"
    );
    assert_eq!(
        run(r#"<?php echo addcslashes("x", "a..b..c");"#),
        "\nWarning: addcslashes(): Invalid '..'-range in Command line code on line 1\nx"
    );
    // `trim` reports the same fault under its OWN name.
    assert_eq!(
        run(r#"<?php echo trim("a..b", "z..a");"#),
        "\nWarning: trim(): Invalid '..'-range, '..'-range needs to be incrementing in Command line code on line 1\nb"
    );
    // …and `str_word_count` under its own — but only once there is something to
    // scan, because it answers an empty subject before building the mask.
    assert_eq!(
        run(r#"<?php echo str_word_count("a b", 0, "z..a");"#),
        "\nWarning: str_word_count(): Invalid '..'-range, '..'-range needs to be incrementing in Command line code on line 1\n2"
    );
    assert_eq!(run(r#"<?php echo str_word_count("", 0, "z..a");"#), "0");
}

/// `stripcslashes` did not exist. `\xHH` takes one or two hex digits, `\NNN` up
/// to three octal, and an unknown escape yields the escaped character itself.
#[test]
fn stripcslashes_reverses_the_c_escapes() {
    assert_eq!(run(r#"<?php echo stripcslashes('a\tb\x41');"#), "a\tbA");
    assert_eq!(run(r#"<?php echo stripcslashes('\101\x42\z');"#), "ABz");
    // A lone trailing backslash has nothing to escape and is kept.
    assert_eq!(run(r#"<?php var_dump(stripcslashes("a\\"));"#), "string(2) \"a\\\"\n");
}

/// `count_chars` did not exist. Modes 0-2 answer counter arrays, 3 and 4 answer
/// strings of the bytes that did / did not occur.
#[test]
fn count_chars_histogram_modes() {
    assert_eq!(
        run(r#"<?php print_r(count_chars("aab", 1));"#),
        "Array\n(\n    [97] => 2\n    [98] => 1\n)\n"
    );
    assert_eq!(run(r#"<?php echo count(count_chars("abc", 0));"#), "256");
    assert_eq!(run(r#"<?php echo count(count_chars("aab", 2));"#), "254");
    assert_eq!(run(r#"<?php echo count_chars("aab", 3);"#), "ab");
    // An empty subject has no non-zero counters at all.
    assert_eq!(run(r#"<?php echo count(count_chars("", 1));"#), "0");
    assert_eq!(
        run(
            r#"<?php try { count_chars("a", 5); } catch (Throwable $e) { echo get_class($e), ': ', $e->getMessage(); }"#
        ),
        "ValueError: count_chars(): Argument #2 ($mode) must be between 0 and 4 (inclusive)"
    );
}

/// `strtok` did not exist. The load-bearing part is the state machine: running
/// out of tokens DISCARDS the subject, so later one-argument calls keep
/// answering false instead of restarting.
#[test]
fn strtok_tokenizes_and_then_stays_exhausted() {
    assert_eq!(
        run(r#"<?php var_dump(strtok("a b c", " "), strtok(" "), strtok(" "), strtok(" "), strtok(" "));"#),
        "string(1) \"a\"\nstring(1) \"b\"\nstring(1) \"c\"\nbool(false)\nbool(false)\n"
    );
    // Runs of delimiters are collapsed, leading and trailing alike.
    assert_eq!(
        run(r#"<?php var_dump(strtok("  a  b  ", " "), strtok(" "), strtok(" "));"#),
        "string(1) \"a\"\nstring(1) \"b\"\nbool(false)\n"
    );
    // The delimiter set may change between continuations.
    assert_eq!(
        run(r#"<?php var_dump(strtok("a;b.c", ";."), strtok(";."), strtok(";."));"#),
        "string(1) \"a\"\nstring(1) \"b\"\nstring(1) \"c\"\n"
    );
    assert_eq!(run(r#"<?php var_dump(strtok("", "x"));"#), "bool(false)\n");
}

/// An array `$string` used to be stringified to `"Array"` and spliced, so
/// `substr_replace(["ab","cd"], "Z", 1, 1)` answered the string `"AZray"`.
#[test]
fn substr_replace_array_forms() {
    assert_eq!(
        run(r#"<?php print_r(substr_replace(["ab", "cd"], "Z", 1, 1));"#),
        "Array\n(\n    [0] => aZ\n    [1] => cZ\n)\n"
    );
    // Array `$replace` is consumed positionally, one entry per subject.
    assert_eq!(
        run(r#"<?php print_r(substr_replace(["a", "b"], ["X", "Y"], 0, 1));"#),
        "Array\n(\n    [0] => X\n    [1] => Y\n)\n"
    );
    // Exhausted, it falls back to the empty string rather than repeating.
    assert_eq!(
        run(r#"<?php print_r(substr_replace(["abc", "de"], ["X"], 0, 1));"#),
        "Array\n(\n    [0] => Xbc\n    [1] => e\n)\n"
    );
    // Array `$offset` and `$length` are consumed positionally too.
    assert_eq!(
        run(r#"<?php print_r(substr_replace(["abcd", "efgh"], "X", [1, 2], [2, 1]));"#),
        "Array\n(\n    [0] => aXd\n    [1] => efXh\n)\n"
    );
    // String keys are preserved.
    assert_eq!(
        run(r#"<?php print_r(substr_replace(["k" => "abc"], "X", 0, 1));"#),
        "Array\n(\n    [k] => Xbc\n)\n"
    );
    // Against a SINGLE string an array offset or length is a TypeError.
    assert_eq!(
        run(
            r#"<?php try { substr_replace("Hello", "X", [1], 2); } catch (Throwable $e) { echo $e->getMessage(); }"#
        ),
        "substr_replace(): Argument #3 ($offset) cannot be an array when working on a single string"
    );
    assert_eq!(
        run(
            r#"<?php try { substr_replace("Hello", "X", 1, [2]); } catch (Throwable $e) { echo $e->getMessage(); }"#
        ),
        "substr_replace(): Argument #4 ($length) cannot be an array when working on a single string"
    );
}

/// `substr_compare` ignored `$case_insensitive` entirely, and normalized its
/// result to -1/0/1 where the reference answers the raw byte difference.
#[test]
fn substr_compare_is_case_aware_and_unnormalized() {
    assert_eq!(run(r#"<?php echo substr_compare("Hello", "hello", 0, 5, true);"#), "0");
    assert_eq!(run(r#"<?php echo substr_compare("Hello World", "world", 6, 5, true);"#), "0");
    // Differing bytes answer their signed difference: 'c' - 'z' is -23.
    assert_eq!(run(r#"<?php echo substr_compare("abc", "abz", 0, 3);"#), "-23");
    assert_eq!(run(r#"<?php echo substr_compare("abz", "abc", 0, 3);"#), "23");
    // Only a tie on content falls back to the three-way length comparison.
    assert_eq!(run(r#"<?php echo substr_compare("abc", "abcdef", 0);"#), "-1");
    assert_eq!(run(r#"<?php echo substr_compare("a", "", 0);"#), "1");
    // A zero length answers 0 without comparing; a negative one throws.
    assert_eq!(run(r#"<?php echo substr_compare("abc", "x", 0, 0);"#), "0");
    assert_eq!(
        run(
            r#"<?php try { substr_compare("abc", "x", 0, -1); } catch (Throwable $e) { echo $e->getMessage(); }"#
        ),
        "substr_compare(): Argument #4 ($length) must be greater than or equal to 0"
    );
    assert_eq!(
        run(
            r#"<?php try { substr_compare("abc", "x", 5); } catch (Throwable $e) { echo $e->getMessage(); }"#
        ),
        "substr_compare(): Argument #3 ($offset) must be contained in argument #1 ($haystack)"
    );
}

/// `array_replace_recursive` did not exist.
#[test]
fn array_replace_recursive_merges_only_matching_arrays() {
    assert_eq!(
        run(r#"<?php print_r(array_replace_recursive(["a" => ["b" => 1, "c" => 2]], ["a" => ["b" => 9]]));"#),
        "Array\n(\n    [a] => Array\n        (\n            [b] => 9\n            [c] => 2\n        )\n\n)\n"
    );
    // A scalar under the same key replaces rather than merging, both ways round.
    assert_eq!(
        run(r#"<?php print_r(array_replace_recursive(["a" => ["x"]], ["a" => "s"]));"#),
        "Array\n(\n    [a] => s\n)\n"
    );
    assert_eq!(
        run(r#"<?php print_r(array_replace_recursive(["a" => 1], ["a" => ["b" => 2]]));"#),
        "Array\n(\n    [a] => Array\n        (\n            [b] => 2\n        )\n\n)\n"
    );
    // The inputs are value types: merging must not write through into the base.
    assert_eq!(
        run(r#"<?php $b = ["a" => ["b" => 1]]; array_replace_recursive($b, ["a" => ["b" => 9]]); print_r($b);"#),
        "Array\n(\n    [a] => Array\n        (\n            [b] => 1\n        )\n\n)\n"
    );
}

/// `array_walk_recursive` passed leaves by value, so a `&$v` callback could not
/// write back — the array came out unchanged.
#[test]
fn array_walk_recursive_mutates_leaves_by_reference() {
    assert_eq!(
        run(r#"<?php $a = [1, [2, [3]]]; array_walk_recursive($a, function (&$v) { $v *= 2; }); print_r($a);"#),
        "Array\n(\n    [0] => 2\n    [1] => Array\n        (\n            [0] => 4\n            [1] => Array\n                (\n                    [0] => 6\n                )\n\n        )\n\n)\n"
    );
    // The extra argument still reaches the callback.
    assert_eq!(
        run(r#"<?php $a = ["x" => 1]; array_walk_recursive($a, function ($v, $k, $e) { echo "$k=$v/$e"; }, "E");"#),
        "x=1/E"
    );
}

/// The fold used to coerce every entry silently, so a value `+`/`*` rejects was
/// an undiagnosed 0. Upstream warns and — for arrays and objects — skips.
#[test]
fn array_fold_diagnoses_unsupported_operands() {
    assert_eq!(
        run(r#"<?php var_dump(array_sum([1, "a"]));"#),
        "\nWarning: array_sum(): Addition is not supported on type string in Command line code on line 1\nint(1)\n"
    );
    // An array contributes NOTHING, where a non-numeric string counts as 0 —
    // which is the whole reason `array_product([2, "a"])` is 0 and not 2.
    assert_eq!(
        run(r#"<?php var_dump(array_sum([1, [2]]));"#),
        "\nWarning: array_sum(): Addition is not supported on type array in Command line code on line 1\nint(1)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(array_product([2, "a"]));"#),
        "\nWarning: array_product(): Multiplication is not supported on type string in Command line code on line 1\nint(0)\n"
    );
    // A leading-numeric string is an ordinary coercion, with the ordinary notice.
    assert_eq!(
        run(r#"<?php var_dump(array_sum([1, "2abc"]));"#),
        "\nWarning: A non-numeric value encountered in Command line code on line 1\nint(3)\n"
    );
    // Clean input stays silent, and the empty-array identities are unchanged.
    assert_eq!(
        run(r#"<?php var_dump(array_sum([1, null, true, false]), array_sum([]), array_product([]));"#),
        "int(2)\nint(0)\nint(1)\n"
    );
}

/// PHP 8.4 deprecated relying on `str_getcsv`'s default `$escape`; the notice
/// fires on the argument count, before any parsing.
#[test]
fn str_getcsv_deprecates_the_default_escape() {
    assert_eq!(
        run(r#"<?php print_r(str_getcsv("a,b"));"#),
        "\nDeprecated: str_getcsv(): the $escape parameter must be provided as its default value will change in Command line code on line 1\nArray\n(\n    [0] => a\n    [1] => b\n)\n"
    );
    // Passing it explicitly is silent.
    assert_eq!(
        run(r#"<?php print_r(str_getcsv("a,b", ",", '"', "\\"));"#),
        "Array\n(\n    [0] => a\n    [1] => b\n)\n"
    );
}
