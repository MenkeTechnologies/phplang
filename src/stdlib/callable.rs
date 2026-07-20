//! PHP standard-library `callable` functions. Part of the `stdlib` chain; see
//! `src/stdlib/mod.rs`. `dispatch` returns `None` for names it does not handle.
//!
//! These functions invoke user-supplied callables through
//! `crate::host::call_value`, which accepts either a closure handle or a
//! function-name string (`call_user_func("strtoupper", "hi") === "HI"`).

use crate::host::{call_method, call_value, with_host};
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
    // NOTE: `isset`/`empty`/`unset`/`list`/`echo`/`print`/`eval` are PHP language
    // constructs, not functions — real PHP `function_exists` returns false for
    // them, so they are deliberately absent here.
    "is_numeric", "is_callable", "is_scalar", "var_dump",
    "var_export", "print_r", "serialize", "unserialize",
    // more strings
    "strtr", "substr_replace", "str_ireplace", "str_word_count", "str_getcsv",
    "str_rot13", "strpbrk", "strspn", "strcspn", "strchr", "strrchr", "strripos",
    "strncasecmp", "strnatcmp", "strnatcasecmp", "quotemeta", "strip_tags",
    "soundex", "sscanf", "chunk_split", "convert_uuencode",
    "convert_uudecode", "quoted_printable_encode", "quoted_printable_decode",
    // more arrays
    "array_key_first", "array_key_last", "array_is_list", "array_fill_keys",
    "array_replace", "array_rand", "array_count_values", "array_merge_recursive",
    "array_diff_key", "array_diff_assoc", "array_intersect_key",
    "array_intersect_assoc", "array_walk_recursive", "array_find", "array_find_key",
    "array_any", "array_all", "natsort", "natcasesort", "shuffle", "reset", "end",
    "next", "prev", "current", "key", "pos",
    // more math
    "sinh", "cosh", "tanh", "asinh", "acosh", "atanh", "expm1", "log1p", "fdiv",
    "is_nan", "is_finite", "is_infinite", "getrandmax", "mt_getrandmax",
    // ctype
    "ctype_alnum", "ctype_alpha", "ctype_digit", "ctype_lower", "ctype_upper",
    "ctype_space", "ctype_punct", "ctype_xdigit", "ctype_cntrl", "ctype_graph",
    "ctype_print",
    // mbstring
    "mb_strlen", "mb_substr", "mb_strtoupper", "mb_strtolower", "mb_strpos",
    "mb_stripos", "mb_strrpos", "mb_str_split", "mb_convert_case", "mb_ord",
    "mb_chr", "mb_str_pad", "mb_substr_count", "mb_strwidth",
    // hash / encoding
    "md5", "sha1", "crc32", "hash", "hash_hmac", "hash_algos", "base64_encode",
    "base64_decode", "urlencode", "urldecode", "rawurlencode", "rawurldecode",
    "http_build_query", "parse_url", "parse_str", "utf8_encode", "utf8_decode",
    // reflection / class introspection
    "class_exists", "interface_exists", "trait_exists", "enum_exists",
    "method_exists", "property_exists", "get_class", "get_parent_class",
    "get_object_vars", "get_class_methods", "get_debug_type", "is_subclass_of",
    "is_a", "is_iterable", "is_countable",
    // fileio
    "file_exists", "file_get_contents", "file_put_contents", "dirname",
    "basename", "pathinfo", "realpath", "getcwd", "scandir", "is_dir", "is_file",
    "is_readable", "is_writable", "file", "readfile", "unlink", "mkdir", "rmdir",
    // filter / constants
    "filter_var", "filter_var_array", "constant", "define", "defined",
    // preg
    "preg_quote", "preg_grep", "preg_replace_callback", "preg_last_error",
    // callable / json / misc
    "call_user_func", "call_user_func_array", "function_exists", "json_encode",
    "json_decode", "json_last_error", "preg_match", "preg_replace", "preg_split",
    "preg_match_all", "strtotime", "date", "gmdate", "mktime", "checkdate",
    "time", "microtime",
];

/// Resolve and invoke any PHP callable form with `args`.
///
/// `host::call_value` already handles the two simplest forms — a closure handle
/// and a plain function-name string. This wrapper adds the three forms PHP
/// accepts that `call_value` does not model:
///
///   * `[$obj, "method"]`    → instance method invoked with `$this = $obj`
///   * `["Class", "method"]` → static method on the named class
///   * `"Class::method"`     → static method on the named class
///
/// Class names resolve case-insensitively up the parent chain (via
/// `host::call_method`); an unknown class or method surfaces as PHP's
/// "call to undefined method" error rather than a panic.
fn invoke_callable(callee: Value, args: Vec<Value>) -> Result<Value, String> {
    // Array callable: PHP requires exactly two elements, [target, method].
    if with_host(|h| h.is_array(&callee)) {
        let pairs = with_host(|h| h.array_pairs(&callee)).unwrap_or_default();
        if pairs.len() != 2 {
            return Err("array callable must have exactly two elements".to_string());
        }
        let target = pairs[0].1.clone();
        let method = with_host(|h| h.to_str(&pairs[1].1));
        // First element an object → instance call; otherwise a class-name string
        // → static call.
        if with_host(|h| h.is_object(&target)) {
            let class = with_host(|h| h.object_class(&target)).unwrap_or_default();
            return call_method(&class, &method, Some(target), args);
        }
        let class = with_host(|h| h.to_str(&target));
        return call_method(&class, &method, None, args);
    }
    // "Class::method" string form → static call. A bare function name (no "::")
    // and any closure handle fall through to `call_value`.
    if let Value::Str(s) = &callee {
        if let Some((class, method)) = s.as_str().split_once("::") {
            return call_method(class, method, None, args);
        }
    }
    call_value(callee, args)
}

/// Dispatch a `callable`-category PHP function by lowercased name. Returns `None`
/// for names this category does not handle.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    match name {
        // call_user_func(callable, ...args): invoke with the remaining arguments.
        // The callable may be any PHP form (closure, "func", [$obj,"m"],
        // ["Class","m"], "Class::m").
        "call_user_func" => {
            let callee = arg(args, 0);
            let rest = args.get(1..).map(<[Value]>::to_vec).unwrap_or_default();
            Some(invoke_callable(callee, rest))
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
            Some(invoke_callable(callee, call_args))
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
