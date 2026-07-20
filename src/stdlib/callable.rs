//! PHP standard-library `callable` functions. Part of the `stdlib` chain; see
//! `src/stdlib/mod.rs`. `dispatch` returns `None` for names it does not handle.
//!
//! These functions invoke user-supplied callables through
//! `crate::host::call_value`, which accepts either a closure handle or a
//! function-name string (`call_user_func("strtoupper", "hi") === "HI"`).

use crate::host::{call_value, with_host};
use crate::stdlib::common::*;
use fusevm::Value;

/// A curated set of very common builtin function names, used by
/// `function_exists` as a best-effort fallback. There is no global builtin
/// registry, so builtin coverage here is intentionally PARTIAL: only these
/// well-known names report `true` for builtins. User-defined functions are
/// detected exactly via `host::function_defined`. Names are matched
/// case-insensitively (compared already-lowercased).
const KNOWN_BUILTINS: &[&str] = &[
    // strings
    "strlen", "count", "sizeof", "strtoupper", "strtolower", "ucfirst", "ucwords",
    "lcfirst", "trim", "ltrim", "rtrim", "chop", "substr", "strpos", "stripos",
    "strrpos", "str_replace", "str_repeat", "str_pad", "str_split", "strrev",
    "str_contains", "str_starts_with", "str_ends_with", "sprintf", "printf",
    "vsprintf", "vprintf", "implode", "join", "explode", "number_format", "nl2br",
    "wordwrap", "chr", "ord", "bin2hex", "hex2bin", "htmlspecialchars",
    "htmlentities", "addslashes", "stripslashes", "similar_text", "levenshtein",
    "strcmp", "strcasecmp", "strncmp", "strstr", "stristr", "substr_count",
    // arrays
    "array_map", "array_filter", "array_reduce", "array_keys", "array_values",
    "array_merge", "array_push", "array_pop", "array_shift", "array_unshift",
    "array_slice", "array_splice", "array_search", "in_array", "array_key_exists",
    "array_flip", "array_reverse", "array_unique", "array_combine", "array_fill",
    "array_column", "array_diff", "array_intersect", "array_sum", "array_product",
    "sort", "rsort", "asort", "arsort", "ksort", "krsort", "usort", "uasort",
    "uksort", "range", "compact", "extract", "array_walk", "array_pad", "array_chunk",
    // math
    "abs", "ceil", "floor", "round", "sqrt", "pow", "intdiv", "fmod", "max", "min",
    "rand", "mt_rand", "random_int", "pi", "exp", "log", "log10", "sin", "cos",
    "tan", "asin", "acos", "atan", "atan2", "deg2rad", "rad2deg", "hypot",
    "dechex", "hexdec", "decbin", "bindec", "decoct", "octdec", "base_convert",
    "intval", "floatval", "doubleval", "boolval",
    // type / util
    "gettype", "settype", "is_int", "is_integer", "is_long", "is_float",
    "is_double", "is_string", "is_bool", "is_array", "is_object", "is_null",
    "is_numeric", "is_callable", "is_scalar", "isset", "empty", "var_dump",
    "var_export", "print_r", "serialize", "unserialize",
    // callable / json / misc
    "call_user_func", "call_user_func_array", "function_exists", "json_encode",
    "json_decode", "preg_match", "preg_replace", "preg_split", "preg_match_all",
    "strtotime", "date", "time", "microtime",
];

/// Dispatch a `callable`-category PHP function by lowercased name. Returns `None`
/// for names this category does not handle.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    match name {
        // call_user_func(callable, ...args): invoke with the remaining arguments.
        "call_user_func" => {
            let callee = arg(args, 0);
            let rest = args.get(1..).map(<[Value]>::to_vec).unwrap_or_default();
            Some(call_value(callee, rest))
        }

        // call_user_func_array(callable, array): unpack the array's VALUES (in
        // order, keys ignored — named-argument spreading is not modeled) and
        // invoke. A non-array second argument yields no arguments.
        "call_user_func_array" => {
            let callee = arg(args, 0);
            let arr = arg(args, 1);
            let call_args = with_host(|h| h.array_pairs(&arr))
                .unwrap_or_default()
                .into_iter()
                .map(|(_, v)| v)
                .collect::<Vec<_>>();
            Some(call_value(callee, call_args))
        }

        // function_exists(name): true for a defined user function, or for a
        // recognized builtin. Builtin coverage is PARTIAL — see KNOWN_BUILTINS.
        "function_exists" => {
            let fname = str_arg(args, 0);
            let lname = fname.to_ascii_lowercase();
            let exists = with_host(|h| h.function_defined(&fname))
                || KNOWN_BUILTINS.contains(&lname.as_str());
            Some(Ok(Value::bool(exists)))
        }

        _ => None,
    }
}
