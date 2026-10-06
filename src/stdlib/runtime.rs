//! PHP runtime / error / introspection functions. Part of the `stdlib` chain;
//! see `src/stdlib/mod.rs`.
//!
//! phplang has no full error subsystem, so these are deliberately graceful and
//! program-non-breaking: assertions do not throw, and the error/exception-handler
//! registration functions are documented no-ops that store nothing.
//!
//! `trigger_error` is the exception — it is a real diagnostic, routed through the
//! same [`crate::host::PhpHost::diagnose`] every engine-raised one takes, so it
//! obeys `error_reporting`, `@` suppression and the `display_errors`/`log_errors`
//! stream split rather than printing its own line to stderr.

use crate::host::with_host;
use crate::stdlib::common::*;
use fusevm::Value;

/// Dispatch a runtime-category PHP function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let v = match name {
        // `Exception::__toString` / `Error::__toString`, called by the prelude.
        "__phplang_throwable_string" => return Some(crate::host::throwable_string(&arg(args, 0))),
        // `SplFixedArray`'s offset conversion, called by the prelude.
        "__phplang_spl_offset" => return Some(crate::host::spl_offset(&arg(args, 0))),
        // Call a closure for the prelude and answer `[true, $result]`, or
        // `[false, $exception]` with the exception it threw taken off the
        // stack: the prelude cannot hold a `try` (its try-defs are not loaded).
        "__phplang_try_call" => return Some(crate::host::try_call(arg(args, 0))),
        // An E_WARNING raised by a prelude method, at the user's call site.
        "__phplang_warn" => {
            crate::host::warn_from_prelude(&str_arg(args, 0));
            Value::Undef
        }
        // The lowercased class that declares the method an object would run
        // for a lowercased name, or null: how the prelude tells a subclass
        // override from the method it inherits.
        "__phplang_method_owner" => {
            let lname = str_arg(args, 1);
            match with_host(|h| h.method_owner(&arg(args, 0), &lname)) {
                Some(c) => Value::str(c),
                None => Value::Undef,
            }
        }
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
        //
        // Raises a real diagnostic in the engine's own shape, so it reads
        // `Notice: m in <file> on line N` on both streams, is silenced by
        // `error_reporting(0)` and by `@`, and answers `true`.
        //
        // `$level` accepts the four E_USER_* levels and NOTHING else — an engine
        // level such as `E_WARNING` is a `ValueError`, not a fallback. And
        // E_USER_ERROR is doubly special: PHP 8.4 deprecated passing it, and it
        // still ends the request with a fatal after saying so.
        "trigger_error" | "user_error" => {
            let msg = str_arg(args, 0);
            let level = if args.len() > 1 {
                int_arg(args, 1)
            } else {
                1024 // E_USER_NOTICE
            };
            if !matches!(level, 256 | 512 | 1024 | 16384) {
                return Some(Err(crate::builtins::throws(
                    "ValueError",
                    "trigger_error(): Argument #2 ($error_level) must be one of E_USER_ERROR, \
                     E_USER_WARNING, E_USER_NOTICE, or E_USER_DEPRECATED",
                )));
            }
            if level == 256 {
                return Some(user_error(&msg, args));
            }
            with_host(|h| {
                let line = h.cur_frame_line();
                h.diagnose(level_label(level), level, line, &msg);
            });
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
                crate::host::PhpHost::write_err(format!("{msg}\n").as_bytes());
                Value::bool(true)
            }
        }

        // debug_backtrace($options = ..., $limit = 0): array
        // No real call-stack capture; return an empty frame list.
        "debug_backtrace" => make_list(vec![]),
        // debug_print_backtrace(): void — nothing to print, returns null.
        "debug_print_backtrace" => Value::Undef,

        // A stack of `(callback, error_levels)`; see `host::drain_error_handlers`.
        "set_error_handler" => {
            let levels = match args.get(1) {
                Some(v) if !matches!(v, Value::Undef) => int_arg(args, 1),
                _ => crate::errlevel::E_ALL,
            };
            with_host(|h| h.set_error_handler(arg(args, 0), levels))
        }
        "restore_error_handler" => {
            with_host(|h| h.restore_error_handler());
            Value::bool(true)
        }
        // A stack: `set_*` answers the handler it replaces, `restore_*` pops.
        "set_exception_handler" => with_host(|h| h.set_exception_handler(arg(args, 0))),
        "restore_exception_handler" => {
            with_host(|h| h.restore_exception_handler());
            Value::bool(true)
        }
        // Queued with its arguments; run at request end by `host::request_shutdown`.
        "register_shutdown_function" => {
            let rest = args.get(1..).unwrap_or_default().to_vec();
            with_host(|h| h.register_shutdown_fn(arg(args, 0), rest));
            Value::Undef
        }
        // spl_autoload_register(): bool — no autoloader chain; accept and report
        // success.
        "spl_autoload_register" => Value::bool(true),
        // spl_autoload_unregister(): bool — nothing registered; report success.
        "spl_autoload_unregister" => Value::bool(true),

        // get_defined_vars(): array — the current frame's bound variables, in
        // the order they were first bound. This used to return an empty array
        // because no scope enumerator was exposed; the frame now stores its
        // variables as ordered slots, so one is.
        "get_defined_vars" => with_host(|h| {
            let out = h.new_array();
            for (name, val) in h.defined_vars() {
                h.arr_set_key(&out, &Value::str(name), val);
            }
            out
        }),

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
/// `trigger_error($msg, E_USER_ERROR)` — the one level that ends the request.
///
/// PHP 8.4 deprecated the level itself, so the deprecation is raised FIRST and
/// is visible even though the fatal follows it. The fatal then renders like an
/// uncaught throw's block minus the `thrown in` line, with the `trigger_error`
/// call as frame `#0`, and the process exits 255.
///
/// A user error handler that takes E_USER_ERROR is called instead, after the
/// deprecation has reached it; only a `false` from it lets the fatal through.
fn user_error(msg: &str, args: &[Value]) -> Result<Value, String> {
    with_host(|h| {
        let line = h.cur_frame_line();
        h.diagnose(
            "Deprecated",
            crate::errlevel::E_DEPRECATED,
            line,
            "Passing E_USER_ERROR to trigger_error() is deprecated since 8.4, throw an \
             exception or call exit with a string message instead",
        );
    });
    if crate::host::drain_error_handlers() || crate::host::unwinding() {
        return Ok(Value::Undef);
    }
    let Some(handler) = with_host(|h| h.error_handler_for(crate::errlevel::E_USER_ERROR)) else {
        return user_error_fatal(msg, args);
    };
    let (file, line) = with_host(|h| (h.current_file().to_string(), h.cur_frame_line()));
    let r =
        crate::host::invoke_error_handler(handler, crate::errlevel::E_USER_ERROR, msg, file, line)?;
    if crate::host::unwinding() {
        return Ok(Value::Undef);
    }
    match r {
        Value::Bool(false) => user_error_fatal(msg, args),
        _ => Ok(Value::bool(true)),
    }
}

/// The fatal itself, once the deprecation has been raised.
fn user_error_fatal(msg: &str, args: &[Value]) -> Result<Value, String> {
    with_host(|h| {
        let line = h.cur_frame_line();
        // The frame the trace names is the library call itself, pushed the way
        // `throw_from_internal` pushes it so `backtrace` renders the arguments.
        let argsarr = h.new_array();
        for a in args {
            h.arr_push_auto(&argsarr, a.clone());
        }
        h.push_internal_frame("trigger_error", line, argsarr);
        let body = format!(
            "{msg} in {} on line {line}\nStack trace:\n{}",
            h.current_file(),
            h.backtrace()
        );
        h.pop_internal_frame();
        h.fatal("Fatal error", &body);
    });
    crate::host::with_host(|h| h.disable_destructors());
    crate::host::set_pending_exit(255);
    Ok(Value::Undef)
}

fn level_label(level: i64) -> &'static str {
    match level {
        256 => "Fatal error",  // E_USER_ERROR
        512 => "Warning",      // E_USER_WARNING
        16384 => "Deprecated", // E_USER_DEPRECATED
        _ => "Notice",         // E_USER_NOTICE (1024) and anything else
    }
}
