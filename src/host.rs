//! The PHP object heap and runtime, reached from fusevm through registered
//! builtins (`builtins::install`) and the strict numeric hook.
//!
//! Scalars (int/float/bool/string/null) ride through the VM as native
//! `fusevm::Value`s. Arrays are heap objects: a `Value::Obj(u32)` handle indexes
//! `PhpHost::objs`. All mutable runtime state — the current variable scope stack,
//! the user-function table, the output buffer, the pending error/return signal —
//! lives in a `thread_local!` `PhpHost`, so a fresh `VM` can be spun up per
//! function call (see `call_function`) while sharing one heap.

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
}

/// A compiled user function: its parameters plus the lowered body chunk.
#[derive(Debug, Clone)]
pub struct FuncDef {
    pub params: Vec<Param>,
    pub chunk: Chunk,
}

/// A closure unpacked for a call: its parameters, body chunk, and the
/// `(name, value)` bindings it captured at creation time.
type ClosureCall = (Vec<Param>, Chunk, Vec<(String, Value)>);

/// A compiled class: its parent (for single-inheritance resolution), constant and
/// property-default initializers (each an expression chunk that leaves its value
/// on the stack), and its methods keyed by lowercase name. `self::`/`parent::`
/// were resolved to concrete class names at compile time.
#[derive(Debug, Clone)]
pub struct ClassDef {
    pub parent: Option<String>,
    pub consts: Vec<(String, Chunk)>,
    pub prop_defaults: Vec<(String, Chunk)>,
    pub methods: FxHashMap<String, FuncDef>,
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
    /// `(name, value)` bindings captured (by value) at creation time.
    Closure {
        params: Vec<Param>,
        chunk: Chunk,
        captured: Vec<(String, Value)>,
    },
    /// A class instance: its class name and its properties. Referenced by a
    /// `Value::Obj(u32)` handle, so objects have PHP reference semantics (passing
    /// one around shares the same instance).
    Object {
        class: String,
        props: IndexMap<String, Value>,
    },
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
    Break,
    Continue,
}

/// One variable scope (the global scope, or a function-call frame).
#[derive(Default)]
struct Scope {
    vars: FxHashMap<String, Value>,
    /// The source line this frame is currently executing (DAP line hook). Only
    /// meaningful under `--dap`; `0` otherwise.
    line: u32,
    /// The function name for a call frame, `None` for the global scope. Reported
    /// as the frame name in a DAP `stackTrace`.
    name: Option<String>,
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
    /// Compiled `try`/`catch`/`finally` constructs, indexed by the id the
    /// compiler bakes into each `RUN_TRY` call.
    try_defs: Vec<TryDef>,
    /// Named constants (`PHP_EOL`, `M_PI`, user `define`s), keyed case-sensitively.
    /// Seeded with the standard predefined constants on every fresh host.
    constants: FxHashMap<String, Value>,
}

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
            try_defs: Vec::new(),
            constants: predefined_constants(),
            ob_stack: Vec::new(),
        };
        h.init_superglobals();
        h
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
        for name in ["_GET", "_POST", "_REQUEST", "_COOKIE", "_FILES", "_SESSION", "GLOBALS"] {
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
        let mut cur = Some(class.to_ascii_lowercase());
        while let Some(c) = cur {
            if c == ancestor {
                return true;
            }
            cur = self
                .classes
                .get(&c)
                .and_then(|d| d.parent.as_ref().map(|p| p.to_ascii_lowercase()));
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

    pub fn set_error(&mut self, msg: impl Into<String>) {
        if self.error.is_none() {
            self.error = Some(msg.into());
        }
    }

    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    // ── variables ──────────────────────────────────────────────────────────

    pub fn get_var(&self, name: &str) -> Value {
        // Superglobals (`$_SERVER`, `$_ENV`, `$GLOBALS`, `$argv`, …) live in the
        // global scope and are visible from every function frame.
        let scope = if is_superglobal(name) {
            self.scopes.first()
        } else {
            self.scopes.last()
        };
        scope
            .and_then(|s| s.vars.get(name).cloned())
            .unwrap_or(Value::Undef)
    }

    pub fn set_var(&mut self, name: &str, val: Value) {
        let idx = if is_superglobal(name) {
            0
        } else {
            self.scopes.len().saturating_sub(1)
        };
        if let Some(scope) = self.scopes.get_mut(idx) {
            scope.vars.insert(name.to_string(), val);
        }
    }

    /// `unset($name)` — remove the scope variable.
    pub fn unset_var(&mut self, name: &str) {
        let idx = if is_superglobal(name) {
            0
        } else {
            self.scopes.len().saturating_sub(1)
        };
        if let Some(scope) = self.scopes.get_mut(idx) {
            scope.vars.remove(name);
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

    /// Record the source line the innermost frame is executing (DAP line hook).
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
        let vars: Vec<(String, Value)> = self
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

    // ── closures ───────────────────────────────────────────────────────────

    /// Build a closure object from a compiler-registered function definition
    /// (`def_name`, a synthetic name in the function table) plus the values
    /// captured at creation time. Returns the new handle, or `Undef` if the
    /// definition is missing (should never happen for compiler-emitted names).
    pub fn make_closure(&mut self, def_name: &str, captured: Vec<(String, Value)>) -> Value {
        let Some(def) = self.functions.get(def_name).cloned() else {
            return Value::Undef;
        };
        self.objs.push(PhpObj::Closure {
            params: def.params,
            chunk: def.chunk,
            captured,
        });
        Value::Obj((self.objs.len() - 1) as u32)
    }

    /// The closure held by `v` (a handle), cloned for a call: its parameters,
    /// body chunk, and captured bindings. `None` if `v` is not a closure.
    fn closure_of(&self, v: &Value) -> Option<ClosureCall> {
        match self.as_array(v) {
            Some(PhpObj::Closure {
                params,
                chunk,
                captured,
            }) => Some((params.clone(), chunk.clone(), captured.clone())),
            _ => None,
        }
    }

    /// Whether `v` is a closure handle.
    pub fn is_closure(&self, v: &Value) -> bool {
        matches!(self.as_array(v), Some(PhpObj::Closure { .. }))
    }

    // ── objects / classes ──────────────────────────────────────────────────

    pub fn is_object(&self, v: &Value) -> bool {
        matches!(self.as_array(v), Some(PhpObj::Object { .. }))
    }

    /// The heap handle of any object/array/resource value — a stable per-instance
    /// id (`spl_object_id`). `None` for non-heap values.
    pub fn object_id(&self, v: &Value) -> Option<i64> {
        match v {
            Value::Obj(h) => Some(*h as i64),
            _ => None,
        }
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
            let Some(def) = self.classes.get(&c) else { break };
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
            let Some(def) = self.classes.get(&c) else { break };
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
            Some(PhpObj::Object { props, .. }) => {
                props.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
            }
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
        matches!(self.as_array(v), Some(PhpObj::Resource { closed: false, .. }))
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
            Some(PhpObj::Resource { pos, closed: false, .. }) => Some(*pos as i64),
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
            Some(PhpObj::Object { props, .. }) => props.get(name).cloned().unwrap_or(Value::Undef),
            _ => Value::Undef,
        }
    }

    /// `$obj->name = val` — mutates the shared instance behind the handle.
    pub fn prop_set(&mut self, recv: &Value, name: &str, val: Value) {
        if let Some(PhpObj::Object { props, .. }) = self.as_array_mut(recv) {
            props.insert(name.to_string(), val);
        }
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

    /// `$arr[key]` read. Also indexes strings (single-character substring).
    pub fn index_get(&self, recv: &Value, key: &Value) -> Value {
        if let Some(PhpObj::Array { entries, .. }) = self.as_array(recv) {
            let k = self.norm_key(key);
            return entries.get(&k).cloned().unwrap_or(Value::Undef);
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

    /// `$var[key] = val` on the named scope variable, auto-vivifying an array.
    pub fn index_set_var(&mut self, name: &str, key: &Value, val: Value) {
        let arr = self.ensure_array_var(name);
        let k = self.norm_key(key);
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
            entries.insert(k, val);
        }
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
                Some(PhpObj::Array { entries, .. }) => entries.get(&k).cloned(),
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

    /// Insert `v` under `key` in the array `arr` (a handle).
    pub fn arr_set_key(&mut self, arr: &Value, key: &Value, v: Value) {
        let k = self.norm_key(key);
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
            Some(PhpObj::Array { entries, .. }) => Some(
                entries
                    .iter()
                    .map(|(k, v)| (k.to_value(), v.clone()))
                    .collect(),
            ),
            _ => None,
        }
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
    si("E_ERROR", 1);
    si("E_WARNING", 2);
    si("E_PARSE", 4);
    si("E_NOTICE", 8);
    si("E_STRICT", 2048);
    si("E_DEPRECATED", 8192);
    si("E_ALL", 32767);
    si("E_USER_ERROR", 256);
    si("E_USER_WARNING", 512);
    si("E_USER_NOTICE", 1024);
    si("E_USER_DEPRECATED", 16384);
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
    ss("DIRECTORY_SEPARATOR", if cfg!(windows) { "\\" } else { "/" });
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
    HOST.with(|h| *h.borrow_mut() = PhpHost::new());
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
    let mut vm = VM::new(chunk);
    crate::builtins::install(&mut vm);
    vm.set_numeric_hook(std::sync::Arc::new(|op, a, b| {
        crate::builtins::numeric_hook(op, a, b)
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
        VMResult::Error(e) => Err(e),
    }
}

/// Run the top-level program chunk.
pub fn run_main(chunk: Chunk) -> Result<Value, String> {
    let r = run_chunk_on(chunk);
    // A top-level `return` just ends the program; clear any leftover signal.
    with_host(|h| h.signal.take());
    // An exception that reached the top uncaught is a fatal error, shaped like
    // the PHP CLI's `PHP Fatal error:  Uncaught <Class>: <message>`.
    if let Some(exc) = with_host(|h| h.pending_throw.take()) {
        let (class, msg) = with_host(|h| {
            let class = h.object_class(&exc).unwrap_or_else(|| "Exception".to_string());
            let msg = h.to_str(&h.prop_get(&exc, "message"));
            (class, msg)
        });
        return Err(format!("PHP Fatal error:  Uncaught {class}: {msg}"));
    }
    r
}

/// Invoke a user function (or fall through to the builtin library) by name.
/// Pushes a fresh scope, binds positional parameters, runs the body chunk, and
/// returns the `return` value (or null if the body fell off the end).
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
        return invoke(name, &def.params, def.chunk, Vec::new(), args);
    }
    crate::builtins::call_library(name, args)
}

/// Run a user-defined function or closure body: push a call frame named `frame`,
/// pre-bind `pre` (a closure's captured `(name, value)` pairs, empty for a plain
/// function), bind `args` to `params` (variadic collection, then default chunks
/// for omitted parameters), run `body`, and return the `return` value (or null if
/// the body fell off the end). Default chunks run OUTSIDE the binding `with_host`
/// closure because `run_chunk_on` itself borrows the thread-local host.
fn invoke(
    frame: &str,
    params: &[Param],
    body: Chunk,
    pre: Vec<(String, Value)>,
    args: Vec<Value>,
) -> Result<Value, String> {
    with_host(|h| {
        let scope = Scope {
            name: Some(frame.to_string()),
            ..Scope::default()
        };
        h.scopes.push(scope);
        // Stash the full call argument list (hidden `@args`) for func_get_args /
        // func_num_args / func_get_arg, which read the current frame.
        let argsarr = h.new_array();
        for a in &args {
            h.arr_push_auto(&argsarr, a.clone());
        }
        h.set_var("@args", argsarr);
        // Captured bindings first, then parameters (a parameter of the same name
        // as a capture shadows it, as PHP does).
        for (k, v) in pre {
            h.set_var(&k, v);
        }
        let mut ai = 0;
        for p in params {
            if p.variadic {
                // A variadic (`...$rest`, always last) collects the remaining args.
                let arr = h.new_array();
                while ai < args.len() {
                    h.arr_push_auto(&arr, args[ai].clone());
                    ai += 1;
                }
                h.set_var(&p.name, arr);
            } else if ai < args.len() {
                h.set_var(&p.name, args[ai].clone());
                ai += 1;
            } else {
                // Omitted: a parameter without a default reads as null; one with a
                // default is filled below by running its chunk.
                if p.default.is_none() {
                    h.set_var(&p.name, Value::Undef);
                }
                ai += 1;
            }
        }
    });
    // Evaluate defaults for the omitted parameters, left to right, so a default
    // may reference an earlier parameter already bound in this frame.
    for (i, p) in params.iter().enumerate() {
        if p.variadic || i < args.len() {
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
    let r = run_chunk_on(body);
    let sig = with_host(|h| {
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
        Some(Signal::Break) | Some(Signal::Continue) | None => r.map(|_| Value::Undef),
    }
}

/// Invoke a callable *value*: a closure handle runs its captured-plus-bound body
/// in a fresh scope; a string is dispatched by name through `call_function`. Used
/// by `$f(...)` calls and callback builtins (`array_map`).
pub fn call_value(callee: Value, args: Vec<Value>) -> Result<Value, String> {
    if let Some((params, chunk, captured)) = with_host(|h| h.closure_of(&callee)) {
        return invoke("{closure}", &params, chunk, captured, args);
    }
    match callee {
        Value::Str(s) => call_function(&s, args),
        _ => Err("value is not callable".to_string()),
    }
}

// ── objects (new / method dispatch / constants) ─────────────────────────────

/// `new Class(args)`: allocate an object seeded with its (inherited) property
/// defaults, then run its constructor. Property-default and constructor code run
/// on fresh VMs, so no host borrow is held across them.
pub fn new_object(class: &str, args: Vec<Value>) -> Result<Value, String> {
    let cl = class.to_ascii_lowercase();
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
    // Run the constructor if one exists anywhere in the chain.
    if with_host(|h| h.resolve_method(&cl, "__construct").is_some()) {
        call_method(&cl, "__construct", Some(obj.clone()), args)?;
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
    let Some((def_class, def)) = with_host(|h| h.resolve_method(class, &method_l)) else {
        return Err(format!("call to undefined method {class}::{method}()"));
    };
    let pre = match this {
        Some(t) => vec![("this".to_string(), t)],
        None => Vec::new(),
    };
    invoke(&format!("{def_class}::{method}"), &def.params, def.chunk, pre, args)
}

/// `Class::CONST` — evaluate the (inherited) constant initializer.
pub fn class_const(class: &str, name: &str) -> Result<Value, String> {
    let Some(chunk) = with_host(|h| h.resolve_const_chunk(class, name)) else {
        return Err(format!("undefined constant {class}::{name}"));
    };
    run_chunk_on(chunk)
}

/// Record a pending `return` value for the enclosing function frame.
pub fn set_return(v: Value) {
    with_host(|h| h.signal = Some(Signal::Return(v)));
}

/// Record a `break`/`continue` control signal for the enclosing `try` body (the
/// orchestrator relays it to the loop the `try` sits inside).
pub fn set_break() {
    with_host(|h| h.signal = Some(Signal::Break));
}

pub fn set_continue() {
    with_host(|h| h.signal = Some(Signal::Continue));
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
                Some(Signal::Break) => TryStatus::Break,
                Some(Signal::Continue) => TryStatus::Continue,
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

    let mut status = run_body(def.try_chunk);

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

/// Parse the leading numeric prefix of a string into an `Int`/`Float` `Value`,
/// as PHP does when a string is used in arithmetic (`"12abc" + 0 == 12`).
fn parse_php_number(s: &str) -> Value {
    let t = s.trim_start();
    let bytes = t.as_bytes();
    let mut i = 0;
    let mut seen_dot = false;
    let mut seen_exp = false;
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        i += 1;
    }
    while i < bytes.len() {
        match bytes[i] {
            b'0'..=b'9' => i += 1,
            b'.' if !seen_dot && !seen_exp => {
                seen_dot = true;
                i += 1;
            }
            b'e' | b'E' if !seen_exp && i > 0 => {
                seen_exp = true;
                i += 1;
                if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
                    i += 1;
                }
            }
            _ => break,
        }
    }
    let prefix = &t[..i];
    if prefix.is_empty() || prefix == "+" || prefix == "-" {
        return Value::int(0);
    }
    if seen_dot || seen_exp {
        Value::float(prefix.parse().unwrap_or(0.0))
    } else {
        match prefix.parse::<i64>() {
            Ok(n) => Value::int(n),
            Err(_) => Value::float(prefix.parse().unwrap_or(0.0)),
        }
    }
}

/// Whether a string is a fully numeric PHP string (for loose comparison).
pub fn is_numeric_string(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return false;
    }
    parse_php_number_full(t).is_some()
}

/// Parse a string that is *entirely* numeric (no trailing garbage), or `None`.
pub fn parse_php_number_full(s: &str) -> Option<Value> {
    let t = s.trim();
    if let Ok(n) = t.parse::<i64>() {
        return Some(Value::int(n));
    }
    if let Ok(f) = t.parse::<f64>() {
        if f.is_finite() {
            return Some(Value::float(f));
        }
    }
    None
}
