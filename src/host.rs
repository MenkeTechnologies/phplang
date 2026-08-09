//! The PHP object heap and runtime, reached from fusevm through registered
//! builtins (`builtins::install`) and the strict numeric hook.
//!
//! Scalars (int/float/bool/string/null) ride through the VM as native
//! `fusevm::Value`s. Arrays are heap objects: a `Value::Obj(u32)` handle indexes
//! `PhpHost::objs`. All mutable runtime state — the current variable scope stack,
//! the user-function table, the output buffer, the pending error/return signal —
//! lives in a `thread_local!` `PhpHost`, so a fresh `VM` can be spun up per
//! function call (see `call_function`) while sharing one heap.

use crate::ast::Visibility;
use crate::errlevel;
use fusevm::{Chunk, VMResult, Value, VM};
use indexmap::IndexMap;
use rustc_hash::FxHashMap;
use std::cell::RefCell;

/// Builtin ids emitted by the compiler and registered on every VM.
pub mod ops {
    pub const ECHO: u16 = 1; // [parts...] argc=n -> Undef (prints each)
    pub const GETVAR: u16 = 2; // [name] -> value
    pub const SETVAR: u16 = 3; // [name, value] -> value
    pub const CONCAT: u16 = 4; // [a, b] -> string
    pub const TRUTHY: u16 = 5; // [v] -> Bool (PHP truthiness)
    pub const CALL: u16 = 6; // [name, args...] argc=1+n -> return value
    pub const MKARRAY: u16 = 7; // [k,v,...] argc=2n -> array handle
    pub const INDEX_GET: u16 = 8; // [recv, key] -> value
    pub const INDEX_SET: u16 = 9; // [name, key, val] -> val ($var[key] = val)
    pub const ARR_APPEND: u16 = 10; // [name, val] -> val ($var[] = val)
    pub const DIV: u16 = 11; // [a, b] -> a / b
    pub const MOD: u16 = 12; // [a, b] -> a % b
    pub const POW: u16 = 13; // [a, b] -> a ** b
    pub const LOOSE_EQ: u16 = 14; // [a, b] -> Bool (==)
    pub const LOOSE_NE: u16 = 15; // [a, b] -> Bool (!=)
    pub const STRICT_EQ: u16 = 16; // [a, b] -> Bool (===)
    pub const STRICT_NE: u16 = 17; // [a, b] -> Bool (!==)
    pub const LT: u16 = 18; // [a, b] -> Bool (<)
    pub const GT: u16 = 19; // [a, b] -> Bool (>)
    pub const LE: u16 = 20; // [a, b] -> Bool (<=)
    pub const GE: u16 = 21; // [a, b] -> Bool (>=)
    pub const SIG_RETURN: u16 = 22; // [v] -> halt function, return v
    pub const INCDEC: u16 = 23; // [name, code] -> value (++/--)
    pub const ARRAYKEYS: u16 = 24; // [arr] -> array of keys
    pub const ARRAYLEN: u16 = 25; // [arr] -> Int element count
    pub const BITAND: u16 = 26; // [a, b] -> a & b
    pub const BITOR: u16 = 27; // [a, b] -> a | b
    pub const BITXOR: u16 = 28; // [a, b] -> a ^ b
    pub const SHL: u16 = 29; // [a, b] -> a << b
    pub const SHR: u16 = 30; // [a, b] -> a >> b
    pub const BITNOT: u16 = 31; // [a] -> ~a
    pub const SPACESHIP: u16 = 32; // [a, b] -> -1 | 0 | 1
    pub const DBG_LINE: u16 = 33; // [line] -> DAP statement marker (debug only)
    pub const ARR_MUT: u16 = 34; // [name, subop, args...] -> by-reference array mutator
    pub const GET_PATH: u16 = 35; // [name, k1..kN] -> value (read $a[k1]..[kN])
    pub const SET_PATH: u16 = 36; // [name, k1..kN, val] -> val ($a[k1]..[kN] = val, N>=1)
    pub const APPEND_PATH: u16 = 37; // [name, k1..kM, val] -> val ($a[k1]..[kM][] = val)
    pub const INCDEC_PATH: u16 = 38; // [name, k1..kN, code] -> value (++/-- on element)
    pub const CALL_SPREAD: u16 = 39; // [name, flag, val, ...] -> return value (arg unpacking)
    pub const MKCLOSURE: u16 = 40; // [def_name, k, v, ...] argc=1+2n -> closure handle
    pub const CALL_VALUE: u16 = 41; // [callee, args...] argc=1+n -> return value
    pub const NEW: u16 = 42; // [class, args...] argc=1+n -> object handle
    pub const PROP_GET: u16 = 43; // [recv, name] -> value ($o->p)
    pub const PROP_SET: u16 = 44; // [recv, name, val] -> val ($o->p = v)
    pub const MCALL: u16 = 45; // [recv, method, args...] argc=2+n -> return
    pub const SCALL: u16 = 46; // [class, method, args...] argc=2+n -> return
    pub const SCONST: u16 = 47; // [class, name] -> class constant value
    pub const PATH_APPEND_CHILD: u16 = 48; // [name, k1..kN] -> handle of a freshly appended child array
    pub const PROP_ENSURE_ARRAY: u16 = 49; // [recv, name] -> handle of the array held in $o->name (vivified)
    pub const PROP_INCDEC: u16 = 50; // [recv, name, code] -> value (++/-- on $o->p)
    pub const THROW: u16 = 51; // [exc] -> record pending exception, halt chunk
    pub const RUN_TRY: u16 = 52; // [try-def id] -> control status int (see run_try)
    pub const SIG_HALT: u16 = 53; // [] -> halt chunk (propagate an already-set signal)
    pub const SIG_BREAK: u16 = 54; // [] -> signal `break`, halt chunk
    pub const SIG_CONTINUE: u16 = 55; // [] -> signal `continue`, halt chunk
    pub const CONST_FETCH: u16 = 56; // [name] -> constant value (or the bare name)
    pub const UNSET_VAR: u16 = 57; // [name] -> Undef (remove the scope variable)
    pub const UNSET_PATH: u16 = 58; // [name, k1..kN] -> Undef (remove $a[k1]..[kN])
    pub const FOREACH_PREP: u16 = 59; // [v] -> an iterable array (object -> iterated)
    pub const INSTANCEOF: u16 = 60; // [obj, class-name] -> Bool
    pub const REF_BIND: u16 = 61; // [target, source] -> value ($t = &$s)
    pub const BYREF_OUT: u16 = 62; // [position] -> the last call's by-ref param value
    /// `[name, value]` — a source-level `$x = expr`. Same as [`SETVAR`] but the
    /// copied, everything else passes through. Emitted by the compiler on the
    /// right-hand side of a source-level assignment only, so a compiler
    /// temporary and a `&` binding still share the handle they are given.
    /// See `PhpHost::copy_on_assign`.
    pub const COPY: u16 = 81;
    /// `[name]` — the enclosing variable's reference cell as a `PhpObj::Ref`
    /// handle, promoting the variable to a cell if it was not one. What
    /// `use (&$v)` captures; binding one into a frame aliases the name to the
    /// cell rather than storing the handle.
    pub const REF_CELL: u16 = 82;
    pub const SPROP_GET: u16 = 63; // [class, name] -> static property value (Class::$p)
    pub const SPROP_SET: u16 = 64; // [class, name, val] -> val (Class::$p = v)
    pub const SPROP_INCDEC: u16 = 65; // [class, name, code] -> value (++/-- on Class::$p)
    pub const STATIC_BIND: u16 = 66; // [name, slot-key, init] -> Undef (bind `static $var`)
                                     // Named-argument call variants (PHP 8.0). Each argument is encoded as a
                                     // `(name, value)` pair on the stack — `name` is a `Str` for a named argument
                                     // or `Undef` for a positional one — so the host can rebind by parameter name.
    pub const CALL_NAMED: u16 = 67; // [fname, (n,v)...] argc=1+2k -> return value
    pub const MCALL_NAMED: u16 = 68; // [recv, method, (n,v)...] argc=2+2k -> return
    pub const SCALL_NAMED: u16 = 69; // [class, method, (n,v)...] argc=2+2k -> return
    pub const NEW_NAMED: u16 = 70; // [class, (n,v)...] argc=1+2k -> object handle
    pub const CALLVALUE_NAMED: u16 = 71; // [callee, (n,v)...] argc=1+2k -> return value
                                         // Generators (host-side stackful coroutines). YIELD/YIELD_KV/YIELD_FROM run
                                         // inside a generator body and suspend it; the GEN_* ops drive a Generator from
                                         // a `foreach` (lazily, preserving side-effect order).
    pub const YIELD: u16 = 72; // [value] -> the value the next send()/next() supplies
    pub const YIELD_KV: u16 = 73; // [value, key] -> same, with an explicit key
    pub const YIELD_FROM: u16 = 74; // [iterable] -> delegate; leaves the delegate's return
    pub const IS_GENERATOR: u16 = 75; // [v] -> Bool (v is a Generator handle)
    pub const GEN_REWIND: u16 = 76; // [gen] -> Undef (prime to the first yield)
    pub const GEN_VALID: u16 = 77; // [gen] -> Bool (not yet finished)
    pub const GEN_KEY: u16 = 78; // [gen] -> current key
    pub const GEN_CURRENT: u16 = 79; // [gen] -> current value
    pub const GEN_NEXT: u16 = 80; // [gen] -> Undef (resume to the next yield)

    pub const SIG_LEVEL: u16 = 83; // [] -> Int, level of the last break/continue

    // `&` bindings to a *container slot* (`$r = &$a['x']['y']`, `$a[] = &$x`,
    // `$o->p = &$x`). Split into an ACQUIRE half, which returns the reference
    // cell the right-hand side denotes — promoting the element/property into a
    // reference if it was a plain value — and a BIND half, which points the
    // left-hand side at that cell. Composing the two halves covers every
    // combination of variable / element / property on either side without an
    // opcode per pair. The slot travels between them as a plain `Int`; it is a
    // compiler-internal value that never reaches a PHP expression.
    pub const REF_SLOT_VAR: u16 = 84; // [name] -> Int slot of $name
    pub const REF_SLOT_ELEM: u16 = 85; // [name, k1..kN] -> Int slot of $name[k1]..[kN]
    pub const REF_SLOT_PROP: u16 = 86; // [recv, prop] -> Int slot of $recv->prop
    pub const REF_TO_VAR: u16 = 87; // [name, slot] -> Undef ($name = &<slot>)
    pub const REF_TO_ELEM: u16 = 88; // [name, k1..kN, slot] -> Undef
    pub const REF_TO_APPEND: u16 = 89; // [name, k1..kM, slot] -> Undef ($name[..][] = &…)
    pub const REF_TO_PROP: u16 = 90; // [recv, prop, slot] -> Undef
    /// `[slot]` — a `function &f()` body's `return <lvalue>`: publish the slot as
    /// the call's returned reference and leave its value, which is what a plain
    /// (non-`&`) call of the function sees.
    pub const RET_REF: u16 = 91;
    /// `[]` — the reference cell the last call returned, for `$r = &f()`. A call
    /// that did not return by reference yields a fresh detached cell holding the
    /// returned value, so the binding degrades to a private copy rather than
    /// aliasing something arbitrary.
    pub const REF_SLOT_RET: u16 = 92;

    // Diagnostic-free reads. PHP fetches a variable / element / property in
    // "isset mode" (`BP_VAR_IS`) inside `isset()`, `empty()` and the left operand
    // of `??`, where a missing one is the question being asked rather than a
    // mistake, and raises no diagnostic. These are the same reads as their loud
    // twins with the warning suppressed; the compiler picks between them
    // statically, so a function call nested in a key still warns normally.
    pub const GETVAR_Q: u16 = 93; // [name] -> value, no "Undefined variable"
    pub const INDEX_GET_Q: u16 = 94; // [recv, key] -> value, no "Undefined array key"
    pub const PROP_GET_Q: u16 = 95; // [recv, name] -> value, no "Undefined property"

    // Late static binding. `static::` names the class the call was made *on*, not
    // the class the running method was declared in, so it can only be resolved at
    // run time — a method inherited by two subclasses sees a different
    // `static::` in each.
    /// `[fallback]` -> the running frame's late-static-binding class. The
    /// compiler supplies the enclosing class as the fallback, which is what the
    /// name means outside any call (and what `self::` always means).
    pub const LSB_CLASS: u16 = 96;
    /// `[]` -> Undef. Marks the next call as *forwarding*: `self::m()`,
    /// `parent::m()` and `static::m()` keep the caller's late-static-binding
    /// class, whereas naming a class explicitly (`Base::m()`) replaces it.
    pub const LSB_FORWARD: u16 = 97;

    /// `[recv, name]` -> `recv`, unchanged. Marks a property as being fetched
    /// FOR WRITING, which is when PHP decides whether the write creates a dynamic
    /// property. Emitted ahead of the read half of a compound assignment
    /// (`$o->p .= "x"`), because the reference engine raises the
    /// "Creation of dynamic property" deprecation before the "Undefined property"
    /// warning the read then produces — the slot exists by the time it is read.
    pub const PROP_TOUCH: u16 = 98;
    /// `[recv, name, val]` -> `val`. The write half of a read-modify-write, whose
    /// creation of a dynamic property was already announced by the [`PROP_TOUCH`]
    /// that opened it. Identical to [`PROP_SET`] but silent, so `$o->p .= "x"`
    /// deprecates once rather than twice.
    pub const PROP_SET_RW: u16 = 99;
    /// `[] -> null`. Open an `@expr` suppression region (see
    /// [`super::PhpHost::suppress_push`]). The pushed null is dropped by the
    /// `Pop` the compiler emits after it.
    ///
    /// `@` needs a RUN-TIME region, not just the quiet read opcodes: a warning
    /// raised inside a library function (`@preg_match('/[a', $s)`,
    /// `@range('ab', 'c')`) is raised from Rust with no opcode of its own to
    /// quieten, and the reference silences those too.
    pub const SUPPRESS_PUSH: u16 = 100;
    /// `[value] -> value`. Close the region [`SUPPRESS_PUSH`] opened, passing the
    /// operand's value through so `@` stays an expression.
    pub const SUPPRESS_POP: u16 = 101;
    /// `[recv, name] -> null`. `unset($o->p)`.
    pub const PROP_UNSET: u16 = 102;
    /// `[recv, name] -> bool`. `isset($o->p)` — see [`super::PropAccess`] and the
    /// `IssetOf` arm of the compiler for why this cannot be a value comparison.
    pub const PROP_ISSET: u16 = 103;
    /// `[recv, name] -> value`. The read inside `empty($o->p)`. [`PROP_GET_Q`]
    /// with one arm changed: a property no `__isset` vouched for reads as null
    /// instead of falling back to `__get`, so a class with `__get` and no
    /// `__isset` is `empty()` without `__get` being called at all.
    pub const PROP_GET_EMPTY: u16 = 104;
}

/// Sub-ops for the by-reference array mutators lowered through `ops::ARR_MUT`
/// (`array_push`/`array_pop`/`array_shift`/`array_unshift`/`array_splice`). These
/// take the array by variable name so the host can rewrite the bound array (and
/// auto-vivify an unset variable) the way PHP's by-reference parameter does.
pub mod arrmut {
    pub const PUSH: i64 = 0;
    pub const POP: i64 = 1;
    pub const SHIFT: i64 = 2;
    pub const UNSHIFT: i64 = 3;
    pub const SPLICE: i64 = 4;
}

/// A compiled formal parameter: its name, an optional default-value chunk (run in
/// the callee scope when the caller omits the argument), and whether it collects
/// all trailing arguments into an array (`...$rest`).
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub default: Option<Chunk>,
    pub variadic: bool,
    /// `&$x` — a by-reference parameter (final value copied back to the caller).
    pub by_ref: bool,
}

/// A compiled user function: its parameters plus the lowered body chunk.
#[derive(Debug, Clone)]
pub struct FuncDef {
    pub params: Vec<Param>,
    pub chunk: Chunk,
    /// True when the body contains a `yield`: calling it builds a suspended
    /// `Generator` (a host-side stackful coroutine) instead of running the body.
    pub is_generator: bool,
}

/// A closure unpacked for a call: its parameters, body chunk, captured bindings,
/// and any `Closure::bind` rebinding (`$this` object + private-access scope class).
pub struct ClosureCall {
    pub params: Vec<Param>,
    pub chunk: Chunk,
    pub captured: Vec<(String, Value)>,
    pub bound_this: Option<Value>,
    pub scope: Option<String>,
    pub is_generator: bool,
}

/// A compiled class: its parent (for single-inheritance resolution), constant and
/// property-default initializers (each an expression chunk that leaves its value
/// on the stack), and its methods keyed by lowercase name. `self::`/`parent::`
/// were resolved to concrete class names at compile time.
#[derive(Debug, Clone)]
pub struct ClassDef {
    /// The class name in its declared spelling. The map is keyed by the
    /// lowercased name (PHP class lookup is case-insensitive), so this is the
    /// only place the source casing survives for display — a stack trace names
    /// the class that DEFINED the method, which may have no live instance to
    /// read the spelling back from.
    pub name: String,
    pub parent: Option<String>,
    /// Implemented interfaces (lowercased) — for an `interface`, the interfaces it
    /// extends. Consulted by `class_is_a`/`instanceof`/`catch`.
    pub interfaces: Vec<String>,
    pub consts: Vec<(String, Chunk)>,
    pub prop_defaults: Vec<(String, Chunk)>,
    /// `static` property declarations (`public static $n = 0;`). Kept apart from
    /// `prop_defaults` so they are never copied into an instance; each initializer
    /// runs once, on first access, into the host's per-class static store.
    pub static_prop_defaults: Vec<(String, Chunk)>,
    pub methods: FxHashMap<String, FuncDef>,
    /// Declared visibility of properties declared in THIS class (instance and
    /// static), by property name. Consulted (walking the parent chain) to enforce
    /// `private`/`protected` on external access.
    pub prop_vis: FxHashMap<String, Visibility>,
    /// Declared visibility of methods declared in THIS class, by lowercased name.
    pub method_vis: FxHashMap<String, Visibility>,
    /// Whether this class is an `enum` (PHP 8.1).
    pub is_enum: bool,
    /// Whether the class is `abstract` or an `interface` — either way, `new` on it
    /// is a fatal error in PHP.
    pub is_abstract: bool,
    pub is_interface: bool,
    /// Whether the class carries `#[AllowDynamicProperties]`, which opts it — and
    /// everything that extends it — out of the "Creation of dynamic property"
    /// deprecation. Checked by walking the parent chain, because the attribute is
    /// inherited.
    pub allow_dynamic_props: bool,
    /// `enum` cases, in source order: `(case name, optional backing-value chunk)`.
    /// Consulted to build the singleton case instances (`E::Case`, `E::cases()`,
    /// `E::from()`, `E::tryFrom()`).
    pub enum_cases: Vec<(String, Option<Chunk>)>,
}

/// One compiled `catch (T1 | T2 [$var]) { body }` clause of a `try`.
#[derive(Debug, Clone)]
pub struct CatchClause {
    pub classes: Vec<String>,
    pub var: Option<String>,
    pub chunk: Chunk,
}

/// A compiled `try`/`catch`/`finally` construct: each body is its own detached
/// chunk, run by the `run_try` orchestrator on the *current* scope (no new frame),
/// so variables are shared with the enclosing code.
#[derive(Debug, Clone)]
pub struct TryDef {
    pub try_chunk: Chunk,
    pub catches: Vec<CatchClause>,
    pub finally_chunk: Option<Chunk>,
}

/// A PHP array key — always an integer or a string (bool/float/null keys are
/// normalized to one of these on insert, as PHP does).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ArrayKey {
    Int(i64),
    Str(String),
}

impl ArrayKey {
    /// The key as a PHP `Value` (for `array_keys`/`foreach`).
    fn to_value(&self) -> Value {
        match self {
            ArrayKey::Int(n) => Value::int(*n),
            ArrayKey::Str(s) => Value::str(s.clone()),
        }
    }
}

/// A heap object. Arrays and closures live here in the scaffold; user-defined
/// objects are a later wave.
#[derive(Debug, Clone)]
pub enum PhpObj {
    Array {
        entries: IndexMap<ArrayKey, Value>,
        /// The next integer key an append (`$a[] = ...`) will use.
        next_index: i64,
    },
    /// A first-class callable: its parameters, its lowered body chunk, and the
    /// `(name, value)` bindings captured (by value) at creation time. `bound_this`
    /// / `scope` are set by `Closure::bind`/`bindTo`/`call`: the bound `$this`
    /// object and the class name whose private/protected members the body may reach.
    Closure {
        params: Vec<Param>,
        /// Boxed to keep the `Closure` variant from dominating `PhpObj`'s size
        /// (a `Chunk` is ~300 bytes; boxing it shrinks the enum to a handle).
        chunk: Box<Chunk>,
        captured: Vec<(String, Value)>,
        bound_this: Option<Value>,
        scope: Option<String>,
        is_generator: bool,
    },
    /// A live generator: an index into `PhpHost::generators`, where the suspended
    /// stackful coroutine and its execution context live. Built by calling a
    /// generator function; iterated by `foreach` or the `Generator` methods.
    Generator { id: u32 },
    /// A class instance: its class name and its properties. Referenced by a
    /// `Value::Obj(u32)` handle, so objects have PHP reference semantics (passing
    /// one around shares the same instance).
    Object {
        class: String,
        props: IndexMap<String, Value>,
    },
    /// A handle to a variable's reference cell, which is what `use (&$v)`
    /// captures. It is never a value a PHP script can hold: it is created at
    /// closure creation and consumed when the closure's frame is built, where
    /// it binds the frame's name to the cell instead of storing the handle.
    Ref { slot: usize },
    /// An open file stream (`fopen`). Content is buffered in memory: read modes
    /// load the file up front; write/append modes accumulate and flush to `path`
    /// on `fclose`/`fflush`. `Value::Obj` handle, so the position is shared.
    Resource {
        path: String,
        buf: Vec<u8>,
        pos: usize,
        writable: bool,
        dirty: bool,
        closed: bool,
    },
}

/// A control-flow signal that unwinds out of a function body. `Throw` rides a
/// separate `pending_throw` field (not this enum) so an in-flight exception is
/// never confused with a `return`/`break`/`continue` and survives frame unwinding.
enum Signal {
    Return(Value),
    /// `break n` — the payload is how many loop levels are still to be left,
    /// so a `try` that sits inside fewer loops than that can decrement it and
    /// re-raise for the next frame out.
    Break(u32),
    /// `continue n` — see [`Signal::Break`].
    Continue(u32),
}

/// One variable scope (the global scope, or a function-call frame).
#[derive(Default)]
struct Scope {
    vars: FxHashMap<String, Value>,
    /// Names bound as references (`$b = &$a`) map to a shared cell in
    /// `PhpHost::ref_cells`; reads/writes of such a name go through the cell so
    /// aliased variables observe each other's mutations.
    refs: FxHashMap<String, usize>,
    /// The source line this frame is currently executing. Under `--dap` every
    /// statement updates it; on the normal path only the ops that can start a
    /// call or raise a throw do (see `builtins::mark_frame_line`), which is all
    /// an exception backtrace needs — the frame's line at the moment it called
    /// deeper is exactly the call-site line PHP prints for the frame above it.
    line: u32,
    /// The function name for a call frame, `None` for the global scope. Reported
    /// as the frame name in a DAP `stackTrace`.
    name: Option<String>,
    /// The late-static-binding class of this frame — the class the call named,
    /// which `static::` resolves to. `None` outside a method call.
    static_class: Option<String>,
}

/// The PHP runtime state for one thread.
pub struct PhpHost {
    objs: Vec<PhpObj>,
    scopes: Vec<Scope>,
    functions: FxHashMap<String, FuncDef>,
    /// User-declared classes, keyed by lowercase class name.
    classes: FxHashMap<String, ClassDef>,
    /// When `Some`, `echo` appends here instead of writing to stdout (used by
    /// `eval_capture` and the test harness).
    capture: Option<String>,
    /// Output-buffering stack (`ob_start`/`ob_get_clean`/…). When non-empty,
    /// `echo` appends to the top buffer instead of the capture buffer / stdout.
    ob_stack: Vec<String>,
    error: Option<String>,
    signal: Option<Signal>,
    /// The in-flight exception object, if a `throw` has fired and not yet been
    /// caught. Kept apart from `signal` so it survives function-frame unwinding
    /// and bubbles through nested VMs on its own.
    pending_throw: Option<Value>,
    /// Level of the most recent `break`/`continue` signal — see
    /// [`last_break_level`].
    last_break_level: u32,
    /// Compiled `try`/`catch`/`finally` constructs, indexed by the id the
    /// compiler bakes into each `RUN_TRY` call.
    try_defs: Vec<TryDef>,
    /// Named constants (`PHP_EOL`, `M_PI`, user `define`s), keyed case-sensitively.
    /// Seeded with the standard predefined constants on every fresh host.
    constants: FxHashMap<String, Value>,
    /// Shared storage cells for reference bindings (`$b = &$a`). A scope's `refs`
    /// map points names at these slots.
    ref_cells: Vec<Value>,
    /// The most recent call's by-reference parameters' final values, indexed by
    /// parameter position — read by the caller's post-call write-back (`BYREF_OUT`).
    byref_out: Vec<Value>,
    /// Static-property storage, keyed by `"declaringclass::prop"` (lowercased
    /// class). One cell per declaring class is shared by every subclass and every
    /// instance, matching PHP's static-property semantics.
    static_props: FxHashMap<String, Value>,
    /// Persistent slots for function `static $var` locals, keyed by the
    /// compile-time-unique id the compiler bakes into each declaration. The value
    /// is an index into `ref_cells`, so a body's `$var` aliases the cell and its
    /// value survives across calls.
    static_slots: FxHashMap<String, usize>,
    /// Singleton `enum` case instances, keyed by `"enumlower::CaseName"`. Built
    /// once on first access so `E::Case === E::Case` holds by object identity.
    enum_case_cache: FxHashMap<String, Value>,
    /// What a diagnostic names as the source: the script path, or
    /// `"Command line code"` for `php -r`. Set once at startup.
    script_name: String,
    /// The source line a diagnostic raised right now belongs to. The builtin that
    /// is about to warn records it from the line table of the op it is executing,
    /// so the host never has to walk the VM.
    warn_line: u32,
    /// Nesting depth of `@expr` suppression regions; a warning raised while this
    /// is non-zero is discarded.
    suppress: usize,
    /// The late-static-binding class the next call's frame will take. Set from
    /// the called class name, or — for a forwarding call (`self::`, `parent::`,
    /// `static::`) — from the caller's own, and taken when the frame is pushed.
    pending_lsb: Option<String>,
    /// The reference cell the most recent `function &f()` return published, for
    /// the caller's `$r = &f()` to bind. Taken (not just read) by the binding so a
    /// stale slot can never be picked up by a later plain call.
    ret_ref_slot: Option<usize>,
    /// Live generators, indexed by the id a `PhpObj::Generator` handle carries.
    /// Each holds a suspended stackful coroutine plus its swapped-out execution
    /// context (its own call frame, in-flight signal/throw).
    generators: Vec<GenCell>,
    /// Set once a fatal error has been displayed in PHP's own shape (see
    /// [`PhpHost::fatal`]), so the CLI wrapper reports the exit status without
    /// printing the message a second time in its terse `php: …` form.
    fatal_reported: bool,
    /// The `error_reporting` mask: a diagnostic is displayed only when its `E_*`
    /// bit is set here. Seeded from `-d error_reporting=…` (or `E_ALL`), then
    /// writable at run time through `error_reporting()` / `ini_set`.
    error_reporting: i64,
    /// `ini_set`/`ini_get` string store for settings with no dedicated field, so a
    /// value written by one is read back by the other unchanged.
    ini: FxHashMap<String, String>,
    /// What `preg_last_error()` reports: the outcome of the last `preg_*` call
    /// that reached the regex compiler. Sticky — only another such call rewrites
    /// it, so `preg_last_error()` itself and `preg_quote()` leave it alone.
    preg_error: i64,
    /// Magic property accesses currently on the stack, as
    /// `(object handle, property, magic method)`. PHP does not re-enter a magic
    /// method for a property already being handled by it, which is what lets
    /// `__get($n) { return $this->$n ?? 'd'; }` terminate: the inner `$this->$n`
    /// finds no property, sees `__get` already in progress for that name, and
    /// takes the ordinary undefined-property path instead of recursing.
    magic_in_progress: Vec<(u32, String, &'static str)>,
}

/// What `$obj->name` should do, once the object's property table and the
/// visibility rules have both been consulted. Returned by
/// [`PhpHost::prop_access`], which every property opcode routes through so the
/// four of them cannot disagree about when a magic method fires.
pub enum PropAccess {
    /// The property is present on the object and reachable from here: read,
    /// write or remove the slot directly.
    Direct,
    /// Not directly reachable, and the class defines the magic method for this
    /// operation — call it.
    Magic,
    /// Not reachable, no magic method, and the property IS declared: its
    /// visibility puts it out of reach. Carries the reference's message.
    Denied(String),
    /// Not reachable, no magic method, and no such property. What that means is
    /// the caller's decision: a read warns and yields null, a write creates the
    /// property, an unset does nothing, an `isset` is false.
    Absent,
}

/// Ini settings applied to every host the moment it is created — what `php -d
/// name=value` writes.
///
/// A process-wide cell rather than an argument to `PhpHost::new`: hosts are
/// created implicitly on first `with_host` and thrown away by every
/// `reset_host`, so there is no single call site that could pass them in, and a
/// value set once on a host would not survive the next reset.
static INITIAL_INI: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

/// Record a `-d name=value` override for every host created from now on.
pub fn add_initial_ini(name: &str, value: &str) {
    let mut g = INITIAL_INI.lock().unwrap_or_else(|e| e.into_inner());
    g.retain(|(k, _)| k != name);
    g.push((name.to_string(), value.to_string()));
}

/// The ini settings this engine knows, with the values the reference CLI reports
/// for them when nothing overrides them.
///
/// Every entry here is an ENGINE default — compiled into the interpreter, not
/// read from a php.ini. The distinction is testable rather than a judgement
/// call: `php -n` starts with no ini file at all, so a setting whose value is
/// the same under `php -n` and `php` is one the engine supplies itself, and a
/// setting whose value differs between them came from the file. Only the former
/// are listed, which is why `memory_limit` (`128M`) and `date.timezone` (`UTC`)
/// belong here — both survive `php -n` unchanged — while `variables_order`,
/// `enable_dl` and `short_open_tag`, which do not, are deliberately absent.
///
/// Two further exclusions, for values that are engine defaults but not portable
/// ones: anything whose default is a build path (`extension_dir`,
/// `include_path`, `openssl.cafile`), which encodes the prefix the reference
/// happened to be built with, and settings belonging to optional extensions
/// (`mysqli.*`, `pgsql.*`, `tidy.*`), which a differently configured build does
/// not register at all. What remains is PHP core plus `date` and `pcre` — the
/// two extensions PHP 8 cannot be built without.
fn default_ini() -> FxHashMap<String, String> {
    [
        // The startup mask, also readable through `ini_get`. `-d` and
        // `error_reporting()` both rewrite this entry, so the two views agree.
        ("error_reporting", "30719"),
        // Error handling and output.
        ("precision", "14"),
        ("serialize_precision", "-1"),
        ("display_errors", "1"),
        ("display_startup_errors", "1"),
        ("log_errors", "1"),
        ("html_errors", "0"),
        ("docref_ext", ""),
        ("error_append_string", ""),
        ("error_prepend_string", ""),
        ("ignore_repeated_errors", "0"),
        ("ignore_repeated_source", "0"),
        ("report_memleaks", "1"),
        ("fatal_error_backtraces", "1"),
        ("output_buffering", "0"),
        ("implicit_flush", "1"),
        ("output_handler", ""),
        // Resource limits. `max_execution_time` and `max_input_time` are the CLI
        // SAPI's own hardcoded overrides, not the common defaults.
        ("memory_limit", "128M"),
        ("max_memory_limit", "-1"),
        ("max_execution_time", "0"),
        ("max_input_time", "-1"),
        ("max_input_nesting_level", "64"),
        ("max_input_vars", "1000"),
        ("post_max_size", "8M"),
        ("default_socket_timeout", "60"),
        ("hard_timeout", "2"),
        ("unserialize_max_depth", "4096"),
        ("unserialize_callback_func", ""),
        // Language and encoding.
        ("default_charset", "UTF-8"),
        ("default_mimetype", "text/html"),
        ("input_encoding", ""),
        ("internal_encoding", ""),
        ("output_encoding", ""),
        ("disable_functions", ""),
        ("expose_php", "1"),
        ("ignore_user_abort", "0"),
        ("register_argc_argv", "0"),
        ("auto_detect_line_endings", "0"),
        ("allow_url_fopen", "1"),
        ("arg_separator.input", "&"),
        ("arg_separator.output", "&"),
        ("user_agent", ""),
        ("from", ""),
        // Zend engine.
        ("zend.assertions", "1"),
        ("zend.enable_gc", "1"),
        ("zend.detect_unicode", "1"),
        ("zend.multibyte", "0"),
        ("zend.exception_ignore_args", "0"),
        ("zend.exception_string_param_max_len", "15"),
        ("zend.script_encoding", ""),
        // ext/date — always built.
        ("date.timezone", "UTC"),
        ("date.default_latitude", "31.7667"),
        ("date.default_longitude", "35.2333"),
        ("date.sunrise_zenith", "90.833333"),
        ("date.sunset_zenith", "90.833333"),
        // ext/pcre — always built.
        ("pcre.backtrack_limit", "1000000"),
        ("pcre.recursion_limit", "100000"),
        ("pcre.jit", "1"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

/// Settings from [`default_ini`] that `ini_set` cannot change at run time —
/// PHP's `PHP_INI_PERDIR`/`PHP_INI_SYSTEM` entries, which only a php.ini or a
/// `-d` may set. `ini_set` reports `false` for these and changes nothing, while
/// `ini_get` still reads them.
///
/// Determined by running `ini_set($name, ini_get($name))` on the reference for
/// every name in `default_ini` and recording which ones answered `false` —
/// writing a setting its OWN current value can only fail because the write
/// itself is refused.
const INI_FIXED: &[&str] = &[
    "allow_url_fopen",
    "arg_separator.input",
    "disable_functions",
    "expose_php",
    "hard_timeout",
    "max_input_nesting_level",
    "max_input_time",
    "max_input_vars",
    "max_memory_limit",
    "output_buffering",
    "output_handler",
    "post_max_size",
    "register_argc_argv",
    "zend.multibyte",
    "zend.script_encoding",
];

// DIVERGENCE — per-setting VALUE validation is not modelled. The reference
// refuses a value a setting will not take, with a `Warning` and `false`:
// `ini_set('date.timezone', '-1')` keeps UTC, `ini_set('memory_limit', '20')`
// keeps 128M. Neither is reproducible here. The timezone check needs a zone
// database this build does not carry (see `stdlib::datetime`, which documents
// the same gap for `date_default_timezone_set`), and `memory_limit`'s refusal
// message quotes the process's CURRENT memory usage in bytes, which is not a
// reproducible number. A write of a value the reference would refuse is
// therefore accepted here rather than guessed at; see the `ini_set` corpus entry.

impl Default for PhpHost {
    fn default() -> Self {
        Self::new()
    }
}

impl PhpHost {
    pub fn new() -> Self {
        let mut h = PhpHost {
            objs: Vec::new(),
            // Start with the global scope already open.
            scopes: vec![Scope::default()],
            functions: FxHashMap::default(),
            classes: FxHashMap::default(),
            capture: None,
            error: None,
            signal: None,
            pending_throw: None,
            last_break_level: 1,
            try_defs: Vec::new(),
            constants: predefined_constants(),
            ob_stack: Vec::new(),
            ref_cells: Vec::new(),
            byref_out: Vec::new(),
            static_props: FxHashMap::default(),
            static_slots: FxHashMap::default(),
            enum_case_cache: FxHashMap::default(),
            script_name: "Command line code".to_string(),
            warn_line: 0,
            suppress: 0,
            pending_lsb: None,
            ret_ref_slot: None,
            generators: Vec::new(),
            fatal_reported: false,
            error_reporting: errlevel::E_ALL,
            ini: default_ini(),
            preg_error: 0,
            magic_in_progress: Vec::new(),
        };
        h.init_superglobals();
        // `-d` overrides land before the program is read, so a compile-time
        // notice is already tested against the level they set.
        for (name, value) in INITIAL_INI.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            if name == "error_reporting" {
                // The ini path — unlike `ini_set` — runs the constant-expression
                // scanner, so `-d 'error_reporting=E_ALL & ~E_NOTICE'` works. The
                // RESOLVED number is what `ini_get` reports back afterwards.
                h.set_error_reporting(crate::errlevel::parse_ini_level(value).unwrap_or(0));
            } else {
                // Unlike `ini_set`, `-d` may introduce a name the engine has no
                // default for — the reference CLI registers it either way.
                h.ini.insert(name.clone(), value.clone());
            }
        }
        h
    }

    /// The last call's by-reference parameter value at `pos` (for the caller's
    /// write-back); `Undef` if out of range.
    pub fn byref_out_get(&self, pos: usize) -> Value {
        self.byref_out.get(pos).cloned().unwrap_or(Value::Undef)
    }

    /// Publish one by-reference OUT value from a *builtin* — `preg_match`'s
    /// `$matches`, `parse_str`'s result array. A user function fills the whole
    /// vector from its frame when it returns; a builtin has no frame, so it
    /// names the position it is writing.
    pub fn byref_out_put(&mut self, pos: usize, v: Value) {
        if self.byref_out.len() <= pos {
            self.byref_out.resize(pos + 1, Value::Undef);
        }
        self.byref_out[pos] = v;
    }

    /// Drop the previous call's OUT values, so a call that writes none cannot
    /// be read as having written the one before it.
    pub fn byref_out_clear(&mut self) {
        self.byref_out.clear();
    }

    /// Seed the superglobal arrays in the global scope: `$_ENV`/`$_SERVER` from
    /// the real process environment, empty `$_GET`/`$_POST`/… request arrays, and
    /// `$argv`/`$argc`. Visible from every scope via `is_superglobal` resolution.
    fn init_superglobals(&mut self) {
        let env: Vec<(String, String)> = std::env::vars().collect();
        let mkenv = |h: &mut PhpHost| -> Value {
            let a = h.new_array();
            for (k, v) in &env {
                h.arr_set_key(&a, &Value::str(k.clone()), Value::str(v.clone()));
            }
            a
        };
        let e = mkenv(self);
        self.set_var("_ENV", e);
        let s = mkenv(self);
        // A few conventional `$_SERVER` entries on top of the environment.
        self.arr_set_key(&s, &Value::str("PHP_SELF"), Value::str(String::new()));
        self.arr_set_key(&s, &Value::str("SCRIPT_NAME"), Value::str(String::new()));
        self.arr_set_key(&s, &Value::str("REQUEST_TIME"), Value::int(0));
        self.set_var("_SERVER", s);
        for name in [
            "_GET", "_POST", "_REQUEST", "_COOKIE", "_FILES", "_SESSION", "GLOBALS",
        ] {
            let empty = self.new_array();
            self.set_var(name, empty);
        }
        let argv = self.new_array();
        self.arr_push_auto(&argv, Value::str(String::new()));
        self.set_var("argv", argv);
        self.set_var("argc", Value::int(1));
    }

    /// A snapshot of all defined constants as `(name, value)` pairs — for
    /// `get_defined_constants`.
    pub fn all_constants(&self) -> Vec<(String, Value)> {
        self.constants
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// The names of all declared classes (lowercased, as stored) — for
    /// `get_declared_classes`.
    pub fn all_class_names(&self) -> Vec<String> {
        self.classes.keys().cloned().collect()
    }

    // ── constants ───────────────────────────────────────────────────────────

    /// `constant(name)` / a bare constant reference: the defined value, or the
    /// bare name as a string when undefined (PHP 7 leniency, minus the notice).
    pub fn const_fetch(&self, name: &str) -> Value {
        self.constants
            .get(name)
            .cloned()
            .unwrap_or_else(|| Value::str(name.to_string()))
    }

    /// Whether a constant of this name is defined.
    pub fn const_defined(&self, name: &str) -> bool {
        self.constants.contains_key(name)
    }

    /// `define(name, value)` — defines a constant, returning `true` unless it was
    /// already defined (PHP does not redefine and returns `false`).
    pub fn const_define(&mut self, name: &str, value: Value) -> bool {
        if self.constants.contains_key(name) {
            return false;
        }
        self.constants.insert(name.to_string(), value);
        true
    }

    // ── program loading ────────────────────────────────────────────────────

    /// Install a compiled program's user functions onto the host.
    pub fn load_program(&mut self, functions: Vec<(String, FuncDef)>) {
        for (name, def) in functions {
            self.functions.insert(name, def);
        }
    }

    /// Install a compiled program's classes onto the host.
    pub fn load_classes(&mut self, classes: Vec<(String, ClassDef)>) {
        for (name, def) in classes {
            self.classes.insert(name, def);
        }
    }

    /// Install a program's compiled `try`/`catch`/`finally` table. The `RUN_TRY`
    /// ids the main chunk carries are offsets into this vector.
    pub fn load_try_defs(&mut self, defs: Vec<TryDef>) {
        self.try_defs = defs;
    }

    /// Whether the object class `class` is caught by `catch (target)`: the same
    /// class, a subclass (walking the parent chain), or `target == Throwable`
    /// (the interface both exception roots implement). Names are compared
    /// case-insensitively, matching PHP and the lowercased class table.
    pub fn catch_matches(&self, class: &str, target: &str) -> bool {
        let target = target.to_ascii_lowercase();
        if target == "throwable" {
            return self.class_is_a(class, "exception") || self.class_is_a(class, "error");
        }
        self.class_is_a(class, &target)
    }

    /// Whether `class` is `ancestor` or descends from it, walking the compiled
    /// class table's parent chain. `ancestor` must already be lowercased.
    fn class_is_a(&self, class: &str, ancestor: &str) -> bool {
        // Traverse the parent chain AND implemented/extended interfaces
        // (transitively). A visited set + bound guards against malformed cycles.
        let mut stack = vec![class.to_ascii_lowercase()];
        let mut seen: Vec<String> = Vec::new();
        while let Some(c) = stack.pop() {
            if c == ancestor {
                return true;
            }
            if seen.contains(&c) || seen.len() > 1000 {
                continue;
            }
            seen.push(c.clone());
            if let Some(d) = self.classes.get(&c) {
                if let Some(p) = &d.parent {
                    stack.push(p.to_ascii_lowercase());
                }
                for i in &d.interfaces {
                    stack.push(i.to_ascii_lowercase());
                }
            }
        }
        false
    }

    // ── output capture ─────────────────────────────────────────────────────

    pub fn begin_capture(&mut self) {
        self.capture = Some(String::new());
    }

    pub fn end_capture(&mut self) -> String {
        self.capture.take().unwrap_or_default()
    }

    /// Emit a rendered string via `echo`: to the top output-buffering level if
    /// any, else the capture buffer if active, else stdout (no trailing newline —
    /// PHP `echo` writes exactly its argument).
    pub fn write_out(&mut self, s: &str) {
        if let Some(top) = self.ob_stack.last_mut() {
            top.push_str(s);
        } else if let Some(buf) = &mut self.capture {
            buf.push_str(s);
        } else {
            use std::io::Write;
            let mut out = std::io::stdout();
            let _ = out.write_all(s.as_bytes());
        }
    }

    // ── output buffering (ob_*) ──────────────────────────────────────────────

    /// `ob_start()` — push a new output-buffering level.
    pub fn ob_start(&mut self) {
        self.ob_stack.push(String::new());
    }

    /// The current buffering nesting level (`ob_get_level`).
    pub fn ob_level(&self) -> i64 {
        self.ob_stack.len() as i64
    }

    /// `ob_get_contents()` — the top buffer's contents, or `None` if inactive.
    pub fn ob_contents(&self) -> Option<String> {
        self.ob_stack.last().cloned()
    }

    /// `ob_get_clean()` — pop the top buffer and return its contents (`None` if
    /// none active).
    pub fn ob_get_clean(&mut self) -> Option<String> {
        self.ob_stack.pop()
    }

    /// `ob_end_clean()` — discard and pop the top buffer; `false` if none active.
    pub fn ob_end_clean(&mut self) -> bool {
        self.ob_stack.pop().is_some()
    }

    /// `ob_end_flush()` — pop the top buffer and write its contents down one level
    /// (the next buffer, the capture buffer, or stdout); `false` if none active.
    pub fn ob_end_flush(&mut self) -> bool {
        match self.ob_stack.pop() {
            Some(s) => {
                self.write_out(&s);
                true
            }
            None => false,
        }
    }

    /// `ob_flush()` — send the top buffer's contents down one level but keep the
    /// buffer active (cleared); `false` if none active.
    pub fn ob_flush(&mut self) -> bool {
        if self.ob_stack.is_empty() {
            return false;
        }
        let s = std::mem::take(self.ob_stack.last_mut().unwrap());
        // Write down a level: temporarily drop this (now empty) buffer so the
        // content lands one level below, then restore the empty buffer on top.
        let empty = self.ob_stack.pop().unwrap();
        self.write_out(&s);
        self.ob_stack.push(empty);
        true
    }

    // ── errors ─────────────────────────────────────────────────────────────

    // ── diagnostics ────────────────────────────────────────────────────────

    /// Name this run's source for diagnostics: the script path, or
    /// `"Command line code"` for `php -r` (which is the default).
    pub fn set_script_name(&mut self, name: impl Into<String>) {
        self.script_name = name.into();
    }

    /// What a diagnostic names as the source — the script path, or
    /// `"Command line code"` for `php -r`.
    pub fn script_name(&self) -> &str {
        &self.script_name
    }

    /// Record the line a diagnostic raised next belongs to. Called by the builtin
    /// that is about to warn, from the line table of the op it is executing.
    pub fn set_warn_line(&mut self, line: u32) {
        self.warn_line = line;
    }

    /// The line the innermost frame is executing — where an exception created
    /// right now records itself as having been raised, and the call site the
    /// frame below it reports in a backtrace.
    pub fn cur_frame_line(&self) -> u32 {
        self.scopes.last().map(|s| s.line).unwrap_or(0)
    }

    /// Enter/leave an `@expr` suppression region.
    pub fn suppress_push(&mut self) {
        self.suppress += 1;
    }

    pub fn suppress_pop(&mut self) {
        self.suppress = self.suppress.saturating_sub(1);
    }

    /// The open `@expr` region count, for a caller that has to put it back.
    pub fn suppress_depth(&self) -> usize {
        self.suppress
    }

    /// Force the region count back to `depth` — see the call in
    /// [`run_try_orchestrator`], the one place an `@expr` can be abandoned
    /// part-way through.
    pub fn suppress_restore(&mut self, depth: usize) {
        self.suppress = depth;
    }

    /// Emit a PHP `Warning` on the output stream.
    ///
    /// With the CLI defaults (`display_errors=STDOUT`, `html_errors=Off`) the
    /// reference interpreter writes exactly
    /// `"\nWarning: {msg} in {file} on line {n}\n"` to stdout, interleaved with
    /// the script's own output — so it goes through [`write_out`] and lands in an
    /// output buffer or a capture just as `echo` does. The copy `log_errors`
    /// sends to stderr is not reproduced: nothing observes it.
    pub fn warn(&mut self, msg: impl std::fmt::Display) {
        self.diagnose("Warning", errlevel::E_WARNING, self.warn_line, msg);
    }

    /// Emit a PHP `Deprecated` diagnostic. Same stream and shape as [`warn`],
    /// which is the only thing that distinguishes the severities on output.
    pub fn deprecated(&mut self, msg: impl std::fmt::Display) {
        self.diagnose("Deprecated", errlevel::E_DEPRECATED, self.warn_line, msg);
    }

    /// Emit a PHP `Notice`.
    pub fn notice(&mut self, msg: impl std::fmt::Display) {
        self.diagnose("Notice", errlevel::E_NOTICE, self.warn_line, msg);
    }

    /// The one place a non-fatal diagnostic is rendered, for every severity.
    ///
    /// Two independent gates silence it: `@expr` suppression (lexically scoped,
    /// leaves the level alone) and the `error_reporting` mask (a global the script
    /// or `-d` sets). A diagnostic whose `E_*` bit is clear is not merely hidden —
    /// it is never written, so it cannot land in an output buffer either.
    pub fn diagnose(&mut self, severity: &str, level: i64, line: u32, msg: impl std::fmt::Display) {
        if self.suppress > 0 || self.error_reporting & level == 0 {
            return;
        }
        let text = format!(
            "\n{severity}: {msg} in {} on line {line}\n",
            self.script_name
        );
        self.write_out(&text);
    }

    /// The current `error_reporting` mask.
    pub fn error_reporting(&self) -> i64 {
        self.error_reporting
    }

    /// What `preg_last_error()` reports.
    pub fn preg_error(&self) -> i64 {
        self.preg_error
    }

    /// Record the outcome of a `preg_*` call that reached the regex compiler.
    /// Called on SUCCESS too, which is what clears a previous failure: in the
    /// reference a pattern that compiles resets the state even when the match
    /// then finds nothing.
    pub fn set_preg_error(&mut self, code: i64) {
        self.preg_error = code;
    }

    /// Set the `error_reporting` mask, returning the previous one — what
    /// `error_reporting($level)` reports back. The ini store is updated with the
    /// decimal spelling, so `ini_get('error_reporting')` follows the function.
    pub fn set_error_reporting(&mut self, level: i64) -> i64 {
        self.ini
            .insert("error_reporting".to_string(), level.to_string());
        std::mem::replace(&mut self.error_reporting, level)
    }

    /// Read an ini setting as `ini_get` returns it — always a STRING, even for a
    /// numeric setting. `None` means the name is not a setting this engine knows,
    /// which `ini_get` reports as `false`.
    ///
    /// `error_reporting` is stored here VERBATIM as well as parsed into the mask,
    /// because the two can disagree: `ini_set('error_reporting', '12abc')` leaves
    /// `ini_get` reporting `"12abc"` while the mask becomes 12.
    ///
    /// DIVERGENCE: only the settings [`default_ini`] lists are known — PHP core
    /// plus `date` and `pcre`. A name belonging to an optional extension the
    /// reference happened to be built with (`mysqli.default_host`), or one whose
    /// default is that build's install prefix (`extension_dir`), reports `false`
    /// rather than a value that would be wrong on another machine.
    pub fn ini_get(&self, name: &str) -> Option<String> {
        self.ini.get(name).cloned()
    }

    /// Write an ini setting, returning the previous value — or `None` for a name
    /// the engine does not know, which `ini_set` reports as `false` without
    /// storing anything (PHP does not let `ini_set` invent settings).
    ///
    /// `error_reporting` also updates the mask, by PHP's ordinary string-to-int
    /// coercion — NOT the constant-expression scanner. `E_ALL & ~E_NOTICE` is a
    /// php.ini spelling, understood on the `-d`/ini path only; handed to
    /// `ini_set` at run time it reads as the integer 0 and mutes everything,
    /// which is exactly what the reference does with it.
    ///
    /// Two more ways a write is refused, both of which report `false` and change
    /// nothing: a setting that is not runtime-changeable (see [`INI_FIXED`]), and
    /// a value the setting rejects (see [`ini_value_rejected`]).
    pub fn ini_set(&mut self, name: &str, value: &str) -> Option<String> {
        if INI_FIXED.contains(&name) {
            return None;
        }
        let level = (name == "error_reporting")
            .then(|| self.to_number(&Value::str(value.to_string())).to_int());
        let slot = self.ini.get_mut(name)?;
        let old = std::mem::replace(slot, value.to_string());
        if let Some(level) = level {
            self.error_reporting = level;
        }
        Some(old)
    }

    /// One argument as a stack trace renders it. PHP does not print values in
    /// full here: a string is single-quoted and cut to 15 characters with a
    /// literal `...` inside the quotes, and an array or object collapses to its
    /// kind, so a trace can never be arbitrarily long.
    fn trace_arg(&self, v: &Value) -> String {
        match v {
            Value::Undef => "NULL".to_string(),
            Value::Bool(true) => "true".to_string(),
            Value::Bool(false) => "false".to_string(),
            Value::Str(s) => {
                if s.chars().count() > 15 {
                    let cut: String = s.chars().take(15).collect();
                    format!("'{cut}...'")
                } else {
                    format!("'{s}'")
                }
            }
            Value::Obj(_) if self.is_array(v) => "Array".to_string(),
            Value::Obj(_) if self.is_closure(v) => "Object(Closure)".to_string(),
            Value::Obj(_) => match self.object_class(v) {
                Some(c) => format!("Object({c})"),
                None => "Object(stdClass)".to_string(),
            },
            other => self.to_str(other),
        }
    }

    /// A frame's name as a trace prints it: `f`, `A->m` for an instance method,
    /// `A::m` for a static one. Frames are recorded as `Class::method` with the
    /// class lowercased (that is the lookup key); the declared spelling comes back
    /// off the `ClassDef`, and the arrow form from whether the frame bound `$this`.
    fn trace_frame_name(&self, scope: &Scope) -> String {
        let Some(name) = &scope.name else {
            return String::new();
        };
        let Some((class, method)) = name.split_once("::") else {
            return name.clone();
        };
        let class = self
            .classes
            .get(class)
            .map(|d| d.name.as_str())
            .unwrap_or(class);
        let sep = if scope.vars.contains_key("this") {
            "->"
        } else {
            "::"
        };
        format!("{class}{sep}{method}")
    }

    /// PHP's `Stack trace:` body for the call stack as it stands right now —
    /// captured when an exception object is *created*, which is when PHP snapshots
    /// it, because by the time the throw reaches the top the frames are gone.
    ///
    /// Frame `#i` names the function that was entered and the file/line of the
    /// call that entered it, so its call site is the *caller's* current line. The
    /// global scope is not a frame: it closes the list as `#N {main}`.
    ///
    /// DIVERGENCE: a frame entered from inside a library function (an `array_map`
    /// callback, say) prints that call site rather than PHP's `[internal
    /// function]`, and a closure frame prints `{closure}` rather than PHP 8.4's
    /// `{closure:file:line}`.
    pub fn backtrace(&self) -> String {
        let mut out = String::new();
        let mut n = 0;
        for i in (1..self.scopes.len()).rev() {
            let scope = &self.scopes[i];
            let args = self
                .array_pairs(&self.get_var_in(i, "@args"))
                .unwrap_or_default()
                .iter()
                .map(|(_, v)| self.trace_arg(v))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!(
                "#{n} {}({}): {}({args})\n",
                self.script_name,
                self.scopes[i - 1].line,
                self.trace_frame_name(scope)
            ));
            n += 1;
        }
        out.push_str(&format!("#{n} {{main}}"));
        out
    }

    /// Read a variable out of a specific frame — the backtrace needs each frame's
    /// own hidden `@args`, not the innermost one's.
    fn get_var_in(&self, idx: usize, name: &str) -> Value {
        self.scopes
            .get(idx)
            .and_then(|s| s.vars.get(name))
            .cloned()
            .unwrap_or(Value::Undef)
    }

    /// Report a fatal error the way the PHP CLI does: the *display* copy goes on
    /// the output stream (so it lands inside an open `ob_start` buffer and
    /// interleaves with the program's own output exactly as the reference does),
    /// and `log_errors` puts a `PHP `-prefixed copy on stderr.
    pub fn fatal(&mut self, severity: &str, body: &str) {
        self.write_out(&format!("\n{severity}: {body}\n"));
        eprintln!("PHP {severity}:  {body}");
        self.fatal_reported = true;
    }

    /// Whether a fatal has already been displayed in PHP's shape, so the CLI
    /// wrapper must not print it again.
    pub fn fatal_reported(&self) -> bool {
        self.fatal_reported
    }

    /// Flush every still-open output buffer, innermost first — what PHP's
    /// shutdown does, so `ob_start()` without a matching end still prints.
    pub fn ob_flush_all(&mut self) {
        while !self.ob_stack.is_empty() {
            self.ob_end_flush();
        }
    }

    /// The type name PHP uses inside a diagnostic — the short spelling (`int`,
    /// `bool`), not `gettype`'s (`integer`, `boolean`).
    pub fn diag_type(&self, v: &Value) -> &'static str {
        match v {
            Value::Undef => "null",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Str(_) => "string",
            Value::Obj(_) => {
                if self.is_array(v) {
                    "array"
                } else {
                    "object"
                }
            }
            _ => "null",
        }
    }

    /// The type name `Trying to access array offset on …` uses. It differs from
    /// [`diag_type`] for booleans, which it spells as the literal `true`/`false`.
    fn diag_offset_type(&self, v: &Value) -> &'static str {
        match v {
            Value::Bool(true) => "true",
            Value::Bool(false) => "false",
            other => self.diag_type(other),
        }
    }

    /// An array key as a diagnostic renders it: a string key is quoted, an
    /// integer key is bare (`Undefined array key "k"` vs `Undefined array key 7`).
    pub fn diag_key(&self, key: &Value) -> String {
        match self.norm_key(key) {
            ArrayKey::Int(n) => n.to_string(),
            ArrayKey::Str(s) => format!("\"{s}\""),
        }
    }

    pub fn set_error(&mut self, msg: impl Into<String>) {
        if self.error.is_none() {
            self.error = Some(msg.into());
        }
    }

    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    // ── variables ──────────────────────────────────────────────────────────

    /// The index of the scope a variable name resolves in: the global frame for a
    /// superglobal, else the innermost frame.
    fn scope_idx(&self, name: &str) -> usize {
        if is_superglobal(name) {
            0
        } else {
            self.scopes.len().saturating_sub(1)
        }
    }

    pub fn get_var(&self, name: &str) -> Value {
        let idx = self.scope_idx(name);
        let Some(scope) = self.scopes.get(idx) else {
            return Value::Undef;
        };
        // A name bound as a reference reads through its shared cell.
        if let Some(&slot) = scope.refs.get(name) {
            return self.ref_cells.get(slot).cloned().unwrap_or(Value::Undef);
        }
        scope.vars.get(name).cloned().unwrap_or(Value::Undef)
    }

    pub fn set_var(&mut self, name: &str, val: Value) {
        let idx = self.scope_idx(name);
        let slot = self.scopes.get(idx).and_then(|s| s.refs.get(name).copied());
        if let Some(slot) = slot {
            if let Some(cell) = self.ref_cells.get_mut(slot) {
                *cell = val;
            }
        } else if let Some(scope) = self.scopes.get_mut(idx) {
            scope.vars.insert(name.to_string(), val);
        }
    }

    /// `$target = &$source` — bind `target` as a reference to `source`, so both
    /// names share one storage cell (either's mutation is visible to the other).
    /// The reference cell `name` resolves to, promoting a plain variable into
    /// one first, wrapped in a [`PhpObj::Ref`] handle. What `use (&$v)` captures
    /// — see [`PhpHost::bind_ref_slot`] for the other half.
    pub fn ref_cell_of(&mut self, name: &str) -> Value {
        let slot = self.ref_slot_of(name);
        self.objs.push(PhpObj::Ref { slot });
        Value::Obj((self.objs.len() - 1) as u32)
    }

    /// Point `name` in the current scope at an existing reference cell, so it
    /// and whatever else shares the cell are one variable.
    pub fn bind_ref_slot(&mut self, name: &str, slot: usize) {
        let idx = self.scope_idx(name);
        if let Some(scope) = self.scopes.get_mut(idx) {
            scope.vars.remove(name);
            scope.refs.insert(name.to_string(), slot);
        }
    }

    /// Allocate a reference cell that no variable owns yet, holding `val`, and
    /// return a [`PhpObj::Ref`] handle to it together with its slot.
    ///
    /// Builtins that hand an element to a user callback by reference
    /// (`array_walk`) use this: pass the handle as the argument, let the callee's
    /// by-ref parameter bind the slot, then read the slot back with
    /// [`PhpHost::ref_cell_value`] to see what the callback wrote.
    pub fn new_ref_cell(&mut self, val: Value) -> (Value, usize) {
        self.ref_cells.push(val);
        let slot = self.ref_cells.len() - 1;
        self.objs.push(PhpObj::Ref { slot });
        (Value::Obj((self.objs.len() - 1) as u32), slot)
    }

    /// Read the current contents of the reference cell at `slot`.
    pub fn ref_cell_value(&self, slot: usize) -> Value {
        self.ref_cells.get(slot).cloned().unwrap_or(Value::Undef)
    }

    /// The reference slot a value denotes, if it is a [`PhpObj::Ref`] handle.
    ///
    /// A stored `Ref` handle is how an array element or an object property that a
    /// `&` binding has made into a reference is represented: the container keeps
    /// the handle, every read derefs it, and every write goes through the cell, so
    /// the element and the alias are one storage location — PHP's `IS_REFERENCE`
    /// zval in the slot.
    pub fn ref_slot_of_value(&self, v: &Value) -> Option<usize> {
        match v {
            Value::Obj(h) => match self.objs.get(*h as usize) {
                Some(PhpObj::Ref { slot }) => Some(*slot),
                _ => None,
            },
            _ => None,
        }
    }

    /// Resolve a stored value: a `Ref` handle reads as its cell's contents,
    /// everything else as itself. Applied on the way out of every container read
    /// so a reference in a slot is invisible to code that only reads the value.
    fn deref(&self, v: Value) -> Value {
        match self.ref_slot_of_value(&v) {
            Some(slot) => self.ref_cells.get(slot).cloned().unwrap_or(Value::Undef),
            None => v,
        }
    }

    /// Write `val` into the reference cell at `slot`.
    fn ref_cell_set(&mut self, slot: usize, val: Value) {
        if let Some(cell) = self.ref_cells.get_mut(slot) {
            *cell = val;
        }
    }

    /// The reference cell `name` resolves to, creating one — and moving the
    /// variable's current value into it — if the variable is still a plain one.
    fn ref_slot_of(&mut self, name: &str) -> usize {
        let idx = self.scope_idx(name);
        if let Some(slot) = self.scopes.get(idx).and_then(|s| s.refs.get(name).copied()) {
            return slot;
        }
        let cur = self
            .scopes
            .get(idx)
            .and_then(|s| s.vars.get(name).cloned())
            .unwrap_or(Value::Undef);
        self.ref_cells.push(cur);
        let slot = self.ref_cells.len() - 1;
        if let Some(scope) = self.scopes.get_mut(idx) {
            scope.vars.remove(name);
            scope.refs.insert(name.to_string(), slot);
        }
        slot
    }

    /// The reference cell for the array element `$name[k1]..[kN]`, promoting the
    /// element into a reference (and auto-vivifying the path, as PHP does for a
    /// `&` lvalue) if it is not one already. `keys` must be non-empty.
    ///
    /// This is the storage half of `$r = &$a[k]`: the element slot keeps a
    /// [`PhpObj::Ref`] handle to the returned cell, so a later write through
    /// either name lands in the one cell.
    pub fn elem_ref_slot(&mut self, name: &str, keys: &[Value]) -> usize {
        let Some((last, inter)) = keys.split_last() else {
            return self.ref_slot_of(name);
        };
        let arr = self.ensure_path_array(name, inter);
        self.arr_elem_ref_slot(&arr, last)
    }

    /// The reference cell for `arr[key]` (a handle), promoting the element into a
    /// reference if it is not one already.
    pub fn arr_elem_ref_slot(&mut self, arr: &Value, key: &Value) -> usize {
        let k = self.norm_key(key);
        if let Some(slot) = self.entry_ref_slot(arr, &k) {
            return slot;
        }
        // PHP vivifies a `&`-taken element to null when it does not exist yet.
        let cur = match self.as_array(arr) {
            Some(PhpObj::Array { entries, .. }) => entries.get(&k).cloned().unwrap_or(Value::Undef),
            _ => Value::Undef,
        };
        self.ref_cells.push(cur);
        let slot = self.ref_cells.len() - 1;
        self.objs.push(PhpObj::Ref { slot });
        let handle = Value::Obj((self.objs.len() - 1) as u32);
        if let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.as_array_mut(arr)
        {
            if let ArrayKey::Int(n) = k {
                if n >= *next_index {
                    *next_index = n + 1;
                }
            }
            entries.insert(k, handle);
        }
        slot
    }

    /// The reference cell for `$obj->name`, promoting the property into a
    /// reference if it is not one already.
    pub fn prop_ref_slot_ensure(&mut self, recv: &Value, name: &str) -> usize {
        if let Some(slot) = self.prop_ref_slot(recv, name) {
            return slot;
        }
        let cur = match self.as_array(recv) {
            Some(PhpObj::Object { props, .. }) => props.get(name).cloned().unwrap_or(Value::Undef),
            _ => Value::Undef,
        };
        self.ref_cells.push(cur);
        let slot = self.ref_cells.len() - 1;
        self.objs.push(PhpObj::Ref { slot });
        let handle = Value::Obj((self.objs.len() - 1) as u32);
        if let Some(PhpObj::Object { props, .. }) = self.as_array_mut(recv) {
            props.insert(name.to_string(), handle);
        }
        slot
    }

    /// Store a [`PhpObj::Ref`] handle for `slot` into `$name[k1]..[kN]` — the
    /// `$a[k] = &$x` direction, where the *container slot* becomes the alias.
    /// An empty `keys` binds the plain variable instead.
    pub fn bind_elem_to_slot(&mut self, name: &str, keys: &[Value], slot: usize) {
        let Some((last, inter)) = keys.split_last() else {
            self.bind_ref_slot(name, slot);
            return;
        };
        let arr = self.ensure_path_array(name, inter);
        self.objs.push(PhpObj::Ref { slot });
        let handle = Value::Obj((self.objs.len() - 1) as u32);
        let k = self.norm_key(last);
        if let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.as_array_mut(&arr)
        {
            if let ArrayKey::Int(n) = k {
                if n >= *next_index {
                    *next_index = n + 1;
                }
            }
            entries.insert(k, handle);
        }
    }

    /// `$name[k1]..[kM][] = &$x` — append a [`PhpObj::Ref`] handle for `slot`.
    pub fn append_elem_to_slot(&mut self, name: &str, keys: &[Value], slot: usize) {
        let arr = self.ensure_path_array(name, keys);
        self.objs.push(PhpObj::Ref { slot });
        let handle = Value::Obj((self.objs.len() - 1) as u32);
        if let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.as_array_mut(&arr)
        {
            let k = ArrayKey::Int(*next_index);
            *next_index += 1;
            entries.insert(k, handle);
        }
    }

    /// `$obj->p = &$x` — store a [`PhpObj::Ref`] handle for `slot` in a property.
    pub fn bind_prop_to_slot(&mut self, recv: &Value, name: &str, slot: usize) {
        self.objs.push(PhpObj::Ref { slot });
        let handle = Value::Obj((self.objs.len() - 1) as u32);
        if let Some(PhpObj::Object { props, .. }) = self.as_array_mut(recv) {
            props.insert(name.to_string(), handle);
        }
    }

    /// The running frame's late-static-binding class, or `fallback` (the
    /// enclosing class the compiler baked in) outside any method call.
    pub fn lsb_class(&self, fallback: &str) -> String {
        self.scopes
            .last()
            .and_then(|s| s.static_class.clone())
            .unwrap_or_else(|| fallback.to_string())
    }

    /// Mark the next call as forwarding: it inherits this frame's
    /// late-static-binding class instead of taking the class it names.
    pub fn lsb_forward(&mut self) {
        self.pending_lsb = self.scopes.last().and_then(|s| s.static_class.clone());
    }

    /// Set the late-static-binding class the next call's frame takes, unless a
    /// forwarding call has already claimed it.
    pub fn lsb_set_for_next_call(&mut self, class: &str) {
        if self.pending_lsb.is_none() {
            self.pending_lsb = Some(class.to_string());
        }
    }

    /// Take the late-static-binding class a pushed frame should carry.
    fn lsb_take(&mut self) -> Option<String> {
        self.pending_lsb.take()
    }

    /// Publish `slot` as the reference the running `function &f()` returns.
    pub fn set_ret_ref_slot(&mut self, slot: usize) {
        self.ret_ref_slot = Some(slot);
    }

    /// Take the reference cell the last call returned. A call that did not return
    /// by reference has none, so `fallback` is parked in a fresh detached cell —
    /// `$r = &f()` on a by-value function then aliases only its own copy.
    pub fn take_ret_ref_slot(&mut self, fallback: Value) -> usize {
        match self.ret_ref_slot.take() {
            Some(slot) => slot,
            None => {
                self.ref_cells.push(fallback);
                self.ref_cells.len() - 1
            }
        }
    }

    pub fn ref_bind(&mut self, target: &str, source: &str) {
        let idx = self.scope_idx(source);
        // Resolve the source's cell, promoting a plain variable into one.
        let slot = match self
            .scopes
            .get(idx)
            .and_then(|s| s.refs.get(source).copied())
        {
            Some(s) => s,
            None => {
                let cur = self
                    .scopes
                    .get(idx)
                    .and_then(|s| s.vars.get(source).cloned())
                    .unwrap_or(Value::Undef);
                self.ref_cells.push(cur);
                let s = self.ref_cells.len() - 1;
                if let Some(scope) = self.scopes.get_mut(idx) {
                    scope.vars.remove(source);
                    scope.refs.insert(source.to_string(), s);
                }
                s
            }
        };
        let tidx = self.scope_idx(target);
        if let Some(scope) = self.scopes.get_mut(tidx) {
            scope.vars.remove(target);
            scope.refs.insert(target.to_string(), slot);
        }
    }

    /// `unset($name)` — remove the scope variable (and break any reference
    /// binding for that name, leaving the shared cell intact for other aliases).
    pub fn unset_var(&mut self, name: &str) {
        let idx = self.scope_idx(name);
        if let Some(scope) = self.scopes.get_mut(idx) {
            scope.vars.remove(name);
            scope.refs.remove(name);
        }
    }

    /// `unset($name[k1]..[kN])` — remove the deepest array element along the key
    /// path. The key is removed without renumbering the remaining keys (PHP
    /// leaves a hole), matching `unset` on an array element.
    pub fn unset_path(&mut self, name: &str, keys: &[Value]) {
        let Some((last, inter)) = keys.split_last() else {
            return;
        };
        // Navigate (read-only, no vivification) to the array holding `last`.
        let mut arr = self.get_var(name);
        for k in inter {
            arr = self.index_get(&arr, k);
        }
        let nk = self.norm_key(last);
        if let Some(PhpObj::Array { entries, .. }) = self.as_array_mut(&arr) {
            entries.shift_remove(&nk);
        }
    }

    // ── DAP debug introspection (used only under `--dap`) ────────────────────

    /// Number of active scopes (the debugger's step-depth reference).
    pub fn frame_depth(&self) -> usize {
        self.scopes.len()
    }

    /// Record the source line the innermost frame is executing.
    ///
    /// Two callers, one meaning. Under `--dap` the per-statement line hook drives
    /// it; on every path the ops that can enter a frame or raise a throw record
    /// the op's own line, which is what a backtrace reads back as the call site.
    /// Distinct from [`set_warn_line`](Self::set_warn_line), which is the line of
    /// the op that is warning *now* and need not survive a nested call returning.
    pub fn set_cur_line(&mut self, line: u32) {
        if let Some(s) = self.scopes.last_mut() {
            s.line = line;
        }
    }

    /// The call stack as `(frame name, line)` pairs, innermost first — for the
    /// DAP `stackTrace`. The global scope is reported as `{main}`.
    pub fn dbg_stack(&self) -> Vec<(String, u32)> {
        self.scopes
            .iter()
            .rev()
            .map(|s| {
                let name = s.name.clone().unwrap_or_else(|| "{main}".to_string());
                (name, s.line)
            })
            .collect()
    }

    /// The innermost frame's locals as `(name, string-cast)` pairs — for DAP
    /// `variables`. Compiler temporaries (`@`-prefixed) are hidden, matching a
    /// debugger's default locals view.
    pub fn dbg_locals(&self) -> Vec<(String, String)> {
        let mut vars: Vec<(String, Value)> = self
            .scopes
            .last()
            .map(|s| {
                s.vars
                    .iter()
                    .filter(|(k, _)| !k.starts_with('@'))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default();
        // Reference-bound names read through their shared cell.
        if let Some(s) = self.scopes.last() {
            for (k, &slot) in &s.refs {
                if !k.starts_with('@') {
                    vars.push((
                        k.clone(),
                        self.ref_cells.get(slot).cloned().unwrap_or(Value::Undef),
                    ));
                }
            }
        }
        vars.into_iter()
            .map(|(n, v)| (format!("${n}"), self.to_str(&v)))
            .collect()
    }

    // ── arrays ─────────────────────────────────────────────────────────────

    pub fn new_array(&mut self) -> Value {
        self.objs.push(PhpObj::Array {
            entries: IndexMap::new(),
            next_index: 0,
        });
        Value::Obj((self.objs.len() - 1) as u32)
    }

    /// A PHP array is a **value**: assigning one, passing one to a function or
    /// returning one hands over a copy, so a write through the new name is
    /// invisible through the old one. Everything else — an object, a closure,
    /// a generator, a stream — is a handle and is passed through untouched,
    /// which is exactly PHP's own split.
    ///
    /// The copy is deep in the arrays and shallow everywhere else: an array
    /// nested in an array is itself a value and is copied with it, while an
    /// object stored in one is a handle and stays shared. PHP defers the same
    /// copy until a write (copy-on-write), which no program can observe — the
    /// difference is the cost of a copy that turns out to be unread, not the
    /// answer.
    pub fn copy_on_assign(&mut self, v: Value) -> Value {
        let Value::Obj(id) = v else { return v };
        let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.objs.get(id as usize)
        else {
            return v;
        };
        let next_index = *next_index;
        let pairs: Vec<(ArrayKey, Value)> = entries
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let copied: IndexMap<ArrayKey, Value> = pairs
            .into_iter()
            .map(|(k, v)| (k, self.copy_on_assign(v)))
            .collect();
        self.objs.push(PhpObj::Array {
            entries: copied,
            next_index,
        });
        Value::Obj((self.objs.len() - 1) as u32)
    }

    // ── closures ───────────────────────────────────────────────────────────

    /// Build a closure object from a compiler-registered function definition
    /// (`def_name`, a synthetic name in the function table) plus the values
    /// captured at creation time. Returns the new handle, or `Undef` if the
    /// definition is missing (should never happen for compiler-emitted names).
    pub fn make_closure(&mut self, def_name: &str, captured: Vec<(String, Value)>) -> Value {
        let Some(def) = self.functions.get(def_name).cloned() else {
            return Value::Undef;
        };
        // A closure defined inside a method auto-binds the current `$this` and its
        // class scope (PHP semantics), so `$this` and private member access work
        // without an explicit `bindTo`. `None`/`None` at the top level or in a free
        // function keeps the previous behavior exactly.
        let bound_this = match self.get_var("this") {
            t @ Value::Obj(_) => Some(t),
            _ => None,
        };
        let scope = self.current_class_ctx();
        self.objs.push(PhpObj::Closure {
            params: def.params,
            chunk: Box::new(def.chunk),
            captured,
            bound_this,
            scope,
            is_generator: def.is_generator,
        });
        Value::Obj((self.objs.len() - 1) as u32)
    }

    /// The closure held by `v` (a handle), cloned for a call: its parameters,
    /// body chunk, captured bindings, and any bound `$this`/scope. `None` if `v`
    /// is not a closure.
    fn closure_of(&self, v: &Value) -> Option<ClosureCall> {
        match self.as_array(v) {
            Some(PhpObj::Closure {
                params,
                chunk,
                captured,
                bound_this,
                scope,
                is_generator,
            }) => Some(ClosureCall {
                params: params.clone(),
                chunk: (**chunk).clone(),
                captured: captured.clone(),
                bound_this: bound_this.clone(),
                scope: scope.clone(),
                is_generator: *is_generator,
            }),
            _ => None,
        }
    }

    /// A new closure identical to `v` but with `$this` rebound to `this` and the
    /// private-access scope set to `scope` (both optional). Backs
    /// `Closure::bind`/`bindTo`/`call`.
    pub fn rebind_closure(
        &mut self,
        v: &Value,
        this: Option<Value>,
        scope: Option<String>,
    ) -> Option<Value> {
        let (params, chunk, captured, is_generator) = match self.as_array(v) {
            Some(PhpObj::Closure {
                params,
                chunk,
                captured,
                is_generator,
                ..
            }) => (
                params.clone(),
                chunk.clone(),
                captured.clone(),
                *is_generator,
            ),
            _ => return None,
        };
        self.objs.push(PhpObj::Closure {
            params,
            chunk,
            captured,
            bound_this: this,
            scope,
            is_generator,
        });
        Some(Value::Obj((self.objs.len() - 1) as u32))
    }

    /// Whether `v` is a closure handle.
    pub fn is_closure(&self, v: &Value) -> bool {
        matches!(self.as_array(v), Some(PhpObj::Closure { .. }))
    }

    /// The closure's current private-access scope class, if any (for `bindTo`'s
    /// default "keep the current scope" behavior).
    fn closure_scope(&self, v: &Value) -> Option<String> {
        match self.as_array(v) {
            Some(PhpObj::Closure { scope, .. }) => scope.clone(),
            _ => None,
        }
    }

    // ── objects / classes ──────────────────────────────────────────────────

    pub fn is_object(&self, v: &Value) -> bool {
        matches!(self.as_array(v), Some(PhpObj::Object { .. }))
    }

    /// The heap handle of any object/array/resource value — a stable per-instance
    /// id (`spl_object_id`). `None` for non-heap values.
    ///
    /// This is the same number `var_dump` prints as `#N`, as it is in PHP —
    /// both are the object's handle — so it is [`object_ordinal`], not the raw
    /// heap index, which also counts arrays, closures and resources.
    pub fn object_id(&self, v: &Value) -> Option<i64> {
        match v {
            Value::Obj(_) if self.is_object(v) => Some(self.object_ordinal(v) as i64),
            Value::Obj(h) => Some(*h as i64),
            _ => None,
        }
    }

    /// The 1-based creation-order number PHP shows as `#N` in `var_dump`.
    ///
    /// PHP numbers class instances only, so count the `Object` entries up to and
    /// including this handle rather than using the raw heap index (which also
    /// covers arrays, closures and resources).
    ///
    /// KNOWN DIVERGENCE: PHP frees an object's handle when its refcount drops to
    /// zero and hands the number to the next allocation, so a program that
    /// discards an object sees a *lower* number afterwards than phplang, which
    /// allocates from an append-only arena and never frees. Closing this needs
    /// refcounted handles and a free list ordered the way PHP's allocator orders
    /// its own — substrate that does not exist here — so the numbers agree only
    /// while no object has become unreachable.
    pub fn object_ordinal(&self, v: &Value) -> usize {
        let Value::Obj(h) = v else { return 0 };
        self.objs
            .iter()
            .take(*h as usize + 1)
            .filter(|o| matches!(o, PhpObj::Object { .. }))
            .count()
    }

    /// The class name of an object handle, or `None` if `v` is not an object.
    pub fn object_class(&self, v: &Value) -> Option<String> {
        match self.as_array(v) {
            Some(PhpObj::Object { class, .. }) => Some(class.clone()),
            _ => None,
        }
    }

    // ── reflection support (for the `reflection` stdlib module) ──────────────

    /// Whether a class of the given name is declared (case-insensitive).
    pub fn class_exists(&self, name: &str) -> bool {
        self.classes.contains_key(&name.to_ascii_lowercase())
    }

    /// The PHP fatal-error message if `class` cannot be instantiated with `new`
    /// (it is `abstract` or an `interface`), else `None`. The original casing is
    /// used in the message.
    fn class_instantiation_error(&self, class: &str) -> Option<String> {
        let def = self.classes.get(&class.to_ascii_lowercase())?;
        if def.is_interface {
            Some(format!("Cannot instantiate interface {class}"))
        } else if def.is_abstract {
            Some(format!("Cannot instantiate abstract class {class}"))
        } else {
            None
        }
    }

    /// Whether a user function of the given name is defined (case-insensitive).
    pub fn function_defined(&self, name: &str) -> bool {
        self.functions.contains_key(&name.to_ascii_lowercase())
    }

    /// The declared parent-class name of `name`, or `None` (no parent / unknown).
    pub fn class_parent(&self, name: &str) -> Option<String> {
        self.classes
            .get(&name.to_ascii_lowercase())
            .and_then(|d| d.parent.clone())
    }

    /// Whether `class` is `target` or descends from it (case-insensitive);
    /// `target == "Throwable"` matches either exception root. The public form of
    /// the catch-matching walk, for `is_a`/`is_subclass_of`/`instanceof`.
    pub fn is_a_class(&self, class: &str, target: &str) -> bool {
        self.catch_matches(class, target)
    }

    /// Method names visible on `class`, walking the parent chain (lowercased, as
    /// stored). For `get_class_methods`.
    pub fn class_method_names(&self, class: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = Some(class.to_ascii_lowercase());
        while let Some(c) = cur {
            let Some(def) = self.classes.get(&c) else {
                break;
            };
            for m in def.methods.keys() {
                if !out.contains(m) {
                    out.push(m.clone());
                }
            }
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
        }
        out
    }

    /// Whether `class` (or an ancestor) defines `method` (case-insensitive).
    pub fn class_has_method(&self, class: &str, method: &str) -> bool {
        self.resolve_method(class, &method.to_ascii_lowercase())
            .is_some()
    }

    /// Whether `class` (or an ancestor) declares the property `name`.
    pub fn class_has_prop(&self, class: &str, name: &str) -> bool {
        let mut cur = Some(class.to_ascii_lowercase());
        while let Some(c) = cur {
            let Some(def) = self.classes.get(&c) else {
                break;
            };
            if def.prop_defaults.iter().any(|(n, _)| n == name) {
                return true;
            }
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
        }
        false
    }

    /// An object's `(name, value)` properties in insertion order. For
    /// `get_object_vars`; empty if `v` is not an object.
    pub fn object_props(&self, v: &Value) -> Vec<(String, Value)> {
        match self.as_array(v) {
            Some(PhpObj::Object { props, .. }) => props
                .iter()
                .map(|(k, v)| (k.clone(), self.deref(v.clone())))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// An object's properties that are *visible from the current scope* — what
    /// `get_object_vars` returns. Called from outside the class, that is the
    /// public ones only; called from inside a method of the class, the private
    /// and protected ones as well.
    pub fn object_props_visible(&self, v: &Value) -> Vec<(String, Value)> {
        let Some(class) = self.object_class(v) else {
            return Vec::new();
        };
        self.object_props(v)
            .into_iter()
            .filter(|(name, _)| match self.resolve_prop_vis(&class, name) {
                Some((declaring, vis)) => self.visibility_allows(vis, &declaring),
                // A dynamic property was never declared, so it is public.
                None => true,
            })
            .collect()
    }

    /// An object's properties keyed the way `(array)` spells them: PHP mangles a
    /// non-public name so the cast cannot silently collide two properties of the
    /// same name from different visibilities. A private `$p` declared by `C`
    /// becomes `"\0C\0p"`, a protected one `"\0*\0p"`, and a public one stays as
    /// it is. The NUL bytes are why such a key cannot be reached with `$arr['p']`.
    pub fn object_props_mangled(&self, v: &Value) -> Vec<(String, Value)> {
        let class = self.object_class(v).unwrap_or_default();
        self.object_props(v)
            .into_iter()
            .map(|(name, val)| {
                let key = match self.declaring_class_of_prop(&class, &name) {
                    // The mangled name carries the declaring class *as written*.
                    Some((declaring, Visibility::Private)) => format!("\0{declaring}\0{name}"),
                    Some((_, Visibility::Protected)) => format!("\0*\0{name}"),
                    _ => name,
                };
                (key, val)
            })
            .collect()
    }

    /// [`resolve_prop_vis`] with the declaring class in its source spelling,
    /// which is the form `(array)`'s mangled key needs. Class definitions are
    /// keyed by lowercase name but the `parent` link keeps its original case, so
    /// the walk carries the declared name alongside the lookup key.
    fn declaring_class_of_prop(&self, class: &str, name: &str) -> Option<(String, Visibility)> {
        let mut cur = Some(class.to_string());
        while let Some(c) = cur {
            let def = self.classes.get(&c.to_ascii_lowercase())?;
            if let Some(v) = def.prop_vis.get(name) {
                return Some((c, *v));
            }
            cur = def.parent.clone();
        }
        None
    }

    /// The object's `(name, value, is_reference)` triples — the property form of
    /// [`array_pairs_marked`], for `var_dump`'s `&` marker.
    pub fn object_props_marked(&self, v: &Value) -> Vec<(String, Value, bool)> {
        match self.as_array(v) {
            Some(PhpObj::Object { props, .. }) => props
                .iter()
                .map(|(k, v)| {
                    let is_ref = self.ref_slot_of_value(v).is_some();
                    (k.clone(), self.deref(v.clone()), is_ref)
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    // ── file resources (fopen family) ────────────────────────────────────────

    /// Allocate a file-stream resource. `buf`/`pos` seed the in-memory content
    /// and cursor; `writable` marks write/append modes (flushed on close).
    pub fn new_resource(&mut self, path: &str, buf: Vec<u8>, pos: usize, writable: bool) -> Value {
        self.objs.push(PhpObj::Resource {
            path: path.to_string(),
            buf,
            pos,
            writable,
            dirty: false,
            closed: false,
        });
        Value::Obj((self.objs.len() - 1) as u32)
    }

    pub fn is_resource(&self, v: &Value) -> bool {
        matches!(
            self.as_array(v),
            Some(PhpObj::Resource { closed: false, .. })
        )
    }

    /// At-or-past end of the stream (`feof`).
    pub fn res_eof(&self, v: &Value) -> bool {
        match self.as_array(v) {
            Some(PhpObj::Resource { buf, pos, .. }) => *pos >= buf.len(),
            _ => true,
        }
    }

    /// The current cursor (`ftell`), or `None` if `v` is not an open resource.
    pub fn res_tell(&self, v: &Value) -> Option<i64> {
        match self.as_array(v) {
            Some(PhpObj::Resource {
                pos, closed: false, ..
            }) => Some(*pos as i64),
            _ => None,
        }
    }

    /// Read up to `n` bytes from the cursor, advancing it (`fread`).
    pub fn res_read(&mut self, v: &Value, n: usize) -> Option<String> {
        if let Some(PhpObj::Resource { buf, pos, .. }) = self.as_array_mut(v) {
            let end = (*pos + n).min(buf.len());
            let out = String::from_utf8_lossy(&buf[*pos..end]).into_owned();
            *pos = end;
            Some(out)
        } else {
            None
        }
    }

    /// Read one line (through the next `\n`, or `max` bytes) from the cursor
    /// (`fgets`); `None` at EOF or on a non-resource.
    pub fn res_gets(&mut self, v: &Value, max: Option<usize>) -> Option<String> {
        if let Some(PhpObj::Resource { buf, pos, .. }) = self.as_array_mut(v) {
            if *pos >= buf.len() {
                return None;
            }
            let cap = max.map(|m| (*pos + m).min(buf.len())).unwrap_or(buf.len());
            let mut end = *pos;
            while end < cap && buf[end] != b'\n' {
                end += 1;
            }
            if end < cap && buf[end] == b'\n' {
                end += 1; // include the newline, as fgets does
            }
            let out = String::from_utf8_lossy(&buf[*pos..end]).into_owned();
            *pos = end;
            Some(out)
        } else {
            None
        }
    }

    /// Write bytes at the cursor, extending the buffer (`fwrite`); returns the
    /// number of bytes written, or `None` for a non-writable/closed resource.
    pub fn res_write(&mut self, v: &Value, bytes: &[u8]) -> Option<usize> {
        if let Some(PhpObj::Resource {
            buf,
            pos,
            writable: true,
            dirty,
            closed: false,
            ..
        }) = self.as_array_mut(v)
        {
            if *pos > buf.len() {
                buf.resize(*pos, 0);
            }
            for (i, b) in bytes.iter().enumerate() {
                if *pos + i < buf.len() {
                    buf[*pos + i] = *b;
                } else {
                    buf.push(*b);
                }
            }
            *pos += bytes.len();
            *dirty = true;
            Some(bytes.len())
        } else {
            None
        }
    }

    /// Reposition the cursor (`fseek`/`rewind`). `whence`: 0 SEEK_SET, 1 SEEK_CUR,
    /// 2 SEEK_END. Returns `true` on a valid resource.
    pub fn res_seek(&mut self, v: &Value, offset: i64, whence: i64) -> bool {
        if let Some(PhpObj::Resource { buf, pos, .. }) = self.as_array_mut(v) {
            let base = match whence {
                1 => *pos as i64,
                2 => buf.len() as i64,
                _ => 0,
            };
            *pos = (base + offset).clamp(0, buf.len() as i64) as usize;
            true
        } else {
            false
        }
    }

    /// The data to flush to disk for a dirty resource, clearing the dirty flag —
    /// the `fileio` module performs the actual write (keeping `fs` out of the
    /// host). `None` when there is nothing to flush.
    pub fn res_flush_data(&mut self, v: &Value) -> Option<(String, Vec<u8>)> {
        if let Some(PhpObj::Resource {
            path, buf, dirty, ..
        }) = self.as_array_mut(v)
        {
            if *dirty {
                *dirty = false;
                return Some((path.clone(), buf.clone()));
            }
        }
        None
    }

    /// Mark a resource closed (`fclose`); returns `false` if it was not an open
    /// resource.
    pub fn res_close(&mut self, v: &Value) -> bool {
        if let Some(PhpObj::Resource { closed, .. }) = self.as_array_mut(v) {
            if !*closed {
                *closed = true;
                return true;
            }
        }
        false
    }

    /// `$obj->name` read (`Undef` if the object lacks the property).
    pub fn prop_get(&self, recv: &Value, name: &str) -> Value {
        match self.as_array(recv) {
            Some(PhpObj::Object { props, .. }) => {
                let raw = props.get(name).cloned().unwrap_or(Value::Undef);
                self.deref(raw)
            }
            _ => Value::Undef,
        }
    }

    /// `$obj->name = val` — mutates the shared instance behind the handle. A
    /// property a `&` binding has turned into a reference is written through its
    /// cell rather than replaced, so the alias observes the write.
    pub fn prop_set(&mut self, recv: &Value, name: &str, val: Value) {
        if let Some(slot) = self.prop_ref_slot(recv, name) {
            self.ref_cell_set(slot, val);
            return;
        }
        if let Some(PhpObj::Object { props, .. }) = self.as_array_mut(recv) {
            props.insert(name.to_string(), val);
        }
    }

    /// `$obj->name = val` from PHP source, with the PHP 8.2 deprecation for
    /// creating a property the class never declared.
    ///
    /// Split from [`prop_set`] because the engine's own writes must NOT warn:
    /// `seed_throwable` stamps `file`/`line`/`trace` onto every Throwable, and an
    /// internal write is not the user creating a dynamic property. Only the
    /// opcodes that lower a source-level `->` assignment come through here.
    pub fn prop_set_checked(&mut self, recv: &Value, name: &str, val: Value) {
        self.warn_dynamic_prop(recv, name);
        self.prop_set(recv, name, val);
    }

    /// Raise `Creation of dynamic property C::$p is deprecated` if writing `name`
    /// on `recv` would create one.
    ///
    /// It creates one when the object does not already carry the property AND no
    /// class in its chain declares it. `stdClass` is exempt — it is PHP's
    /// property bag, and `(object)` casts and `json_decode` produce it — as is any
    /// class opted in with `#[AllowDynamicProperties]`, itself or inherited.
    pub fn warn_dynamic_prop(&mut self, recv: &Value, name: &str) {
        let Some(PhpObj::Object { class, props }) = self.as_array(recv) else {
            return;
        };
        if props.contains_key(name) || class.eq_ignore_ascii_case("stdClass") {
            return;
        }
        let class = class.clone();
        if self.prop_is_declared(&class, name) || self.allows_dynamic_props(&class) {
            return;
        }
        self.deprecated(format_args!(
            "Creation of dynamic property {class}::${name} is deprecated"
        ));
    }

    /// Whether `class` or any ancestor declares a property called `name`.
    /// A declaration with no initializer still counts, so `prop_vis` — which every
    /// declaration writes — is the authority, not the defaults list.
    fn prop_is_declared(&self, class: &str, name: &str) -> bool {
        self.class_chain(class).any(|d| {
            d.prop_vis.contains_key(name) || d.prop_defaults.iter().any(|(p, _)| p == name)
        })
    }

    /// Whether `class` or any ancestor carries `#[AllowDynamicProperties]`.
    fn allows_dynamic_props(&self, class: &str) -> bool {
        self.class_chain(class).any(|d| d.allow_dynamic_props)
    }

    /// The class and its ancestors, innermost first. Stops at an undeclared
    /// parent rather than looping, so a broken chain cannot hang a lookup.
    fn class_chain<'a>(&'a self, class: &str) -> impl Iterator<Item = &'a ClassDef> + 'a {
        let mut cur = Some(class.to_ascii_lowercase());
        std::iter::from_fn(move || {
            let def = self.classes.get(&cur.take()?)?;
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
            Some(def)
        })
    }

    /// The reference slot `$obj->name` holds, if that property is a reference.
    fn prop_ref_slot(&self, recv: &Value, name: &str) -> Option<usize> {
        let Some(PhpObj::Object { props, .. }) = self.as_array(recv) else {
            return None;
        };
        self.ref_slot_of_value(props.get(name)?)
    }

    /// Ensure `$obj->name` holds an array and return its handle, creating an empty
    /// array (and storing it back on the object) if the property is unset or not an
    /// array. The pivot for indexing/appending into an array-valued property
    /// (`$this->items[] = x`, `$this->map[$k] = v`).
    pub fn prop_ensure_array(&mut self, recv: &Value, name: &str) -> Value {
        let cur = self.prop_get(recv, name);
        if self.is_array(&cur) {
            return cur;
        }
        let arr = self.new_array();
        self.prop_set(recv, name, arr.clone());
        arr
    }

    /// Resolve a method by walking the class up its parent chain; returns the
    /// defining class name plus the method definition.
    fn resolve_method(&self, class: &str, method: &str) -> Option<(String, FuncDef)> {
        let mut cur = Some(class.to_ascii_lowercase());
        while let Some(c) = cur {
            let def = self.classes.get(&c)?;
            if let Some(m) = def.methods.get(method) {
                return Some((c, m.clone()));
            }
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
        }
        None
    }

    /// The class whose method is currently executing (for visibility checks),
    /// derived from the innermost frame's `Class::method` name. `None` in the
    /// global scope, a free function, or a closure (frame names without `::`).
    fn current_class_ctx(&self) -> Option<String> {
        let name = self.scopes.last()?.name.as_ref()?;
        name.rsplit_once("::").map(|(cls, _)| cls.to_string())
    }

    /// Resolve the declared visibility of property `name` on `class`, walking the
    /// parent chain; returns the declaring class (lowercased) and its visibility.
    /// `None` for a dynamic/undeclared property (treated as public).
    fn resolve_prop_vis(&self, class: &str, name: &str) -> Option<(String, Visibility)> {
        let mut cur = Some(class.to_ascii_lowercase());
        while let Some(c) = cur {
            let def = self.classes.get(&c)?;
            if let Some(v) = def.prop_vis.get(name) {
                return Some((c, *v));
            }
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
        }
        None
    }

    /// Whether the current class context may access a member declared with
    /// `vis` in `declaring` (lowercased). Public: always. Private: only from the
    /// declaring class itself. Protected: from the declaring class or any class in
    /// the same inheritance line (ancestor or descendant).
    fn visibility_allows(&self, vis: Visibility, declaring: &str) -> bool {
        match vis {
            Visibility::Public => true,
            Visibility::Private => {
                matches!(self.current_class_ctx(), Some(c) if c.eq_ignore_ascii_case(declaring))
            }
            Visibility::Protected => match self.current_class_ctx() {
                Some(c) => self.class_is_a(&c, declaring) || self.class_is_a(declaring, &c),
                None => false,
            },
        }
    }

    /// Decide what `$obj->name` does for the operation whose magic method is
    /// `magic` (`__get`, `__set`, `__isset`, `__unset`).
    ///
    /// The order is the reference's, and the two halves are easy to get backwards:
    /// a magic method is consulted for a property that is not THERE just as much
    /// as for one that is out of reach, and it is consulted BEFORE any access
    /// error. So `unset($o->pub)` on a class with `__get` really does route later
    /// reads of that public property through `__get` — the slot is gone, and gone
    /// is indistinguishable from never-declared at this point.
    ///
    /// A magic method already running for this object and property is skipped,
    /// which is the recursion guard described on [`magic_in_progress`].
    ///
    /// [`magic_in_progress`]: PhpHost::magic_in_progress
    pub fn prop_access(&self, recv: &Value, name: &str, magic: &'static str) -> PropAccess {
        let Some(PhpObj::Object { class, props }) = self.as_array(recv) else {
            // Not an object: no visibility to enforce and no magic to call.
            return PropAccess::Absent;
        };
        let declared = self.resolve_prop_vis(class, name);
        // A property no class declares carries no visibility, so it is reachable.
        let reachable = match &declared {
            Some((declaring, vis)) => self.visibility_allows(*vis, declaring),
            None => true,
        };
        if reachable && props.contains_key(name) {
            return PropAccess::Direct;
        }
        let handle = match recv {
            Value::Obj(h) => *h,
            _ => return PropAccess::Absent,
        };
        let guarded = self
            .magic_in_progress
            .iter()
            .any(|(h, p, m)| *h == handle && p == name && *m == magic);
        if !guarded && self.class_has_method(class, magic) {
            return PropAccess::Magic;
        }
        match declared {
            // Out of reach rather than absent — the reference names the
            // visibility and the class that declared it, not the class of the
            // object, which for an inherited private property is not the same.
            Some((declaring, vis)) if !reachable => {
                let vname = match vis {
                    Visibility::Private => "private",
                    Visibility::Protected => "protected",
                    Visibility::Public => unreachable!("public is always reachable"),
                };
                let class = self.class_display_name(&declaring);
                PropAccess::Denied(format!("Cannot access {vname} property {class}::${name}"))
            }
            _ => PropAccess::Absent,
        }
    }

    /// Mark a magic property access as in progress; the matching
    /// [`magic_leave`](PhpHost::magic_leave) must follow it.
    pub fn magic_enter(&mut self, recv: &Value, name: &str, magic: &'static str) {
        if let Value::Obj(h) = recv {
            self.magic_in_progress.push((*h, name.to_string(), magic));
        }
    }

    pub fn magic_leave(&mut self) {
        self.magic_in_progress.pop();
    }

    /// The open magic-access count, for a caller that has to put it back after an
    /// unwind — the same problem, and the same fix, as `@expr` suppression.
    pub fn magic_depth(&self) -> usize {
        self.magic_in_progress.len()
    }

    pub fn magic_restore(&mut self, depth: usize) {
        self.magic_in_progress.truncate(depth);
    }

    /// A class name as declared, recovered from the lowercased key the class
    /// table is indexed by, so a diagnostic prints `MyClass` and not `myclass`.
    fn class_display_name(&self, lowered: &str) -> String {
        self.classes
            .get(lowered)
            .map(|d| d.name.clone())
            .unwrap_or_else(|| lowered.to_string())
    }

    /// Remove `$obj->name` from the object's property table — `unset($o->p)`.
    /// A property that is not there is not an error; the caller has already
    /// decided this is the right thing to do (see [`prop_access`]).
    ///
    /// [`prop_access`]: PhpHost::prop_access
    pub fn prop_remove(&mut self, recv: &Value, name: &str) {
        if let Some(PhpObj::Object { props, .. }) = self.as_array_mut(recv) {
            props.shift_remove(name);
        }
    }

    /// Enforce method visibility for `$obj->method()`. Same policy as properties;
    /// the message matches PHP's `Call to <vis> method C::m() from <scope>`.
    pub fn check_method_access(&self, class: &str, method: &str) -> Result<(), String> {
        let method_l = method.to_ascii_lowercase();
        // Walk the chain to the declaring class and its visibility.
        let mut cur = Some(class.to_ascii_lowercase());
        let found = loop {
            let Some(c) = cur else {
                break None;
            };
            let Some(def) = self.classes.get(&c) else {
                break None;
            };
            if let Some(v) = def.method_vis.get(&method_l) {
                break Some((c, *v));
            }
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
        };
        let Some((declaring, vis)) = found else {
            return Ok(());
        };
        if self.visibility_allows(vis, &declaring) {
            return Ok(());
        }
        let vname = match vis {
            Visibility::Private => "private",
            Visibility::Protected => "protected",
            Visibility::Public => unreachable!(),
        };
        let scope = match self.current_class_ctx() {
            Some(c) => format!("scope {c}"),
            None => "global scope".to_string(),
        };
        Err(format!(
            "Call to {vname} method {class}::{method}() from {scope}"
        ))
    }

    /// The ordered property-default initializer chunks for a class, parent props
    /// first and child declarations overriding by name. `None` if unknown class.
    fn class_prop_default_chunks(&self, class: &str) -> Option<Vec<(String, Chunk)>> {
        let cl = class.to_ascii_lowercase();
        if !self.classes.contains_key(&cl) {
            return None;
        }
        // Build the chain child → root, then apply root → child so a child's
        // redeclared default wins while parent props keep their leading position.
        let mut chain: Vec<&ClassDef> = Vec::new();
        let mut cur = Some(cl);
        while let Some(c) = cur {
            let Some(def) = self.classes.get(&c) else {
                break;
            };
            chain.push(def);
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
        }
        let mut map: IndexMap<String, Chunk> = IndexMap::new();
        for def in chain.into_iter().rev() {
            for (name, chunk) in &def.prop_defaults {
                map.insert(name.clone(), chunk.clone());
            }
        }
        Some(map.into_iter().collect())
    }

    /// Resolve `Class::$name` to its storage key and initializer chunk, walking
    /// the parent chain to the class that actually declares the static property.
    /// The key is `"declaringclass::name"` so a subclass shares the parent's cell.
    fn resolve_static_key(&self, class: &str, name: &str) -> Option<(String, Chunk)> {
        let mut cur = Some(class.to_ascii_lowercase());
        while let Some(c) = cur {
            let def = self.classes.get(&c)?;
            if let Some((_, chunk)) = def.static_prop_defaults.iter().find(|(n, _)| n == name) {
                return Some((format!("{c}::{name}"), chunk.clone()));
            }
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
        }
        None
    }

    /// The stored value of a static property, by storage key (`None` = not yet
    /// initialized).
    fn get_static_stored(&self, key: &str) -> Option<Value> {
        self.static_props.get(key).cloned()
    }

    /// Store a static property's value by storage key.
    fn set_static_stored(&mut self, key: &str, val: Value) {
        self.static_props.insert(key.to_string(), val);
    }

    /// Bind a function-local `$name` to its persistent `static` slot. On first
    /// encounter the cell is created with `init`; later calls reuse it, so the
    /// value survives across calls. The current scope's name is aliased to the
    /// cell (like a reference), so ordinary reads/writes of `$name` in the body
    /// hit the persistent storage.
    pub fn bind_static_local(&mut self, name: &str, slot_key: &str, init: Value) {
        let slot = match self.static_slots.get(slot_key) {
            Some(&s) => s,
            None => {
                self.ref_cells.push(init);
                let s = self.ref_cells.len() - 1;
                self.static_slots.insert(slot_key.to_string(), s);
                s
            }
        };
        let idx = self.scope_idx(name);
        if let Some(scope) = self.scopes.get_mut(idx) {
            scope.vars.remove(name);
            scope.refs.insert(name.to_string(), slot);
        }
    }

    // ── enums (PHP 8.1) ──────────────────────────────────────────────────────

    /// Whether `class` is an `enum` (case-insensitive).
    pub fn is_enum_class(&self, class: &str) -> bool {
        self.classes
            .get(&class.to_ascii_lowercase())
            .map(|d| d.is_enum)
            .unwrap_or(false)
    }

    /// The declared case's optional backing-value chunk (`Some(chunk_opt)` when the
    /// enum declares a case of this exact name, else `None`).
    fn enum_case_chunk(&self, class: &str, case: &str) -> Option<Option<Chunk>> {
        let d = self.classes.get(&class.to_ascii_lowercase())?;
        d.enum_cases
            .iter()
            .find(|(n, _)| n == case)
            .map(|(_, c)| c.clone())
    }

    /// The enum's case names in declaration order.
    fn enum_case_names(&self, class: &str) -> Vec<String> {
        self.classes
            .get(&class.to_ascii_lowercase())
            .map(|d| d.enum_cases.iter().map(|(n, _)| n.clone()).collect())
            .unwrap_or_default()
    }

    fn enum_case_cached(&self, key: &str) -> Option<Value> {
        self.enum_case_cache.get(key).cloned()
    }

    fn enum_case_store(&mut self, key: &str, v: Value) {
        self.enum_case_cache.insert(key.to_string(), v);
    }

    /// Allocate an enum-case instance with its `name` (and, for a backed enum,
    /// `value`) properties. `class` keeps its source casing for `get_class`.
    fn new_enum_object(&mut self, class: &str, name: &str, value: Option<Value>) -> Value {
        let mut props: IndexMap<String, Value> = IndexMap::new();
        props.insert("name".to_string(), Value::str(name.to_string()));
        if let Some(v) = value {
            props.insert("value".to_string(), v);
        }
        self.objs.push(PhpObj::Object {
            class: class.to_string(),
            props,
        });
        Value::Obj((self.objs.len() - 1) as u32)
    }

    /// The initializer chunk for `Class::name`, walking the parent chain.
    fn resolve_const_chunk(&self, class: &str, name: &str) -> Option<Chunk> {
        let mut cur = Some(class.to_ascii_lowercase());
        while let Some(c) = cur {
            let def = self.classes.get(&c)?;
            if let Some((_, chunk)) = def.consts.iter().find(|(n, _)| n == name) {
                return Some(chunk.clone());
            }
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
        }
        None
    }

    fn as_array_mut(&mut self, v: &Value) -> Option<&mut PhpObj> {
        match v {
            Value::Obj(h) => self.objs.get_mut(*h as usize),
            _ => None,
        }
    }

    fn as_array(&self, v: &Value) -> Option<&PhpObj> {
        match v {
            Value::Obj(h) => self.objs.get(*h as usize),
            _ => None,
        }
    }

    /// Normalize a value used as an array key, matching PHP: integer-valued
    /// strings and floats become int keys, bools/null fold to int/"".
    fn norm_key(&self, key: &Value) -> ArrayKey {
        match key {
            Value::Int(n) => ArrayKey::Int(*n),
            Value::Bool(b) => ArrayKey::Int(*b as i64),
            Value::Float(f) => ArrayKey::Int(*f as i64),
            Value::Undef => ArrayKey::Str(String::new()),
            Value::Str(s) => {
                // A canonical decimal integer string becomes an int key.
                if let Ok(n) = s.parse::<i64>() {
                    if n.to_string() == **s {
                        return ArrayKey::Int(n);
                    }
                }
                ArrayKey::Str(s.to_string())
            }
            Value::Obj(_) => ArrayKey::Str("Array".into()),
            _ => ArrayKey::Str(self.to_str(key)),
        }
    }

    /// The reference slot the entry `arr[k]` holds, if that element is a reference.
    fn entry_ref_slot(&self, arr: &Value, k: &ArrayKey) -> Option<usize> {
        let Some(PhpObj::Array { entries, .. }) = self.as_array(arr) else {
            return None;
        };
        self.ref_slot_of_value(entries.get(k)?)
    }

    /// `$arr[key]` read. Also indexes strings (single-character substring).
    pub fn index_get(&self, recv: &Value, key: &Value) -> Value {
        if let Some(PhpObj::Array { entries, .. }) = self.as_array(recv) {
            let k = self.norm_key(key);
            let raw = entries.get(&k).cloned().unwrap_or(Value::Undef);
            return self.deref(raw);
        }
        if let Value::Str(s) = recv {
            // PHP string offsets are byte-indexed and accept negatives (`$s[-1]`
            // is the last byte). An out-of-range offset yields `Undef`, so
            // `isset($s[i])` is false there (a plain read still echoes as "").
            let bytes = s.as_bytes();
            let len = bytes.len() as i64;
            let mut i = key.to_int();
            if i < 0 {
                i += len;
            }
            if i >= 0 && i < len {
                let b = i as usize;
                return Value::str(String::from_utf8_lossy(&bytes[b..b + 1]).into_owned());
            }
        }
        Value::Undef
    }

    /// `$arr[key]` read in a *value* context, where a miss is a mistake rather
    /// than a question. Same result as [`index_get`], plus PHP's diagnostic:
    /// `Undefined array key K` for a missing element, `Uninitialized string
    /// offset N` past the end of a string, and `Trying to access array offset on
    /// <type>` when the receiver is not subscriptable at all.
    pub fn index_get_warn(&mut self, recv: &Value, key: &Value) -> Value {
        if let Some(PhpObj::Array { entries, .. }) = self.as_array(recv) {
            let k = self.norm_key(key);
            match entries.get(&k).cloned() {
                Some(v) => return self.deref(v),
                None => {
                    let k = self.diag_key(key);
                    self.warn(format_args!("Undefined array key {k}"));
                    return Value::Undef;
                }
            }
        }
        if let Value::Str(s) = recv {
            let len = s.len() as i64;
            let mut i = key.to_int();
            if i < 0 {
                i += len;
            }
            if i >= 0 && i < len {
                return self.index_get(recv, key);
            }
            let off = key.to_int();
            self.warn(format_args!("Uninitialized string offset {off}"));
            return Value::Undef;
        }
        // A closure/generator/resource handle is not an array either, but PHP
        // raises an Error for those rather than a warning; only the scalars and
        // null reach this warning.
        if !matches!(recv, Value::Obj(_)) {
            let t = self.diag_offset_type(recv);
            self.warn(format_args!("Trying to access array offset on {t}"));
        }
        Value::Undef
    }

    /// `$name` read in a value context — [`get_var`] plus `Undefined variable $x`
    /// when the name is not bound. Compiler temporaries (which are prefixed with
    /// `@`, outside the PHP identifier space) never warn: they are not the user's
    /// variables and are always written before they are read.
    pub fn get_var_warn(&mut self, name: &str) -> Value {
        let idx = self.scope_idx(name);
        if let Some(scope) = self.scopes.get(idx) {
            if let Some(&slot) = scope.refs.get(name) {
                return self.ref_cells.get(slot).cloned().unwrap_or(Value::Undef);
            }
            if let Some(v) = scope.vars.get(name) {
                return v.clone();
            }
        }
        if !name.starts_with('@') {
            self.warn(format_args!("Undefined variable ${name}"));
        }
        Value::Undef
    }

    /// `$obj->name` read in a value context — [`prop_get`] plus PHP's diagnostic:
    /// `Undefined property: C::$p` when the instance has no such property, and
    /// `Attempt to read property "p" on <type>` when the receiver is not an
    /// object at all.
    pub fn prop_get_warn(&mut self, recv: &Value, name: &str) -> Value {
        match self.as_array(recv) {
            Some(PhpObj::Object { class, props }) => match props.get(name).cloned() {
                Some(v) => self.deref(v),
                None => {
                    let class = class.clone();
                    self.warn(format_args!("Undefined property: {class}::${name}"));
                    Value::Undef
                }
            },
            // An array handle reads as a property-less value; PHP reports the
            // same "read property on array" as for any other non-object.
            _ => {
                let t = self.diag_type(recv);
                self.warn(format_args!("Attempt to read property \"{name}\" on {t}"));
                Value::Undef
            }
        }
    }

    /// `$var[key] = val` on the named scope variable, auto-vivifying an array.
    pub fn index_set_var(&mut self, name: &str, key: &Value, val: Value) {
        let arr = self.ensure_array_var(name);
        self.arr_set_key(&arr, key, val);
    }

    /// `$var[] = val` append on the named scope variable, auto-vivifying.
    pub fn append_var(&mut self, name: &str, val: Value) {
        let arr = self.ensure_array_var(name);
        if let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.as_array_mut(&arr)
        {
            let k = ArrayKey::Int(*next_index);
            *next_index += 1;
            entries.insert(k, val);
        }
    }

    /// Return the array handle held by `$name`, creating an empty array (and
    /// storing it back) if the variable is unset or not an array.
    fn ensure_array_var(&mut self, name: &str) -> Value {
        let cur = self.get_var(name);
        if matches!(self.as_array(&cur), Some(PhpObj::Array { .. })) {
            return cur;
        }
        let arr = self.new_array();
        self.set_var(name, arr.clone());
        arr
    }

    /// Descend the array handle `arr` along `keys`, auto-vivifying an array at each
    /// step, and return the handle of the array the final segment lands in. A slot
    /// that is unset — or holds a non-array — is (re)created as an empty array,
    /// matching PHP's auto-vivification of nested lvalues.
    fn ensure_path_from(&mut self, mut arr: Value, keys: &[Value]) -> Value {
        for key in keys {
            let k = self.norm_key(key);
            let child = match self.as_array(&arr) {
                // A referenced element descends into whatever its cell holds, so
                // `$r = &$a['x']; $a['x']['y'] = 1;` writes through the alias.
                Some(PhpObj::Array { entries, .. }) => {
                    entries.get(&k).cloned().map(|v| self.deref(v))
                }
                _ => None,
            };
            arr = match child {
                Some(v) if self.is_array(&v) => v,
                _ => {
                    let new = self.new_array();
                    self.arr_set_key(&arr, key, new.clone());
                    new
                }
            };
        }
        arr
    }

    /// Like `ensure_path_from`, but rooted at the scope variable `$name`
    /// (auto-vivifying the variable itself into an array).
    fn ensure_path_array(&mut self, name: &str, keys: &[Value]) -> Value {
        let root = self.ensure_array_var(name);
        self.ensure_path_from(root, keys)
    }

    /// `$name[k1]..[kN] = val` — set the deepest element along a key path,
    /// auto-vivifying the intermediate arrays (`N >= 1`).
    pub fn index_set_path(&mut self, name: &str, keys: &[Value], val: Value) {
        let Some((last, inter)) = keys.split_last() else {
            return;
        };
        let arr = self.ensure_path_array(name, inter);
        self.arr_set_key(&arr, last, val);
    }

    /// `$name[k1]..[kM][] = val` — append into the array reached along `keys`,
    /// auto-vivifying (`M >= 0`, so `$a[] = v` is the empty-path case).
    pub fn append_path(&mut self, name: &str, keys: &[Value], val: Value) {
        let arr = self.ensure_path_array(name, keys);
        self.arr_push_auto(&arr, val);
    }

    /// Read `$name[k1]..[kN]` without mutating (for read-modify-write compound
    /// assignment and `++`/`--` on an element). Missing slots read as `Undef`.
    pub fn index_get_path(&self, name: &str, keys: &[Value]) -> Value {
        let mut cur = self.get_var(name);
        for key in keys {
            cur = self.index_get(&cur, key);
        }
        cur
    }

    /// [`index_get_path`] for the read half of a read-modify-write — `$a[k] += 1`,
    /// `$a[k]++` — which diagnoses every step of the path.
    ///
    /// PHP fetches an RW path in write mode: an unset container is reported and
    /// then *auto-vivified* into an empty array, so the next segment reports its
    /// own missing key rather than "array offset on null". That is what makes
    /// `$a['p']['q'] += 5` on a fresh `$a` print `Undefined variable $a`,
    /// `Undefined array key "p"`, `Undefined array key "q"` — three misses, not a
    /// miss followed by two null-offset complaints, which is what a plain read of
    /// the same path prints.
    pub fn index_get_path_warn(&mut self, name: &str, keys: &[Value]) -> Value {
        let mut cur = self.get_var_warn(name);
        for key in keys {
            if matches!(cur, Value::Undef) {
                // The write-mode fetch put an empty array here; the key is still
                // missing from it.
                let k = self.diag_key(key);
                self.warn(format_args!("Undefined array key {k}"));
                continue;
            }
            cur = self.index_get_warn(&cur, key);
        }
        cur
    }

    /// Append a fresh empty array as a new element of `$name[k1]..[kN]` and return
    /// its handle — the pivot for a mid-path append (`$a[][k] = v`): the caller
    /// keeps writing through the returned child handle, so each `[]` in the chain
    /// appends exactly one new element the way PHP does.
    pub fn path_append_child(&mut self, name: &str, keys: &[Value]) -> Value {
        let arr = self.ensure_path_array(name, keys);
        let new = self.new_array();
        self.arr_push_auto(&arr, new.clone());
        new
    }

    /// Append `v` under the next integer key of the array `arr` (a handle).
    pub fn arr_push_auto(&mut self, arr: &Value, v: Value) {
        if let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.as_array_mut(arr)
        {
            let k = ArrayKey::Int(*next_index);
            *next_index += 1;
            entries.insert(k, v);
        }
    }

    /// Insert `v` under `key` in the array `arr` (a handle). An element that a `&`
    /// binding has turned into a reference is written *through* — the alias sees
    /// the new value and the slot stays a reference — instead of being replaced.
    pub fn arr_set_key(&mut self, arr: &Value, key: &Value, v: Value) {
        let k = self.norm_key(key);
        if let Some(slot) = self.entry_ref_slot(arr, &k) {
            self.ref_cell_set(slot, v);
            return;
        }
        if let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.as_array_mut(arr)
        {
            if let ArrayKey::Int(n) = k {
                if n >= *next_index {
                    *next_index = n + 1;
                }
            }
            entries.insert(k, v);
        }
    }

    /// Replace an array handle's entries with a re-indexed (`0..n`) value list,
    /// in place — the mutation is visible through every variable holding the same
    /// handle (`sort`/`rsort`). No-op if `arr` is not an array.
    /// Replace every entry of `arr` with `src`'s, keys included — how a
    /// by-reference OUT array is written through a handle the caller already
    /// held. A no-op unless both are arrays.
    pub fn arr_replace_all(&mut self, arr: &Value, src: &Value) {
        let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.as_array(src)
        else {
            return;
        };
        let (entries, next_index) = (entries.clone(), *next_index);
        if let Some(PhpObj::Array {
            entries: dst,
            next_index: dst_next,
        }) = self.as_array_mut(arr)
        {
            *dst = entries;
            *dst_next = next_index;
        }
    }

    pub fn arr_set_reindexed(&mut self, arr: &Value, vals: Vec<Value>) {
        if let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.as_array_mut(arr)
        {
            entries.clear();
            *next_index = 0;
            for v in vals {
                let k = ArrayKey::Int(*next_index);
                *next_index += 1;
                entries.insert(k, v);
            }
        }
    }

    /// Replace an array handle's entries with an explicit ordered `(key, value)`
    /// list, preserving keys, in place (`asort`/`ksort`).
    pub fn arr_set_pairs(&mut self, arr: &Value, pairs: Vec<(Value, Value)>) {
        // Normalize keys under `&self` first, then take the `&mut self` borrow.
        let normed: Vec<(ArrayKey, Value)> = pairs
            .into_iter()
            .map(|(k, v)| (self.norm_key(&k), v))
            .collect();
        if let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.as_array_mut(arr)
        {
            entries.clear();
            *next_index = 0;
            for (k, v) in normed {
                if let ArrayKey::Int(n) = k {
                    if n >= *next_index {
                        *next_index = n + 1;
                    }
                }
                entries.insert(k, v);
            }
        }
    }

    /// Rebuild an array handle's entries from an ordered `(key, value)` list,
    /// renumbering integer keys `0..` while preserving string keys — the shared
    /// core of `array_shift`/`array_unshift`/`array_splice`. No-op if `arr` is not
    /// an array.
    fn arr_rebuild_reindexed(&mut self, arr: &Value, pairs: Vec<(Value, Value)>) {
        // Normalize keys under `&self` first, then take the `&mut self` borrow.
        let normed: Vec<(ArrayKey, Value)> = pairs
            .into_iter()
            .map(|(k, v)| (self.norm_key(&k), v))
            .collect();
        if let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.as_array_mut(arr)
        {
            entries.clear();
            *next_index = 0;
            for (k, v) in normed {
                match k {
                    // Integer keys are renumbered sequentially; string keys stay.
                    ArrayKey::Int(_) => {
                        let nk = ArrayKey::Int(*next_index);
                        *next_index += 1;
                        entries.insert(nk, v);
                    }
                    ArrayKey::Str(_) => {
                        entries.insert(k, v);
                    }
                }
            }
        }
    }

    /// Remove and return the last element of `$var` (`array_pop`). Remaining keys
    /// are left untouched. PHP resets the next-append index only when the popped
    /// element held the top of the current append run (its key was `next_index-1`);
    /// a sparse/gapped array (e.g. `[5=>a, 10=>b]`) keeps its `next_index`.
    pub fn arr_pop_var(&mut self, name: &str) -> Value {
        let arr = self.get_var(name);
        if let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.as_array_mut(&arr)
        {
            let before = *next_index;
            let popped = entries.pop();
            if let Some((ArrayKey::Int(n), _)) = &popped {
                if *n == before - 1 {
                    *next_index = *n;
                }
            }
            return popped.map(|(_, v)| v).unwrap_or(Value::Undef);
        }
        Value::Undef
    }

    /// Remove and return the first element of `$var` (`array_shift`), reindexing
    /// integer keys from `0` while preserving string keys. Returns null if the
    /// variable is unset/empty or not an array.
    pub fn arr_shift_var(&mut self, name: &str) -> Value {
        let arr = self.get_var(name);
        let Some(mut pairs) = self.array_pairs(&arr) else {
            return Value::Undef;
        };
        if pairs.is_empty() {
            return Value::Undef;
        }
        let (_, first) = pairs.remove(0);
        self.arr_rebuild_reindexed(&arr, pairs);
        first
    }

    /// Append each value to `$var` (`array_push`), auto-vivifying an array;
    /// returns the new element count.
    pub fn arr_push_var(&mut self, name: &str, vals: Vec<Value>) -> Value {
        let arr = self.ensure_array_var(name);
        for v in vals {
            self.arr_push_auto(&arr, v);
        }
        Value::int(self.array_len(&arr))
    }

    /// Prepend `vals` to `$var` (`array_unshift`) as a fresh `0`-based run,
    /// reindexing the existing integer keys after them (string keys preserved);
    /// returns the new element count.
    pub fn arr_unshift_var(&mut self, name: &str, vals: Vec<Value>) -> Value {
        let arr = self.ensure_array_var(name);
        let existing = self.array_pairs(&arr).unwrap_or_default();
        let mut combined: Vec<(Value, Value)> = Vec::with_capacity(vals.len() + existing.len());
        // A placeholder integer key marks each new value for renumbering; the
        // rebuild renumbers all integer keys, so the placeholder value is unused.
        for v in vals {
            combined.push((Value::int(0), v));
        }
        combined.extend(existing);
        self.arr_rebuild_reindexed(&arr, combined);
        Value::int(self.array_len(&arr))
    }

    /// `array_splice($var, offset, length?, replacement?)` — remove `length`
    /// elements at `offset` (negatives count from the end; omitted length runs to
    /// the end) and splice `replacement` (an array or a single value) in their
    /// place, reindexing integer keys. Returns a new array of the removed
    /// elements.
    pub fn arr_splice_var(&mut self, name: &str, args: &[Value]) -> Value {
        let arr = self.ensure_array_var(name);
        let pairs = self.array_pairs(&arr).unwrap_or_default();
        let n = pairs.len() as i64;
        let mut off = args.first().map(|v| v.to_int()).unwrap_or(0);
        if off < 0 {
            off = (n + off).max(0);
        }
        let off = off.min(n).max(0) as usize;
        let len = match args.get(1) {
            Some(v) if !matches!(v, Value::Undef) => {
                let l = v.to_int();
                if l < 0 {
                    (n - off as i64 + l).max(0) as usize
                } else {
                    (l as usize).min(pairs.len() - off)
                }
            }
            _ => pairs.len() - off,
        };
        let end = (off + len).min(pairs.len());
        // The replacement flattens to a value list (an array's keys are dropped).
        let replacement: Vec<Value> = match args.get(2) {
            Some(v) if self.is_array(v) => self
                .array_pairs(v)
                .unwrap_or_default()
                .into_iter()
                .map(|(_, x)| x)
                .collect(),
            Some(v) if !matches!(v, Value::Undef) => vec![v.clone()],
            _ => Vec::new(),
        };
        let removed: Vec<(Value, Value)> = pairs[off..end].to_vec();
        // Rebuild the source: kept prefix, replacement run, kept suffix.
        let mut rebuilt: Vec<(Value, Value)> = Vec::with_capacity(pairs.len());
        rebuilt.extend(pairs[..off].iter().cloned());
        for v in replacement {
            rebuilt.push((Value::int(0), v));
        }
        rebuilt.extend(pairs[end..].iter().cloned());
        self.arr_rebuild_reindexed(&arr, rebuilt);
        let out = self.new_array();
        self.arr_rebuild_reindexed(&out, removed);
        out
    }

    pub fn array_keys(&mut self, recv: &Value) -> Value {
        let keys: Vec<Value> = match self.as_array(recv) {
            Some(PhpObj::Array { entries, .. }) => entries.keys().map(|k| k.to_value()).collect(),
            _ => Vec::new(),
        };
        let arr = self.new_array();
        if let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.as_array_mut(&arr)
        {
            for (i, k) in keys.into_iter().enumerate() {
                entries.insert(ArrayKey::Int(i as i64), k);
                *next_index = (i + 1) as i64;
            }
        }
        arr
    }

    pub fn array_len(&self, recv: &Value) -> i64 {
        match self.as_array(recv) {
            Some(PhpObj::Array { entries, .. }) => entries.len() as i64,
            _ => 0,
        }
    }

    /// The array's `(key, value)` pairs, cloned (for `print_r`/`implode`/etc.).
    pub fn array_pairs(&self, recv: &Value) -> Option<Vec<(Value, Value)>> {
        match self.as_array(recv) {
            Some(PhpObj::Array { entries, .. }) => {
                let raw: Vec<(Value, Value)> = entries
                    .iter()
                    .map(|(k, v)| (k.to_value(), v.clone()))
                    .collect();
                Some(raw.into_iter().map(|(k, v)| (k, self.deref(v))).collect())
            }
            _ => None,
        }
    }

    /// The array's `(key, value, is_reference)` triples, values still raw. Only
    /// `var_dump`, which prints a `&` before a referenced element, needs to see
    /// which slots are references; every other reader wants [`array_pairs`].
    pub fn array_pairs_marked(&self, recv: &Value) -> Option<Vec<(Value, Value, bool)>> {
        let Some(PhpObj::Array { entries, .. }) = self.as_array(recv) else {
            return None;
        };
        let raw: Vec<(Value, Value)> = entries
            .iter()
            .map(|(k, v)| (k.to_value(), v.clone()))
            .collect();
        Some(
            raw.into_iter()
                .map(|(k, v)| {
                    let is_ref = self.ref_slot_of_value(&v).is_some();
                    (k, self.deref(v), is_ref)
                })
                .collect(),
        )
    }

    pub fn is_array(&self, v: &Value) -> bool {
        matches!(self.as_array(v), Some(PhpObj::Array { .. }))
    }

    // ── value coercions (PHP semantics) ────────────────────────────────────

    /// PHP truthiness: `false`, `0`, `0.0`, `""`, `"0"`, `null`, and the empty
    /// array are falsy; everything else is truthy. A closure/object handle is
    /// always truthy (only the empty *array* is falsy among heap objects).
    pub fn is_truthy(&self, v: &Value) -> bool {
        match v {
            Value::Undef => false,
            Value::Bool(b) => *b,
            Value::Int(n) => *n != 0,
            Value::Float(f) => *f != 0.0,
            Value::Str(s) => !(s.is_empty() || s.as_str() == "0"),
            Value::Obj(_) => !self.is_array(v) || self.array_len(v) != 0,
            _ => true,
        }
    }

    /// PHP string cast (`(string)` / echo / interpolation).
    pub fn to_str(&self, v: &Value) -> String {
        match v {
            Value::Undef => String::new(),
            Value::Bool(b) => if *b { "1" } else { "" }.to_string(),
            Value::Int(n) => n.to_string(),
            Value::Float(f) => float_to_php_string(*f),
            Value::Str(s) => s.to_string(),
            Value::Obj(_) => "Array".to_string(),
            _ => String::new(),
        }
    }

    /// `++`/`--` applied to one value, with the diagnostics PHP raises for the
    /// operand types the operators do not actually change.
    ///
    /// The operators are not arithmetic on `$x + 1`: `null--` and `true++` leave
    /// the value alone, `""++` produces the *string* `"1"`, and `++` on a
    /// non-numeric string is Perl-style alphanumeric succession (`"Az"` → `"Ba"`,
    /// `"zz"` → `"aaa"`) rather than a numeric coercion. Only a numeric string
    /// goes through the number path.
    pub fn incdec_value(&mut self, old: &Value, inc: bool) -> Value {
        let delta = if inc { 1 } else { -1 };
        match old {
            // null++ is 1; null-- is a no-op PHP has announced it will change.
            Value::Undef => {
                if inc {
                    Value::int(1)
                } else {
                    self.warn(
                        "Decrement on type null has no effect, this will change in the next \
                         major version of PHP",
                    );
                    Value::Undef
                }
            }
            // Neither operator has ever affected a bool.
            Value::Bool(_) => {
                let word = if inc { "Increment" } else { "Decrement" };
                self.warn(format_args!(
                    "{word} on type bool has no effect, this will change in the next major \
                     version of PHP"
                ));
                old.clone()
            }
            Value::Int(n) => Value::int(n + delta),
            Value::Float(f) => Value::float(f + delta as f64),
            Value::Str(s) if is_numeric_string(s) => match parse_php_number(s) {
                Value::Float(f) => Value::float(f + delta as f64),
                Value::Int(n) => Value::int(n + delta),
                other => other,
            },
            Value::Str(s) if s.is_empty() => {
                if inc {
                    self.deprecated(
                        "Increment on non-numeric string is deprecated, use str_increment() \
                         instead",
                    );
                    Value::str("1")
                } else {
                    self.deprecated("Decrement on empty string is deprecated as non-numeric");
                    Value::int(-1)
                }
            }
            Value::Str(s) => {
                if inc {
                    self.deprecated(
                        "Increment on non-numeric string is deprecated, use str_increment() \
                         instead",
                    );
                    Value::str(increment_alnum_string(s))
                } else {
                    self.deprecated(
                        "Decrement on non-numeric string has no effect and is deprecated",
                    );
                    old.clone()
                }
            }
            other => other.clone(),
        }
    }

    /// Coerce to an `Int`/`Float` `Value` for arithmetic and comparison.
    pub fn to_number(&self, v: &Value) -> Value {
        match v {
            Value::Int(_) | Value::Float(_) => v.clone(),
            Value::Bool(b) => Value::int(*b as i64),
            Value::Undef => Value::int(0),
            Value::Str(s) => parse_php_number(s),
            // Empty array → 0, non-empty array → 1; a closure/object casts to 1.
            Value::Obj(_) => Value::int(if self.is_array(v) && self.array_len(v) == 0 {
                0
            } else {
                1
            }),
            _ => Value::int(0),
        }
    }

    /// PHP `gettype` name.
    pub fn type_name(&self, v: &Value) -> &'static str {
        match v {
            Value::Undef => "NULL",
            Value::Bool(_) => "boolean",
            Value::Int(_) => "integer",
            Value::Float(_) => "double",
            Value::Str(_) => "string",
            Value::Obj(_) => {
                if self.is_array(v) {
                    "array"
                } else if self.is_resource(v) {
                    "resource"
                } else {
                    "object"
                }
            }
            _ => "unknown type",
        }
    }
}

/// The standard predefined constants seeded onto every fresh host. Covers the
/// core/version/OS constants, the `M_*` math constants, and the integer flag
/// constants the standard library accepts (sort/count/str_pad/array_filter/preg/
/// json/filter/mbstring/file/pathinfo/entities/error-level families), so a
/// program that writes `SORT_STRING` or `FILTER_VALIDATE_EMAIL` gets the real
/// integer rather than the bare name.
/// Whether a variable name (sans `$`) is a PHP superglobal — auto-global across
/// every scope, resolved against the global frame.
fn is_superglobal(name: &str) -> bool {
    matches!(
        name,
        "_SERVER"
            | "_GET"
            | "_POST"
            | "_REQUEST"
            | "_COOKIE"
            | "_FILES"
            | "_ENV"
            | "_SESSION"
            | "GLOBALS"
            | "argv"
            | "argc"
    )
}

fn predefined_constants() -> FxHashMap<String, Value> {
    let mut m = FxHashMap::default();
    let mut si = |k: &str, v: i64| {
        m.insert(k.to_string(), Value::int(v));
    };
    // core / platform
    si("PHP_INT_MAX", i64::MAX);
    si("PHP_INT_MIN", i64::MIN);
    si("PHP_INT_SIZE", 8);
    si("PHP_FLOAT_DIG", 15);
    si("PHP_MAJOR_VERSION", 8);
    si("PHP_MINOR_VERSION", 3);
    si("PHP_RELEASE_VERSION", 0);
    si("PHP_VERSION_ID", 80300);
    si("PHP_ROUND_HALF_UP", 1);
    si("PHP_ROUND_HALF_DOWN", 2);
    si("PHP_ROUND_HALF_EVEN", 3);
    si("PHP_ROUND_HALF_ODD", 4);
    // Error levels — the canonical values live in `crate::errlevel`, which the
    // `error_reporting` mask and the `-d error_reporting=…` parser share.
    si("E_ERROR", errlevel::E_ERROR);
    si("E_WARNING", errlevel::E_WARNING);
    si("E_PARSE", errlevel::E_PARSE);
    si("E_NOTICE", errlevel::E_NOTICE);
    si("E_CORE_ERROR", errlevel::E_CORE_ERROR);
    si("E_CORE_WARNING", errlevel::E_CORE_WARNING);
    si("E_COMPILE_ERROR", errlevel::E_COMPILE_ERROR);
    si("E_COMPILE_WARNING", errlevel::E_COMPILE_WARNING);
    si("E_STRICT", errlevel::E_STRICT);
    si("E_RECOVERABLE_ERROR", errlevel::E_RECOVERABLE_ERROR);
    si("E_DEPRECATED", errlevel::E_DEPRECATED);
    si("E_ALL", errlevel::E_ALL);
    si("E_USER_ERROR", errlevel::E_USER_ERROR);
    si("E_USER_WARNING", errlevel::E_USER_WARNING);
    si("E_USER_NOTICE", errlevel::E_USER_NOTICE);
    si("E_USER_DEPRECATED", errlevel::E_USER_DEPRECATED);
    // sort / count / str_pad / array_filter
    si("SORT_REGULAR", 0);
    si("SORT_NUMERIC", 1);
    si("SORT_STRING", 2);
    si("SORT_DESC", 3);
    si("SORT_ASC", 4);
    si("SORT_LOCALE_STRING", 5);
    si("SORT_NATURAL", 6);
    si("SORT_FLAG_CASE", 8);
    si("COUNT_NORMAL", 0);
    si("COUNT_RECURSIVE", 1);
    si("STR_PAD_RIGHT", 1);
    si("STR_PAD_LEFT", 0);
    si("STR_PAD_BOTH", 2);
    si("ARRAY_FILTER_USE_KEY", 2);
    si("ARRAY_FILTER_USE_BOTH", 1);
    // preg
    si("PREG_PATTERN_ORDER", 1);
    si("PREG_SET_ORDER", 2);
    si("PREG_OFFSET_CAPTURE", 256);
    si("PREG_UNMATCHED_AS_NULL", 512);
    si("PREG_SPLIT_NO_EMPTY", 1);
    si("PREG_SPLIT_DELIM_CAPTURE", 2);
    si("PREG_SPLIT_OFFSET_CAPTURE", 4);
    si("PREG_GREP_INVERT", 1);
    // The `preg_last_error()` codes, whose names are what user code compares
    // against rather than the bare numbers.
    si("PREG_NO_ERROR", 0);
    si("PREG_INTERNAL_ERROR", 1);
    si("PREG_BACKTRACK_LIMIT_ERROR", 2);
    si("PREG_RECURSION_LIMIT_ERROR", 3);
    si("PREG_BAD_UTF8_ERROR", 4);
    si("PREG_BAD_UTF8_OFFSET_ERROR", 5);
    si("PREG_JIT_STACKLIMIT_ERROR", 6);
    // json
    si("JSON_HEX_TAG", 1);
    si("JSON_HEX_AMP", 2);
    si("JSON_HEX_APOS", 4);
    si("JSON_HEX_QUOT", 8);
    si("JSON_FORCE_OBJECT", 16);
    si("JSON_NUMERIC_CHECK", 32);
    si("JSON_UNESCAPED_SLASHES", 64);
    si("JSON_PRETTY_PRINT", 128);
    si("JSON_UNESCAPED_UNICODE", 256);
    si("JSON_THROW_ON_ERROR", 4194304);
    si("JSON_OBJECT_AS_ARRAY", 1);
    si("JSON_BIGINT_AS_STRING", 2);
    si("JSON_ERROR_NONE", 0);
    si("JSON_ERROR_DEPTH", 1);
    si("JSON_ERROR_STATE_MISMATCH", 2);
    si("JSON_ERROR_CTRL_CHAR", 3);
    si("JSON_ERROR_SYNTAX", 4);
    si("JSON_ERROR_UTF8", 5);
    // filter
    si("INPUT_GET", 1);
    si("INPUT_POST", 0);
    si("FILTER_DEFAULT", 516);
    si("FILTER_UNSAFE_RAW", 516);
    si("FILTER_VALIDATE_INT", 257);
    si("FILTER_VALIDATE_BOOLEAN", 258);
    si("FILTER_VALIDATE_BOOL", 258);
    si("FILTER_VALIDATE_FLOAT", 259);
    si("FILTER_VALIDATE_REGEXP", 272);
    si("FILTER_VALIDATE_DOMAIN", 277);
    si("FILTER_VALIDATE_URL", 273);
    si("FILTER_VALIDATE_EMAIL", 274);
    si("FILTER_VALIDATE_IP", 275);
    si("FILTER_VALIDATE_MAC", 276);
    si("FILTER_SANITIZE_STRING", 513);
    si("FILTER_SANITIZE_STRIPPED", 513);
    si("FILTER_SANITIZE_ENCODED", 514);
    si("FILTER_SANITIZE_SPECIAL_CHARS", 515);
    si("FILTER_SANITIZE_FULL_SPECIAL_CHARS", 522);
    si("FILTER_SANITIZE_EMAIL", 517);
    si("FILTER_SANITIZE_URL", 518);
    si("FILTER_SANITIZE_NUMBER_INT", 519);
    si("FILTER_SANITIZE_NUMBER_FLOAT", 520);
    si("FILTER_SANITIZE_ADD_SLASHES", 523);
    si("FILTER_FLAG_ALLOW_OCTAL", 1);
    si("FILTER_FLAG_ALLOW_HEX", 2);
    si("FILTER_FLAG_STRIP_LOW", 4);
    si("FILTER_FLAG_STRIP_HIGH", 8);
    si("FILTER_FLAG_ALLOW_FRACTION", 4096);
    si("FILTER_FLAG_ALLOW_THOUSAND", 8192);
    si("FILTER_FLAG_ALLOW_SCIENTIFIC", 16384);
    si("FILTER_FLAG_IPV4", 1048576);
    si("FILTER_FLAG_IPV6", 2097152);
    si("FILTER_FLAG_HOSTNAME", 1048576);
    si("FILTER_NULL_ON_FAILURE", 134217728);
    si("FILTER_REQUIRE_SCALAR", 33554432);
    si("FILTER_REQUIRE_ARRAY", 16777216);
    si("FILTER_FORCE_ARRAY", 67108864);
    // mbstring
    si("MB_CASE_UPPER", 0);
    si("MB_CASE_LOWER", 1);
    si("MB_CASE_TITLE", 2);
    // file / dir
    si("FILE_USE_INCLUDE_PATH", 1);
    si("FILE_APPEND", 8);
    si("FILE_IGNORE_NEW_LINES", 2);
    si("FILE_SKIP_EMPTY_LINES", 4);
    si("FILE_NO_DEFAULT_CONTEXT", 16);
    si("LOCK_SH", 1);
    si("LOCK_EX", 2);
    si("LOCK_UN", 3);
    si("SCANDIR_SORT_ASCENDING", 0);
    si("SCANDIR_SORT_DESCENDING", 1);
    si("SCANDIR_SORT_NONE", 2);
    si("PATHINFO_DIRNAME", 1);
    si("PATHINFO_BASENAME", 2);
    si("PATHINFO_EXTENSION", 4);
    si("PATHINFO_FILENAME", 8);
    si("PATHINFO_ALL", 15);
    // html entities
    si("ENT_NOQUOTES", 0);
    si("ENT_COMPAT", 2);
    si("ENT_QUOTES", 3);
    si("ENT_HTML401", 0);
    si("ENT_HTML5", 48);
    si("ENT_XML1", 16);
    si("ENT_XHTML", 32);
    si("ENT_SUBSTITUTE", 8);
    si("ENT_IGNORE", 4);
    // string constants
    let mut ss = |k: &str, v: &str| {
        m.insert(k.to_string(), Value::str(v.to_string()));
    };
    ss("PHP_EOL", "\n");
    ss("PHP_VERSION", "8.3.0");
    ss(
        "PHP_OS",
        if cfg!(target_os = "macos") {
            "Darwin"
        } else if cfg!(target_os = "windows") {
            "WINNT"
        } else {
            "Linux"
        },
    );
    ss(
        "PHP_OS_FAMILY",
        if cfg!(target_os = "windows") {
            "Windows"
        } else if cfg!(target_os = "macos") {
            "Darwin"
        } else {
            "Linux"
        },
    );
    ss(
        "DIRECTORY_SEPARATOR",
        if cfg!(windows) { "\\" } else { "/" },
    );
    ss("PATH_SEPARATOR", if cfg!(windows) { ";" } else { ":" });
    // math constants
    let mut sf = |k: &str, v: f64| {
        m.insert(k.to_string(), Value::float(v));
    };
    sf("M_PI", std::f64::consts::PI);
    sf("M_E", std::f64::consts::E);
    sf("M_SQRT2", std::f64::consts::SQRT_2);
    sf("M_SQRT1_2", std::f64::consts::FRAC_1_SQRT_2);
    sf("M_SQRT3", 1.7320508075688772);
    sf("M_2_SQRTPI", std::f64::consts::FRAC_2_SQRT_PI);
    sf("M_PI_2", std::f64::consts::FRAC_PI_2);
    sf("M_PI_4", std::f64::consts::FRAC_PI_4);
    sf("M_1_PI", std::f64::consts::FRAC_1_PI);
    sf("M_2_PI", std::f64::consts::FRAC_2_PI);
    sf("M_LN2", std::f64::consts::LN_2);
    sf("M_LN10", std::f64::consts::LN_10);
    sf("M_LOG2E", std::f64::consts::LOG2_E);
    sf("M_LOG10E", std::f64::consts::LOG10_E);
    sf("M_EULER", 0.5772156649015329);
    sf("M_SQRTPI", 1.7724538509055159);
    sf("PHP_FLOAT_EPSILON", f64::EPSILON);
    sf("PHP_FLOAT_MAX", f64::MAX);
    sf("PHP_FLOAT_MIN", f64::MIN_POSITIVE);
    sf("INF", f64::INFINITY);
    sf("NAN", f64::NAN);
    m
}

// ── thread-local host access ──────────────────────────────────────────────

thread_local! {
    static HOST: RefCell<PhpHost> = RefCell::new(PhpHost::new());
}

/// Run `f` with mutable access to the current thread's `PhpHost`.
pub fn with_host<R>(f: impl FnOnce(&mut PhpHost) -> R) -> R {
    HOST.with(|h| f(&mut h.borrow_mut()))
}

/// Reset the host to a fresh state (new heap, new global scope).
pub fn reset_host() {
    CUR_GEN.with(|c| c.set(None));
    HOST.with(|h| *h.borrow_mut() = PhpHost::new());
}

// ── generators (host-side stackful coroutines) ──────────────────────────────
//
// A generator function's body runs on its own native stack inside a
// `corosensei::Coroutine`. `run_chunk_on` (the fusevm VM run loop) executes *on
// that stack*, so a `yield` deep inside the body suspends the whole VM back to
// the resumer with one stack switch — no fusevm change. Same design the sibling
// node-js frontend uses. Single-threaded: the coroutine shares the thread-local
// `HOST`, and no `with_host` borrow is ever held across a `resume`/`suspend`.
//
// The coroutine yields `()` (the current key/value are stashed in the `GenCell`),
// is resumed with a `Value` (the `->send($x)` argument), and returns
// `Result<Value, String>` (the generator body's `return` value, or an error).
// corosensei's generics are `Coroutine<Input, Yield, Return>`: the body is resumed
// with `Input` (the `->send($x)` value), yields `Yield` (here `()` — the current
// key/value ride in the `GenCell`), and finally returns `Return`.
type GenCoro = corosensei::Coroutine<Value, (), Result<Value, String>>;
type GenYielder = corosensei::Yielder<Value, ()>;

thread_local! {
    /// Id of the generator whose body is currently executing (the one a `yield`
    /// suspends), or `None` at the root / inside a non-generator call.
    static CUR_GEN: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
}

/// A forced completion injected at a suspended `yield` by `->throw()` (there is no
/// `Generator::return` in PHP, so only the throw case is needed).
enum GenInject {
    Throw(Value),
}

/// The volatile execution state swapped in/out at every generator resume/suspend
/// boundary, so a suspended generator's half-finished call frame and in-flight
/// signal/throw never leak into the resumer (and vice versa). The global scope
/// (`scopes[0]`) is *not* part of this — it stays shared, so superglobals and
/// `global $x` work identically inside a generator.
#[derive(Default)]
struct GenContext {
    /// The generator's own call frames (everything above the global scope).
    frames: Vec<Scope>,
    signal: Option<Signal>,
    pending_throw: Option<Value>,
    error: Option<String>,
}

/// One live generator.
struct GenCell {
    /// The suspended coroutine. `None` only while it is actively running (taken
    /// out across `resume` so the body can re-borrow the host freely).
    coro: Option<GenCoro>,
    /// Raw pointer to the coroutine body's `Yielder`, published on entry. Valid
    /// for the whole body lifetime; only read from inside that body (its stack is
    /// live), on the same thread.
    yielder: *const GenYielder,
    ctx: GenContext,
    /// The most recently yielded key and value (read by `current()`/`key()`).
    cur_key: Value,
    cur_val: Value,
    /// The next auto-increment integer key (PHP numbers un-keyed yields 0,1,2…).
    auto_key: i64,
    /// The body's `return` value, once it has finished (`getReturn`).
    ret: Value,
    started: bool,
    done: bool,
    inject: Option<GenInject>,
}

impl PhpHost {
    pub fn is_generator_val(&self, v: &Value) -> bool {
        matches!(self.as_array(v), Some(PhpObj::Generator { .. }))
    }
    fn gen_id(&self, v: &Value) -> Option<u32> {
        match self.as_array(v) {
            Some(PhpObj::Generator { id }) => Some(*id),
            _ => None,
        }
    }
    /// Swap the volatile execution context in one shot, returning the previous one:
    /// installs a generator's frames/signal/throw on resume, pulls them back on
    /// suspend. The global scope (index 0) is left untouched, so superglobals and
    /// `global $x` stay shared.
    fn install_gen_ctx(&mut self, mut c: GenContext) -> GenContext {
        // Detach the caller's frames (everything above the global scope) and
        // install the generator's own frames in their place.
        let caller_frames = self.scopes.split_off(1);
        self.scopes.append(&mut c.frames); // drains c.frames into the active stack
        std::mem::swap(&mut self.signal, &mut c.signal);
        std::mem::swap(&mut self.pending_throw, &mut c.pending_throw);
        std::mem::swap(&mut self.error, &mut c.error);
        // `c` now carries the caller's saved signal/throw/error; hand it the
        // caller's frames so the reverse call restores them.
        c.frames = caller_frames;
        c
    }
}

/// Build a suspended generator over `body`, run in the already-bound call frame
/// `frame` (its parameters/captures set by the caller). Nothing runs until the
/// first resume.
fn make_generator(body: Chunk, frame: Scope) -> Value {
    let id = with_host(|h| {
        let id = h.generators.len() as u32;
        h.generators.push(GenCell {
            coro: None,
            yielder: std::ptr::null(),
            ctx: GenContext {
                frames: vec![frame],
                ..GenContext::default()
            },
            cur_key: Value::Undef,
            cur_val: Value::Undef,
            auto_key: 0,
            ret: Value::Undef,
            started: false,
            done: false,
            inject: None,
        });
        id
    });
    let coro = GenCoro::new(move |yielder: &GenYielder, _first: Value| {
        // Same thread → publish the yielder so `yield` (deep in the body's VM) can
        // reach it. Valid for the whole body.
        with_host(|h| h.generators[id as usize].yielder = yielder as *const _);
        let r = run_chunk_on(body);
        // A `return v;` in the body leaves a Return signal; capture it as the
        // generator's completion value (`getReturn`).
        let ret = with_host(|h| match h.signal.take() {
            Some(Signal::Return(v)) => v,
            _ => Value::Undef,
        });
        r.map(|_| ret)
    });
    with_host(|h| h.generators[id as usize].coro = Some(coro));
    with_host(|h| {
        h.objs.push(PhpObj::Generator { id });
        Value::Obj((h.objs.len() - 1) as u32)
    })
}

/// How a `yield` computes its key.
enum YieldKey {
    /// `yield $v` — take (and advance) the next auto-increment integer key.
    Auto,
    /// `yield $k => $v` — an explicit key (an int at/above the counter advances it,
    /// matching array-append semantics).
    Explicit(Value),
    /// `yield from` passthrough — re-emit the delegate's key without touching the
    /// outer generator's auto-key counter.
    Passthrough(Value),
}

/// Suspend the running generator, publishing `(key, val)` to the resumer. Returns
/// the value the next `->send($x)` supplies (`Undef` for `->next()`), or unwinds a
/// `->throw()` injected at this point.
fn gen_yield(key: YieldKey, val: Value) -> Result<Value, String> {
    let id = match CUR_GEN.with(|c| c.get()) {
        Some(id) => id,
        None => return Err("cannot yield outside a generator".to_string()),
    };
    let yp = with_host(|h| {
        let g = &mut h.generators[id as usize];
        let k = match key {
            YieldKey::Auto => {
                let k = g.auto_key;
                g.auto_key += 1;
                Value::int(k)
            }
            YieldKey::Explicit(Value::Int(i)) => {
                if i >= g.auto_key {
                    g.auto_key = i + 1;
                }
                Value::int(i)
            }
            YieldKey::Explicit(other) => other,
            YieldKey::Passthrough(k) => k,
        };
        g.cur_key = k;
        g.cur_val = val;
        g.yielder
    });
    // SAFETY: same-thread coroutine; the yielder lives for the whole body and we
    // only reach here from inside that live body.
    let yielder = unsafe { &*yp };
    let sent = yielder.suspend(());
    if let Some(GenInject::Throw(e)) = with_host(|h| h.generators[id as usize].inject.take()) {
        // Re-raise the injected exception at the suspension point so an enclosing
        // try/catch in the body handles it (or it unwinds the body).
        set_pending_throw(e);
        return Err("__generator_throw__".to_string());
    }
    Ok(sent)
}

/// `yield $v` — suspend with an auto-incremented integer key.
pub fn yield_value(val: Value) -> Result<Value, String> {
    gen_yield(YieldKey::Auto, val)
}

/// `yield $k => $v` — suspend with an explicit key.
pub fn yield_kv(key: Value, val: Value) -> Result<Value, String> {
    gen_yield(YieldKey::Explicit(key), val)
}

/// `yield from $src` — the public entry point (see `gen_yield_from`).
pub fn yield_from(src: Value) -> Result<Value, String> {
    gen_yield_from(src)
}

/// `yield from $src` — delegate to an array, a Generator, or any Traversable,
/// re-yielding each key/value from the enclosing generator. Evaluates to the
/// delegate's `return` value (null for an array or a returnless generator).
fn gen_yield_from(src: Value) -> Result<Value, String> {
    // A sub-generator is driven lazily (preserving side-effect order and passing
    // sent values through). Anything else is normalized to an array and replayed.
    if with_host(|h| h.is_generator_val(&src)) {
        gen_rewind(&src)?;
        loop {
            if !gen_valid(&src)? {
                break;
            }
            let k = with_host(|h| {
                h.generators[h.gen_id(&src).unwrap() as usize]
                    .cur_key
                    .clone()
            });
            let v = with_host(|h| {
                h.generators[h.gen_id(&src).unwrap() as usize]
                    .cur_val
                    .clone()
            });
            let sent = gen_yield(YieldKey::Passthrough(k), v)?;
            gen_send_raw(&src, sent)?;
        }
        return Ok(with_host(|h| {
            h.generators[h.gen_id(&src).unwrap() as usize].ret.clone()
        }));
    }
    let arr = foreach_prep(src)?;
    let pairs: Vec<(Value, Value)> = with_host(|h| match h.as_array(&arr) {
        Some(PhpObj::Array { entries, .. }) => entries
            .iter()
            .map(|(k, v)| (k.to_value(), v.clone()))
            .collect(),
        _ => Vec::new(),
    });
    for (k, v) in pairs {
        gen_yield(YieldKey::Passthrough(k), v)?;
    }
    Ok(Value::Undef)
}

/// Resume a generator until its next `yield` or its body returns. The coroutine is
/// taken out (so the body re-enters `with_host` freely) and the volatile context
/// is swapped so the caller's frames/signal survive the switch.
fn gen_resume(id: u32, send: Value) -> Result<(), String> {
    if with_host(|h| h.generators[id as usize].done) {
        return Ok(());
    }
    let mut coro = match with_host(|h| h.generators[id as usize].coro.take()) {
        Some(c) => c,
        None => return Err("cannot resume an already-running generator".to_string()),
    };
    with_host(|h| h.generators[id as usize].started = true);
    let gen_ctx = with_host(|h| std::mem::take(&mut h.generators[id as usize].ctx));
    let caller_ctx = with_host(|h| h.install_gen_ctx(gen_ctx));
    let prev = CUR_GEN.with(|c| c.replace(Some(id)));

    let out = coro.resume(send); // no host borrow held; body drives its own VM

    CUR_GEN.with(|c| c.set(prev));
    // An uncaught exception in the body leaves a pending throw in the generator's
    // (currently installed) context. Take it out before the context is swapped
    // away so it can be re-raised in the resumer's context below.
    let body_throw = with_host(|h| h.pending_throw.take());
    let gen_ctx = with_host(|h| h.install_gen_ctx(caller_ctx));
    with_host(|h| {
        h.generators[id as usize].ctx = gen_ctx;
        h.generators[id as usize].coro = Some(coro);
    });

    let result = match out {
        corosensei::CoroutineResult::Yield(()) => Ok(()),
        corosensei::CoroutineResult::Return(r) => {
            with_host(|h| {
                let g = &mut h.generators[id as usize];
                g.done = true;
                g.cur_key = Value::Undef;
                g.cur_val = Value::Undef;
            });
            match r {
                Ok(v) => {
                    with_host(|h| h.generators[id as usize].ret = v);
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
    };
    // Re-raise an uncaught body exception in the resumer's context.
    if let Some(e) = body_throw {
        set_pending_throw(e);
        return Err("__generator_throw__".to_string());
    }
    result
}

/// Prime an unstarted generator to its first `yield` (PHP `rewind`/implicit on the
/// first `current`/`valid`/`foreach`). A no-op once started.
pub fn gen_rewind(gen: &Value) -> Result<(), String> {
    let id = with_host(|h| h.gen_id(gen)).ok_or("not a generator")?;
    if !with_host(|h| h.generators[id as usize].started) {
        gen_resume(id, Value::Undef)?;
    }
    Ok(())
}

/// `->valid()` — whether the generator has not yet finished (priming it first).
pub fn gen_valid(gen: &Value) -> Result<bool, String> {
    gen_rewind(gen)?;
    let id = with_host(|h| h.gen_id(gen)).ok_or("not a generator")?;
    Ok(with_host(|h| !h.generators[id as usize].done))
}

/// `->current()` — the current yielded value (priming first). Null once finished.
pub fn gen_current(gen: &Value) -> Result<Value, String> {
    gen_rewind(gen)?;
    let id = with_host(|h| h.gen_id(gen)).ok_or("not a generator")?;
    Ok(with_host(|h| h.generators[id as usize].cur_val.clone()))
}

/// `->key()` — the current yielded key (priming first). Null once finished.
pub fn gen_key(gen: &Value) -> Result<Value, String> {
    gen_rewind(gen)?;
    let id = with_host(|h| h.gen_id(gen)).ok_or("not a generator")?;
    Ok(with_host(|h| h.generators[id as usize].cur_key.clone()))
}

/// `->next()` — advance to the next yield (priming first, then resuming with null).
pub fn gen_next(gen: &Value) -> Result<Value, String> {
    gen_rewind(gen)?;
    let id = with_host(|h| h.gen_id(gen)).ok_or("not a generator")?;
    if !with_host(|h| h.generators[id as usize].done) {
        gen_resume(id, Value::Undef)?;
    }
    Ok(Value::Undef)
}

/// Resume `gen` with `sent` without the unstarted auto-prime — the internal step
/// used to pass a sent value down through `yield from`.
fn gen_send_raw(gen: &Value, sent: Value) -> Result<(), String> {
    let id = with_host(|h| h.gen_id(gen)).ok_or("not a generator")?;
    if !with_host(|h| h.generators[id as usize].done) {
        gen_resume(id, sent)?;
    }
    Ok(())
}

/// `->send($v)` — resume the generator, making the paused `yield` evaluate to `$v`,
/// and return the next yielded value (null once finished). On an unstarted
/// generator PHP first primes to the first yield, then sends.
pub fn gen_send(gen: &Value, sent: Value) -> Result<Value, String> {
    let id = with_host(|h| h.gen_id(gen)).ok_or("not a generator")?;
    if !with_host(|h| h.generators[id as usize].started) {
        gen_resume(id, Value::Undef)?;
    }
    if !with_host(|h| h.generators[id as usize].done) {
        gen_resume(id, sent)?;
    }
    Ok(with_host(|h| h.generators[id as usize].cur_val.clone()))
}

/// `->throw($e)` — raise `$e` at the current suspension point (running an enclosing
/// try/catch/finally in the body), then return the next yielded value.
pub fn gen_throw(gen: &Value, e: Value) -> Result<Value, String> {
    let id = with_host(|h| h.gen_id(gen)).ok_or("not a generator")?;
    gen_rewind(gen)?;
    if with_host(|h| h.generators[id as usize].done) {
        // Throw into a finished generator: it propagates straight to the caller.
        set_pending_throw(e);
        return Ok(Value::Undef);
    }
    with_host(|h| h.generators[id as usize].inject = Some(GenInject::Throw(e)));
    gen_resume(id, Value::Undef)?;
    Ok(with_host(|h| h.generators[id as usize].cur_val.clone()))
}

/// `->getReturn()` — the value the body `return`ed (null if none / not finished).
pub fn gen_get_return(gen: &Value) -> Result<Value, String> {
    let id = with_host(|h| h.gen_id(gen)).ok_or("not a generator")?;
    Ok(with_host(|h| h.generators[id as usize].ret.clone()))
}

/// Dispatch a `$gen->method(...)` call on a Generator object.
pub fn call_generator_method(gen: &Value, method: &str, args: Vec<Value>) -> Result<Value, String> {
    let mut args = args;
    match method.to_ascii_lowercase().as_str() {
        "current" => gen_current(gen),
        "key" => gen_key(gen),
        "next" => gen_next(gen),
        "valid" => gen_valid(gen).map(Value::bool),
        "rewind" => gen_rewind(gen).map(|_| Value::Undef),
        "send" => gen_send(gen, args.into_iter().next().unwrap_or(Value::Undef)),
        "throw" => gen_throw(gen, args.into_iter().next().unwrap_or(Value::Undef)),
        "getreturn" => gen_get_return(gen),
        other => {
            let _ = args.pop();
            Err(format!("call to undefined method Generator::{other}()"))
        }
    }
}

// ── execution ─────────────────────────────────────────────────────────────

thread_local! {
    static DEBUG_MODE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Enable/disable DAP debug execution. When on, `run_chunk_on` installs the
/// extension-handler seam and skips the tracing JIT (which would compile hot
/// loops and step over the `DBG_LINE` markers the debugger relies on).
pub fn set_debug_mode(on: bool) {
    DEBUG_MODE.with(|d| d.set(on));
}

/// Register every phplang builtin + the strict numeric hook on a VM, then run it.
fn run_chunk_on(chunk: Chunk) -> Result<Value, String> {
    // The hook warns (`A non-numeric value encountered`), so it needs the line
    // of the operator that delegated to it. Native arithmetic ops run no
    // builtin, so nothing else sets the warn site — hand the hook this chunk's
    // line table and let it index the op position the VM reports. Capturing it
    // per VM rather than in a thread-local keeps it correct when a generator
    // suspends mid-chunk, and the clone is dwarfed by the `Chunk` clone every
    // call already pays.
    let lines: std::sync::Arc<[u32]> = chunk.lines.as_slice().into();
    let mut vm = VM::new(chunk);
    crate::builtins::install(&mut vm);
    vm.set_sited_numeric_hook(std::sync::Arc::new(move |call| {
        crate::builtins::numeric_hook_sited(call, &lines)
    }));
    if DEBUG_MODE.with(|d| d.get()) {
        vm.set_extension_handler(Box::new(|vm, id, _| {
            crate::dap::on_ext(vm, id);
        }));
    } else {
        vm.enable_tracing_jit();
    }
    let outcome = vm.run();
    if let Some(e) = with_host(|h| h.take_error()) {
        return Err(e);
    }
    match outcome {
        VMResult::Ok(v) => Ok(v),
        VMResult::Halted => Ok(vm.stack.last().cloned().unwrap_or(Value::Undef)),
        // A builtin raising a PHP exception halts its chunk cleanly, but the
        // numeric hook has no VM handle to do that with — its only way out is
        // an error return. When it leaves an exception pending, that exception
        // is the real signal, so report the stop as a clean halt: every caller
        // (`run_body`, `run_main`, the call dispatcher) already checks for a
        // pending throw on this path, and only that path routes it to `catch`.
        VMResult::Error(_) if has_pending_throw() => Ok(Value::Undef),
        VMResult::Error(e) => Err(e),
    }
}

/// Run the top-level program chunk.
pub fn run_main(chunk: Chunk) -> Result<Value, String> {
    let r = run_chunk_on(chunk);
    // A top-level `return` just ends the program; clear any leftover signal.
    with_host(|h| h.signal.take());
    // An exception that reached the top uncaught is a fatal error, displayed in
    // the PHP CLI's shape. `write_out` puts it where the reference puts it — on
    // stdout, inside any open output buffer — and the returned string is the
    // stderr log copy, which the CLI wrapper reports without re-displaying.
    if let Some(exc) = with_host(|h| h.pending_throw.take()) {
        let body = with_host(|h| {
            let class = h
                .object_class(&exc)
                .unwrap_or_else(|| "Exception".to_string());
            let msg = h.to_str(&h.prop_get(&exc, "message"));
            let file = h.to_str(&h.prop_get(&exc, "file"));
            let line = h.prop_get(&exc, "line").to_int();
            let trace = h.to_str(&h.prop_get(&exc, "trace"));
            format!(
                "Uncaught {class}: {msg} in {file}:{line}\nStack trace:\n{trace}\n  \
                 thrown in {file} on line {line}"
            )
        });
        with_host(|h| {
            h.fatal("Fatal error", &body);
            h.ob_flush_all();
        });
        return Err(format!("Fatal error:  {body}"));
    }
    with_host(|h| h.ob_flush_all());
    r
}

/// Invoke a user function (or fall through to the builtin library) by name.
/// Pushes a fresh scope, binds positional parameters, runs the body chunk, and
/// returns the `return` value (or null if the body fell off the end).
/// PHP's string conversion with `__toString` honoured.
///
/// [`PhpHost::to_str`] takes `&self` and so cannot run PHP code; an object that
/// defines `__toString` therefore has to be converted *outside* the host borrow,
/// which is what this free function is for. It must not be called from inside a
/// `with_host` closure — invoking the method re-enters the host and the
/// `RefCell` would be borrowed twice.
///
/// Every other value takes the ordinary cast, and the probe for the method is a
/// single class lookup, so the non-object path costs one `with_host` round trip
/// and nothing else.
pub fn to_str_ext(v: &Value) -> String {
    let class = with_host(|h| {
        h.object_class(v)
            .filter(|c| h.class_has_method(c, "__tostring"))
    });
    let Some(class) = class else {
        return with_host(|h| h.to_str(v));
    };
    match call_method(&class, "__toString", Some(v.clone()), Vec::new()) {
        Ok(r) => with_host(|h| h.to_str(&r)),
        // A throwing `__toString` leaves the exception pending for the caller's
        // dispatcher; the conversion itself yields the empty string.
        Err(_) => String::new(),
    }
}

pub fn call_function(name: &str, args: Vec<Value>) -> Result<Value, String> {
    // Inline Rust FFI: the `rust { ... }` desugar emits `__rust_compile(b64,
    // line)`; compile + register the block's exported functions.
    if name == "__rust_compile" {
        let b64 = args.first().map(|v| v.to_str()).unwrap_or_default();
        return fusevm::ffi::compile_and_register(&b64).map(|_| Value::Undef);
    }
    // A `rust { ... }` block's exported functions are callable by bareword.
    // User-defined PHP functions still win (resolved below); the registry is
    // consulted before the PHP standard library so an exported name is
    // reachable, and the membership check keeps this off the hot path.
    let def = with_host(|h| h.functions.get(&name.to_ascii_lowercase()).cloned());
    if def.is_none() && fusevm::ffi::is_registered(name) {
        if let Some(r) = fusevm::ffi::try_call(name, &args) {
            return r;
        }
    }
    if let Some(def) = def {
        return invoke(
            name,
            &def.params,
            def.chunk,
            Vec::new(),
            args,
            Vec::new(),
            def.is_generator,
        );
    }
    call_library_throwing(name, args)
}

/// Call a standard-library function, turning a tagged argument error into the
/// PHP exception it stands for (see [`crate::builtins::throws`]).
///
/// This wrapper is the caller rather than part of `call_library` itself because
/// it needs the argument list AFTER the call, to render the trace frame —
/// `call_library` borrows it, so the owner has to be the one that reacts.
fn call_library_throwing(name: &str, args: Vec<Value>) -> Result<Value, String> {
    match crate::builtins::call_library(name, &args) {
        Err(e) => match crate::builtins::untag_throw(&e) {
            Some((class, message)) => throw_from_internal(name, &args, class, message),
            None => Err(e),
        },
        ok => ok,
    }
}

/// `call_function` with PHP 8.0 named arguments. User functions bind by parameter
/// name; a builtin (no name metadata) falls back to positional binding with the
/// named values appended in call order.
pub fn call_function_named(
    name: &str,
    args: Vec<Value>,
    named: Vec<(String, Value)>,
) -> Result<Value, String> {
    let def = with_host(|h| h.functions.get(&name.to_ascii_lowercase()).cloned());
    if let Some(def) = def {
        return invoke(
            name,
            &def.params,
            def.chunk,
            Vec::new(),
            args,
            named,
            def.is_generator,
        );
    }
    let mut all = args;
    all.extend(named.into_iter().map(|(_, v)| v));
    call_function(name, all)
}

/// Run a user-defined function or closure body: push a call frame named `frame`,
/// pre-bind `pre` (a closure's captured `(name, value)` pairs, empty for a plain
/// function), bind `args` to `params` (variadic collection, then default chunks
/// for omitted parameters), run `body`, and return the `return` value (or null if
/// the body fell off the end). Default chunks run OUTSIDE the binding `with_host`
/// closure because `run_chunk_on` itself borrows the thread-local host.
#[allow(clippy::too_many_arguments)]
fn invoke(
    frame: &str,
    params: &[Param],
    body: Chunk,
    pre: Vec<(String, Value)>,
    args: Vec<Value>,
    named: Vec<(String, Value)>,
    is_generator: bool,
) -> Result<Value, String> {
    // Which parameter positions a positional or named argument (or a null-fill for
    // a no-default omitted param) already bound — so the default pass below runs
    // only the chunks that are actually needed. Computed inside the binding closure.
    let bound = with_host(|h| {
        let scope = Scope {
            name: Some(frame.to_string()),
            static_class: h.lsb_take(),
            ..Scope::default()
        };
        h.scopes.push(scope);
        // Stash the full call argument list (hidden `@args`) for func_get_args /
        // func_num_args / func_get_arg: positional args then named values, in call
        // order.
        let argsarr = h.new_array();
        for a in &args {
            h.arr_push_auto(&argsarr, a.clone());
        }
        for (_, v) in &named {
            h.arr_push_auto(&argsarr, v.clone());
        }
        h.set_var("@args", argsarr);
        // Captured bindings first, then parameters (a parameter of the same name
        // as a capture shadows it, as PHP does).
        for (k, v) in pre {
            // A `use (&$v)` capture arrives as a handle to the enclosing
            // variable's cell: bind the name to that cell rather than storing
            // the handle, and the two are one variable from here on.
            match v {
                Value::Obj(id) => match h.objs.get(id as usize) {
                    Some(PhpObj::Ref { slot }) => {
                        let slot = *slot;
                        h.bind_ref_slot(&k, slot);
                    }
                    _ => h.set_var(&k, v),
                },
                _ => h.set_var(&k, v),
            }
        }
        let mut bound = vec![false; params.len()];
        // Positional binding (a variadic `...$rest` collects the rest positionally).
        let mut ai = 0;
        for (i, p) in params.iter().enumerate() {
            if p.variadic {
                let arr = h.new_array();
                while ai < args.len() {
                    h.arr_push_auto(&arr, args[ai].clone());
                    ai += 1;
                }
                h.set_var(&p.name, arr);
                bound[i] = true;
            } else if ai < args.len() {
                // A by-value parameter takes a copy of an array argument; a
                // by-reference one must see the caller's array itself.
                match (p.by_ref, &args[ai]) {
                    // A caller that supplied an explicit reference cell (a
                    // builtin passing an array element to a callback, say) binds
                    // the parameter to that cell, so scalar writes are visible to
                    // the caller too — not just in-place array mutation.
                    (true, Value::Obj(id))
                        if matches!(h.objs.get(*id as usize), Some(PhpObj::Ref { .. })) =>
                    {
                        let Some(PhpObj::Ref { slot }) = h.objs.get(*id as usize) else {
                            unreachable!("guarded by the match arm above")
                        };
                        let slot = *slot;
                        h.bind_ref_slot(&p.name, slot);
                    }
                    (true, a) => h.set_var(&p.name, a.clone()),
                    (false, a) => {
                        // A by-value parameter never sees a reference cell: read
                        // through it so a builtin that offers an element by
                        // reference still works with a plain `function ($v)`.
                        let a = match a {
                            Value::Obj(id) => match h.objs.get(*id as usize) {
                                Some(PhpObj::Ref { slot }) => {
                                    let slot = *slot;
                                    h.ref_cell_value(slot)
                                }
                                _ => a.clone(),
                            },
                            _ => a.clone(),
                        };
                        let copied = h.copy_on_assign(a);
                        h.set_var(&p.name, copied);
                    }
                }
                bound[i] = true;
                ai += 1;
            }
        }
        // Named binding: match each `name: value` to the parameter of that name; an
        // unmatched name lands in the variadic parameter's array under a string key
        // (as PHP collects extra named args), or is dropped if there is no variadic.
        for (n, v) in &named {
            if let Some(i) = params.iter().position(|p| !p.variadic && p.name == *n) {
                let v = if params[i].by_ref {
                    v.clone()
                } else {
                    h.copy_on_assign(v.clone())
                };
                h.set_var(&params[i].name, v);
                bound[i] = true;
            } else if let Some(vi) = params.iter().position(|p| p.variadic) {
                let arr = h.get_var(&params[vi].name);
                h.arr_set_key(&arr, &Value::str(n.clone()), v.clone());
            }
        }
        // A parameter with neither an argument nor a default reads as null.
        for (i, p) in params.iter().enumerate() {
            if !bound[i] && !p.variadic && p.default.is_none() {
                h.set_var(&p.name, Value::Undef);
                bound[i] = true;
            }
        }
        bound
    });
    // Evaluate defaults for the still-unbound parameters, left to right, so a
    // default may reference an earlier parameter already bound in this frame.
    for (i, p) in params.iter().enumerate() {
        if p.variadic || bound[i] {
            continue;
        }
        if let Some(chunk) = &p.default {
            match run_chunk_on(chunk.clone()) {
                Ok(v) => with_host(|h| h.set_var(&p.name, v)),
                Err(e) => {
                    with_host(|h| {
                        h.scopes.pop();
                    });
                    return Err(e);
                }
            }
        }
    }
    // A generator function does not run its body on call: it hands the fully-bound
    // call frame to a suspended coroutine and returns a Generator handle. The frame
    // is pulled off the active stack and lives in the generator until it finishes.
    if is_generator {
        let frame_scope = with_host(|h| h.scopes.pop().expect("generator call frame"));
        return Ok(make_generator(body, frame_scope));
    }
    let r = run_chunk_on(body);
    let sig = with_host(|h| {
        // Capture by-reference parameters' final values (read from the still-open
        // callee frame) so the caller can write them back after the call.
        if params.iter().any(|p| p.by_ref) {
            h.byref_out = params
                .iter()
                .map(|p| {
                    if p.by_ref {
                        h.get_var(&p.name)
                    } else {
                        Value::Undef
                    }
                })
                .collect();
        }
        h.scopes.pop();
        h.signal.take()
    });
    // A pending exception (set by `throw`, kept in its own field) survives the
    // scope pop and takes precedence — the caller's dispatcher checks
    // `has_pending_throw` and re-halts to keep it bubbling.
    match sig {
        Some(Signal::Return(v)) => Ok(v),
        // A `break`/`continue` that escapes a function body has no loop to
        // target; PHP treats it as falling off the end (null result).
        Some(Signal::Break(_)) | Some(Signal::Continue(_)) | None => r.map(|_| Value::Undef),
    }
}

/// Run a closure body: bind its captures (a bound `$this` overriding any captured
/// one) and set the frame's class scope so private/protected access matches the
/// bound scope, then dispatch (as a generator, if its body yields).
fn invoke_closure(
    cc: ClosureCall,
    args: Vec<Value>,
    named: Vec<(String, Value)>,
) -> Result<Value, String> {
    // Encode the private-access scope in the frame name (`Scope::{closure}`), which
    // `current_class_ctx` reads back for visibility checks.
    let frame = match &cc.scope {
        Some(s) => format!("{s}::{{closure}}"),
        None => "{closure}".to_string(),
    };
    let mut pre = cc.captured;
    if let Some(t) = cc.bound_this {
        pre.retain(|(k, _)| k != "this");
        pre.push(("this".to_string(), t));
    }
    invoke(
        &frame,
        &cc.params,
        cc.chunk,
        pre,
        args,
        named,
        cc.is_generator,
    )
}

/// Invoke a callable *value*: a closure handle runs its captured-plus-bound body
/// in a fresh scope; a string is dispatched by name through `call_function`. Used
/// by `$f(...)` calls and callback builtins (`array_map`).
pub fn call_value(callee: Value, args: Vec<Value>) -> Result<Value, String> {
    if let Some(cc) = with_host(|h| h.closure_of(&callee)) {
        return invoke_closure(cc, args, Vec::new());
    }
    match callee {
        Value::Str(s) => call_function(&s, args),
        _ => Err("value is not callable".to_string()),
    }
}

/// `call_value` with PHP 8.0 named arguments (for `$closure(name: v)` / `$f(...)`).
pub fn call_value_named(
    callee: Value,
    args: Vec<Value>,
    named: Vec<(String, Value)>,
) -> Result<Value, String> {
    if let Some(cc) = with_host(|h| h.closure_of(&callee)) {
        return invoke_closure(cc, args, named);
    }
    match callee {
        Value::Str(s) => call_function_named(&s, args, named),
        _ => Err("value is not callable".to_string()),
    }
}

/// `Closure::bind($fn, $obj, $scope)` / `$fn->bindTo($obj, $scope)` — a copy of the
/// closure with `$this` rebound to `obj` and its private-access scope set. `scope`
/// may be a class-name string, an object (its class is used), or null/"static"
/// (keep the current scope). A null `obj` unbinds `$this` (a static closure).
pub fn closure_bind(closure: &Value, obj: Value, scope: Option<Value>) -> Result<Value, String> {
    let this = matches!(obj, Value::Obj(_)).then_some(obj.clone());
    let scope_class = with_host(|h| resolve_bind_scope(h, closure, &obj, scope));
    match with_host(|h| h.rebind_closure(closure, this, scope_class)) {
        Some(v) => Ok(v),
        None => Err("Closure::bind expects a Closure".to_string()),
    }
}

/// Resolve the `$scope` argument of `bind`/`bindTo` to a class name (or `None` for
/// the global scope). An object argument uses its class; a class-name string is
/// taken verbatim; an omitted scope or the literal `"static"` keeps the closure's
/// *current* scope (PHP's default — it does NOT grant access to the new object's
/// class unless a scope is named explicitly).
fn resolve_bind_scope(
    h: &mut PhpHost,
    closure: &Value,
    _obj: &Value,
    scope: Option<Value>,
) -> Option<String> {
    match scope {
        Some(Value::Obj(_)) => h.object_class(scope.as_ref().unwrap()),
        Some(Value::Str(s)) if !s.eq_ignore_ascii_case("static") => Some(s.to_string()),
        // Omitted / null / "static" → leave the closure's scope unchanged.
        _ => h.closure_scope(closure),
    }
}

/// `$fn->call($obj, ...$args)` — bind `$fn` to `$obj` with the scope set to
/// `$obj`'s class (so it can reach that class's private members), then call it.
pub fn closure_call(closure: &Value, obj: Value, args: Vec<Value>) -> Result<Value, String> {
    // `call` always scopes to the bound object's class — pass the object as the
    // scope so `resolve_bind_scope` derives its class.
    let bound = closure_bind(closure, obj.clone(), Some(obj))?;
    call_value(bound, args)
}

/// Dispatch a method call on a `Closure` object: `bindTo`, `call`, or `__invoke`.
pub fn call_closure_method(
    closure: &Value,
    method: &str,
    args: Vec<Value>,
) -> Result<Value, String> {
    let mut args = args;
    match method.to_ascii_lowercase().as_str() {
        "bindto" => {
            let obj = if args.is_empty() {
                Value::Undef
            } else {
                args.remove(0)
            };
            let scope = if args.is_empty() {
                None
            } else {
                Some(args.remove(0))
            };
            closure_bind(closure, obj, scope)
        }
        "call" => {
            let obj = if args.is_empty() {
                Value::Undef
            } else {
                args.remove(0)
            };
            closure_call(closure, obj, args)
        }
        "__invoke" => call_value(closure.clone(), args),
        other => Err(format!("call to undefined method Closure::{other}()")),
    }
}

// ── objects (new / method dispatch / constants) ─────────────────────────────

/// Record on a freshly allocated Throwable where it was raised.
///
/// PHP fixes `file`, `line` and the backtrace when the exception object is
/// *constructed*, not when it is thrown — `throw $e;` on a later line still
/// reports the `new` site — and that is also the only moment the frames are
/// still on the stack. `getFile`/`getLine`/`getTraceAsString` read the three
/// back, and the uncaught-exception fatal prints all of them.
fn seed_throwable(class: &str, obj: &Value) {
    with_host(|h| {
        let cl = class.to_ascii_lowercase();
        if !h.class_is_a(&cl, "exception") && !h.class_is_a(&cl, "error") {
            return;
        }
        let file = h.script_name().to_string();
        let line = h.cur_frame_line() as i64;
        let trace = h.backtrace();
        h.prop_set(obj, "file", Value::str(file));
        h.prop_set(obj, "line", Value::int(line));
        h.prop_set(obj, "trace", Value::str(trace));
    });
}

/// Zero-based positions of the parameters an internal function declares
/// `#[\SensitiveParameter]`, which a backtrace must never print.
///
/// Only functions that can THROW need an entry, because an internal frame exists
/// only for the duration of [`throw_from_internal`]; a library function that
/// merely returns never reaches a trace. Verified against the reference by
/// throwing out of each and reading back `getTraceAsString`.
fn sensitive_params(func: &str) -> &'static [usize] {
    match func.to_ascii_lowercase().as_str() {
        "hash_hmac" | "hash_hmac_file" => &[2], // $key
        "hash_pbkdf2" => &[1],                  // $password
        _ => &[],
    }
}

/// Throw a PHP exception *from inside a library function*, with the internal
/// call as a real backtrace frame.
///
/// PHP renders such a throw with the library function itself at `#0`:
///
/// ```text
/// Fatal error: Uncaught ValueError: range(): Argument #3 ($step) … in file:1
/// Stack trace:
/// #0 file(1): range(9, 10, 2)
/// #1 {main}
/// ```
///
/// A library function has no PHP call frame — it is Rust — so one is pushed for
/// exactly as long as the exception object takes to construct, which is when PHP
/// snapshots `file`, `line` and the trace ([`seed_throwable`]). The frame carries
/// the arguments as its hidden `@args` so the trace prints them through the same
/// `trace_arg` rendering a user frame uses, and its line is the CALLER's, which
/// is the line PHP reports for a throw out of an internal function.
///
/// Returns `Ok(Undef)`: the exception is recorded as pending, and the `bubbled`
/// helper at every call site halts the chunk the moment it sees one, so the value
/// is never observed. Reporting it as `Err` instead would surface the scaffold's
/// terse `php: …` fatal and lose the catchability.
pub fn throw_from_internal(
    func: &str,
    args: &[Value],
    class: &str,
    message: &str,
) -> Result<Value, String> {
    let sensitive = sensitive_params(func);
    with_host(|h| {
        let line = h.cur_frame_line();
        h.scopes.push(Scope {
            name: Some(func.to_string()),
            line,
            ..Scope::default()
        });
        let argsarr = h.new_array();
        for (i, a) in args.iter().enumerate() {
            if sensitive.contains(&i) {
                // A `#[\SensitiveParameter]` argument never appears in a trace:
                // the engine substitutes a `SensitiveParameterValue` wrapper so a
                // password or key cannot leak into an error log. The wrapper is
                // built as a real object of that class so `trace_arg` renders it
                // `Object(SensitiveParameterValue)` through the ordinary path.
                let obj = h.objs.len() as u32;
                h.objs.push(PhpObj::Object {
                    class: "SensitiveParameterValue".to_string(),
                    props: IndexMap::new(),
                });
                h.arr_push_auto(&argsarr, Value::Obj(obj));
            } else {
                h.arr_push_auto(&argsarr, a.clone());
            }
        }
        h.set_var("@args", argsarr);
    });
    // `new_object` runs PHP (the exception constructor), so it must not be called
    // inside the borrow above.
    let exc = new_object(class, vec![Value::str(message.to_string())]);
    with_host(|h| {
        h.scopes.pop();
    });
    set_pending_throw(exc?);
    Ok(Value::Undef)
}

/// `new Class(args)`: allocate an object seeded with its (inherited) property
/// defaults, then run its constructor. Property-default and constructor code run
/// on fresh VMs, so no host borrow is held across them.
pub fn new_object(class: &str, args: Vec<Value>) -> Result<Value, String> {
    let cl = class.to_ascii_lowercase();
    if let Some(e) = with_host(|h| h.class_instantiation_error(class)) {
        return Err(e);
    }
    let Some(defaults) = with_host(|h| h.class_prop_default_chunks(&cl)) else {
        return Err(format!("class \"{class}\" not found"));
    };
    // Evaluate each property default (a constant expression chunk).
    let mut props: IndexMap<String, Value> = IndexMap::new();
    for (name, chunk) in defaults {
        let v = run_chunk_on(chunk)?;
        props.insert(name, v);
    }
    // Allocate the instance. The class name is stored with its original casing
    // for display (`get_class`, an uncaught-exception fatal); all lookups
    // (`resolve_method`, `catch_matches`, …) lowercase internally.
    let obj = with_host(|h| {
        h.objs.push(PhpObj::Object {
            class: class.to_string(),
            props,
        });
        Value::Obj((h.objs.len() - 1) as u32)
    });
    seed_throwable(class, &obj);
    // Run the constructor if one exists anywhere in the chain.
    if with_host(|h| h.resolve_method(&cl, "__construct").is_some()) {
        // `static::` inside the constructor is the instantiated class in its
        // declared spelling, not the lowercased lookup key.
        with_host(|h| h.lsb_set_for_next_call(class));
        call_method(&cl, "__construct", Some(obj.clone()), args)?;
    }
    Ok(obj)
}

/// `new Class(...)` with PHP 8.0 named constructor arguments.
pub fn new_object_named(
    class: &str,
    args: Vec<Value>,
    named: Vec<(String, Value)>,
) -> Result<Value, String> {
    let cl = class.to_ascii_lowercase();
    if let Some(e) = with_host(|h| h.class_instantiation_error(class)) {
        return Err(e);
    }
    let Some(defaults) = with_host(|h| h.class_prop_default_chunks(&cl)) else {
        return Err(format!("class \"{class}\" not found"));
    };
    let mut props: IndexMap<String, Value> = IndexMap::new();
    for (name, chunk) in defaults {
        let v = run_chunk_on(chunk)?;
        props.insert(name, v);
    }
    let obj = with_host(|h| {
        h.objs.push(PhpObj::Object {
            class: class.to_string(),
            props,
        });
        Value::Obj((h.objs.len() - 1) as u32)
    });
    seed_throwable(class, &obj);
    if with_host(|h| h.resolve_method(&cl, "__construct").is_some()) {
        with_host(|h| h.lsb_set_for_next_call(class));
        call_method_named(&cl, "__construct", Some(obj.clone()), args, named)?;
    }
    Ok(obj)
}

/// Invoke `Class::method` (resolved up the parent chain) with `this` bound to
/// `$this` when present. Reuses the shared `invoke` frame handling, so methods
/// honor default and variadic parameters.
pub fn call_method(
    class: &str,
    method: &str,
    this: Option<Value>,
    args: Vec<Value>,
) -> Result<Value, String> {
    let method_l = method.to_ascii_lowercase();
    // `Closure::bind($fn, $obj, $scope)` / `Closure::fromCallable($c)` — the only
    // static `Closure` methods, synthesized here (there is no `Closure` class).
    if class.eq_ignore_ascii_case("Closure") {
        return call_closure_static(&method_l, args);
    }
    // Enum static helpers `cases()`/`from()`/`tryFrom()` are synthesized, not
    // declared, so they are handled before the ordinary method resolution.
    if with_host(|h| h.is_enum_class(class)) {
        match method_l.as_str() {
            "cases" => return enum_cases_all(class),
            "from" => {
                return enum_from(
                    class,
                    args.into_iter().next().unwrap_or(Value::Undef),
                    false,
                )
            }
            "tryfrom" => {
                return enum_from(class, args.into_iter().next().unwrap_or(Value::Undef), true)
            }
            _ => {}
        }
    }
    let Some((def_class, def)) = with_host(|h| h.resolve_method(class, &method_l)) else {
        return Err(format!("call to undefined method {class}::{method}()"));
    };
    let pre = match this {
        Some(t) => vec![("this".to_string(), t)],
        None => Vec::new(),
    };
    // `static::` inside the body resolves to the class the call NAMED, which is
    // not `def_class` — the method may have been inherited from an ancestor. A
    // forwarding call (`self::`, `parent::`, `static::`) has already claimed the
    // slot with the caller's own, and keeps it.
    with_host(|h| h.lsb_set_for_next_call(class));
    invoke(
        &format!("{def_class}::{method}"),
        &def.params,
        def.chunk,
        pre,
        args,
        Vec::new(),
        def.is_generator,
    )
}

/// Static `Closure::` helpers: `bind($fn, $obj, $scope)` and
/// `fromCallable($callable)`.
fn call_closure_static(method_l: &str, mut args: Vec<Value>) -> Result<Value, String> {
    match method_l {
        "bind" => {
            let closure = if args.is_empty() {
                Value::Undef
            } else {
                args.remove(0)
            };
            let obj = if args.is_empty() {
                Value::Undef
            } else {
                args.remove(0)
            };
            let scope = if args.is_empty() {
                None
            } else {
                Some(args.remove(0))
            };
            closure_bind(&closure, obj, scope)
        }
        "fromcallable" => {
            // A closure passes through; a callable string wraps to a closure-like
            // handle. The scaffold returns the argument unchanged (a string is
            // already dispatchable through `call_value`).
            Ok(args.into_iter().next().unwrap_or(Value::Undef))
        }
        other => Err(format!("call to undefined method Closure::{other}()")),
    }
}

/// `call_method` with PHP 8.0 named arguments.
pub fn call_method_named(
    class: &str,
    method: &str,
    this: Option<Value>,
    args: Vec<Value>,
    named: Vec<(String, Value)>,
) -> Result<Value, String> {
    let method_l = method.to_ascii_lowercase();
    let Some((def_class, def)) = with_host(|h| h.resolve_method(class, &method_l)) else {
        return Err(format!("call to undefined method {class}::{method}()"));
    };
    let pre = match this {
        Some(t) => vec![("this".to_string(), t)],
        None => Vec::new(),
    };
    // `static::` inside the body resolves to the class the call NAMED, which is
    // not `def_class` — the method may have been inherited from an ancestor. A
    // forwarding call (`self::`, `parent::`, `static::`) has already claimed the
    // slot with the caller's own, and keeps it.
    with_host(|h| h.lsb_set_for_next_call(class));
    invoke(
        &format!("{def_class}::{method}"),
        &def.params,
        def.chunk,
        pre,
        args,
        named,
        def.is_generator,
    )
}

/// `Class::CONST` — evaluate the (inherited) constant initializer. On an `enum`, a
/// name that is not a real constant is resolved as an enum case singleton.
pub fn class_const(class: &str, name: &str) -> Result<Value, String> {
    if let Some(chunk) = with_host(|h| h.resolve_const_chunk(class, name)) {
        return run_chunk_on(chunk);
    }
    if let Some(r) = enum_case(class, name) {
        return r;
    }
    Err(format!("undefined constant {class}::{name}"))
}

/// Resolve `Enum::Case` to its singleton instance (built once, then cached so
/// `Enum::Case === Enum::Case` holds by object identity). `None` when `class` is
/// not an enum or has no case of that name — the caller then falls through to the
/// normal class-constant lookup / error.
pub fn enum_case(class: &str, case: &str) -> Option<Result<Value, String>> {
    let lower = class.to_ascii_lowercase();
    if !with_host(|h| h.is_enum_class(&lower)) {
        return None;
    }
    let chunk_opt = with_host(|h| h.enum_case_chunk(&lower, case))?;
    let key = format!("{lower}::{case}");
    if let Some(v) = with_host(|h| h.enum_case_cached(&key)) {
        return Some(Ok(v));
    }
    let value = match chunk_opt {
        Some(chunk) => match run_chunk_on(chunk) {
            Ok(v) => Some(v),
            Err(e) => return Some(Err(e)),
        },
        None => None,
    };
    let obj = with_host(|h| h.new_enum_object(class, case, value));
    with_host(|h| h.enum_case_store(&key, obj.clone()));
    Some(Ok(obj))
}

/// `Enum::cases()` — an array of every case singleton, in declaration order.
pub fn enum_cases_all(class: &str) -> Result<Value, String> {
    let names = with_host(|h| h.enum_case_names(class));
    let arr = with_host(|h| h.new_array());
    for name in names {
        match enum_case(class, &name) {
            Some(Ok(v)) => with_host(|h| h.arr_push_auto(&arr, v)),
            Some(Err(e)) => return Err(e),
            None => {}
        }
    }
    Ok(arr)
}

/// `Enum::from(value)` / `Enum::tryFrom(value)` — the case whose backing `value`
/// matches. `tryFrom` yields null on no match; `from` is a hard error (PHP throws
/// a catchable `ValueError`; the scaffold surfaces it as a fatal).
pub fn enum_from(class: &str, needle: Value, is_try: bool) -> Result<Value, String> {
    let names = with_host(|h| h.enum_case_names(class));
    let needle_s = with_host(|h| h.to_str(&needle));
    for name in names {
        let case = match enum_case(class, &name) {
            Some(Ok(v)) => v,
            Some(Err(e)) => return Err(e),
            None => continue,
        };
        let val = with_host(|h| h.prop_get(&case, "value"));
        if with_host(|h| h.to_str(&val)) == needle_s {
            return Ok(case);
        }
    }
    if is_try {
        Ok(Value::Undef)
    } else {
        Err(format!(
            "\"{needle_s}\" is not a valid backing value for enum {class}"
        ))
    }
}

/// `Class::$prop` read. On first access the (constant) initializer runs once and
/// is stored in the per-class static cell; subsequent reads return the cell.
pub fn static_prop_get(class: &str, name: &str) -> Result<Value, String> {
    let Some((key, chunk)) = with_host(|h| h.resolve_static_key(class, name)) else {
        return Err(format!(
            "access to undeclared static property {class}::${name}"
        ));
    };
    if let Some(v) = with_host(|h| h.get_static_stored(&key)) {
        return Ok(v);
    }
    let v = run_chunk_on(chunk)?;
    with_host(|h| h.set_static_stored(&key, v.clone()));
    Ok(v)
}

/// `Class::$prop = val` — write the per-class static cell, initializing lazily.
pub fn static_prop_set(class: &str, name: &str, val: Value) -> Result<Value, String> {
    let Some((key, _)) = with_host(|h| h.resolve_static_key(class, name)) else {
        return Err(format!(
            "access to undeclared static property {class}::${name}"
        ));
    };
    with_host(|h| h.set_static_stored(&key, val.clone()));
    Ok(val)
}

/// Normalize a `foreach` subject to an iterable array. Arrays pass through; an
/// object is iterated eagerly into a `(key, value)` array: `IteratorAggregate`
/// via `getIterator()`, the `Iterator` protocol (`rewind`/`valid`/`current`/
/// `key`/`next`), or — as a fallthrough — its public properties. Eager
/// materialization is fine for the finite iterators phplang supports; an infinite
/// iterator would not terminate (documented). A non-iterable yields an empty array.
pub fn foreach_prep(v: Value) -> Result<Value, String> {
    if with_host(|h| h.is_array(&v)) {
        return Ok(v);
    }
    let Some(class) = with_host(|h| h.object_class(&v)) else {
        return Ok(with_host(|h| h.new_array()));
    };
    // IteratorAggregate: `getIterator()` returns the real iterator (or a backing
    // array, for the SPL preludes) — recurse on it.
    if with_host(|h| h.class_has_method(&class, "getIterator")) {
        let it = call_method(&class, "getIterator", Some(v.clone()), Vec::new())?;
        return foreach_prep(it);
    }
    // Iterator protocol.
    if with_host(|h| h.class_has_method(&class, "valid") && h.class_has_method(&class, "current")) {
        let arr = with_host(|h| h.new_array());
        if with_host(|h| h.class_has_method(&class, "rewind")) {
            call_method(&class, "rewind", Some(v.clone()), Vec::new())?;
        }
        let has_key = with_host(|h| h.class_has_method(&class, "key"));
        // Bound the walk so a broken `valid()` cannot hang the interpreter.
        for _ in 0..100_000_000u64 {
            let valid = call_method(&class, "valid", Some(v.clone()), Vec::new())?;
            if !with_host(|h| h.is_truthy(&valid)) {
                break;
            }
            let cur = call_method(&class, "current", Some(v.clone()), Vec::new())?;
            let key = if has_key {
                call_method(&class, "key", Some(v.clone()), Vec::new())?
            } else {
                Value::Undef
            };
            with_host(|h| match key {
                Value::Undef => h.arr_push_auto(&arr, cur),
                k => h.arr_set_key(&arr, &k, cur),
            });
            call_method(&class, "next", Some(v.clone()), Vec::new())?;
        }
        return Ok(arr);
    }
    // Plain object: iterate its public properties (name => value).
    let props = with_host(|h| h.object_props(&v));
    Ok(with_host(|h| {
        let a = h.new_array();
        for (k, val) in props {
            h.arr_set_key(&a, &Value::str(k), val);
        }
        a
    }))
}

/// Whether the run already displayed a fatal in PHP's own shape, so the CLI
/// wrapper reports only the exit status instead of printing a second, terse copy.
pub fn fatal_reported() -> bool {
    with_host(|h| h.fatal_reported())
}

/// Record a pending `return` value for the enclosing function frame.
pub fn set_return(v: Value) {
    with_host(|h| h.signal = Some(Signal::Return(v)));
}

/// Record a `break`/`continue` control signal for the enclosing `try` body (the
/// orchestrator relays it to the loop the `try` sits inside).
pub fn set_break(level: u32) {
    with_host(|h| h.signal = Some(Signal::Break(level)));
}

pub fn set_continue(level: u32) {
    with_host(|h| h.signal = Some(Signal::Continue(level)));
}

/// The level of the `break`/`continue` that ended the most recent `try` body.
/// Read by the dispatch code the compiler emits after `RUN_TRY`, which needs to
/// know which enclosing loop the signal was aimed at.
pub fn last_break_level() -> u32 {
    with_host(|h| h.last_break_level)
}

/// Record a thrown exception object; the caller's dispatcher then unwinds.
pub fn set_pending_throw(v: Value) {
    with_host(|h| h.pending_throw = Some(v));
}

/// Whether an exception is in flight (checked by call dispatchers after every
/// nested call so a throw halts the caller too).
pub fn has_pending_throw() -> bool {
    with_host(|h| h.pending_throw.is_some())
}

/// The control status of running one `try`/`catch`/`finally` sub-body.
enum TryStatus {
    Normal,
    Return(Value),
    Throw(Value),
    Break,
    Continue,
    /// A non-catchable Rust-level fatal (e.g. an undefined function) — carried so
    /// `finally` still runs before it surfaces.
    Fatal(String),
}

/// Run one detached `try`/`catch`/`finally` body on the current scope, then read
/// back whichever control signal it raised (throw > return/break/continue >
/// normal). Signals are *taken* here so the next body runs clean — the value is
/// re-stashed only at the final propagation moment (see `run_try_orchestrator`).
fn run_body(chunk: Chunk) -> TryStatus {
    match run_chunk_on(chunk) {
        Err(e) => TryStatus::Fatal(e),
        Ok(_) => with_host(|h| {
            if let Some(exc) = h.pending_throw.take() {
                return TryStatus::Throw(exc);
            }
            match h.signal.take() {
                Some(Signal::Return(v)) => TryStatus::Return(v),
                Some(Signal::Break(n)) => {
                    h.last_break_level = n;
                    TryStatus::Break
                }
                Some(Signal::Continue(n)) => {
                    h.last_break_level = n;
                    TryStatus::Continue
                }
                None => TryStatus::Normal,
            }
        }),
    }
}

/// Orchestrate a `try`/`catch`/`finally` by id (baked into the `RUN_TRY` call).
/// Returns a status code the compiler branches on: `0` normal, `1` return,
/// `2` throw, `3` break, `4` continue. The propagated value is stashed into the
/// matching host signal just before returning, so the parent chunk consumes it
/// immediately. `finally` runs unconditionally and its own non-normal status
/// overrides whatever the `try`/`catch` produced. The whole thing lives on the
/// Rust stack, so nested `try`s recurse cleanly and no signal leaks across
/// unrelated constructs.
pub fn run_try_orchestrator(id: i64) -> Result<i64, String> {
    let Some(def) = with_host(|h| h.try_defs.get(id as usize).cloned()) else {
        return Err(format!("internal: no try-def #{id}"));
    };

    // An `@expr` that throws never reaches its `SUPPRESS_POP`, so the region it
    // opened would stay open and silence everything after the catch. Unwinding
    // out of the expression restores the depth, which is what the reference does
    // when it restores the error-reporting level on the way out.
    let suppress_depth = with_host(|h| h.suppress_depth());
    let mut status = run_body(def.try_chunk);
    with_host(|h| h.suppress_restore(suppress_depth));

    // A thrown exception: try each catch clause in order; the first whose union
    // of class names matches the thrown object's class wins.
    if let TryStatus::Throw(exc) = &status {
        if let Some(class) = with_host(|h| h.object_class(exc)) {
            let exc = exc.clone();
            for c in &def.catches {
                let matched = with_host(|h| c.classes.iter().any(|t| h.catch_matches(&class, t)));
                if matched {
                    if let Some(var) = &c.var {
                        with_host(|h| h.set_var(var, exc.clone()));
                    }
                    status = run_body(c.chunk.clone());
                    break;
                }
            }
        }
    }

    // `finally` always runs; a non-normal status from it replaces the pending one.
    if let Some(fin) = def.finally_chunk {
        match run_body(fin) {
            TryStatus::Normal => {}
            other => status = other,
        }
    }

    Ok(match status {
        TryStatus::Normal => 0,
        TryStatus::Return(v) => {
            with_host(|h| h.signal = Some(Signal::Return(v)));
            1
        }
        TryStatus::Throw(v) => {
            with_host(|h| h.pending_throw = Some(v));
            2
        }
        TryStatus::Break => 3,
        TryStatus::Continue => 4,
        TryStatus::Fatal(e) => return Err(e),
    })
}

// ── numeric formatting / parsing helpers ──────────────────────────────────

/// Format a float the way PHP's default `precision=14` echo does.
fn float_to_php_string(f: f64) -> String {
    if f.is_nan() {
        return "NAN".into();
    }
    if f.is_infinite() {
        return if f < 0.0 { "-INF" } else { "INF" }.into();
    }
    if f == 0.0 {
        // PHP prints negative zero as "-0".
        return if f.is_sign_negative() { "-0" } else { "0" }.into();
    }
    // PHP's default echo uses `zend_gcvt` with precision 14 (significant digits):
    // fixed notation in the normal range, scientific (`1.5E-10`, `1.0E+100`) once
    // the decimal exponent is < -4 or >= 15.
    php_gcvt(f, 14)
}

/// Format a positive-or-negative float the way PHP renders it — 14 significant
/// digits, `%G`-style choice of fixed vs scientific. Also used by `sprintf`'s
/// `%g` with a caller-supplied precision.
pub fn php_gcvt(f: f64, precision: usize) -> String {
    if f == 0.0 {
        return if f.is_sign_negative() {
            "-0".into()
        } else {
            "0".into()
        };
    }
    let neg = f < 0.0;
    let a = f.abs();
    let exp = a.log10().floor() as i32;
    // `%G` rule: scientific when the exponent is below -4 or reaches the
    // significant-digit precision (PHP's default precision is 14).
    let sci = exp < -4 || exp >= precision as i32;
    let body = if sci {
        // Mantissa with (precision-1) fractional digits, trailing zeros stripped
        // but a decimal point kept (`1.0E+100`); explicit exponent sign.
        let raw = format!("{:.*e}", precision.saturating_sub(1), a);
        let (mant, ex) = raw.split_once('e').unwrap_or((raw.as_str(), "0"));
        let mant = mant.trim_end_matches('0');
        // Keep exactly one fractional digit when the mantissa is integer-valued:
        // "1." → "1.0", "1" → "1.0"; leave "1.844674407371" untouched.
        let mant = if let Some(stripped) = mant.strip_suffix('.') {
            format!("{stripped}.0")
        } else if mant.contains('.') {
            mant.to_string()
        } else {
            format!("{mant}.0")
        };
        let exp_n: i32 = ex.parse().unwrap_or(0);
        format!(
            "{mant}E{}{}",
            if exp_n < 0 { "-" } else { "+" },
            exp_n.abs()
        )
    } else {
        let decimals = (precision as i32 - 1 - exp).max(0) as usize;
        let s = format!("{a:.decimals$}");
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s
        }
    };
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

/// The whitespace PHP's numeric-string scanner skips. Deliberately not
/// `str::trim`, which also strips Unicode spaces: PHP reads `"\u{a0}5"` as
/// non-numeric, and trimming NBSP here would silently make it `5`.
const PHP_SPACE: &[char] = &[' ', '\t', '\n', '\r', '\x0b', '\x0c'];

/// Scan the leading numeric run of `s`, returning its value and how many bytes
/// of `s.trim_start_matches(PHP_SPACE)` it consumed, or `None` when the string
/// does not begin with a number at all.
///
/// This is the single source of truth behind all three questions PHP asks about
/// a string in a numeric context — is it numeric, does it merely *start*
/// numeric, and what is the number — so the three can never disagree.
///
/// Two rules are easy to get wrong and are what the byte scan is here for:
/// an exponent counts only when digits actually follow it, so `"5e"` reads as
/// `5` rather than failing; and `"5."` and `".5"` are both complete numbers
/// while a bare `"."` is not.
fn scan_php_number(s: &str) -> Option<(Value, usize, bool)> {
    let t = s.trim_start_matches(PHP_SPACE);
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let digits_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    // `end` is the length of the longest complete number seen so far; 0 means
    // nothing valid has been scanned yet, which is how a bare sign or `.` fails.
    let mut end = if i > digits_start { i } else { 0 };
    let mut is_float = false;
    if i < b.len() && b[i] == b'.' {
        let frac_start = i + 1;
        let mut j = frac_start;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        // `".5"` needs the fraction digits; `"5."` is already complete without
        // them. A lone `"."` has neither and stays invalid.
        if j > frac_start || end != 0 {
            i = j;
            end = j;
            is_float = true;
        }
    }
    if end == 0 {
        return None;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let exp_digits = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > exp_digits {
            end = j;
            is_float = true;
        }
    }
    let text = &t[..end];
    // An integer literal too wide for `i64` becomes a float, as PHP does.
    let value = match (is_float, text.parse::<i64>()) {
        (false, Ok(n)) => Value::int(n),
        _ => Value::float(text.parse::<f64>().ok()?),
    };
    Some((value, end, is_float))
}

/// Whether a string's numeric prefix is *written* in float form — it carries a
/// decimal point or an exponent.
///
/// This is not the same question as "did it parse to a float": an integer-format
/// string too wide for `i64` also parses to a float, and PHP treats the two
/// differently when narrowing back to int. `"1e20" % 2` deprecates the
/// narrowing, `"9223372036854775808" % 2` does not.
pub fn numeric_prefix_is_float(s: &str) -> bool {
    scan_php_number(s).is_some_and(|(_, _, is_float)| is_float)
}

/// Parse the leading numeric prefix of a string into an `Int`/`Float` `Value`,
/// as PHP does when a string is used in arithmetic (`"12abc" + 0 == 12`).
/// A string with no numeric prefix at all reads as `0`, matching `(int)"abc"`.
fn parse_php_number(s: &str) -> Value {
    scan_php_number(s).map_or(Value::int(0), |(v, _, _)| v)
}

/// Whether a string is a fully numeric PHP string (for loose comparison).
/// PHP's alphanumeric string succession, the `++` operator on a non-numeric
/// string (`"a"` → `"b"`, `"z"` → `"aa"`, `"Az"` → `"Ba"`, `"a9"` → `"b0"`).
///
/// Carry propagates right to left through `[a-zA-Z0-9]` only: the first
/// non-alphanumeric character stops it outright, leaving the rest of the string
/// untouched (`"a-z"` → `"a-a"`, `"a_"` unchanged). A carry that runs off the
/// front prepends a character of the same class as the one it left — `a`, `A` or
/// `1`.
fn increment_alnum_string(s: &str) -> String {
    let mut b = s.as_bytes().to_vec();
    let mut i = b.len();
    let mut carry = false;
    while i > 0 {
        i -= 1;
        match b[i] {
            b'z' => {
                b[i] = b'a';
                carry = true;
            }
            b'Z' => {
                b[i] = b'A';
                carry = true;
            }
            b'9' => {
                b[i] = b'0';
                carry = true;
            }
            c if c.is_ascii_alphanumeric() => {
                b[i] = c + 1;
                carry = false;
                break;
            }
            // A non-alphanumeric character ends the succession entirely.
            _ => {
                carry = false;
                break;
            }
        }
        if !carry {
            break;
        }
    }
    if carry {
        let lead = match b.first() {
            Some(b'0') => b'1',
            Some(b'A') => b'A',
            _ => b'a',
        };
        b.insert(0, lead);
    }
    String::from_utf8_lossy(&b).into_owned()
}

pub fn is_numeric_string(s: &str) -> bool {
    parse_php_number_full(s).is_some()
}

/// Parse a string that is *entirely* numeric (no trailing garbage), or `None`.
///
/// "Entirely" allows whitespace on both ends (`" 5 "` is numeric in PHP 8) but
/// nothing else. Routing through [`scan_php_number`] rather than Rust's own
/// float parser is what keeps `"INF"`/`"NAN"` non-numeric — Rust accepts those
/// spellings and PHP does not — while still admitting `"1e400"`, which is a
/// numeric string that happens to overflow to infinity.
pub fn parse_php_number_full(s: &str) -> Option<Value> {
    let t = s.trim_matches(PHP_SPACE);
    let (value, used, _) = scan_php_number(t)?;
    (used == t.len()).then_some(value)
}

/// How an operand reads to PHP 8's arithmetic operators.
///
/// PHP 8 split what PHP 7 did silently into three outcomes, and an operator has
/// to tell them apart *before* it computes anything. See [`classify_arith`].
pub enum ArithOperand {
    /// Converts silently: a number, a bool, null, or a fully numeric string.
    Numeric(Value),
    /// A leading-numeric string such as `"5g"`. The operator raises
    /// `A non-numeric value encountered` and then uses the prefix.
    Leading(Value),
    /// No numeric reading at all — `"g"`, `""`, an array, an object. The
    /// operator raises `TypeError` instead of producing a value.
    Unsupported,
}

/// Classify an operand for PHP 8's arithmetic operators.
///
/// The three-way split is the whole of the PHP 7 → 8 juggling change: `"g" + 9`
/// used to be `9` and is now a `TypeError`, while `"5g" + 1` used to be silent
/// and is now a warning that still yields `6`. Booleans and `null` were left
/// alone by that change and still convert without complaint.
pub fn classify_arith(h: &PhpHost, v: &Value) -> ArithOperand {
    match v {
        Value::Int(_) | Value::Float(_) => ArithOperand::Numeric(v.clone()),
        Value::Bool(b) => ArithOperand::Numeric(Value::int(*b as i64)),
        Value::Undef => ArithOperand::Numeric(Value::int(0)),
        Value::Str(s) => match parse_php_number_full(s) {
            Some(n) => ArithOperand::Numeric(n),
            // A prefix exists but did not cover the string: warn and use it.
            None => match scan_php_number(s) {
                Some((n, _, _)) => ArithOperand::Leading(n),
                None => ArithOperand::Unsupported,
            },
        },
        // Arrays and objects have no arithmetic reading. `__toString` is not
        // consulted here: the reference reports `Closure + int`, not `string`.
        _ => {
            let _ = h;
            ArithOperand::Unsupported
        }
    }
}

/// The type name PHP prints in `Unsupported operand types: X op Y`.
///
/// These are the short spellings (`int`, `bool`, `null`), not the `gettype`
/// ones ([`PhpHost::type_name`] returns `integer`/`boolean`/`NULL`), and an
/// object contributes its class name rather than the word `object`.
pub fn arith_type_name(h: &PhpHost, v: &Value) -> String {
    match v {
        Value::Undef => "null".into(),
        Value::Bool(_) => "bool".into(),
        Value::Int(_) => "int".into(),
        Value::Float(_) => "float".into(),
        Value::Str(_) => "string".into(),
        Value::Obj(_) if h.is_array(v) => "array".into(),
        // A closure is an object of class `Closure`, and the reference names it
        // that way even though it has no declared class entry.
        Value::Obj(_) if h.is_closure(v) => "Closure".into(),
        Value::Obj(_) => h.object_class(v).unwrap_or_else(|| "object".into()),
        _ => "mixed".into(),
    }
}
