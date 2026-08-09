//! PHP `define`/`defined`/`constant`. Part of the `stdlib` chain; see
//! `src/stdlib/mod.rs`. Constants live in the host constant table (seeded with the
//! predefined constants), and a bare constant reference resolves through it.

use crate::host::with_host;
use fusevm::Value;

/// Dispatch the constant-management functions by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    match name {
        // define(name, value) — returns true, or false if already defined. The
        // case-insensitive 3rd argument (removed in PHP 8) is accepted and ignored.
        "define" => {
            let cname = with_host(|h| h.to_str(args.first().unwrap_or(&Value::Undef)));
            let value = args.get(1).cloned().unwrap_or(Value::Undef);
            Some(Ok(Value::bool(with_host(|h| {
                h.const_define(&cname, value)
            }))))
        }
        // defined(name) — whether a constant of that name exists.
        "defined" => {
            let cname = with_host(|h| h.to_str(args.first().unwrap_or(&Value::Undef)));
            Some(Ok(Value::bool(with_host(|h| h.const_defined(&cname)))))
        }
        // constant(name) — the value of the named constant. Undefined is the same
        // catchable `Error` a bare reference raises, with the same message.
        "constant" => {
            let cname = with_host(|h| h.to_str(args.first().unwrap_or(&Value::Undef)));
            Some(match with_host(|h| h.const_fetch(&cname)) {
                Some(v) => Ok(v),
                None => Err(crate::builtins::throws(
                    "Error",
                    crate::builtins::undefined_constant(&cname),
                )),
            })
        }
        _ => None,
    }
}
