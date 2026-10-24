//! PHP standard-library `reflection` functions. Part of the `stdlib` chain; see
//! `src/stdlib/mod.rs`. `dispatch` returns `None` for names it does not handle.
//!
//! These mirror PHP 8's class/object introspection builtins on top of the host
//! reflection helpers (`class_exists`, `class_parent`, `is_a_class`,
//! `class_method_names`, `class_has_method`, `property_exists`, `object_props`,
//! `object_class`). The runtime has no interfaces, traits, or enums, so
//! There is no calling-scope context available here, so the no-argument form of
//! `get_parent_class` (which PHP resolves to the current class) is unsupported
//! and reported as `false`.
//!
//! `get_defined_constants` has no reachable constants
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
    // A class NAME these look up is handed to the autoloaders first
    // (`zend_lookup_class`), unless the call says not to.
    if let Some(class) = autoload_subject(name, args) {
        if let Err(e) = crate::host::class_known(&class) {
            return Some(Err(e));
        }
    }
    let v = match name {
        "__phplang_reflect" => return Some(reflect(args)),
        // ── existence predicates ────────────────────────────────────────────
        // `class_exists($name, $autoload = true)`: a name not yet declared has
        // been handed to the autoloaders (see `autoload_subject`) unless
        // `$autoload` is false. Case-insensitive lookup.
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
                let class = h
                    .instance_class(&a)
                    .unwrap_or_else(|| crate::host::unanchored(&h.to_str(&a)).to_string());
                let lname = method.to_ascii_lowercase();
                Value::bool(
                    h.class_has_method(&class, &method) && !h.spl_array_forwarder(&class, &lname),
                )
            })
        }
        // `property_exists($object_or_class, $property)`: see
        // `PhpHost::property_exists` for which declarations count.
        "property_exists" => {
            let a = arg(args, 0);
            let prop = str_arg(args, 1);
            with_host(|h| Value::bool(h.property_exists(&a, &prop)))
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
        // `get_called_class()`: the late-static-binding class of the running
        // frame (`zend_get_called_scope`), the same answer as `static::class`.
        "get_called_class" => {
            let cls = with_host(|h| h.lsb_class(&h.magic_class()));
            if cls.is_empty() {
                return Some(Err(crate::builtins::throws_bare(
                    "Error",
                    "get_called_class() must be called from within a class",
                )));
            }
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
                    Some(crate::host::unanchored(&h.to_str(&a)).to_string())
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
                    h.arr_set_key(&arr, &crate::host::prop_key(&k), val);
                }
                arr
            })
        }
        // `get_class_methods($object_or_class)`: see `class_methods_visible`.
        "get_class_methods" => {
            let a = arg(args, 0);
            // `Z_PARAM_OBJ_OR_CLASS_NAME`: an object, or a string naming a
            // declared class.
            let named = with_host(|h| match &a {
                Value::Str(s) => h.class_exists(s),
                _ => h.instance_class(&a).is_some(),
            });
            if !named {
                let t = with_host(|h| h.type_name_for_error(&a));
                return Some(Err(throws(
                    "TypeError",
                    format!("get_class_methods(): Argument #1 ($object_or_class) must be an object or a valid class name, {t} given"),
                )));
            }
            with_host(|h| {
                let class = h.instance_class(&a).unwrap_or_else(|| h.to_str(&a));
                let arr = h.new_array();
                for m in h.class_methods_visible(&class) {
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
        // the subject is not a declared class/object. Names carry the parent's declared
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
        // `get_class_vars($class)`: the default instance properties, then the
        // statics at their current value, limited to what the calling scope can
        // see. An unknown class is a `TypeError`.
        "get_class_vars" => {
            let name = str_arg(args, 0);
            match crate::host::class_vars(&name) {
                Ok(Some(arr)) => arr,
                Ok(None) => {
                    return Some(Err(throws(
                        "TypeError",
                        format!("get_class_vars(): Argument #1 ($class) must be a valid class name, {name} given"),
                    )))
                }
                Err(e) => return Some(Err(e)),
            }
        }
        // `get_declared_classes()` and its siblings: one declaration kind, in
        // declared spelling and declaration order (`get_declared_class_impl`).
        "get_declared_classes" | "get_declared_interfaces" | "get_declared_traits" => {
            with_host(|h| {
                let kind = match name {
                    "get_declared_classes" => TypeKind::Class,
                    "get_declared_interfaces" => TypeKind::Interface,
                    _ => TypeKind::Trait,
                };
                let names: Vec<Value> =
                    h.declared_names(kind).into_iter().map(Value::str).collect();
                let arr = h.new_array();
                for n in names {
                    h.arr_push_auto(&arr, n);
                }
                arr
            })
        }

        _ => return None,
    };
    Some(Ok(v))
}

/// The class-name string `name`'s call looks up — and so autoloads — if any:
/// the first argument when it is a string and the function takes a class
/// there, gated by `$allow_string` for `is_a`/`is_subclass_of` and by
/// `$autoload` for the `class_*` family and the `*_exists` predicates.
fn autoload_subject(name: &str, args: &[Value]) -> Option<String> {
    let Some(Value::Str(class)) = args.first() else {
        return None;
    };
    let flag = |i: usize, default: bool| {
        args.get(i)
            .map_or(default, |v| with_host(|h| h.is_truthy(v)))
    };
    let looks_up = match name {
        "method_exists" | "property_exists" | "get_parent_class" | "get_class_methods"
        | "get_class_vars" => true,
        "is_a" => flag(2, false),
        "is_subclass_of" => flag(2, true),
        "class_exists" | "interface_exists" | "trait_exists" | "enum_exists" | "class_parents"
        | "class_implements" | "class_uses" => flag(1, true),
        _ => false,
    };
    looks_up.then(|| class.to_string())
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
            let s = crate::host::unanchored(&h.to_str(a)).to_string();
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
            let s = crate::host::unanchored(&h.to_str(a)).to_string();
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

// ── `__phplang_reflect`: the prelude's `Reflection*` classes ───────────────

use super::common::{make_list, make_map};

fn key(s: &str) -> Value {
    Value::str(s.to_string())
}

/// A declared type as the prelude builds a `ReflectionType` from it:
/// `[name => as printed, parts => the alternatives but null, null => allows
/// null]`, or `null` for no declared type.
fn type_info(ty: Option<&crate::ast::TypeHint>, self_class: &str) -> Value {
    let Some(ty) = ty else {
        return Value::Undef;
    };
    // `self` in a method names its class; outside one it stays `self`.
    let me = if self_class.is_empty() {
        "self"
    } else {
        self_class
    };
    let name = ty.declared(me, Some("parent"));
    let alts: Vec<&str> = name.trim_start_matches('?').split('|').collect();
    let parts: Vec<Value> = if alts == ["null"] {
        vec![key("null")]
    } else {
        alts.iter()
            .filter(|p| **p != "null")
            .map(|p| Value::str(p.to_string()))
            .collect()
    };
    let null = ty.nullable() || name == "mixed" || name == "null";
    make_map(vec![
        (key("name"), Value::str(name.clone())),
        (key("parts"), make_list(parts)),
        (key("null"), Value::bool(null)),
    ])
}

fn member_info(m: crate::host::ReflMember) -> Value {
    let ty = type_info(m.ty.as_ref(), &m.class);
    make_map(vec![
        (key("name"), Value::str(m.name)),
        (key("class"), Value::str(m.class)),
        (key("modifiers"), Value::Int(m.modifiers)),
        (key("type"), ty),
        (key("default"), Value::bool(m.has_default)),
    ])
}

/// One parameter: `[name, optional, variadic, byref, type, default]`.
fn param_rows(params: Vec<(String, bool, bool, bool, Value)>) -> Value {
    // `required_num_args`: up to the last parameter with no default.
    let required = params
        .iter()
        .rposition(|(_, has_default, variadic, ..)| !has_default && !variadic)
        .map_or(0, |i| i + 1);
    make_list(
        params
            .into_iter()
            .enumerate()
            .map(|(i, (name, has_default, variadic, by_ref, ty))| {
                make_map(vec![
                    (key("name"), Value::str(name)),
                    (key("optional"), Value::bool(i >= required)),
                    (key("variadic"), Value::bool(variadic)),
                    (key("byref"), Value::bool(by_ref)),
                    (key("type"), ty),
                    (key("default"), Value::bool(has_default)),
                ])
            })
            .collect(),
    )
}

/// What a `[kind, target, name]` function spec names: its display name,
/// the class its defaults run in, its user parameters (or a library
/// function's), and its return type.
enum Callee {
    User {
        class: String,
        params: Vec<crate::host::Param>,
    },
    Library(Vec<crate::argsig::LibParam>),
}

/// The attributes `getAttributes()` reads (see `host::attr_defs`), or a
/// parameter's when `param` is its position.
fn attr_defs_at(
    kind: &str,
    target: &Value,
    name: &str,
    param: i64,
) -> Option<(String, Vec<crate::host::AttrDef>)> {
    if param < 0 {
        return crate::host::attr_defs(kind, target, name);
    }
    match callee(kind, target, name)? {
        Callee::User { class, params } => params
            .into_iter()
            .nth(param as usize)
            .map(|p| (class, p.attrs)),
        Callee::Library(_) => Some((String::new(), Vec::new())),
    }
}

fn callee(kind: &str, target: &Value, name: &str) -> Option<Callee> {
    match kind {
        "method" => {
            let class = with_host(|h| h.to_str(target));
            let (class, params) = with_host(|h| h.refl_method_params(&class, name))?;
            Some(Callee::User { class, params })
        }
        "closure" => {
            let (_, params, _) = with_host(|h| h.refl_closure(target))?;
            Some(Callee::User {
                class: String::new(),
                params,
            })
        }
        _ => {
            let fname = with_host(|h| h.to_str(target));
            if let Some((_, params, _)) = with_host(|h| h.refl_user_function(&fname)) {
                return Some(Callee::User {
                    class: String::new(),
                    params,
                });
            }
            crate::argsig::library_params(&fname).map(Callee::Library)
        }
    }
}

/// A library parameter's default as a value, or `None` when the table does
/// not know it.
fn lib_default(d: crate::argsig::Def) -> Option<Value> {
    use crate::argsig::Def;
    Some(match d {
        Def::Required | Def::Unknown => return None,
        Def::Null => Value::Undef,
        Def::Bool(b) => Value::bool(b),
        Def::Int(n) => Value::Int(n),
        Def::Float(f) => Value::Float(f),
        Def::Str(s) => Value::str(s.to_string()),
        Def::EmptyArray => make_list(Vec::new()),
    })
}

/// `__phplang_reflect($op, ...)` — see the prelude's `Reflection*` classes.
pub(crate) fn reflect(args: &[Value]) -> Result<Value, String> {
    let op = str_arg(args, 0);
    let a1 = arg(args, 1);
    let s1 = str_arg(args, 1);
    let s2 = str_arg(args, 2);
    let s3 = str_arg(args, 3);
    Ok(match op.as_str() {
        "class" => {
            let name = with_host(|h| h.instance_class(&a1)).unwrap_or(s1);
            match with_host(|h| h.refl_class(&name)) {
                None => Value::bool(false),
                Some(c) => make_map(vec![
                    (key("name"), Value::str(c.name)),
                    (key("parent"), c.parent.map_or(Value::Undef, Value::str)),
                    (
                        key("interfaces"),
                        make_list(c.interfaces.into_iter().map(Value::str).collect()),
                    ),
                    (
                        key("traits"),
                        make_list(c.traits.into_iter().map(Value::str).collect()),
                    ),
                    (key("interface"), Value::bool(c.is_interface)),
                    (key("trait"), Value::bool(c.is_trait)),
                    (key("enum"), Value::bool(c.is_enum)),
                    (key("abstract"), Value::bool(c.is_abstract)),
                    (key("final"), Value::bool(c.is_final)),
                    (key("internal"), Value::bool(c.internal)),
                    (key("anonymous"), Value::bool(c.anonymous)),
                ]),
            }
        }
        "methods" => make_list(
            with_host(|h| h.refl_methods(&s1))
                .into_iter()
                .map(member_info)
                .collect(),
        ),
        "method" => with_host(|h| h.refl_method(&s1, &s2)).map_or(Value::Undef, member_info),
        "props" => make_list(
            with_host(|h| h.refl_props(&s1))
                .into_iter()
                .map(member_info)
                .collect(),
        ),
        "prop" => with_host(|h| h.refl_prop(&s1, &s2)).map_or(Value::Undef, member_info),
        "prop_default" => match with_host(|h| h.refl_prop_default(&s1, &s2)) {
            Some((class, chunk)) => crate::host::refl_eval_default(&class, chunk)?,
            None => Value::Undef,
        },
        "consts" => {
            let names = with_host(|h| h.refl_const_names(&s1));
            let mut pairs = Vec::with_capacity(names.len());
            for n in names {
                let v = crate::host::class_const(&s1, &n)?;
                pairs.push((Value::str(n), v));
            }
            make_map(pairs)
        }
        // `getAttributes()`: the names of a declaration's attributes, in
        // order — a parameter's when argument 4 is its position (else -1).
        "attrs" => make_list(
            attr_defs_at(&s1, &arg(args, 2), &s3, arg(args, 4).to_int())
                .map(|(_, defs)| defs.into_iter().map(|d| Value::str(d.name)).collect())
                .unwrap_or_default(),
        ),
        // `ReflectionAttribute::getArguments()`: attribute argument 5 of that
        // declaration's list, evaluated now, in the declaring class's scope.
        "attr_args" => {
            let found = attr_defs_at(&s1, &arg(args, 2), &s3, arg(args, 4).to_int());
            let index = arg(args, 5).to_int() as usize;
            match found.and_then(|(class, defs)| defs.into_iter().nth(index).map(|d| (class, d))) {
                Some((class, def)) => crate::host::refl_eval_default(&class, def.args)?,
                None => make_list(Vec::new()),
            }
        }
        // `getDocComment()`: see `host::doc_comment`.
        "doc" => {
            crate::host::doc_comment(&s1, &arg(args, 2), &s3).map_or(Value::bool(false), Value::str)
        }
        // `getStaticVariables`: a closure's is what its dump lists under
        // `static` — the captured variables, then its `static`s; a function's
        // or a method's, its `static`s.
        "statics" if args.len() > 2 => {
            let pairs = crate::host::method_statics(&s1, &s2);
            make_map(pairs.into_iter().map(|(n, v)| (Value::str(n), v)).collect())
        }
        "statics" if !with_host(|h| h.is_closure(&a1)) => make_map(
            crate::host::function_statics(&s1)
                .into_iter()
                .map(|(n, v)| (Value::str(n), v))
                .collect(),
        ),
        "statics" => {
            let info = crate::host::closure_debug_info(&a1);
            let st =
                info.map(|i| with_host(|h| h.index_get(&i, &Value::str("static".to_string()))));
            match st {
                Some(v) if !matches!(v, Value::Undef) => v,
                _ => make_map(Vec::new()),
            }
        }
        "function" => {
            let closure = with_host(|h| h.is_closure(&a1));
            let found = if closure {
                with_host(|h| h.refl_closure(&a1))
                    .map(|(n, _, ret)| (n, false, type_info(ret.as_ref(), "")))
            } else if let Some((n, _, ret)) = with_host(|h| h.refl_user_function(&s1)) {
                Some((n, false, type_info(ret.as_ref(), "")))
            } else if crate::stdlib::callable::function_resolves(&s1) {
                let ret = crate::argsig::return_type(&s1)
                    .map(|t| type_info(Some(&crate::ast::TypeHint { parts: t }), ""))
                    .unwrap_or(Value::Undef);
                Some((crate::host::unanchored(&s1).to_ascii_lowercase(), true, ret))
            } else {
                None
            };
            match found {
                None => Value::bool(false),
                Some((name, internal, ret)) => make_map(vec![
                    (key("name"), Value::str(name)),
                    (key("internal"), Value::bool(internal)),
                    (key("closure"), Value::bool(closure)),
                    (key("return"), ret),
                ]),
            }
        }
        "params" => match callee(&s1, &arg(args, 2), &s3) {
            None => Value::bool(false),
            Some(Callee::User { params, class }) => param_rows(
                params
                    .into_iter()
                    .map(|p| {
                        let ty = type_info(p.ty.as_ref(), &class);
                        (p.name, p.default.is_some(), p.variadic, p.by_ref, ty)
                    })
                    .collect(),
            ),
            Some(Callee::Library(params)) => param_rows(
                params
                    .into_iter()
                    .map(|p| {
                        let ty = (!p.ty.is_empty()).then(|| crate::ast::TypeHint {
                            parts: p
                                .ty
                                .trim_start_matches('?')
                                .split('|')
                                .map(str::to_string)
                                .chain(p.ty.starts_with('?').then(|| "null".to_string()))
                                .collect(),
                        });
                        (
                            p.name,
                            p.def.is_some(),
                            p.variadic,
                            p.by_ref,
                            type_info(ty.as_ref(), ""),
                        )
                    })
                    .collect(),
            ),
        },
        "default" => {
            let index = arg(args, 4).to_int() as usize;
            match callee(&s1, &arg(args, 2), &s3) {
                Some(Callee::User { class, params }) => {
                    match params.into_iter().nth(index).and_then(|p| p.default) {
                        Some(chunk) => crate::host::refl_eval_default(&class, chunk)?,
                        None => Value::Undef,
                    }
                }
                Some(Callee::Library(params)) => params
                    .get(index)
                    .and_then(|p| p.def)
                    .and_then(lib_default)
                    .unwrap_or(Value::Undef),
                None => Value::Undef,
            }
        }
        // Static property access from a closure the prelude bound to the
        // declaring class, so the visibility checks pass as they would there.
        "static_get" => crate::host::static_prop_get(&s1, &s2)?,
        "static_set" => crate::host::static_prop_set(&s1, &s2, arg(args, 3))?,
        // `newInstanceWithoutConstructor()`: the defaults, no constructor.
        "instantiate" => crate::host::alloc_object(&s1)?,
        "deprecated" => {
            with_host(|h| h.deprecated(s1));
            Value::Undef
        }
        _ => Value::Undef,
    })
}
