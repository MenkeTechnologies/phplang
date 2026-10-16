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

/// `get_class_vars`: the declared defaults the calling scope can see — the
/// class's own declarations ahead of the inherited ones, a typed property with
/// no default as null, then statics at their declared default.
///
/// ```text
/// $ php -r 'class P { public $p=1; protected $pp=2; private $pv=3; public static $ps=4; public int $pt; }
///           class Q extends P { public $q=2; public $p=9; protected $qq=1; static function in() { return get_class_vars("Q"); } }
///           echo json_encode(get_class_vars("Q")), json_encode(Q::in());'
/// {"q":2,"p":9,"pt":null,"ps":4}{"q":2,"p":9,"qq":1,"pp":2,"pt":null,"ps":4}
/// ```
#[test]
fn get_class_vars_reports_defaults_visible_from_scope() {
    let src = r#"<?php
class P { public $p=1; protected $pp=2; private $pv=3; public static $ps=4; public int $pt; }
class Q extends P { public $q=2; public $p=9; protected $qq=1; public static $qs;
  static function in() { return get_class_vars("Q"); } }
Q::$ps = 40;
echo json_encode(get_class_vars("Q")), "\n", json_encode(Q::in()), "\n";
enum E: int { case A = 1; }
echo json_encode(get_class_vars("E")), "\n";
try { get_class_vars("Nope"); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }"#;
    assert_eq!(
        run(src),
        concat!(
            "{\"q\":2,\"p\":9,\"pt\":null,\"qs\":null,\"ps\":4}\n",
            "{\"q\":2,\"p\":9,\"qq\":1,\"pp\":2,\"pt\":null,\"qs\":null,\"ps\":4}\n",
            "{\"name\":null,\"value\":null}\n",
            "get_class_vars(): Argument #1 ($class) must be a valid class name, Nope given\n",
        )
    );
}

/// `class_implements` on a user class: the parent's list, the interfaces named
/// in the class, an automatic `Stringable` for a `__toString`, then the named
/// interfaces' own parents last to first.
///
/// ```text
/// $ php -r 'interface A{} interface B extends A{} interface C{} interface D extends B, C{}
///           class X implements D {}
///           class Y extends X implements Countable { function count():int{return 0;} function __toString(){return "";} }
///           echo implode(",", class_implements("X")), "|", implode(",", class_implements("Y"));'
/// D,A,C,B|D,A,C,B,Countable,Stringable
/// ```
#[test]
fn class_implements_orders_like_the_reference_and_adds_stringable() {
    assert_eq!(
        run(r#"<?php interface A{} interface B extends A{} interface C{} interface D extends B, C{}
class X implements D {}
class Y extends X implements Countable { function count():int{return 0;} function __toString(){return "";} }
class G { function __toString() { return ""; } }
echo implode(",", class_implements("X")), "|", implode(",", class_implements("Y")), "|",
     implode(",", class_implements("D")), "|", implode(",", class_implements("G"));"#),
        "D,A,C,B|D,A,C,B,Countable,Stringable|B,C,A|Stringable"
    );
}

/// An array write into an int, float or `true` is an `Error` — it must not
/// replace the scalar with an array — while `false` is converted with a
/// deprecation that names the write's own line.
///
/// ```text
/// $ php -r '$i = 5; try { $i[0] = 1; } catch (Error $e) { echo $e->getMessage(); }'
/// Cannot use a scalar value as an array
/// ```
#[test]
fn array_write_into_a_scalar_is_an_error_and_false_is_deprecated() {
    let msg = "Cannot use a scalar value as an array\n";
    for stmt in [
        "$v = 5; $v[0] = 1;",
        "$v = 5; $v[] = 1;",
        "$v = 1.5; $v['a']['b'] = 1;",
        "$v = true; $v[0] .= 'a';",
        "$v = 3; $v[0]++;",
        "$v = 3; $v[0] += 1;",
        "$v = ['p' => 5]; $v['p'][] = 1;",
        "$v = 5; $v[0][1] = 1;",
    ] {
        let src = format!("<?php try {{ {stmt} }} catch (Error $e) {{ echo $e->getMessage(), \"\\n\"; }}");
        assert_eq!(run(&src), msg, "statement: {stmt}");
    }
    // The scalar is left alone, not clobbered by an array.
    assert_eq!(
        run("<?php $v = 5; try { $v[0] = 1; } catch (Error $e) {} var_dump($v);"),
        "int(5)\n"
    );
    let out = run("<?php\n$b = false;\n$b[] = 1;\nvar_dump($b);");
    assert!(
        out.starts_with("\nDeprecated: Automatic conversion of false to array is deprecated in ")
            && out.contains(" on line 3\n")
            && out.ends_with("array(1) {\n  [0]=>\n  int(1)\n}\n"),
        "{out}"
    );
}

/// Attributes may precede a closure or an arrow function, `static` or not. They
/// used to be a parse error at the `#[`.
///
/// ```text
/// $ php -r '$f = #[Pure] fn() => 1; $g = #[A, B(1)] static function() { return 2; }; echo $f(), $g();'
/// 12
/// ```
#[test]
fn attributes_may_precede_closures() {
    assert_eq!(
        run("<?php $f = #[Pure] fn() => 1; $g = #[A, B(1)] static function() { return 2; };
$h = #[A] static fn() => 3; $i = #[A] function() { return 4; }; echo $f(), $g(), $h(), $i();"),
        "1234"
    );
}
