//! End-to-end inline Rust FFI: a `rust { ... }` block is desugared, compiled to
//! a cdylib via `rustc`, dlopened, and its exports called from PHP. Requires
//! `rustc` on PATH (always present in a Rust CI); skips cleanly otherwise so a
//! toolchain-less environment never reports a false failure.

use phplang::eval_capture;

fn rustc_available() -> bool {
    std::process::Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn rust_block_exports_are_callable_across_all_v1_signatures() {
    if !rustc_available() {
        eprintln!("skipping FFI test: rustc not on PATH");
        return;
    }
    // Distinct names so this test's registry entries never collide with another
    // test's. Exercises int-arity, float-arity, and string→int marshalling.
    let src = r#"<?php
rust {
    pub extern "C" fn ffi_addi(a: i64, b: i64) -> i64 { a + b }
    pub extern "C" fn ffi_mulf(x: f64, y: f64, z: f64) -> f64 { x * y * z }
    pub extern "C" fn ffi_slen(s: *const c_char) -> i64 {
        unsafe { CStr::from_ptr(s).to_bytes().len() as i64 }
    }
}
echo ffi_addi(21, 21), "|";
echo ffi_mulf(1.5, 2.0, 3.0), "|";
echo ffi_slen("hello world"), "\n";
"#;
    let out = eval_capture(src).expect("FFI program should run");
    assert_eq!(out, "42|9|11\n");
}

#[test]
fn rust_block_with_no_exports_errors() {
    if !rustc_available() {
        return;
    }
    // A block with no `pub extern "C" fn` is a hard error — v1 requires at least
    // one exported function.
    let src = "<?php\nrust { fn helper() -> i64 { 1 } }\necho 1;\n";
    let err = eval_capture(src).expect_err("empty-export block must error");
    assert!(err.contains("rust FFI"), "unexpected error: {err}");
}
