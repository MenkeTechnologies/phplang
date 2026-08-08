//! `__toString` and string interpolation — the two halves of "how does a value
//! become text".
//!
//! Every expectation is the verbatim stdout of the same program under the
//! reference `php` 8.5.9.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

/// A class whose `__toString` reads a property, which is how essentially every
/// real one is written — and which is why the interpolation forms below have to
/// work for this feature to be usable at all.
const S: &str = r#"class S {
        public $n;
        function __construct($n) { $this->n = $n; }
        function __toString(): string { return "S<{$this->n}>"; }
    }"#;

// ── __toString ───────────────────────────────────────────────────────────────

#[test]
fn to_string_runs_for_echo_concatenation_and_interpolation() {
    let src = format!(
        r#"<?php {S} $s = new S(7);
        echo $s, "|", "x" . $s . "y", "|", "in $s out";"#
    );
    assert_eq!(run(&src), "S<7>|xS<7>y|in S<7> out");
}

#[test]
fn to_string_runs_for_the_explicit_cast_and_strval() {
    let src = format!(r#"<?php {S} $s = new S(7); echo (string)$s, "|", strval($s);"#);
    assert_eq!(run(&src), "S<7>|S<7>");
}

#[test]
fn to_string_runs_for_string_parameter_builtins() {
    // These take a declared `string`, so PHP converts the argument before the
    // call — `strlen` sees the four characters of `S<7>`, not an object.
    let src = format!(
        r#"<?php {S} $s = new S(7);
        echo strlen($s), "|", strtoupper($s), "|", substr($s, 0, 2), "|",
             trim($s, "S<>"), "|", sprintf("[%s]", $s), "|",
             str_contains($s, "S<") ? "y" : "n";"#
    );
    assert_eq!(run(&src), "4|S<7>|S<|7|[S<7>]|y");
}

#[test]
fn implode_converts_each_element() {
    let src = format!(r#"<?php {S} $a = [new S(1), new S(2)]; echo implode(",", $a);"#);
    assert_eq!(run(&src), "S<1>,S<2>");
}

#[test]
fn an_object_without_to_string_is_left_alone_by_a_string_builtin() {
    // The coercion is keyed on the method's presence, so a plain object still
    // reaches the function as an object.
    let src = r#"<?php class P {} $p = new P(); echo get_class($p);"#;
    assert_eq!(run(src), "P");
}

// ── interpolation ────────────────────────────────────────────────────────────

#[test]
fn simple_interpolation_reaches_one_property_or_element_deep() {
    let src = r#"<?php class C { public $n = 7; }
        $o = new C(); $arr = ['k' => 'V', 5 => 'five']; $x = 3;
        echo "1:$x|2:$o->n|3:$arr[k]|4:$arr[5]";"#;
    assert_eq!(run(src), "1:3|2:7|3:V|4:five");
}

#[test]
fn an_unquoted_key_in_simple_interpolation_is_a_string_not_a_constant() {
    // Outside a string `$arr[k]` would look up the constant `k`; inside one it
    // is `$arr['k']`, and defining `k` must not change the answer.
    let src = r#"<?php define('k', 'other');
        $arr = ['k' => 'V', 'other' => 'W'];
        echo "$arr[k]|", $arr[k];"#;
    assert_eq!(run(src), "V|W");
}

#[test]
fn complex_interpolation_takes_any_expression() {
    let src = r#"<?php class C { public $n = 7; public $a = [1,2]; }
        $o = new C(); $arr = ['k' => 'V']; $x = 3;
        echo "1:{$o->n}|2:{$arr['k']}|3:{$o->a[1]}|4:{$x}y";"#;
    assert_eq!(run(src), "1:7|2:V|3:2|4:3y");
}

#[test]
fn a_brace_not_introducing_an_interpolation_stays_literal() {
    // `\{` is an unknown escape, so PHP keeps the backslash and the brace stays
    // literal; the `$x` after it still interpolates and the `}` is text.
    let src = r#"<?php $x = 1; echo "{ }|{a}|{$x}|\{$x}";"#;
    assert_eq!(run(src), "{ }|{a}|1|\\{1}");
}

#[test]
fn the_dollar_brace_form_still_substitutes() {
    // `${var}` is deprecated but not yet removed, and still names the variable.
    let src = r#"<?php $x = 3; echo "${x}";"#;
    assert_eq!(run(src), "3");
}

// ── strcmp's return value ────────────────────────────────────────────────────

#[test]
fn strcmp_returns_the_byte_difference_not_its_sign() {
    // PHP hands back `memcmp`'s value unchanged over the shared prefix, and only
    // falls back to -1/0/1 when one string is a prefix of the other.
    let src = r#"<?php echo strcmp("a","z"), "|", strcmp("z","a"), "|",
        strcmp("a","abcd"), "|", strcmp("A","a"), "|", strcasecmp("A","z"), "|",
        strncmp("aX","zY",1), "|", strncasecmp("aX","ZY",1), "|", strcmp("a","a");"#;
    assert_eq!(run(src), "-25|25|-1|-32|-25|-25|-25|0");
}
