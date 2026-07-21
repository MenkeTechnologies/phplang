//! PHP runtime / error / introspection functions. Part of the `stdlib` chain;
//! see `src/stdlib/mod.rs`.
//!
//! phplang has no full error subsystem, so these are deliberately graceful and
//! program-non-breaking: assertions do not throw, `trigger_error` writes to
//! stderr but never halts, and the error/exception-handler registration
//! functions are documented no-ops that store nothing.

use crate::host::with_host;
use crate::stdlib::common::*;
use fusevm::Value;

/// Dispatch a runtime-category PHP function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let v = match name {
        // assert($assertion, $description = null): bool
        // Assertions are "enabled" but non-fatal here — a falsy assertion yields
        // false rather than throwing AssertionError (phplang has no throw path in
        // this subsystem). The optional $description is accepted and ignored.
        "assert" => {
            let ok = with_host(|h| h.is_truthy(&arg(args, 0)));
            Value::bool(ok)
        }
        // assert_options(...) — no configurable assert state; return 0, PHP's
        // default value for the queried option.
        "assert_options" => Value::int(0),

        // trigger_error($message, $level = E_USER_NOTICE): bool
        // Emits "<Level>: <message>" on stderr and continues; never halts the
        // program (phplang has no fatal user-error path).
        "trigger_error" | "user_error" => {
            let msg = str_arg(args, 0);
            let level = if args.len() > 1 {
                int_arg(args, 1)
            } else {
                1024 // E_USER_NOTICE
            };
            eprintln!("{}: {}", level_label(level), msg);
            Value::bool(true)
        }

        // error_log($message, $message_type = 0, $destination = null): bool
        // With type 3, append the raw message to the $destination file; otherwise
        // write it to stderr. Other message types are not supported (mail/SAPI
        // log) and fall back to stderr. Always returns true.
        "error_log" => {
            let msg = str_arg(args, 0);
            let msg_type = if args.len() > 1 { int_arg(args, 1) } else { 0 };
            if msg_type == 3 && args.len() > 2 {
                let dest = str_arg(args, 2);
                use std::io::Write;
                match std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&dest)
                {
                    Ok(mut f) => {
                        let _ = f.write_all(msg.as_bytes());
                        Value::bool(true)
                    }
                    Err(_) => Value::bool(false),
                }
            } else {
                eprintln!("{msg}");
                Value::bool(true)
            }
        }

        // debug_backtrace($options = ..., $limit = 0): array
        // No real call-stack capture; return an empty frame list.
        "debug_backtrace" => make_list(vec![]),
        // debug_print_backtrace(): void — nothing to print, returns null.
        "debug_print_backtrace" => Value::Undef,

        // Error/exception handler registration — documented no-ops. Nothing is
        // stored, so the "previous handler" is always null.
        // set_* return the previous handler (none here → null); restore_* always
        // return bool true per the PHP manual.
        "set_error_handler" | "set_exception_handler" => Value::Undef,
        "restore_error_handler" | "restore_exception_handler" => Value::bool(true),

        // register_shutdown_function(): void — accepted, never invoked. Returns
        // null, matching PHP's signature.
        "register_shutdown_function" => Value::Undef,
        // spl_autoload_register(): bool — no autoloader chain; accept and report
        // success.
        "spl_autoload_register" => Value::bool(true),
        // spl_autoload_unregister(): bool — nothing registered; report success.
        "spl_autoload_unregister" => Value::bool(true),

        // get_defined_vars(): array — no scope enumerator is exposed to the
        // stdlib, so this returns an empty array (documented deviation).
        "get_defined_vars" => make_list(vec![]),

        // class_alias($original, $alias, $autoload = true): bool
        // Best-effort no-op: true when the original class exists, else false.
        // No real alias is registered (phplang has no class-alias table here).
        "class_alias" => {
            let orig = str_arg(args, 0);
            Value::bool(with_host(|h| h.class_exists(&orig)))
        }

        _ => return None,
    };
    Some(Ok(v))
}

/// Map a PHP error-level constant to its human label used in trigger_error
/// output. Unknown levels fall back to the E_USER_NOTICE label.
fn level_label(level: i64) -> &'static str {
    match level {
        256 => "Fatal error",  // E_USER_ERROR
        512 => "Warning",      // E_USER_WARNING
        16384 => "Deprecated", // E_USER_DEPRECATED
        _ => "Notice",         // E_USER_NOTICE (1024) and anything else
    }
}
