//! Round 34 regressions found by differential probing against `php 8.5.12`
//! (`/opt/homebrew/bin/php`, `php -r`). Expected strings are the reference's
//! recorded output; no PHP is needed to run them.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn every_use_of_a_deprecated_predefined_constant_is_reported() {
    let src = r##"<?php namespace A;
echo E_STRICT, \E_STRICT, constant("E_STRICT"), "\n";
var_dump(defined("E_STRICT"), @DATE_RFC7231);
echo date(DATE_RFC7231, 0), "\n";
echo SUNFUNCS_RET_STRING, "\n";"##;
    assert_eq!(
        run(src),
        r##"
Deprecated: Constant E_STRICT is deprecated since 8.4, the error level was removed in Command line code on line 2
2048
Deprecated: Constant E_STRICT is deprecated since 8.4, the error level was removed in Command line code on line 2
2048
Deprecated: Constant E_STRICT is deprecated since 8.4, the error level was removed in Command line code on line 2
2048
bool(true)
string(21) "D, d M Y H:i:s \G\M\T"

Deprecated: Constant DATE_RFC7231 is deprecated since 8.5, as this format ignores the associated timezone and always uses GMT in Command line code on line 4
Thu, 01 Jan 1970 00:00:00 GMT

Deprecated: Constant SUNFUNCS_RET_STRING is deprecated since 8.4, as date_sunrise() and date_sunset() were deprecated in 8.1 in Command line code on line 5
1
"##
    );
}

#[test]
fn a_constant_deprecation_reaches_the_handler_and_respects_the_mask() {
    let src = r##"<?php set_error_handler(function ($no, $str) { echo "[$no] $str\n"; return true; });
echo FILTER_SANITIZE_STRIPPED, "\n";
error_reporting(E_ALL & ~E_DEPRECATED);
restore_error_handler();
echo FILTER_SANITIZE_STRING, "\n";
set_error_handler(function ($no, $str) { throw new \ErrorException($str, 0, $no); });
try { echo E_STRICT; } catch (ErrorException $e) { echo get_class($e), " ", $e->getSeverity(), " ", $e->getLine(), "\n"; }"##;
    assert_eq!(
        run(src),
        r##"[8192] Constant FILTER_SANITIZE_STRIPPED is deprecated since 8.1, use htmlspecialchars() instead
513
513
ErrorException 8192 6
"##
    );
}

#[test]
fn deprecated_class_constants_and_enum_cases_name_the_declaring_class() {
    let src = r##"<?php class K { #[\Deprecated("use Y", since: "2.0")] const X = 1; const Y = 2; }
class L extends K {}
interface I { #[\Deprecated] const C = 3; }
class Q implements I {}
enum E: int { #[\Deprecated("gone")] case A = 1; case B = 2; }
echo L::X, K::Y, Q::C, constant("K::X"), "\n";
var_dump(E::A, count(E::cases()), E::from(1) === E::tryFrom(1));
echo DateTime::RFC7231, "\n";
echo (new DateTime("@0"))->format(DateTimeInterface::RFC2822), "\n";
set_error_handler(function ($no, $str) { echo "[$no] $str\n"; return true; });
echo K::X, DateTimeInterface::RFC7231, "\n";"##;
    assert_eq!(
        run(src),
        r##"
Deprecated: Constant K::X is deprecated since 2.0, use Y in Command line code on line 6
12
Deprecated: Constant I::C is deprecated in Command line code on line 6
3
Deprecated: Constant K::X is deprecated since 2.0, use Y in Command line code on line 6
1

Deprecated: Enum case E::A is deprecated, gone in Command line code on line 7
enum(E::A)
int(2)
bool(true)

Deprecated: Constant DateTimeInterface::RFC7231 is deprecated since 8.5, as this format ignores the associated timezone and always uses GMT in Command line code on line 8
D, d M Y H:i:s \G\M\T
Thu, 01 Jan 1970 00:00:00 +0000
[16384] Constant K::X is deprecated since 2.0, use Y
1[8192] Constant DateTimeInterface::RFC7231 is deprecated since 8.5, as this format ignores the associated timezone and always uses GMT
D, d M Y H:i:s \G\M\T
"##
    );
}

#[test]
fn php_sapi_is_the_cli_sapi() {
    assert_eq!(
        run(r#"<?php var_dump(PHP_SAPI, PHP_SAPI === php_sapi_name());"#),
        "string(3) \"cli\"\nbool(true)\n"
    );
}

/// `PHP_BINARY` is the `realpath` of `argv[0]` — or, for a name with no `/`,
/// of the first executable regular file of that name along `PATH`.
#[test]
fn php_binary_is_the_resolved_interpreter_path() {
    use std::path::Path;
    use std::process::Command;

    let exe = Path::new(env!("CARGO_BIN_EXE_php"));
    let real = std::fs::canonicalize(exe).unwrap();
    let real = real.to_str().unwrap();

    let by_path = Command::new(exe).args(["-r", "echo PHP_BINARY;"]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&by_path.stdout), real);

    // Spawned by bare name, so `argv[0]` is `php` and the `PATH` search runs.
    let dir = exe.parent().unwrap();
    let by_name = Command::new("php")
        .env("PATH", format!("/nonexistent-dir::{}", dir.display()))
        .current_dir(std::env::temp_dir())
        .args(["-r", "echo PHP_BINARY;"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&by_name.stdout), real);
}

#[test]
fn inspecting_a_deprecated_constant_is_silent_and_reflection_ignores_visibility() {
    let src = r##"<?php class K { #[\Deprecated] const X = 1; private const P = 2; } enum E { #[\Deprecated] case A; } var_dump(defined("K::X"), defined("K::P"), (new ReflectionClassConstant("K", "X"))->getValue(), (new ReflectionClassConstant("K", "P"))->getValue(), (new ReflectionClass("K"))->getConstants(), unserialize(serialize(E::A)) === E::A);"##;
    assert_eq!(
        run(src),
        r##"
Deprecated: Enum case E::A is deprecated in Command line code on line 1

Deprecated: Enum case E::A is deprecated in Command line code on line 1
bool(true)
bool(false)
int(1)
int(2)
array(2) {
  ["X"]=>
  int(1)
  ["P"]=>
  int(2)
}
bool(true)
"##
    );
}
