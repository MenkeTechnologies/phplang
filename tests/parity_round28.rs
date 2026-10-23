//! Round 28 regressions found by differential probing against `php 8.5.11`
//! (`/opt/homebrew/bin/php`, `php -r`). Expected strings are the reference's
//! recorded output; no PHP is needed to run them.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn a_closure_dumps_its_name_file_line_captures_and_parameters() {
    let src = r##"<?php $y = 5; $z = [1]; $f = function ($p, $q = 2, &$r = 3) use ($y, &$z) {}; var_dump($f); print_r($f);"##;
    assert_eq!(
        run(src),
        r##"object(Closure)#1 (5) {
  ["name"]=>
  string(29) "{closure:Command line code:1}"
  ["file"]=>
  string(17) "Command line code"
  ["line"]=>
  int(1)
  ["static"]=>
  array(2) {
    ["y"]=>
    int(5)
    ["z"]=>
    &array(1) {
      [0]=>
      int(1)
    }
  }
  ["parameter"]=>
  array(3) {
    ["$p"]=>
    string(10) "<required>"
    ["$q"]=>
    string(10) "<optional>"
    ["&$r"]=>
    string(10) "<optional>"
  }
}
Closure Object
(
    [name] => {closure:Command line code:1}
    [file] => Command line code
    [line] => 1
    [static] => Array
        (
            [y] => 5
            [z] => Array
                (
                    [0] => 1
                )

        )

    [parameter] => Array
        (
            [$p] => <required>
            [$q] => <optional>
            [&$r] => <optional>
        )

)
"##
    );
}

#[test]
fn a_bound_closure_dumps_its_this() {
    let src = r##"<?php class K { public $v = 1; function m($a, &$b = null) { return function (int $x, ...$rest) use ($a) {}; } static function s() {} } $k = new K; $c = $k->m(1); var_dump($c);"##;
    assert_eq!(
        run(src),
        r##"object(Closure)#2 (6) {
  ["name"]=>
  string(18) "{closure:K::m():1}"
  ["file"]=>
  string(17) "Command line code"
  ["line"]=>
  int(1)
  ["static"]=>
  array(1) {
    ["a"]=>
    int(1)
  }
  ["this"]=>
  object(K)#1 (1) {
    ["v"]=>
    int(1)
  }
  ["parameter"]=>
  array(2) {
    ["$x"]=>
    string(10) "<required>"
    ["$rest"]=>
    string(10) "<optional>"
  }
}
"##
    );
}

#[test]
fn a_first_class_callable_dumps_the_function_it_forwards_to() {
    let src = r##"<?php class K { public $v = 1; function m($a, &$b = null) {} static function s() {} } $l = strlen(...); $k = new K; $m = $k->m(...); $s = K::s(...); $t = sort(...); $p = sprintf(...); var_dump($l, $m, $s, $t, $p);"##;
    assert_eq!(
        run(src),
        r##"object(Closure)#1 (2) {
  ["function"]=>
  string(6) "strlen"
  ["parameter"]=>
  array(1) {
    ["$string"]=>
    string(10) "<required>"
  }
}
object(Closure)#3 (3) {
  ["function"]=>
  string(4) "K::m"
  ["this"]=>
  object(K)#2 (1) {
    ["v"]=>
    int(1)
  }
  ["parameter"]=>
  array(2) {
    ["$a"]=>
    string(10) "<required>"
    ["&$b"]=>
    string(10) "<optional>"
  }
}
object(Closure)#4 (1) {
  ["function"]=>
  string(4) "K::s"
}
object(Closure)#5 (2) {
  ["function"]=>
  string(4) "sort"
  ["parameter"]=>
  array(2) {
    ["&$array"]=>
    string(10) "<required>"
    ["$flags"]=>
    string(10) "<optional>"
  }
}
object(Closure)#6 (2) {
  ["function"]=>
  string(7) "sprintf"
  ["parameter"]=>
  array(2) {
    ["$format"]=>
    string(10) "<required>"
    ["$values"]=>
    string(10) "<optional>"
  }
}
"##
    );
}

#[test]
fn a_closure_exports_and_encodes_as_an_empty_object() {
    let src = r##"<?php var_export([fn() => 1]); echo json_encode(["a" => fn () => 1]), "\n";"##;
    assert_eq!(
        run(src),
        r##"array (
  0 => 
  \Closure::__set_state(array(
  )),
){"a":{}}
"##
    );
}

#[test]
fn a_constant_class_name_drops_its_leading_backslash_a_variable_one_keeps_it() {
    let src = r##"<?php try { new ("\\Nope"); } catch (Error $e) { echo $e->getMessage(), "|"; } $c = "\\Nope"; try { new $c; } catch (Error $e) { echo $e->getMessage(); }"##;
    assert_eq!(
        run(src),
        r##"Class "Nope" not found|Class "\Nope" not found"##
    );
}

#[test]
fn an_unqualified_namespaced_call_site_resolves_once_when_it_first_runs() {
    let src = r##"<?php namespace A; function t() { return strlen("x"); } function u() { return strlen("x"); } echo t(); eval('namespace A; function strlen($s) { return 99; }'); echo t(), u(), "\n";"##;
    assert_eq!(
        run(src),
        r##"1199
"##
    );
}

#[test]
fn call_user_func_is_checked_by_the_caller_frame() {
    let src = r##"<?php try { call_user_func("NoPe"); } catch (TypeError $e) { echo $e->getTraceAsString(), "|"; } try { call_user_func_array("NoPe", []); } catch (TypeError $e) { echo $e->getTraceAsString(), "|"; } try { call_user_func_array("NoPe", [], 3); } catch (ArgumentCountError $e) { echo $e->getTraceAsString(); }"##;
    assert_eq!(
        run(src),
        r##"#0 {main}|#0 {main}|#0 Command line code(1): call_user_func_array('NoPe', Array, 3)
#1 {main}"##
    );
}

#[test]
fn a_namespaced_library_call_is_an_ordinary_framed_call() {
    let src = r##"<?php namespace A; try { call_user_func("NoPe"); } catch (\TypeError $e) { echo $e->getTraceAsString(), "|"; } try { strlen([]); } catch (\TypeError $e) { echo $e->getTraceAsString(), "|"; } var_dump(min(1, NAN));"##;
    assert_eq!(
        run(src),
        r##"#0 Command line code(1): call_user_func('NoPe')
#1 {main}|#0 Command line code(1): strlen(Array)
#1 {main}|float(NAN)
"##
    );
}

#[test]
fn a_class_implementing_nothing_lists_inherited_interfaces_in_reverse() {
    let src = r##"<?php interface S {} interface U {} interface W {} class B implements S, U {} class Q extends B {} class R extends Q {} class T extends B implements W {} echo implode(",", class_implements("Q")), "|", implode(",", class_implements("R")), "|", implode(",", class_implements("T")), "|", implode(",", (new ReflectionClass("Q"))->getInterfaceNames());"##;
    assert_eq!(run(src), r##"U,S|S,U|S,U,W|U,S"##);
}

#[test]
fn a_variable_static_method_name_calls_the_method() {
    let src = r##"<?php class A { static function s($x) { return "s$x"; } protected static function p() { return "p"; } static function q() { $m = "p"; return static::$m() . self::$m(); } } class B extends A { static $v = "s"; } $m = "s"; $c = "A"; echo A::$m(1), $c::$m(2), B::q(), (new B)::$m(3);"##;
    assert_eq!(run(src), r##"s1s2pps3"##);
}
