//! PHP standard-library `reflection` functions. Part of the `stdlib` chain; see
//! `src/stdlib/mod.rs`. `dispatch` returns `None` for names it does not handle.
//!
//! These mirror PHP 8's class/object introspection builtins on top of the host
//! reflection helpers (`class_exists`, `class_parent`, `is_a_class`,
//! `class_method_names`, `class_has_method`, `class_has_prop`, `object_props`,
//! `object_class`). The runtime has no interfaces, traits, or enums, so
//! `interface_exists`/`trait_exists`/`enum_exists` are always `false`, and
//! `class_implements`/`class_uses` return an empty array for any declared
//! class/object. There is no calling-scope context available here, so the
//! no-argument forms of `get_class`/`get_parent_class` (which PHP resolves to the
//! current class) are unsupported and reported as `false`.
//!
//! Two PHP enumerators are limited by what the host exposes. `get_declared_classes`
//! and `get_class_vars` cannot reach the private class table / property-default
//! chunks, so they degrade gracefully (empty array; `false` for an unknown class)
//! rather than fabricate data. `get_defined_constants` has no reachable constants
//! iterator at all — the table exposes only single-name accessors — so it is not
//! handled by this module (a call falls through as an undefined function).

use crate::host::with_host;
use fusevm::Value;

use super::common::{arg, str_arg};

/// Dispatch a `reflection`-category PHP function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let v = match name {
        // ── existence predicates ────────────────────────────────────────────
        // `class_exists($name, $autoload = true)`: the autoload flag is ignored
        // (nothing to autoload in this runtime). Case-insensitive lookup.
        "class_exists" => {
            let n = str_arg(args, 0);
            with_host(|h| Value::bool(h.class_exists(&n)))
        }
        // No interfaces, traits, or enums exist in this runtime — always false.
        "interface_exists" | "trait_exists" | "enum_exists" => Value::bool(false),

        // ── member existence ────────────────────────────────────────────────
        // `method_exists($object_or_class, $method)`. First arg may be an object
        // handle or a class-name string.
        "method_exists" => {
            let a = arg(args, 0);
            let method = str_arg(args, 1);
            with_host(|h| {
                let class = h.object_class(&a).unwrap_or_else(|| h.to_str(&a));
                Value::bool(h.class_has_method(&class, &method))
            })
        }
        // `property_exists($object_or_class, $property)`. For an object, dynamic
        // (runtime-added) properties count in addition to declared ones.
        "property_exists" => {
            let a = arg(args, 0);
            let prop = str_arg(args, 1);
            with_host(|h| {
                if h.is_object(&a) {
                    let class = h.object_class(&a).unwrap_or_default();
                    let declared = h.class_has_prop(&class, &prop);
                    let dynamic = h.object_props(&a).iter().any(|(n, _)| *n == prop);
                    Value::bool(declared || dynamic)
                } else {
                    let class = h.to_str(&a);
                    Value::bool(h.class_has_prop(&class, &prop))
                }
            })
        }

        // ── class identity ──────────────────────────────────────────────────
        // `get_class($object)`: the object's class name (original casing). The
        // no-argument form (PHP: the enclosing class) is unsupported here; a
        // non-object argument yields `false`, mirroring PHP's failure result
        // rather than raising, since this module cannot throw a TypeError.
        "get_class" => {
            let a = arg(args, 0);
            with_host(|h| match h.object_class(&a) {
                Some(c) => Value::str(c),
                None => Value::bool(false),
            })
        }
        // `get_parent_class($object_or_class = null)`: parent name or `false`.
        // The no-argument form (enclosing class) is unsupported — returns false.
        "get_parent_class" => {
            let a = arg(args, 0);
            with_host(|h| {
                let class = if h.is_object(&a) {
                    h.object_class(&a)
                } else if matches!(a, Value::Undef) {
                    None
                } else {
                    Some(h.to_str(&a))
                };
                match class.and_then(|c| h.class_parent(&c)) {
                    Some(p) => Value::str(p),
                    None => Value::bool(false),
                }
            })
        }

        // ── member enumeration ──────────────────────────────────────────────
        // `get_object_vars($object)`: the object's accessible properties as an
        // associative array (insertion order). A non-object yields `false`.
        "get_object_vars" => {
            let a = arg(args, 0);
            with_host(|h| {
                if !h.is_object(&a) {
                    return Value::bool(false);
                }
                let arr = h.new_array();
                for (k, val) in h.object_props(&a) {
                    h.arr_set_key(&arr, &Value::str(k), val);
                }
                arr
            })
        }
        // `get_class_methods($object_or_class)`: method names, walking the parent
        // chain. NOTE: the host stores/returns method names lowercased, so the
        // returned names are lowercased rather than PHP's declared casing.
        "get_class_methods" => {
            let a = arg(args, 0);
            with_host(|h| {
                let class = h.object_class(&a).unwrap_or_else(|| h.to_str(&a));
                let arr = h.new_array();
                for m in h.class_method_names(&class) {
                    h.arr_push_auto(&arr, Value::str(m));
                }
                arr
            })
        }

        // ── inheritance tests ───────────────────────────────────────────────
        // `is_a($object_or_class, $class, $allow_string = false)`: true when the
        // subject is an instance of, or a subclass of, `$class`. When the subject
        // is a string and `$allow_string` is false, PHP returns false.
        "is_a" => {
            let a = arg(args, 0);
            let target = str_arg(args, 1);
            let allow_string = args.get(2).map(|v| with_host(|h| h.is_truthy(v)));
            with_host(|h| match class_of(h, &a, allow_string.unwrap_or(false)) {
                Some(class) => Value::bool(h.is_a_class(&class, &target)),
                None => Value::bool(false),
            })
        }
        // `is_subclass_of($object_or_class, $class, $allow_string = true)`: like
        // `is_a` but the subject's own class does not count — only true ancestors.
        "is_subclass_of" => {
            let a = arg(args, 0);
            let target = str_arg(args, 1);
            let allow_string = args.get(2).map(|v| with_host(|h| h.is_truthy(v)));
            with_host(|h| match class_of(h, &a, allow_string.unwrap_or(true)) {
                Some(class) => Value::bool(
                    !class.eq_ignore_ascii_case(&target) && h.is_a_class(&class, &target),
                ),
                None => Value::bool(false),
            })
        }

        // ── ancestry / composition enumeration ──────────────────────────────
        // `class_parents($object_or_class, $autoload = true)`: an associative
        // array `name => name` for each ancestor, nearest first, or `false` when
        // the subject is not a declared class/object. The autoload flag is
        // ignored (nothing to autoload). Names carry the parent's declared
        // casing, as PHP does.
        "class_parents" => {
            let a = arg(args, 0);
            with_host(|h| {
                let Some(start) = resolve_named_class(h, &a) else {
                    return Value::bool(false);
                };
                let arr = h.new_array();
                let mut cur = h.class_parent(&start);
                while let Some(p) = cur {
                    h.arr_set_key(&arr, &Value::str(p.clone()), Value::str(p.clone()));
                    cur = h.class_parent(&p);
                }
                arr
            })
        }
        // `class_implements($object_or_class, $autoload = true)`: PHP returns the
        // interfaces a class implements. This runtime has no interfaces, so the
        // result is an empty array for a valid class/object, or `false` when the
        // subject is not a declared class/object (matching PHP's failure result).
        "class_implements" => {
            let a = arg(args, 0);
            with_host(|h| match resolve_named_class(h, &a) {
                Some(_) => h.new_array(),
                None => Value::bool(false),
            })
        }
        // `class_uses($object_or_class, $autoload = true)`: the traits a class
        // uses. This runtime has no traits, so an empty array for a valid
        // class/object, or `false` for a non-declared subject.
        "class_uses" => {
            let a = arg(args, 0);
            with_host(|h| match resolve_named_class(h, &a) {
                Some(_) => h.new_array(),
                None => Value::bool(false),
            })
        }
        // `get_class_vars($class_name)`: PHP returns the default property values
        // of a class as an associative array, or `false` for an unknown class.
        // The property-default initializers are stored as compiled expression
        // chunks that only the (private) host instantiation path can evaluate;
        // no public host accessor exposes them, and instantiating to read them
        // back would run the constructor (wrong: defaults are the pre-construct
        // values). So a declared class yields an empty array here (documented
        // limitation) while an unknown class still returns `false`, keeping the
        // existence semantics correct.
        "get_class_vars" => {
            let name = str_arg(args, 0);
            with_host(|h| {
                if h.class_exists(&name) {
                    h.new_array()
                } else {
                    Value::bool(false)
                }
            })
        }
        // `get_declared_classes()`: PHP returns the names of every declared class.
        // The host class table is private with no public enumeration accessor, so
        // the names are not reachable from this module; a best-effort empty array
        // is returned rather than a fabricated list. (`get_defined_constants` is
        // likewise unreachable — the constants table exposes only single-name
        // `const_fetch`/`const_defined`/`const_define`, no iteration — so it is
        // intentionally not handled here and falls through as undefined.)
        "get_declared_classes" => with_host(|h| h.new_array()),

        _ => return None,
    };
    Some(Ok(v))
}

/// Resolve the subject of `is_a`/`is_subclass_of` to a class name. Objects use
/// their class; a string is only honored when `allow_string` is set (PHP
/// otherwise rejects a class-name string outright) AND it names a declared
/// class. The `class_exists` guard matters: without it a bare name equals itself
/// on the first step of the ancestry walk, so `is_a('Ghost', 'Ghost', true)`
/// would wrongly report `true` for a class that was never declared — PHP returns
/// `false` there.
fn class_of(h: &crate::host::PhpHost, a: &Value, allow_string: bool) -> Option<String> {
    if let Some(c) = h.object_class(a) {
        return Some(c);
    }
    match a {
        Value::Str(_) if allow_string => {
            let s = h.to_str(a);
            h.class_exists(&s).then_some(s)
        }
        _ => None,
    }
}

/// Resolve an `object|string` subject to a declared class name for the
/// `class_parents`/`class_implements`/`class_uses`/`get_class_vars` family.
/// Objects yield their class (always declared); a string is honored only when it
/// names a declared class, so a bad name produces the `false` these functions
/// return for an unknown class rather than a misleading empty result.
fn resolve_named_class(h: &crate::host::PhpHost, a: &Value) -> Option<String> {
    if let Some(c) = h.object_class(a) {
        return Some(c);
    }
    match a {
        Value::Str(_) => {
            let s = h.to_str(a);
            h.class_exists(&s).then_some(s)
        }
        _ => None,
    }
}
