//! PHP standard-library `callable` functions. Part of the `stdlib` chain; see
//! `src/stdlib/mod.rs`. `dispatch` returns `None` for names it does not handle.
//!
//! These functions invoke user-supplied callables through
//! `crate::host::call_value`, which accepts either a closure handle or a
//! function-name string (`call_user_func("strtoupper", "hi") === "HI"`).

use crate::host::{call_value, call_value_named, with_host};
use crate::stdlib::common::*;
use fusevm::Value;

/// Every function name this build implements, derived from [`crate::corpus`] —
/// the same table `gen-docs` renders and the LSP completes from, and the one
/// place a new stdlib function is already required to be registered.
///
/// It replaces a hand-maintained list, which had drifted 195 names behind the
/// dispatcher: `bcadd`, `array_multisort`, `class_implements`, `debug_backtrace`
/// and 191 others all ran correctly while `function_exists` denied them. A
/// hand-written second registry cannot help but drift, so there is no longer
/// one — adding a function to the corpus (already mandatory) is what makes it
/// visible here.
///
/// Chapters that do not describe callable functions are excluded. Names the
/// corpus documents as absent from reference PHP are KEPT here — this set is
/// what this build dispatches, and `(object)`/`(array)` casts lower to
/// `__cast_object`/`__cast_array` calls that must resolve. It is
/// [`function_resolves`] that subtracts them, because that one answers the
/// different question of what a PHP program may see.
fn known_builtins() -> &'static std::collections::HashSet<&'static str> {
    use std::sync::OnceLock;
    static SET: OnceLock<std::collections::HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| {
        // Keywords, operators, casts, constants, classes and the magic methods
        // a program declares rather than calls: none is a function name.
        const NOT_FUNCTIONS: &[&str] = &[
            "Predefined constant",
            "Keyword",
            "Operator",
            "Prelude class",
            "Language construct",
            "Magic method",
            "Cast",
            "Built-in object methods",
        ];
        let mut set: std::collections::HashSet<&'static str> = crate::corpus::CORPUS
            .iter()
            .filter(|(_, chapter, ..)| !NOT_FUNCTIONS.contains(chapter))
            .map(|(name, ..)| *name)
            .collect();
        // `exit`/`die` are spelled as language constructs and live in the
        // corpus under that chapter, but PHP 8.4 turned them into real
        // functions and the reference reports them as such.
        set.insert("exit");
        set.insert("die");
        set
    })
}

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
/// never disagree about a name; see [`known_builtins`].
pub(crate) fn function_resolves(name: &str) -> bool {
    let lname = name.to_ascii_lowercase();
    // Implemented and dispatchable, but the corpus entry for each says in so
    // many words that reference PHP has no such function. A PHP program must not
    // see them, so `function_exists`/`is_callable` deny them exactly as the
    // reference does — while `dispatches` still lets the engine call them.
    const NOT_IN_PHP: &[&str] = &["__cast_array", "__cast_object", "gmp_pow2"];
    !NOT_IN_PHP.contains(&lname.as_str()) && dispatches(&lname)
}

/// Whether a name reaches an implementation AT ALL — a user function or any
/// builtin in [`known_builtins`], the compiler's internal cast targets included.
///
/// This is what a call site asks before it evaluates arguments; [`function_resolves`]
/// is what a PHP program asks, and the two differ only by the handful of names
/// that exist here but not in the reference.
pub(crate) fn dispatches(name: &str) -> bool {
    with_host(|h| h.function_defined(name))
        || known_builtins().contains(name.to_ascii_lowercase().as_str())
        // A `rust { … }` block's exports are registered at RUN time and are
        // callable by bareword, so they are in none of the tables above —
        // `call_function_dispatched` consults this same registry.
        || fusevm::ffi::is_registered(name)
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

        // call_user_func_array(callable, array): unpack the array and invoke.
        // An INTEGER key contributes a positional argument, in order; a STRING
        // key contributes a NAMED one, bound to the parameter it spells — the
        // same reading `f(...$array)` gives, and the reason
        // `call_user_func_array("g", ["b" => 2, "a" => 1])` fills `$a` with `1`.
        // Taking every value positionally filled it with `2`. A non-array second
        // argument yields no arguments.
        "call_user_func_array" => {
            let callee = arg(args, 0);
            let arr = arg(args, 1);
            let (mut pos, mut named) = (Vec::new(), Vec::new());
            for (k, v) in with_host(|h| h.array_pairs(&arr)).unwrap_or_default() {
                match k {
                    Value::Str(name) => named.push((name.to_string(), v)),
                    _ => pos.push(v),
                }
            }
            Some(call_value_named(callee, pos, named))
        }

        // is_callable(value): whether the value names something this engine could
        // actually invoke — see `callable_resolves`.
        "is_callable" => Some(Ok(Value::bool(callable_resolves(&arg(args, 0))))),

        // function_exists(name): true for a defined user function, or for any
        // builtin this build implements that the reference also has. It answers
        // from `function_resolves` rather than reading the tables itself, so it
        // cannot drift from `is_callable`, which reads the same predicate.
        "function_exists" => Some(Ok(Value::bool(function_resolves(&str_arg(args, 0))))),

        _ => None,
    }
}
