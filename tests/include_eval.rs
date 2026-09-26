//! `include`, `require`, their `_once` forms, and `eval()`.
//!
//! Each test lays a small multi-file program out in a fresh directory, runs
//! the crate's binary on it from there, and compares stdout with the
//! reference's. Every expectation is the verbatim output of `php 8.5.11 -d
//! log_errors=0` on the same files, with the directory they were written to
//! replaced by `D` (the files print it through `__DIR__` and diagnostics).

use std::path::PathBuf;
use std::process::Command;

/// The program every test lays out: `(relative path, contents)`.
const FILES: &[(&str, &str)] = &[
    (
        "sub/a.php",
        r#"<?php
$y = $x * 2;
function af($n) { return $n + 1; }
function af2() { throw new Error("boom in " . basename(__FILE__) . " dir " . basename(__DIR__) . " line " . __LINE__); }
return "ret-a";
"#,
    ),
    (
        "sub/b.php",
        r#"<?php
echo "in b\n";
"#,
    ),
    (
        "sub/bad.php",
        r#"<?php
echo "x"
echo "y";
"#,
    ),
    (
        "sub/c.php",
        r#"<?php
return $local * 2;
"#,
    ),
    (
        "sub/loop.php",
        r#"<?php
$sum = 0;
foreach ([10, 20] as $v) { $sum += $v; }
try { if ($i == 2) throw new Exception("x"); } catch (Exception $e) { $sum = -1; }
return $sum;
"#,
    ),
    (
        "sub/nested1.php",
        r#"<?php
$fromNested = include 'nested2.php';
"#,
    ),
    (
        "sub/nested2.php",
        r#"<?php
return basename(__DIR__) . "/" . basename(__FILE__) . ":" . __LINE__;
"#,
    ),
    (
        "sub/ns.php",
        r#"<?php
class K2 { function hello() { return __CLASS__ . " " . __FUNCTION__ . " " . basename(__FILE__); } }
function helper() { return __NAMESPACE__ . " " . __FUNCTION__; }
"#,
    ),
    (
        "sub/static.php",
        r#"<?php
static $n = 0;
return ++$n;
"#,
    ),
    (
        "sub/thrower.php",
        r#"<?php
function thr() {
    throw new RuntimeException("thrown");
}
thr();
"#,
    ),
    (
        "sub/warn.php",
        r#"<?php

echo $undefined_here;
$f = function() { return 1 + []; };
try { $f(); } catch (TypeError $e) { echo $e->getMessage(), "|", basename($e->getFile()), ":", $e->getLine(), "\n", $e->getTraceAsString(), "\n"; }
"#,
    ),
    (
        "m2.php",
        r#"<?php
include "sub/ns.php";
foreach ([1, 2] as $i) {
    $r = include 'sub/loop.php';
    echo "outer i=$i r=$r\n";
}
function counter() { return include __DIR__ . '/sub/static.php'; }
echo counter(), counter(), "\n";
$k = new K2(); echo $k->hello(), "\n";
echo helper(), "\n";
try { include 'sub/thrower.php'; } catch (RuntimeException $e) { echo $e->getMessage(), " ", basename($e->getFile()), ":", $e->getLine(), "\n", str_replace(__DIR__, "D", $e->getTraceAsString()), "\n"; }
function inner() { require 'sub/nested1.php'; return $fromNested; }
var_dump(inner());
var_dump(eval('return __LINE__ + 40;'));
$z = 3; eval('$z *= 2; $w = "set";'); var_dump($z, $w);
try { eval('throw new LogicException("ev");'); } catch (LogicException $e) { echo str_replace(__DIR__, "D", $e->getFile() . ":" . $e->getLine() . "\n" . $e->getTraceAsString()), "\n"; }
var_dump(eval('echo "no return\n";'));
$f = eval('return fn($x) => $x * $z;'); var_dump($f(5));
try { eval('$x = ;'); } catch (ParseError $e) { var_dump($e->getMessage(), str_replace(__DIR__, "D", $e->getFile()), $e->getLine()); }
var_dump(count(get_included_files()));
"#,
    ),
    (
        "m3.php",
        r#"<?php
$x = 10;
$r = include 'sub/a.php';
var_dump($r, $y, af(3), __FILE__ === realpath('m3.php'));
var_dump(include_once 'sub/a.php', include_once __FILE__);
var_dump(require 'sub/b.php');
var_dump((include 'sub/b.php') + 1);
function g() { $local = 5; return include 'sub/c.php'; }
var_dump(g());
var_dump(@include 'nope.php');
var_dump(array_map('basename', get_included_files()));
try { include 'sub/bad.php'; } catch (ParseError $e) { echo get_class($e), " @ ", basename($e->getFile()), ":", $e->getLine(), "\n"; }
try { af2(); } catch (Error $e) { echo $e->getMessage(), "\n", str_replace(__DIR__, "D", $e->getTraceAsString()), "\n"; }
ob_start(); include 'sub/warn.php'; echo str_replace(__DIR__, "D", ob_get_clean());
try { require 'nope2.php'; } catch (Error $e) { echo get_class($e), "\n"; }
echo "end\n";
"#,
    ),
];

/// Write [`FILES`] into a fresh directory and return its resolved path.
fn lay_out(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("phplang_include_{}_{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).expect("mkdir");
    for (path, body) in FILES {
        std::fs::write(dir.join(path), body).expect("write");
    }
    std::fs::canonicalize(&dir).expect("canonicalize")
}

/// Run `script` from inside the laid-out directory; stdout with the directory
/// replaced by `D`.
fn run(tag: &str, script: &str) -> String {
    let dir = lay_out(tag);
    let out = Command::new(env!("CARGO_BIN_EXE_php"))
        .args(["-d", "log_errors=0", script])
        .current_dir(&dir)
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .output()
        .expect("spawn php");
    let _ = std::fs::remove_dir_all(&dir);
    let text = String::from_utf8_lossy(&out.stdout).replace(&dir.display().to_string(), "D");
    elide_include_args(&text)
}

/// A trace shows an include frame's argument as the first 15 bytes of the
/// resolved path, which depends on where the files were written; `…` stands
/// in for it on both sides.
fn elide_include_args(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("include('") {
        out.push_str(&rest[..i + 9]);
        rest = &rest[i + 9..];
        if let Some(j) = rest.find("')") {
            out.push('…');
            rest = &rest[j..];
        }
    }
    out.push_str(rest);
    out
}

#[test]
fn temporaries_statics_classes_traces_and_eval() {
    let expected = r#"outer i=1 r=30
outer i=2 r=-1
11
K2 hello ns.php
 helper
thrown thrower.php:3
#0 D/sub/thrower.php(5): thr()
#1 D/m2.php(11): include('…')
#2 {main}
string(17) "sub/nested2.php:2"
int(41)
int(6)
string(3) "set"
D/m2.php(16) : eval()'d code:1
#0 D/m2.php(16): eval()
#1 {main}
no return
NULL
int(30)
string(34) "syntax error, unexpected token ";""
string(28) "D/m2.php(19) : eval()'d code"
int(1)
int(7)
"#;
    assert_eq!(run("m2", "m2.php"), expected);
}

#[test]
fn include_forms_scope_return_values_and_failures() {
    let expected = r#"string(5) "ret-a"
int(20)
int(4)
bool(true)
bool(true)
bool(true)
in b
int(1)
in b
int(2)
int(10)
bool(false)
array(4) {
  [0]=>
  string(6) "m3.php"
  [1]=>
  string(5) "a.php"
  [2]=>
  string(5) "b.php"
  [3]=>
  string(5) "c.php"
}
ParseError @ bad.php:3
boom in a.php dir sub line 4
#0 D/m3.php(13): af2()
#1 {main}

Warning: Undefined variable $undefined_here in D/sub/warn.php on line 3
Unsupported operand types: int + array|warn.php:4
#0 D/sub/warn.php(5): {closure:D/sub/warn.php:4}()
#1 D/m3.php(14): include('…')
#2 {main}

Warning: require(nope2.php): Failed to open stream: No such file or directory in D/m3.php on line 15
Error
end
"#;
    assert_eq!(run("m3", "m3.php"), expected);
}

/// `php -r code`: `(stdout, exit status)`.
fn run_code(code: &str) -> (String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_php"))
        .args(["-d", "log_errors=0", "-r", code])
        .output()
        .expect("spawn php");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn an_uncaught_parse_error_in_eval_is_a_parse_error_not_an_uncaught_exception() {
    let (out, status) = run_code(r#"echo "a\n"; eval("\$x = ;"); echo "b\n";"#);
    assert_eq!(
        out,
        "a\n\nParse error: syntax error, unexpected token \";\" in Command line code(1) : \
         eval()'d code on line 1\n"
    );
    assert_eq!(status, 255);
}

#[test]
fn eval_is_a_frame_of_its_own_in_a_trace() {
    let (out, _) = run_code(
        r#"function f() { eval("g();"); } function g() { throw new Exception("e"); } try { f(); } catch (Exception $e) { echo $e->getTraceAsString(), "\n"; }"#,
    );
    assert_eq!(
        out,
        "#0 Command line code(1) : eval()'d code(1): g()\n\
         #1 Command line code(1): eval()\n\
         #2 Command line code(1): f()\n\
         #3 {main}\n"
    );
}
