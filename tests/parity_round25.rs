//! Regressions found by differential probing against `php 8.5.10`
//! (`/opt/homebrew/bin/php`, `LC_ALL=C TZ=UTC`, default php.ini). Expected
//! strings are the reference's recorded output; no PHP is needed to run them.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

/// An object without `__toString` has no string form. Echo, `.`, interpolation
/// and `(string)` throw `Error` instead of printing `Array`, and nothing from
/// the failed statement reaches the output.
///
/// ```text
/// $ php -r '$o = new stdClass; try { echo "a $o b"; } catch (Error $e) { echo $e->getMessage(); }'
/// Object of class stdClass could not be converted to string
/// ```
#[test]
fn object_without_to_string_throws_on_string_conversion() {
    let msg = "Object of class Z could not be converted to string\n";
    for stmt in [
        r#"echo "a $o b";"#,
        r#"echo "a " . $o;"#,
        r#"echo $o . "x";"#,
        r#"echo $o;"#,
        r#"echo (string)$o;"#,
        r#"echo strval($o);"#,
        r#"echo implode(",", [$o]);"#,
    ] {
        let src = format!(
            "<?php class Z {{}} $o = new Z; try {{ {stmt} }} catch (Error $e) {{ echo $e->getMessage(), \"\\n\"; }}"
        );
        assert_eq!(run(&src), msg, "statement: {stmt}");
    }
}

/// A nested property in a simple interpolation reads ONE level, so the inner
/// object is what fails to convert.
#[test]
fn simple_interpolation_reads_one_property() {
    assert_eq!(
        run(r#"<?php $o = new stdClass; $o->p = new stdClass;
try { echo "v: $o->p->q"; } catch (Error $e) { echo $e->getMessage(); }"#),
        "Object of class stdClass could not be converted to string"
    );
}

/// A class that DOES define `__toString` is unaffected.
#[test]
fn to_string_still_converts() {
    assert_eq!(
        run(r#"<?php class S { function __toString() { return "s"; } } echo new S, "|", "x" . new S;"#),
        "s|xs"
    );
}

/// Under `declare(strict_types=1)` a library function's scalar parameter takes
/// exactly its own type (int widens to float): no string/float/int coercion, no
/// `__toString`, and `null` is a `TypeError` rather than a deprecation.
///
/// ```text
/// $ php -r 'declare(strict_types=1); try { strlen(5); } catch (TypeError $e) { echo $e->getMessage(); }'
/// strlen(): Argument #1 ($string) must be of type string, int given
/// ```
#[test]
fn strict_types_applies_to_library_calls() {
    let probe = |call: &str| {
        run(&format!(
            "<?php declare(strict_types=1); class S {{ function __toString() {{ return 'x'; }} }}
try {{ var_dump({call}); }} catch (TypeError $e) {{ echo $e->getMessage(), \"\\n\"; }}"
        ))
    };
    assert_eq!(probe("strlen(5)"), "strlen(): Argument #1 ($string) must be of type string, int given\n");
    assert_eq!(probe("strlen(new S)"), "strlen(): Argument #1 ($string) must be of type string, S given\n");
    assert_eq!(probe("str_repeat('a', '3')"), "str_repeat(): Argument #2 ($times) must be of type int, string given\n");
    assert_eq!(probe("intdiv(1.0, 1)"), "intdiv(): Argument #1 ($num1) must be of type int, float given\n");
    assert_eq!(probe("round('1.5')"), "round(): Argument #1 ($num) must be of type int|float, string given\n");
    assert_eq!(probe("strtoupper(null)"), "strtoupper(): Argument #1 ($string) must be of type string, null given\n");
    // Accepted as-is, including the int -> float widening.
    assert_eq!(probe("strlen('abc')"), "int(3)\n");
    assert_eq!(probe("sqrt(4)"), "float(2)\n");
    assert_eq!(probe("str_repeat('ab', 2)"), "string(4) \"abab\"\n");
}

/// Without the declaration the same calls coerce, as before.
#[test]
fn coercive_mode_still_coerces_library_calls() {
    assert_eq!(run("<?php var_dump(strlen(5), str_repeat('a', '3'));"), "int(1)\nstring(3) \"aaa\"\n");
}
