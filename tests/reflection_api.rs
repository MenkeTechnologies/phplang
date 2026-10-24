//! The Reflection API (`ReflectionClass`, `ReflectionMethod`, `ReflectionProperty`,
//! `ReflectionFunction`, `ReflectionParameter`, the types), measured against `php 8.5.11`
//! (`/opt/homebrew/bin/php`, `php -r`). Expected strings are the reference's
//! recorded output; no PHP is needed to run them.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn a_reflection_class_describes_a_class() {
    let src = r##"<?php namespace App; interface Shape {} trait Named {} abstract class Base implements Shape {} final class Square extends Base { use Named; } $rc = new \ReflectionClass(Square::class); var_dump($rc); echo $rc->getName(), "|", $rc->getShortName(), "|", $rc->getNamespaceName(), "|", $rc->getParentClass()->getName(), "\n"; var_dump($rc->isFinal(), $rc->isAbstract(), $rc->isInterface(), $rc->isInstantiable(), $rc->isUserDefined(), $rc->getParentClass()->getParentClass(), (new \ReflectionClass(Base::class))->isInstantiable(), (new \ReflectionClass('Exception'))->isInternal()); print_r($rc->getInterfaceNames()); print_r($rc->getTraitNames()); var_dump($rc->implementsInterface(Shape::class), $rc->isSubclassOf(Base::class), $rc->isSubclassOf(Square::class));"##;
    assert_eq!(
        run(src),
        r##"object(ReflectionClass)#1 (1) {
  ["name"]=>
  string(10) "App\Square"
}
App\Square|Square|App|App\Base
bool(true)
bool(false)
bool(false)
bool(true)
bool(true)
bool(false)
bool(false)
bool(true)
Array
(
    [0] => App\Shape
)
Array
(
    [0] => App\Named
)
bool(true)
bool(true)
bool(false)
"##
    );
}

#[test]
fn methods_and_properties_are_listed_in_the_reference_order() {
    let src = r##"<?php trait Named { public string $label = "n"; function name() {} } abstract class Base { protected int $id = 7; public static $count = 0; private $secret = "s"; function __construct(public readonly int $w = 1, protected ?string $tag = null) {} abstract function area(): float; private function hidden() {} final static function make() {} } final class Square extends Base { use Named; public $extra; function area(): float { return 4.0; } } $rc = new ReflectionClass('Square'); foreach ($rc->getMethods() as $m) { echo $m->class, "::", $m->getName(), " ", implode(" ", Reflection::getModifierNames($m->getModifiers())), "\n"; } foreach ($rc->getProperties() as $p) { echo $p->class, "::$", $p->getName(), " ", implode(" ", Reflection::getModifierNames($p->getModifiers())), " ", $p->hasType() ? $p->getType() : "-", "\n"; } echo count($rc->getMethods(ReflectionMethod::IS_STATIC)), count($rc->getProperties(ReflectionProperty::IS_PUBLIC)), "\n"; var_dump($rc->hasMethod('HIDDEN'), (new ReflectionClass('Base'))->hasMethod('hidden'), $rc->hasProperty('secret'));"##;
    assert_eq!(
        run(src),
        r##"Square::area public
Square::name public
Base::__construct public
Base::make final public static
Square::$extra public -
Square::$label public string
Base::$id protected int
Base::$count public static -
Base::$w public protected(set) readonly int
Base::$tag protected ?string
14
bool(true)
bool(true)
bool(false)
"##
    );
}

#[test]
fn constants_and_enum_cases() {
    let src = r##"<?php interface I { const A = 1; } class B implements I { const K = "b"; } class C extends B { const K = "c"; const D = 2; } enum Suit: string { case H = 'h'; case S = 's'; const X = 1; } print_r((new ReflectionClass('C'))->getConstants()); var_dump((new ReflectionClass('Suit'))->getConstants(), (new ReflectionClass('C'))->hasConstant('A'), (new ReflectionClass('C'))->getConstant('K'), (new ReflectionClass('C'))->getConstant('NOPE')); $e = new ReflectionEnum('Suit'); var_dump($e->isBacked(), count($e->getCases()), $e->hasCase('S')); echo implode(",", array_map(fn($m) => $m->name, $e->getMethods())), "\n";"##;
    assert_eq!(
        run(src),
        r##"Array
(
    [K] => c
    [D] => 2
    [A] => 1
)

Deprecated: ReflectionClass::getConstant() for a non-existent constant is deprecated, use ReflectionClass::hasConstant() to check if the constant exists in Command line code on line 1
array(3) {
  ["H"]=>
  enum(Suit::H)
  ["S"]=>
  enum(Suit::S)
  ["X"]=>
  int(1)
}
bool(true)
string(1) "c"
bool(false)
bool(true)
int(2)
bool(true)
cases,from,tryFrom
"##
    );
}

#[test]
fn instances_are_built_with_or_without_the_constructor() {
    let src = r##"<?php class P { public $a = 1; public function __construct(public int $w = 0, public string $t = "") { echo "[ctor]"; } } $r = new ReflectionClass('P'); $o = $r->newInstance(3, "x"); echo $o->w, $o->t, "\n"; $o = $r->newInstanceArgs(['t' => 'y', 'w' => 4]); echo $o->w, $o->t, "\n"; $o = $r->newInstanceWithoutConstructor(); echo $o->a, isset($o->w) ? "set" : "unset", "\n"; var_dump($r->isInstance($o), $r->getConstructor()->getName());"##;
    assert_eq!(
        run(src),
        r##"[ctor]3x
[ctor]4y
1unset
bool(true)
string(11) "__construct"
"##
    );
}

#[test]
fn a_reflection_method_reads_parameters_and_invokes() {
    let src = r##"<?php class Base { private function hidden($x) { return "hidden:$x"; } static function make(int|string $a, &$b = [], ...$rest): static { return new static(); } protected static function st($v = PHP_INT_SIZE) { return "st:$v"; } } class Child extends Base {} $m = new ReflectionMethod('Base', 'make'); var_dump($m); echo $m->getNumberOfParameters(), $m->getNumberOfRequiredParameters(), " ", $m->getReturnType(), " ", var_export($m->isStatic(), true), "\n"; foreach ($m->getParameters() as $p) { echo $p->getPosition(), " ", $p->getName(), " opt=", var_export($p->isOptional(), true), " var=", var_export($p->isVariadic(), true), " ref=", var_export($p->isPassedByReference(), true), " type=", $p->hasType() ? $p->getType() : "-", " def=", $p->isDefaultValueAvailable() ? var_export($p->getDefaultValue(), true) : "-", "\n"; } echo (new ReflectionMethod(new Child, 'hidden'))->invoke(new Child, 5), "|", (new ReflectionMethod('Base', 'st'))->invoke(null), "|", (new ReflectionMethod('Base', 'st'))->invokeArgs(null, [9]), "|", ReflectionMethod::createFromMethodName('Base::st')->getName(), "\n"; $h = new ReflectionMethod('Base', 'hidden'); $h->setAccessible(true); new ReflectionMethod('Base::st');"##;
    assert_eq!(
        run(src),
        r##"object(ReflectionMethod)#1 (2) {
  ["name"]=>
  string(4) "make"
  ["class"]=>
  string(4) "Base"
}
31 static true
0 a opt=false var=false ref=false type=string|int def=-
1 b opt=true var=false ref=true type=- def=array (
)
2 rest opt=true var=true ref=false type=- def=-
hidden:5|st:8|st:9|st

Deprecated: Method ReflectionMethod::setAccessible() is deprecated since 8.5, as it has no effect since PHP 8.1 in Command line code on line 1

Deprecated: Calling ReflectionMethod::__construct() with 1 argument is deprecated, use ReflectionMethod::createFromMethodName() instead in Command line code on line 1
"##
    );
}

#[test]
fn a_reflection_property_reads_and_writes_across_visibility() {
    let src = r##"<?php class P { public int $typed; public $plain; public ?array $n = null; public static int $s = 3; private string $p = "x"; } $o = (new ReflectionClass('P'))->newInstanceWithoutConstructor(); $p = new ReflectionProperty('P', 'p'); echo $p->getValue($o); $p->setValue($o, "y"); echo $p->getValue($o), "\n"; $t = new ReflectionProperty('P', 'typed'); var_dump($t->isInitialized($o), $t->hasDefaultValue(), (new ReflectionProperty('P', 'plain'))->hasDefaultValue(), (new ReflectionProperty('P', 'n'))->getDefaultValue()); $t->setValue($o, 5); var_dump($t->isInitialized($o), $t->getValue($o)); $st = new ReflectionProperty('P', 's'); $st->setValue(null, 9); echo P::$s, $st->getValue(), "\n"; print_r((new ReflectionClass('P'))->getDefaultProperties()); print_r((new ReflectionClass('P'))->getStaticProperties()); $st->setValue(4); $x = new stdClass; $x->dyn = 1; var_dump((new ReflectionObject($x))->hasProperty('dyn'), (new ReflectionClass('stdClass'))->hasProperty('dyn'));"##;
    assert_eq!(
        run(src),
        r##"xy
bool(false)
bool(false)
bool(true)
NULL
bool(true)
int(5)
99
Array
(
    [s] => 3
    [plain] => 
    [n] => 
    [p] => x
)
Array
(
    [s] => 9
)

Deprecated: Calling ReflectionProperty::setValue() with a single argument is deprecated in Command line code on line 1
bool(true)
bool(false)
"##
    );
}

#[test]
fn declared_types_become_reflection_types() {
    let src = r##"<?php interface I { function f(?self $a): ?self; } class K implements I { function f(?I $a): ?self { return null; } } $t = (new ReflectionMethod('I', 'f'))->getReturnType(); echo get_class($t), " ", $t, " ", $t->getName(), " ", var_export($t->allowsNull(), true), " ", var_export($t->isBuiltin(), true), "\n"; $u = (new ReflectionFunction(function (int|string|null $x, array|bool $y = [], mixed $z = PHP_INT_MAX, $w = null) {}))->getParameters(); echo get_class($u[0]->getType()), " ", $u[0]->getType(), " ", implode(",", array_map('strval', $u[0]->getType()->getTypes())), " ", $u[1]->getType(), "\n"; var_dump($u[2]->getDefaultValue(), $u[1]->getDefaultValue(), $u[0]->allowsNull(), $u[2]->allowsNull(), $u[3]->hasType(), $u[3]->allowsNull(), (string) (new ReflectionMethod('K', 'f'))->getParameters()[0]->getType());"##;
    assert_eq!(
        run(src),
        r##"ReflectionNamedType ?I I true false
ReflectionUnionType string|int|null string,int,null array|bool
int(9223372036854775807)
array(0) {
}
bool(true)
bool(true)
bool(false)
bool(true)
string(2) "?I"
"##
    );
}

#[test]
fn a_reflection_function_wraps_named_and_anonymous_functions() {
    let src = r##"<?php function add(int $a, int $b = 2): int { return $a + $b; } $f = new ReflectionFunction('ADD'); var_dump($f->getName(), $f->isInternal(), $f->getNumberOfParameters(), $f->getNumberOfRequiredParameters(), (string) $f->getReturnType(), $f->invoke(1), $f->invokeArgs([1, 5])); $s = new ReflectionFunction('str_replace'); foreach ($s->getParameters() as $p) { echo $p->getName(), $p->isPassedByReference() ? "&" : "", $p->isOptional() ? "?" : "", " "; } echo $s->getNumberOfRequiredParameters(), " ", var_export($s->isInternal(), true), "\n"; var_dump((new ReflectionFunction('array_slice'))->getParameters()[3]->getDefaultValue()); $cl = function (int $a, $b = 2): ?string { return "c$a"; }; $rf = new ReflectionFunction($cl); var_dump($rf->getNumberOfRequiredParameters(), (string) $rf->getReturnType(), $rf->isClosure(), $rf->invoke(7));"##;
    assert_eq!(
        run(src),
        r##"string(3) "add"
bool(false)
int(2)
int(1)
string(3) "int"
int(3)
int(6)
search replace subject count&? 3 true
bool(false)
int(1)
string(7) "?string"
bool(true)
string(2) "c7"
"##
    );
}

#[test]
fn missing_names_raise_reflection_exceptions() {
    let src = r##"<?php foreach ([fn() => new ReflectionClass('Nope'), fn() => new ReflectionMethod('stdClass', 'nope'), fn() => new ReflectionProperty('stdClass', 'nope'), fn() => new ReflectionFunction('nope'), fn() => (new ReflectionClass('stdClass'))->implementsInterface('stdClass'), fn() => new ReflectionEnum('stdClass')] as $f) { try { $f(); } catch (ReflectionException $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; } }"##;
    assert_eq!(
        run(src),
        r##"ReflectionException: Class "Nope" does not exist
ReflectionException: Method stdClass::nope() does not exist
ReflectionException: Property stdClass::$nope does not exist
ReflectionException: Function nope() does not exist
ReflectionException: stdClass is not an interface
ReflectionException: Class "stdClass" is not an enum
"##
    );
}

#[test]
fn a_library_functions_return_type_is_the_references() {
    let src = r##"<?php foreach (["str_replace", "array_search", "sort", "var_export", "json_encode", "array_key_first", "hexdec", "strlen", "print_r"] as $f) { $t = (new ReflectionFunction($f))->getReturnType(); echo $f, " ", $t, " ", get_class($t), " ", var_export($t->allowsNull(), true), "\n"; }"##;
    assert_eq!(
        run(src),
        r##"str_replace array|string ReflectionUnionType false
array_search string|int|false ReflectionUnionType false
sort true ReflectionNamedType false
var_export ?string ReflectionNamedType true
json_encode string|false ReflectionUnionType false
array_key_first string|int|null ReflectionUnionType true
hexdec int|float ReflectionUnionType false
strlen int ReflectionNamedType false
print_r string|true ReflectionUnionType false
"##
    );
}
