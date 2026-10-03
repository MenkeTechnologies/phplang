//! When a function or type is DECLARED, and what declaring one twice does.
//!
//! The reference enters a top-level function into the function table while it
//! compiles the file, so redeclaring one is a compile-time fatal: the program
//! prints nothing. A declaration inside a block (`if`, a loop, another
//! function's body) is entered only when its statement runs, so until then the
//! name is undefined, and running it twice is a run-time fatal. A type is
//! declared where it stands unless it can be bound early, and a clash is always
//! reported where it stands — naming the type that is ALREADY declared.
//!
//! Every expectation is the verbatim stdout of `php -r` under the reference
//! `php` 8.5.11 (stderr carries only the `log_errors` copy and is dropped).

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

fn fatal(before: &str, msg: &str, trace: &str) -> (String, i32) {
    (
        format!(
            "{before}\nFatal error: {msg} in Command line code on line 1\nStack trace:\n{trace}\n"
        ),
        255,
    )
}

#[test]
fn a_top_level_function_declared_twice_fails_before_anything_runs() {
    assert_eq!(
        run_r("echo 1; function Foo(){} function foo(){}"),
        fatal(
            "",
            "Cannot redeclare function foo() (previously declared in Command line code:1)",
            "#0 {main}"
        )
    );
}

#[test]
fn a_library_function_cannot_be_redeclared_and_has_no_site_to_quote() {
    assert_eq!(
        run_r("function strlen(){}"),
        fatal("", "Cannot redeclare function strlen()", "#0 {main}")
    );
}

#[test]
fn a_namespaced_function_may_share_a_library_or_sibling_name() {
    assert_eq!(
        run_r(r#"namespace A; function strlen($s){ return 0; } echo "ok";"#),
        ("ok".to_string(), 0)
    );
    assert_eq!(
        run_r(
            r#"namespace A { function f(){ return 1; } } namespace B { function f(){ return 2; } } namespace { echo "ok"; }"#
        ),
        ("ok".to_string(), 0)
    );
}

#[test]
fn a_function_in_a_block_is_declared_when_the_block_runs() {
    assert_eq!(
        run_r(
            r#"var_dump(function_exists("g")); if (false) { function g(){} } var_dump(function_exists("g"));"#
        ),
        ("bool(false)\nbool(false)\n".to_string(), 0)
    );
    assert_eq!(
        run_r(r#"echo 1; if (true) { function f(){} } function f(){} echo 2;"#),
        fatal(
            "1",
            "Cannot redeclare function f() (previously declared in Command line code:1)",
            "#0 {main}"
        )
    );
}

#[test]
fn a_polyfill_guarded_by_function_exists_does_not_replace_the_library_function() {
    assert_eq!(
        run_r(
            r#"if (!function_exists("str_contains")) { function str_contains($a,$b){ return "user"; } } var_dump(str_contains("ab","b"));"#
        ),
        ("bool(true)\n".to_string(), 0)
    );
}

#[test]
fn a_nested_function_declared_by_a_second_call_is_a_runtime_redeclaration() {
    assert_eq!(
        run_r("echo 1; function g(){ function h(){} } g(); g();"),
        fatal(
            "1",
            "Cannot redeclare function h() (previously declared in Command line code:1)",
            "#0 Command line code(1): g()\n#1 {main}"
        )
    );
}

#[test]
fn a_type_declared_twice_names_the_one_already_declared() {
    assert_eq!(
        run_r("echo 1; class A{} class a{}"),
        fatal(
            "1",
            "Cannot redeclare class A (previously declared in Command line code:1)",
            "#0 {main}"
        )
    );
    assert_eq!(
        run_r("echo 1; enum E{} if(1){class E{}}"),
        fatal(
            "1",
            "Cannot redeclare enum E (previously declared in Command line code:1)",
            "#0 {main}"
        )
    );
}

#[test]
fn a_built_in_type_cannot_be_redeclared() {
    assert_eq!(
        run_r("echo 1; class Countable {}"),
        fatal("1", "Cannot redeclare interface Countable", "#0 {main}")
    );
    assert_eq!(
        run_r("echo 1; class Exception {}"),
        fatal("1", "Cannot redeclare class Exception", "#0 {main}")
    );
}

#[test]
fn a_type_in_a_block_is_declared_when_the_block_runs() {
    assert_eq!(
        run_r(
            r#"var_dump(class_exists("X")); if (false) { class X{} } var_dump(class_exists("X")); if (true) { class X { function m(){ return 5; } } } var_dump((new X)->m());"#
        ),
        ("bool(false)\nbool(false)\nint(5)\n".to_string(), 0)
    );
}

#[test]
fn early_binding_decides_which_declaration_the_clash_is_reported_at() {
    // `B extends A` cannot bind early (A is not declared yet), so the SECOND
    // `B` is bound first and the first is the one that fails.
    let src = "echo 1;\nclass B extends A {}\nclass A {}\necho 2;\nclass B {}\n";
    let dir = std::env::temp_dir().join(format!("phplang-redecl-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("c.php");
    std::fs::write(&file, format!("<?php\n{src}")).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_php"))
        .arg(&file)
        .stderr(Stdio::null())
        .output()
        .expect("spawn php");
    let path = std::fs::canonicalize(&file).unwrap().display().to_string();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("1\nFatal error: Cannot redeclare class B (previously declared in {path}:6) in {path} on line 3\nStack trace:\n#0 {{main}}\n")
    );
    let _ = std::fs::remove_dir_all(&dir);
}
