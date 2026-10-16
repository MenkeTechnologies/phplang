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
