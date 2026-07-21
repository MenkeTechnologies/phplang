//! The phplang builtins: the `CallBuiltin` handlers the compiler emits, the
//! strict numeric hook, and the PHP standard-library functions reached through
//! `CALL`.
//!
//! Every handler has the fusevm signature `fn(&mut VM, u8) -> Value` and leaves
//! exactly one value on the stack. Operations that can fail (division by zero, an
//! undefined function) record the message on the host and halt the current chunk
//! by setting `vm.ip` past the end; `host::run_chunk_on` then surfaces it as an
//! `Err`.

use crate::host::{self, ops, with_host};
use fusevm::{NumOp, Value, VM};

/// Register every compiler-emitted builtin on a fresh VM.
pub fn install(vm: &mut VM) {
    vm.register_builtin(ops::ECHO, b_echo);
    vm.register_builtin(ops::GETVAR, b_getvar);
    vm.register_builtin(ops::SETVAR, b_setvar);
    vm.register_builtin(ops::CONCAT, b_concat);
    vm.register_builtin(ops::TRUTHY, b_truthy);
    vm.register_builtin(ops::CALL, b_call);
    vm.register_builtin(ops::CALL_SPREAD, b_call_spread);
    vm.register_builtin(ops::MKARRAY, b_mkarray);
    vm.register_builtin(ops::INDEX_GET, b_index_get);
    vm.register_builtin(ops::INDEX_SET, b_index_set);
    vm.register_builtin(ops::ARR_APPEND, b_arr_append);
    vm.register_builtin(ops::SET_PATH, b_set_path);
    vm.register_builtin(ops::APPEND_PATH, b_append_path);
    vm.register_builtin(ops::GET_PATH, b_get_path);
    vm.register_builtin(ops::INCDEC_PATH, b_incdec_path);
    vm.register_builtin(ops::PATH_APPEND_CHILD, b_path_append_child);
    vm.register_builtin(ops::DIV, b_div);
    vm.register_builtin(ops::MOD, b_mod);
    vm.register_builtin(ops::POW, b_pow);
    vm.register_builtin(ops::LOOSE_EQ, b_loose_eq);
    vm.register_builtin(ops::LOOSE_NE, b_loose_ne);
    vm.register_builtin(ops::STRICT_EQ, b_strict_eq);
    vm.register_builtin(ops::STRICT_NE, b_strict_ne);
    vm.register_builtin(ops::LT, b_lt);
    vm.register_builtin(ops::GT, b_gt);
    vm.register_builtin(ops::LE, b_le);
    vm.register_builtin(ops::GE, b_ge);
    vm.register_builtin(ops::SIG_RETURN, b_sig_return);
    vm.register_builtin(ops::INCDEC, b_incdec);
    vm.register_builtin(ops::ARRAYKEYS, b_arraykeys);
    vm.register_builtin(ops::ARRAYLEN, b_arraylen);
    vm.register_builtin(ops::BITAND, b_bitand);
    vm.register_builtin(ops::BITOR, b_bitor);
    vm.register_builtin(ops::BITXOR, b_bitxor);
    vm.register_builtin(ops::SHL, b_shl);
    vm.register_builtin(ops::SHR, b_shr);
    vm.register_builtin(ops::BITNOT, b_bitnot);
    vm.register_builtin(ops::SPACESHIP, b_spaceship);
    vm.register_builtin(ops::DBG_LINE, b_dbg_line);
    vm.register_builtin(ops::ARR_MUT, b_arr_mut);
    vm.register_builtin(ops::MKCLOSURE, b_mkclosure);
    vm.register_builtin(ops::CALL_VALUE, b_call_value);
    vm.register_builtin(ops::NEW, b_new);
    vm.register_builtin(ops::PROP_GET, b_prop_get);
    vm.register_builtin(ops::PROP_SET, b_prop_set);
    vm.register_builtin(ops::PROP_ENSURE_ARRAY, b_prop_ensure_array);
    vm.register_builtin(ops::PROP_INCDEC, b_prop_incdec);
    vm.register_builtin(ops::MCALL, b_mcall);
    vm.register_builtin(ops::SCALL, b_scall);
    vm.register_builtin(ops::SCONST, b_sconst);
    vm.register_builtin(ops::THROW, b_throw);
    vm.register_builtin(ops::RUN_TRY, b_run_try);
    vm.register_builtin(ops::SIG_HALT, b_sig_halt);
    vm.register_builtin(ops::SIG_BREAK, b_sig_break);
    vm.register_builtin(ops::SIG_CONTINUE, b_sig_continue);
    vm.register_builtin(ops::CONST_FETCH, b_const_fetch);
    vm.register_builtin(ops::UNSET_VAR, b_unset_var);
    vm.register_builtin(ops::UNSET_PATH, b_unset_path);
    vm.register_builtin(ops::FOREACH_PREP, b_foreach_prep);
    vm.register_builtin(ops::INSTANCEOF, b_instanceof);
    vm.register_builtin(ops::REF_BIND, b_ref_bind);
}

/// `$target = &$source` — bind the two names to a shared cell; leaves the value.
fn b_ref_bind(vm: &mut VM, _: u8) -> Value {
    let source = pop_name(vm);
    let target = pop_name(vm);
    with_host(|h| {
        h.ref_bind(&target, &source);
        h.get_var(&target)
    })
}

/// `$obj instanceof Class` — true if `$obj` is an object whose class is, or
/// descends from / implements, `Class`. Stack `[obj, class-name]`.
fn b_instanceof(vm: &mut VM, _: u8) -> Value {
    let target = pop_name(vm);
    let obj = vm.pop();
    Value::bool(with_host(|h| match h.object_class(&obj) {
        Some(class) => h.is_a_class(&class, &target),
        None => false,
    }))
}

/// Normalize a `foreach` subject to an iterable array (objects are iterated).
fn b_foreach_prep(vm: &mut VM, _: u8) -> Value {
    let v = vm.pop();
    match host::foreach_prep(v) {
        Ok(a) => {
            // A throw inside an iterator method must unwind the caller too.
            if bubble_throw(vm) {
                Value::Undef
            } else {
                a
            }
        }
        Err(e) => fail(vm, e),
    }
}

/// Resolve a bare constant reference to its value (or the bare name as a string
/// when undefined, matching PHP 7 leniency).
fn b_const_fetch(vm: &mut VM, _: u8) -> Value {
    let name = pop_name(vm);
    with_host(|h| h.const_fetch(&name))
}

/// `unset($var)` — remove the scope variable.
fn b_unset_var(vm: &mut VM, _: u8) -> Value {
    let name = pop_name(vm);
    with_host(|h| h.unset_var(&name));
    Value::Undef
}

/// `unset($name[k1]..[kN])` — remove the deepest array element. Stack:
/// `[name, k1..kN]`, `N = argc-1`.
fn b_unset_path(vm: &mut VM, argc: u8) -> Value {
    let keys = pop_args(vm, argc as usize - 1);
    let name = pop_name(vm);
    with_host(|h| h.unset_path(&name, &keys));
    Value::Undef
}

/// Halt the current chunk if an exception is now in flight, so a `throw` raised
/// by a nested call bubbles up through the caller's VM too. Returns `true` when
/// it halted (the caller should return immediately).
fn bubble_throw(vm: &mut VM) -> bool {
    if host::has_pending_throw() {
        vm.ip = vm.chunk.ops.len();
        true
    } else {
        false
    }
}

// ── exceptions / try-catch ───────────────────────────────────────────────────

/// `throw e` — record the exception object as pending and halt this chunk.
fn b_throw(vm: &mut VM, _: u8) -> Value {
    let exc = vm.pop();
    host::set_pending_throw(exc);
    vm.ip = vm.chunk.ops.len();
    Value::Undef
}

/// Run a `try`/`catch`/`finally` construct by id; leaves its control status int.
fn b_run_try(vm: &mut VM, _: u8) -> Value {
    let id = vm.pop().to_int();
    match host::run_try_orchestrator(id) {
        Ok(status) => Value::int(status),
        Err(e) => fail(vm, e),
    }
}

/// Halt the current chunk, leaving whatever signal `run_try` already stashed (a
/// pending return or throw) for the enclosing frame to pick up.
fn b_sig_halt(vm: &mut VM, _: u8) -> Value {
    vm.ip = vm.chunk.ops.len();
    Value::Undef
}

fn b_sig_break(vm: &mut VM, _: u8) -> Value {
    host::set_break();
    vm.ip = vm.chunk.ops.len();
    Value::Undef
}

fn b_sig_continue(vm: &mut VM, _: u8) -> Value {
    host::set_continue();
    vm.ip = vm.chunk.ops.len();
    Value::Undef
}

/// Pop two operands as PHP integers (bitwise ops cast their operands to int).
fn pop_two_ints(vm: &mut VM) -> (i64, i64) {
    let b = vm.pop();
    let a = vm.pop();
    with_host(|h| (h.to_number(&a).to_int(), h.to_number(&b).to_int()))
}

fn b_bitand(vm: &mut VM, _: u8) -> Value {
    let (a, b) = pop_two_ints(vm);
    Value::int(a & b)
}
fn b_bitor(vm: &mut VM, _: u8) -> Value {
    let (a, b) = pop_two_ints(vm);
    Value::int(a | b)
}
fn b_bitxor(vm: &mut VM, _: u8) -> Value {
    let (a, b) = pop_two_ints(vm);
    Value::int(a ^ b)
}

fn b_shl(vm: &mut VM, _: u8) -> Value {
    let (a, b) = pop_two_ints(vm);
    // PHP throws ArithmeticError on a negative shift.
    if b < 0 {
        return fail(vm, "Bit shift by negative number");
    }
    // A left shift by >= 64 bits yields 0. Within range PHP wraps on overflow
    // (`1 << 63` = INT_MIN) — `wrapping_shl` matches that where a plain `<<`
    // would panic in a debug build.
    if b >= 64 {
        return Value::int(0);
    }
    Value::int(a.wrapping_shl(b as u32))
}

fn b_shr(vm: &mut VM, _: u8) -> Value {
    let (a, b) = pop_two_ints(vm);
    if b < 0 {
        return fail(vm, "Bit shift by negative number");
    }
    // A right shift by >= 63 is a full arithmetic sign-fill in PHP (0 for
    // non-negative, -1 for negative); Rust's `>>` on i64 is arithmetic, so
    // clamp the shift amount to 63.
    Value::int(a >> b.min(63))
}

fn b_bitnot(vm: &mut VM, _: u8) -> Value {
    let v = vm.pop();
    Value::int(!with_host(|h| h.to_number(&v).to_int()))
}

fn b_spaceship(vm: &mut VM, _: u8) -> Value {
    let b = vm.pop();
    let a = vm.pop();
    Value::int(with_host(|h| php_compare(h, &a, &b)) as i64)
}

/// Per-statement DAP line marker (emitted only under `php --dap`). Pops the line
/// argument and hands control to the debugger, which pauses here at a breakpoint
/// or step target. A normal (non-`--dap`) build never emits this op, so the hook
/// costs nothing outside the debugger.
fn b_dbg_line(vm: &mut VM, _: u8) -> Value {
    let _line = vm.pop();
    crate::dap::on_debug_line(vm);
    Value::Undef
}

// ── stack helpers ──────────────────────────────────────────────────────────

/// Pop `n` values, restoring source (left-to-right) order.
fn pop_args(vm: &mut VM, n: usize) -> Vec<Value> {
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        v.push(vm.pop());
    }
    v.reverse();
    v
}

/// Pop a value used as a name (variable / function name).
fn pop_name(vm: &mut VM) -> String {
    let v = vm.pop();
    match v {
        Value::Str(s) => s.to_string(),
        other => with_host(|h| h.to_str(&other)),
    }
}

/// Record an error and halt the current chunk.
fn fail(vm: &mut VM, msg: impl Into<String>) -> Value {
    with_host(|h| h.set_error(msg));
    vm.ip = vm.chunk.ops.len();
    Value::Undef
}

// ── core builtins ────────────────────────────────────────────────────────────

fn b_echo(vm: &mut VM, argc: u8) -> Value {
    let args = pop_args(vm, argc as usize);
    with_host(|h| {
        for a in &args {
            let s = h.to_str(a);
            h.write_out(&s);
        }
    });
    Value::Undef
}

fn b_getvar(vm: &mut VM, _: u8) -> Value {
    let name = pop_name(vm);
    with_host(|h| h.get_var(&name))
}

fn b_setvar(vm: &mut VM, _: u8) -> Value {
    let val = vm.pop();
    let name = pop_name(vm);
    with_host(|h| h.set_var(&name, val.clone()));
    val
}

fn b_concat(vm: &mut VM, _: u8) -> Value {
    let b = vm.pop();
    let a = vm.pop();
    with_host(|h| Value::str(format!("{}{}", h.to_str(&a), h.to_str(&b))))
}

fn b_truthy(vm: &mut VM, _: u8) -> Value {
    let v = vm.pop();
    Value::bool(with_host(|h| h.is_truthy(&v)))
}

fn b_call(vm: &mut VM, argc: u8) -> Value {
    let args = pop_args(vm, argc as usize - 1);
    let name = pop_name(vm);
    match host::call_function(&name, args) {
        Ok(v) => bubbled(vm, v),
        Err(e) => fail(vm, e),
    }
}

/// Return the call result, or `Undef` if a throw raised inside the callee is now
/// unwinding the caller too (see `bubble_throw`).
fn bubbled(vm: &mut VM, v: Value) -> Value {
    if bubble_throw(vm) {
        Value::Undef
    } else {
        v
    }
}

/// A call with `...$arr` argument unpacking. The stack holds the callee name then
/// one `(is_spread, value)` pair per source argument; a spread pair's value is an
/// array whose elements are flattened, in order, into the positional arguments.
/// Unpacking a non-array is a silent no-op here — real PHP 8 raises a `TypeError`;
/// the scaffold drops it rather than erroring. Spread arrays are flattened
/// positionally (string keys are not turned into named arguments, which the
/// scaffold does not support).
fn b_call_spread(vm: &mut VM, argc: u8) -> Value {
    let pairs = pop_args(vm, argc as usize - 1);
    let name = pop_name(vm);
    let mut args = Vec::with_capacity(pairs.len() / 2);
    with_host(|h| {
        let mut it = pairs.into_iter();
        while let (Some(flag), Some(val)) = (it.next(), it.next()) {
            if h.is_truthy(&flag) {
                if let Some(entries) = h.array_pairs(&val) {
                    args.extend(entries.into_iter().map(|(_, v)| v));
                }
            } else {
                args.push(val);
            }
        }
    });
    match host::call_function(&name, args) {
        Ok(v) => bubbled(vm, v),
        Err(e) => fail(vm, e),
    }
}

/// Create a closure: the first argument is the synthetic definition name, the
/// rest are `(capture-name, captured-value)` pairs read from the current scope.
fn b_mkclosure(vm: &mut VM, argc: u8) -> Value {
    let raw = pop_args(vm, argc as usize);
    let mut it = raw.into_iter();
    let def_name = match it.next() {
        Some(v) => with_host(|h| h.to_str(&v)),
        None => return Value::Undef,
    };
    let mut captured = Vec::new();
    while let Some(k) = it.next() {
        let Some(v) = it.next() else { break };
        let name = with_host(|h| h.to_str(&k));
        captured.push((name, v));
    }
    with_host(|h| h.make_closure(&def_name, captured))
}

/// Call a callable value (`$f(...)`): the callee is under its arguments.
fn b_call_value(vm: &mut VM, argc: u8) -> Value {
    let args = pop_args(vm, argc as usize - 1);
    let callee = vm.pop();
    match host::call_value(callee, args) {
        Ok(v) => bubbled(vm, v),
        Err(e) => fail(vm, e),
    }
}

fn b_mkarray(vm: &mut VM, argc: u8) -> Value {
    let raw = pop_args(vm, argc as usize);
    with_host(|h| {
        let arr = h.new_array();
        let mut it = raw.into_iter();
        while let Some(k) = it.next() {
            let Some(v) = it.next() else { break };
            match k {
                Value::Undef => h.arr_push_auto(&arr, v),
                key => h.arr_set_key(&arr, &key, v),
            }
        }
        arr
    })
}

fn b_index_get(vm: &mut VM, _: u8) -> Value {
    let key = vm.pop();
    let recv = vm.pop();
    with_host(|h| h.index_get(&recv, &key))
}

fn b_index_set(vm: &mut VM, _: u8) -> Value {
    let val = vm.pop();
    let key = vm.pop();
    let name = pop_name(vm);
    with_host(|h| h.index_set_var(&name, &key, val.clone()));
    val
}

fn b_arr_append(vm: &mut VM, _: u8) -> Value {
    let val = vm.pop();
    let name = pop_name(vm);
    with_host(|h| h.append_var(&name, val.clone()));
    val
}

/// `$a[k1]..[kN] = val` — stack `[name, k1..kN, val]`, `N = argc-2 >= 1`.
fn b_set_path(vm: &mut VM, argc: u8) -> Value {
    let val = vm.pop();
    let keys = pop_args(vm, argc as usize - 2);
    let name = pop_name(vm);
    with_host(|h| h.index_set_path(&name, &keys, val.clone()));
    val
}

/// `$a[k1]..[kM][] = val` — stack `[name, k1..kM, val]`, `M = argc-2 >= 0`.
fn b_append_path(vm: &mut VM, argc: u8) -> Value {
    let val = vm.pop();
    let keys = pop_args(vm, argc as usize - 2);
    let name = pop_name(vm);
    with_host(|h| h.append_path(&name, &keys, val.clone()));
    val
}

/// Read `$a[k1]..[kN]` — stack `[name, k1..kN]`, `N = argc-1`. Used for the read
/// half of a compound assignment (`$a[k] += ...`).
fn b_get_path(vm: &mut VM, argc: u8) -> Value {
    let keys = pop_args(vm, argc as usize - 1);
    let name = pop_name(vm);
    with_host(|h| h.index_get_path(&name, &keys))
}

/// `++`/`--` on `$a[k1]..[kN]` — stack `[name, k1..kN, code]`, `N = argc-2`. The
/// `code` bits match `b_incdec` (bit0 = increment, bit1 = prefix). Decrement of an
/// unset/null element yields -1 here (consistent with `b_incdec` on a plain
/// `$var`); real PHP leaves it null — a documented scaffold deviation.
fn b_incdec_path(vm: &mut VM, argc: u8) -> Value {
    let code = vm.pop().to_int();
    let keys = pop_args(vm, argc as usize - 2);
    let name = pop_name(vm);
    let inc = code & 1 != 0;
    let prefix = code & 2 != 0;
    with_host(|h| {
        let old = h.index_get_path(&name, &keys);
        let delta = if inc { 1 } else { -1 };
        let newv = match h.to_number(&old) {
            Value::Int(n) => Value::int(n + delta),
            Value::Float(f) => Value::float(f + delta as f64),
            _ => Value::int(delta),
        };
        h.index_set_path(&name, &keys, newv.clone());
        if prefix {
            newv
        } else {
            old
        }
    })
}

/// Append a fresh child array to `$a[k1]..[kN]` and leave its handle on the stack
/// — stack `[name, k1..kN]`, `N = argc-1`. Lets the compiler pivot a mid-path
/// append (`$a[][k] = v`) onto the new child.
fn b_path_append_child(vm: &mut VM, argc: u8) -> Value {
    let keys = pop_args(vm, argc as usize - 1);
    let name = pop_name(vm);
    with_host(|h| h.path_append_child(&name, &keys))
}

fn b_sig_return(vm: &mut VM, _: u8) -> Value {
    let v = vm.pop();
    host::set_return(v);
    vm.ip = vm.chunk.ops.len();
    Value::Undef
}

fn b_incdec(vm: &mut VM, _: u8) -> Value {
    let code = vm.pop().to_int();
    let name = pop_name(vm);
    let inc = code & 1 != 0;
    let prefix = code & 2 != 0;
    with_host(|h| {
        let old = h.get_var(&name);
        let delta = if inc { 1 } else { -1 };
        let newv = match h.to_number(&old) {
            Value::Int(n) => Value::int(n + delta),
            Value::Float(f) => Value::float(f + delta as f64),
            _ => Value::int(delta),
        };
        h.set_var(&name, newv.clone());
        if prefix {
            newv
        } else {
            old
        }
    })
}

fn b_arraykeys(vm: &mut VM, _: u8) -> Value {
    let recv = vm.pop();
    with_host(|h| h.array_keys(&recv))
}

fn b_arraylen(vm: &mut VM, _: u8) -> Value {
    let recv = vm.pop();
    Value::int(with_host(|h| h.array_len(&recv)))
}

/// The by-reference array mutators (`array_push`/`array_pop`/`array_shift`/
/// `array_unshift`/`array_splice`). The compiler lowers a call whose first
/// argument is a plain `$var` to `[name, subop, args...]` so the host can rewrite
/// the bound array in place (PHP passes the array by reference here).
fn b_arr_mut(vm: &mut VM, argc: u8) -> Value {
    use host::arrmut;
    let mut args = pop_args(vm, argc as usize);
    // args[0] = variable name, args[1] = sub-op; args[2..] = the call arguments.
    let name = with_host(|h| h.to_str(&args[0]));
    let sub = args[1].to_int();
    let extra: Vec<Value> = args.split_off(2);
    with_host(|h| match sub {
        arrmut::PUSH => h.arr_push_var(&name, extra),
        arrmut::POP => h.arr_pop_var(&name),
        arrmut::SHIFT => h.arr_shift_var(&name),
        arrmut::UNSHIFT => h.arr_unshift_var(&name, extra),
        arrmut::SPLICE => h.arr_splice_var(&name, &extra),
        _ => Value::Undef,
    })
}

// ── object builtins (classes / OOP) ──────────────────────────────────────────

fn b_new(vm: &mut VM, argc: u8) -> Value {
    let mut args = pop_args(vm, argc as usize);
    let class = with_host(|h| h.to_str(&args.remove(0)));
    match host::new_object(&class, args) {
        Ok(v) => bubbled(vm, v),
        Err(e) => fail(vm, e),
    }
}

fn b_prop_get(vm: &mut VM, _: u8) -> Value {
    let name = pop_name(vm);
    let recv = vm.pop();
    with_host(|h| h.prop_get(&recv, &name))
}

fn b_prop_set(vm: &mut VM, _: u8) -> Value {
    let val = vm.pop();
    let name = pop_name(vm);
    let recv = vm.pop();
    with_host(|h| h.prop_set(&recv, &name, val.clone()));
    val
}

/// Vivify `$o->name` into an array and leave its handle on the stack — the pivot
/// for indexing/appending into an array-valued property. Stack `[recv, name]`.
fn b_prop_ensure_array(vm: &mut VM, _: u8) -> Value {
    let name = pop_name(vm);
    let recv = vm.pop();
    with_host(|h| h.prop_ensure_array(&recv, &name))
}

/// `++`/`--` on `$o->name` — stack `[recv, name, code]`. The `code` bits match
/// `b_incdec` (bit0 = increment, bit1 = prefix).
fn b_prop_incdec(vm: &mut VM, _: u8) -> Value {
    let code = vm.pop().to_int();
    let name = pop_name(vm);
    let recv = vm.pop();
    let inc = code & 1 != 0;
    let prefix = code & 2 != 0;
    with_host(|h| {
        let old = h.prop_get(&recv, &name);
        let delta = if inc { 1 } else { -1 };
        let newv = match h.to_number(&old) {
            Value::Int(n) => Value::int(n + delta),
            Value::Float(f) => Value::float(f + delta as f64),
            _ => Value::int(delta),
        };
        h.prop_set(&recv, &name, newv.clone());
        if prefix {
            newv
        } else {
            old
        }
    })
}

fn b_mcall(vm: &mut VM, argc: u8) -> Value {
    let mut args = pop_args(vm, argc as usize);
    let recv = args.remove(0);
    let method = with_host(|h| h.to_str(&args.remove(0)));
    let class = with_host(|h| h.object_class(&recv));
    match class {
        Some(c) => match host::call_method(&c, &method, Some(recv), args) {
            Ok(v) => bubbled(vm, v),
            Err(e) => fail(vm, e),
        },
        None => fail(vm, format!("call to a member function {method}() on a non-object")),
    }
}

fn b_scall(vm: &mut VM, argc: u8) -> Value {
    let mut args = pop_args(vm, argc as usize);
    let class = with_host(|h| h.to_str(&args.remove(0)));
    let method = with_host(|h| h.to_str(&args.remove(0)));
    // Forward `$this` when the call is made from an object context (so
    // `parent::m()` / `self::m()` inside a method keep the current instance).
    let this = with_host(|h| {
        let t = h.get_var("this");
        matches!(t, Value::Obj(_)).then_some(t)
    });
    match host::call_method(&class, &method, this, args) {
        Ok(v) => bubbled(vm, v),
        Err(e) => fail(vm, e),
    }
}

fn b_sconst(vm: &mut VM, _: u8) -> Value {
    let name = pop_name(vm);
    let class = pop_name(vm);
    match host::class_const(&class, &name) {
        Ok(v) => v,
        Err(e) => fail(vm, e),
    }
}

// ── arithmetic builtins (PHP semantics) ──────────────────────────────────────

fn b_div(vm: &mut VM, _: u8) -> Value {
    let b = vm.pop();
    let a = vm.pop();
    let (an, bn) = with_host(|h| (h.to_number(&a), h.to_number(&b)));
    if bn.to_float() == 0.0 {
        return fail(vm, "Division by zero");
    }
    match (an, bn) {
        (Value::Int(x), Value::Int(y)) if x % y == 0 => Value::int(x / y),
        (an, bn) => Value::float(an.to_float() / bn.to_float()),
    }
}

fn b_mod(vm: &mut VM, _: u8) -> Value {
    let b = vm.pop();
    let a = vm.pop();
    let (x, y) = with_host(|h| (h.to_number(&a).to_int(), h.to_number(&b).to_int()));
    if y == 0 {
        return fail(vm, "Modulo by zero");
    }
    Value::int(x % y)
}

fn b_pow(vm: &mut VM, _: u8) -> Value {
    let b = vm.pop();
    let a = vm.pop();
    let (an, bn) = with_host(|h| (h.to_number(&a), h.to_number(&b)));
    match (an, bn) {
        (Value::Int(x), Value::Int(y)) if y >= 0 => {
            if let Some(v) = checked_ipow(x, y as u32) {
                Value::int(v)
            } else {
                Value::float((x as f64).powf(y as f64))
            }
        }
        (an, bn) => Value::float(an.to_float().powf(bn.to_float())),
    }
}

fn checked_ipow(mut base: i64, mut exp: u32) -> Option<i64> {
    let mut acc: i64 = 1;
    while exp > 0 {
        if exp & 1 == 1 {
            acc = acc.checked_mul(base)?;
        }
        exp >>= 1;
        if exp > 0 {
            base = base.checked_mul(base)?;
        }
    }
    Some(acc)
}

// ── comparison builtins ──────────────────────────────────────────────────────

fn b_loose_eq(vm: &mut VM, _: u8) -> Value {
    let b = vm.pop();
    let a = vm.pop();
    Value::bool(with_host(|h| loose_eq(h, &a, &b)))
}

fn b_loose_ne(vm: &mut VM, _: u8) -> Value {
    let b = vm.pop();
    let a = vm.pop();
    Value::bool(with_host(|h| !loose_eq(h, &a, &b)))
}

fn b_strict_eq(vm: &mut VM, _: u8) -> Value {
    let b = vm.pop();
    let a = vm.pop();
    Value::bool(with_host(|h| strict_eq(h, &a, &b)))
}

fn b_strict_ne(vm: &mut VM, _: u8) -> Value {
    let b = vm.pop();
    let a = vm.pop();
    Value::bool(with_host(|h| !strict_eq(h, &a, &b)))
}

fn b_lt(vm: &mut VM, _: u8) -> Value {
    cmp_bool(vm, |o| o < 0)
}
fn b_gt(vm: &mut VM, _: u8) -> Value {
    cmp_bool(vm, |o| o > 0)
}
fn b_le(vm: &mut VM, _: u8) -> Value {
    cmp_bool(vm, |o| o <= 0)
}
fn b_ge(vm: &mut VM, _: u8) -> Value {
    cmp_bool(vm, |o| o >= 0)
}

fn cmp_bool(vm: &mut VM, f: impl Fn(i32) -> bool) -> Value {
    let b = vm.pop();
    let a = vm.pop();
    Value::bool(f(with_host(|h| php_compare(h, &a, &b))))
}

/// PHP loose equality (`==`) over the scaffold's value set.
fn loose_eq(h: &host::PhpHost, a: &Value, b: &Value) -> bool {
    use Value::*;
    match (a, b) {
        // A bool operand compares by truthiness.
        (Bool(_), _) | (_, Bool(_)) => h.is_truthy(a) == h.is_truthy(b),
        // null/undef converts based on the other operand's type.
        (Undef, x) | (x, Undef) => match x {
            Int(n) => *n == 0,
            Float(f) => *f == 0.0,
            Str(s) => s.is_empty(),
            Obj(_) => h.array_len(x) == 0,
            Undef => true,
            _ => false,
        },
        (Str(x), Str(y)) => {
            if host::is_numeric_string(x) && host::is_numeric_string(y) {
                num_eq(a, b)
            } else {
                x == y
            }
        }
        (Obj(_), Obj(_)) => arrays_loose_eq(h, a, b),
        (Obj(_), _) | (_, Obj(_)) => false,
        // number vs number, or number vs string.
        _ => {
            // A non-numeric string vs a number compares as strings in PHP 8.
            if let Str(s) = a {
                if !host::is_numeric_string(s) {
                    return h.to_str(a) == h.to_str(b);
                }
            }
            if let Str(s) = b {
                if !host::is_numeric_string(s) {
                    return h.to_str(a) == h.to_str(b);
                }
            }
            num_eq(&h.to_number(a), &h.to_number(b))
        }
    }
}

fn num_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        _ => a.to_float() == b.to_float(),
    }
}

fn arrays_loose_eq(h: &host::PhpHost, a: &Value, b: &Value) -> bool {
    let (Some(pa), Some(pb)) = (h.array_pairs(a), h.array_pairs(b)) else {
        return false;
    };
    if pa.len() != pb.len() {
        return false;
    }
    // `==` on arrays: same key/value pairs, order-independent, values loose-equal.
    pa.iter().all(|(ka, va)| {
        pb.iter()
            .any(|(kb, vb)| strict_eq(h, ka, kb) && loose_eq(h, va, vb))
    })
}

/// PHP strict equality (`===`).
fn strict_eq(h: &host::PhpHost, a: &Value, b: &Value) -> bool {
    use Value::*;
    match (a, b) {
        (Int(x), Int(y)) => x == y,
        (Float(x), Float(y)) => x == y,
        (Str(x), Str(y)) => x == y,
        (Bool(x), Bool(y)) => x == y,
        (Undef, Undef) => true,
        (Obj(_), Obj(_)) => {
            let (Some(pa), Some(pb)) = (h.array_pairs(a), h.array_pairs(b)) else {
                return false;
            };
            pa.len() == pb.len()
                && pa
                    .iter()
                    .zip(pb.iter())
                    .all(|((ka, va), (kb, vb))| strict_eq(h, ka, kb) && strict_eq(h, va, vb))
        }
        _ => false,
    }
}

/// PHP ordering: -1 / 0 / 1. Numeric unless both operands are non-numeric
/// strings (then byte comparison).
fn php_compare(h: &host::PhpHost, a: &Value, b: &Value) -> i32 {
    use Value::*;
    match (a, b) {
        (Str(x), Str(y)) => {
            if host::is_numeric_string(x) && host::is_numeric_string(y) {
                cmp_f64(a.to_float(), b.to_float())
            } else {
                strcmp_i32(x, y)
            }
        }
        // PHP 8: number vs string compares numerically only when the string is
        // numeric; otherwise the number is cast to a string and compared as text
        // (`"abc" <= 10` is false because "abc" > "10" lexically).
        (Str(x), Int(_) | Float(_)) => {
            if host::is_numeric_string(x) {
                cmp_f64(a.to_float(), b.to_float())
            } else {
                strcmp_i32(x, &h.to_str(b))
            }
        }
        (Int(_) | Float(_), Str(y)) => {
            if host::is_numeric_string(y) {
                cmp_f64(a.to_float(), b.to_float())
            } else {
                strcmp_i32(&h.to_str(a), y)
            }
        }
        (Obj(_), Obj(_)) => cmp_f64(h.array_len(a) as f64, h.array_len(b) as f64),
        _ => cmp_f64(h.to_number(a).to_float(), h.to_number(b).to_float()),
    }
}

fn strcmp_i32(x: &str, y: &str) -> i32 {
    match x.cmp(y) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

fn cmp_f64(x: f64, y: f64) -> i32 {
    if x < y {
        -1
    } else if x > y {
        1
    } else {
        0
    }
}

// ── the strict numeric hook ──────────────────────────────────────────────────

/// Supplies PHP arithmetic for the native `Add`/`Sub`/`Mul`/`Negate` ops when an
/// operand is non-numeric (string/array/bool/null) or an `i64` op overflows.
pub fn numeric_hook(op: NumOp, a: &Value, b: &Value) -> Result<Value, String> {
    with_host(|h| {
        let an = h.to_number(a);
        if op == NumOp::Neg {
            return Ok(match an {
                Value::Int(n) => n
                    .checked_neg()
                    .map(Value::int)
                    .unwrap_or(Value::float(-(n as f64))),
                Value::Float(f) => Value::float(-f),
                _ => Value::int(0),
            });
        }
        let bn = h.to_number(b);
        Ok(arith(op, an, bn))
    })
}

fn arith(op: NumOp, an: Value, bn: Value) -> Value {
    if let (Value::Int(x), Value::Int(y)) = (&an, &bn) {
        let r = match op {
            NumOp::Add => x.checked_add(*y),
            NumOp::Sub => x.checked_sub(*y),
            NumOp::Mul => x.checked_mul(*y),
            _ => None,
        };
        if let Some(v) = r {
            return Value::int(v);
        }
    }
    let (x, y) = (an.to_float(), bn.to_float());
    Value::float(match op {
        NumOp::Add => x + y,
        NumOp::Sub => x - y,
        NumOp::Mul => x * y,
        NumOp::Div => x / y,
        NumOp::Mod => x % y,
        NumOp::Pow => x.powf(y),
        _ => 0.0,
    })
}

// ── PHP standard library (reached through CALL) ──────────────────────────────

pub(crate) fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Undef)
}

/// Dispatch a PHP library function by (case-insensitive) name.
pub fn call_library(name: &str, args: Vec<Value>) -> Result<Value, String> {
    let lname = name.to_ascii_lowercase();
    let v = match lname.as_str() {
        "strlen" => Value::int(with_host(|h| h.to_str(&arg(&args, 0)).len() as i64)),
        "count" | "sizeof" => Value::int(with_host(|h| {
            let a = arg(&args, 0);
            if h.is_array(&a) {
                h.array_len(&a)
            } else {
                1
            }
        })),
        "strtoupper" => with_host(|h| Value::str(h.to_str(&arg(&args, 0)).to_uppercase())),
        "strtolower" => with_host(|h| Value::str(h.to_str(&arg(&args, 0)).to_lowercase())),
        "ucfirst" => with_host(|h| Value::str(ucfirst(&h.to_str(&arg(&args, 0))))),
        "trim" => with_host(|h| Value::str(h.to_str(&arg(&args, 0)).trim().to_string())),
        "ltrim" => with_host(|h| Value::str(h.to_str(&arg(&args, 0)).trim_start().to_string())),
        "rtrim" | "chop" => {
            with_host(|h| Value::str(h.to_str(&arg(&args, 0)).trim_end().to_string()))
        }
        "str_repeat" => with_host(|h| {
            let s = h.to_str(&arg(&args, 0));
            let n = arg(&args, 1).to_int().max(0) as usize;
            Value::str(s.repeat(n))
        }),
        "strrev" => {
            with_host(|h| Value::str(h.to_str(&arg(&args, 0)).chars().rev().collect::<String>()))
        }
        "wordwrap" => with_host(|h| php_wordwrap(h, &args)),
        "substr" => with_host(|h| Value::str(php_substr(&h.to_str(&arg(&args, 0)), &args))),
        "strpos" => with_host(|h| php_strpos(h, &args)),
        "str_replace" => with_host(|h| php_str_replace(h, &args)),
        "abs" => with_host(|h| match h.to_number(&arg(&args, 0)) {
            Value::Int(n) => Value::int(n.abs()),
            Value::Float(f) => Value::float(f.abs()),
            other => other,
        }),
        "floor" => with_host(|h| Value::float(h.to_number(&arg(&args, 0)).to_float().floor())),
        "ceil" => with_host(|h| Value::float(h.to_number(&arg(&args, 0)).to_float().ceil())),
        "sqrt" => with_host(|h| Value::float(h.to_number(&arg(&args, 0)).to_float().sqrt())),
        "round" => with_host(|h| {
            let x = h.to_number(&arg(&args, 0)).to_float();
            let p = args.get(1).map(|v| v.to_int()).unwrap_or(0);
            let m = 10f64.powi(p as i32);
            Value::float((x * m).round() / m)
        }),
        "intval" => with_host(|h| Value::int(h.to_number(&arg(&args, 0)).to_int())),
        "floatval" | "doubleval" => {
            with_host(|h| Value::float(h.to_number(&arg(&args, 0)).to_float()))
        }
        "strval" => with_host(|h| Value::str(h.to_str(&arg(&args, 0)))),
        "max" => with_host(|h| fold_cmp(h, &args, true)),
        "min" => with_host(|h| fold_cmp(h, &args, false)),
        "gettype" => with_host(|h| Value::str(h.type_name(&arg(&args, 0)).to_string())),
        "is_array" => with_host(|h| Value::bool(h.is_array(&arg(&args, 0)))),
        "is_int" | "is_integer" | "is_long" => Value::bool(matches!(arg(&args, 0), Value::Int(_))),
        "is_float" | "is_double" => Value::bool(matches!(arg(&args, 0), Value::Float(_))),
        "is_string" => Value::bool(matches!(arg(&args, 0), Value::Str(_))),
        "is_bool" => Value::bool(matches!(arg(&args, 0), Value::Bool(_))),
        "is_null" => Value::bool(matches!(arg(&args, 0), Value::Undef)),
        "is_numeric" => Value::bool(match arg(&args, 0) {
            Value::Int(_) | Value::Float(_) => true,
            Value::Str(s) => host::is_numeric_string(&s),
            _ => false,
        }),
        "implode" | "join" => with_host(|h| php_implode(h, &args)),
        "explode" => with_host(|h| php_explode(h, &args)),
        "in_array" => with_host(|h| php_in_array(h, &args)),
        "array_keys" => with_host(|h| h.array_keys(&arg(&args, 0))),
        "array_values" => with_host(|h| php_array_values(h, &arg(&args, 0))),
        "array_push" => with_host(|h| php_array_push(h, &args)),
        "range" => with_host(|h| php_range(h, &args)),
        "sprintf" => with_host(|h| Value::str(php_sprintf(h, &args))),
        "printf" => with_host(|h| {
            let s = php_sprintf(h, &args);
            h.write_out(&s);
            Value::int(s.len() as i64)
        }),
        "print_r" => with_host(|h| {
            let s = php_print_r(h, &arg(&args, 0), 0);
            if args.get(1).map(|v| h.is_truthy(v)).unwrap_or(false) {
                Value::str(s)
            } else {
                h.write_out(&s);
                Value::bool(true)
            }
        }),
        "var_dump" => with_host(|h| {
            for a in &args {
                let s = php_var_dump(h, a, 0);
                h.write_out(&s);
            }
            Value::Undef
        }),

        // ── strings ──────────────────────────────────────────────────────
        "str_split" => with_host(|h| php_str_split(h, &args)),
        "str_pad" => with_host(|h| Value::str(php_str_pad(h, &args))),
        "str_contains" => {
            with_host(|h| Value::bool(h.to_str(&arg(&args, 0)).contains(&h.to_str(&arg(&args, 1)))))
        }
        "str_starts_with" => with_host(|h| {
            Value::bool(
                h.to_str(&arg(&args, 0))
                    .starts_with(&h.to_str(&arg(&args, 1))),
            )
        }),
        "str_ends_with" => with_host(|h| {
            Value::bool(
                h.to_str(&arg(&args, 0))
                    .ends_with(&h.to_str(&arg(&args, 1))),
            )
        }),
        "ucwords" => with_host(|h| Value::str(ucwords(&h.to_str(&arg(&args, 0))))),
        "lcfirst" => with_host(|h| Value::str(lcfirst(&h.to_str(&arg(&args, 0))))),
        "number_format" => with_host(|h| Value::str(php_number_format(h, &args))),
        "htmlspecialchars" | "htmlentities" => {
            with_host(|h| Value::str(html_special_chars(&h.to_str(&arg(&args, 0)))))
        }
        "strcmp" => with_host(|h| {
            Value::int(sign(
                h.to_str(&arg(&args, 0)).cmp(&h.to_str(&arg(&args, 1))),
            ))
        }),
        "strcasecmp" => with_host(|h| {
            let a = h.to_str(&arg(&args, 0)).to_lowercase();
            let b = h.to_str(&arg(&args, 1)).to_lowercase();
            Value::int(sign(a.cmp(&b)))
        }),
        "strncmp" => with_host(|h| {
            let a = h.to_str(&arg(&args, 0));
            let b = h.to_str(&arg(&args, 1));
            let n = arg(&args, 2).to_int().max(0) as usize;
            let (ab, bb) = (a.as_bytes(), b.as_bytes());
            Value::int(sign(ab[..n.min(ab.len())].cmp(&bb[..n.min(bb.len())])))
        }),
        "substr_compare" => with_host(|h| php_substr_compare(h, &args)),
        // `str_word_count` is served by `stdlib::misc`, which implements the
        // `$format`/`$characters` arguments; it is intentionally not handled here
        // so the full version wins (this core arm was a count-only stub).
        "chr" => with_host(|h| {
            let n = (h.to_number(&arg(&args, 0)).to_int().rem_euclid(256)) as u8;
            Value::str((n as char).to_string())
        }),
        "ord" => {
            with_host(|h| Value::int(h.to_str(&arg(&args, 0)).bytes().next().unwrap_or(0) as i64))
        }
        "dechex" => {
            with_host(|h| Value::str(format!("{:x}", h.to_number(&arg(&args, 0)).to_int())))
        }
        "hexdec" => with_host(|h| {
            Value::int(i64::from_str_radix(h.to_str(&arg(&args, 0)).trim(), 16).unwrap_or(0))
        }),
        "bin2hex" => with_host(|h| {
            let s = h.to_str(&arg(&args, 0));
            Value::str(s.bytes().map(|b| format!("{b:02x}")).collect::<String>())
        }),

        // ── math ─────────────────────────────────────────────────────────
        "pow" => with_host(|h| php_pow(h, &args)),
        "intdiv" => return php_intdiv(&args),
        "fmod" => with_host(|h| {
            Value::float(
                h.to_number(&arg(&args, 0)).to_float() % h.to_number(&arg(&args, 1)).to_float(),
            )
        }),
        "sin" => with_host(|h| Value::float(h.to_number(&arg(&args, 0)).to_float().sin())),
        "cos" => with_host(|h| Value::float(h.to_number(&arg(&args, 0)).to_float().cos())),
        "tan" => with_host(|h| Value::float(h.to_number(&arg(&args, 0)).to_float().tan())),
        "exp" => with_host(|h| Value::float(h.to_number(&arg(&args, 0)).to_float().exp())),
        "log" => with_host(|h| {
            let x = h.to_number(&arg(&args, 0)).to_float();
            match args.get(1) {
                Some(b) if !matches!(b, Value::Undef) => {
                    Value::float(x.log(h.to_number(b).to_float()))
                }
                _ => Value::float(x.ln()),
            }
        }),
        "log10" => with_host(|h| Value::float(h.to_number(&arg(&args, 0)).to_float().log10())),
        "pi" => Value::float(std::f64::consts::PI),

        // ── arrays ───────────────────────────────────────────────────────
        "array_merge" => with_host(|h| php_array_merge(h, &args)),
        "array_map" => return php_array_map(&args),
        "array_filter" => return php_array_filter(&args),
        "array_reduce" => return php_array_reduce(&args),
        "array_slice" => with_host(|h| php_array_slice(h, &args)),
        "array_reverse" => with_host(|h| php_array_reverse(h, &args)),
        "array_sum" => with_host(|h| php_array_fold(h, &arg(&args, 0), false)),
        "array_product" => with_host(|h| php_array_fold(h, &arg(&args, 0), true)),
        "array_flip" => with_host(|h| php_array_flip(h, &arg(&args, 0))),
        "array_unique" => with_host(|h| php_array_unique(h, &arg(&args, 0))),
        "array_key_exists" | "key_exists" => with_host(|h| php_array_key_exists(h, &args)),
        "array_search" => with_host(|h| php_array_search(h, &args)),
        "sort" => with_host(|h| php_sort(h, &arg(&args, 0), false)),
        "rsort" => with_host(|h| php_sort(h, &arg(&args, 0), true)),
        "asort" => with_host(|h| php_asort(h, &arg(&args, 0), false)),
        "arsort" => with_host(|h| php_asort(h, &arg(&args, 0), true)),
        "ksort" => with_host(|h| php_ksort(h, &arg(&args, 0), false)),
        "krsort" => with_host(|h| php_ksort(h, &arg(&args, 0), true)),
        "array_fill" => with_host(|h| php_array_fill(h, &args)),
        "array_combine" => with_host(|h| php_array_combine(h, &args)),
        "array_diff" => with_host(|h| php_array_diff(h, &args, false)),
        "array_intersect" => with_host(|h| php_array_diff(h, &args, true)),

        // ── type / util ──────────────────────────────────────────────────
        "boolval" => with_host(|h| Value::bool(h.is_truthy(&arg(&args, 0)))),
        "is_callable" => {
            // A closure handle is always callable; otherwise the scaffold treats
            // any non-empty string name as callable (the host exposes no
            // function-table accessor, so it cannot verify the name resolves).
            let a = arg(&args, 0);
            Value::bool(
                with_host(|h| h.is_closure(&a)) || matches!(&a, Value::Str(s) if !s.is_empty()),
            )
        }
        "var_export" => with_host(|h| {
            let s = php_var_export(h, &arg(&args, 0), 0);
            if args.get(1).map(|v| h.is_truthy(v)).unwrap_or(false) {
                Value::str(s)
            } else {
                h.write_out(&s);
                Value::Undef
            }
        }),
        "json_encode" => with_host(|h| Value::str(php_json_encode(h, &arg(&args, 0)))),

        // Extended standard library lives in `src/stdlib/*`, one module per
        // category, consulted only for names this core match does not handle.
        _ => {
            return crate::stdlib::dispatch(&lname, &args)
                .unwrap_or_else(|| Err(format!("call to undefined function {name}()")))
        }
    };
    Ok(v)
}

fn ucfirst(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

fn fold_cmp(h: &host::PhpHost, args: &[Value], want_max: bool) -> Value {
    // max/min accept either a single array or a variadic list.
    let items: Vec<Value> = if args.len() == 1 && h.is_array(&args[0]) {
        h.array_pairs(&args[0])
            .unwrap_or_default()
            .into_iter()
            .map(|(_, v)| v)
            .collect()
    } else {
        args.to_vec()
    };
    let mut best: Option<Value> = None;
    for v in items {
        best = Some(match best {
            None => v,
            Some(cur) => {
                let ord = php_compare(h, &v, &cur);
                if (want_max && ord > 0) || (!want_max && ord < 0) {
                    v
                } else {
                    cur
                }
            }
        });
    }
    best.unwrap_or(Value::Undef)
}

fn php_substr(s: &str, args: &[Value]) -> String {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as i64;
    let mut start = arg(args, 1).to_int();
    if start < 0 {
        start = (len + start).max(0);
    }
    let start = start.min(len).max(0) as usize;
    let count = match args.get(2) {
        Some(v) if !matches!(v, Value::Undef) => {
            let l = v.to_int();
            if l < 0 {
                (len - start as i64 + l).max(0) as usize
            } else {
                l as usize
            }
        }
        _ => chars.len() - start,
    };
    chars[start..(start + count).min(chars.len())]
        .iter()
        .collect()
}

fn php_strpos(h: &host::PhpHost, args: &[Value]) -> Value {
    let hay = h.to_str(&arg(args, 0));
    let needle = h.to_str(&arg(args, 1));
    match hay.find(&needle) {
        Some(byte_idx) => Value::int(hay[..byte_idx].chars().count() as i64),
        None => Value::bool(false),
    }
}

fn php_str_replace(h: &host::PhpHost, args: &[Value]) -> Value {
    let search = h.to_str(&arg(args, 0));
    let replace = h.to_str(&arg(args, 1));
    let subject = h.to_str(&arg(args, 2));
    Value::str(subject.replace(&search, &replace))
}

fn php_implode(h: &mut host::PhpHost, args: &[Value]) -> Value {
    // implode($glue, $array) or implode($array).
    let (glue, arr) = if h.is_array(&arg(args, 0)) {
        (String::new(), arg(args, 0))
    } else {
        (h.to_str(&arg(args, 0)), arg(args, 1))
    };
    let parts: Vec<String> = h
        .array_pairs(&arr)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, v)| h.to_str(&v))
        .collect();
    Value::str(parts.join(&glue))
}

fn php_explode(h: &mut host::PhpHost, args: &[Value]) -> Value {
    let sep = h.to_str(&arg(args, 0));
    let subject = h.to_str(&arg(args, 1));
    let arr = h.new_array();
    if sep.is_empty() {
        h.arr_push_auto(&arr, Value::str(subject));
        return arr;
    }
    for part in subject.split(&sep) {
        h.arr_push_auto(&arr, Value::str(part.to_string()));
    }
    arr
}

fn php_in_array(h: &host::PhpHost, args: &[Value]) -> Value {
    let needle = arg(args, 0);
    let hay = arg(args, 1);
    let strict = args.get(2).map(|v| h.is_truthy(v)).unwrap_or(false);
    let found = h
        .array_pairs(&hay)
        .unwrap_or_default()
        .iter()
        .any(|(_, v)| {
            if strict {
                strict_eq(h, &needle, v)
            } else {
                loose_eq(h, &needle, v)
            }
        });
    Value::bool(found)
}

fn php_array_values(h: &mut host::PhpHost, v: &Value) -> Value {
    let pairs = h.array_pairs(v).unwrap_or_default();
    let arr = h.new_array();
    for (_, val) in pairs {
        h.arr_push_auto(&arr, val);
    }
    arr
}

fn php_array_push(h: &mut host::PhpHost, args: &[Value]) -> Value {
    let arr = arg(args, 0);
    for v in &args[1.min(args.len())..] {
        h.arr_push_auto(&arr, v.clone());
    }
    Value::int(h.array_len(&arr))
}

fn php_range(h: &mut host::PhpHost, args: &[Value]) -> Value {
    let start = h.to_number(&arg(args, 0));
    let end = h.to_number(&arg(args, 1));
    let arr = h.new_array();
    // Integer range only in the scaffold.
    let (mut i, e) = (start.to_int(), end.to_int());
    let step = if i <= e { 1 } else { -1 };
    loop {
        h.arr_push_auto(&arr, Value::int(i));
        if i == e {
            break;
        }
        i += step;
    }
    arr
}

/// A parsed conversion spec `%[argnum$][flags][width][.precision]conv`.
struct FmtSpec {
    argnum: Option<usize>,
    left: bool,
    plus: bool,
    space: bool,
    pad: char,
    width: usize,
    precision: Option<usize>,
    conv: char,
}

/// `sprintf`: a format engine covering PHP's flags (`- + 0 ' `), width, precision,
/// positional args (`%2$s`), and the `d i u f F e E g G s x X o b c %` conversions.
fn php_sprintf(h: &host::PhpHost, args: &[Value]) -> String {
    let fmt: Vec<char> = h.to_str(&arg(args, 0)).chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut next_arg = 1usize;
    while i < fmt.len() {
        if fmt[i] != '%' {
            out.push(fmt[i]);
            i += 1;
            continue;
        }
        i += 1;
        if i < fmt.len() && fmt[i] == '%' {
            out.push('%');
            i += 1;
            continue;
        }
        let Some(spec) = parse_spec(&fmt, &mut i) else {
            out.push('%');
            continue;
        };
        let ai = spec.argnum.unwrap_or_else(|| {
            let a = next_arg;
            next_arg += 1;
            a
        });
        out.push_str(&render_spec(h, &spec, &arg(args, ai)));
    }
    out
}

/// Parse one conversion spec, advancing `i` past it. Returns `None` (and leaves
/// `i` unmoved past a stray `%`) if the spec is malformed.
fn parse_spec(fmt: &[char], i: &mut usize) -> Option<FmtSpec> {
    let mut j = *i;
    // Positional `N$`.
    let mut argnum = None;
    let mut k = j;
    while k < fmt.len() && fmt[k].is_ascii_digit() {
        k += 1;
    }
    if k > j && k < fmt.len() && fmt[k] == '$' {
        argnum = fmt[j..k].iter().collect::<String>().parse::<usize>().ok();
        j = k + 1;
    }
    // Flags.
    let (mut left, mut plus, mut space, mut pad) = (false, false, false, ' ');
    loop {
        match fmt.get(j) {
            Some('-') => left = true,
            Some('+') => plus = true,
            Some(' ') => space = true,
            Some('0') => pad = '0',
            Some('\'') => {
                pad = *fmt.get(j + 1)?;
                j += 2;
                continue;
            }
            _ => break,
        }
        j += 1;
    }
    // Width.
    let mut width = 0usize;
    while let Some(d) = fmt.get(j).filter(|c| c.is_ascii_digit()) {
        width = width * 10 + (*d as usize - '0' as usize);
        j += 1;
    }
    // Precision.
    let mut precision = None;
    if fmt.get(j) == Some(&'.') {
        j += 1;
        let mut p = 0usize;
        while let Some(d) = fmt.get(j).filter(|c| c.is_ascii_digit()) {
            p = p * 10 + (*d as usize - '0' as usize);
            j += 1;
        }
        precision = Some(p);
    }
    let conv = *fmt.get(j)?;
    j += 1;
    *i = j;
    Some(FmtSpec {
        argnum,
        left,
        plus,
        space,
        pad,
        width,
        precision,
        conv,
    })
}

/// Render one parsed spec against its argument value.
fn render_spec(h: &host::PhpHost, s: &FmtSpec, v: &Value) -> String {
    // `body` = the value with sign but no field padding; `is_num` gates
    // zero-padding-after-sign.
    let (body, is_num) = match s.conv {
        'd' | 'i' => {
            let n = h.to_number(v).to_int();
            (signed(n.unsigned_abs().to_string(), n < 0, s), true)
        }
        'u' => ((h.to_number(v).to_int() as u64).to_string(), true),
        'b' => (
            (h.to_number(v).to_int() as u64).pipe(|u| format!("{u:b}")),
            true,
        ),
        'o' => (
            (h.to_number(v).to_int() as u64).pipe(|u| format!("{u:o}")),
            true,
        ),
        'x' => (
            (h.to_number(v).to_int() as u64).pipe(|u| format!("{u:x}")),
            true,
        ),
        'X' => (
            (h.to_number(v).to_int() as u64).pipe(|u| format!("{u:X}")),
            true,
        ),
        'c' => (
            char::from_u32(h.to_number(v).to_int() as u32 & 0xff)
                .map(|c| c.to_string())
                .unwrap_or_default(),
            false,
        ),
        'f' | 'F' => {
            let f = h.to_number(v).to_float();
            let p = s.precision.unwrap_or(6);
            (
                signed(format!("{:.*}", p, f.abs()), f.is_sign_negative(), s),
                true,
            )
        }
        'e' | 'E' => {
            let f = h.to_number(v).to_float();
            (fmt_exp(f, s.precision.unwrap_or(6), s.conv == 'E'), true)
        }
        'g' | 'G' => {
            let f = h.to_number(v).to_float();
            let p = s.precision.unwrap_or(6).max(1);
            let g = host::php_gcvt(f, p);
            (if s.conv == 'g' { g.to_lowercase() } else { g }, true)
        }
        's' => {
            let mut txt = h.to_str(v);
            if let Some(p) = s.precision {
                txt = txt.chars().take(p).collect();
            }
            (txt, false)
        }
        other => return format!("%{other}"),
    };
    pad_field(body, s, is_num)
}

/// Prefix a magnitude string with the correct sign per the `+`/` ` flags.
fn signed(mag: String, neg: bool, s: &FmtSpec) -> String {
    if neg {
        format!("-{mag}")
    } else if s.plus {
        format!("+{mag}")
    } else if s.space {
        format!(" {mag}")
    } else {
        mag
    }
}

/// PHP `%e`: `d.dddddde±d`, exponent always signed with at least one digit.
fn fmt_exp(f: f64, prec: usize, upper: bool) -> String {
    let raw = format!("{:.*e}", prec, f);
    let (mant, ex) = raw.split_once('e').unwrap_or((raw.as_str(), "0"));
    let exp_n: i32 = ex.parse().unwrap_or(0);
    let e = if upper { 'E' } else { 'e' };
    format!(
        "{mant}{e}{}{}",
        if exp_n < 0 { "-" } else { "+" },
        exp_n.abs()
    )
}

/// Apply width/justification/pad to a rendered body.
fn pad_field(body: String, s: &FmtSpec, is_num: bool) -> String {
    let len = body.chars().count();
    if len >= s.width {
        return body;
    }
    let fill = s.width - len;
    if s.left {
        // Left-justified fields always pad with spaces on the right.
        format!("{body}{}", " ".repeat(fill))
    } else if s.pad == '0' && is_num {
        // Zero-pad after any leading sign character.
        let mut chars = body.chars();
        match body.chars().next() {
            Some(sign @ ('-' | '+' | ' ')) => {
                chars.next();
                format!("{sign}{}{}", "0".repeat(fill), chars.as_str())
            }
            _ => format!("{}{body}", "0".repeat(fill)),
        }
    } else {
        format!("{}{body}", s.pad.to_string().repeat(fill))
    }
}

/// `wordwrap($str, $width = 75, $break = "\n", $cut = false)`.
fn php_wordwrap(h: &host::PhpHost, args: &[Value]) -> Value {
    let text = h.to_str(&arg(args, 0));
    let width = args.get(1).map(|v| v.to_int()).unwrap_or(75).max(1) as usize;
    let brk = args
        .get(2)
        .map(|v| h.to_str(v))
        .unwrap_or_else(|| "\n".to_string());
    let cut = args.get(3).map(|v| h.is_truthy(v)).unwrap_or(false);

    let mut out = String::new();
    for (li, line) in text.split('\n').enumerate() {
        if li > 0 {
            out.push('\n');
        }
        let mut cur = 0usize; // chars on the current output line
        let mut first = true;
        for word in line.split(' ') {
            let wlen = word.chars().count();
            if !first {
                if cur + 1 + wlen <= width {
                    out.push(' ');
                    cur += 1;
                } else {
                    out.push_str(&brk);
                    cur = 0;
                }
            }
            first = false;
            if cut && wlen > width {
                // Break the long word into width-sized pieces.
                let chars: Vec<char> = word.chars().collect();
                let mut idx = 0;
                while idx < chars.len() {
                    if cur == width {
                        out.push_str(&brk);
                        cur = 0;
                    }
                    let take = (width - cur).min(chars.len() - idx);
                    out.extend(&chars[idx..idx + take]);
                    cur += take;
                    idx += take;
                }
            } else {
                out.push_str(word);
                cur += wlen;
            }
        }
    }
    Value::str(out)
}

/// Tiny postfix-apply helper so integer→radix formatting reads left-to-right.
trait Pipe: Sized {
    fn pipe<R>(self, f: impl FnOnce(Self) -> R) -> R {
        f(self)
    }
}
impl Pipe for u64 {}

/// `print_r` rendering (arrays one level indented, as PHP).
fn php_print_r(h: &host::PhpHost, v: &Value, depth: usize) -> String {
    if let Some(pairs) = h.array_pairs(v) {
        let pad = "    ".repeat(depth);
        let inner = "    ".repeat(depth + 1);
        let mut s = format!("Array\n{pad}(\n");
        for (k, val) in pairs {
            s.push_str(&format!(
                "{inner}[{}] => {}\n",
                h.to_str(&k),
                php_print_r(h, &val, depth + 2)
            ));
        }
        s.push_str(&format!("{pad})\n"));
        s
    } else {
        h.to_str(v)
    }
}

/// `var_dump` rendering for scalars and one level of arrays.
fn php_var_dump(h: &host::PhpHost, v: &Value, depth: usize) -> String {
    let pad = "  ".repeat(depth);
    match v {
        Value::Undef => format!("{pad}NULL\n"),
        Value::Bool(b) => format!("{pad}bool({})\n", if *b { "true" } else { "false" }),
        Value::Int(n) => format!("{pad}int({n})\n"),
        Value::Float(_) => format!("{pad}float({})\n", h.to_str(v)),
        Value::Str(s) => format!("{pad}string({}) \"{s}\"\n", s.len()),
        Value::Obj(_) => {
            let pairs = h.array_pairs(v).unwrap_or_default();
            let mut s = format!("{pad}array({}) {{\n", pairs.len());
            for (k, val) in pairs {
                let key = match k {
                    Value::Int(n) => format!("{}  [{n}]=>\n", "  ".repeat(depth)),
                    other => format!("{}  [\"{}\"]=>\n", "  ".repeat(depth), h.to_str(&other)),
                };
                s.push_str(&key);
                s.push_str(&php_var_dump(h, &val, depth + 1));
            }
            s.push_str(&format!("{pad}}}\n"));
            s
        }
        _ => format!("{pad}NULL\n"),
    }
}

// ── string helpers (added stdlib wave) ───────────────────────────────────────

/// Sign of an ordering as PHP's strcmp-style -1/0/1.
fn sign(o: std::cmp::Ordering) -> i64 {
    match o {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// `substr_compare($main, $str, $offset, $length = null)` — compares `$str` with
/// the substring of `$main` beginning at `$offset` (negatives count from the
/// end), over at most `$length` bytes. Byte-accurate, case-sensitive.
fn php_substr_compare(h: &host::PhpHost, args: &[Value]) -> Value {
    let main = h.to_str(&arg(args, 0));
    let needle = h.to_str(&arg(args, 1));
    let mb = main.as_bytes();
    let len = mb.len() as i64;
    let mut off = arg(args, 2).to_int();
    if off < 0 {
        off += len;
    }
    let off = off.clamp(0, len) as usize;
    let hay = &mb[off..];
    let nb = needle.as_bytes();
    let (a, b): (&[u8], &[u8]) = match args.get(3) {
        Some(v) if !matches!(v, Value::Undef) => {
            let l = v.to_int().max(0) as usize;
            (&hay[..l.min(hay.len())], &nb[..l.min(nb.len())])
        }
        _ => (hay, nb),
    };
    Value::int(sign(a.cmp(b)))
}

fn php_str_split(h: &mut host::PhpHost, args: &[Value]) -> Value {
    let s = h.to_str(&arg(args, 0));
    let len = args.get(1).map(|v| v.to_int()).unwrap_or(1).max(1) as usize;
    let chars: Vec<char> = s.chars().collect();
    let arr = h.new_array();
    if chars.is_empty() {
        // PHP returns [""] for an empty subject.
        h.arr_push_auto(&arr, Value::str(String::new()));
        return arr;
    }
    for chunk in chars.chunks(len) {
        h.arr_push_auto(&arr, Value::str(chunk.iter().collect::<String>()));
    }
    arr
}

fn php_str_pad(h: &host::PhpHost, args: &[Value]) -> String {
    let s = h.to_str(&arg(args, 0));
    let target = arg(args, 1).to_int();
    let pad = args
        .get(2)
        .map(|v| h.to_str(v))
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| " ".to_string());
    // STR_PAD_RIGHT=1 (default), STR_PAD_LEFT=0, STR_PAD_BOTH=2.
    let ty = args.get(3).map(|v| v.to_int()).unwrap_or(1);
    let cur = s.chars().count() as i64;
    if target <= cur {
        return s;
    }
    let need = (target - cur) as usize;
    let make = |n: usize| -> String { pad.chars().cycle().take(n).collect::<String>() };
    match ty {
        0 => format!("{}{}", make(need), s),
        2 => {
            let left = need / 2;
            let right = need - left;
            format!("{}{}{}", make(left), s, make(right))
        }
        _ => format!("{}{}", s, make(need)),
    }
}

fn ucwords(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut cap = true;
    for c in s.chars() {
        if cap && c.is_alphabetic() {
            out.extend(c.to_uppercase());
            cap = false;
        } else {
            out.push(c);
            if c.is_whitespace() {
                cap = true;
            }
        }
    }
    out
}

fn lcfirst(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_lowercase().chain(c).collect(),
        None => String::new(),
    }
}

fn html_special_chars(s: &str) -> String {
    // ENT_QUOTES-equivalent: escape &, <, >, ", and '.
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#039;")
}

fn php_number_format(h: &host::PhpHost, args: &[Value]) -> String {
    let num = h.to_number(&arg(args, 0)).to_float();
    let dec = args.get(1).map(|v| v.to_int()).unwrap_or(0).max(0) as usize;
    let dp = args
        .get(2)
        .map(|v| h.to_str(v))
        .unwrap_or_else(|| ".".to_string());
    let ts = args
        .get(3)
        .map(|v| h.to_str(v))
        .unwrap_or_else(|| ",".to_string());
    let neg = num < 0.0;
    // PHP rounds half away from zero; pre-round so Rust's round-half-to-even
    // formatting can't turn 100.25 into "100.2" where PHP gives "100.3".
    let m = 10f64.powi(dec as i32);
    let rounded = (num.abs() * m).round() / m;
    let formatted = format!("{:.*}", dec, rounded);
    let (int_part, frac_part) = match formatted.split_once('.') {
        Some((i, f)) => (i.to_string(), f.to_string()),
        None => (formatted.clone(), String::new()),
    };
    // Group the integer part into threes.
    let bytes: Vec<char> = int_part.chars().collect();
    let mut grouped = String::new();
    for (i, c) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            grouped.push_str(&ts);
        }
        grouped.push(*c);
    }
    let mut out = String::new();
    if neg && (grouped.chars().any(|c| c != '0') || frac_part.chars().any(|c| c != '0')) {
        out.push('-');
    }
    out.push_str(&grouped);
    if dec > 0 {
        out.push_str(&dp);
        out.push_str(&frac_part);
    }
    out
}

// ── math helpers ─────────────────────────────────────────────────────────────

fn php_pow(h: &host::PhpHost, args: &[Value]) -> Value {
    let an = h.to_number(&arg(args, 0));
    let bn = h.to_number(&arg(args, 1));
    match (an, bn) {
        (Value::Int(x), Value::Int(y)) if y >= 0 => match checked_ipow(x, y as u32) {
            Some(v) => Value::int(v),
            None => Value::float((x as f64).powf(y as f64)),
        },
        (an, bn) => Value::float(an.to_float().powf(bn.to_float())),
    }
}

fn php_intdiv(args: &[Value]) -> Result<Value, String> {
    let (x, y) = with_host(|h| {
        (
            h.to_number(&arg(args, 0)).to_int(),
            h.to_number(&arg(args, 1)).to_int(),
        )
    });
    if y == 0 {
        return Err("Division by zero".to_string());
    }
    Ok(Value::int(x / y))
}

// ── array helpers (added stdlib wave) ────────────────────────────────────────

/// Whether normalized keys form the list 0,1,2,… (so JSON/`array_merge` treat it
/// as a sequential array rather than a map).
fn is_list(pairs: &[(Value, Value)]) -> bool {
    pairs
        .iter()
        .enumerate()
        .all(|(i, (k, _))| matches!(k, Value::Int(n) if *n == i as i64))
}

fn php_array_merge(h: &mut host::PhpHost, args: &[Value]) -> Value {
    let all: Vec<Vec<(Value, Value)>> = args.iter().filter_map(|a| h.array_pairs(a)).collect();
    let out = h.new_array();
    for pairs in all {
        for (k, v) in pairs {
            match k {
                // Integer keys are renumbered; string keys overwrite.
                Value::Int(_) => h.arr_push_auto(&out, v),
                _ => h.arr_set_key(&out, &k, v),
            }
        }
    }
    out
}

/// Whether `cb` names or holds something callable — a non-empty function-name
/// string or a closure handle. A non-callable callback drives the "identity"
/// paths of `array_map`/`array_filter`.
fn is_callable_arg(cb: &Value) -> bool {
    match cb {
        Value::Str(s) => !s.is_empty(),
        v => with_host(|h| h.is_closure(v)),
    }
}

fn php_array_map(args: &[Value]) -> Result<Value, String> {
    let cb = arg(args, 0);
    let callable = is_callable_arg(&cb);

    // Single-array form preserves keys; the callback (or identity) maps values.
    if args.len() <= 2 {
        let arr = arg(args, 1);
        let pairs = with_host(|h| h.array_pairs(&arr)).unwrap_or_default();
        let mut mapped: Vec<(Value, Value)> = Vec::with_capacity(pairs.len());
        for (k, v) in pairs {
            let out = if callable {
                host::call_value(cb.clone(), vec![v])?
            } else {
                v
            };
            // A callback that threw stops the walk; the pending exception unwinds
            // through the `array_map(...)` call site.
            if host::has_pending_throw() {
                return Ok(Value::Undef);
            }
            mapped.push((k, out));
        }
        return Ok(with_host(|h| {
            let out = h.new_array();
            for (k, v) in mapped {
                h.arr_set_key(&out, &k, v);
            }
            out
        }));
    }

    // Multi-array form: iterate up to the longest input by position (keys are
    // dropped, 0-based). With a null callback the rows are zipped into arrays.
    let arrays: Vec<Vec<Value>> = args[1..]
        .iter()
        .map(|a| {
            with_host(|h| h.array_pairs(a))
                .unwrap_or_default()
                .into_iter()
                .map(|(_, v)| v)
                .collect()
        })
        .collect();
    let len = arrays.iter().map(|a| a.len()).max().unwrap_or(0);
    let mut result: Vec<Value> = Vec::with_capacity(len);
    for i in 0..len {
        let row: Vec<Value> = arrays
            .iter()
            .map(|a| a.get(i).cloned().unwrap_or(Value::Undef))
            .collect();
        if callable {
            let out = host::call_value(cb.clone(), row)?;
            if host::has_pending_throw() {
                return Ok(Value::Undef);
            }
            result.push(out);
        } else {
            // Null callback: zip each position into a sub-array.
            result.push(with_host(|h| {
                let sub = h.new_array();
                for v in row {
                    h.arr_push_auto(&sub, v);
                }
                sub
            }));
        }
    }
    Ok(with_host(|h| {
        let out = h.new_array();
        for v in result {
            h.arr_push_auto(&out, v);
        }
        out
    }))
}

fn php_array_filter(args: &[Value]) -> Result<Value, String> {
    let arr = arg(args, 0);
    let cb = arg(args, 1);
    let pairs = with_host(|h| h.array_pairs(&arr)).unwrap_or_default();
    let callable = is_callable_arg(&cb);
    let mut kept: Vec<(Value, Value)> = Vec::new();
    for (k, v) in pairs {
        let keep = if callable {
            let r = host::call_value(cb.clone(), vec![v.clone()])?;
            if host::has_pending_throw() {
                return Ok(Value::Undef);
            }
            with_host(|h| h.is_truthy(&r))
        } else {
            with_host(|h| h.is_truthy(&v))
        };
        if keep {
            kept.push((k, v));
        }
    }
    Ok(with_host(|h| {
        let out = h.new_array();
        for (k, v) in kept {
            h.arr_set_key(&out, &k, v);
        }
        out
    }))
}

fn php_array_reduce(args: &[Value]) -> Result<Value, String> {
    let arr = arg(args, 0);
    let cb = arg(args, 1);
    let init = arg(args, 2);
    let pairs = with_host(|h| h.array_pairs(&arr)).unwrap_or_default();
    if !is_callable_arg(&cb) {
        return Ok(init);
    }
    let mut acc = init;
    for (_, v) in pairs {
        acc = host::call_value(cb.clone(), vec![acc, v])?;
        if host::has_pending_throw() {
            return Ok(Value::Undef);
        }
    }
    Ok(acc)
}

fn php_array_slice(h: &mut host::PhpHost, args: &[Value]) -> Value {
    let arr = arg(args, 0);
    let pairs = h.array_pairs(&arr).unwrap_or_default();
    let n = pairs.len() as i64;
    let mut off = arg(args, 1).to_int();
    if off < 0 {
        off = (n + off).max(0);
    }
    let off = off.min(n).max(0) as usize;
    let len = match args.get(2) {
        Some(v) if !matches!(v, Value::Undef) => {
            let l = v.to_int();
            if l < 0 {
                (n - off as i64 + l).max(0) as usize
            } else {
                l as usize
            }
        }
        _ => pairs.len() - off,
    };
    let preserve = args.get(3).map(|v| h.is_truthy(v)).unwrap_or(false);
    let end = (off + len).min(pairs.len());
    let out = h.new_array();
    for (k, v) in &pairs[off..end] {
        match k {
            Value::Int(_) if !preserve => h.arr_push_auto(&out, v.clone()),
            _ => h.arr_set_key(&out, k, v.clone()),
        }
    }
    out
}

fn php_array_reverse(h: &mut host::PhpHost, args: &[Value]) -> Value {
    let arr = arg(args, 0);
    let pairs = h.array_pairs(&arr).unwrap_or_default();
    let preserve = args.get(1).map(|v| h.is_truthy(v)).unwrap_or(false);
    let out = h.new_array();
    for (k, v) in pairs.into_iter().rev() {
        match k {
            Value::Int(_) if !preserve => h.arr_push_auto(&out, v),
            _ => h.arr_set_key(&out, &k, v),
        }
    }
    out
}

fn php_array_fold(h: &host::PhpHost, arr: &Value, product: bool) -> Value {
    let pairs = h.array_pairs(arr).unwrap_or_default();
    let all_int = pairs
        .iter()
        .all(|(_, v)| matches!(h.to_number(v), Value::Int(_)));
    if all_int {
        let mut acc: i64 = if product { 1 } else { 0 };
        for (_, v) in &pairs {
            let n = h.to_number(v).to_int();
            acc = if product {
                acc.wrapping_mul(n)
            } else {
                acc.wrapping_add(n)
            };
        }
        Value::int(acc)
    } else {
        let mut acc: f64 = if product { 1.0 } else { 0.0 };
        for (_, v) in &pairs {
            let n = h.to_number(v).to_float();
            if product {
                acc *= n;
            } else {
                acc += n;
            }
        }
        Value::float(acc)
    }
}

fn php_array_flip(h: &mut host::PhpHost, arr: &Value) -> Value {
    let pairs = h.array_pairs(arr).unwrap_or_default();
    let out = h.new_array();
    for (k, v) in pairs {
        // value becomes key, key becomes value (host normalizes the new key).
        h.arr_set_key(&out, &v, k);
    }
    out
}

fn php_array_unique(h: &mut host::PhpHost, arr: &Value) -> Value {
    let pairs = h.array_pairs(arr).unwrap_or_default();
    let out = h.new_array();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (k, v) in pairs {
        // PHP's default SORT_STRING comparison: dedupe by string form.
        if seen.insert(h.to_str(&v)) {
            h.arr_set_key(&out, &k, v);
        }
    }
    out
}

fn php_array_key_exists(h: &host::PhpHost, args: &[Value]) -> Value {
    let key = arg(args, 0);
    let arr = arg(args, 1);
    let want = h.to_str(&key);
    let found = h
        .array_pairs(&arr)
        .unwrap_or_default()
        .iter()
        .any(|(k, _)| h.to_str(k) == want);
    Value::bool(found)
}

fn php_array_search(h: &host::PhpHost, args: &[Value]) -> Value {
    let needle = arg(args, 0);
    let arr = arg(args, 1);
    let strict = args.get(2).map(|v| h.is_truthy(v)).unwrap_or(false);
    for (k, v) in h.array_pairs(&arr).unwrap_or_default() {
        let hit = if strict {
            strict_eq(h, &needle, &v)
        } else {
            loose_eq(h, &needle, &v)
        };
        if hit {
            return k;
        }
    }
    Value::bool(false)
}

/// `sort`/`rsort` — sorts by value in place and re-indexes (keys 0..n), returning
/// `true`, like PHP. Arrays are reference handles here, so mutating the handle's
/// target is visible through the caller's `$var`.
fn php_sort(h: &mut host::PhpHost, arr: &Value, reverse: bool) -> Value {
    let mut vals: Vec<Value> = h
        .array_pairs(arr)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, v)| v)
        .collect();
    vals.sort_by(|a, b| php_compare(h, a, b).cmp(&0));
    if reverse {
        vals.reverse();
    }
    h.arr_set_reindexed(arr, vals);
    Value::bool(true)
}

/// `asort`/`arsort` — sorts by value in place, preserving keys; returns `true`.
fn php_asort(h: &mut host::PhpHost, arr: &Value, reverse: bool) -> Value {
    let mut pairs = h.array_pairs(arr).unwrap_or_default();
    pairs.sort_by(|(_, a), (_, b)| php_compare(h, a, b).cmp(&0));
    if reverse {
        pairs.reverse();
    }
    h.arr_set_pairs(arr, pairs);
    Value::bool(true)
}

/// `ksort`/`krsort` — sorts by key in place, preserving keys; returns `true`.
fn php_ksort(h: &mut host::PhpHost, arr: &Value, reverse: bool) -> Value {
    let mut pairs = h.array_pairs(arr).unwrap_or_default();
    pairs.sort_by(|(a, _), (b, _)| php_compare(h, a, b).cmp(&0));
    if reverse {
        pairs.reverse();
    }
    h.arr_set_pairs(arr, pairs);
    Value::bool(true)
}

fn php_array_fill(h: &mut host::PhpHost, args: &[Value]) -> Value {
    let start = arg(args, 0).to_int();
    let count = arg(args, 1).to_int().max(0);
    let val = arg(args, 2);
    let out = h.new_array();
    for i in 0..count {
        h.arr_set_key(&out, &Value::int(start + i), val.clone());
    }
    out
}

fn php_array_combine(h: &mut host::PhpHost, args: &[Value]) -> Value {
    let keys = h.array_pairs(&arg(args, 0)).unwrap_or_default();
    let vals = h.array_pairs(&arg(args, 1)).unwrap_or_default();
    let out = h.new_array();
    for ((_, k), (_, v)) in keys.into_iter().zip(vals) {
        h.arr_set_key(&out, &k, v);
    }
    out
}

/// `array_diff` (`intersect=false`) / `array_intersect` (`intersect=true`) over
/// the first array against the rest, compared by string form (PHP's default).
fn php_array_diff(h: &mut host::PhpHost, args: &[Value], intersect: bool) -> Value {
    let first = h.array_pairs(&arg(args, 0)).unwrap_or_default();
    let others: Vec<std::collections::HashSet<String>> = args[1.min(args.len())..]
        .iter()
        .map(|a| {
            h.array_pairs(a)
                .unwrap_or_default()
                .into_iter()
                .map(|(_, v)| h.to_str(&v))
                .collect()
        })
        .collect();
    let out = h.new_array();
    for (k, v) in first {
        let s = h.to_str(&v);
        let in_all = others.iter().all(|set| set.contains(&s));
        let in_any = others.iter().any(|set| set.contains(&s));
        let keep = if intersect { in_all } else { !in_any };
        if keep {
            h.arr_set_key(&out, &k, v);
        }
    }
    out
}

// ── var_export / json_encode ─────────────────────────────────────────────────

fn php_var_export(h: &host::PhpHost, v: &Value, depth: usize) -> String {
    match v {
        Value::Undef => "NULL".to_string(),
        Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(_) => h.to_str(v),
        Value::Str(s) => format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'")),
        Value::Obj(_) => {
            let pad = "  ".repeat(depth);
            let inner = "  ".repeat(depth + 1);
            let mut out = "array (\n".to_string();
            for (k, val) in h.array_pairs(v).unwrap_or_default() {
                let key = match k {
                    Value::Int(n) => n.to_string(),
                    other => format!("'{}'", h.to_str(&other).replace('\'', "\\'")),
                };
                out.push_str(&format!(
                    "{inner}{key} => {},\n",
                    php_var_export(h, &val, depth + 1)
                ));
            }
            out.push_str(&format!("{pad})"));
            out
        }
        _ => "NULL".to_string(),
    }
}

fn php_json_encode(h: &host::PhpHost, v: &Value) -> String {
    match v {
        Value::Undef => "null".to_string(),
        Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(_) => h.to_str(v),
        Value::Str(s) => json_string(s),
        Value::Obj(_) => {
            let pairs = h.array_pairs(v).unwrap_or_default();
            if is_list(&pairs) {
                let items: Vec<String> = pairs
                    .iter()
                    .map(|(_, val)| php_json_encode(h, val))
                    .collect();
                format!("[{}]", items.join(","))
            } else {
                let items: Vec<String> = pairs
                    .iter()
                    .map(|(k, val)| {
                        format!("{}:{}", json_string(&h.to_str(k)), php_json_encode(h, val))
                    })
                    .collect();
                format!("{{{}}}", items.join(","))
            }
        }
        _ => "null".to_string(),
    }
}

fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
