//! `$GLOBALS` (PHP 8.1+): element reads and writes reach the global frame from
//! any scope, the array read whole is a copy, and the whole variable cannot be
//! assigned. Also the `??=` evaluation order and `foreach` into a non-variable
//! target, which share the write-through-an-lvalue machinery.
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

#[test]
fn elements_are_read_and_written_from_inside_a_function() {
    assert_eq!(
        run_r(
            r#"function g() { return $GLOBALS["a"]; } $a = 1; echo g(), "\n";
function w() { $GLOBALS["gv"] = 2; $GLOBALS["arr"][] = 3; $GLOBALS["arr"]["k"] = 4;
  $GLOBALS["n"] ??= 5; $GLOBALS["n"] ??= 6; $GLOBALS["a"]++; $GLOBALS["a"] += 10;
  $k = "dyn"; $GLOBALS[$k] = "v"; }
$a = 1; w(); echo $gv, json_encode($arr), $n, $a, $dyn, "\n";"#
        ),
        ("1\n2{\"0\":3,\"k\":4}512v\n".to_string(), 0)
    );
}

#[test]
fn isset_unset_references_and_missing_names() {
    assert_eq!(
        run_r(
            r#"$a = 1; $gv = 1;
function r() { $x = &$GLOBALS["a"]; $x = 9; unset($GLOBALS["gv"]);
  return [isset($GLOBALS["gv"]), isset($GLOBALS["a"]), isset($GLOBALS["a"]["b"]),
          $GLOBALS["nope"] ?? "dflt", array_key_exists("a", $GLOBALS), isset($GLOBALS["GLOBALS"])]; }
echo json_encode(r()), $a, "\n"; var_dump(isset($gv));
function u() { return $GLOBALS["nope"]; } var_dump(u());"#
        ),
        (
            "[false,true,false,\"dflt\",true,false]9\nbool(false)\n\
             \nWarning: Undefined global variable $nope in Command line code on line 6\nNULL\n"
                .to_string(),
            0
        )
    );
}

#[test]
fn the_whole_array_is_a_copy_in_the_references_order() {
    assert_eq!(
        run_r(
            r#"$a = 1; $arr = [];
function c() { $copy = $GLOBALS; $GLOBALS["a"] = 7; return $copy["a"]; } echo c(), $a, "\n";
echo implode(",", array_keys($GLOBALS)), "\n";
echo implode(",", array_keys(get_defined_vars())), "\n";"#
        ),
        (
            "17\nargv,argc,_GET,_POST,_COOKIE,_FILES,_SERVER,a,arr\n\
             argv,argc,_GET,_POST,_COOKIE,_FILES,_SERVER,a,arr\n"
                .to_string(),
            0
        )
    );
}

#[test]
fn assigning_the_whole_variable_is_a_compile_time_fatal() {
    assert_eq!(
        run_r("function g() { $GLOBALS = []; }"),
        (
            "\nFatal error: $GLOBALS can only be modified using the $GLOBALS[$name] = $value \
             syntax in Command line code on line 1\nStack trace:\n#0 {main}\n"
                .to_string(),
            255
        )
    );
}

#[test]
fn coalesce_assign_writes_only_when_null_and_evaluates_its_target_once() {
    assert_eq!(
        run_r(
            r#"class A { public readonly int $x; function __construct() { $this->x = 1; } }
$a = new A; $a->x ??= 5; echo $a->x, "\n";
function f() { echo "f"; return "k"; }
$m = []; $m[f()] ??= 1; $m[f()] ??= 2; echo json_encode($m), "\n";
class M { function __get($n) { echo "get "; return 5; } function __set($n, $v) { echo "set "; }
          function __isset($n) { echo "isset "; return true; } }
$o = new M; $o->p ??= 1; echo "done\n";
$s = new ArrayObject; $s["a"] ??= 1; $s["a"] ??= 2; echo json_encode($s->getArrayCopy()), "\n";"#
        ),
        ("1\nff{\"k\":1}\nisset get done\n{\"a\":1}\n".to_string(), 0)
    );
}

#[test]
fn foreach_assigns_each_element_to_a_property_element_or_static_target() {
    assert_eq!(
        run_r(
            r#"$b = new stdClass; foreach ([1, 2] as $b->k) {} echo $b->k;
foreach ([[1, 2]] as [$b->p, $b->q]) {} echo $b->p, $b->q;
$a = []; foreach (["x" => 1, "y" => 2] as $k => $a["v"]) { echo $k, $a["v"]; }
$o = new stdClass; foreach (["x" => 1, "y" => 2] as $o->key => $o->val) { echo $o->key, $o->val; }
class S { static $v; } foreach ([5, 6] as S::$v) { echo S::$v; }
$o->list = []; foreach ([1, 2, 3] as $o->list[]) {} echo json_encode($o->list), "\n";"#
        ),
        ("212x1y2x1y256[1,2,3]\n".to_string(), 0)
    );
}

#[test]
fn appending_with_coalesce_assign_is_a_compile_time_fatal() {
    assert_eq!(
        run_r("$z = [1]; $z[] ??= 5;"),
        (
            "\nFatal error: Cannot use [] for reading in Command line code on line 1\n\
             Stack trace:\n#0 {main}\n"
                .to_string(),
            255
        )
    );
}
