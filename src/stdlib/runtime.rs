//! PHP standard-library `runtime` functions. Part of the `stdlib` chain; see
//! `src/stdlib/mod.rs`. `dispatch` returns `None` for names it does not handle.

use fusevm::Value;

/// Dispatch a `runtime`-category PHP function by lowercased name. Filled in per
/// category; the scaffold stub recognizes no names yet.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let _ = (name, args);
    None
}
