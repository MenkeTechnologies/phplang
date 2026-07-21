//! The extended PHP standard library, one module per category. Each module
//! exposes `dispatch(name, args) -> Option<Result<Value, String>>`, returning
//! `None` when the name is not one of its functions. `builtins::call_library`
//! consults this chain for every name its own core match does not handle, so
//! categories can be developed independently without touching a shared table.
//!
//! Functions reach the runtime through `crate::host::with_host` and produce a
//! `fusevm::Value`; behavior mirrors PHP 8 (verified against the reference `php`
//! where possible). Vetted crates supply the primitives that must not be
//! hand-rolled: `regex` (preg), `chrono` (date), `md-5`/`sha1`/`sha2`/`crc32fast`
//! (hashing), `base64`/`hex`/`urlencoding` (encoding).

use fusevm::Value;

pub mod arrays;
pub mod bcmath;
pub mod callable;
pub mod constants;
pub mod ctype;
pub mod datefn;
pub mod hashext;
pub mod runtime;
pub mod textx;
pub mod datetime;
pub mod encoding;
pub mod fileio;
pub mod fileres;
pub mod filter;
pub mod gmp;
pub mod hash;
pub mod json;
pub mod math;
pub mod mbstring;
pub mod misc;
pub mod preg;
pub mod reflection;
pub mod strings;
pub mod system;
pub mod types;
pub mod url;

/// Try each category in turn; the first that recognizes `name` wins.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    strings::dispatch(name, args)
        .or_else(|| arrays::dispatch(name, args))
        .or_else(|| math::dispatch(name, args))
        .or_else(|| ctype::dispatch(name, args))
        .or_else(|| types::dispatch(name, args))
        .or_else(|| preg::dispatch(name, args))
        .or_else(|| datetime::dispatch(name, args))
        .or_else(|| datefn::dispatch(name, args))
        .or_else(|| hash::dispatch(name, args))
        .or_else(|| encoding::dispatch(name, args))
        .or_else(|| url::dispatch(name, args))
        .or_else(|| json::dispatch(name, args))
        .or_else(|| fileio::dispatch(name, args))
        .or_else(|| reflection::dispatch(name, args))
        .or_else(|| callable::dispatch(name, args))
        .or_else(|| filter::dispatch(name, args))
        .or_else(|| mbstring::dispatch(name, args))
        .or_else(|| misc::dispatch(name, args))
        .or_else(|| constants::dispatch(name, args))
        .or_else(|| system::dispatch(name, args))
        .or_else(|| fileres::dispatch(name, args))
        .or_else(|| gmp::dispatch(name, args))
        .or_else(|| bcmath::dispatch(name, args))
        .or_else(|| hashext::dispatch(name, args))
        .or_else(|| textx::dispatch(name, args))
        .or_else(|| runtime::dispatch(name, args))
}

/// Shared helpers for the category modules. Each helper is used by at least one
/// category once filled in; `allow(dead_code)` keeps the scaffold warning-free
/// before those modules land.
#[allow(dead_code)]
pub(crate) mod common {
    use crate::host::with_host;
    use fusevm::Value;

    /// The `i`-th argument, or `Undef` (PHP: a missing argument reads as null).
    pub fn arg(args: &[Value], i: usize) -> Value {
        args.get(i).cloned().unwrap_or(Value::Undef)
    }

    /// PHP string cast of the `i`-th argument.
    pub fn str_arg(args: &[Value], i: usize) -> String {
        with_host(|h| h.to_str(&arg(args, i)))
    }

    /// PHP integer cast of the `i`-th argument.
    pub fn int_arg(args: &[Value], i: usize) -> i64 {
        with_host(|h| h.to_number(&arg(args, i)).to_int())
    }

    /// PHP float cast of the `i`-th argument.
    pub fn float_arg(args: &[Value], i: usize) -> f64 {
        with_host(|h| h.to_number(&arg(args, i)).to_float())
    }

    /// Build a PHP array (list) from an ordered value list.
    pub fn make_list(vals: Vec<Value>) -> Value {
        with_host(|h| {
            let arr = h.new_array();
            for v in vals {
                h.arr_push_auto(&arr, v);
            }
            arr
        })
    }

    /// Build a PHP array from `(key, value)` pairs, preserving keys.
    pub fn make_map(pairs: Vec<(Value, Value)>) -> Value {
        with_host(|h| {
            let arr = h.new_array();
            for (k, v) in pairs {
                h.arr_set_key(&arr, &k, v);
            }
            arr
        })
    }
}
