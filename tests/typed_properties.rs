//! Typed properties declared without a default start UNINITIALIZED: absent
//! from the instance, printed by `var_dump` as `uninitialized(T)` in their
//! declared slot, and an `Error` to read before the first write. Expected
//! outputs are recorded from `php` 8.5.11.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

/// Every surface sees an uninitialized property as absent except `var_dump`,
/// which lists it without counting it; a read, `++` and a read after `unset()`
/// are the `Error`.
#[test]
fn uninitialized_is_absent_everywhere_but_var_dump() {
    let src = r##"<?php
class C { public int $p; public ?string $q; public $u; public int $d = 3; }
$c = new C;
var_dump($c);
var_dump($c->u, isset($c->p), property_exists($c, "p"));
print_r($c); echo "\n";
var_dump((array)$c, get_object_vars($c));
try { echo $c->p; } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { $c->p++; } catch (Error $e) { echo $e->getMessage(), "\n"; }
$c->p = 5; var_dump($c->p);
unset($c->p);
try { echo $c->p; } catch (Error $e) { echo $e->getMessage(), "\n"; }
echo json_encode($c), "\n";
var_export($c); echo "\n";
echo serialize($c), "\n";
foreach ($c as $k => $v) echo "$k ";
echo "\n";
var_dump($c->q ?? "dflt");
"##;
    assert_eq!(run(src), r##"object(C)#1 (2) {
  ["p"]=>
  uninitialized(int)
  ["q"]=>
  uninitialized(?string)
  ["u"]=>
  NULL
  ["d"]=>
  int(3)
}
NULL
bool(false)
bool(true)
C Object
(
    [u] => 
    [d] => 3
)

array(2) {
  ["u"]=>
  NULL
  ["d"]=>
  int(3)
}
array(2) {
  ["u"]=>
  NULL
  ["d"]=>
  int(3)
}
Error: Typed property C::$p must not be accessed before initialization
Typed property C::$p must not be accessed before initialization
int(5)
Typed property C::$p must not be accessed before initialization
{"u":null,"d":3}
\C::__set_state(array(
   'u' => NULL,
   'd' => 3,
))
O:1:"C":2:{s:1:"u";N;s:1:"d";i:3;}
u d 
string(4) "dflt"
"##);
}

/// The declared type prints the way `zend_type_to_string` spells it: class
/// names first, builtins in the engine's fixed order, `?T` for a single
/// nullable type, `self` resolved, `iterable` as `Traversable|array`.
#[test]
fn declared_type_spelling() {
    let src = r##"<?php class A { public int|string $a; public ?Foo $b; public string|int|null $c; public Foo|Bar|null $d; public null|int $e; public INT $f; public mixed $h; public float|bool $i; public array|false $j; public self $k; public (A&B)|null $l; public iterable $m; public static $s; } var_dump(new A);
"##;
    assert_eq!(run(src), r##"object(A)#1 (0) {
  ["a"]=>
  uninitialized(string|int)
  ["b"]=>
  uninitialized(?Foo)
  ["c"]=>
  uninitialized(string|int|null)
  ["d"]=>
  uninitialized(Foo|Bar|null)
  ["e"]=>
  uninitialized(?int)
  ["f"]=>
  uninitialized(int)
  ["h"]=>
  uninitialized(mixed)
  ["i"]=>
  uninitialized(float|bool)
  ["j"]=>
  uninitialized(array|false)
  ["k"]=>
  uninitialized(A)
  ["l"]=>
  uninitialized((A&B)|null)
  ["m"]=>
  uninitialized(Traversable|array)
}
"##);
}

/// Inherited, trait-supplied, redeclared-with-default and promoted properties;
/// a property written back after `unset()` returns to its declared slot, a
/// trait's properties follow the class's own, `__get` answers only after an
/// explicit `unset()`, and an array-typed property auto-vivifies.
#[test]
fn slots_inheritance_traits_and_magic() {
    let src = r##"<?php
trait T { public string $fromTrait; }
#[AllowDynamicProperties]
class P { protected int $pp; private ?array $priv; public $x = 1; }
class K extends P { use T; public float $k; public ?int $n = null; public int $pp2; }
$k = new K;
var_dump($k);
$k->k = 1.5; $k->fromTrait = "t";
var_dump($k);
$c = clone $k; var_dump($c);
unset($k->k);
var_dump($k);
$k->k = 2.0; $k->dyn = 1;
foreach ($k as $name => $v) echo "$name ";
echo "\n";
var_dump($k->n, $k->pp2 ?? "unset", isset($k->pp2), empty($k->pp2));
class Q extends K { public float $k = 0.5; }
$q = new Q; var_dump($q);
class RO { public function __construct(public readonly int $a, public string $b = "x") {} }
$ro = new RO(3); var_dump($ro);
class G { public int $v; public function __get($n) { return 42; } }
$g = new G; try { var_dump($g->v); } catch (Error $e) { echo $e->getMessage(), "\n"; }
$k2 = new K; print_r($k2); echo "\n";
$k3 = new K; $p3 = new P; var_dump(count((array) $k3), get_object_vars($p3));
class Arr { public array $a; } $o = new Arr; $o->a[] = 1; $o->a["k"] = 2; var_dump($o);
$g2 = new G; unset($g2->v); var_dump($g2->v); try { $g->v++; } catch (Error $e) { echo $e->getMessage(), "
"; }
"##;
    assert_eq!(run(src), r##"object(K)#1 (2) {
  ["pp":protected]=>
  uninitialized(int)
  ["priv":"P":private]=>
  uninitialized(?array)
  ["x"]=>
  int(1)
  ["k"]=>
  uninitialized(float)
  ["n"]=>
  NULL
  ["pp2"]=>
  uninitialized(int)
  ["fromTrait"]=>
  uninitialized(string)
}
object(K)#1 (4) {
  ["pp":protected]=>
  uninitialized(int)
  ["priv":"P":private]=>
  uninitialized(?array)
  ["x"]=>
  int(1)
  ["k"]=>
  float(1.5)
  ["n"]=>
  NULL
  ["pp2"]=>
  uninitialized(int)
  ["fromTrait"]=>
  string(1) "t"
}
object(K)#2 (4) {
  ["pp":protected]=>
  uninitialized(int)
  ["priv":"P":private]=>
  uninitialized(?array)
  ["x"]=>
  int(1)
  ["k"]=>
  float(1.5)
  ["n"]=>
  NULL
  ["pp2"]=>
  uninitialized(int)
  ["fromTrait"]=>
  string(1) "t"
}
object(K)#1 (3) {
  ["pp":protected]=>
  uninitialized(int)
  ["priv":"P":private]=>
  uninitialized(?array)
  ["x"]=>
  int(1)
  ["k"]=>
  uninitialized(float)
  ["n"]=>
  NULL
  ["pp2"]=>
  uninitialized(int)
  ["fromTrait"]=>
  string(1) "t"
}
x k n fromTrait dyn 
NULL
string(5) "unset"
bool(false)
bool(true)
object(Q)#3 (3) {
  ["pp":protected]=>
  uninitialized(int)
  ["priv":"P":private]=>
  uninitialized(?array)
  ["x"]=>
  int(1)
  ["k"]=>
  float(0.5)
  ["n"]=>
  NULL
  ["pp2"]=>
  uninitialized(int)
  ["fromTrait"]=>
  uninitialized(string)
}
object(RO)#4 (2) {
  ["a"]=>
  int(3)
  ["b"]=>
  string(1) "x"
}
Typed property G::$v must not be accessed before initialization
K Object
(
    [x] => 1
    [n] => 
)

int(2)
array(1) {
  ["x"]=>
  int(1)
}
object(Arr)#10 (1) {
  ["a"]=>
  array(2) {
    [0]=>
    int(1)
    ["k"]=>
    int(2)
  }
}
int(42)
Typed property G::$v must not be accessed before initialization
"##);
}

/// The error names an anonymous class the way the reference displays it.
#[test]
fn anonymous_class_is_named_class_at_anonymous() {
    let src = r#"<?php try { echo (new class { public int $p; })->p; } catch (Error $e) { echo $e->getMessage(), "\n"; }"#;
    assert_eq!(
        run(src),
        "Typed property class@anonymous::$p must not be accessed before initialization\n"
    );
}
