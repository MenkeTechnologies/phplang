//! PHP wiring for inline Rust FFI (`rust { ... }` blocks).
//!
//! The heavy lifting lives in fusevm: [`fusevm::rust_sugar`] scans and rewrites
//! the block at the source level, and [`fusevm::ffi`] compiles/loads/marshals
//! it. This module only supplies the PHP-flavored [`fusevm::RustSugar`] config
//! and the desugar entry the parser calls. The emitted `__rust_compile(...)`
//! call and every exported bareword are resolved in [`crate::host::call_function`].

use fusevm::RustSugar;

/// Emit the PHP statement a `rust { ... }` block desugars to: a call to the
/// `__rust_compile` builtin carrying the base64-encoded block body and its line.
fn emit(b64: &str, line: usize) -> String {
    format!("__rust_compile(\"{b64}\", {line});")
}

/// PHP desugar config: C-family braces with `//`, `#`, and `/* */` comments.
/// `newline_boundary` is `true` so a block right after the `<?php` tag on its
/// own line (`<?php\nrust { ... }`) is still recognized — `rust {` is never
/// valid PHP otherwise, so this only ever matches an intended FFI block.
pub const SUGAR: RustSugar = RustSugar {
    keyword: "rust",
    line_comments: &["//", "#"],
    block_comment: Some(("/*", "*/")),
    newline_boundary: true,
    emit,
};

/// Rewrite every top-level `rust { ... }` block in PHP source into a
/// `__rust_compile(...)` call, before lexing. No-op when the source has no
/// `rust` token.
pub fn desugar(src: &str) -> String {
    SUGAR.desugar(src)
}

#[cfg(test)]
mod tests {
    #[test]
    fn desugars_php_block_after_open_tag() {
        let src = "<?php\nrust { pub extern \"C\" fn add(a: i64, b: i64) -> i64 { a + b } }\necho add(2, 3);\n";
        let out = super::desugar(src);
        assert!(out.contains("__rust_compile("), "no builtin call: {out}");
        assert!(!out.contains("pub extern"), "Rust body leaked: {out}");
        assert!(out.contains("echo add(2, 3);"));
    }

    #[test]
    fn leaves_ordinary_php_untouched() {
        let src = "<?php $x = strlen(\"hi\"); echo $x;\n";
        assert_eq!(super::desugar(src), src);
    }
}
