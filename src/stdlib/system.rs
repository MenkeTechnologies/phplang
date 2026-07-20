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
                "core" | "standard" | "json" | "pcre" | "mbstring" | "ctype" | "filter"
                    | "date" | "hash" | "spl" | "tokenizer"
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
        // get_declared_classes() -> list of declared class names.
        "get_declared_classes" => make_list(
            with_host(|h| h.all_class_names())
                .into_iter()
                .map(Value::str)
                .collect(),
        ),
        // Object identity (spl).
        "spl_object_id" => {
            Value::int(with_host(|h| h.object_id(&arg(args, 0))).unwrap_or(0))
        }
        "spl_object_hash" => {
            let id = with_host(|h| h.object_id(&arg(args, 0))).unwrap_or(0);
            Value::str(format!("{id:032x}"))
        }
        "php_ini_loaded_file" | "get_include_path" => Value::bool(false),
        "set_time_limit" | "ignore_user_abort" => Value::bool(true),
        "error_reporting" => Value::int(32767), // E_ALL
        "ini_get" => Value::bool(false),
        "ini_set" => Value::bool(false),

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
        // These read the enclosing frame's hidden `@args` list (set by `invoke`).
        "func_get_args" => with_host(|h| {
            let a = h.get_var("@args");
            if h.is_array(&a) {
                a
            } else {
                h.new_array()
            }
        }),
        "func_num_args" => with_host(|h| Value::int(h.array_len(&h.get_var("@args")))),
        "func_get_arg" => {
            let i = int_arg(args, 0);
            with_host(|h| h.index_get(&h.get_var("@args"), &Value::int(i)))
        }

        _ => return None,
    };
    Some(Ok(v))
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
