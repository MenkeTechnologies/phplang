//! PHP system / environment / introspection functions. Part of the `stdlib`
//! chain; see `src/stdlib/mod.rs`.

use crate::host::with_host;
use crate::stdlib::common::*;
use fusevm::Value;

/// Dispatch a system-category PHP function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let v = match name {
        // getenv() -> array of all vars; getenv(name) -> value or false.
        "getenv" => {
            if args.is_empty() {
                make_map(
                    std::env::vars()
                        .map(|(k, v)| (Value::str(k), Value::str(v)))
                        .collect(),
                )
            } else {
                let key = str_arg(args, 0);
                match std::env::var(&key) {
                    Ok(v) => Value::str(v),
                    Err(_) => Value::bool(false),
                }
            }
        }
        // putenv("K=V") sets, putenv("K") unsets; returns true.
        "putenv" => {
            let s = str_arg(args, 0);
            match s.split_once('=') {
                Some((k, val)) => std::env::set_var(k, val),
                None => std::env::remove_var(&s),
            }
            Value::bool(true)
        }
        "phpversion" => Value::str("8.3.0"),
        "php_sapi_name" => Value::str("cli"),
        "php_uname" => {
            let mode = if args.is_empty() {
                "a".to_string()
            } else {
                str_arg(args, 0)
            };
            Value::str(php_uname(&mode))
        }
        "getmypid" => Value::int(std::process::id() as i64),
        "getmyuid" | "getmygid" => {
            // Best-effort: not available portably without libc calls here.
            Value::int(0)
        }
        // Memory/GC introspection: phplang has no PHP-level allocator to report,
        // so these return conventional, stable values (documented deviation).
        "memory_get_usage" | "memory_get_peak_usage" => Value::int(2_097_152),
        "gc_collect_cycles" | "gc_mem_caches" => Value::int(0),
        "gc_enable" | "gc_disable" | "gc_enabled" => Value::bool(true),
        // Common extensions phplang emulates are reported as loaded.
        "extension_loaded" => {
            let ext = str_arg(args, 0).to_ascii_lowercase();
            Value::bool(matches!(
                ext.as_str(),
                "core"
                    | "standard"
                    | "json"
                    | "pcre"
                    | "mbstring"
                    | "ctype"
                    | "filter"
                    | "date"
                    | "hash"
                    | "spl"
                    | "tokenizer"
            ))
        }
        // Cooperative: does not actually block, keeping the runtime responsive
        // and tests deterministic (returns 0 = slept fully).
        "sleep" => Value::int(0),
        "usleep" | "time_nanosleep" => Value::Undef,
        "sys_getloadavg" => make_list(vec![
            Value::float(0.0),
            Value::float(0.0),
            Value::float(0.0),
        ]),
        // get_defined_constants() -> assoc name => value of every constant.
        "get_defined_constants" => make_map(
            with_host(|h| h.all_constants())
                .into_iter()
                .map(|(k, v)| (Value::str(k), v))
                .collect(),
        ),
        // NOTE: `get_declared_classes` is NOT handled here. `stdlib::dispatch`
        // reaches `reflection` before `system`, so a copy in this module would
        // never run — it lives in `stdlib::reflection` alone.
        // Iterator helpers — materialize any Traversable to an array via the
        // host's foreach normalization.
        "iterator_to_array" => {
            let arr = match crate::host::foreach_prep(arg(args, 0)) {
                Ok(a) => a,
                Err(e) => return Some(Err(e)),
            };
            // $preserve_keys defaults to true; false re-indexes 0..n.
            let preserve = args.len() < 2 || with_host(|h| h.is_truthy(&arg(args, 1)));
            if preserve {
                arr
            } else {
                let vals = with_host(|h| {
                    h.array_pairs(&arr)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(_, v)| v)
                        .collect::<Vec<_>>()
                });
                make_list(vals)
            }
        }
        "iterator_count" => {
            let arr = match crate::host::foreach_prep(arg(args, 0)) {
                Ok(a) => a,
                Err(e) => return Some(Err(e)),
            };
            Value::int(with_host(|h| h.array_len(&arr)))
        }
        "iterator_apply" => {
            let arr = match crate::host::foreach_prep(arg(args, 0)) {
                Ok(a) => a,
                Err(e) => return Some(Err(e)),
            };
            let cb = arg(args, 1);
            let extra = args.get(2).cloned().unwrap_or(Value::Undef);
            let cb_args = with_host(|h| {
                if h.is_array(&extra) {
                    h.array_pairs(&extra)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(_, v)| v)
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                }
            });
            let n = with_host(|h| h.array_len(&arr));
            let mut count = 0i64;
            for _ in 0..n {
                let r = match crate::host::call_value(cb.clone(), cb_args.clone()) {
                    Ok(r) => r,
                    Err(e) => return Some(Err(e)),
                };
                count += 1;
                if !with_host(|h| h.is_truthy(&r)) {
                    break;
                }
            }
            Value::int(count)
        }
        // Object identity (spl).
        "spl_object_id" => Value::int(with_host(|h| h.object_id(&arg(args, 0))).unwrap_or(0)),
        "spl_object_hash" => {
            let id = with_host(|h| h.object_id(&arg(args, 0))).unwrap_or(0);
            Value::str(format!("{id:032x}"))
        }
        "php_ini_loaded_file" | "get_include_path" => Value::bool(false),
        "set_time_limit" | "ignore_user_abort" => Value::bool(true),
        // `error_reporting($level = null)`: read the mask, or set it and return
        // the PREVIOUS one. Passing null (or nothing) only reads — the two are
        // indistinguishable here because a missing argument arrives as `Undef`,
        // which is also how an explicit `null` arrives, and PHP treats them alike.
        "error_reporting" => with_host(|h| match args.first() {
            Some(v) if !matches!(v, Value::Undef) => Value::int(h.set_error_reporting(v.to_int())),
            _ => Value::int(h.error_reporting()),
        }),
        "ini_get" => {
            let name = str_arg(args, 0);
            with_host(|h| match h.ini_get(&name) {
                Some(s) => Value::str(s),
                None => Value::bool(false),
            })
        }
        "ini_set" => {
            let name = str_arg(args, 0);
            let value = with_host(|h| h.to_str(&arg(args, 1)));
            with_host(|h| match h.ini_set(&name, &value) {
                Some(old) => Value::str(old),
                None => Value::bool(false),
            })
        }

        // ── output buffering ──────────────────────────────────────────────
        "ob_start" => {
            with_host(|h| h.ob_start());
            Value::bool(true)
        }
        "ob_get_contents" => with_host(|h| match h.ob_contents() {
            Some(s) => Value::str(s),
            None => Value::bool(false),
        }),
        "ob_get_clean" => with_host(|h| match h.ob_get_clean() {
            Some(s) => Value::str(s),
            None => Value::bool(false),
        }),
        "ob_end_clean" => Value::bool(with_host(|h| h.ob_end_clean())),
        "ob_end_flush" => Value::bool(with_host(|h| h.ob_end_flush())),
        "ob_get_flush" => with_host(|h| {
            let c = h.ob_contents();
            h.ob_end_flush();
            match c {
                Some(s) => Value::str(s),
                None => Value::bool(false),
            }
        }),
        "ob_flush" => {
            with_host(|h| h.ob_flush());
            Value::Undef
        }
        "flush" => Value::Undef,
        "ob_get_level" => Value::int(with_host(|h| h.ob_level())),
        "ob_get_length" => with_host(|h| match h.ob_contents() {
            Some(s) => Value::int(s.len() as i64),
            None => Value::bool(false),
        }),

        // ── variadic call introspection ───────────────────────────────────
        // These read the enclosing frame's hidden `@args`/`@argnames` pair, set
        // by `invoke`. A position whose `@argnames` entry is a name is answered
        // by READING that parameter now, because the reference reports a
        // parameter's current value rather than the value that was passed.
        //
        // Absent entirely at the global scope, where all three are a fatal in
        // the reference — and each has its own wording.
        "func_get_args" => {
            let Some(vals) = frame_args() else {
                return Some(Err(throws(
                    "Error",
                    "func_get_args() cannot be called from the global scope",
                )));
            };
            with_host(|h| {
                let out = h.new_array();
                for v in vals {
                    h.arr_push_auto(&out, v);
                }
                out
            })
        }
        "func_num_args" => {
            let Some(vals) = frame_args() else {
                return Some(Err(throws(
                    "Error",
                    "func_num_args() must be called from a function context",
                )));
            };
            Value::int(vals.len() as i64)
        }
        "func_get_arg" => {
            let Some(vals) = frame_args() else {
                return Some(Err(throws(
                    "Error",
                    "func_get_arg() cannot be called from the global scope",
                )));
            };
            // Out of range in the two directions is two DIFFERENT ValueErrors in
            // the reference, and a negative position is rejected on its own terms
            // rather than folded into the upper-bound message.
            let i = int_arg(args, 0);
            let Ok(i) = usize::try_from(i) else {
                return Some(Err(throws(
                    "ValueError",
                    "func_get_arg(): Argument #1 ($position) must be greater \
                     than or equal to 0",
                )));
            };
            let Some(v) = vals.get(i) else {
                return Some(Err(throws(
                    "ValueError",
                    "func_get_arg(): Argument #1 ($position) must be less than \
                     the number of the arguments passed to the currently \
                     executed function",
                )));
            };
            v.clone()
        }

        _ => return None,
    };
    Some(Ok(v))
}

/// The enclosing call's arguments as the reference reports them, or `None` at
/// the global scope. `invoke` records them on EVERY function frame and on no
/// other, so their presence is what separates "called from a function" from
/// "called from the top level" — the distinction all three `func_*`
/// introspection functions are a fatal on the wrong side of.
///
/// The reporting rule itself lives in `PhpHost::current_frame_args`, shared with
/// the stack trace, which renders the same values for the same reason.
fn frame_args() -> Option<Vec<Value>> {
    with_host(|h| h.current_frame_args())
}

/// `php_uname($mode)` — `s` OS name, `n` node/host, `r` release, `v` version,
/// `m` machine, `a` (default) the full string. Best-effort from compile-time
/// target info since a portable runtime `uname` needs libc.
fn php_uname(mode: &str) -> String {
    let os = if cfg!(target_os = "macos") {
        "Darwin"
    } else if cfg!(target_os = "windows") {
        "Windows"
    } else {
        "Linux"
    };
    let machine = std::env::consts::ARCH;
    let node = std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .unwrap_or_else(|| "localhost".to_string());
    match mode.chars().next().unwrap_or('a') {
        's' => os.to_string(),
        'n' => node,
        'r' => String::new(),
        'v' => String::new(),
        'm' => machine.to_string(),
        _ => format!("{os} {node} {machine}"),
    }
}
