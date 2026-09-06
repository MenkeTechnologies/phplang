//! PHP standard-library `reflection` functions. Part of the `stdlib` chain; see
//! `src/stdlib/mod.rs`. `dispatch` returns `None` for names it does not handle.
//!
//! These mirror PHP 8's class/object introspection builtins on top of the host
//! reflection helpers (`class_exists`, `class_parent`, `is_a_class`,
//! `class_method_names`, `class_has_method`, `class_has_prop`, `object_props`,
//! `object_class`). The runtime has no interfaces, traits, or enums, so
//! There is no calling-scope context available here, so the no-argument form of
//! `get_parent_class` (which PHP resolves to the current class) is unsupported
//! and reported as `false`.
//!
//! Two PHP enumerators are limited by what the host exposes. `get_declared_classes`
//! and `get_class_vars` cannot reach the private class table / property-default
//! chunks, so they degrade gracefully (empty array; `false` for an unknown class)
//! rather than fabricate data. `get_defined_constants` has no reachable constants
//! iterator at all — the table exposes only single-name accessors — so it is not
//! handled by this module (a call falls through as an undefined function).

use crate::host::{with_host, TypeKind};
use fusevm::Value;

use super::common::{arg, str_arg, throws};

/// The `TypeError` a single-`$object`-parameter reflection function raises for a
/// non-object argument.
///
/// The type is named by [`crate::host::PhpHost::type_name_for_error`], not by
/// `get_debug_type()`: `get_class(true)` reports `true given`, not `bool given`.
fn object_arg_type_error(func: &str, v: &Value) -> String {
    let t = with_host(|h| h.type_name_for_error(v));
    throws(
        "TypeError",
        format!("{func}(): Argument #1 ($object) must be of type object, {t} given"),
    )
}

/// Dispatch a `reflection`-category PHP function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let v = match name {
        // ── existence predicates ────────────────────────────────────────────
        // `class_exists($name, $autoload = true)`: the autoload flag is ignored
        // (nothing to autoload in this runtime). Case-insensitive lookup.
        // Each of the four answers for exactly ONE declaration kind: an
        // `interface I {}` makes `interface_exists('I')` true and
        // `class_exists('I')` FALSE, and an `enum E {}` is an enum, not a class.
        "class_exists" | "interface_exists" | "trait_exists" | "enum_exists" => {
            let n = str_arg(args, 0);
            with_host(|h| {
                let kind = h.type_kind(&n);
                // An enum is a class too: `enum E {}` answers yes to BOTH
                // `class_exists('E')` and `enum_exists('E')` in the reference.
                Value::bool(match name {
                    "class_exists" => matches!(kind, Some(TypeKind::Class | TypeKind::Enum)),
                    "interface_exists" => kind == Some(TypeKind::Interface),
                    "trait_exists" => kind == Some(TypeKind::Trait),
                    _ => kind == Some(TypeKind::Enum),
                })
            })
        }

        // ── member existence ────────────────────────────────────────────────
        // `method_exists($object_or_class, $method)`. First arg may be an object
        // handle or a class-name string.
        "method_exists" => {
            let a = arg(args, 0);
            // `object|string` is DECLARED on the parameter, so anything else is
            // a TypeError rather than a `false` — an int, a null or an array all
            // reach it, and each is named by what it is.
            if !matches!(a, Value::Str(_)) && !with_host(|h| h.is_object_value(&a)) {
                let t = with_host(|h| h.type_name_for_error(&a));
                return Some(Err(throws(
                    "TypeError",
                    format!(
                        "method_exists(): Argument #1 ($object_or_class) must be of type object|string, {t} given"
                    ),
                )));
            }
            let method = str_arg(args, 1);
            with_host(|h| {
                let class = h.instance_class(&a).unwrap_or_else(|| h.to_str(&a));
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
        // no-argument form (PHP: the enclosing class) is unsupported here.
        //
        // A non-object argument is a `TypeError`. Returning `false` was PHP 7's
        // answer; PHP 8 made every non-object a hard rejection, so the old
        // return value is stale rather than invented.
        //
        // Passing NOTHING is a different case from passing `null`, and the two
        // must not share a path: no argument means the ENCLOSING class (with a
        // deprecation since PHP 8.3), while an explicit `null` is just a
        // non-object and gets the `TypeError`. Collapsing them would answer
        // `null given` to a call that named no argument at all.
        "get_class" if args.is_empty() => {
            let cls = with_host(|h| h.magic_class());
            if cls.is_empty() {
                return Some(Err(crate::builtins::throws_bare(
                    "Error",
                    "get_class() without arguments must be called from within a class",
                )));
            }
            with_host(|h| h.deprecated("Calling get_class() without arguments is deprecated"));
            Value::str(cls)
        }
        "get_class" => {
            let a = arg(args, 0);
            match with_host(|h| h.instance_class(&a)) {
                Some(c) => Value::str(c),
                None => return Some(Err(object_arg_type_error("get_class", &a))),
            }
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
        // associative array (insertion order). A non-object is a `TypeError`,
        // for the same reason `get_class` is — see there.
        "get_object_vars" => {
            let a = arg(args, 0);
            if !with_host(|h| h.is_object_value(&a)) {
                return Some(Err(object_arg_type_error("get_object_vars", &a)));
            }
            with_host(|h| {
                // Only the properties the *calling scope* may see: public ones
                // from outside the class, everything from inside a method of it.
                let arr = h.new_array();
                for (k, val) in h.object_props_visible(&a) {
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
                let class = h.instance_class(&a).unwrap_or_else(|| h.to_str(&a));
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
                let Some(start) = resolve_named_class_warn(h, &a, "class_parents") else {
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
            with_host(
                |h| match resolve_named_class_warn(h, &a, "class_implements") {
                    Some(c) => {
                        let arr = h.new_array();
                        for i in h.class_interface_names(&c) {
                            h.arr_set_key(&arr, &Value::str(i.clone()), Value::str(i));
                        }
                        arr
                    }
                    None => Value::bool(false),
                },
            )
        }
        // `class_uses($object_or_class, $autoload = true)`: the traits THIS
        // class composes with `use` — PHP does not walk the parent chain for it.
        // `false` for a non-declared subject.
        "class_uses" => {
            let a = arg(args, 0);
            with_host(|h| match resolve_named_class_warn(h, &a, "class_uses") {
                Some(c) => {
                    let arr = h.new_array();
                    for t in h.class_trait_names(&c) {
                        h.arr_set_key(&arr, &Value::str(t.clone()), Value::str(t));
                    }
                    arr
                }
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
        // `get_declared_classes()`: the names of every declared class (lowercased,
        // as the host stores them), via the host enumerator.
        "get_declared_classes" => with_host(|h| {
            let names: Vec<Value> = h.all_class_names().into_iter().map(Value::str).collect();
            let arr = h.new_array();
            for n in names {
                h.arr_push_auto(&arr, n);
            }
            arr
        }),

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
    if let Some(c) = h.instance_class(a) {
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
    if let Some(c) = h.instance_class(a) {
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

/// [`resolve_named_class`] plus the warning the reference raises when the name
/// does not resolve: `class_parents`/`class_implements`/`class_uses` all say
/// `<fn>(): Class <name> does not exist and could not be loaded` before
/// returning `false`.
fn resolve_named_class_warn(h: &mut crate::host::PhpHost, a: &Value, func: &str) -> Option<String> {
    if let Some(c) = resolve_named_class(h, a) {
        return Some(c);
    }
    let name = h.to_str(a);
    h.warn(format!(
        "{func}(): Class {name} does not exist and could not be loaded"
    ));
    None
}
