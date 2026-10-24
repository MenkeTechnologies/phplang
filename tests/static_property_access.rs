//! Static properties named at run time (`A::$$n`, `A::${expr}`) and bound by
//! reference (`&A::$p`, `A::$p = &$x`). Expected strings are `php -r` output
//! from php 8.5.12 (`/opt/homebrew/bin/php`).

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn a_static_property_name_may_be_computed() {
    let src = r##"<?php class A { public static $x = 5; public static $arr = []; protected static $p = 1;
  static function m() { $k = "p"; return static::$$k . self::${"p"}; } }
class B extends A {}
$n = "x"; $nn = "n";
echo A::$$n, A::${"x"}, A::${"x" . ""}, " ";
A::$$n = 7; echo A::$x, " "; A::$$n += 3; echo A::$x, " "; A::$$n++; echo A::$x, "\n";
$k = "arr"; A::$$k[] = 1; A::$$k["a"] = 2; echo json_encode(A::$arr), "\n";
var_dump(isset(A::$$n), isset(A::${"nope"}), empty(A::$$n));
echo A::m(), " ";
$c = "B"; echo $c::$$n, " "; $o = new B; echo $o::${"x"}, " "; echo A::$$$nn, "\n";
function f() { echo "f "; return "x"; } echo A::${f()}, "\n"; A::${f()} .= "!"; echo A::$x, "\n";
try { echo A::${"nope"}; } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { echo A::${"p"}; } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
$fn = function () use ($n) { return A::$$n; }; echo $fn(), "\n";
$af = fn() => A::${$n . ""}; echo $af(), "\n";"##;
    assert_eq!(
        run(src),
        r##"555 7 10 11
{"0":1,"a":2}
bool(true)
bool(false)
bool(false)
11 11 11 11
f 11
f 11!
Error: Access to undeclared static property A::$nope
Error: Cannot access protected property A::$p
11!
11!
"##
    );
}

#[test]
fn a_static_property_can_be_bound_by_reference() {
    let src = r##"<?php class A { public static $x = 1; public static $arr = [1]; private static $p = 0;
  static function bump() { $r = &static::$p; $r++; return self::$p; } }
class B extends A {}
$r = &A::$x; A::$x = 3; echo $r, " "; $r = 4; echo B::$x, "\n";
$a = &A::$arr; $a[] = 2; echo count(A::$arr), " "; A::$arr[] = 3; echo count($a), "\n";
echo A::bump(), A::bump(), "\n";
$y = 5; A::$x = &$y; $y = 6; echo A::$x, "\n";
try { $z = &A::$nope; } catch (Error $e) { echo $e->getMessage(), "\n"; }
try { $z = &A::$p; } catch (Error $e) { echo $e->getMessage(), "\n"; }
$e0 = &A::$arr[0]; $e0 = 99; echo A::$arr[0], "\n";
$n = "x"; $w = &A::$$n; $w = 7; unset($w); var_dump(A::$x);"##;
    assert_eq!(
        run(src),
        r##"3 4
2 3
12
6
Access to undeclared static property A::$nope
Cannot access private property A::$p
99
int(7)
"##
    );
}
