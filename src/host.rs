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
}

/// A compiled user function: its parameter names plus the lowered body chunk.
#[derive(Debug, Clone)]
pub struct FuncDef {
    pub params: Vec<String>,
    pub chunk: Chunk,
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

/// A heap object. Only arrays live here in the scaffold; objects/closures are a
/// later wave.
#[derive(Debug, Clone)]
pub enum PhpObj {
    Array {
        entries: IndexMap<ArrayKey, Value>,
        /// The next integer key an append (`$a[] = ...`) will use.
        next_index: i64,
    },
}

/// A control-flow signal that unwinds out of a function body.
enum Signal {
    Return(Value),
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
    /// When `Some`, `echo` appends here instead of writing to stdout (used by
    /// `eval_capture` and the test harness).
    capture: Option<String>,
    error: Option<String>,
    signal: Option<Signal>,
}

impl Default for PhpHost {
    fn default() -> Self {
        Self::new()
    }
}

impl PhpHost {
    pub fn new() -> Self {
        PhpHost {
            objs: Vec::new(),
            // Start with the global scope already open.
            scopes: vec![Scope::default()],
            functions: FxHashMap::default(),
            capture: None,
            error: None,
            signal: None,
        }
    }

    // ── program loading ────────────────────────────────────────────────────

    /// Install a compiled program's user functions onto the host.
    pub fn load_program(&mut self, functions: Vec<(String, FuncDef)>) {
        for (name, def) in functions {
            self.functions.insert(name, def);
        }
    }

    // ── output capture ─────────────────────────────────────────────────────

    pub fn begin_capture(&mut self) {
        self.capture = Some(String::new());
    }

    pub fn end_capture(&mut self) -> String {
        self.capture.take().unwrap_or_default()
    }

    /// Emit a rendered string via `echo`: to the capture buffer if active, else
    /// to stdout (no trailing newline — PHP `echo` writes exactly its argument).
    pub fn write_out(&mut self, s: &str) {
        match &mut self.capture {
            Some(buf) => buf.push_str(s),
            None => {
                use std::io::Write;
                let mut out = std::io::stdout();
                let _ = out.write_all(s.as_bytes());
            }
        }
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
        self.scopes
            .last()
            .and_then(|s| s.vars.get(name).cloned())
            .unwrap_or(Value::Undef)
    }

    pub fn set_var(&mut self, name: &str, val: Value) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.vars.insert(name.to_string(), val);
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

    /// Remove and return the last element of `$var` (for `array_pop`).
    pub fn arr_pop_var(&mut self, name: &str) -> Value {
        let arr = self.get_var(name);
        if let Some(PhpObj::Array { entries, .. }) = self.as_array_mut(&arr) {
            return entries.pop().map(|(_, v)| v).unwrap_or(Value::Undef);
        }
        Value::Undef
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
    /// array are falsy; everything else is truthy.
    pub fn is_truthy(&self, v: &Value) -> bool {
        match v {
            Value::Undef => false,
            Value::Bool(b) => *b,
            Value::Int(n) => *n != 0,
            Value::Float(f) => *f != 0.0,
            Value::Str(s) => !(s.is_empty() || s.as_str() == "0"),
            Value::Obj(_) => self.array_len(v) != 0,
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
            Value::Obj(_) => Value::int(if self.array_len(v) == 0 { 0 } else { 1 }),
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
            Value::Obj(_) => "array",
            _ => "unknown type",
        }
    }
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
        with_host(|h| {
            let mut scope = Scope {
                name: Some(name.to_string()),
                ..Scope::default()
            };
            for (i, p) in def.params.iter().enumerate() {
                scope
                    .vars
                    .insert(p.clone(), args.get(i).cloned().unwrap_or(Value::Undef));
            }
            h.scopes.push(scope);
        });
        let r = run_chunk_on(def.chunk.clone());
        let sig = with_host(|h| {
            h.scopes.pop();
            h.signal.take()
        });
        return match sig {
            Some(Signal::Return(v)) => Ok(v),
            None => r.map(|_| Value::Undef),
        };
    }
    crate::builtins::call_library(name, args)
}

/// Record a pending `return` value for the enclosing function frame.
pub fn set_return(v: Value) {
    with_host(|h| h.signal = Some(Signal::Return(v)));
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
