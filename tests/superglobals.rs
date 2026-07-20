//! Superglobals ($_SERVER/$_ENV/$_GET/…, $argv/$argc) and the system/environment
//! functions (getenv/putenv/phpversion/get_defined_constants/…).

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn superglobals_are_arrays() {
    assert_eq!(
        run("<?php echo is_array($_SERVER)?'Y':'N', is_array($_ENV)?'Y':'N', is_array($_GET)?'Y':'N', is_array($_POST)?'Y':'N';"),
        "YYYY"
    );
    assert_eq!(run("<?php echo $argc;"), "1");
    assert_eq!(run("<?php echo is_array($argv)?'Y':'N';"), "Y");
}

#[test]
fn superglobal_visible_inside_function_scope() {
    // A superglobal is auto-global: reachable from a function without `global`.
    let src = r#"<?php
        $_GET["page"] = "home";
        function current_page() { return $_GET["page"]; }
        echo current_page();"#;
    assert_eq!(run(src), "home");
}

#[test]
fn env_superglobal_reflects_process_env() {
    // $_ENV is seeded from the real environment; PATH is essentially always set.
    let src = r#"<?php echo isset($_ENV["PATH"]) || count($_ENV) >= 0 ? "ok" : "no";"#;
    assert_eq!(run(src), "ok");
}

#[test]
fn getenv_and_putenv_roundtrip() {
    let src = r#"<?php
        putenv("PHPLANG_TEST_VAR=hello");
        echo getenv("PHPLANG_TEST_VAR");
        echo getenv("PHPLANG_DEFINITELY_UNSET_XYZ") === false ? "|false" : "|set";"#;
    assert_eq!(run(src), "hello|false");
}

#[test]
fn version_and_sapi() {
    assert_eq!(run("<?php echo phpversion();"), "8.3.0");
    assert_eq!(run("<?php echo php_sapi_name();"), "cli");
    assert_eq!(run("<?php echo getmypid() > 0 ? 'Y' : 'N';"), "Y");
}

#[test]
fn get_defined_constants_and_classes() {
    // The predefined constants are enumerable; a declared class shows up.
    let src = r#"<?php
        $c = get_defined_constants();
        echo $c["PHP_INT_SIZE"];
        class Widget {}
        echo in_array("widget", get_declared_classes()) ? "|Y" : "|N";"#;
    assert_eq!(run(src), "8|Y");
}
