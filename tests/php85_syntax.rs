//! The PHP 8.5 language additions: the `|>` pipe operator, the `(void)` discard
//! cast and the function form of `clone` with a property array.
//!
//! Every expectation is the stdout of `php -r` under the reference `php` 8.5.11.

use std::process::{Command, Stdio};

fn run_r(code: &str) -> (String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_php"))
        .arg("-r")
        .arg(code)
        .stderr(Stdio::null())
        .output()
        .expect("spawn php -r");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn ok(stdout: &str) -> (String, i32) {
    (stdout.to_string(), 0)
}

#[test]
fn the_pipe_calls_its_right_operand_with_the_left() {
    assert_eq!(
        run_r(
            r#"function f($x) { return $x + 1; }
echo 1 |> f(...), 2 |> f(...) |> f(...), "abc" |> strtoupper(...) |> strrev(...),
     1 + 2 |> (fn($x) => $x * 10), "\n";"#
        ),
        ok("24CBA30\n")
    );
}

#[test]
fn the_pipe_binds_tighter_than_comparison_and_looser_than_concat() {
    // `1 == 1 |> f(...)` is `1 == f(1)`, i.e. `1 == "z"`; the others show the
    // pipe taking the whole shift, concatenation and `!` operand on its left.
    assert_eq!(
        run_r(
            r#"function f($x) { return "z"; }
var_dump(1 == 1 |> f(...), 1 << 1 |> f(...), "a" . "b" |> f(...), true && 1 |> f(...), !1 |> f(...));"#
        ),
        ok("bool(false)\nstring(1) \"z\"\nstring(1) \"z\"\nbool(true)\nstring(1) \"z\"\n")
    );
}

#[test]
fn an_unparenthesized_arrow_function_on_the_right_is_refused() {
    assert_eq!(
        run_r("var_dump(1 |> fn($x) => $x);"),
        (
            "\nFatal error: Arrow functions on the right hand side of |> must be parenthesized \
             in Command line code on line 1\nStack trace:\n#0 {main}\n"
                .to_string(),
            255
        )
    );
}

#[test]
fn a_first_class_callable_adds_no_frame_of_its_own() {
    assert_eq!(
        run_r(
            r#"function f($x) { throw new Exception("e$x"); }
try { 3 |> f(...); } catch (Exception $e) { echo $e->getTraceAsString(); }
class A { function m($x) { throw new Exception("m"); } static function s() { throw new Exception("s"); } }
$c = (new A)->m(...); try { $c(1); } catch (Exception $e) { echo "\n", $e->getTraceAsString(); }
$c = A::s(...); try { $c(); } catch (Exception $e) { echo "\n", $e->getTraceAsString(); }"#
        ),
        ok("#0 Command line code(2): f(3)\n#1 {main}\n#0 Command line code(4): A->m(1)\n#1 {main}\n\
            #0 Command line code(5): A::s()\n#1 {main}")
    );
}

#[test]
fn the_void_cast_discards_a_statement_expression() {
    assert_eq!(
        run_r(r#"(void) print("a"); echo "b"; (void) strlen("x"); ( void ) print "c";"#),
        ok("abc")
    );
}

#[test]
fn the_void_cast_is_a_syntax_error_inside_an_expression() {
    assert_eq!(
        run_r("var_dump((void) 1);"),
        (
            "\nParse error: syntax error, unexpected token \"(void)\" in Command line code on \
             line 1\n"
                .to_string(),
            255
        )
    );
    assert_eq!(
        run_r("function g(): void { return (void) 1; }"),
        (
            "\nParse error: syntax error, unexpected token \"(void)\", expecting \";\" in \
             Command line code on line 1\n"
                .to_string(),
            255
        )
    );
}

#[test]
fn clone_with_assigns_from_the_calling_scope() {
    assert_eq!(
        run_r(
            r#"class P { public function __construct(public readonly int $x = 0, public readonly int $y = 0) {}
  public function withX(int $x): static { return clone($this, ["x" => $x]); } }
$p = (new P(1, 2))->withX(9); echo $p->x, $p->y, "\n";
$o = new stdClass; $o->a = 1; $c = clone($o, ["a" => 2, "b" => 3]);
echo json_encode($c), json_encode($o), "\n";
try { clone(new P(1), ["x" => 5]); } catch (Error $e) { echo $e->getMessage(), "\n", $e->getTraceAsString(), "\n"; }
try { clone($o, 5); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
try { clone(); } catch (ArgumentCountError $e) { echo $e->getMessage(), "\n"; }
echo get_class(clone($o, withProperties: [])), "\n";"#
        ),
        ok("92\n{\"a\":2,\"b\":3}{\"a\":1}\n\
            Cannot modify protected(set) readonly property P::$x from global scope\n\
            #0 Command line code(6): clone(Object(P), Array)\n#1 {main}\n\
            clone(): Argument #2 ($withProperties) must be of type array, int given\n\
            clone() expects at least 1 argument, 0 given\nstdClass\n")
    );
}

#[test]
fn a_parenthesized_single_operand_is_still_a_plain_clone() {
    // `clone($a)->m()` clones the result of `($a)->m()`, not `$a`.
    assert_eq!(
        run_r(
            r#"class R { public $a; public function __clone() { echo "cloned "; } }
$r = clone(new R, ["a" => 3]); echo $r->a, "\n";
try { clone(new Exception("x")); } catch (Error $e) { echo $e->getMessage(), "\n"; }
class A { function m($x) { return $x; } }
clone(new A)->m(1);"#
        ),
        (
            "cloned 3\nTrying to clone an uncloneable object of class Exception\n\
             \nFatal error: Uncaught TypeError: clone(): Argument #1 ($object) must be of type \
             object, int given in Command line code:5\nStack trace:\n#0 {main}\n  thrown in \
             Command line code on line 5\n"
                .to_string(),
            255
        )
    );
}

#[test]
fn an_enum_declaration_without_a_brace_is_a_syntax_error_at_the_offender() {
    assert_eq!(
        run_r("enum E extends {}"),
        (
            "\nParse error: syntax error, unexpected token \"extends\", expecting \"{\" in \
             Command line code on line 1\n"
                .to_string(),
            255
        )
    );
    assert_eq!(
        run_r("enum E 1 { case A; }"),
        (
            "\nParse error: syntax error, unexpected integer \"1\", expecting \"{\" in Command \
             line code on line 1\n"
                .to_string(),
            255
        )
    );
}
