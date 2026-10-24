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
