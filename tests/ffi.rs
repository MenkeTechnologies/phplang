//! End-to-end inline Rust FFI: a `rust { ... }` block is desugared, compiled to
//! a cdylib via `rustc`, dlopened, and its exports called from PHP.
//!
//! These tests do NOT skip. `cargo test` cannot have started without a Rust
//! toolchain, so `rustc` is present by construction; if the probe cannot find
//! it, that is a broken environment and the right answer is a loud failure, not
//! a silent pass. The previous `if !rustc_available() { return; }` guard made
//! both tests report PASS while executing zero assertions — and because the
//! probe honoured `$RUSTC`, `RUSTC=/nonexistent cargo test` was enough to turn
//! the only two end-to-end FFI tests in the suite into no-ops.

use phplang::eval_capture;

/// The `rustc` these tests drive, and proof that it runs. Panics rather than
/// returning a bool: there is no environment in which `cargo test` runs and this
/// is legitimately absent.
fn rustc_or_panic() -> String {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let out = std::process::Command::new(&rustc)
        .arg("--version")
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "cannot run `{rustc}` (from $RUSTC or PATH): {e}. \
                 The FFI tests compile real cdylibs and cannot be checked without it."
            )
        });
    assert!(
        out.status.success(),
        "`{rustc} --version` exited {:?}: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(
        version.starts_with("rustc "),
        "`{rustc} --version` did not identify itself as rustc: {version:?}"
    );
    version
}

#[test]
fn rust_block_exports_are_callable_across_all_v1_signatures() {
    rustc_or_panic();
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
    rustc_or_panic();
    // A block with no `pub extern "C" fn` is a hard error — v1 requires at least
    // one exported function.
    let src = "<?php\nrust { fn helper() -> i64 { 1 } }\necho 1;\n";
    let err = eval_capture(src).expect_err("empty-export block must error");
    assert!(err.contains("rust FFI"), "unexpected error: {err}");
}

/// The probe itself, asserted directly. Without this, a future change that made
/// `rustc_or_panic` return early would silently disarm both tests above and
/// nothing would notice.
#[test]
fn the_ffi_toolchain_probe_finds_a_real_rustc() {
    let version = rustc_or_panic();
    assert!(
        version.split_whitespace().count() >= 2,
        "expected `rustc <version>`, got {version:?}"
    );
}
