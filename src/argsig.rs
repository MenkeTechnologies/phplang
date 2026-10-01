//! Declared arity and parameter NAMES for the standard library, and the two
//! checks PHP 8 runs against them before a builtin sees its arguments: the
//! argument COUNT, and the binding of PHP 8.0 named arguments.
//!
//! [`crate::argtypes`] answers "may this value sit in that parameter"; this
//! module answers the two questions that come first — "are there the right
//! number of them", and "which parameter does this name mean". Both were
//! previously unanswered for every builtin: `strtolower("a","b")` returned
//! `"a"`, and `strtolower(a: "AB")` returned `"ab"` because a named argument was
//! appended positionally with its name discarded.
//!
//! The table is GENERATED from the reference's own reflection over the function
//! names this build implements (`crate::corpus`), so a parameter name, its
//! position, its optionality and its default are the ones PHP declares rather
//! than ones inferred from phplang's implementation.
//!
//! # Order of the checks
//!
//! Measured against the reference, which is the order this module reproduces:
//!
//! 1. A named argument that names no declared parameter, or one a positional
//!    already filled, is refused BEFORE anything else and from the CALLER's own
//!    frame — the reference raises these from the VM at send time, so the trace
//!    shows no frame for the callee. A VARIADIC function is exempt: it cannot
//!    know at send time that the name is unplaceable, so the refusal is deferred
//!    to step 4.
//! 2. The argument COUNT, counting a named argument by the slot it fills — so
//!    `array_slice(offset: 1)` passes two, not one. Framed like any other
//!    library error.
//! 3. A slot left EMPTY under a later one: required is `Argument #N ($p) not
//!    passed`, optional-with-a-default is filled silently, and optional whose
//!    default the reference does not publish is `must be passed explicitly`.
//! 4. Only now, for a variadic function, an unplaceable name is
//!    `f() does not accept unknown named parameters`.
//!
//! ```text
//! $ php -r 'strtolower("a","b",c:1);'   # 1 wins over 2
//! Error: Unknown named parameter $c
//! $ php -r 'strtolower([1],"x");'       # 2 wins over argtypes
//! ArgumentCountError: strtolower() expects exactly 1 argument, 2 given
//! $ php -r 'array_fill(count: 2);'      # 2 wins over 3
//! ArgumentCountError: array_fill(): Argument #1 ($start_index) not passed
//! $ php -r 'sprintf(x: 1);'             # 3 wins over 5
//! ArgumentCountError: sprintf() expects at least 1 argument, 0 given
//! $ php -r 'sprintf([], x: 1);'         # 4 wins over 5
//! TypeError: sprintf(): Argument #1 ($format) must be of type string, array given
//! $ php -r 'sprintf("%s", x: 1);'       # 5
//! ArgumentCountError: sprintf() does not accept unknown named parameters
//! ```

use crate::builtins::{throws, throws_bare};
use crate::host::with_host;
use fusevm::Value;

/// The default of an optional parameter, as the reference publishes it through
/// reflection.
///
/// [`Def::Unknown`] is an optional parameter whose default reflection will not
/// name (`mt_rand`'s `$min`, `array_keys`'s `$filter_value`, `round`'s `$mode`).
/// Leaving such a slot empty under a later one is the reference's
/// `must be passed explicitly, because the default value is not known`, so the
/// value is never needed — only the refusal is.
#[derive(Clone, Copy, PartialEq)]
pub enum Def {
    Required,
    Unknown,
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(&'static str),
    EmptyArray,
}

/// One declared parameter: its name, its default, and its declared type.
///
/// The type is carried for the callable check (`callable_param`); the
/// accept/reject decision for every other type stays in [`crate::argtypes`],
/// whose table has hand-corrections this generated one deliberately does not.
pub struct Param {
    pub name: &'static str,
    pub def: Def,
    pub ty: &'static str,
}

/// Shorthand so the generated rows stay one line each.
const fn p(name: &'static str, def: Def, ty: &'static str) -> Param {
    Param { name, def, ty }
}

/// A function's declared shape: how many arguments it requires, the
/// non-variadic parameters in order, and whether a variadic tail follows.
pub struct Sig {
    pub req: usize,
    pub params: &'static [Param],
    pub variadic: bool,
}

impl Sig {
    /// The largest argument count the function accepts, or `None` for a
    /// variadic one.
    fn max(&self) -> Option<usize> {
        (!self.variadic).then_some(self.params.len())
    }
}

/// `name` in lower case, WITHOUT allocating when it already is.
///
/// Every builtin call lowercases its name at least three times — the dispatcher,
/// the type table, and the arity table — and a PHP program writes the name in
/// lower case essentially always, so the common path here borrows.
pub fn lower_name(name: &str) -> std::borrow::Cow<'_, str> {
    if name.bytes().any(|b| b.is_ascii_uppercase()) {
        std::borrow::Cow::Owned(name.to_ascii_lowercase())
    } else {
        std::borrow::Cow::Borrowed(name)
    }
}

/// The declared shape of `name`, or `None` for a function the table does not
/// describe — a user function, an FFI export, or one of the three names this
/// build dispatches that reference PHP has no function for.
pub fn sig_of(name: &str) -> Option<&'static Sig> {
    let lname = lower_name(name);
    SIGS.binary_search_by(|(n, _)| (*n).cmp(lname.as_ref()))
        .ok()
        .map(|i| &SIGS[i].1)
}

/// `"argument"` or `"arguments"`, which is how the reference spells the count.
fn plural(n: usize) -> &'static str {
    if n == 1 {
        "argument"
    } else {
        "arguments"
    }
}

/// Check the argument COUNT of a call to `name`, and report the refusal the
/// reference would raise.
///
/// `rand` and `mt_rand` are the one shape reflection cannot express: both
/// declare two optional parameters but accept only zero or two of them, and for
/// any other count the reference spells the refusal `expects exactly 2
/// arguments, N given` even though neither parameter is required.
///
/// ```text
/// $ php -r 'rand(5);'      -> ArgumentCountError: rand() expects exactly 2 arguments, 1 given
/// $ php -r 'rand(1,2,3);'  -> ArgumentCountError: rand() expects exactly 2 arguments, 3 given
/// ```
pub fn check_argc(name: &str, argc: usize) -> Result<(), String> {
    match sig_of(name) {
        Some(sig) => check_argc_of(name, sig, argc),
        None => Ok(()),
    }
}

/// [`check_argc`] against a signature the caller already looked up.
fn check_argc_of(name: &str, sig: &Sig, argc: usize) -> Result<(), String> {
    if matches!(lower_name(name).as_ref(), "rand" | "mt_rand") && !matches!(argc, 0 | 2) {
        return Err(argc_error(name, "exactly", 2, argc));
    }
    if argc < sig.req {
        let bound = if sig.max() == Some(sig.req) {
            "exactly"
        } else {
            "at least"
        };
        return Err(argc_error(name, bound, sig.req, argc));
    }
    match sig.max() {
        // "exactly" when every parameter is required, "at most" when some are
        // optional.
        Some(max) if argc > max => {
            let bound = if sig.req == max { "exactly" } else { "at most" };
            Err(argc_error(name, bound, max, argc))
        }
        _ => Ok(()),
    }
}

/// The reference's argument-count refusal, spelled the way it spells it.
fn argc_error(name: &str, bound: &str, expected: usize, argc: usize) -> String {
    throws(
        "ArgumentCountError",
        format!(
            "{name}() expects {bound} {expected} {}, {argc} given",
            plural(expected)
        ),
    )
}

/// What [`bind_named`] produced.
///
/// A refusal the reference raises INSIDE the callee comes back here rather than
/// as an `Err`, because it has to be raised with a trace frame, and that frame
/// renders the arguments as the call BOUND them — which only this function
/// knows. `Err` is reserved for the two the reference raises at send time, which
/// carry no frame at all.
pub struct Bound {
    /// The positional argument list to call with; empty when `refusal` is set.
    pub args: Vec<Value>,
    /// The arguments as a trace frame for this call renders them: every slot the
    /// call reached, holes included as null, then the names a variadic could not
    /// place — which the reference prints as `name: value`.
    pub shown: Vec<(Option<String>, Value)>,
    /// A framed refusal: the count, or a slot the call jumped over.
    pub refusal: Option<String>,
    /// A name a variadic function could not place. Reported after `refusal`.
    pub unplaced_name: bool,
}

/// Bind PHP 8.0 named arguments to `name`'s declared parameters.
///
/// Callers check [`sig_of`] first; a function the table does not describe keeps
/// the positional-append fallback and never reaches here.
///
/// # Panics
///
/// Panics if the table does not describe `name`.
pub fn bind_named(
    name: &str,
    args: Vec<Value>,
    named: Vec<(String, Value)>,
) -> Result<Bound, String> {
    let sig = sig_of(name).expect("caller checked sig_of");
    // Slots hold every argument in declared order; `None` is a hole a later
    // named argument jumped over. A variadic tail keeps its positionals beyond
    // the declared parameters, where no name can reach them.
    let mut slots: Vec<Option<Value>> = sig.params.iter().map(|_| None).collect();
    let mut tail: Vec<Value> = Vec::new();
    for (i, v) in args.into_iter().enumerate() {
        match slots.get_mut(i) {
            Some(slot) => *slot = Some(v),
            None => tail.push(v),
        }
    }
    let filled_positionally = slots.iter().filter(|s| s.is_some()).count() + tail.len();

    let mut unplaced: Vec<(String, Value)> = Vec::new();
    for (n, v) in named {
        match sig.params.iter().position(|p| p.name == n) {
            Some(i) if i < filled_positionally => {
                return Err(throws_bare(
                    "Error",
                    format!("Named parameter ${n} overwrites previous argument"),
                ));
            }
            Some(i) => slots[i] = Some(v),
            // A variadic function cannot refuse the name at send time; the
            // refusal waits until after the count check.
            None if sig.variadic => unplaced.push((n, v)),
            None => {
                return Err(throws_bare(
                    "Error",
                    format!("Unknown named parameter ${n}"),
                ));
            }
        }
    }

    // The reference counts a call by the highest slot a name reached, holes
    // included — `array_slice(offset: 1)` passes two arguments, one of them a
    // hole, which is why it reports `$array` not passed rather than a shortfall.
    let last = slots.iter().rposition(Option::is_some).map_or(0, |i| i + 1);
    let argc = last + tail.len();

    // What a trace frame renders, settled before any refusal so every one of
    // them carries the same list the reference shows.
    let mut shown: Vec<(Option<String>, Value)> = slots
        .iter()
        .take(last)
        .map(|s| (None, s.clone().unwrap_or(Value::Undef)))
        .collect();
    shown.extend(tail.iter().map(|v| (None, v.clone())));
    shown.extend(unplaced.iter().map(|(n, v)| (Some(n.clone()), v.clone())));

    // A HOLE is refused before the COUNT. `array_fill(count: 2)` reaches two
    // slots of a function that requires three, and the reference reports the
    // empty first slot rather than the shortfall:
    //
    // ```text
    // $ php -r 'array_fill(count: 2);'
    // ArgumentCountError: array_fill(): Argument #1 ($start_index) not passed
    // ```
    let refusal = slots
        .iter()
        .take(last)
        .enumerate()
        .find(|(_, s)| s.is_none())
        .and_then(|(i, _)| hole_refusal(name, sig, i))
        .or_else(|| check_argc(name, argc).err());
    if refusal.is_some() {
        return Ok(Bound {
            args: Vec::new(),
            shown,
            refusal,
            unplaced_name: false,
        });
    }

    let mut out: Vec<Value> = Vec::with_capacity(argc);
    for (i, slot) in slots.into_iter().take(last).enumerate() {
        out.push(match slot {
            Some(v) => v,
            // Checked above: a hole with no fill has already been refused.
            None => hole_default(sig, i),
        });
    }
    out.extend(tail);
    Ok(Bound {
        args: out,
        shown,
        refusal: None,
        unplaced_name: !unplaced.is_empty(),
    })
}

/// The refusal the reference raises for a slot a later named argument jumped
/// over, or `None` when the slot has a default to fill it with.
fn hole_refusal(name: &str, sig: &Sig, i: usize) -> Option<String> {
    let param = &sig.params[i];
    let argno = i + 1;
    match param.def {
        Def::Required => Some(throws(
            "ArgumentCountError",
            format!("{name}(): Argument #{argno} (${}) not passed", param.name),
        )),
        Def::Unknown => Some(throws(
            "ArgumentCountError",
            format!(
                "{name}(): Argument #{argno} (${}) must be passed explicitly, \
                 because the default value is not known",
                param.name
            ),
        )),
        _ => None,
    }
}

/// The value the reference puts in a jumped-over slot that has a default.
fn hole_default(sig: &Sig, i: usize) -> Value {
    match sig.params[i].def {
        Def::Bool(b) => Value::Bool(b),
        Def::Int(n) => Value::Int(n),
        Def::Float(f) => Value::Float(f),
        Def::Str(s) => Value::str(s.to_string()),
        Def::EmptyArray => with_host(|h| h.new_array()),
        // `Null`, and the two refused above, which cannot reach here.
        _ => Value::Undef,
    }
}

/// The refusal a variadic function raises for a name it cannot place, once the
/// count check has passed.
pub fn unknown_named_for_variadic(name: &str) -> String {
    throws(
        "ArgumentCountError",
        format!("{name}() does not accept unknown named parameters"),
    )
}

/// How many leading parameters the reference's parameter-parsing block reads
/// BEFORE its `Z_PARAM_VARIADIC` — the only ones whose types are judged ahead of
/// an unplaceable name. Usually every declared non-variadic parameter, but the
/// array set operations and `array_map` take their arrays (and the trailing
/// callback) as one undifferentiated variadic and sort them out in the body, so
/// a wrong type there loses to the name:
///
/// ```text
/// $ php -r 'array_diff(1, x: 1);'
/// ArgumentCountError: array_diff() does not accept unknown named parameters
/// $ php -r 'array_map(null, null, x: 1);'
/// ArgumentCountError: array_map() does not accept unknown named parameters
/// ```
fn zpp_fixed(name: &str, sig: &Sig) -> usize {
    match lower_name(name).as_ref() {
        "array_map" => 1,
        "array_diff"
        | "array_diff_key"
        | "array_diff_assoc"
        | "array_diff_ukey"
        | "array_diff_uassoc"
        | "array_udiff"
        | "array_udiff_assoc"
        | "array_udiff_uassoc"
        | "array_intersect"
        | "array_intersect_key"
        | "array_intersect_assoc"
        | "array_intersect_ukey"
        | "array_intersect_uassoc"
        | "array_uintersect"
        | "array_uintersect_assoc"
        | "array_uintersect_uassoc"
        | "array_replace"
        | "array_replace_recursive" => 0,
        _ => sig.params.len(),
    }
}

/// The refusal for a variadic call carrying a name it could not place: the
/// count, then the types of the parameters parsed ahead of the variadic
/// ([`zpp_fixed`]), and only then `does not accept unknown named parameters`.
pub fn refuse_unplaced(name: &str, args: &[Value]) -> String {
    let Some(sig) = sig_of(name) else {
        return check_args(name, args)
            .err()
            .unwrap_or_else(|| unknown_named_for_variadic(name));
    };
    if let Err(e) = check_argc_of(name, sig, args.len()) {
        return e;
    }
    let head = &args[..zpp_fixed(name, sig).min(args.len())];
    let callable = check_callable(name, sig, head);
    let stop_at = callable
        .as_ref()
        .err()
        .map_or(u32::MAX, |(argno, _)| *argno);
    crate::argtypes::check_call(name, head, stop_at)
        .and(callable.map_err(|(_, e)| e))
        .err()
        .unwrap_or_else(|| unknown_named_for_variadic(name))
}

/// Every refusal PHP 8 makes about a call's ARGUMENTS, in the reference's order:
/// the count first, then the parameters left to right, each of them judged by
/// its declared type ([`crate::argtypes`]) or, for a `callable`, by whether the
/// value names something invocable.
///
/// The two per-parameter checks live in different tables, so they are merged by
/// POSITION rather than run one after the other —
/// `array_map("nofunc", "notarray")` reports the callback in #1, not the string
/// in #2.
pub fn check_args(name: &str, args: &[Value]) -> Result<(), String> {
    // One table lookup for the whole call: the count check and the callable
    // check both read the same signature, and this is the hot path every
    // builtin call goes down.
    static LEVEL: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    let level = *LEVEL.get_or_init(|| {
        std::env::var("PHPLANG_LEVEL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    });
    let Some(sig) = sig_of(name) else {
        return crate::argtypes::check_call(name, args, u32::MAX);
    };
    if level == 1 {
        return crate::argtypes::check_call(name, args, u32::MAX);
    }
    check_argc_of(name, sig, args.len())?;
    if level == 2 {
        return crate::argtypes::check_call(name, args, u32::MAX);
    }
    let callable = check_callable(name, sig, args);
    let stop_at = callable
        .as_ref()
        .err()
        .map_or(u32::MAX, |(argno, _)| *argno);
    crate::argtypes::check_call(name, args, stop_at)?;
    callable.map_err(|(_, e)| e)
}

/// Whether the value in `name`'s `callable` parameter names something invocable,
/// and the reference's refusal — with its 1-based position — when it does not.
///
/// A `?callable` parameter takes null as well, which is why `array_map(null,
/// [1], [2])` zips rather than throwing.
fn check_callable(name: &str, sig: &Sig, args: &[Value]) -> Result<(), (u32, String)> {
    let Some((argno, pname, nullable)) = callable_param(sig) else {
        return Ok(());
    };
    let Some(v) = args.get(argno - 1) else {
        return Ok(());
    };
    if nullable && matches!(v, Value::Undef) {
        return Ok(());
    }
    let Some(reason) = crate::stdlib::callable::callable_reason(v) else {
        return Ok(());
    };
    let orn = if nullable { " or null" } else { "" };
    Err((
        argno as u32,
        throws(
            "TypeError",
            format!(
                "{name}(): Argument #{argno} (${pname}) must be a valid callback{orn}, {reason}"
            ),
        ),
    ))
}

/// The 1-based position and name of `sig`'s `callable` parameter, and whether
/// it also accepts null — the shape the reference's
/// `must be a valid callback[ or null]` message needs.
fn callable_param(sig: &Sig) -> Option<(usize, &'static str, bool)> {
    sig.params.iter().enumerate().find_map(|(i, p)| {
        let nullable = p.ty.starts_with('?');
        (p.ty.trim_start_matches('?') == "callable").then_some((i + 1, p.name, nullable))
    })
}

use Def::{Bool, EmptyArray, Float, Int, Null, Required, Str, Unknown};

/// The declared shapes, sorted by name so [`sig_of`] can binary-search.
///
/// `log`'s `$base` defaults to Euler's number, which the reference publishes as
/// the literal below; it is transcribed rather than named because every other
/// default in this generated table is a literal too.
#[allow(clippy::approx_constant)]
static SIGS: &[(&str, Sig)] = &[
    // generated: 525 functions
    (
        "abs",
        Sig {
            req: 1,
            params: &[p("num", Required, "int|float")],
            variadic: false,
        },
    ),
    (
        "acos",
        Sig {
            req: 1,
            params: &[p("num", Required, "float")],
            variadic: false,
        },
    ),
    (
        "acosh",
        Sig {
            req: 1,
            params: &[p("num", Required, "float")],
            variadic: false,
        },
    ),
    (
        "addcslashes",
        Sig {
            req: 2,
            params: &[
                p("string", Required, "string"),
                p("characters", Required, "string"),
            ],
            variadic: false,
        },
    ),
    (
        "addslashes",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "array_all",
        Sig {
            req: 2,
            params: &[
                p("array", Required, "array"),
                p("callback", Required, "callable"),
            ],
            variadic: false,
        },
    ),
    (
        "array_any",
        Sig {
            req: 2,
            params: &[
                p("array", Required, "array"),
                p("callback", Required, "callable"),
            ],
            variadic: false,
        },
    ),
    (
        "array_change_key_case",
        Sig {
            req: 1,
            params: &[p("array", Required, "array"), p("case", Int(0), "int")],
            variadic: false,
        },
    ),
    (
        "array_chunk",
        Sig {
            req: 2,
            params: &[
                p("array", Required, "array"),
                p("length", Required, "int"),
                p("preserve_keys", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "array_column",
        Sig {
            req: 2,
            params: &[
                p("array", Required, "array"),
                p("column_key", Required, "string|int|null"),
                p("index_key", Null, "string|int|null"),
            ],
            variadic: false,
        },
    ),
    (
        "array_combine",
        Sig {
            req: 2,
            params: &[p("keys", Required, "array"), p("values", Required, "array")],
            variadic: false,
        },
    ),
    (
        "array_count_values",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: false,
        },
    ),
    (
        "array_diff",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: true,
        },
    ),
    (
        "array_diff_assoc",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: true,
        },
    ),
    (
        "array_diff_key",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: true,
        },
    ),
    (
        "array_diff_ukey",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: true,
        },
    ),
    (
        "array_fill",
        Sig {
            req: 3,
            params: &[
                p("start_index", Required, "int"),
                p("count", Required, "int"),
                p("value", Required, "mixed"),
            ],
            variadic: false,
        },
    ),
    (
        "array_fill_keys",
        Sig {
            req: 2,
            params: &[p("keys", Required, "array"), p("value", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "array_filter",
        Sig {
            req: 1,
            params: &[
                p("array", Required, "array"),
                p("callback", Null, "?callable"),
                p("mode", Int(0), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "array_find",
        Sig {
            req: 2,
            params: &[
                p("array", Required, "array"),
                p("callback", Required, "callable"),
            ],
            variadic: false,
        },
    ),
    (
        "array_find_key",
        Sig {
            req: 2,
            params: &[
                p("array", Required, "array"),
                p("callback", Required, "callable"),
            ],
            variadic: false,
        },
    ),
    (
        "array_flip",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: false,
        },
    ),
    (
        "array_intersect",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: true,
        },
    ),
    (
        "array_intersect_assoc",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: true,
        },
    ),
    (
        "array_intersect_key",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: true,
        },
    ),
    (
        "array_intersect_ukey",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: true,
        },
    ),
    (
        "array_is_list",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: false,
        },
    ),
    (
        "array_key_exists",
        Sig {
            req: 2,
            params: &[p("key", Required, ""), p("array", Required, "array")],
            variadic: false,
        },
    ),
    (
        "array_key_first",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: false,
        },
    ),
    (
        "array_key_last",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: false,
        },
    ),
    (
        "array_keys",
        Sig {
            req: 1,
            params: &[
                p("array", Required, "array"),
                p("filter_value", Unknown, "mixed"),
                p("strict", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "array_map",
        Sig {
            req: 2,
            params: &[
                p("callback", Required, "?callable"),
                p("array", Required, "array"),
            ],
            variadic: true,
        },
    ),
    (
        "array_merge",
        Sig {
            req: 0,
            params: &[],
            variadic: true,
        },
    ),
    (
        "array_merge_recursive",
        Sig {
            req: 0,
            params: &[],
            variadic: true,
        },
    ),
    (
        "array_multisort",
        Sig {
            req: 1,
            params: &[p("array", Required, "")],
            variadic: true,
        },
    ),
    (
        "array_pad",
        Sig {
            req: 3,
            params: &[
                p("array", Required, "array"),
                p("length", Required, "int"),
                p("value", Required, "mixed"),
            ],
            variadic: false,
        },
    ),
    (
        "array_pop",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: false,
        },
    ),
    (
        "array_product",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: false,
        },
    ),
    (
        "array_push",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: true,
        },
    ),
    (
        "array_rand",
        Sig {
            req: 1,
            params: &[p("array", Required, "array"), p("num", Int(1), "int")],
            variadic: false,
        },
    ),
    (
        "array_reduce",
        Sig {
            req: 2,
            params: &[
                p("array", Required, "array"),
                p("callback", Required, "callable"),
                p("initial", Null, "mixed"),
            ],
            variadic: false,
        },
    ),
    (
        "array_replace",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: true,
        },
    ),
    (
        "array_replace_recursive",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: true,
        },
    ),
    (
        "array_reverse",
        Sig {
            req: 1,
            params: &[
                p("array", Required, "array"),
                p("preserve_keys", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "array_search",
        Sig {
            req: 2,
            params: &[
                p("needle", Required, "mixed"),
                p("haystack", Required, "array"),
                p("strict", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "array_shift",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: false,
        },
    ),
    (
        "array_slice",
        Sig {
            req: 2,
            params: &[
                p("array", Required, "array"),
                p("offset", Required, "int"),
                p("length", Null, "?int"),
                p("preserve_keys", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "array_splice",
        Sig {
            req: 2,
            params: &[
                p("array", Required, "array"),
                p("offset", Required, "int"),
                p("length", Null, "?int"),
                p("replacement", EmptyArray, "mixed"),
            ],
            variadic: false,
        },
    ),
    (
        "array_sum",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: false,
        },
    ),
    (
        "array_udiff",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: true,
        },
    ),
    (
        "array_uintersect",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: true,
        },
    ),
    (
        "array_unique",
        Sig {
            req: 1,
            params: &[p("array", Required, "array"), p("flags", Int(2), "int")],
            variadic: false,
        },
    ),
    (
        "array_unshift",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: true,
        },
    ),
    (
        "array_values",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: false,
        },
    ),
    (
        "array_walk",
        Sig {
            req: 2,
            params: &[
                p("array", Required, "object|array"),
                p("callback", Required, "callable"),
                p("arg", Unknown, "mixed"),
            ],
            variadic: false,
        },
    ),
    (
        "array_walk_recursive",
        Sig {
            req: 2,
            params: &[
                p("array", Required, "object|array"),
                p("callback", Required, "callable"),
                p("arg", Unknown, "mixed"),
            ],
            variadic: false,
        },
    ),
    (
        "arsort",
        Sig {
            req: 1,
            params: &[p("array", Required, "array"), p("flags", Int(0), "int")],
            variadic: false,
        },
    ),
    (
        "asin",
        Sig {
            req: 1,
            params: &[p("num", Required, "float")],
            variadic: false,
        },
    ),
    (
        "asinh",
        Sig {
            req: 1,
            params: &[p("num", Required, "float")],
            variadic: false,
        },
    ),
    (
        "asort",
        Sig {
            req: 1,
            params: &[p("array", Required, "array"), p("flags", Int(0), "int")],
            variadic: false,
        },
    ),
    (
        "assert",
        Sig {
            req: 1,
            params: &[
                p("assertion", Required, "mixed"),
                p("description", Null, "Throwable|string|null"),
            ],
            variadic: false,
        },
    ),
    (
        "assert_options",
        Sig {
            req: 1,
            params: &[p("option", Required, "int"), p("value", Unknown, "mixed")],
            variadic: false,
        },
    ),
    (
        "atan",
        Sig {
            req: 1,
            params: &[p("num", Required, "float")],
            variadic: false,
        },
    ),
    (
        "atan2",
        Sig {
            req: 2,
            params: &[p("y", Required, "float"), p("x", Required, "float")],
            variadic: false,
        },
    ),
    (
        "atanh",
        Sig {
            req: 1,
            params: &[p("num", Required, "float")],
            variadic: false,
        },
    ),
    (
        "base64_decode",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("strict", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "base64_encode",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "base_convert",
        Sig {
            req: 3,
            params: &[
                p("num", Required, "string"),
                p("from_base", Required, "int"),
                p("to_base", Required, "int"),
            ],
            variadic: false,
        },
    ),
    (
        "basename",
        Sig {
            req: 1,
            params: &[
                p("path", Required, "string"),
                p("suffix", Str(""), "string"),
            ],
            variadic: false,
        },
    ),
    (
        "bcadd",
        Sig {
            req: 2,
            params: &[
                p("num1", Required, "string"),
                p("num2", Required, "string"),
                p("scale", Null, "?int"),
            ],
            variadic: false,
        },
    ),
    (
        "bccomp",
        Sig {
            req: 2,
            params: &[
                p("num1", Required, "string"),
                p("num2", Required, "string"),
                p("scale", Null, "?int"),
            ],
            variadic: false,
        },
    ),
    (
        "bcdiv",
        Sig {
            req: 2,
            params: &[
                p("num1", Required, "string"),
                p("num2", Required, "string"),
                p("scale", Null, "?int"),
            ],
            variadic: false,
        },
    ),
    (
        "bcmod",
        Sig {
            req: 2,
            params: &[
                p("num1", Required, "string"),
                p("num2", Required, "string"),
                p("scale", Null, "?int"),
            ],
            variadic: false,
        },
    ),
    (
        "bcmul",
        Sig {
            req: 2,
            params: &[
                p("num1", Required, "string"),
                p("num2", Required, "string"),
                p("scale", Null, "?int"),
            ],
            variadic: false,
        },
    ),
    (
        "bcpow",
        Sig {
            req: 2,
            params: &[
                p("num", Required, "string"),
                p("exponent", Required, "string"),
                p("scale", Null, "?int"),
            ],
            variadic: false,
        },
    ),
    (
        "bcpowmod",
        Sig {
            req: 3,
            params: &[
                p("num", Required, "string"),
                p("exponent", Required, "string"),
                p("modulus", Required, "string"),
                p("scale", Null, "?int"),
            ],
            variadic: false,
        },
    ),
    (
        "bcscale",
        Sig {
            req: 0,
            params: &[p("scale", Null, "?int")],
            variadic: false,
        },
    ),
    (
        "bcsqrt",
        Sig {
            req: 1,
            params: &[p("num", Required, "string"), p("scale", Null, "?int")],
            variadic: false,
        },
    ),
    (
        "bcsub",
        Sig {
            req: 2,
            params: &[
                p("num1", Required, "string"),
                p("num2", Required, "string"),
                p("scale", Null, "?int"),
            ],
            variadic: false,
        },
    ),
    (
        "bin2hex",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "bindec",
        Sig {
            req: 1,
            params: &[p("binary_string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "boolval",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "call_user_func",
        Sig {
            req: 1,
            params: &[p("callback", Required, "callable")],
            variadic: true,
        },
    ),
    (
        "call_user_func_array",
        Sig {
            req: 2,
            params: &[
                p("callback", Required, "callable"),
                p("args", Required, "array"),
            ],
            variadic: false,
        },
    ),
    (
        "ceil",
        Sig {
            req: 1,
            params: &[p("num", Required, "int|float")],
            variadic: false,
        },
    ),
    (
        "checkdate",
        Sig {
            req: 3,
            params: &[
                p("month", Required, "int"),
                p("day", Required, "int"),
                p("year", Required, "int"),
            ],
            variadic: false,
        },
    ),
    (
        "chop",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("characters", Str(" \n\r\t\u{B}\u{0}"), "string"),
            ],
            variadic: false,
        },
    ),
    (
        "chr",
        Sig {
            req: 1,
            params: &[p("codepoint", Required, "int")],
            variadic: false,
        },
    ),
    (
        "chunk_split",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("length", Int(76), "int"),
                p("separator", Str("\r\n"), "string"),
            ],
            variadic: false,
        },
    ),
    (
        "class_alias",
        Sig {
            req: 2,
            params: &[
                p("class", Required, "string"),
                p("alias", Required, "string"),
                p("autoload", Bool(true), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "class_exists",
        Sig {
            req: 1,
            params: &[
                p("class", Required, "string"),
                p("autoload", Bool(true), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "class_implements",
        Sig {
            req: 1,
            params: &[
                p("object_or_class", Required, ""),
                p("autoload", Bool(true), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "class_parents",
        Sig {
            req: 1,
            params: &[
                p("object_or_class", Required, ""),
                p("autoload", Bool(true), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "class_uses",
        Sig {
            req: 1,
            params: &[
                p("object_or_class", Required, ""),
                p("autoload", Bool(true), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "clearstatcache",
        Sig {
            req: 0,
            params: &[
                p("clear_realpath_cache", Bool(false), "bool"),
                p("filename", Str(""), "string"),
            ],
            variadic: false,
        },
    ),
    (
        "compact",
        Sig {
            req: 1,
            params: &[p("var_name", Required, "")],
            variadic: true,
        },
    ),
    (
        "constant",
        Sig {
            req: 1,
            params: &[p("name", Required, "string")],
            variadic: false,
        },
    ),
    (
        "convert_uudecode",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "convert_uuencode",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "copy",
        Sig {
            req: 2,
            params: &[
                p("from", Required, "string"),
                p("to", Required, "string"),
                p("context", Null, ""),
            ],
            variadic: false,
        },
    ),
    (
        "cos",
        Sig {
            req: 1,
            params: &[p("num", Required, "float")],
            variadic: false,
        },
    ),
    (
        "cosh",
        Sig {
            req: 1,
            params: &[p("num", Required, "float")],
            variadic: false,
        },
    ),
    (
        "count",
        Sig {
            req: 1,
            params: &[
                p("value", Required, "Countable|array"),
                p("mode", Int(0), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "count_chars",
        Sig {
            req: 1,
            params: &[p("string", Required, "string"), p("mode", Int(0), "int")],
            variadic: false,
        },
    ),
    (
        "crc32",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "ctype_alnum",
        Sig {
            req: 1,
            params: &[p("text", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "ctype_alpha",
        Sig {
            req: 1,
            params: &[p("text", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "ctype_cntrl",
        Sig {
            req: 1,
            params: &[p("text", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "ctype_digit",
        Sig {
            req: 1,
            params: &[p("text", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "ctype_graph",
        Sig {
            req: 1,
            params: &[p("text", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "ctype_lower",
        Sig {
            req: 1,
            params: &[p("text", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "ctype_print",
        Sig {
            req: 1,
            params: &[p("text", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "ctype_punct",
        Sig {
            req: 1,
            params: &[p("text", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "ctype_space",
        Sig {
            req: 1,
            params: &[p("text", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "ctype_upper",
        Sig {
            req: 1,
            params: &[p("text", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "ctype_xdigit",
        Sig {
            req: 1,
            params: &[p("text", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "current",
        Sig {
            req: 1,
            params: &[p("array", Required, "object|array")],
            variadic: false,
        },
    ),
    (
        "date",
        Sig {
            req: 1,
            params: &[
                p("format", Required, "string"),
                p("timestamp", Null, "?int"),
            ],
            variadic: false,
        },
    ),
    (
        "date_add",
        Sig {
            req: 2,
            params: &[
                p("object", Required, "DateTime"),
                p("interval", Required, "DateInterval"),
            ],
            variadic: false,
        },
    ),
    (
        "date_create",
        Sig {
            req: 0,
            params: &[
                p("datetime", Str("now"), "string"),
                p("timezone", Null, "?DateTimeZone"),
            ],
            variadic: false,
        },
    ),
    (
        "date_create_immutable",
        Sig {
            req: 0,
            params: &[
                p("datetime", Str("now"), "string"),
                p("timezone", Null, "?DateTimeZone"),
            ],
            variadic: false,
        },
    ),
    (
        "date_default_timezone_get",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "date_default_timezone_set",
        Sig {
            req: 1,
            params: &[p("timezoneId", Required, "string")],
            variadic: false,
        },
    ),
    (
        "date_diff",
        Sig {
            req: 2,
            params: &[
                p("baseObject", Required, "DateTimeInterface"),
                p("targetObject", Required, "DateTimeInterface"),
                p("absolute", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "date_format",
        Sig {
            req: 2,
            params: &[
                p("object", Required, "DateTimeInterface"),
                p("format", Required, "string"),
            ],
            variadic: false,
        },
    ),
    (
        "date_interval_create_from_date_string",
        Sig {
            req: 1,
            params: &[p("datetime", Required, "string")],
            variadic: false,
        },
    ),
    (
        "date_interval_format",
        Sig {
            req: 2,
            params: &[
                p("object", Required, "DateInterval"),
                p("format", Required, "string"),
            ],
            variadic: false,
        },
    ),
    (
        "date_modify",
        Sig {
            req: 2,
            params: &[
                p("object", Required, "DateTime"),
                p("modifier", Required, "string"),
            ],
            variadic: false,
        },
    ),
    (
        "date_offset_get",
        Sig {
            req: 1,
            params: &[p("object", Required, "DateTimeInterface")],
            variadic: false,
        },
    ),
    (
        "date_sub",
        Sig {
            req: 2,
            params: &[
                p("object", Required, "DateTime"),
                p("interval", Required, "DateInterval"),
            ],
            variadic: false,
        },
    ),
    (
        "date_timestamp_get",
        Sig {
            req: 1,
            params: &[p("object", Required, "DateTimeInterface")],
            variadic: false,
        },
    ),
    (
        "date_timestamp_set",
        Sig {
            req: 2,
            params: &[
                p("object", Required, "DateTime"),
                p("timestamp", Required, "int"),
            ],
            variadic: false,
        },
    ),
    (
        "debug_backtrace",
        Sig {
            req: 0,
            params: &[p("options", Int(1), "int"), p("limit", Int(0), "int")],
            variadic: false,
        },
    ),
    (
        "debug_print_backtrace",
        Sig {
            req: 0,
            params: &[p("options", Int(0), "int"), p("limit", Int(0), "int")],
            variadic: false,
        },
    ),
    (
        "decbin",
        Sig {
            req: 1,
            params: &[p("num", Required, "int")],
            variadic: false,
        },
    ),
    (
        "dechex",
        Sig {
            req: 1,
            params: &[p("num", Required, "int")],
            variadic: false,
        },
    ),
    (
        "decoct",
        Sig {
            req: 1,
            params: &[p("num", Required, "int")],
            variadic: false,
        },
    ),
    (
        "define",
        Sig {
            req: 2,
            params: &[
                p("constant_name", Required, "string"),
                p("value", Required, "mixed"),
                p("case_insensitive", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "defined",
        Sig {
            req: 1,
            params: &[p("constant_name", Required, "string")],
            variadic: false,
        },
    ),
    (
        "deg2rad",
        Sig {
            req: 1,
            params: &[p("num", Required, "float")],
            variadic: false,
        },
    ),
    (
        "die",
        Sig {
            req: 0,
            params: &[p("status", Int(0), "string|int")],
            variadic: false,
        },
    ),
    (
        "dirname",
        Sig {
            req: 1,
            params: &[p("path", Required, "string"), p("levels", Int(1), "int")],
            variadic: false,
        },
    ),
    (
        "disk_free_space",
        Sig {
            req: 1,
            params: &[p("directory", Required, "string")],
            variadic: false,
        },
    ),
    (
        "disk_total_space",
        Sig {
            req: 1,
            params: &[p("directory", Required, "string")],
            variadic: false,
        },
    ),
    (
        "diskfreespace",
        Sig {
            req: 1,
            params: &[p("directory", Required, "string")],
            variadic: false,
        },
    ),
    (
        "doubleval",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "end",
        Sig {
            req: 1,
            params: &[p("array", Required, "object|array")],
            variadic: false,
        },
    ),
    (
        "enum_exists",
        Sig {
            req: 1,
            params: &[
                p("enum", Required, "string"),
                p("autoload", Bool(true), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "error_clear_last",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "error_get_last",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "error_log",
        Sig {
            req: 1,
            params: &[
                p("message", Required, "string"),
                p("message_type", Int(0), "int"),
                p("destination", Null, "?string"),
                p("additional_headers", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "error_reporting",
        Sig {
            req: 0,
            params: &[p("error_level", Null, "?int")],
            variadic: false,
        },
    ),
    (
        "exit",
        Sig {
            req: 0,
            params: &[p("status", Int(0), "string|int")],
            variadic: false,
        },
    ),
    (
        "exp",
        Sig {
            req: 1,
            params: &[p("num", Required, "float")],
            variadic: false,
        },
    ),
    (
        "explode",
        Sig {
            req: 2,
            params: &[
                p("separator", Required, "string"),
                p("string", Required, "string"),
                p("limit", Int(9223372036854775807), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "expm1",
        Sig {
            req: 1,
            params: &[p("num", Required, "float")],
            variadic: false,
        },
    ),
    (
        "extension_loaded",
        Sig {
            req: 1,
            params: &[p("extension", Required, "string")],
            variadic: false,
        },
    ),
    (
        "extract",
        Sig {
            req: 1,
            params: &[
                p("array", Required, "array"),
                p("flags", Int(0), "int"),
                p("prefix", Str(""), "string"),
            ],
            variadic: false,
        },
    ),
    (
        "fclose",
        Sig {
            req: 1,
            params: &[p("stream", Required, "")],
            variadic: false,
        },
    ),
    (
        "fdiv",
        Sig {
            req: 2,
            params: &[p("num1", Required, "float"), p("num2", Required, "float")],
            variadic: false,
        },
    ),
    (
        "feof",
        Sig {
            req: 1,
            params: &[p("stream", Required, "")],
            variadic: false,
        },
    ),
    (
        "fflush",
        Sig {
            req: 1,
            params: &[p("stream", Required, "")],
            variadic: false,
        },
    ),
    (
        "fgetc",
        Sig {
            req: 1,
            params: &[p("stream", Required, "")],
            variadic: false,
        },
    ),
    (
        "fgets",
        Sig {
            req: 1,
            params: &[p("stream", Required, ""), p("length", Null, "?int")],
            variadic: false,
        },
    ),
    (
        "file",
        Sig {
            req: 1,
            params: &[
                p("filename", Required, "string"),
                p("flags", Int(0), "int"),
                p("context", Null, ""),
            ],
            variadic: false,
        },
    ),
    (
        "file_exists",
        Sig {
            req: 1,
            params: &[p("filename", Required, "string")],
            variadic: false,
        },
    ),
    (
        "file_get_contents",
        Sig {
            req: 1,
            params: &[
                p("filename", Required, "string"),
                p("use_include_path", Bool(false), "bool"),
                p("context", Null, ""),
                p("offset", Int(0), "int"),
                p("length", Null, "?int"),
            ],
            variadic: false,
        },
    ),
    (
        "file_put_contents",
        Sig {
            req: 2,
            params: &[
                p("filename", Required, "string"),
                p("data", Required, "mixed"),
                p("flags", Int(0), "int"),
                p("context", Null, ""),
            ],
            variadic: false,
        },
    ),
    (
        "filemtime",
        Sig {
            req: 1,
            params: &[p("filename", Required, "string")],
            variadic: false,
        },
    ),
    (
        "fileperms",
        Sig {
            req: 1,
            params: &[p("filename", Required, "string")],
            variadic: false,
        },
    ),
    (
        "filesize",
        Sig {
            req: 1,
            params: &[p("filename", Required, "string")],
            variadic: false,
        },
    ),
    (
        "filetype",
        Sig {
            req: 1,
            params: &[p("filename", Required, "string")],
            variadic: false,
        },
    ),
    (
        "filter_has_var",
        Sig {
            req: 2,
            params: &[
                p("input_type", Required, "int"),
                p("var_name", Required, "string"),
            ],
            variadic: false,
        },
    ),
    (
        "filter_var",
        Sig {
            req: 1,
            params: &[
                p("value", Required, "mixed"),
                p("filter", Int(516), "int"),
                p("options", Int(0), "array|int"),
            ],
            variadic: false,
        },
    ),
    (
        "filter_var_array",
        Sig {
            req: 1,
            params: &[
                p("array", Required, "array"),
                p("options", Int(516), "array|int"),
                p("add_empty", Bool(true), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "floatval",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "floor",
        Sig {
            req: 1,
            params: &[p("num", Required, "int|float")],
            variadic: false,
        },
    ),
    (
        "flush",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "fmod",
        Sig {
            req: 2,
            params: &[p("num1", Required, "float"), p("num2", Required, "float")],
            variadic: false,
        },
    ),
    (
        "fnmatch",
        Sig {
            req: 2,
            params: &[
                p("pattern", Required, "string"),
                p("filename", Required, "string"),
                p("flags", Int(0), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "fopen",
        Sig {
            req: 2,
            params: &[
                p("filename", Required, "string"),
                p("mode", Required, "string"),
                p("use_include_path", Bool(false), "bool"),
                p("context", Null, ""),
            ],
            variadic: false,
        },
    ),
    (
        "fpassthru",
        Sig {
            req: 1,
            params: &[p("stream", Required, "")],
            variadic: false,
        },
    ),
    (
        "fprintf",
        Sig {
            req: 2,
            params: &[p("stream", Required, ""), p("format", Required, "string")],
            variadic: true,
        },
    ),
    (
        "fputs",
        Sig {
            req: 2,
            params: &[
                p("stream", Required, ""),
                p("data", Required, "string"),
                p("length", Null, "?int"),
            ],
            variadic: false,
        },
    ),
    (
        "fread",
        Sig {
            req: 2,
            params: &[p("stream", Required, ""), p("length", Required, "int")],
            variadic: false,
        },
    ),
    (
        "fscanf",
        Sig {
            req: 2,
            params: &[p("stream", Required, ""), p("format", Required, "string")],
            variadic: true,
        },
    ),
    (
        "fseek",
        Sig {
            req: 2,
            params: &[
                p("stream", Required, ""),
                p("offset", Required, "int"),
                p("whence", Int(0), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "fstat",
        Sig {
            req: 1,
            params: &[p("stream", Required, "")],
            variadic: false,
        },
    ),
    (
        "ftell",
        Sig {
            req: 1,
            params: &[p("stream", Required, "")],
            variadic: false,
        },
    ),
    (
        "ftruncate",
        Sig {
            req: 2,
            params: &[p("stream", Required, ""), p("size", Required, "int")],
            variadic: false,
        },
    ),
    (
        "func_get_arg",
        Sig {
            req: 1,
            params: &[p("position", Required, "int")],
            variadic: false,
        },
    ),
    (
        "func_get_args",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "func_num_args",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "function_exists",
        Sig {
            req: 1,
            params: &[p("function", Required, "string")],
            variadic: false,
        },
    ),
    (
        "fwrite",
        Sig {
            req: 2,
            params: &[
                p("stream", Required, ""),
                p("data", Required, "string"),
                p("length", Null, "?int"),
            ],
            variadic: false,
        },
    ),
    (
        "gc_collect_cycles",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "gc_disable",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "gc_enable",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "gc_enabled",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "gc_mem_caches",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "get_class",
        Sig {
            req: 0,
            params: &[p("object", Unknown, "object")],
            variadic: false,
        },
    ),
    (
        "get_class_methods",
        Sig {
            req: 1,
            params: &[p("object_or_class", Required, "object|string")],
            variadic: false,
        },
    ),
    (
        "get_class_vars",
        Sig {
            req: 1,
            params: &[p("class", Required, "string")],
            variadic: false,
        },
    ),
    (
        "get_debug_type",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "get_declared_classes",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "get_defined_constants",
        Sig {
            req: 0,
            params: &[p("categorize", Bool(false), "bool")],
            variadic: false,
        },
    ),
    (
        "get_defined_vars",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "get_html_translation_table",
        Sig {
            req: 0,
            params: &[
                p("table", Int(0), "int"),
                p("flags", Int(11), "int"),
                p("encoding", Str("UTF-8"), "string"),
            ],
            variadic: false,
        },
    ),
    (
        "get_include_path",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "get_included_files",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "get_object_vars",
        Sig {
            req: 1,
            params: &[p("object", Required, "object")],
            variadic: false,
        },
    ),
    (
        "get_parent_class",
        Sig {
            req: 0,
            params: &[p("object_or_class", Unknown, "object|string")],
            variadic: false,
        },
    ),
    (
        "get_required_files",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "get_resource_id",
        Sig {
            req: 1,
            params: &[p("resource", Required, "")],
            variadic: false,
        },
    ),
    (
        "get_resource_type",
        Sig {
            req: 1,
            params: &[p("resource", Required, "")],
            variadic: false,
        },
    ),
    (
        "getcwd",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "getdate",
        Sig {
            req: 0,
            params: &[p("timestamp", Null, "?int")],
            variadic: false,
        },
    ),
    (
        "getenv",
        Sig {
            req: 0,
            params: &[
                p("name", Null, "?string"),
                p("local_only", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "getmygid",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "getmypid",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "getmyuid",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "getrandmax",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "gettype",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "glob",
        Sig {
            req: 1,
            params: &[p("pattern", Required, "string"), p("flags", Int(0), "int")],
            variadic: false,
        },
    ),
    (
        "gmdate",
        Sig {
            req: 1,
            params: &[
                p("format", Required, "string"),
                p("timestamp", Null, "?int"),
            ],
            variadic: false,
        },
    ),
    (
        "gmmktime",
        Sig {
            req: 1,
            params: &[
                p("hour", Required, "int"),
                p("minute", Null, "?int"),
                p("second", Null, "?int"),
                p("month", Null, "?int"),
                p("day", Null, "?int"),
                p("year", Null, "?int"),
            ],
            variadic: false,
        },
    ),
    (
        "gmp_abs",
        Sig {
            req: 1,
            params: &[p("num", Required, "GMP|string|int")],
            variadic: false,
        },
    ),
    (
        "gmp_add",
        Sig {
            req: 2,
            params: &[
                p("num1", Required, "GMP|string|int"),
                p("num2", Required, "GMP|string|int"),
            ],
            variadic: false,
        },
    ),
    (
        "gmp_and",
        Sig {
            req: 2,
            params: &[
                p("num1", Required, "GMP|string|int"),
                p("num2", Required, "GMP|string|int"),
            ],
            variadic: false,
        },
    ),
    (
        "gmp_cmp",
        Sig {
            req: 2,
            params: &[
                p("num1", Required, "GMP|string|int"),
                p("num2", Required, "GMP|string|int"),
            ],
            variadic: false,
        },
    ),
    (
        "gmp_div",
        Sig {
            req: 2,
            params: &[
                p("num1", Required, "GMP|string|int"),
                p("num2", Required, "GMP|string|int"),
                p("rounding_mode", Int(0), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "gmp_div_q",
        Sig {
            req: 2,
            params: &[
                p("num1", Required, "GMP|string|int"),
                p("num2", Required, "GMP|string|int"),
                p("rounding_mode", Int(0), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "gmp_div_r",
        Sig {
            req: 2,
            params: &[
                p("num1", Required, "GMP|string|int"),
                p("num2", Required, "GMP|string|int"),
                p("rounding_mode", Int(0), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "gmp_fact",
        Sig {
            req: 1,
            params: &[p("num", Required, "GMP|string|int")],
            variadic: false,
        },
    ),
    (
        "gmp_gcd",
        Sig {
            req: 2,
            params: &[
                p("num1", Required, "GMP|string|int"),
                p("num2", Required, "GMP|string|int"),
            ],
            variadic: false,
        },
    ),
    (
        "gmp_init",
        Sig {
            req: 1,
            params: &[p("num", Required, "string|int"), p("base", Int(0), "int")],
            variadic: false,
        },
    ),
    (
        "gmp_intval",
        Sig {
            req: 1,
            params: &[p("num", Required, "GMP|string|int")],
            variadic: false,
        },
    ),
    (
        "gmp_lcm",
        Sig {
            req: 2,
            params: &[
                p("num1", Required, "GMP|string|int"),
                p("num2", Required, "GMP|string|int"),
            ],
            variadic: false,
        },
    ),
    (
        "gmp_mod",
        Sig {
            req: 2,
            params: &[
                p("num1", Required, "GMP|string|int"),
                p("num2", Required, "GMP|string|int"),
            ],
            variadic: false,
        },
    ),
    (
        "gmp_mul",
        Sig {
            req: 2,
            params: &[
                p("num1", Required, "GMP|string|int"),
                p("num2", Required, "GMP|string|int"),
            ],
            variadic: false,
        },
    ),
    (
        "gmp_neg",
        Sig {
            req: 1,
            params: &[p("num", Required, "GMP|string|int")],
            variadic: false,
        },
    ),
    (
        "gmp_or",
        Sig {
            req: 2,
            params: &[
                p("num1", Required, "GMP|string|int"),
                p("num2", Required, "GMP|string|int"),
            ],
            variadic: false,
        },
    ),
    (
        "gmp_perfect_square",
        Sig {
            req: 1,
            params: &[p("num", Required, "GMP|string|int")],
            variadic: false,
        },
    ),
    (
        "gmp_pow",
        Sig {
            req: 2,
            params: &[
                p("num", Required, "GMP|string|int"),
                p("exponent", Required, "int"),
            ],
            variadic: false,
        },
    ),
    (
        "gmp_powm",
        Sig {
            req: 3,
            params: &[
                p("num", Required, "GMP|string|int"),
                p("exponent", Required, "GMP|string|int"),
                p("modulus", Required, "GMP|string|int"),
            ],
            variadic: false,
        },
    ),
    (
        "gmp_prob_prime",
        Sig {
            req: 1,
            params: &[
                p("num", Required, "GMP|string|int"),
                p("repetitions", Int(10), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "gmp_root",
        Sig {
            req: 2,
            params: &[
                p("num", Required, "GMP|string|int"),
                p("nth", Required, "int"),
            ],
            variadic: false,
        },
    ),
    (
        "gmp_sign",
        Sig {
            req: 1,
            params: &[p("num", Required, "GMP|string|int")],
            variadic: false,
        },
    ),
    (
        "gmp_sqrt",
        Sig {
            req: 1,
            params: &[p("num", Required, "GMP|string|int")],
            variadic: false,
        },
    ),
    (
        "gmp_strval",
        Sig {
            req: 1,
            params: &[
                p("num", Required, "GMP|string|int"),
                p("base", Int(10), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "gmp_sub",
        Sig {
            req: 2,
            params: &[
                p("num1", Required, "GMP|string|int"),
                p("num2", Required, "GMP|string|int"),
            ],
            variadic: false,
        },
    ),
    (
        "gmp_xor",
        Sig {
            req: 2,
            params: &[
                p("num1", Required, "GMP|string|int"),
                p("num2", Required, "GMP|string|int"),
            ],
            variadic: false,
        },
    ),
    (
        "hash",
        Sig {
            req: 2,
            params: &[
                p("algo", Required, "string"),
                p("data", Required, "string"),
                p("binary", Bool(false), "bool"),
                p("options", EmptyArray, "array"),
            ],
            variadic: false,
        },
    ),
    (
        "hash_algos",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "hash_equals",
        Sig {
            req: 2,
            params: &[
                p("known_string", Required, "string"),
                p("user_string", Required, "string"),
            ],
            variadic: false,
        },
    ),
    (
        "hash_file",
        Sig {
            req: 2,
            params: &[
                p("algo", Required, "string"),
                p("filename", Required, "string"),
                p("binary", Bool(false), "bool"),
                p("options", EmptyArray, "array"),
            ],
            variadic: false,
        },
    ),
    (
        "hash_hmac",
        Sig {
            req: 3,
            params: &[
                p("algo", Required, "string"),
                p("data", Required, "string"),
                p("key", Required, "string"),
                p("binary", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "hash_hmac_file",
        Sig {
            req: 3,
            params: &[
                p("algo", Required, "string"),
                p("filename", Required, "string"),
                p("key", Required, "string"),
                p("binary", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "hash_pbkdf2",
        Sig {
            req: 4,
            params: &[
                p("algo", Required, "string"),
                p("password", Required, "string"),
                p("salt", Required, "string"),
                p("iterations", Required, "int"),
                p("length", Int(0), "int"),
                p("binary", Bool(false), "bool"),
                p("options", EmptyArray, "array"),
            ],
            variadic: false,
        },
    ),
    (
        "hex2bin",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "hexdec",
        Sig {
            req: 1,
            params: &[p("hex_string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "html_entity_decode",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("flags", Int(11), "int"),
                p("encoding", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "htmlentities",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("flags", Int(11), "int"),
                p("encoding", Null, "?string"),
                p("double_encode", Bool(true), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "htmlspecialchars",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("flags", Int(11), "int"),
                p("encoding", Null, "?string"),
                p("double_encode", Bool(true), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "htmlspecialchars_decode",
        Sig {
            req: 1,
            params: &[p("string", Required, "string"), p("flags", Int(11), "int")],
            variadic: false,
        },
    ),
    (
        "http_build_query",
        Sig {
            req: 1,
            params: &[
                p("data", Required, "object|array"),
                p("numeric_prefix", Str(""), "string"),
                p("arg_separator", Null, "?string"),
                p("encoding_type", Int(1), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "hypot",
        Sig {
            req: 2,
            params: &[p("x", Required, "float"), p("y", Required, "float")],
            variadic: false,
        },
    ),
    (
        "ignore_user_abort",
        Sig {
            req: 0,
            params: &[p("enable", Null, "?bool")],
            variadic: false,
        },
    ),
    (
        "implode",
        Sig {
            req: 1,
            params: &[
                p("separator", Required, "array|string"),
                p("array", Null, "?array"),
            ],
            variadic: false,
        },
    ),
    (
        "in_array",
        Sig {
            req: 2,
            params: &[
                p("needle", Required, "mixed"),
                p("haystack", Required, "array"),
                p("strict", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "ini_get",
        Sig {
            req: 1,
            params: &[p("option", Required, "string")],
            variadic: false,
        },
    ),
    (
        "ini_set",
        Sig {
            req: 2,
            params: &[
                p("option", Required, "string"),
                p("value", Required, "string|int|float|bool|null"),
            ],
            variadic: false,
        },
    ),
    (
        "intdiv",
        Sig {
            req: 2,
            params: &[p("num1", Required, "int"), p("num2", Required, "int")],
            variadic: false,
        },
    ),
    (
        "interface_exists",
        Sig {
            req: 1,
            params: &[
                p("interface", Required, "string"),
                p("autoload", Bool(true), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "intval",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed"), p("base", Int(10), "int")],
            variadic: false,
        },
    ),
    (
        "is_a",
        Sig {
            req: 2,
            params: &[
                p("object_or_class", Required, "mixed"),
                p("class", Required, "string"),
                p("allow_string", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "is_array",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "is_bool",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "is_callable",
        Sig {
            req: 1,
            params: &[
                p("value", Required, "mixed"),
                p("syntax_only", Bool(false), "bool"),
                p("callable_name", Null, ""),
            ],
            variadic: false,
        },
    ),
    (
        "is_countable",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "is_dir",
        Sig {
            req: 1,
            params: &[p("filename", Required, "string")],
            variadic: false,
        },
    ),
    (
        "is_double",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "is_executable",
        Sig {
            req: 1,
            params: &[p("filename", Required, "string")],
            variadic: false,
        },
    ),
    (
        "is_file",
        Sig {
            req: 1,
            params: &[p("filename", Required, "string")],
            variadic: false,
        },
    ),
    (
        "is_finite",
        Sig {
            req: 1,
            params: &[p("num", Required, "float")],
            variadic: false,
        },
    ),
    (
        "is_float",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "is_infinite",
        Sig {
            req: 1,
            params: &[p("num", Required, "float")],
            variadic: false,
        },
    ),
    (
        "is_int",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "is_integer",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "is_iterable",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "is_link",
        Sig {
            req: 1,
            params: &[p("filename", Required, "string")],
            variadic: false,
        },
    ),
    (
        "is_long",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "is_nan",
        Sig {
            req: 1,
            params: &[p("num", Required, "float")],
            variadic: false,
        },
    ),
    (
        "is_null",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "is_numeric",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "is_object",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "is_readable",
        Sig {
            req: 1,
            params: &[p("filename", Required, "string")],
            variadic: false,
        },
    ),
    (
        "is_resource",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "is_scalar",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "is_string",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "is_subclass_of",
        Sig {
            req: 2,
            params: &[
                p("object_or_class", Required, "mixed"),
                p("class", Required, "string"),
                p("allow_string", Bool(true), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "is_writable",
        Sig {
            req: 1,
            params: &[p("filename", Required, "string")],
            variadic: false,
        },
    ),
    (
        "is_writeable",
        Sig {
            req: 1,
            params: &[p("filename", Required, "string")],
            variadic: false,
        },
    ),
    (
        "iterator_apply",
        Sig {
            req: 2,
            params: &[
                p("iterator", Required, "Traversable"),
                p("callback", Required, "callable"),
                p("args", Null, "?array"),
            ],
            variadic: false,
        },
    ),
    (
        "iterator_count",
        Sig {
            req: 1,
            params: &[p("iterator", Required, "Traversable|array")],
            variadic: false,
        },
    ),
    (
        "iterator_to_array",
        Sig {
            req: 1,
            params: &[
                p("iterator", Required, "Traversable|array"),
                p("preserve_keys", Bool(true), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "join",
        Sig {
            req: 1,
            params: &[
                p("separator", Required, "array|string"),
                p("array", Null, "?array"),
            ],
            variadic: false,
        },
    ),
    (
        "json_decode",
        Sig {
            req: 1,
            params: &[
                p("json", Required, "string"),
                p("associative", Null, "?bool"),
                p("depth", Int(512), "int"),
                p("flags", Int(0), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "json_encode",
        Sig {
            req: 1,
            params: &[
                p("value", Required, "mixed"),
                p("flags", Int(0), "int"),
                p("depth", Int(512), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "json_last_error",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "json_last_error_msg",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "json_validate",
        Sig {
            req: 1,
            params: &[
                p("json", Required, "string"),
                p("depth", Int(512), "int"),
                p("flags", Int(0), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "key",
        Sig {
            req: 1,
            params: &[p("array", Required, "object|array")],
            variadic: false,
        },
    ),
    (
        "key_exists",
        Sig {
            req: 2,
            params: &[p("key", Required, ""), p("array", Required, "array")],
            variadic: false,
        },
    ),
    (
        "krsort",
        Sig {
            req: 1,
            params: &[p("array", Required, "array"), p("flags", Int(0), "int")],
            variadic: false,
        },
    ),
    (
        "ksort",
        Sig {
            req: 1,
            params: &[p("array", Required, "array"), p("flags", Int(0), "int")],
            variadic: false,
        },
    ),
    (
        "lcfirst",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "levenshtein",
        Sig {
            req: 2,
            params: &[
                p("string1", Required, "string"),
                p("string2", Required, "string"),
                p("insertion_cost", Int(1), "int"),
                p("replacement_cost", Int(1), "int"),
                p("deletion_cost", Int(1), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "log",
        Sig {
            req: 1,
            params: &[
                p("num", Required, "float"),
                p("base", Float(2.718281828459045), "float"),
            ],
            variadic: false,
        },
    ),
    (
        "log10",
        Sig {
            req: 1,
            params: &[p("num", Required, "float")],
            variadic: false,
        },
    ),
    (
        "log1p",
        Sig {
            req: 1,
            params: &[p("num", Required, "float")],
            variadic: false,
        },
    ),
    (
        "lstat",
        Sig {
            req: 1,
            params: &[p("filename", Required, "string")],
            variadic: false,
        },
    ),
    (
        "ltrim",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("characters", Str(" \n\r\t\u{B}\u{0}"), "string"),
            ],
            variadic: false,
        },
    ),
    (
        "max",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed")],
            variadic: true,
        },
    ),
    (
        "mb_check_encoding",
        Sig {
            req: 0,
            params: &[
                p("value", Null, "array|string|null"),
                p("encoding", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "mb_chr",
        Sig {
            req: 1,
            params: &[
                p("codepoint", Required, "int"),
                p("encoding", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "mb_convert_case",
        Sig {
            req: 2,
            params: &[
                p("string", Required, "string"),
                p("mode", Required, "int"),
                p("encoding", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "mb_convert_encoding",
        Sig {
            req: 2,
            params: &[
                p("string", Required, "array|string"),
                p("to_encoding", Required, "string"),
                p("from_encoding", Null, "array|string|null"),
            ],
            variadic: false,
        },
    ),
    (
        "mb_convert_kana",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("mode", Str("KV"), "string"),
                p("encoding", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "mb_detect_encoding",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("encodings", Null, "array|string|null"),
                p("strict", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "mb_internal_encoding",
        Sig {
            req: 0,
            params: &[p("encoding", Null, "?string")],
            variadic: false,
        },
    ),
    (
        "mb_lcfirst",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("encoding", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "mb_ord",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("encoding", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "mb_scrub",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("encoding", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "mb_split",
        Sig {
            req: 2,
            params: &[
                p("pattern", Required, "string"),
                p("string", Required, "string"),
                p("limit", Int(-1), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "mb_str_pad",
        Sig {
            req: 2,
            params: &[
                p("string", Required, "string"),
                p("length", Required, "int"),
                p("pad_string", Str(" "), "string"),
                p("pad_type", Int(1), "int"),
                p("encoding", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "mb_str_split",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("length", Int(1), "int"),
                p("encoding", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "mb_strcut",
        Sig {
            req: 2,
            params: &[
                p("string", Required, "string"),
                p("start", Required, "int"),
                p("length", Null, "?int"),
                p("encoding", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "mb_stripos",
        Sig {
            req: 2,
            params: &[
                p("haystack", Required, "string"),
                p("needle", Required, "string"),
                p("offset", Int(0), "int"),
                p("encoding", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "mb_strlen",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("encoding", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "mb_strpos",
        Sig {
            req: 2,
            params: &[
                p("haystack", Required, "string"),
                p("needle", Required, "string"),
                p("offset", Int(0), "int"),
                p("encoding", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "mb_strripos",
        Sig {
            req: 2,
            params: &[
                p("haystack", Required, "string"),
                p("needle", Required, "string"),
                p("offset", Int(0), "int"),
                p("encoding", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "mb_strrpos",
        Sig {
            req: 2,
            params: &[
                p("haystack", Required, "string"),
                p("needle", Required, "string"),
                p("offset", Int(0), "int"),
                p("encoding", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "mb_strtolower",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("encoding", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "mb_strtoupper",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("encoding", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "mb_strwidth",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("encoding", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "mb_substr",
        Sig {
            req: 2,
            params: &[
                p("string", Required, "string"),
                p("start", Required, "int"),
                p("length", Null, "?int"),
                p("encoding", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "mb_substr_count",
        Sig {
            req: 2,
            params: &[
                p("haystack", Required, "string"),
                p("needle", Required, "string"),
                p("encoding", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "mb_ucfirst",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("encoding", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "md5",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("binary", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "md5_file",
        Sig {
            req: 1,
            params: &[
                p("filename", Required, "string"),
                p("binary", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "memory_get_peak_usage",
        Sig {
            req: 0,
            params: &[p("real_usage", Bool(false), "bool")],
            variadic: false,
        },
    ),
    (
        "memory_get_usage",
        Sig {
            req: 0,
            params: &[p("real_usage", Bool(false), "bool")],
            variadic: false,
        },
    ),
    (
        "metaphone",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("max_phonemes", Int(0), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "method_exists",
        Sig {
            req: 2,
            params: &[
                p("object_or_class", Required, ""),
                p("method", Required, "string"),
            ],
            variadic: false,
        },
    ),
    (
        "microtime",
        Sig {
            req: 0,
            params: &[p("as_float", Bool(false), "bool")],
            variadic: false,
        },
    ),
    (
        "min",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed")],
            variadic: true,
        },
    ),
    (
        "mkdir",
        Sig {
            req: 1,
            params: &[
                p("directory", Required, "string"),
                p("permissions", Int(511), "int"),
                p("recursive", Bool(false), "bool"),
                p("context", Null, ""),
            ],
            variadic: false,
        },
    ),
    (
        "mktime",
        Sig {
            req: 1,
            params: &[
                p("hour", Required, "int"),
                p("minute", Null, "?int"),
                p("second", Null, "?int"),
                p("month", Null, "?int"),
                p("day", Null, "?int"),
                p("year", Null, "?int"),
            ],
            variadic: false,
        },
    ),
    (
        "mt_getrandmax",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "mt_rand",
        Sig {
            req: 0,
            params: &[p("min", Unknown, "int"), p("max", Unknown, "int")],
            variadic: false,
        },
    ),
    (
        "mt_srand",
        Sig {
            req: 0,
            params: &[p("seed", Null, "?int"), p("mode", Int(0), "int")],
            variadic: false,
        },
    ),
    (
        "natcasesort",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: false,
        },
    ),
    (
        "natsort",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: false,
        },
    ),
    (
        "next",
        Sig {
            req: 1,
            params: &[p("array", Required, "object|array")],
            variadic: false,
        },
    ),
    (
        "nl2br",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("use_xhtml", Bool(true), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "number_format",
        Sig {
            req: 1,
            params: &[
                p("num", Required, "float"),
                p("decimals", Int(0), "int"),
                p("decimal_separator", Str("."), "?string"),
                p("thousands_separator", Str(","), "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "ob_end_clean",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "ob_end_flush",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "ob_flush",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "ob_get_clean",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "ob_get_contents",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "ob_get_flush",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "ob_get_length",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "ob_get_level",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "ob_start",
        Sig {
            req: 0,
            params: &[
                p("callback", Null, ""),
                p("chunk_size", Int(0), "int"),
                p("flags", Int(112), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "octdec",
        Sig {
            req: 1,
            params: &[p("octal_string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "ord",
        Sig {
            req: 1,
            params: &[p("character", Required, "string")],
            variadic: false,
        },
    ),
    (
        "parse_str",
        Sig {
            req: 2,
            params: &[p("string", Required, "string"), p("result", Required, "")],
            variadic: false,
        },
    ),
    (
        "parse_url",
        Sig {
            req: 1,
            params: &[p("url", Required, "string"), p("component", Int(-1), "int")],
            variadic: false,
        },
    ),
    (
        "pathinfo",
        Sig {
            req: 1,
            params: &[p("path", Required, "string"), p("flags", Int(15), "int")],
            variadic: false,
        },
    ),
    (
        "php_ini_loaded_file",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "php_sapi_name",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "php_uname",
        Sig {
            req: 0,
            params: &[p("mode", Str("a"), "string")],
            variadic: false,
        },
    ),
    (
        "phpversion",
        Sig {
            req: 0,
            params: &[p("extension", Null, "?string")],
            variadic: false,
        },
    ),
    (
        "pi",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "pos",
        Sig {
            req: 1,
            params: &[p("array", Required, "object|array")],
            variadic: false,
        },
    ),
    (
        "pow",
        Sig {
            req: 2,
            params: &[
                p("num", Required, "mixed"),
                p("exponent", Required, "mixed"),
            ],
            variadic: false,
        },
    ),
    (
        "preg_grep",
        Sig {
            req: 2,
            params: &[
                p("pattern", Required, "string"),
                p("array", Required, "array"),
                p("flags", Int(0), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "preg_last_error",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "preg_last_error_msg",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "preg_match",
        Sig {
            req: 2,
            params: &[
                p("pattern", Required, "string"),
                p("subject", Required, "string"),
                p("matches", Null, ""),
                p("flags", Int(0), "int"),
                p("offset", Int(0), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "preg_match_all",
        Sig {
            req: 2,
            params: &[
                p("pattern", Required, "string"),
                p("subject", Required, "string"),
                p("matches", Null, ""),
                p("flags", Int(0), "int"),
                p("offset", Int(0), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "preg_quote",
        Sig {
            req: 1,
            params: &[
                p("str", Required, "string"),
                p("delimiter", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "preg_replace",
        Sig {
            req: 3,
            params: &[
                p("pattern", Required, "array|string"),
                p("replacement", Required, "array|string"),
                p("subject", Required, "array|string"),
                p("limit", Int(-1), "int"),
                p("count", Null, ""),
            ],
            variadic: false,
        },
    ),
    (
        "preg_replace_callback",
        Sig {
            req: 3,
            params: &[
                p("pattern", Required, "array|string"),
                p("callback", Required, "callable"),
                p("subject", Required, "array|string"),
                p("limit", Int(-1), "int"),
                p("count", Null, ""),
                p("flags", Int(0), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "preg_split",
        Sig {
            req: 2,
            params: &[
                p("pattern", Required, "string"),
                p("subject", Required, "string"),
                p("limit", Int(-1), "int"),
                p("flags", Int(0), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "prev",
        Sig {
            req: 1,
            params: &[p("array", Required, "object|array")],
            variadic: false,
        },
    ),
    (
        "print_r",
        Sig {
            req: 1,
            params: &[
                p("value", Required, "mixed"),
                p("return", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "printf",
        Sig {
            req: 1,
            params: &[p("format", Required, "string")],
            variadic: true,
        },
    ),
    (
        "property_exists",
        Sig {
            req: 2,
            params: &[
                p("object_or_class", Required, ""),
                p("property", Required, "string"),
            ],
            variadic: false,
        },
    ),
    (
        "putenv",
        Sig {
            req: 1,
            params: &[p("assignment", Required, "string")],
            variadic: false,
        },
    ),
    (
        "quoted_printable_decode",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "quoted_printable_encode",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "quotemeta",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "rad2deg",
        Sig {
            req: 1,
            params: &[p("num", Required, "float")],
            variadic: false,
        },
    ),
    (
        "rand",
        Sig {
            req: 0,
            params: &[p("min", Unknown, "int"), p("max", Unknown, "int")],
            variadic: false,
        },
    ),
    (
        "random_bytes",
        Sig {
            req: 1,
            params: &[p("length", Required, "int")],
            variadic: false,
        },
    ),
    (
        "random_int",
        Sig {
            req: 2,
            params: &[p("min", Required, "int"), p("max", Required, "int")],
            variadic: false,
        },
    ),
    (
        "range",
        Sig {
            req: 2,
            params: &[
                p("start", Required, "string|int|float"),
                p("end", Required, "string|int|float"),
                p("step", Int(1), "int|float"),
            ],
            variadic: false,
        },
    ),
    (
        "rawurldecode",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "rawurlencode",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "readfile",
        Sig {
            req: 1,
            params: &[
                p("filename", Required, "string"),
                p("use_include_path", Bool(false), "bool"),
                p("context", Null, ""),
            ],
            variadic: false,
        },
    ),
    (
        "realpath",
        Sig {
            req: 1,
            params: &[p("path", Required, "string")],
            variadic: false,
        },
    ),
    (
        "register_shutdown_function",
        Sig {
            req: 1,
            params: &[p("callback", Required, "callable")],
            variadic: true,
        },
    ),
    (
        "rename",
        Sig {
            req: 2,
            params: &[
                p("from", Required, "string"),
                p("to", Required, "string"),
                p("context", Null, ""),
            ],
            variadic: false,
        },
    ),
    (
        "reset",
        Sig {
            req: 1,
            params: &[p("array", Required, "object|array")],
            variadic: false,
        },
    ),
    (
        "restore_error_handler",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "restore_exception_handler",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "rewind",
        Sig {
            req: 1,
            params: &[p("stream", Required, "")],
            variadic: false,
        },
    ),
    (
        "rmdir",
        Sig {
            req: 1,
            params: &[p("directory", Required, "string"), p("context", Null, "")],
            variadic: false,
        },
    ),
    (
        "round",
        Sig {
            req: 1,
            params: &[
                p("num", Required, "int|float"),
                p("precision", Int(0), "int"),
                p("mode", Unknown, "RoundingMode|int"),
            ],
            variadic: false,
        },
    ),
    (
        "rsort",
        Sig {
            req: 1,
            params: &[p("array", Required, "array"), p("flags", Int(0), "int")],
            variadic: false,
        },
    ),
    (
        "rtrim",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("characters", Str(" \n\r\t\u{B}\u{0}"), "string"),
            ],
            variadic: false,
        },
    ),
    (
        "scandir",
        Sig {
            req: 1,
            params: &[
                p("directory", Required, "string"),
                p("sorting_order", Int(0), "int"),
                p("context", Null, ""),
            ],
            variadic: false,
        },
    ),
    (
        "serialize",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "set_error_handler",
        Sig {
            req: 1,
            params: &[
                p("callback", Required, "?callable"),
                p("error_levels", Int(30719), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "set_exception_handler",
        Sig {
            req: 1,
            params: &[p("callback", Required, "?callable")],
            variadic: false,
        },
    ),
    (
        "set_time_limit",
        Sig {
            req: 1,
            params: &[p("seconds", Required, "int")],
            variadic: false,
        },
    ),
    (
        "settype",
        Sig {
            req: 2,
            params: &[p("var", Required, "mixed"), p("type", Required, "string")],
            variadic: false,
        },
    ),
    (
        "sha1",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("binary", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "sha1_file",
        Sig {
            req: 1,
            params: &[
                p("filename", Required, "string"),
                p("binary", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "shuffle",
        Sig {
            req: 1,
            params: &[p("array", Required, "array")],
            variadic: false,
        },
    ),
    (
        "similar_text",
        Sig {
            req: 2,
            params: &[
                p("string1", Required, "string"),
                p("string2", Required, "string"),
                p("percent", Null, ""),
            ],
            variadic: false,
        },
    ),
    (
        "sin",
        Sig {
            req: 1,
            params: &[p("num", Required, "float")],
            variadic: false,
        },
    ),
    (
        "sinh",
        Sig {
            req: 1,
            params: &[p("num", Required, "float")],
            variadic: false,
        },
    ),
    (
        "sizeof",
        Sig {
            req: 1,
            params: &[
                p("value", Required, "Countable|array"),
                p("mode", Int(0), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "sleep",
        Sig {
            req: 1,
            params: &[p("seconds", Required, "int")],
            variadic: false,
        },
    ),
    (
        "sort",
        Sig {
            req: 1,
            params: &[p("array", Required, "array"), p("flags", Int(0), "int")],
            variadic: false,
        },
    ),
    (
        "soundex",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "spl_autoload_register",
        Sig {
            req: 0,
            params: &[
                p("callback", Null, "?callable"),
                p("throw", Bool(true), "bool"),
                p("prepend", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "spl_autoload_unregister",
        Sig {
            req: 1,
            params: &[p("callback", Required, "callable")],
            variadic: false,
        },
    ),
    (
        "spl_object_hash",
        Sig {
            req: 1,
            params: &[p("object", Required, "object")],
            variadic: false,
        },
    ),
    (
        "spl_object_id",
        Sig {
            req: 1,
            params: &[p("object", Required, "object")],
            variadic: false,
        },
    ),
    (
        "sprintf",
        Sig {
            req: 1,
            params: &[p("format", Required, "string")],
            variadic: true,
        },
    ),
    (
        "sqrt",
        Sig {
            req: 1,
            params: &[p("num", Required, "float")],
            variadic: false,
        },
    ),
    (
        "srand",
        Sig {
            req: 0,
            params: &[p("seed", Null, "?int"), p("mode", Int(0), "int")],
            variadic: false,
        },
    ),
    (
        "sscanf",
        Sig {
            req: 2,
            params: &[
                p("string", Required, "string"),
                p("format", Required, "string"),
            ],
            variadic: true,
        },
    ),
    (
        "stat",
        Sig {
            req: 1,
            params: &[p("filename", Required, "string")],
            variadic: false,
        },
    ),
    (
        "str_contains",
        Sig {
            req: 2,
            params: &[
                p("haystack", Required, "string"),
                p("needle", Required, "string"),
            ],
            variadic: false,
        },
    ),
    (
        "str_ends_with",
        Sig {
            req: 2,
            params: &[
                p("haystack", Required, "string"),
                p("needle", Required, "string"),
            ],
            variadic: false,
        },
    ),
    (
        "str_getcsv",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("separator", Str(","), "string"),
                p("enclosure", Str("\""), "string"),
                p("escape", Str("\\"), "string"),
            ],
            variadic: false,
        },
    ),
    (
        "str_ireplace",
        Sig {
            req: 3,
            params: &[
                p("search", Required, "array|string"),
                p("replace", Required, "array|string"),
                p("subject", Required, "array|string"),
                p("count", Null, ""),
            ],
            variadic: false,
        },
    ),
    (
        "str_pad",
        Sig {
            req: 2,
            params: &[
                p("string", Required, "string"),
                p("length", Required, "int"),
                p("pad_string", Str(" "), "string"),
                p("pad_type", Int(1), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "str_repeat",
        Sig {
            req: 2,
            params: &[p("string", Required, "string"), p("times", Required, "int")],
            variadic: false,
        },
    ),
    (
        "str_replace",
        Sig {
            req: 3,
            params: &[
                p("search", Required, "array|string"),
                p("replace", Required, "array|string"),
                p("subject", Required, "array|string"),
                p("count", Null, ""),
            ],
            variadic: false,
        },
    ),
    (
        "str_rot13",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "str_split",
        Sig {
            req: 1,
            params: &[p("string", Required, "string"), p("length", Int(1), "int")],
            variadic: false,
        },
    ),
    (
        "str_starts_with",
        Sig {
            req: 2,
            params: &[
                p("haystack", Required, "string"),
                p("needle", Required, "string"),
            ],
            variadic: false,
        },
    ),
    (
        "str_word_count",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("format", Int(0), "int"),
                p("characters", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "strcasecmp",
        Sig {
            req: 2,
            params: &[
                p("string1", Required, "string"),
                p("string2", Required, "string"),
            ],
            variadic: false,
        },
    ),
    (
        "strchr",
        Sig {
            req: 2,
            params: &[
                p("haystack", Required, "string"),
                p("needle", Required, "string"),
                p("before_needle", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "strcmp",
        Sig {
            req: 2,
            params: &[
                p("string1", Required, "string"),
                p("string2", Required, "string"),
            ],
            variadic: false,
        },
    ),
    (
        "strcspn",
        Sig {
            req: 2,
            params: &[
                p("string", Required, "string"),
                p("characters", Required, "string"),
                p("offset", Int(0), "int"),
                p("length", Null, "?int"),
            ],
            variadic: false,
        },
    ),
    (
        "stream_get_contents",
        Sig {
            req: 1,
            params: &[
                p("stream", Required, ""),
                p("length", Null, "?int"),
                p("offset", Int(-1), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "strip_tags",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("allowed_tags", Null, "array|string|null"),
            ],
            variadic: false,
        },
    ),
    (
        "stripcslashes",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "stripos",
        Sig {
            req: 2,
            params: &[
                p("haystack", Required, "string"),
                p("needle", Required, "string"),
                p("offset", Int(0), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "stripslashes",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "stristr",
        Sig {
            req: 2,
            params: &[
                p("haystack", Required, "string"),
                p("needle", Required, "string"),
                p("before_needle", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "strlen",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "strnatcasecmp",
        Sig {
            req: 2,
            params: &[
                p("string1", Required, "string"),
                p("string2", Required, "string"),
            ],
            variadic: false,
        },
    ),
    (
        "strnatcmp",
        Sig {
            req: 2,
            params: &[
                p("string1", Required, "string"),
                p("string2", Required, "string"),
            ],
            variadic: false,
        },
    ),
    (
        "strncasecmp",
        Sig {
            req: 3,
            params: &[
                p("string1", Required, "string"),
                p("string2", Required, "string"),
                p("length", Required, "int"),
            ],
            variadic: false,
        },
    ),
    (
        "strncmp",
        Sig {
            req: 3,
            params: &[
                p("string1", Required, "string"),
                p("string2", Required, "string"),
                p("length", Required, "int"),
            ],
            variadic: false,
        },
    ),
    (
        "strpbrk",
        Sig {
            req: 2,
            params: &[
                p("string", Required, "string"),
                p("characters", Required, "string"),
            ],
            variadic: false,
        },
    ),
    (
        "strpos",
        Sig {
            req: 2,
            params: &[
                p("haystack", Required, "string"),
                p("needle", Required, "string"),
                p("offset", Int(0), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "strrchr",
        Sig {
            req: 2,
            params: &[
                p("haystack", Required, "string"),
                p("needle", Required, "string"),
                p("before_needle", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "strrev",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "strripos",
        Sig {
            req: 2,
            params: &[
                p("haystack", Required, "string"),
                p("needle", Required, "string"),
                p("offset", Int(0), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "strrpos",
        Sig {
            req: 2,
            params: &[
                p("haystack", Required, "string"),
                p("needle", Required, "string"),
                p("offset", Int(0), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "strspn",
        Sig {
            req: 2,
            params: &[
                p("string", Required, "string"),
                p("characters", Required, "string"),
                p("offset", Int(0), "int"),
                p("length", Null, "?int"),
            ],
            variadic: false,
        },
    ),
    (
        "strstr",
        Sig {
            req: 2,
            params: &[
                p("haystack", Required, "string"),
                p("needle", Required, "string"),
                p("before_needle", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "strtok",
        Sig {
            req: 1,
            params: &[p("string", Required, "string"), p("token", Null, "?string")],
            variadic: false,
        },
    ),
    (
        "strtolower",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "strtotime",
        Sig {
            req: 1,
            params: &[
                p("datetime", Required, "string"),
                p("baseTimestamp", Null, "?int"),
            ],
            variadic: false,
        },
    ),
    (
        "strtoupper",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "strtr",
        Sig {
            req: 2,
            params: &[
                p("string", Required, "string"),
                p("from", Required, "array|string"),
                p("to", Null, "?string"),
            ],
            variadic: false,
        },
    ),
    (
        "strval",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed")],
            variadic: false,
        },
    ),
    (
        "substr",
        Sig {
            req: 2,
            params: &[
                p("string", Required, "string"),
                p("offset", Required, "int"),
                p("length", Null, "?int"),
            ],
            variadic: false,
        },
    ),
    (
        "substr_compare",
        Sig {
            req: 3,
            params: &[
                p("haystack", Required, "string"),
                p("needle", Required, "string"),
                p("offset", Required, "int"),
                p("length", Null, "?int"),
                p("case_insensitive", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "substr_count",
        Sig {
            req: 2,
            params: &[
                p("haystack", Required, "string"),
                p("needle", Required, "string"),
                p("offset", Int(0), "int"),
                p("length", Null, "?int"),
            ],
            variadic: false,
        },
    ),
    (
        "substr_replace",
        Sig {
            req: 3,
            params: &[
                p("string", Required, "array|string"),
                p("replace", Required, "array|string"),
                p("offset", Required, "array|int"),
                p("length", Null, "array|int|null"),
            ],
            variadic: false,
        },
    ),
    (
        "sys_get_temp_dir",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "sys_getloadavg",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "tan",
        Sig {
            req: 1,
            params: &[p("num", Required, "float")],
            variadic: false,
        },
    ),
    (
        "tanh",
        Sig {
            req: 1,
            params: &[p("num", Required, "float")],
            variadic: false,
        },
    ),
    (
        "tempnam",
        Sig {
            req: 2,
            params: &[
                p("directory", Required, "string"),
                p("prefix", Required, "string"),
            ],
            variadic: false,
        },
    ),
    (
        "time",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "time_nanosleep",
        Sig {
            req: 2,
            params: &[
                p("seconds", Required, "int"),
                p("nanoseconds", Required, "int"),
            ],
            variadic: false,
        },
    ),
    (
        "tmpfile",
        Sig {
            req: 0,
            params: &[],
            variadic: false,
        },
    ),
    (
        "touch",
        Sig {
            req: 1,
            params: &[
                p("filename", Required, "string"),
                p("mtime", Null, "?int"),
                p("atime", Null, "?int"),
            ],
            variadic: false,
        },
    ),
    (
        "trait_exists",
        Sig {
            req: 1,
            params: &[
                p("trait", Required, "string"),
                p("autoload", Bool(true), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "trigger_error",
        Sig {
            req: 1,
            params: &[
                p("message", Required, "string"),
                p("error_level", Int(1024), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "trim",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("characters", Str(" \n\r\t\u{B}\u{0}"), "string"),
            ],
            variadic: false,
        },
    ),
    (
        "uasort",
        Sig {
            req: 2,
            params: &[
                p("array", Required, "array"),
                p("callback", Required, "callable"),
            ],
            variadic: false,
        },
    ),
    (
        "ucfirst",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "ucwords",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("separators", Str(" \t\r\n\u{C}\u{B}"), "string"),
            ],
            variadic: false,
        },
    ),
    (
        "uksort",
        Sig {
            req: 2,
            params: &[
                p("array", Required, "array"),
                p("callback", Required, "callable"),
            ],
            variadic: false,
        },
    ),
    (
        "uniqid",
        Sig {
            req: 0,
            params: &[
                p("prefix", Str(""), "string"),
                p("more_entropy", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "unlink",
        Sig {
            req: 1,
            params: &[p("filename", Required, "string"), p("context", Null, "")],
            variadic: false,
        },
    ),
    (
        "unserialize",
        Sig {
            req: 1,
            params: &[
                p("data", Required, "string"),
                p("options", EmptyArray, "array"),
            ],
            variadic: false,
        },
    ),
    (
        "urldecode",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "urlencode",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "user_error",
        Sig {
            req: 1,
            params: &[
                p("message", Required, "string"),
                p("error_level", Int(1024), "int"),
            ],
            variadic: false,
        },
    ),
    (
        "usleep",
        Sig {
            req: 1,
            params: &[p("microseconds", Required, "int")],
            variadic: false,
        },
    ),
    (
        "usort",
        Sig {
            req: 2,
            params: &[
                p("array", Required, "array"),
                p("callback", Required, "callable"),
            ],
            variadic: false,
        },
    ),
    (
        "utf8_decode",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "utf8_encode",
        Sig {
            req: 1,
            params: &[p("string", Required, "string")],
            variadic: false,
        },
    ),
    (
        "var_dump",
        Sig {
            req: 1,
            params: &[p("value", Required, "mixed")],
            variadic: true,
        },
    ),
    (
        "var_export",
        Sig {
            req: 1,
            params: &[
                p("value", Required, "mixed"),
                p("return", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
    (
        "vfprintf",
        Sig {
            req: 3,
            params: &[
                p("stream", Required, ""),
                p("format", Required, "string"),
                p("values", Required, "array"),
            ],
            variadic: false,
        },
    ),
    (
        "vprintf",
        Sig {
            req: 2,
            params: &[
                p("format", Required, "string"),
                p("values", Required, "array"),
            ],
            variadic: false,
        },
    ),
    (
        "vsprintf",
        Sig {
            req: 2,
            params: &[
                p("format", Required, "string"),
                p("values", Required, "array"),
            ],
            variadic: false,
        },
    ),
    (
        "wordwrap",
        Sig {
            req: 1,
            params: &[
                p("string", Required, "string"),
                p("width", Int(75), "int"),
                p("break", Str("\n"), "string"),
                p("cut_long_words", Bool(false), "bool"),
            ],
            variadic: false,
        },
    ),
];
