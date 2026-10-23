//! Regressions found by differential probing against `php 8.5.11`
//! (`/opt/homebrew/bin/php`, `LC_ALL=C TZ=UTC`, default php.ini, `php -r`).
//! Expected strings are the reference's recorded output; no PHP is needed to
//! run them.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

/// `$o->p[] = v` where `p` resolves to a by-value `__get`: the reference calls
/// `__get`, raises `Indirect modification of overloaded property`, and writes
/// into a temporary. It used to create a dynamic property with a deprecation.
#[test]
fn magic_get_indirect_modification() {
    let src = r#"<?php class M { public $d = ['arr' => [1]]; function __get($n) { echo "[get $n]"; return $this->d[$n] ?? null; } }
        $m = new M; $m->arr[] = 2; $m->nope['k'] = 1; echo json_encode($m->d), "\n";
        "#;
    assert_eq!(run(src), "[get arr]\nNotice: Indirect modification of overloaded property M::$arr has no effect in Command line code on line 2\n[get nope]\nNotice: Indirect modification of overloaded property M::$nope has no effect in Command line code on line 2\n{\"arr\":[1]}\n");
}

/// A `function &__get()` hands out a reference, so `$o->p[] = v` writes into the
/// storage it names (vivifying a missing one), with no notice.
#[test]
fn magic_ref_get_writes_through() {
    let src = r#"<?php class R { public $d = ['arr' => [1]]; function &__get($n) { echo "[get $n]"; return $this->d[$n]; } }
        $r = new R; $r->arr[] = 2; $r->arr['k'] = 3; $r->fresh[] = 4; echo json_encode($r->d), "\n";
        "#;
    assert_eq!(
        run(src),
        "[get arr][get arr][get fresh]{\"arr\":{\"0\":1,\"1\":2,\"k\":3},\"fresh\":[4]}\n"
    );
}

/// `spl_handlers_dual_it` and `spl_handlers_rec_it_it` set `clone_obj = NULL`, as
/// does `WeakReference`: cloning one (or a subclass) is an `Error`.
#[test]
fn spl_dual_iterators_uncloneable() {
    let src = r#"<?php class LI extends LimitIterator {}
        foreach ([new IteratorIterator(new ArrayIterator([1])), new RecursiveIteratorIterator(new RecursiveArrayIterator([1])), new CallbackFilterIterator(new ArrayIterator([]), fn() => true), new LI(new ArrayIterator([1])), WeakReference::create(new stdClass), new ArrayIterator([]), new SplObjectStorage] as $o) {
            try { $c = clone $o; echo get_class($c), " cloned\n"; } catch (Error $e) { echo $e->getMessage(), "\n"; }
        }
        "#;
    assert_eq!(run(src), "Trying to clone an uncloneable object of class IteratorIterator\nTrying to clone an uncloneable object of class RecursiveIteratorIterator\nTrying to clone an uncloneable object of class CallbackFilterIterator\nTrying to clone an uncloneable object of class LI\nTrying to clone an uncloneable object of class WeakReference\nArrayIterator cloned\nSplObjectStorage cloned\n");
}

/// `clone` keeps an `ArrayIterator`'s flags (its position starts over) and an
/// `ArrayObject`'s flags and iterator class.
#[test]
fn array_iterator_clone_keeps_flags() {
    let src = r#"<?php $i = new ArrayIterator([1, 2, 3], ArrayIterator::ARRAY_AS_PROPS); $i->next(); $j = clone $i;
        var_dump($j->getFlags(), $j->key());
        $a = new ArrayObject([1], ArrayObject::STD_PROP_LIST, 'RecursiveArrayIterator'); $b = clone $a;
        var_dump($b->getFlags(), $b->getIteratorClass());
        "#;
    assert_eq!(
        run(src),
        "int(2)\nint(0)\nint(1)\nstring(22) \"RecursiveArrayIterator\"\n"
    );
}

/// `ArrayObject` keeps its flags and iterator class, validates the class as the
/// `C` parameter type does, hands the iterator its flags, and serializes both.
#[test]
fn array_object_flags_and_iterator_class() {
    let src = r#"<?php class MyIt extends ArrayIterator {}
        $a = new ArrayObject([1, 2], 0, 'myit');
        var_dump($a->getFlags(), $a->getIteratorClass(), get_class($a->getIterator()));
        $a->setFlags(ArrayObject::STD_PROP_LIST | ArrayObject::ARRAY_AS_PROPS); $a->setIteratorClass('ArrayIterator');
        var_dump($a->getFlags(), $a->getIterator()->getFlags(), serialize($a));
        $u = unserialize(serialize(new ArrayObject([1], 1, 'MyIt'))); var_dump($u->getFlags(), $u->getIteratorClass());
        foreach (['nope', 'stdClass'] as $c) {
            try { new ArrayObject([], 0, $c); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
            try { $a->setIteratorClass($c); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
        }
        "#;
    assert_eq!(run(src), "int(0)\nstring(4) \"MyIt\"\nstring(4) \"MyIt\"\nint(3)\nint(3)\nstring(73) \"O:11:\"ArrayObject\":4:{i:0;i:3;i:1;a:2:{i:0;i:1;i:1;i:2;}i:2;a:0:{}i:3;N;}\"\nint(1)\nstring(4) \"MyIt\"\nArrayObject::__construct(): Argument #3 ($iteratorClass) must be a class name derived from ArrayIterator, nope given\nArrayObject::setIteratorClass(): Argument #1 ($iteratorClass) must be a class name derived from ArrayIterator, nope given\nArrayObject::__construct(): Argument #3 ($iteratorClass) must be a class name derived from ArrayIterator, stdClass given\nArrayObject::setIteratorClass(): Argument #1 ($iteratorClass) must be a class name derived from ArrayIterator, stdClass given\n");
}

/// ARRAY_AS_PROPS: a property the object does not have is an element, for reads,
/// writes, `isset`, `unset`, `++` and `[]=`. Without the flag the property path
/// is the ordinary one, and reflection does not see the forwarding methods.
#[test]
fn array_object_array_as_props() {
    let src = r#"<?php $a = new ArrayObject(['x' => 1], ArrayObject::ARRAY_AS_PROPS);
        var_dump($a->x, isset($a->x), isset($a->y));
        $a->y = 2; $a->list = [1]; $a->list[] = 2; $a->n = 1; $a->n++; unset($a->x);
        var_dump($a->getArrayCopy(), method_exists($a, '__get'));
        class Sub extends ArrayObject { public $real = 'r'; }
        $s = new Sub([], ArrayObject::ARRAY_AS_PROPS); $s->real = 'w'; $s->other = 'o';
        var_dump($s->real, $s['other'], count($s));
        $p = new ArrayObject(['k' => 1]); var_dump(isset($p->k)); $p->dyn = 1; var_dump(count($p));
        "#;
    assert_eq!(run(src), "int(1)\nbool(true)\nbool(false)\narray(3) {\n  [\"y\"]=>\n  int(2)\n  [\"list\"]=>\n  array(2) {\n    [0]=>\n    int(1)\n    [1]=>\n    int(2)\n  }\n  [\"n\"]=>\n  int(2)\n}\nbool(false)\nstring(1) \"w\"\nstring(1) \"o\"\nint(1)\nbool(false)\n\nDeprecated: Creation of dynamic property ArrayObject::$dyn is deprecated in Command line code on line 8\nint(1)\n");
}

/// `print_r` lists the ordinary properties before the storage
/// (`spl_array_get_debug_info`); under STD_PROP_LIST `(array)`, `json_encode` and
/// `var_export` see the properties, not the storage.
#[test]
fn array_object_std_prop_list_and_debug_order() {
    let src = r#"<?php $a = new ArrayObject(['x' => 1]); $a->p = 5;
        print_r($a); echo "\n";
        $s = new ArrayObject([1], ArrayObject::STD_PROP_LIST); $s->q = 2;
        var_dump((array) $s, json_encode($s)); var_export($s); echo "\n";
        "#;
    assert_eq!(run(src), "\nDeprecated: Creation of dynamic property ArrayObject::$p is deprecated in Command line code on line 1\nArrayObject Object\n(\n    [p] => 5\n    [storage:ArrayObject:private] => Array\n        (\n            [x] => 1\n        )\n\n)\n\n\nDeprecated: Creation of dynamic property ArrayObject::$q is deprecated in Command line code on line 3\narray(1) {\n  [\"q\"]=>\n  int(2)\n}\nstring(7) \"{\"q\":2}\"\n\\ArrayObject::__set_state(array(\n   'q' => 2,\n))\n");
}

/// PHP 8.5 deprecates an object as the backing array of `ArrayObject`
/// (`__construct`, `exchangeArray`) and `ArrayIterator`.
#[test]
fn array_object_object_backing_deprecated() {
    let src = r#"<?php $o = new stdClass; $o->a = 1;
        $x = new ArrayObject($o); echo count($x), "\n";
        $x->exchangeArray(new ArrayObject([5, 6])); echo count($x), "\n";
        $i = new ArrayIterator($o); echo count($i), "\n";
        "#;
    assert_eq!(run(src), "\nDeprecated: ArrayObject::__construct(): Using an object as a backing array for ArrayObject is deprecated, as it allows violating class constraints and invariants in Command line code on line 2\n1\n\nDeprecated: ArrayObject::exchangeArray(): Using an object as a backing array for ArrayObject is deprecated, as it allows violating class constraints and invariants in Command line code on line 3\n2\n\nDeprecated: ArrayIterator::__construct(): Using an object as a backing array for ArrayIterator is deprecated, as it allows violating class constraints and invariants in Command line code on line 4\n1\n");
}

/// `get_declared_classes` lists classes and enums in declared spelling and
/// declaration order; interfaces and traits have their own functions. It used to
/// return every table key lowercased in hash order.
#[test]
fn get_declared_classes_order_and_spelling() {
    let src = r#"<?php class MyClass {} interface MyIface {} trait MyTrait {} enum MyEnum { case A; }
        $c = get_declared_classes();
        var_dump(array_slice($c, -2), in_array('ArrayObject', $c), in_array('arrayobject', $c), in_array('MyIface', $c), in_array('MyTrait', $c));
        var_dump(end(get_declared_interfaces()), in_array('Traversable', get_declared_interfaces()), get_declared_traits());
        "#;
    assert_eq!(run(src), "array(2) {\n  [0]=>\n  string(7) \"MyClass\"\n  [1]=>\n  string(6) \"MyEnum\"\n}\nbool(true)\nbool(false)\nbool(false)\nbool(false)\n\nNotice: Only variables should be passed by reference in Command line code on line 4\nstring(7) \"MyIface\"\nbool(true)\narray(1) {\n  [0]=>\n  string(7) \"MyTrait\"\n}\n");
}

/// A promoted constructor parameter declares its property at the constructor's
/// place among the declarations; typed and unassigned, it dumps as
/// `uninitialized(type)`.
#[test]
fn promoted_property_slot_order() {
    let src = r#"<?php class K { public $a = 1; function __construct(public $b = 2) {} public $c = 3; }
        $k = new K; unset($k->a); $k->a = 9; var_dump($k);
        class P { function __construct(public int $x, protected ?string $y = null) {} }
        class Q extends P { public $q = 'q'; function __construct() {} }
        var_dump(new Q);
        "#;
    assert_eq!(run(src), "object(K)#1 (3) {\n  [\"a\"]=>\n  int(9)\n  [\"b\"]=>\n  int(2)\n  [\"c\"]=>\n  int(3)\n}\nobject(Q)#2 (1) {\n  [\"x\"]=>\n  uninitialized(int)\n  [\"y\":protected]=>\n  uninitialized(?string)\n  [\"q\"]=>\n  string(1) \"q\"\n}\n");
}

/// `ZEND_ACC_NOT_SERIALIZABLE`: `serialize` throws for a closure, a generator, an
/// anonymous class or a `WeakMap` anywhere in the value, and `unserialize` throws
/// for such a class name instead of building one.
#[test]
fn serialize_refuses_closures_generators_anonymous() {
    let src = r#"<?php function g() { yield 1; }
        foreach ([fn() => serialize(function () {}), fn() => serialize([1, g()]), fn() => serialize(new class {}), fn() => serialize((object) ['f' => fn() => 1]), fn() => serialize(new WeakMap), fn() => unserialize('O:7:"Closure":0:{}'), fn() => unserialize('a:1:{i:0;O:9:"Generator":0:{}}'), fn() => unserialize('O:7:"WeakMap":0:{}')] as $f) {
            try { var_dump($f()); } catch (Exception $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
        }
        "#;
    assert_eq!(run(src), "Exception: Serialization of 'Closure' is not allowed\nException: Serialization of 'Generator' is not allowed\nException: Serialization of 'class@anonymous' is not allowed\nException: Serialization of 'Closure' is not allowed\nException: Serialization of 'WeakMap' is not allowed\nException: Unserialization of 'Closure' is not allowed\nException: Unserialization of 'Generator' is not allowed\nException: Unserialization of 'WeakMap' is not allowed\n");
}

/// `WeakReference`: one per referent, no properties of its own, `get()` answers
/// the referent; direct instantiation, dynamic properties and a non-object are
/// refused.
#[test]
fn weak_reference() {
    let src = r#"<?php $o = new stdClass; $w = WeakReference::create($o);
        var_dump($w, $w->get() === $o, WeakReference::create($o) === $w, (array) $w, json_encode($w));
        foreach ([fn() => new WeakReference, fn() => $w->x = 1, fn() => WeakReference::create(1)] as $f) {
            try { $f(); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
        }
        "#;
    assert_eq!(run(src), "object(WeakReference)#2 (1) {\n  [\"object\"]=>\n  object(stdClass)#1 (0) {\n  }\n}\nbool(true)\nbool(true)\narray(0) {\n}\nstring(2) \"{}\"\nError: Direct instantiation of WeakReference is not allowed, use WeakReference::create instead\nError: Cannot create dynamic property WeakReference::$x\nTypeError: WeakReference::create(): Argument #1 ($object) must be of type object, int given\n");
}

/// `WeakMap`: object keys, `isset` as present-and-not-null, the debug view's
/// `key`/`value` rows, cloning, and the reference's refusals.
#[test]
fn weak_map() {
    let src = r#"<?php $wm = new WeakMap; $a = new stdClass; $b = new ArrayObject; $wm[$a] = 1; $wm[$b] = [2]; $wm[$a] = 'one';
        var_dump(count($wm), $wm[$a], isset($wm[$a]), isset($wm[new stdClass]), $wm);
        foreach ($wm as $k => $v) { echo get_class($k), " => ", json_encode($v), "\n"; }
        $wm[$a] = null; var_dump(isset($wm[$a]), $wm->offsetExists($a));
        unset($wm[$b]); unset($wm[new stdClass]); var_dump(count($wm));
        $c = clone $wm; $c[$b] = 3; var_dump(count($wm), count($c), (array) $wm, json_encode($wm));
        foreach ([fn() => $wm[1] = 2, fn() => $wm[] = 1, fn() => isset($wm["x"]), fn() => $wm->x = 1] as $f) {
            try { $f(); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
        }
        "#;
    assert_eq!(run(src), "int(2)\nstring(3) \"one\"\nbool(true)\nbool(false)\nobject(WeakMap)#1 (2) {\n  [0]=>\n  array(2) {\n    [\"key\"]=>\n    object(stdClass)#2 (0) {\n    }\n    [\"value\"]=>\n    string(3) \"one\"\n  }\n  [1]=>\n  array(2) {\n    [\"key\"]=>\n    object(ArrayObject)#3 (1) {\n      [\"storage\":\"ArrayObject\":private]=>\n      array(0) {\n      }\n    }\n    [\"value\"]=>\n    array(1) {\n      [0]=>\n      int(2)\n    }\n  }\n}\nstdClass => \"one\"\nArrayObject => [2]\nbool(false)\nbool(false)\nint(1)\nint(1)\nint(2)\narray(0) {\n}\nstring(2) \"{}\"\nTypeError: WeakMap key must be an object\nError: Cannot append to WeakMap\nTypeError: WeakMap key must be an object\nError: Cannot create dynamic property WeakMap::$x\n");
}

/// `php_json_encode_zval` asks `JsonSerializable` before the enum rule, so an enum
/// implementing it encodes as its `jsonSerialize()` answer.
#[test]
fn json_serializable_enum() {
    let src = r#"<?php enum E: int implements JsonSerializable { case A = 1; case B = 2; public function jsonSerialize(): mixed { return strtolower($this->name); } }
        enum F: int { case X = 7; }
        echo json_encode([E::A, E::B, F::X]), "\n";
        "#;
    assert_eq!(run(src), "[\"a\",\"b\",7]\n");
}

/// A loose `in_array` / `array_search` / `array_keys` compares an object with
/// `__toString` against a string through the string, as `==` does; SORT_STRING
/// `array_unique` compares what `__toString` returns.
#[test]
fn loose_search_casts_stringable() {
    let src = r#"<?php class S { function __construct(private $s) {} function __toString(): string { return $this->s; } }
        $s = new S("s"); $t = new S("10");
        var_dump(in_array("s", [$s]), in_array($s, ["x", "s"]), array_search($s, ["x", "s"]), array_keys([$s, "s", 1, $t], "s"), in_array("1e1", [$t]), in_array($s, ["s"], true));
        var_dump(array_unique(["s", $s, $t, "10"]));
        "#;
    assert_eq!(run(src), "bool(true)\nbool(true)\nint(1)\narray(2) {\n  [0]=>\n  int(0)\n  [1]=>\n  int(1)\n}\nbool(true)\nbool(false)\narray(2) {\n  [0]=>\n  string(1) \"s\"\n  [2]=>\n  object(S)#2 (1) {\n    [\"s\":\"S\":private]=>\n    string(2) \"10\"\n  }\n}\n");
}

/// `iterator_apply` rewinds and advances the iterator itself and never calls
/// `current()` or `key()`, so a callback that reads the iterator sees each
/// position in turn; a falsy answer stops the walk.
#[test]
fn iterator_apply_drives_iterator() {
    let src = r#"<?php class It implements Iterator { private $i = 0; function rewind(): void { echo "rw "; $this->i = 0; } function valid(): bool { echo "v "; return $this->i < 2; } function current(): mixed { echo "c "; return $this->i; } function key(): mixed { echo "k "; return $this->i; } function next(): void { echo "n "; $this->i++; } }
        $it = new It; var_dump(iterator_apply($it, function ($x) { echo "[cb $x] "; return true; }, ['a']));
        $ai = new ArrayIterator([5, 6, 7]); var_dump(iterator_apply($ai, function () use ($ai) { echo $ai->key(), "=", $ai->current(), " "; return $ai->current() < 6; }));
        "#;
    assert_eq!(
        run(src),
        "rw v [cb a] n v [cb a] n v int(2)\n0=5 1=6 int(2)\n"
    );
}

/// A by-reference `foreach` walks the live array: an element the body appends is
/// visited, after the deleted ones are skipped. It used to stop at the keys the
/// loop started with.
#[test]
fn foreach_by_ref_visits_appended() {
    let src = r#"<?php $a = [1, 2, 3]; foreach ($a as $k => &$v) { $v *= 2; if ($k == 0) { $a[] = 99; } } unset($v); print_r($a);
        $b = [1]; foreach ($b as $k => &$v) { echo "$k "; if ($k < 4) $b[] = $k + 10; } unset($v); echo count($b), "\n";
        $c = ['a' => 1, 'b' => 2]; foreach ($c as $k => &$v) { echo $k; if ($k == 'a') { unset($c['b']); $c['z'] = 3; } } unset($v); echo "\n";
        $d = [1, 2]; foreach ($d as $k => &$v) { echo $k; if ($k == 1) unset($d[1]); if ($k == 0) $d[] = 5; } echo "\n";
        $e = []; foreach ($e as &$v) { echo "never"; } echo "ok\n";
        $f = [1, 2]; foreach ($f as &$v) { if ($v == 1) { $f[] = 3; } if ($v == 2) break; } unset($v); print_r($f);
        "#;
    assert_eq!(run(src), "Array\n(\n    [0] => 2\n    [1] => 4\n    [2] => 6\n    [3] => 198\n)\n0 1 2 3 4 5\naz\n012\nok\nArray\n(\n    [0] => 1\n    [1] => 2\n    [2] => 3\n)\n");
}

/// A `&` target destructuring a call result has nothing to alias: each
/// by-reference fetch raises `Attempting to set reference to non referenceable
/// value` and binds a copy. A variable subject is aliased with no notice.
#[test]
fn list_ref_on_call_result_notices() {
    let src = r#"<?php function f() { return [1, [2]]; }
        [&$x, [&$y]] = f(); echo $x, $y, "\n";
        [$a, &$b] = f(); echo $a, json_encode($b), "\n";
        $v = [1, [2]]; [&$p, [&$q]] = $v; $p = 5; $q = 6; echo json_encode($v), "\n";
        "#;
    assert_eq!(run(src), "\nNotice: Attempting to set reference to non referenceable value in Command line code on line 2\n\nNotice: Attempting to set reference to non referenceable value in Command line code on line 2\n\nNotice: Attempting to set reference to non referenceable value in Command line code on line 2\n12\n\nNotice: Attempting to set reference to non referenceable value in Command line code on line 3\n1[2]\n[5,[6]]\n");
}

/// An arrow function binds each free variable of its body only when the enclosing
/// scope has it, with no diagnostic (`ZEND_BIND_IMPLICIT`); a `use` clause still
/// warns. A variable the body assigns used to warn `Undefined variable` when the
/// arrow function was created.
#[test]
fn arrow_fn_implicit_capture_is_silent() {
    let src = r#"<?php $f = fn() => $u ?? 1; echo $f(), "\n";
        $g = fn() => ($z = 5) + 1; echo $g(), "\n";
        $n = null; $h = fn() => $n; var_dump($h());
        $k = fn() => $nope; var_dump($k());
        $c = function () use ($w) { return $w; }; var_dump($c());
        "#;
    assert_eq!(run(src), "1\n6\nNULL\n\nWarning: Undefined variable $nope in Command line code on line 4\nNULL\n\nWarning: Undefined variable $w in Command line code on line 5\nNULL\n");
}

/// `array_key_exists` refuses an array or object key with the container-offset
/// `TypeError` and words its own null deprecation; `array_flip` and
/// `array_count_values` skip a value that is neither int nor string with a
/// warning; `array_fill_keys` and `array_combine` key a non-int through its string
/// form. A NAN offset reports the narrowing too.
#[test]
fn array_key_functions_take_only_int_and_string() {
    let src = r#"<?php $a = ['' => 1, 1 => 2, 0 => 3];
        foreach ([null, 1.5, NAN, [], new stdClass] as $k) {
            try { var_dump(array_key_exists($k, $a)); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
        }
        var_dump(array_count_values([1.5, "a", true, "1", 1]));
        var_dump(array_flip([1.5, true, "x", 2]));
        var_dump(array_fill_keys([1.5, true, null], 0), array_combine([1.5, true, null], [1, 2, 3]));
        $b = []; $b[NAN] = 1; var_dump($b);
        "#;
    assert_eq!(run(src), "\nDeprecated: Using null as the key parameter for array_key_exists() is deprecated, use an empty string instead in Command line code on line 3\nbool(true)\n\nDeprecated: Implicit conversion from float 1.5 to int loses precision in Command line code on line 3\nbool(true)\n\nWarning: The float NAN is not representable as an int, cast occurred in Command line code on line 3\n\nDeprecated: Implicit conversion from float NAN to int loses precision in Command line code on line 3\nbool(true)\nCannot access offset of type array on array\nCannot access offset of type stdClass on array\n\nWarning: array_count_values(): Can only count string and integer values, entry skipped in Command line code on line 5\n\nWarning: array_count_values(): Can only count string and integer values, entry skipped in Command line code on line 5\narray(2) {\n  [\"a\"]=>\n  int(1)\n  [1]=>\n  int(2)\n}\n\nWarning: array_flip(): Can only flip string and integer values, entry skipped in Command line code on line 6\n\nWarning: array_flip(): Can only flip string and integer values, entry skipped in Command line code on line 6\narray(2) {\n  [\"x\"]=>\n  int(2)\n  [2]=>\n  int(3)\n}\narray(3) {\n  [\"1.5\"]=>\n  int(0)\n  [1]=>\n  int(0)\n  [\"\"]=>\n  int(0)\n}\narray(3) {\n  [\"1.5\"]=>\n  int(1)\n  [1]=>\n  int(2)\n  [\"\"]=>\n  int(3)\n}\n\nWarning: The float NAN is not representable as an int, cast occurred in Command line code on line 8\n\nDeprecated: Implicit conversion from float NAN to int loses precision in Command line code on line 8\narray(1) {\n  [0]=>\n  int(1)\n}\n");
}

/// `%` as a conversion (`%5%`) consumes an argument and writes one `%`, padding
/// ignored; when the argument is missing the scan resumes on that `%`, which opens
/// another spec, so the count the `ArgumentCountError` names is the reference's.
#[test]
fn sprintf_percent_conversion() {
    let src = r#"<?php foreach (['sprintf("%5%", 1)', 'sprintf("%s%5%", 1, 2)', 'sprintf("%5%%s", "x", "y")', 'sprintf("%5%")', 'sprintf("%-%")', 'sprintf("%5% %s", "a")'] as $code) {
            try { var_dump(eval("return $code;")); } catch (ArgumentCountError $e) { echo $e->getMessage(), "\n"; }
        }
        "#;
    assert_eq!(run(src), "string(1) \"%\"\nstring(2) \"1%\"\nstring(2) \"%y\"\n3 arguments are required, 1 given\n3 arguments are required, 1 given\n3 arguments are required, 2 given\n");
}

/// The array form of `strtr` copies unmatched bytes as bytes — it used to re-encode
/// each byte of a multi-byte character (`é` became `Ã©`) — and warns `Ignoring
/// replacement of empty string` for an empty key on a non-empty subject.
#[test]
fn strtr_array_keeps_utf8_and_warns_on_empty_key() {
    let src = r#"<?php var_dump(strtr("héllo", ["l" => "L", "é" => "e"]), strtr("日本語", ["本" => "x"]));
        var_dump(strtr("abc", ["" => "x", "a" => "A"]), strtr("", ["" => "x"]));
        "#;
    assert_eq!(run(src), "string(5) \"heLLo\"\nstring(7) \"日x語\"\n\nWarning: strtr(): Ignoring replacement of empty string in Command line code on line 2\nstring(3) \"Abc\"\nstring(0) \"\"\n");
}

/// A multi-byte pad string fills the width byte by byte, and `chunk_split`'s
/// length counts bytes, as the rest of the string family does.
#[test]
fn str_pad_and_chunk_split_count_bytes() {
    let src = r#"<?php var_dump(str_pad("héllo", 10, "é"), str_pad("a", 5, "é", STR_PAD_BOTH), chunk_split("héllo wörld", 4, "|"));
        "#;
    assert_eq!(
        run(src),
        "string(10) \"hélloéé\"\nstring(5) \"éaé\"\nstring(17) \"hél|lo w|örl|d|\"\n"
    );
}

/// An enum case's `name` and `value` are readonly and an enum takes no dynamic
/// property, from outside or from its own methods. A write used to change the
/// singleton for the rest of the program.
#[test]
fn enum_case_is_immutable() {
    let src = r#"<?php enum S: int { case H = 1; public function f() { $this->value = 2; } }
        enum U { case A; }
        $x = S::H; $u = U::A;
        foreach ([fn() => $x->value = 5, fn() => $x->value++, fn() => $x->value[] = 1, fn() => $u->name = "z", fn() => $u->foo = 1, function () use ($u) { unset($u->name); }, fn() => S::H->f()] as $f) {
            try { $f(); } catch (Error $e) { echo $e->getMessage(), "\n"; }
        }
        var_dump(S::H->value, U::A);
        "#;
    assert_eq!(run(src), "Cannot modify readonly property S::$value\nCannot modify readonly property S::$value\nCannot indirectly modify readonly property S::$value\nCannot modify readonly property U::$name\nCannot create dynamic property U::$foo\nCannot unset readonly property U::$name\nCannot modify readonly property S::$value\nint(1)\nenum(U::A)\n");
}

/// `zend_compile_list_assign`: a destructuring with a `&` target needs a
/// variable or a call as its subject. A literal, an assignment or a ternary is
/// a compile-time fatal, so the `echo` before it never prints. An assignment
/// subject used to run silently.
#[test]
fn list_ref_needs_a_variable_or_call_subject() {
    let fatal = "\nFatal error: Cannot assign reference to non referenceable value in Command \
                 line code on line 1\nStack trace:\n#0 {main}\n";
    for code in [
        "echo 1; [&$x] = [1];",
        "echo 1; list(&$x) = $y = [1];",
        "echo 1; [$a, [&$b]] = true ? [1, [2]] : [];",
    ] {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_php"))
            .arg("-r")
            .arg(code)
            .stderr(std::process::Stdio::null())
            .output()
            .expect("spawn php -r");
        assert_eq!(String::from_utf8_lossy(&out.stdout), fatal, "code: {code}");
        assert_eq!(out.status.code(), Some(255), "code: {code}");
    }
}
