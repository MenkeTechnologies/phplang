//! Computed member names after `->` and `?->`: `$o->$name`, `$o->{expr}`,
//! and the braced literal `$o->{"x y"}`. Expected outputs are recorded from
//! `php` 8.5.11.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

/// Every access form takes a computed name: read, write, `isset`, `unset`,
/// append through the property, a method call, and the nullsafe arrow. A
/// braced string literal names a property no bare identifier can spell.
#[test]
fn read_write_call_isset_unset_through_a_computed_name() {
    let src = r##"<?php
$o = new stdClass;
$o->{"x y"} = 2;
$n = "x y";
echo $o->{"x y"}, $o->$n, "\n";
$o->{"a" . "b"} = 3;
var_dump($o);
class C { public $v = 5; function m() { return 7; } static function s() { return 9; } }
$c = new C;
echo $c->{"v"}, $c->{"m"}(), "\n";
echo isset($o->{"ab"}) ? "y" : "n", "\n";
unset($o->{"ab"});
var_dump($o);
$o->{"arr"}[] = 1; var_dump($o->arr);
$o?->{"x y"};
echo $o?->{"x y"}, "\n";
"##;
    assert_eq!(
        run(src),
        r##"22
object(stdClass)#1 (2) {
  ["x y"]=>
  int(2)
  ["ab"]=>
  int(3)
}
57
y
object(stdClass)#1 (1) {
  ["x y"]=>
  int(2)
}
array(1) {
  [0]=>
  int(1)
}
2
"##
    );
}

/// The name is evaluated once, after the receiver and before the value:
/// `$o->{f()} .= "y"` calls `f` a single time, `r()->{n()}` runs `r` first,
/// and a nullsafe short-circuit skips the name entirely. An arrow function
/// captures the variable that names the member; `__get` / `__call` receive
/// the computed name; a first-class callable binds it.
#[test]
fn computed_name_evaluation_order_and_magic() {
    let src = r##"<?php
function n($s) { echo "n($s) "; return $s; }
function r() { echo "r "; $o = new stdClass; $o->a = 1; return $o; }
$o = new stdClass; $o->a = "x";
$o->{n("a")} .= "y"; echo $o->a, "\n";
$o->{n("c")} = 0; $o->{n("c")}++; ++$o->{"c"}; echo $o->c, "\n";
echo r()->{n("a")}, "\n";
$p = "a"; $f = fn() => $o->$p; echo $f(), "\n";
$ref = &$o->{n("d")}; $ref = 9; echo $o->d, "\n";
$null = null; var_dump($null?->{n("zz")});
$o->{1} = "one"; var_dump($o->{"1"}, $o->{1});
class M { function __get($k) { return "get:$k"; } function __call($m, $a) { return "call:$m:" . implode(",", $a); } function hi($x) { return "hi $x"; } }
$m = new M; $k = "zz"; echo $m->$k, " ", $m->{"q" . "r"}, " ", $m->$k(1, 2), " ", $m->{"h" . "i"}(n(3)), "\n";
$meth = "hi"; $cb = $m->$meth(...); echo $cb(4), "\n";
$o->{"arr"} = []; $o->{"ar" . "r"}[n("k")] = 5; var_dump($o->arr);
unset($o->{n("d")}); var_dump(isset($o->d), isset($o->{"a"}), empty($o->{"c"}));
list($o->{"l1"}, $o->{"l2"}) = [1, 2]; echo $o->l1 + $o->l2, "\n";
$x = null; echo $x?->{"a"}?->b ?? "dflt", "\n";
echo "{$o->a}\n";
$name = "a"; echo $o->$name . "!", "\n";
$nm = null; try { var_dump((new M)->$nm); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
"##;
    assert_eq!(
        run(src),
        r##"n(a) xy
n(c) n(c) 2
r n(a) 1
xy
n(d) 9
NULL
string(3) "one"
string(3) "one"
get:zz get:qr call:zz:1,2 n(3) hi 3
hi 4
n(k) array(1) {
  ["k"]=>
  int(5)
}
n(d) bool(false)
bool(true)
bool(false)
3
dflt
xy
xy!
string(4) "get:"
"##
    );
}
