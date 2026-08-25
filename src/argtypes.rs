//! Declared parameter types for the standard library, and the check PHP 8 runs
//! against them before a builtin sees its arguments.
//!
//! PHP 8 turned what PHP 7 coerced into a `TypeError` for a whole class of
//! arguments, and the line it drew is per TYPE rather than per function:
//! `strlen([1,2])` is an error, `strlen(1.5)` is `3`. Checking that once here,
//! against a table of the declared types, is what keeps the two consistent —
//! the alternative is a coercion decision restated in each of 374 functions.
//!
//! Only the types whose accept/reject rule was read off the reference are
//! listed. A parameter typed `mixed`, `callable`, `GMP|string|int` or a class
//! is absent, and an absent parameter is not checked at all: this narrows what
//! reaches a builtin, and never widens it.
//!
//! By-reference parameters are absent too. Those are written rather than read,
//! so an unset variable in one is normal and carries no type to check.

use crate::host::{self, PhpHost};
use fusevm::Value;

/// `(function, [(1-based argument number, parameter name, declared type)])`.
///
/// The types are spelled exactly as the reference prints them in the message,
/// so a union reads `int|float` and a nullable reads `?array` — which is the
/// text `must be of type …` needs.
type Params = &'static [(u32, &'static str, &'static str)];

/// Whether `v` satisfies the declared type `ty` under PHP 8's COERCIVE mode —
/// the default, and the only one this table is consulted in.
///
/// The rules are the reference's, probed per type rather than assumed:
///
/// * an array satisfies only `array`, and a union that offers `array`;
/// * an object satisfies `string` only when it has `__toString`, and no numeric
///   type at all;
/// * a string satisfies `int`/`float` only when it is fully numeric, so `"5"`
///   and `"5.0"` and `" 5 "` pass while `"5abc"` and `"0x1A"` do not;
/// * `null` is NOT handled here — see [`check_call`], because for a scalar it is
///   a deprecation and for `array` it is an error;
/// * `bool`, `int`, `float` satisfy every scalar type.
fn satisfies(h: &PhpHost, ty: &str, v: &Value) -> bool {
    // A union is satisfied by any member. `?T` is `T` plus null, and null never
    // reaches here.
    if let Some(rest) = ty.strip_prefix('?') {
        return satisfies(h, rest, v);
    }
    if ty.contains('|') {
        return ty.split('|').any(|m| satisfies(h, m, v));
    }
    let is_array = matches!(v, Value::Obj(_)) && h.is_array(v);
    let is_object = matches!(v, Value::Obj(_)) && !is_array;
    match ty {
        "array" => is_array,
        "string" => match v {
            _ if is_array => false,
            // An object stands in for a string only through `__toString`.
            _ if is_object => h
                .object_class(v)
                .is_some_and(|c| h.class_has_method(&c, "__tostring")),
            _ => true,
        },
        "int" | "float" => match v {
            _ if is_array || is_object => false,
            // `"5"` converts, `"5abc"` does not — this is the numeric-string
            // test, not the leading-numeric one the arithmetic operators use.
            Value::Str(s) => host::is_numeric_string(s),
            _ => true,
        },
        "bool" => !is_array && !is_object,
        // Anything not in the table above was not probed, so it is not judged.
        _ => true,
    }
}

/// The declared types of `name`, or `None` for a function the table does not
/// describe.
fn params_of(name: &str) -> Option<Params> {
    let lname = name.to_ascii_lowercase();
    PARAMS
        .binary_search_by(|(n, _)| (*n).cmp(lname.as_str()))
        .ok()
        .map(|i| PARAMS[i].1)
}

/// Check the arguments of a call to `name` and report the first one the
/// reference would refuse.
///
/// Returns the tagged `TypeError` for the caller to raise, or `Ok(())` when
/// every argument is acceptable. A `null` in a SCALAR parameter is acceptable
/// and merely deprecated, so it is reported here as a diagnostic rather than as
/// an error — but a `null` in an `array` parameter is a plain type failure and
/// falls through to the message below.
pub fn check_call(name: &str, args: &[Value]) -> Result<(), String> {
    let Some(params) = params_of(name) else {
        return Ok(());
    };
    for &(argno, pname, ty) in params {
        let Some(v) = args.get(argno as usize - 1) else {
            continue;
        };
        if matches!(v, Value::Undef) {
            // `null` into a scalar is the deprecation, not the error. Into
            // anything else it is an ordinary type failure and drops through.
            let scalar = matches!(
                ty.trim_start_matches('?'),
                "string" | "int" | "float" | "bool"
            );
            if scalar && !ty.starts_with('?') {
                host::with_host(|h| {
                    h.deprecated(format!(
                        "{name}(): Passing null to parameter #{argno} (${pname}) of type {ty} is deprecated"
                    ))
                });
            }
            if scalar || ty.starts_with('?') {
                continue;
            }
        } else if host::with_host(|h| satisfies(h, ty, v)) {
            continue;
        }
        let given = host::with_host(|h| h.type_name_for_error(v));
        return Err(crate::builtins::throws(
            "TypeError",
            format!("{name}(): Argument #{argno} (${pname}) must be of type {ty}, {given} given"),
        ));
    }
    Ok(())
}

/// Two entries were corrected against the reference's own message, which does
/// not always agree with its reflection: `number_format`'s `$num` reflects as
/// `float` but is reported as `int|float`.
///
/// Two more are deliberately left WIDER than the reference. `implode` and
/// `strtr` each have two signatures, and which type a position carries depends
/// on how many arguments were passed — `implode`'s `$separator` reflects as
/// `array|string` because of the one-argument form, but in the two-argument
/// form it is `string`. The table has no arity dimension, so those positions
/// accept the union and let the function decide, which under-checks rather
/// than inventing a refusal the reference does not make.
///
/// The declared types, sorted by name so [`params_of`] can binary-search.
///
/// Generated from the reference's own reflection over the functions this build
/// implements, so a type here is the one PHP declares rather than one inferred
/// from phplang's implementation.
static PARAMS: &[(&str, Params)] = &[
// generated: 374 functions
    ("abs", &[(1, "num", "int|float")]),
    ("acos", &[(1, "num", "float")]),
    ("acosh", &[(1, "num", "float")]),
    ("addcslashes", &[(1, "string", "string"), (2, "characters", "string")]),
    ("addslashes", &[(1, "string", "string")]),
    ("array_all", &[(1, "array", "array")]),
    ("array_any", &[(1, "array", "array")]),
    ("array_change_key_case", &[(1, "array", "array"), (2, "case", "int")]),
    ("array_chunk", &[(1, "array", "array"), (2, "length", "int"), (3, "preserve_keys", "bool")]),
    ("array_column", &[(1, "array", "array")]),
    ("array_combine", &[(1, "keys", "array"), (2, "values", "array")]),
    ("array_count_values", &[(1, "array", "array")]),
    ("array_diff", &[(1, "array", "array")]),
    ("array_diff_assoc", &[(1, "array", "array")]),
    ("array_diff_key", &[(1, "array", "array")]),
    ("array_diff_ukey", &[(1, "array", "array")]),
    ("array_fill", &[(1, "start_index", "int"), (2, "count", "int")]),
    ("array_fill_keys", &[(1, "keys", "array")]),
    ("array_filter", &[(1, "array", "array"), (3, "mode", "int")]),
    ("array_find", &[(1, "array", "array")]),
    ("array_find_key", &[(1, "array", "array")]),
    ("array_flip", &[(1, "array", "array")]),
    ("array_intersect", &[(1, "array", "array")]),
    ("array_intersect_assoc", &[(1, "array", "array")]),
    ("array_intersect_key", &[(1, "array", "array")]),
    ("array_intersect_ukey", &[(1, "array", "array")]),
    ("array_is_list", &[(1, "array", "array")]),
    ("array_key_exists", &[(2, "array", "array")]),
    ("array_key_first", &[(1, "array", "array")]),
    ("array_key_last", &[(1, "array", "array")]),
    ("array_keys", &[(1, "array", "array"), (3, "strict", "bool")]),
    ("array_map", &[(2, "array", "array")]),
    ("array_pad", &[(1, "array", "array"), (2, "length", "int")]),
    ("array_product", &[(1, "array", "array")]),
    ("array_rand", &[(1, "array", "array"), (2, "num", "int")]),
    ("array_reduce", &[(1, "array", "array")]),
    ("array_replace", &[(1, "array", "array")]),
    ("array_replace_recursive", &[(1, "array", "array")]),
    ("array_reverse", &[(1, "array", "array"), (2, "preserve_keys", "bool")]),
    ("array_search", &[(2, "haystack", "array"), (3, "strict", "bool")]),
    ("array_slice", &[(1, "array", "array"), (2, "offset", "int"), (3, "length", "?int"), (4, "preserve_keys", "bool")]),
    ("array_splice", &[(2, "offset", "int"), (3, "length", "?int")]),
    ("array_sum", &[(1, "array", "array")]),
    ("array_udiff", &[(1, "array", "array")]),
    ("array_uintersect", &[(1, "array", "array")]),
    ("array_unique", &[(1, "array", "array"), (2, "flags", "int")]),
    ("array_values", &[(1, "array", "array")]),
    ("arsort", &[(2, "flags", "int")]),
    ("asin", &[(1, "num", "float")]),
    ("asinh", &[(1, "num", "float")]),
    ("asort", &[(2, "flags", "int")]),
    ("assert_options", &[(1, "option", "int")]),
    ("atan", &[(1, "num", "float")]),
    ("atan2", &[(1, "y", "float"), (2, "x", "float")]),
    ("atanh", &[(1, "num", "float")]),
    ("base64_decode", &[(1, "string", "string"), (2, "strict", "bool")]),
    ("base64_encode", &[(1, "string", "string")]),
    ("base_convert", &[(1, "num", "string"), (2, "from_base", "int"), (3, "to_base", "int")]),
    ("basename", &[(1, "path", "string"), (2, "suffix", "string")]),
    ("bcadd", &[(1, "num1", "string"), (2, "num2", "string"), (3, "scale", "?int")]),
    ("bccomp", &[(1, "num1", "string"), (2, "num2", "string"), (3, "scale", "?int")]),
    ("bcdiv", &[(1, "num1", "string"), (2, "num2", "string"), (3, "scale", "?int")]),
    ("bcmod", &[(1, "num1", "string"), (2, "num2", "string"), (3, "scale", "?int")]),
    ("bcmul", &[(1, "num1", "string"), (2, "num2", "string"), (3, "scale", "?int")]),
    ("bcpow", &[(1, "num", "string"), (2, "exponent", "string"), (3, "scale", "?int")]),
    ("bcpowmod", &[(1, "num", "string"), (2, "exponent", "string"), (3, "modulus", "string"), (4, "scale", "?int")]),
    ("bcscale", &[(1, "scale", "?int")]),
    ("bcsqrt", &[(1, "num", "string"), (2, "scale", "?int")]),
    ("bcsub", &[(1, "num1", "string"), (2, "num2", "string"), (3, "scale", "?int")]),
    ("bin2hex", &[(1, "string", "string")]),
    ("bindec", &[(1, "binary_string", "string")]),
    ("call_user_func_array", &[(2, "args", "array")]),
    ("ceil", &[(1, "num", "int|float")]),
    ("checkdate", &[(1, "month", "int"), (2, "day", "int"), (3, "year", "int")]),
    ("chop", &[(1, "string", "string"), (2, "characters", "string")]),
    ("chr", &[(1, "codepoint", "int")]),
    ("chunk_split", &[(1, "string", "string"), (2, "length", "int"), (3, "separator", "string")]),
    ("class_alias", &[(1, "class", "string"), (2, "alias", "string"), (3, "autoload", "bool")]),
    ("class_exists", &[(1, "class", "string"), (2, "autoload", "bool")]),
    ("class_implements", &[(2, "autoload", "bool")]),
    ("class_parents", &[(2, "autoload", "bool")]),
    ("class_uses", &[(2, "autoload", "bool")]),
    ("clearstatcache", &[(1, "clear_realpath_cache", "bool"), (2, "filename", "string")]),
    ("constant", &[(1, "name", "string")]),
    ("convert_uudecode", &[(1, "string", "string")]),
    ("convert_uuencode", &[(1, "string", "string")]),
    ("copy", &[(1, "from", "string"), (2, "to", "string")]),
    ("cos", &[(1, "num", "float")]),
    ("cosh", &[(1, "num", "float")]),
    ("count", &[(2, "mode", "int")]),
    ("count_chars", &[(1, "string", "string"), (2, "mode", "int")]),
    ("crc32", &[(1, "string", "string")]),
    ("date", &[(1, "format", "string"), (2, "timestamp", "?int")]),
    ("date_create", &[(1, "datetime", "string")]),
    ("date_create_immutable", &[(1, "datetime", "string")]),
    ("date_default_timezone_set", &[(1, "timezoneId", "string")]),
    ("date_diff", &[(3, "absolute", "bool")]),
    ("date_format", &[(2, "format", "string")]),
    ("date_interval_create_from_date_string", &[(1, "datetime", "string")]),
    ("date_interval_format", &[(2, "format", "string")]),
    ("date_modify", &[(2, "modifier", "string")]),
    ("date_timestamp_set", &[(2, "timestamp", "int")]),
    ("debug_backtrace", &[(1, "options", "int"), (2, "limit", "int")]),
    ("debug_print_backtrace", &[(1, "options", "int"), (2, "limit", "int")]),
    ("decbin", &[(1, "num", "int")]),
    ("dechex", &[(1, "num", "int")]),
    ("decoct", &[(1, "num", "int")]),
    ("define", &[(1, "constant_name", "string"), (3, "case_insensitive", "bool")]),
    ("defined", &[(1, "constant_name", "string")]),
    ("deg2rad", &[(1, "num", "float")]),
    ("dirname", &[(1, "path", "string"), (2, "levels", "int")]),
    ("disk_free_space", &[(1, "directory", "string")]),
    ("disk_total_space", &[(1, "directory", "string")]),
    ("diskfreespace", &[(1, "directory", "string")]),
    ("enum_exists", &[(1, "enum", "string"), (2, "autoload", "bool")]),
    ("error_log", &[(1, "message", "string"), (2, "message_type", "int"), (3, "destination", "?string"), (4, "additional_headers", "?string")]),
    ("error_reporting", &[(1, "error_level", "?int")]),
    ("exp", &[(1, "num", "float")]),
    ("explode", &[(1, "separator", "string"), (2, "string", "string"), (3, "limit", "int")]),
    ("expm1", &[(1, "num", "float")]),
    ("extension_loaded", &[(1, "extension", "string")]),
    ("extract", &[(2, "flags", "int"), (3, "prefix", "string")]),
    ("fdiv", &[(1, "num1", "float"), (2, "num2", "float")]),
    ("fgets", &[(2, "length", "?int")]),
    ("file", &[(1, "filename", "string"), (2, "flags", "int")]),
    ("file_exists", &[(1, "filename", "string")]),
    ("file_get_contents", &[(1, "filename", "string"), (2, "use_include_path", "bool"), (4, "offset", "int"), (5, "length", "?int")]),
    ("file_put_contents", &[(1, "filename", "string"), (3, "flags", "int")]),
    ("filemtime", &[(1, "filename", "string")]),
    ("fileperms", &[(1, "filename", "string")]),
    ("filesize", &[(1, "filename", "string")]),
    ("filetype", &[(1, "filename", "string")]),
    ("filter_has_var", &[(1, "input_type", "int"), (2, "var_name", "string")]),
    ("filter_var", &[(2, "filter", "int")]),
    ("filter_var_array", &[(1, "array", "array"), (3, "add_empty", "bool")]),
    ("floor", &[(1, "num", "int|float")]),
    ("fmod", &[(1, "num1", "float"), (2, "num2", "float")]),
    ("fnmatch", &[(1, "pattern", "string"), (2, "filename", "string"), (3, "flags", "int")]),
    ("fopen", &[(1, "filename", "string"), (2, "mode", "string"), (3, "use_include_path", "bool")]),
    ("fprintf", &[(2, "format", "string")]),
    ("fputs", &[(2, "data", "string"), (3, "length", "?int")]),
    ("fread", &[(2, "length", "int")]),
    ("fscanf", &[(2, "format", "string")]),
    ("fseek", &[(2, "offset", "int"), (3, "whence", "int")]),
    ("func_get_arg", &[(1, "position", "int")]),
    ("function_exists", &[(1, "function", "string")]),
    ("fwrite", &[(2, "data", "string"), (3, "length", "?int")]),
    ("get_class_vars", &[(1, "class", "string")]),
    ("get_defined_constants", &[(1, "categorize", "bool")]),
    ("get_html_translation_table", &[(1, "table", "int"), (2, "flags", "int"), (3, "encoding", "string")]),
    ("getdate", &[(1, "timestamp", "?int")]),
    ("getenv", &[(1, "name", "?string"), (2, "local_only", "bool")]),
    ("glob", &[(1, "pattern", "string"), (2, "flags", "int")]),
    ("gmdate", &[(1, "format", "string"), (2, "timestamp", "?int")]),
    ("gmmktime", &[(1, "hour", "int"), (2, "minute", "?int"), (3, "second", "?int"), (4, "month", "?int"), (5, "day", "?int"), (6, "year", "?int")]),
    ("gmp_div", &[(3, "rounding_mode", "int")]),
    ("gmp_div_q", &[(3, "rounding_mode", "int")]),
    ("gmp_div_r", &[(3, "rounding_mode", "int")]),
    ("gmp_init", &[(1, "num", "string|int"), (2, "base", "int")]),
    ("gmp_pow", &[(2, "exponent", "int")]),
    ("gmp_prob_prime", &[(2, "repetitions", "int")]),
    ("gmp_root", &[(2, "nth", "int")]),
    ("gmp_strval", &[(2, "base", "int")]),
    ("hash", &[(1, "algo", "string"), (2, "data", "string"), (3, "binary", "bool"), (4, "options", "array")]),
    ("hash_equals", &[(1, "known_string", "string"), (2, "user_string", "string")]),
    ("hash_file", &[(1, "algo", "string"), (2, "filename", "string"), (3, "binary", "bool"), (4, "options", "array")]),
    ("hash_hmac", &[(1, "algo", "string"), (2, "data", "string"), (3, "key", "string"), (4, "binary", "bool")]),
    ("hash_hmac_file", &[(1, "algo", "string"), (2, "filename", "string"), (3, "key", "string"), (4, "binary", "bool")]),
    ("hash_pbkdf2", &[(1, "algo", "string"), (2, "password", "string"), (3, "salt", "string"), (4, "iterations", "int"), (5, "length", "int"), (6, "binary", "bool"), (7, "options", "array")]),
    ("hex2bin", &[(1, "string", "string")]),
    ("hexdec", &[(1, "hex_string", "string")]),
    ("html_entity_decode", &[(1, "string", "string"), (2, "flags", "int"), (3, "encoding", "?string")]),
    ("htmlentities", &[(1, "string", "string"), (2, "flags", "int"), (3, "encoding", "?string"), (4, "double_encode", "bool")]),
    ("htmlspecialchars", &[(1, "string", "string"), (2, "flags", "int"), (3, "encoding", "?string"), (4, "double_encode", "bool")]),
    ("htmlspecialchars_decode", &[(1, "string", "string"), (2, "flags", "int")]),
    ("http_build_query", &[(2, "numeric_prefix", "string"), (3, "arg_separator", "?string"), (4, "encoding_type", "int")]),
    ("hypot", &[(1, "x", "float"), (2, "y", "float")]),
    ("ignore_user_abort", &[(1, "enable", "?bool")]),
    ("implode", &[(1, "separator", "array|string"), (2, "array", "?array")]),
    ("in_array", &[(2, "haystack", "array"), (3, "strict", "bool")]),
    ("ini_get", &[(1, "option", "string")]),
    ("ini_set", &[(1, "option", "string")]),
    ("intdiv", &[(1, "num1", "int"), (2, "num2", "int")]),
    ("interface_exists", &[(1, "interface", "string"), (2, "autoload", "bool")]),
    ("intval", &[(2, "base", "int")]),
    ("is_a", &[(2, "class", "string"), (3, "allow_string", "bool")]),
    ("is_callable", &[(2, "syntax_only", "bool")]),
    ("is_dir", &[(1, "filename", "string")]),
    ("is_executable", &[(1, "filename", "string")]),
    ("is_file", &[(1, "filename", "string")]),
    ("is_finite", &[(1, "num", "float")]),
    ("is_infinite", &[(1, "num", "float")]),
    ("is_link", &[(1, "filename", "string")]),
    ("is_nan", &[(1, "num", "float")]),
    ("is_readable", &[(1, "filename", "string")]),
    ("is_subclass_of", &[(2, "class", "string"), (3, "allow_string", "bool")]),
    ("is_writable", &[(1, "filename", "string")]),
    ("is_writeable", &[(1, "filename", "string")]),
    ("iterator_apply", &[(3, "args", "?array")]),
    ("iterator_to_array", &[(2, "preserve_keys", "bool")]),
    ("join", &[(1, "separator", "array|string"), (2, "array", "?array")]),
    ("json_decode", &[(1, "json", "string"), (2, "associative", "?bool"), (3, "depth", "int"), (4, "flags", "int")]),
    ("json_encode", &[(2, "flags", "int"), (3, "depth", "int")]),
    ("json_validate", &[(1, "json", "string"), (2, "depth", "int"), (3, "flags", "int")]),
    ("key_exists", &[(2, "array", "array")]),
    ("krsort", &[(2, "flags", "int")]),
    ("ksort", &[(2, "flags", "int")]),
    ("lcfirst", &[(1, "string", "string")]),
    ("levenshtein", &[(1, "string1", "string"), (2, "string2", "string"), (3, "insertion_cost", "int"), (4, "replacement_cost", "int"), (5, "deletion_cost", "int")]),
    ("log", &[(1, "num", "float"), (2, "base", "float")]),
    ("log10", &[(1, "num", "float")]),
    ("log1p", &[(1, "num", "float")]),
    ("lstat", &[(1, "filename", "string")]),
    ("ltrim", &[(1, "string", "string"), (2, "characters", "string")]),
    ("mb_check_encoding", &[(2, "encoding", "?string")]),
    ("mb_chr", &[(1, "codepoint", "int"), (2, "encoding", "?string")]),
    ("mb_convert_case", &[(1, "string", "string"), (2, "mode", "int"), (3, "encoding", "?string")]),
    ("mb_convert_encoding", &[(1, "string", "array|string"), (2, "to_encoding", "string")]),
    ("mb_convert_kana", &[(1, "string", "string"), (2, "mode", "string"), (3, "encoding", "?string")]),
    ("mb_detect_encoding", &[(1, "string", "string"), (3, "strict", "bool")]),
    ("mb_internal_encoding", &[(1, "encoding", "?string")]),
    ("mb_lcfirst", &[(1, "string", "string"), (2, "encoding", "?string")]),
    ("mb_ord", &[(1, "string", "string"), (2, "encoding", "?string")]),
    ("mb_scrub", &[(1, "string", "string"), (2, "encoding", "?string")]),
    ("mb_split", &[(1, "pattern", "string"), (2, "string", "string"), (3, "limit", "int")]),
    ("mb_str_pad", &[(1, "string", "string"), (2, "length", "int"), (3, "pad_string", "string"), (4, "pad_type", "int"), (5, "encoding", "?string")]),
    ("mb_str_split", &[(1, "string", "string"), (2, "length", "int"), (3, "encoding", "?string")]),
    ("mb_strcut", &[(1, "string", "string"), (2, "start", "int"), (3, "length", "?int"), (4, "encoding", "?string")]),
    ("mb_stripos", &[(1, "haystack", "string"), (2, "needle", "string"), (3, "offset", "int"), (4, "encoding", "?string")]),
    ("mb_strlen", &[(1, "string", "string"), (2, "encoding", "?string")]),
    ("mb_strpos", &[(1, "haystack", "string"), (2, "needle", "string"), (3, "offset", "int"), (4, "encoding", "?string")]),
    ("mb_strripos", &[(1, "haystack", "string"), (2, "needle", "string"), (3, "offset", "int"), (4, "encoding", "?string")]),
    ("mb_strrpos", &[(1, "haystack", "string"), (2, "needle", "string"), (3, "offset", "int"), (4, "encoding", "?string")]),
    ("mb_strtolower", &[(1, "string", "string"), (2, "encoding", "?string")]),
    ("mb_strtoupper", &[(1, "string", "string"), (2, "encoding", "?string")]),
    ("mb_strwidth", &[(1, "string", "string"), (2, "encoding", "?string")]),
    ("mb_substr", &[(1, "string", "string"), (2, "start", "int"), (3, "length", "?int"), (4, "encoding", "?string")]),
    ("mb_substr_count", &[(1, "haystack", "string"), (2, "needle", "string"), (3, "encoding", "?string")]),
    ("mb_ucfirst", &[(1, "string", "string"), (2, "encoding", "?string")]),
    ("md5", &[(1, "string", "string"), (2, "binary", "bool")]),
    ("md5_file", &[(1, "filename", "string"), (2, "binary", "bool")]),
    ("memory_get_peak_usage", &[(1, "real_usage", "bool")]),
    ("memory_get_usage", &[(1, "real_usage", "bool")]),
    ("metaphone", &[(1, "string", "string"), (2, "max_phonemes", "int")]),
    ("method_exists", &[(2, "method", "string")]),
    ("microtime", &[(1, "as_float", "bool")]),
    ("mkdir", &[(1, "directory", "string"), (2, "permissions", "int"), (3, "recursive", "bool")]),
    ("mktime", &[(1, "hour", "int"), (2, "minute", "?int"), (3, "second", "?int"), (4, "month", "?int"), (5, "day", "?int"), (6, "year", "?int")]),
    ("mt_rand", &[(1, "min", "int"), (2, "max", "int")]),
    ("mt_srand", &[(1, "seed", "?int"), (2, "mode", "int")]),
    ("nl2br", &[(1, "string", "string"), (2, "use_xhtml", "bool")]),
    ("number_format", &[(1, "num", "int|float"), (2, "decimals", "int"), (3, "decimal_separator", "?string"), (4, "thousands_separator", "?string")]),
    ("ob_start", &[(2, "chunk_size", "int"), (3, "flags", "int")]),
    ("octdec", &[(1, "octal_string", "string")]),
    ("ord", &[(1, "character", "string")]),
    ("parse_str", &[(1, "string", "string")]),
    ("parse_url", &[(1, "url", "string"), (2, "component", "int")]),
    ("pathinfo", &[(1, "path", "string"), (2, "flags", "int")]),
    ("php_uname", &[(1, "mode", "string")]),
    ("phpversion", &[(1, "extension", "?string")]),
    ("preg_grep", &[(1, "pattern", "string"), (2, "array", "array"), (3, "flags", "int")]),
    ("preg_match", &[(1, "pattern", "string"), (2, "subject", "string"), (4, "flags", "int"), (5, "offset", "int")]),
    ("preg_match_all", &[(1, "pattern", "string"), (2, "subject", "string"), (4, "flags", "int"), (5, "offset", "int")]),
    ("preg_quote", &[(1, "str", "string"), (2, "delimiter", "?string")]),
    ("preg_replace", &[(1, "pattern", "array|string"), (2, "replacement", "array|string"), (3, "subject", "array|string"), (4, "limit", "int")]),
    ("preg_replace_callback", &[(1, "pattern", "array|string"), (3, "subject", "array|string"), (4, "limit", "int"), (6, "flags", "int")]),
    ("preg_split", &[(1, "pattern", "string"), (2, "subject", "string"), (3, "limit", "int"), (4, "flags", "int")]),
    ("print_r", &[(2, "return", "bool")]),
    ("printf", &[(1, "format", "string")]),
    ("property_exists", &[(2, "property", "string")]),
    ("putenv", &[(1, "assignment", "string")]),
    ("quoted_printable_decode", &[(1, "string", "string")]),
    ("quoted_printable_encode", &[(1, "string", "string")]),
    ("quotemeta", &[(1, "string", "string")]),
    ("rad2deg", &[(1, "num", "float")]),
    ("rand", &[(1, "min", "int"), (2, "max", "int")]),
    ("random_bytes", &[(1, "length", "int")]),
    ("random_int", &[(1, "min", "int"), (2, "max", "int")]),
    ("range", &[(1, "start", "string|int|float"), (2, "end", "string|int|float"), (3, "step", "int|float")]),
    ("rawurldecode", &[(1, "string", "string")]),
    ("rawurlencode", &[(1, "string", "string")]),
    ("readfile", &[(1, "filename", "string"), (2, "use_include_path", "bool")]),
    ("realpath", &[(1, "path", "string")]),
    ("rename", &[(1, "from", "string"), (2, "to", "string")]),
    ("rmdir", &[(1, "directory", "string")]),
    ("round", &[(1, "num", "int|float"), (2, "precision", "int")]),
    ("rsort", &[(2, "flags", "int")]),
    ("rtrim", &[(1, "string", "string"), (2, "characters", "string")]),
    ("scandir", &[(1, "directory", "string"), (2, "sorting_order", "int")]),
    ("set_error_handler", &[(2, "error_levels", "int")]),
    ("set_time_limit", &[(1, "seconds", "int")]),
    ("settype", &[(2, "type", "string")]),
    ("sha1", &[(1, "string", "string"), (2, "binary", "bool")]),
    ("sha1_file", &[(1, "filename", "string"), (2, "binary", "bool")]),
    ("similar_text", &[(1, "string1", "string"), (2, "string2", "string")]),
    ("sin", &[(1, "num", "float")]),
    ("sinh", &[(1, "num", "float")]),
    ("sizeof", &[(2, "mode", "int")]),
    ("sleep", &[(1, "seconds", "int")]),
    ("sort", &[(2, "flags", "int")]),
    ("soundex", &[(1, "string", "string")]),
    ("spl_autoload_register", &[(2, "throw", "bool"), (3, "prepend", "bool")]),
    ("sprintf", &[(1, "format", "string")]),
    ("sqrt", &[(1, "num", "float")]),
    ("srand", &[(1, "seed", "?int"), (2, "mode", "int")]),
    ("sscanf", &[(1, "string", "string"), (2, "format", "string")]),
    ("stat", &[(1, "filename", "string")]),
    ("str_contains", &[(1, "haystack", "string"), (2, "needle", "string")]),
    ("str_ends_with", &[(1, "haystack", "string"), (2, "needle", "string")]),
    ("str_getcsv", &[(1, "string", "string"), (2, "separator", "string"), (3, "enclosure", "string"), (4, "escape", "string")]),
    ("str_ireplace", &[(1, "search", "array|string"), (2, "replace", "array|string"), (3, "subject", "array|string")]),
    ("str_pad", &[(1, "string", "string"), (2, "length", "int"), (3, "pad_string", "string"), (4, "pad_type", "int")]),
    ("str_repeat", &[(1, "string", "string"), (2, "times", "int")]),
    ("str_replace", &[(1, "search", "array|string"), (2, "replace", "array|string"), (3, "subject", "array|string")]),
    ("str_rot13", &[(1, "string", "string")]),
    ("str_split", &[(1, "string", "string"), (2, "length", "int")]),
    ("str_starts_with", &[(1, "haystack", "string"), (2, "needle", "string")]),
    ("str_word_count", &[(1, "string", "string"), (2, "format", "int"), (3, "characters", "?string")]),
    ("strcasecmp", &[(1, "string1", "string"), (2, "string2", "string")]),
    ("strchr", &[(1, "haystack", "string"), (2, "needle", "string"), (3, "before_needle", "bool")]),
    ("strcmp", &[(1, "string1", "string"), (2, "string2", "string")]),
    ("strcspn", &[(1, "string", "string"), (2, "characters", "string"), (3, "offset", "int"), (4, "length", "?int")]),
    ("stream_get_contents", &[(2, "length", "?int"), (3, "offset", "int")]),
    ("strip_tags", &[(1, "string", "string")]),
    ("stripcslashes", &[(1, "string", "string")]),
    ("stripos", &[(1, "haystack", "string"), (2, "needle", "string"), (3, "offset", "int")]),
    ("stripslashes", &[(1, "string", "string")]),
    ("stristr", &[(1, "haystack", "string"), (2, "needle", "string"), (3, "before_needle", "bool")]),
    ("strlen", &[(1, "string", "string")]),
    ("strnatcasecmp", &[(1, "string1", "string"), (2, "string2", "string")]),
    ("strnatcmp", &[(1, "string1", "string"), (2, "string2", "string")]),
    ("strncasecmp", &[(1, "string1", "string"), (2, "string2", "string"), (3, "length", "int")]),
    ("strncmp", &[(1, "string1", "string"), (2, "string2", "string"), (3, "length", "int")]),
    ("strpbrk", &[(1, "string", "string"), (2, "characters", "string")]),
    ("strpos", &[(1, "haystack", "string"), (2, "needle", "string"), (3, "offset", "int")]),
    ("strrchr", &[(1, "haystack", "string"), (2, "needle", "string"), (3, "before_needle", "bool")]),
    ("strrev", &[(1, "string", "string")]),
    ("strripos", &[(1, "haystack", "string"), (2, "needle", "string"), (3, "offset", "int")]),
    ("strrpos", &[(1, "haystack", "string"), (2, "needle", "string"), (3, "offset", "int")]),
    ("strspn", &[(1, "string", "string"), (2, "characters", "string"), (3, "offset", "int"), (4, "length", "?int")]),
    ("strstr", &[(1, "haystack", "string"), (2, "needle", "string"), (3, "before_needle", "bool")]),
    ("strtok", &[(1, "string", "string"), (2, "token", "?string")]),
    ("strtolower", &[(1, "string", "string")]),
    ("strtotime", &[(1, "datetime", "string"), (2, "baseTimestamp", "?int")]),
    ("strtoupper", &[(1, "string", "string")]),
    ("strtr", &[(1, "string", "string"), (2, "from", "array|string"), (3, "to", "?string")]),
    ("substr", &[(1, "string", "string"), (2, "offset", "int"), (3, "length", "?int")]),
    ("substr_compare", &[(1, "haystack", "string"), (2, "needle", "string"), (3, "offset", "int"), (4, "length", "?int"), (5, "case_insensitive", "bool")]),
    ("substr_count", &[(1, "haystack", "string"), (2, "needle", "string"), (3, "offset", "int"), (4, "length", "?int")]),
    ("substr_replace", &[(1, "string", "array|string"), (2, "replace", "array|string")]),
    ("tan", &[(1, "num", "float")]),
    ("tanh", &[(1, "num", "float")]),
    ("tempnam", &[(1, "directory", "string"), (2, "prefix", "string")]),
    ("time_nanosleep", &[(1, "seconds", "int"), (2, "nanoseconds", "int")]),
    ("touch", &[(1, "filename", "string"), (2, "mtime", "?int"), (3, "atime", "?int")]),
    ("trait_exists", &[(1, "trait", "string"), (2, "autoload", "bool")]),
    ("trigger_error", &[(1, "message", "string"), (2, "error_level", "int")]),
    ("trim", &[(1, "string", "string"), (2, "characters", "string")]),
    ("ucfirst", &[(1, "string", "string")]),
    ("ucwords", &[(1, "string", "string"), (2, "separators", "string")]),
    ("uniqid", &[(1, "prefix", "string"), (2, "more_entropy", "bool")]),
    ("unlink", &[(1, "filename", "string")]),
    ("unserialize", &[(1, "data", "string"), (2, "options", "array")]),
    ("urldecode", &[(1, "string", "string")]),
    ("urlencode", &[(1, "string", "string")]),
    ("user_error", &[(1, "message", "string"), (2, "error_level", "int")]),
    ("usleep", &[(1, "microseconds", "int")]),
    ("utf8_decode", &[(1, "string", "string")]),
    ("utf8_encode", &[(1, "string", "string")]),
    ("var_export", &[(2, "return", "bool")]),
    ("vfprintf", &[(2, "format", "string"), (3, "values", "array")]),
    ("vprintf", &[(1, "format", "string"), (2, "values", "array")]),
    ("vsprintf", &[(1, "format", "string"), (2, "values", "array")]),
    ("wordwrap", &[(1, "string", "string"), (2, "width", "int"), (3, "break", "string"), (4, "cut_long_words", "bool")]),
];
