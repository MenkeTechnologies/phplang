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
    "strlen",
    "count",
    "sizeof",
    "strtoupper",
    "strtolower",
    "ucfirst",
    "ucwords",
    "lcfirst",
    "trim",
    "ltrim",
    "rtrim",
    "chop",
    "substr",
    "strpos",
    "stripos",
    "strrpos",
    "str_replace",
    "str_repeat",
    "str_pad",
    "str_split",
    "strrev",
    "str_contains",
    "str_starts_with",
    "str_ends_with",
    "sprintf",
    "printf",
    "vsprintf",
    "vprintf",
    "implode",
    "join",
    "explode",
    "number_format",
    "nl2br",
    "wordwrap",
    "chr",
    "ord",
    "bin2hex",
    "hex2bin",
    "htmlspecialchars",
    "htmlentities",
    "addslashes",
    "stripslashes",
    "similar_text",
    "levenshtein",
    "strcmp",
    "strcasecmp",
    "strncmp",
    "strstr",
    "stristr",
    "substr_count",
    // arrays
    "array_map",
    "array_filter",
    "array_reduce",
    "array_keys",
    "array_values",
    "array_merge",
    "array_push",
    "array_pop",
    "array_shift",
    "array_unshift",
    "array_slice",
    "array_splice",
    "array_search",
    "in_array",
    "array_key_exists",
    "array_flip",
    "array_reverse",
    "array_unique",
    "array_combine",
    "array_fill",
    "array_column",
    "array_diff",
    "array_intersect",
    "array_sum",
    "array_product",
    "sort",
    "rsort",
    "asort",
    "arsort",
    "ksort",
    "krsort",
    "usort",
    "uasort",
    "uksort",
    "range",
    "compact",
    "extract",
    "array_walk",
    "array_pad",
    "array_chunk",
    // math
    "abs",
    "ceil",
    "floor",
    "round",
    "sqrt",
    "pow",
    "intdiv",
    "fmod",
    "max",
    "min",
    "rand",
    "mt_rand",
    "random_int",
    "pi",
    "exp",
    "log",
    "log10",
    "sin",
    "cos",
    "tan",
    "asin",
    "acos",
    "atan",
    "atan2",
    "deg2rad",
    "rad2deg",
    "hypot",
    "dechex",
    "hexdec",
    "decbin",
    "bindec",
    "decoct",
    "octdec",
    "base_convert",
    "intval",
    "floatval",
    "doubleval",
    "boolval",
    // type / util
    "gettype",
    "settype",
    "is_int",
    "is_integer",
    "is_long",
    "is_float",
    "is_double",
    "is_string",
    "is_bool",
    "is_array",
    "is_object",
    "is_null",
    // NOTE: `isset`/`empty`/`unset`/`list`/`echo`/`print`/`eval` are PHP language
    // constructs, not functions — real PHP `function_exists` returns false for
    // them, so they are deliberately absent here. `exit`/`die` are the exception
    // that proves the rule: PHP 8.4 made them real functions, and 8.5 answers
    // `true` for both (verified against `php -r 'var_dump(function_exists("exit"),
    // function_exists("die"));'` on 8.5.9).
    "exit",
    "die",
    "is_numeric",
    "is_callable",
    "is_scalar",
    "var_dump",
    "var_export",
    "print_r",
    "serialize",
    "unserialize",
    // more strings
    "strtr",
    "substr_replace",
    "str_ireplace",
    "str_word_count",
    "str_getcsv",
    "str_rot13",
    "strpbrk",
    "strspn",
    "strcspn",
    "strchr",
    "strrchr",
    "strripos",
    "strncasecmp",
    "strnatcmp",
    "strnatcasecmp",
    "quotemeta",
    "strip_tags",
    "soundex",
    "sscanf",
    "chunk_split",
    "convert_uuencode",
    "convert_uudecode",
    "quoted_printable_encode",
    "quoted_printable_decode",
    // more arrays
    "array_key_first",
    "array_key_last",
    "array_is_list",
    "array_fill_keys",
    "array_replace",
    "array_rand",
    "array_count_values",
    "array_merge_recursive",
    "array_diff_key",
    "array_diff_assoc",
    "array_intersect_key",
    "array_intersect_assoc",
    "array_walk_recursive",
    "array_find",
    "array_find_key",
    "array_any",
    "array_all",
    "natsort",
    "natcasesort",
    "shuffle",
    "reset",
    "end",
    "next",
    "prev",
    "current",
    "key",
    "pos",
    // more math
    "sinh",
    "cosh",
    "tanh",
    "asinh",
    "acosh",
    "atanh",
    "expm1",
    "log1p",
    "fdiv",
    "is_nan",
    "is_finite",
    "is_infinite",
    "getrandmax",
    "mt_getrandmax",
    // ctype
    "ctype_alnum",
    "ctype_alpha",
    "ctype_digit",
    "ctype_lower",
    "ctype_upper",
    "ctype_space",
    "ctype_punct",
    "ctype_xdigit",
    "ctype_cntrl",
    "ctype_graph",
    "ctype_print",
    // mbstring
    "mb_strlen",
    "mb_substr",
    "mb_strtoupper",
    "mb_strtolower",
    "mb_strpos",
    "mb_stripos",
    "mb_strrpos",
    "mb_str_split",
    "mb_convert_case",
    "mb_ord",
    "mb_chr",
    "mb_str_pad",
    "mb_substr_count",
    "mb_strwidth",
    // hash / encoding
    "md5",
    "sha1",
    "crc32",
    "hash",
    "hash_hmac",
    "hash_algos",
    "base64_encode",
    "base64_decode",
    "urlencode",
    "urldecode",
    "rawurlencode",
    "rawurldecode",
    "http_build_query",
    "parse_url",
    "parse_str",
    "utf8_encode",
    "utf8_decode",
    // reflection / class introspection
    "class_exists",
    "interface_exists",
    "trait_exists",
    "enum_exists",
    "method_exists",
    "property_exists",
    "get_class",
    "get_parent_class",
    "get_object_vars",
    "get_class_methods",
    "get_debug_type",
    "is_subclass_of",
    "is_a",
    "is_iterable",
    "is_countable",
    // fileio
    "file_exists",
    "file_get_contents",
    "file_put_contents",
    "dirname",
    "basename",
    "pathinfo",
    "realpath",
    "getcwd",
    "scandir",
    "is_dir",
    "is_file",
    "is_readable",
    "is_writable",
    "file",
    "readfile",
    "unlink",
    "mkdir",
    "rmdir",
    // filter / constants
    "filter_var",
    "filter_var_array",
    "constant",
    "define",
    "defined",
    // preg
    "preg_quote",
    "preg_grep",
    "preg_replace_callback",
    "preg_last_error",
    // callable / json / misc
    "call_user_func",
    "call_user_func_array",
    "function_exists",
    "json_encode",
    "json_decode",
    "json_last_error",
    "preg_match",
    "preg_replace",
    "preg_split",
    "preg_match_all",
    "strtotime",
    "date",
    "gmdate",
    "mktime",
    "checkdate",
    "time",
    "microtime",
];

/// Resolve and invoke any PHP callable form with `args`.
///
/// Every form — closure handle, `"function"`, `"Class::method"`,
/// `[$obj, "method"]`, `["Class", "method"]`, and an object with `__invoke` — is
/// decoded by `host::call_value`, so `call_user_func` and a bare `$f(…)` cannot
/// disagree about what is callable. This wrapper exists only to name the entry
/// point the `call_user_func*` builtins go through.
fn invoke_callable(callee: Value, args: Vec<Value>) -> Result<Value, String> {
    call_value(callee, args)
}

/// Whether a bare function name resolves — a user-defined function or a builtin
/// this engine implements. Same authority as `function_exists`, so the two can
/// never disagree about a name; builtin coverage is PARTIAL (see
/// [`KNOWN_BUILTINS`]).
fn function_resolves(name: &str) -> bool {
    with_host(|h| h.function_defined(name))
        || KNOWN_BUILTINS.contains(&name.to_ascii_lowercase().as_str())
}

/// Whether `class::method` is reachable from the current scope, either directly
/// or through the magic catch-all. `has_this` distinguishes the instance form
/// (`[$obj, "m"]`, which may fall back to `__call`) from the static one
/// (`["C", "m"]` / `"C::m"`, which falls back to `__callStatic`).
fn method_resolves(class: &str, method: &str, has_this: bool) -> bool {
    with_host(|h| {
        matches!(
            h.method_dispatch(class, method, has_this),
            crate::host::MethodDispatch::Direct | crate::host::MethodDispatch::Magic
        )
    })
}

/// `is_callable($v)` — whether the value names something invocable.
///
/// The rules are the reference's, and the two that are easy to miss are that a
/// method out of reach is NOT callable while one backed by `__call` IS, and that
/// an object is callable exactly when its class defines `__invoke`. A closure
/// handle is always callable.
fn callable_resolves(v: &Value) -> bool {
    if with_host(|h| h.is_closure(v)) {
        return true;
    }
    // Array callable: PHP requires exactly two elements, [target, method].
    if with_host(|h| h.is_array(v)) {
        let pairs = with_host(|h| h.array_pairs(v)).unwrap_or_default();
        if pairs.len() != 2 {
            return false;
        }
        let target = pairs[0].1.clone();
        let method = with_host(|h| h.to_str(&pairs[1].1));
        if with_host(|h| h.is_object(&target)) {
            let Some(class) = with_host(|h| h.object_class(&target)) else {
                return false;
            };
            return method_resolves(&class, &method, true);
        }
        let class = with_host(|h| h.to_str(&target));
        return with_host(|h| h.class_exists(&class)) && method_resolves(&class, &method, false);
    }
    // A non-array object is callable through `__invoke`.
    if with_host(|h| h.is_object(v)) {
        return with_host(|h| {
            h.object_class(v)
                .is_some_and(|c| h.class_has_method(&c, "__invoke"))
        });
    }
    // Only a string can name a function; every other scalar is not callable.
    let Value::Str(s) = v else {
        return false;
    };
    match s.as_str().split_once("::") {
        Some((class, method)) => {
            with_host(|h| h.class_exists(class)) && method_resolves(class, method, false)
        }
        None => function_resolves(s.as_str()),
    }
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

        // is_callable(value): whether the value names something this engine could
        // actually invoke — see `callable_resolves`.
        "is_callable" => Some(Ok(Value::bool(callable_resolves(&arg(args, 0))))),

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
