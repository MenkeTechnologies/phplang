//! Round 33 regressions found by differential probing against `php 8.5.12`
//! (`/opt/homebrew/bin/php`, `php -r`). Expected strings are the reference's
//! recorded output; no PHP is needed to run them.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn exception_and_error_implement_throwable() {
    let src = r##"<?php foreach (["Exception", "Error", "TypeError", "ErrorException", "ArgumentCountError"] as $c) { echo $c, ": ", implode(",", class_implements($c)), " | ", implode(",", (new ReflectionClass($c))->getInterfaceNames()), "\n"; } class E extends Exception {} echo implode(",", class_implements("E")), "\n"; var_dump(is_subclass_of("Exception", "Throwable"), (new ReflectionClass("Error"))->implementsInterface("Throwable"));"##;
    assert_eq!(
        run(src),
        r##"Exception: Stringable,Throwable | Stringable,Throwable
Error: Stringable,Throwable | Stringable,Throwable
TypeError: Throwable,Stringable | Throwable,Stringable
ErrorException: Throwable,Stringable | Throwable,Stringable
ArgumentCountError: Stringable,Throwable | Stringable,Throwable
Throwable,Stringable
bool(true)
bool(true)
"##
    );
}

#[test]
fn an_internal_class_lists_its_interfaces_in_registration_order() {
    let src = r##"<?php foreach (["ArrayIterator", "ArrayObject", "RecursiveArrayIterator", "SplObjectStorage", "CachingIterator", "SplFixedArray", "SplDoublyLinkedList", "Generator", "AppendIterator"] as $c) { echo $c, ": ", implode(",", class_implements($c)), "\n"; }"##;
    assert_eq!(
        run(src),
        r##"ArrayIterator: SeekableIterator,Traversable,Iterator,ArrayAccess,Serializable,Countable
ArrayObject: IteratorAggregate,Traversable,ArrayAccess,Serializable,Countable
RecursiveArrayIterator: Countable,Serializable,ArrayAccess,Iterator,Traversable,SeekableIterator,RecursiveIterator
SplObjectStorage: Countable,SeekableIterator,Traversable,Iterator,Serializable,ArrayAccess
CachingIterator: Stringable,Iterator,Traversable,OuterIterator,ArrayAccess,Countable
SplFixedArray: IteratorAggregate,Traversable,ArrayAccess,Countable,JsonSerializable
SplDoublyLinkedList: Iterator,Traversable,Countable,ArrayAccess,Serializable
Generator: Iterator,Traversable
AppendIterator: Iterator,Traversable,OuterIterator
"##
    );
}

#[test]
fn spl_arrays_are_serializable() {
    let src = r##"<?php $a = new ArrayObject([1, "x" => 2]); echo $a->serialize(), "\n"; $i = new ArrayIterator([1], 2); echo $i->serialize(), "\n";
$b = new ArrayObject([9]);
try { $b->unserialize("x:i:0;zz"); } catch (Exception $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { $b->unserialize("garbage"); } catch (Exception $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
$b->unserialize('x:i:2;a:1:{i:0;i:5;};m:a:0:{}'); var_dump($b->getFlags(), $b->getArrayCopy());
$b->unserialize(""); var_dump(count($b));
$j = new ArrayIterator([]); $j->unserialize('x:i:1;a:2:{i:0;s:3:"a;m";i:1;b:0;};m:a:0:{}'); var_dump($j->getArrayCopy(), $j->getFlags());
var_dump(new ArrayObject instanceof Serializable, new ArrayIterator instanceof Serializable);"##;
    assert_eq!(
        run(src),
        r##"x:i:0;a:2:{i:0;i:1;s:1:"x";i:2;};m:a:0:{}
x:i:2;a:1:{i:0;i:1;};m:a:0:{}
UnexpectedValueException: Error at offset 6 of 8 bytes
UnexpectedValueException: Error at offset 0 of 7 bytes
int(2)
array(1) {
  [0]=>
  int(5)
}
int(1)
array(2) {
  [0]=>
  string(3) "a;m"
  [1]=>
  bool(false)
}
int(1)
bool(true)
bool(true)
"##
    );
}

#[test]
fn an_element_of_a_call_result_passes_by_value() {
    let src = r##"<?php function h() { return [1, [2]]; } function g(&$a) { $a = 5; echo "g $a\n"; } g(h()[0]); g(h()[1][0]);
class W { static function r(&$a) { $a = 2; echo "r\n"; } static function v($a) { echo "v $a\n"; } function m(&$a) { echo "m\n"; } }
W::r(h()[0]); W::v(h()[0]); (new W)->m(h()[0]); W::v(K::k()[0]); class K { static function k() { return [7]; } }"##;
    assert_eq!(
        run(src),
        r##"g 5
g 5
r
v 1
m
v 7
"##
    );
}

#[test]
fn an_exception_subclass_may_implement_an_interface_extending_throwable() {
    let src = r##"<?php interface I extends Throwable {} class Y extends Exception implements I {} echo implode(",", class_implements("Y")), "\n";"##;
    assert_eq!(run(src), "Stringable,Throwable,I\n");
}

/// `php -r CODE`: stdout and the exit status. A link-time fatal is not
/// something `eval_capture` returns as output.
fn run_cli(code: &str) -> (String, i32) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_php"))
        .args(["-r", code])
        .stderr(std::process::Stdio::null())
        .output()
        .expect("spawn php");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// The reference's stdout for a fatal error raised on line 1 of `php -r`.
fn fatal(msg: &str) -> (String, i32) {
    (
        format!("\nFatal error: {msg} in Command line code on line 1\nStack trace:\n#0 {{main}}\n"),
        255,
    )
}

#[test]
fn only_exception_and_error_may_implement_throwable() {
    let refusal = "cannot implement interface Throwable, extend Exception or Error instead";
    assert_eq!(
        run_cli("class X implements Throwable {}"),
        fatal(&format!("Class X {refusal}"))
    );
    assert_eq!(
        run_cli("abstract class X implements Throwable {}"),
        fatal(&format!("Class X {refusal}"))
    );
    assert_eq!(
        run_cli("interface I extends Throwable {} class X implements I {}"),
        fatal(&format!("Class X {refusal}"))
    );
    assert_eq!(
        run_cli("enum E implements Throwable {}"),
        fatal("Enum E cannot implement interface Throwable")
    );
}

#[test]
fn an_append_through_an_arrayobject_adds_an_integer_keyed_property() {
    let src = r##"<?php $o = new stdClass; $o->a = 1; $ao = @new ArrayObject($o); $ao["b"] = 2; $ao[] = 3; $ao[] = 4;
var_dump($o); print_r($o); var_export($o); echo "\n", serialize($o), " ", json_encode($o), "\n";
var_dump(get_object_vars($o), (array) $o, isset($o->{"0"}), count($ao)); foreach ($o as $k => $v) { var_dump($k); }"##;
    assert_eq!(
        run(src),
        r##"object(stdClass)#1 (4) {
  ["a"]=>
  int(1)
  ["b"]=>
  int(2)
  [0]=>
  int(3)
  [1]=>
  int(4)
}
stdClass Object
(
    [a] => 1
    [b] => 2
    [0] => 3
    [1] => 4
)
(object) array(
   'a' => 1,
   'b' => 2,
   0 => 3,
   1 => 4,
)
O:8:"stdClass":4:{s:1:"a";i:1;s:1:"b";i:2;i:0;i:3;i:1;i:4;} {"a":1,"b":2,"0":3,"1":4}
array(4) {
  ["a"]=>
  int(1)
  ["b"]=>
  int(2)
  [0]=>
  int(3)
  [1]=>
  int(4)
}
array(4) {
  ["a"]=>
  int(1)
  ["b"]=>
  int(2)
  [0]=>
  int(3)
  [1]=>
  int(4)
}
bool(false)
int(4)
string(1) "a"
string(1) "b"
int(0)
int(1)
"##
    );
}

#[test]
fn an_arrayiterator_writes_through_to_the_object_it_wraps() {
    let src = r##"<?php $o2 = new stdClass; $o2->x = 1; $i = @new ArrayIterator($o2); $i["b"] = 2; $i[] = 5; print_r($o2); $o2->c = 9;
foreach ($i as $k => $v) echo "$k=$v "; echo count($i), "\n";
$o3 = new stdClass; $it = @new ArrayIterator($o3); $o3->z = 1; var_dump(isset($it["z"]), $it["z"]);
try { $it->append(1); } catch (Error $e) { echo $e->getMessage(), "\n"; }"##;
    assert_eq!(
        run(src),
        r##"stdClass Object
(
    [x] => 1
    [b] => 2
    [0] => 5
)
x=1 b=2 0=5 c=9 4
bool(true)
int(1)
Cannot append properties to objects, use ArrayIterator::offsetSet() instead
"##
    );
}

#[test]
fn an_object_backing_counts_and_iterates_only_its_public_properties() {
    let src = r##"<?php class P { public $pub = 1; protected $pro = 2; private $pri = 3; } $p = new P; $ap = @new ArrayObject($p);
var_dump(count($ap), array_keys($ap->getArrayCopy()) == ["pub", "\0*\0pro", "\0P\0pri"]); foreach ($ap as $k => $v) { var_dump($k); } var_dump(iterator_count($ap->getIterator()));
$ap["pub"] = 7; var_dump($p->pub);"##;
    assert_eq!(
        run(src),
        r##"int(1)
bool(true)
string(3) "pub"
int(1)
int(7)
"##
    );
}

#[test]
fn an_arrayobjects_iterator_shares_its_storage() {
    let src = r##"<?php $ao = new ArrayObject([1]); $it = $ao->getIterator(); $ao[] = 2; var_dump(count($it)); $it[] = 3; var_dump(count($ao)); print_r($it->getArrayCopy());
class AI extends ArrayIterator { function __construct() { echo "ctor\n"; } } $ao->setIteratorClass("AI"); $j = $ao->getIterator(); var_dump(get_class($j), count($j));
$wrap = new ArrayObject(new ArrayObject([1, 2])); $wrap[] = 3; var_dump(count($wrap));"##;
    assert_eq!(
        run(src),
        r##"int(2)
int(3)
Array
(
    [0] => 1
    [1] => 2
    [2] => 3
)
string(2) "AI"
int(3)

Deprecated: ArrayObject::__construct(): Using an object as a backing array for ArrayObject is deprecated, as it allows violating class constraints and invariants in Command line code on line 3
int(3)
"##
    );
}

#[test]
fn sorting_an_arrayobject_reorders_the_wrapped_objects_table() {
    let src = r##"<?php class P { public $b = 3; protected $a = 1; private $c = 2; } $p = new P; $x = @new ArrayObject($p); $x[] = 0; $x->asort(); var_dump($p);
$o = new stdClass; $y = @new ArrayObject($o); $y["k"] = 2; $y[] = 1; $y->ksort(); print_r($o); var_dump(isset($o->{"0"}));"##;
    assert_eq!(
        run(src),
        r##"object(P)#1 (4) {
  [0]=>
  int(0)
  ["a":protected]=>
  int(1)
  ["c":"P":private]=>
  int(2)
  ["b"]=>
  int(3)
}
stdClass Object
(
    [0] => 1
    [k] => 2
)
bool(false)
"##
    );
}

#[test]
fn a_closure_dump_lists_its_static_variables() {
    let src = r##"<?php $y = 5; $f = function ($a) use ($y) { static $n = 0, $m; static $arr = [1], $c = PHP_INT_SIZE * 2, $s = "s"; static $late = strlen("abc"); $n++; return $n; };
print_r($f); $f(1); $f(1); print_r($f); print_r((new ReflectionFunction($f))->getStaticVariables());"##;
    assert_eq!(
        run(src),
        r##"Closure Object
(
    [name] => {closure:Command line code:1}
    [file] => Command line code
    [line] => 1
    [static] => Array
        (
            [y] => 5
            [n] => 0
            [m] => 
            [arr] => Array
                (
                    [0] => 1
                )

            [c] => 16
            [s] => s
            [late] => 
        )

    [parameter] => Array
        (
            [$a] => <required>
        )

)
Closure Object
(
    [name] => {closure:Command line code:1}
    [file] => Command line code
    [line] => 1
    [static] => Array
        (
            [y] => 5
            [n] => 2
            [m] => 
            [arr] => Array
                (
                    [0] => 1
                )

            [c] => 16
            [s] => s
            [late] => 3
        )

    [parameter] => Array
        (
            [$a] => <required>
        )

)
Array
(
    [y] => 5
    [n] => 2
    [m] => 
    [arr] => Array
        (
            [0] => 1
        )

    [c] => 16
    [s] => s
    [late] => 3
)
"##
    );
}

#[test]
fn each_closure_object_has_its_own_static_variables() {
    let src = r##"<?php $mk = fn() => function () { static $n = 0; return ++$n; }; $a = $mk(); $b = $mk(); $a(); $a(); echo $b(), "\n";
$c = clone $a; echo $c(), $a(), "\n"; $f = function () { static $z = 0; return ++$z; }; $f(); $g = $f->bindTo(null); echo $g(), $f(), "\n";
function h() { static $w = 0; return ++$w; } h(); $fc = h(...); echo $fc(), "\n";"##;
    assert_eq!(
        run(src),
        r##"1
33
22
2
"##
    );
}

#[test]
fn a_static_initializer_runs_once() {
    let src = r##"<?php function foo() { echo "foo "; return 9; } function t() { static $x = foo(); return $x++; } t(); t(); echo t(), "\n";
$q = function () { static $v = foo(); return $v++; }; $q(); echo $q(), "\n";"##;
    assert_eq!(
        run(src),
        r##"foo 11
foo 10
"##
    );
}

#[test]
fn a_function_and_a_methods_static_variables_are_reflected() {
    let src = r##"<?php function named($a) { static $k = 3, $u; static $c = PHP_INT_SIZE * 2; return $k++; }
print_r(named(...)); print_r((new ReflectionFunction("named"))->getStaticVariables()); named(1); print_r(Closure::fromCallable("named"));
class M { function m() { static $q = [1]; static $r = 1 + 1; $q[] = 2; } static function s() { static $z = "z"; } }
print_r((new ReflectionMethod("M", "m"))->getStaticVariables()); (new M)->m(); print_r((new ReflectionMethod("M", "m"))->getStaticVariables()); print_r(M::s(...));
var_dump((new ReflectionFunction("strlen"))->getStaticVariables());"##;
    assert_eq!(
        run(src),
        r##"Closure Object
(
    [function] => named
    [static] => Array
        (
            [k] => 3
            [u] => 
            [c] => 16
        )

    [parameter] => Array
        (
            [$a] => <required>
        )

)
Array
(
    [k] => 3
    [u] => 
    [c] => 16
)
Closure Object
(
    [function] => named
    [static] => Array
        (
            [k] => 4
            [u] => 
            [c] => 16
        )

    [parameter] => Array
        (
            [$a] => <required>
        )

)
Array
(
    [q] => Array
        (
            [0] => 1
        )

    [r] => 2
)
Array
(
    [q] => Array
        (
            [0] => 1
            [1] => 2
        )

    [r] => 2
)
Closure Object
(
    [function] => M::s
    [static] => Array
        (
            [z] => z
        )

)
array(0) {
}
"##
    );
}
