//! Te phplang builtins: the `CallBuiltin` handlers the compiler emits, the
//! strict numeric hook, and the PHP standard-library functions reached through
//! `CALL`.
//!
//! Every handler has the fusevm signature `fn(&mut VM, u8) -> Value` and leaves
//! exactly one value on the stack. Operations that can fail (division by zero, an
//! undefined function) record the message on the host and halt the current chunk
//! by setting `vm.ip` past the end; `host::run_chunk_on` then surfaces it as an
//! `Err`.

use crate::host::{self, ops, with_host, PropAccess};
use fusevm::{NumOp, NumericCall, Value, VM};

/// Register every compiler-emitted builtin on a fresh VM.
pub fn install(vm: &mut VM) {
    vm.register_builtin(ops::ECHO, b_echo);
    vm.register_builtin(ops::GETVAR, b_getvar);
    vm.register_builtin(ops::SETVAR, b_setvar);
    vm.register_builtin(ops::COPY, b_copy);
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
    vm.register_builtin(ops::PROP_TOUCH, b_prop_touch);
    vm.register_builtin(ops::PROP_SET_RW, b_prop_set_rw);
    vm.register_builtin(ops::PROP_UNSET, b_prop_unset);
    vm.register_builtin(ops::PROP_ISSET, b_prop_isset);
    vm.register_builtin(ops::INDEX_ISSET, b_index_isset);
    vm.register_builtin(ops::DECL_FATAL, b_decl_fatal);
    vm.register_builtin(ops::DYN_CLASS, b_dyn_class);
    vm.register_builtin(ops::DYN_CLASS_CONST, b_dyn_class_const);
    vm.register_builtin(ops::CLONE, b_clone);
    vm.register_builtin(ops::CONST_DECL, b_const_decl);
    vm.register_builtin(ops::PROP_GET_EMPTY, b_prop_get_empty);
    vm.register_builtin(ops::PROP_INCDEC, b_prop_incdec);
    vm.register_builtin(ops::SUPPRESS_PUSH, b_suppress_push);
    vm.register_builtin(ops::SUPPRESS_POP, b_suppress_pop);
    vm.register_builtin(ops::MCALL, b_mcall);
    vm.register_builtin(ops::SCALL, b_scall);
    vm.register_builtin(ops::SCONST, b_sconst);
    vm.register_builtin(ops::THROW, b_throw);
    vm.register_builtin(ops::RUN_TRY, b_run_try);
    vm.register_builtin(ops::SIG_HALT, b_sig_halt);
    vm.register_builtin(ops::SIG_BREAK, b_sig_break);
    vm.register_builtin(ops::SIG_CONTINUE, b_sig_continue);
    vm.register_builtin(ops::SIG_LEVEL, b_sig_level);
    vm.register_builtin(ops::CONST_FETCH, b_const_fetch);
    vm.register_builtin(ops::MAGIC_FILE, b_magic_file);
    vm.register_builtin(ops::MAGIC_DIR, b_magic_dir);
    vm.register_builtin(ops::MAGIC_CLASS, b_magic_class);
    vm.register_builtin(ops::UNSET_VAR, b_unset_var);
    vm.register_builtin(ops::UNSET_PATH, b_unset_path);
    vm.register_builtin(ops::FOREACH_PREP, b_foreach_prep);
    vm.register_builtin(ops::INSTANCEOF, b_instanceof);
    vm.register_builtin(ops::REF_BIND, b_ref_bind);
    vm.register_builtin(ops::REF_CELL, b_ref_cell);
    vm.register_builtin(ops::REF_SLOT_VAR, b_ref_slot_var);
    vm.register_builtin(ops::REF_SLOT_ELEM, b_ref_slot_elem);
    vm.register_builtin(ops::REF_SLOT_PROP, b_ref_slot_prop);
    vm.register_builtin(ops::REF_TO_VAR, b_ref_to_var);
    vm.register_builtin(ops::REF_TO_ELEM, b_ref_to_elem);
    vm.register_builtin(ops::REF_TO_APPEND, b_ref_to_append);
    vm.register_builtin(ops::REF_TO_PROP, b_ref_to_prop);
    vm.register_builtin(ops::GETVAR_Q, b_getvar_q);
    vm.register_builtin(ops::INDEX_GET_Q, b_index_get_q);
    vm.register_builtin(ops::LIST_ELEM_GET, b_list_elem_get);
    vm.register_builtin(ops::PROP_GET_Q, b_prop_get_q);
    vm.register_builtin(ops::LSB_CLASS, b_lsb_class);
    vm.register_builtin(ops::LSB_FORWARD, b_lsb_forward);
    vm.register_builtin(ops::RET_REF, b_ret_ref);
    vm.register_builtin(ops::REF_SLOT_RET, b_ref_slot_ret);
    vm.register_builtin(ops::BYREF_OUT, b_byref_out);
    vm.register_builtin(ops::BYREF_LIVE, b_byref_live);
    vm.register_builtin(ops::SPROP_GET, b_sprop_get);
    vm.register_builtin(ops::SPROP_SET, b_sprop_set);
    vm.register_builtin(ops::SPROP_INCDEC, b_sprop_incdec);
    vm.register_builtin(ops::STATIC_BIND, b_static_bind);
    vm.register_builtin(ops::CALL_NAMED, b_call_named);
    vm.register_builtin(ops::MCALL_NAMED, b_mcall_named);
    vm.register_builtin(ops::SCALL_NAMED, b_scall_named);
    vm.register_builtin(ops::NEW_NAMED, b_new_named);
    vm.register_builtin(ops::CALLVALUE_NAMED, b_callvalue_named);
    vm.register_builtin(ops::YIELD, b_yield);
    vm.register_builtin(ops::YIELD_KV, b_yield_kv);
    vm.register_builtin(ops::YIELD_FROM, b_yield_from);
    vm.register_builtin(ops::IS_GENERATOR, b_is_generator);
    vm.register_builtin(ops::GEN_REWIND, b_gen_rewind);
    vm.register_builtin(ops::GEN_VALID, b_gen_valid);
    vm.register_builtin(ops::GEN_KEY, b_gen_key);
    vm.register_builtin(ops::GEN_CURRENT, b_gen_current);
    vm.register_builtin(ops::GEN_NEXT, b_gen_next);
}

// ── generators ───────────────────────────────────────────────────────────────

/// `yield $v` — suspend the running generator; returns the next `->send()` value.
/// An error carrying a now-pending throw halts the body so it unwinds normally.
fn b_yield(vm: &mut VM, _: u8) -> Value {
    let val = vm.pop();
    match host::yield_value(val) {
        Ok(v) => bubbled(vm, v),
        Err(e) => yield_err(vm, e),
    }
}

/// `yield $k => $v` — suspend with an explicit key. Stack `[value, key]`.
fn b_yield_kv(vm: &mut VM, _: u8) -> Value {
    let key = vm.pop();
    let val = vm.pop();
    match host::yield_kv(key, val) {
        Ok(v) => bubbled(vm, v),
        Err(e) => yield_err(vm, e),
    }
}

/// `yield from $it` — delegate; leaves the delegate's return value on the stack.
fn b_yield_from(vm: &mut VM, _: u8) -> Value {
    let src = vm.pop();
    match host::yield_from(src) {
        Ok(v) => bubbled(vm, v),
        Err(e) => yield_err(vm, e),
    }
}

/// A `yield` error is either a real error (yield outside a generator) or the
/// sentinel for an injected/uncaught throw already recorded as pending — in the
/// latter case halt the chunk so the pending exception unwinds the body.
fn yield_err(vm: &mut VM, e: String) -> Value {
    if host::unwinding() {
        vm.ip = vm.chunk.ops.len();
        Value::Undef
    } else {
        fail(vm, e)
    }
}

fn b_is_generator(vm: &mut VM, _: u8) -> Value {
    let v = vm.pop();
    Value::bool(with_host(|h| h.is_generator_val(&v)))
}

fn b_gen_rewind(vm: &mut VM, _: u8) -> Value {
    let g = vm.pop();
    match host::gen_rewind(&g) {
        Ok(()) => bubbled(vm, Value::Undef),
        Err(e) => yield_err(vm, e),
    }
}

fn b_gen_valid(vm: &mut VM, _: u8) -> Value {
    let g = vm.pop();
    match host::gen_valid(&g) {
        Ok(b) => bubbled(vm, Value::bool(b)),
        Err(e) => yield_err(vm, e),
    }
}

fn b_gen_key(vm: &mut VM, _: u8) -> Value {
    let g = vm.pop();
    match host::gen_key(&g) {
        Ok(v) => bubbled(vm, v),
        Err(e) => yield_err(vm, e),
    }
}

fn b_gen_current(vm: &mut VM, _: u8) -> Value {
    let g = vm.pop();
    match host::gen_current(&g) {
        Ok(v) => bubbled(vm, v),
        Err(e) => yield_err(vm, e),
    }
}

fn b_gen_next(vm: &mut VM, _: u8) -> Value {
    let g = vm.pop();
    match host::gen_next(&g) {
        Ok(v) => bubbled(vm, v),
        Err(e) => yield_err(vm, e),
    }
}

/// Split a `(name, value)` argument-pair stream into positional arguments (name is
/// `Undef`) and named arguments (name is a `Str`), preserving source order.
fn split_named(pairs: Vec<Value>) -> (Vec<Value>, Vec<(String, Value)>) {
    let mut positional = Vec::new();
    let mut named = Vec::new();
    let mut it = pairs.into_iter();
    while let (Some(name), Some(val)) = (it.next(), it.next()) {
        match name {
            Value::Str(s) => named.push((s.to_string(), val)),
            _ => positional.push(val),
        }
    }
    (positional, named)
}

/// `f(name: v, ...)` — named-argument function call. Stack `[fname, (n,v)...]`.
fn b_call_named(vm: &mut VM, argc: u8) -> Value {
    let pairs = pop_args(vm, argc as usize - 1);
    let name = pop_name(vm);
    let (pos, named) = split_named(pairs);
    match host::call_function_named(&name, pos, named) {
        Ok(v) => bubbled(vm, v),
        Err(e) => fail(vm, e),
    }
}

/// `$f(name: v, ...)` — named-argument dynamic call. Stack `[callee, (n,v)...]`.
fn b_callvalue_named(vm: &mut VM, argc: u8) -> Value {
    let pairs = pop_args(vm, argc as usize - 1);
    let callee = vm.pop();
    let (pos, named) = split_named(pairs);
    mark_frame_line(vm);
    match host::call_value_named(callee, pos, named) {
        Ok(v) => bubbled(vm, v),
        Err(e) => fail(vm, e),
    }
}

/// `$o->m(name: v, ...)` — named-argument method call. Stack `[recv, method, (n,v)...]`.
fn b_mcall_named(vm: &mut VM, argc: u8) -> Value {
    let pairs = pop_args(vm, argc as usize - 2);
    let method = pop_name(vm);
    let recv = vm.pop();
    let (pos, named) = split_named(pairs);
    mark_frame_line(vm);
    // Built-in `Closure`/`Generator` methods take positional args (no named form).
    if with_host(|h| h.is_closure(&recv)) {
        return match host::call_closure_method(&recv, &method, pos) {
            Ok(v) => bubbled(vm, v),
            Err(e) => fail(vm, e),
        };
    }
    if with_host(|h| h.is_generator_val(&recv)) {
        return match host::call_generator_method(&recv, &method, pos) {
            Ok(v) => bubbled(vm, v),
            Err(e) => yield_err(vm, e),
        };
    }
    match with_host(|h| h.object_class(&recv)) {
        Some(c) => {
            let magic = match method_plan(vm, &c, &method, true) {
                Ok(m) => m,
                Err(v) => return v,
            };
            let r = if magic {
                host::call_magic_call_named(&c, &method, Some(recv), pos, named)
            } else {
                host::call_method_named(&c, &method, Some(recv), pos, named)
            };
            match r {
                Ok(v) => bubbled(vm, v),
                Err(e) => fail(vm, e),
            }
        }
        None => {
            let ty = with_host(|h| receiver_type_name(h, &recv));
            throw_php(
                vm,
                "Error",
                &format!("Call to a member function {method}() on {ty}"),
            )
        }
    }
}

/// `C::m(name: v, ...)` — named-argument static call. Stack `[class, method, (n,v)...]`.
fn b_scall_named(vm: &mut VM, argc: u8) -> Value {
    let pairs = pop_args(vm, argc as usize - 2);
    let method = pop_name(vm);
    let class = pop_name(vm);
    let (pos, named) = split_named(pairs);
    let this = with_host(|h| {
        let t = h.get_var("this");
        matches!(t, Value::Obj(_)).then_some(t)
    });
    mark_frame_line(vm);
    let magic = match static_method_plan(vm, &class, &method, &this) {
        Ok(m) => m,
        Err(v) => return v,
    };
    let r = if magic {
        host::call_magic_call_named(&class, &method, this, pos, named)
    } else {
        host::call_method_named(&class, &method, this, pos, named)
    };
    match r {
        Ok(v) => bubbled(vm, v),
        Err(e) => fail(vm, e),
    }
}

/// `new C(name: v, ...)` — named-argument constructor. Stack `[class, (n,v)...]`.
fn b_new_named(vm: &mut VM, argc: u8) -> Value {
    let pairs = pop_args(vm, argc as usize - 1);
    let class = pop_name(vm);
    let (pos, named) = split_named(pairs);
    mark_frame_line(vm);
    match host::new_object_named(&class, pos, named) {
        Ok(v) => bubbled(vm, v),
        Err(e) => fail(vm, e),
    }
}

/// `Class::$prop` read. Stack `[class, name]`.
fn b_sprop_get(vm: &mut VM, _: u8) -> Value {
    let name = pop_name(vm);
    let class = pop_name(vm);
    match host::static_prop_get(&class, &name) {
        Ok(v) => v,
        Err(e) => fail_or_throw(vm, e),
    }
}

/// `Class::$prop = val`. Stack `[class, name, val]`; leaves the assigned value.
fn b_sprop_set(vm: &mut VM, _: u8) -> Value {
    let val = vm.pop();
    let name = pop_name(vm);
    let class = pop_name(vm);
    match host::static_prop_set(&class, &name, val) {
        Ok(v) => v,
        Err(e) => fail_or_throw(vm, e),
    }
}

/// `++`/`--` on `Class::$prop`. Stack `[class, name, code]`; the `code` bits match
/// `b_incdec` (bit0 = increment, bit1 = prefix).
fn b_sprop_incdec(vm: &mut VM, _: u8) -> Value {
    let code = vm.pop().to_int();
    let name = pop_name(vm);
    let class = pop_name(vm);
    let inc = code & 1 != 0;
    let prefix = code & 2 != 0;
    let old = match host::static_prop_get(&class, &name) {
        Ok(v) => v,
        Err(e) => return fail_or_throw(vm, e),
    };
    mark_warn_site(vm);
    if incdec_refused(vm, &old, inc) {
        return Value::Undef;
    }
    let newv = with_host(|h| h.incdec_value(&old, inc));
    if let Err(e) = host::static_prop_set(&class, &name, newv.clone()) {
        return fail_or_throw(vm, e);
    }
    if prefix {
        newv
    } else {
        old
    }
}

/// `static $var = init;` — alias `$var` to its persistent slot. Stack
/// `[name, slot-key, init]`. The `init` is only used the first time the slot is
/// created; later calls reuse the existing cell.
fn b_static_bind(vm: &mut VM, _: u8) -> Value {
    let init = vm.pop();
    let slot_key = pop_name(vm);
    let name = pop_name(vm);
    with_host(|h| h.bind_static_local(&name, &slot_key, init));
    Value::Undef
}

/// Read the last call's by-reference parameter value at the given position (for
/// the caller's post-call write-back). Stack `[position]`.
fn b_byref_out(vm: &mut VM, _: u8) -> Value {
    let pos = vm.pop().to_int().max(0) as usize;
    with_host(|h| h.byref_out_get(pos))
}

/// Whether the call that just returned took parameter `pos` by reference — the
/// guard a call site emits when its callee is not known until run time.
fn b_byref_live(vm: &mut VM, _: u8) -> Value {
    let pos = vm.pop().to_int().max(0) as usize;
    Value::bool(with_host(|h| h.byref_out_live(pos)))
}

/// `$target = &$source` — bind the two names to a shared cell; leaves the value.
/// `use (&$v)`: the enclosing variable's reference cell, as a handle the
/// closure carries until its frame is built.
fn b_ref_cell(vm: &mut VM, _: u8) -> Value {
    let name = pop_name(vm);
    with_host(|h| h.ref_cell_of(&name))
}

fn b_ref_bind(vm: &mut VM, _: u8) -> Value {
    let source = pop_name(vm);
    let target = pop_name(vm);
    with_host(|h| {
        h.ref_bind(&target, &source);
        h.get_var(&target)
    })
}

// ── `&` bindings to a container slot ────────────────────────────────────────
//
// The ACQUIRE half (`REF_SLOT_*`) returns the reference cell the right-hand side
// of a `&` denotes, promoting the element/property into a reference if it was a
// plain value; the BIND half (`REF_TO_*`) points the left-hand side at that
// cell. The slot travels between them as an `Int` the compiler never lets reach
// a PHP expression. See `ops::REF_SLOT_VAR`.

/// Stack `[name]` → the reference slot of `$name`.
fn b_ref_slot_var(vm: &mut VM, _: u8) -> Value {
    let name = pop_name(vm);
    Value::int(with_host(|h| h.elem_ref_slot(&name, &[])) as i64)
}

/// Stack `[name, k1..kN]`, `N = argc-1` → the reference slot of `$name[k1]..[kN]`.
fn b_ref_slot_elem(vm: &mut VM, argc: u8) -> Value {
    let keys = pop_args(vm, argc as usize - 1);
    let name = pop_name(vm);
    Value::int(with_host(|h| h.elem_ref_slot(&name, &keys)) as i64)
}

/// Stack `[recv, prop]` → the reference slot of `$recv->prop`.
fn b_ref_slot_prop(vm: &mut VM, _: u8) -> Value {
    let prop = pop_name(vm);
    let recv = vm.pop();
    Value::int(with_host(|h| h.prop_ref_slot_ensure(&recv, &prop)) as i64)
}

/// Stack `[name, slot]` — bind `$name` to the cell at `slot`.
fn b_ref_to_var(vm: &mut VM, _: u8) -> Value {
    let slot = vm.pop().to_int() as usize;
    let name = pop_name(vm);
    with_host(|h| {
        h.bind_ref_slot(&name, slot);
        h.get_var(&name)
    })
}

/// Stack `[name, k1..kN, slot]`, `N = argc-2` — make `$name[k1]..[kN]` a
/// reference to the cell at `slot`.
fn b_ref_to_elem(vm: &mut VM, argc: u8) -> Value {
    let slot = vm.pop().to_int() as usize;
    let keys = pop_args(vm, argc as usize - 2);
    let name = pop_name(vm);
    with_host(|h| {
        h.bind_elem_to_slot(&name, &keys, slot);
        h.ref_cell_value(slot)
    })
}

/// Stack `[name, k1..kM, slot]`, `M = argc-2` — append a reference to the cell at
/// `slot` to `$name[k1]..[kM]`.
fn b_ref_to_append(vm: &mut VM, argc: u8) -> Value {
    let slot = vm.pop().to_int() as usize;
    let keys = pop_args(vm, argc as usize - 2);
    let name = pop_name(vm);
    with_host(|h| {
        h.append_elem_to_slot(&name, &keys, slot);
        h.ref_cell_value(slot)
    })
}

/// Stack `[recv, prop, slot]` — make `$recv->prop` a reference to `slot`'s cell.
fn b_ref_to_prop(vm: &mut VM, _: u8) -> Value {
    let slot = vm.pop().to_int() as usize;
    let prop = pop_name(vm);
    let recv = vm.pop();
    with_host(|h| {
        h.bind_prop_to_slot(&recv, &prop, slot);
        h.ref_cell_value(slot)
    })
}

/// Stack `[slot]` — publish the running by-reference function's returned cell and
/// leave its value (what a plain call of the function sees).
fn b_ret_ref(vm: &mut VM, _: u8) -> Value {
    let slot = vm.pop().to_int() as usize;
    with_host(|h| {
        h.set_ret_ref_slot(slot);
        h.ref_cell_value(slot)
    })
}

/// Stack `[value]` — the cell the last call returned by reference, as an `Int`
/// slot. `value` is the call's result, used as the fallback cell's contents when
/// the callee did not return by reference.
fn b_ref_slot_ret(vm: &mut VM, _: u8) -> Value {
    let v = vm.pop();
    Value::int(with_host(|h| h.take_ret_ref_slot(v)) as i64)
}

/// Stack `[fallback]` — the running frame's late-static-binding class.
fn b_lsb_class(vm: &mut VM, _: u8) -> Value {
    let fallback = pop_name(vm);
    Value::str(with_host(|h| h.lsb_class(&fallback)))
}

/// Mark the next call as forwarding its caller's late-static-binding class.
fn b_lsb_forward(_: &mut VM, _: u8) -> Value {
    with_host(|h| h.lsb_forward());
    Value::Undef
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

/// `$expr::` — resolve the value standing left of a `::` to a class name.
///
/// PHP accepts exactly two things here and coerces neither: an object, whose
/// class is used, and a string, which already IS the class name (the lookups
/// downstream are case-insensitive, so it is passed through as written).
/// Everything else — int, float, bool, null, array — is an `Error`, raised
/// before the member is even looked at.
fn b_dyn_class(vm: &mut VM, _: u8) -> Value {
    let v = vm.pop();
    if let Value::Str(s) = &v {
        return Value::str(s.to_string());
    }
    match with_host(|h| h.object_class(&v)) {
        Some(class) => Value::str(class),
        None => throw_php(vm, "Error", "Class name must be a valid object or a string"),
    }
}

/// `$expr::class` — the class name of an OBJECT.
///
/// Deliberately not `b_dyn_class` plus a read: PHP refuses the class-name
/// string that every other `::` accepts, because `::class` is documented to
/// report the class of a value rather than to echo one back.
fn b_dyn_class_const(vm: &mut VM, _: u8) -> Value {
    let v = vm.pop();
    match with_host(|h| h.object_class(&v).filter(|_| !h.is_array(&v))) {
        Some(class) => Value::str(class),
        None => {
            let what = with_host(|h| crate::stdlib::types::value_name(h, &v));
            throw_php(
                vm,
                "TypeError",
                &format!("Cannot use \"::class\" on {what}"),
            )
        }
    }
}

/// `clone $o` — see [`host::clone_object`]. The `__clone` hook runs PHP code,
/// so a throw from inside it has to unwind this chunk like any other call.
fn b_clone(vm: &mut VM, _: u8) -> Value {
    let v = vm.pop();
    mark_frame_line(vm);
    match host::clone_object(v) {
        Ok(copy) => bubbled(vm, copy),
        Err(e) => fail_or_throw(vm, e),
    }
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
/// A bare constant reference. Undefined is an `Error` in PHP 8 — the bareword
/// no longer falls back to its own name as a string.
fn b_const_fetch(vm: &mut VM, _: u8) -> Value {
    let name = pop_name(vm);
    match with_host(|h| h.const_fetch(&name)) {
        Some(v) => v,
        None => {
            mark_frame_line(vm);
            throw_php(vm, "Error", &undefined_constant(&name))
        }
    }
}

/// `__FILE__`, wrapped in the affixes the compiler emitted: empty for `__FILE__`
/// itself, and `{closure:` / `:<line>}` for a closure declared at file scope,
/// whose PHP-given name embeds the script's own.
fn b_magic_file(vm: &mut VM, _: u8) -> Value {
    let suffix = pop_name(vm);
    let prefix = pop_name(vm);
    let file = with_host(|h| h.script_name().to_string());
    Value::str(format!("{prefix}{file}{suffix}"))
}

fn b_magic_dir(vm: &mut VM, _: u8) -> Value {
    let _ = vm;
    Value::str(with_host(|h| h.magic_dir()))
}

/// `__CLASS__` where the parse could not name the class, wrapped in the affixes
/// the compiler emitted — `::q` builds an anonymous class's `__METHOD__` out of
/// the same node.
fn b_magic_class(vm: &mut VM, _: u8) -> Value {
    let suffix = pop_name(vm);
    let prefix = pop_name(vm);
    let class = with_host(|h| h.magic_class());
    Value::str(format!("{prefix}{class}{suffix}"))
}

/// The message PHP raises for a constant that is not defined, shared by the
/// bareword reference and the `constant()` library function so the two cannot
/// drift.
pub(crate) fn undefined_constant(name: &str) -> String {
    format!("Undefined constant \"{name}\"")
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
    // `unset($o[k])` on an `ArrayAccess` object is `offsetUnset(k)`. Only the
    // single-key form reaches the object; a deeper path indexes whatever
    // `offsetGet` returned, which this scaffold does not model.
    let recv = with_host(|h| h.get_var(&name));
    if keys.len() == 1 {
        if let Some(r) = array_access_call(vm, &recv, "offsetUnset", vec![keys[0].clone()]) {
            return match r {
                Ok(_) => bubbled(vm, Value::Undef),
                Err(e) => fail(vm, e),
            };
        }
    }
    with_host(|h| h.unset_path(&name, &keys));
    Value::Undef
}

/// Halt the current chunk if an exception is now in flight, so a `throw` raised
/// by a nested call bubbles up through the caller's VM too. Returns `true` when
/// it halted (the caller should return immediately).
fn bubble_throw(vm: &mut VM) -> bool {
    if host::unwinding() {
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

fn b_sig_break(vm: &mut VM, argc: u8) -> Value {
    let level = sig_level_arg(vm, argc);
    host::set_break(level);
    vm.ip = vm.chunk.ops.len();
    Value::Undef
}

fn b_sig_continue(vm: &mut VM, argc: u8) -> Value {
    let level = sig_level_arg(vm, argc);
    host::set_continue(level);
    vm.ip = vm.chunk.ops.len();
    Value::Undef
}

/// The level operand of a `break`/`continue` signal, defaulting to 1 when the
/// compiler emitted the no-argument form.
fn sig_level_arg(vm: &mut VM, argc: u8) -> u32 {
    if argc == 0 {
        return 1;
    }
    vm.pop().to_int().max(1) as u32
}

/// `SIG_LEVEL` — push the level of the `break`/`continue` that just ended a
/// `try` body, so the dispatch code can pick the loop it was aimed at.
fn b_sig_level(_: &mut VM, _: u8) -> Value {
    Value::int(host::last_break_level() as i64)
}

/// Pop two operands as PHP integers (bitwise ops cast their operands to int).
/// PHP's bitwise operators take two paths. Two *string* operands are combined
/// byte by byte and produce a string — `"5" | "3"` is `"7"` because `0x35|0x33`
/// is `0x37`, not because either side was read as a number. Anything else is
/// numeric and therefore subject to the PHP 8 operand rules, so `"g" | 1` is a
/// `TypeError` while `"5g" | 1` warns and uses `5`.
///
/// `None` means the chunk has been halted with a pending `TypeError`.
fn pop_two_ints(vm: &mut VM, sym: &str) -> Option<(i64, i64)> {
    let b = vm.pop();
    let a = vm.pop();
    int_args(vm, sym, &a, &b)
}

/// [`arith_args`] for the operators that then narrow both operands to `int`.
fn int_args(vm: &mut VM, sym: &str, a: &Value, b: &Value) -> Option<(i64, i64)> {
    mark_warn_site(vm);
    let resolve = || -> Result<(i64, i64), String> {
        Ok((
            int_arith_operand(sym, a, b, true)?,
            int_arith_operand(sym, a, b, false)?,
        ))
    };
    match resolve() {
        Ok(pair) => Some(pair),
        Err(_) => {
            mark_frame_line(vm);
            vm.ip = vm.chunk.ops.len();
            None
        }
    }
}

/// Narrow an already-coerced operand to `int` for the operators that accept
/// nothing else (`% << >> & | ^`, `intdiv`), raising the diagnostics PHP
/// attaches to that narrowing.
///
/// A float that is already integral converts in silence; one with a fraction is
/// deprecated, and one outside `i64` is a warning instead. `orig` is the operand
/// before coercion, because the deprecation quotes the *string* a float-string
/// came from (`float-string ".5g"`) rather than the number it parsed to.
fn int_operand(orig: &Value, coerced: &Value) -> i64 {
    let Value::Float(f) = *coerced else {
        return coerced.to_int();
    };
    let out_of_range = !f.is_finite() || f < i64::MIN as f64 || f > i64::MAX as f64;
    let lossy = out_of_range || f.fract() != 0.0;
    // A float that *came from a string* is always reported as a lost-precision
    // deprecation, even when the loss is an overflow rather than a fraction —
    // but only when the string was written in float form. An integer-format
    // string that merely outgrew `i64` narrows silently.
    if let Value::Str(s) = orig {
        if lossy && host::numeric_prefix_is_float(s) {
            with_host(|h| {
                h.deprecated(format!(
                    "Implicit conversion from float-string \"{s}\" to int loses precision"
                ))
            });
        }
        // Saturating, which is what `as` already does for a finite float.
        return if f.is_finite() { f as i64 } else { 0 };
    }
    if out_of_range {
        let shown = with_host(|h| h.to_str(&Value::float(f)));
        with_host(|h| {
            h.warn(format!(
                "The float {shown} is not representable as an int, cast occurred"
            ))
        });
        return 0;
    }
    if f.fract() != 0.0 {
        let shown = with_host(|h| h.to_str(&Value::float(f)));
        with_host(|h| {
            h.deprecated(format!(
                "Implicit conversion from float {shown} to int loses precision"
            ))
        });
    }
    f as i64
}

/// Resolve one operand of an int-only operator: the PHP 8 operand rules first,
/// then the narrowing to int.
///
/// Doing both for the left operand before touching the right is observable —
/// `2.5 % "INF"` deprecates the left narrowing and only then throws on the
/// right — so the two steps cannot be batched across operands.
fn int_arith_operand(sym: &str, a: &Value, b: &Value, left: bool) -> Result<i64, String> {
    let n = coerce_arith(sym, a, b, left)?;
    Ok(int_operand(if left { a } else { b }, &n))
}

/// The two operands of a bitwise operator when both are strings, which selects
/// the byte-wise form.
fn pop_two_strs(vm: &mut VM) -> Option<(Vec<u8>, Vec<u8>)> {
    match (vm.stack.iter().nth_back(1), vm.stack.last()) {
        (Some(Value::Str(x)), Some(Value::Str(y))) => {
            let (x, y) = (x.as_bytes().to_vec(), y.as_bytes().to_vec());
            vm.pop();
            vm.pop();
            Some((x, y))
        }
        _ => None,
    }
}

fn b_bitand(vm: &mut VM, _: u8) -> Value {
    // `&` and `^` truncate to the shorter operand; only `|` pads.
    if let Some((x, y)) = pop_two_strs(vm) {
        return str_bitop(&x, &y, false, |p, q| p & q);
    }
    let Some((a, b)) = pop_two_ints(vm, "&") else {
        return Value::Undef;
    };
    Value::int(a & b)
}
fn b_bitor(vm: &mut VM, _: u8) -> Value {
    if let Some((x, y)) = pop_two_strs(vm) {
        return str_bitop(&x, &y, true, |p, q| p | q);
    }
    let Some((a, b)) = pop_two_ints(vm, "|") else {
        return Value::Undef;
    };
    Value::int(a | b)
}
fn b_bitxor(vm: &mut VM, _: u8) -> Value {
    if let Some((x, y)) = pop_two_strs(vm) {
        return str_bitop(&x, &y, false, |p, q| p ^ q);
    }
    let Some((a, b)) = pop_two_ints(vm, "^") else {
        return Value::Undef;
    };
    Value::int(a ^ b)
}

/// Combine two byte strings. `pad` picks the length rule: `|` runs to the longer
/// operand treating the missing bytes as `\0`, while `&` and `^` stop at the
/// shorter one.
fn str_bitop(x: &[u8], y: &[u8], pad: bool, f: impl Fn(u8, u8) -> u8) -> Value {
    let n = if pad {
        x.len().max(y.len())
    } else {
        x.len().min(y.len())
    };
    // Result bytes become chars one-for-one, which is the same mapping `chr`
    // already uses here. A byte above 0x7f therefore lands as that code point
    // rather than as a raw byte — the frontend's existing string
    // representation, not a choice this operator makes.
    Value::str(
        (0..n)
            .map(|i| {
                f(
                    x.get(i).copied().unwrap_or(0),
                    y.get(i).copied().unwrap_or(0),
                ) as char
            })
            .collect::<String>(),
    )
}

fn b_shl(vm: &mut VM, _: u8) -> Value {
    let Some((a, b)) = pop_two_ints(vm, "<<") else {
        return Value::Undef;
    };
    // PHP throws a catchable ArithmeticError on a negative shift, not a fatal.
    if b < 0 {
        return throw_php(vm, "ArithmeticError", "Bit shift by negative number");
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
    let Some((a, b)) = pop_two_ints(vm, ">>") else {
        return Value::Undef;
    };
    if b < 0 {
        return throw_php(vm, "ArithmeticError", "Bit shift by negative number");
    }
    // A right shift by >= 63 is a full arithmetic sign-fill in PHP (0 for
    // non-negative, -1 for negative); Rust's `>>` on i64 is arithmetic, so
    // clamp the shift amount to 63.
    Value::int(a >> b.min(63))
}

fn b_bitnot(vm: &mut VM, _: u8) -> Value {
    let v = vm.pop();
    // `~` on a string flips its bytes and stays a string; on an array or object
    // it is refused by name rather than by the `Unsupported operand types`
    // wording the binary operators use.
    match &v {
        Value::Str(s) => str_bitop(s.as_bytes(), s.as_bytes(), false, |p, _| !p),
        Value::Obj(_) => {
            let what = with_host(|h| host::arith_type_name(h, &v));
            mark_frame_line(vm);
            throw_php(
                vm,
                "TypeError",
                &format!("Cannot perform bitwise not on {what}"),
            )
        }
        _ => Value::int(!with_host(|h| h.to_number(&v).to_int())),
    }
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

/// `++`/`--` refuse arrays and objects outright, by name rather than with the
/// `Unsupported operand types` wording the binary operators use.
///
/// `true` means the chunk has been halted with a pending `TypeError`, so the
/// caller must not go on to compute or store a new value.
fn incdec_refused(vm: &mut VM, old: &Value, inc: bool) -> bool {
    if !matches!(old, Value::Obj(_)) {
        return false;
    }
    let what = with_host(|h| host::arith_type_name(h, old));
    let word = if inc { "increment" } else { "decrement" };
    mark_frame_line(vm);
    throw_php(vm, "TypeError", &format!("Cannot {word} {what}"));
    true
}

/// Halt the chunk on an error a host helper returned: as the PHP exception it
/// is tagged as (see [`throws_bare`]), or as a plain scaffold failure when it
/// carries no tag.
///
/// A host helper cannot raise the exception itself — it has no VM to unwind —
/// so it tags the message and the opcode handler that called it converts here.
fn fail_or_throw(vm: &mut VM, e: String) -> Value {
    match untag_bare_throw(&e) {
        Some((class, message)) => {
            let (class, message) = (class.to_string(), message.to_string());
            throw_php(vm, &class, &message)
        }
        None => fail(vm, e),
    }
}

/// Raise a catchable PHP exception from inside a builtin: construct `class` with
/// `message` and record it as the pending throw, then unwind this chunk exactly
/// as the `throw` builtin does, so an enclosing `try`/`catch` can handle it.
///
/// Used by the zero-divisor arithmetic, which PHP 8 reports as a catchable
/// `DivisionByZeroError` rather than a fatal error.
fn throw_php(vm: &mut VM, class: &str, message: &str) -> Value {
    mark_frame_line(vm);
    match pending_php_throw(class, message) {
        Ok(_) => {
            vm.ip = vm.chunk.ops.len();
            Value::Undef
        }
        // The prelude always defines these classes; if construction somehow
        // fails there is no exception to throw, so fall back to a host error.
        Err(e) => fail(vm, e),
    }
}

/// Screen a method call against the class's method table, visibility rules and
/// magic catch-all before it is dispatched.
///
/// `Err(v)` means the chunk has been halted with a thrown `Error` — the two
/// failing arms are both catchable `Error`s in PHP 8, not fatals. `Ok(true)`
/// means the call must be re-dispatched through `__call`/`__callStatic` even
/// though a method of that name may exist: an inaccessible one is the catch-all's
/// business, not an access error.
fn method_plan(vm: &mut VM, class: &str, method: &str, has_this: bool) -> Result<bool, Value> {
    match with_host(|h| h.method_dispatch(class, method, has_this)) {
        host::MethodDispatch::Direct => Ok(false),
        host::MethodDispatch::Magic => Ok(true),
        host::MethodDispatch::Denied(msg) => Err(throw_php(vm, "Error", &msg)),
        host::MethodDispatch::Undefined => Err(throw_php(
            vm,
            "Error",
            &format!(
                "Call to undefined method {}::{method}()",
                host::display_class(class)
            ),
        )),
    }
}

/// [`method_plan`] for the `C::m()` form.
///
/// Three families of static call never reach the class method table and must be
/// let through untouched: `Closure::bind`/`fromCallable`, the synthesized enum
/// helpers `cases`/`from`/`tryFrom`, and a call on a class that was never
/// declared (which reports "class not found", a different diagnostic).
fn static_method_plan(
    vm: &mut VM,
    class: &str,
    method: &str,
    this: &Option<Value>,
) -> Result<bool, Value> {
    // `Closure::bind`/`fromCallable` are synthesized — there is no `Closure`
    // class to find — so they are let through before the existence check.
    if class.eq_ignore_ascii_case("Closure") {
        return Ok(false);
    }
    // A call on a class that was never declared never reaches a method table:
    // the class is what is missing, and PHP says so instead of naming a method
    // that could not have existed either way.
    if !with_host(|h| h.class_exists(class)) {
        return Err(throw_php(
            vm,
            "Error",
            &format!("Class \"{}\" not found", host::display_class(class)),
        ));
    }
    if with_host(|h| h.is_enum_class(class))
        && matches!(
            method.to_ascii_lowercase().as_str(),
            "cases" | "from" | "tryfrom"
        )
    {
        return Ok(false);
    }
    // `__call` handles the static form only when `$this` is set AND is an
    // instance of the named class; otherwise the fallback is `__callStatic`.
    let has_this = this
        .as_ref()
        .and_then(|t| with_host(|h| h.object_class(t)))
        .is_some_and(|c| with_host(|h| h.class_is_a_pub(&c, class)));
    method_plan(vm, class, method, has_this)
}

/// PHP's name for a value's type in the "Call to a member function f() on X"
/// error. Booleans are spelled `true`/`false` there rather than `bool`.
fn receiver_type_name(h: &host::PhpHost, v: &Value) -> &'static str {
    match v {
        Value::Undef => "null",
        Value::Bool(true) => "true",
        Value::Bool(false) => "false",
        Value::Int(_) => "int",
        Value::Float(_) => "float",
        Value::Str(_) => "string",
        Value::Obj(_) if h.is_array(v) => "array",
        _ => "mixed",
    }
}

/// [`throw_php`] for library functions that return `Result` and have no VM
/// handle. Recording the pending throw is enough: the `bubbled` helper on the
/// calling side halts the chunk as soon as it sees one, so the `Ok(Undef)`
/// returned here is never observed as a value.
fn pending_php_throw(class: &str, message: &str) -> Result<Value, String> {
    let exc = host::new_object(class, vec![Value::str(message.to_string())])?;
    host::set_pending_throw(exc);
    Ok(Value::Undef)
}

// ── core builtins ────────────────────────────────────────────────────────────

fn b_echo(vm: &mut VM, argc: u8) -> Value {
    let args = pop_args(vm, argc as usize);
    for a in &args {
        // Conversion first, *outside* the host borrow: an object with
        // `__toString` runs PHP code to produce its string.
        let s = host::to_str_ext(a);
        with_host(|h| h.write_out(&s));
    }
    Value::Undef
}

/// Tell the host which source line a diagnostic raised from here belongs to.
///
/// `VM::ip` has already been advanced past the instruction being dispatched, so
/// the executing op — the `CallBuiltin` that reached this handler — is at
/// `ip - 1`, and the chunk's parallel line table gives its line. Reading it here
/// costs nothing on the paths that never warn.
fn mark_warn_site(vm: &VM) {
    with_host(|h| h.set_warn_line(cur_op_line(vm)));
}

/// The source line of the op being dispatched. `VM::ip` has already advanced past
/// it, so the executing op is at `ip - 1` in the chunk's parallel line table.
fn cur_op_line(vm: &VM) -> u32 {
    vm.ip
        .checked_sub(1)
        .and_then(|i| vm.chunk.lines.get(i))
        .copied()
        .unwrap_or(0)
}

/// Record the executing op's line as the current call frame's position, so an
/// exception created below here can report its own line and name this frame's
/// call site in its backtrace. Called by the ops that can enter a new frame or
/// raise a throw — nothing else needs it, and reading the line table costs a
/// bounds-checked index.
fn mark_frame_line(vm: &VM) {
    let line = cur_op_line(vm);
    with_host(|h| {
        h.set_cur_line(line);
        // A LIBRARY function warns from inside Rust, with no op of its own to
        // read a line from — `range(): … subsequent bytes are ignored` belongs to
        // the line of the call. Setting the warn site here is what gives every
        // such diagnostic a line at all; a user function that goes on to warn
        // overwrites this from its own ops.
        h.set_warn_line(line);
    });
}

fn b_getvar(vm: &mut VM, _: u8) -> Value {
    let name = pop_name(vm);
    mark_warn_site(vm);
    with_host(|h| h.get_var_warn(&name))
}

/// `$name` read with no `Undefined variable` diagnostic — see `ops::GETVAR_Q`.
fn b_getvar_q(vm: &mut VM, _: u8) -> Value {
    let name = pop_name(vm);
    with_host(|h| h.get_var(&name))
}

fn b_setvar(vm: &mut VM, _: u8) -> Value {
    let val = vm.pop();
    let name = pop_name(vm);
    with_host(|h| h.set_var(&name, val.clone()));
    val
}

/// The copy an assignment makes. A PHP array is a value, so `$b = $a`,
/// `$o->p = $a` and `$box[k] = $a` each store a copy; an object, a closure and a
/// stream are handles and pass through. The compiler emits this on the
/// right-hand side of a source-level assignment only, so a compiler temporary,
/// a `&` binding and a `foreach` reference keep the handle they were given.
fn b_copy(vm: &mut VM, _: u8) -> Value {
    let val = vm.pop();
    with_host(|h| h.copy_on_assign(val))
}

fn b_concat(vm: &mut VM, _: u8) -> Value {
    let b = vm.pop();
    let a = vm.pop();
    // Both conversions happen outside the host borrow so an operand with
    // `__toString` can run it. String interpolation lowers to this op too.
    let (a, b) = (host::to_str_ext(&a), host::to_str_ext(&b));
    Value::str(format!("{a}{b}"))
}

fn b_truthy(vm: &mut VM, _: u8) -> Value {
    let v = vm.pop();
    Value::bool(with_host(|h| h.is_truthy(&v)))
}

fn b_call(vm: &mut VM, argc: u8) -> Value {
    let args = pop_args(vm, argc as usize - 1);
    let name = pop_name(vm);
    mark_frame_line(vm);
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
    mark_frame_line(vm);
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
    mark_frame_line(vm);
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

/// Route `$o[k]` through an `ArrayAccess` object's `offsetX` method, if that is
/// what the receiver is. `None` means the receiver is an ordinary array, string
/// or scalar and the caller's own path applies.
fn array_access_call(
    vm: &mut VM,
    recv: &Value,
    method: &str,
    args: Vec<Value>,
) -> Option<Result<Value, String>> {
    let class = with_host(|h| h.array_access_class(recv))?;
    mark_frame_line(vm);
    Some(host::call_method(&class, method, Some(recv.clone()), args))
}

/// The result of an `offsetX` dispatch, with a failure turned into a halted
/// chunk the same way every other method call is.
fn array_access_result(vm: &mut VM, r: Result<Value, String>) -> Value {
    match r {
        Ok(v) => bubbled(vm, v),
        Err(e) => fail(vm, e),
    }
}

fn b_index_get(vm: &mut VM, _: u8) -> Value {
    let key = vm.pop();
    let recv = vm.pop();
    if let Some(r) = array_access_call(vm, &recv, "offsetGet", vec![key.clone()]) {
        return array_access_result(vm, r);
    }
    mark_warn_site(vm);
    with_host(|h| h.index_get_warn(&recv, &key))
}

/// `$a[k]` read with no missing-key diagnostic — see `ops::INDEX_GET_Q`.
fn b_index_get_q(vm: &mut VM, _: u8) -> Value {
    let key = vm.pop();
    let recv = vm.pop();
    // `$o[k] ?? d` on an `ArrayAccess` asks `offsetExists` first and only reads
    // through `offsetGet` once that says yes — the same two-step `??` uses for a
    // magic property.
    if with_host(|h| h.array_access_class(&recv)).is_some() {
        let present = match array_access_call(vm, &recv, "offsetExists", vec![key.clone()]) {
            Some(Ok(v)) => with_host(|h| h.is_truthy(&v)),
            Some(Err(e)) => return fail(vm, e),
            None => false,
        };
        if !present {
            return Value::Undef;
        }
        let r = array_access_call(vm, &recv, "offsetGet", vec![key]);
        return match r {
            Some(r) => array_access_result(vm, r),
            None => Value::Undef,
        };
    }
    with_host(|h| h.index_get(&recv, &key))
}

/// One element of a destructuring assignment — see `ops::LIST_ELEM_GET`.
///
/// A list read is not an index read. PHP refuses to walk into anything that is
/// not an array here, so the string case in particular differs sharply:
/// `"ab"[0]` is `'a'`, while `[$x] = "ab"` warns and assigns null. Only the
/// array case reaches the ordinary keyed read, which is what still supplies the
/// `Undefined array key N` warning for a too-short element.
fn b_list_elem_get(vm: &mut VM, _: u8) -> Value {
    let key = vm.pop();
    let recv = vm.pop();
    // An object is a hard error, not a warning — including ArrayAccess, which
    // PHP does not consult for destructuring.
    let is_obj = with_host(|h| matches!(recv, Value::Obj(_)) && !h.is_array(&recv));
    if is_obj {
        let cls = with_host(|h| h.object_class(&recv).unwrap_or_else(|| "object".into()));
        return throw_php(
            vm,
            "Error",
            &format!("Cannot use object of type {cls} as array"),
        );
    }
    if with_host(|h| h.is_array(&recv)) {
        mark_warn_site(vm);
        return with_host(|h| h.index_get_warn(&recv, &key));
    }
    // `null` destructures to null in silence — it is the one non-array subject
    // PHP does not complain about.
    if matches!(recv, Value::Undef) {
        return Value::Undef;
    }
    let ty = match recv {
        Value::Bool(_) => "bool",
        Value::Int(_) => "int",
        Value::Float(_) => "float",
        Value::Str(_) => "string",
        _ => "mixed",
    };
    mark_warn_site(vm);
    with_host(|h| h.warn(format_args!("Cannot use {ty} as array")));
    Value::Undef
}

/// `const NAME = expr;` — see `ops::CONST_DECL`. The declaration writes the same
/// table `define()` writes, and a redefinition warns there rather than here, so
/// the two spellings cannot drift apart.
fn b_const_decl(vm: &mut VM, _: u8) -> Value {
    let value = vm.pop();
    let name = pop_name(vm);
    // A redefinition warns, and the warning carries a line — which has to be
    // taken from the op before the write, since nothing else sets it on this
    // path and it would otherwise report line 0.
    let line = cur_op_line(vm);
    with_host(|h| {
        h.set_warn_line(line);
        h.const_define(&name, value)
    });
    Value::Undef
}

/// A declaration the reference refuses to link — see `ops::DECL_FATAL`.
///
/// Displayed like an uncaught error (message, then a stack trace) but *not*
/// thrown: PHP raises these below the exception machinery, so no `try`/`catch`
/// can see one and the program always stops here.
fn b_decl_fatal(vm: &mut VM, _: u8) -> Value {
    let msg = with_host(|h| h.to_str(&vm.pop()));
    let line = cur_op_line(vm);
    let body = with_host(|h| {
        let trace = h.backtrace();
        format!(
            "{msg} in {} on line {line}\nStack trace:\n{trace}",
            h.script_name()
        )
    });
    with_host(|h| {
        h.fatal("Fatal error", &body);
        h.ob_flush_all();
    });
    fail(vm, format!("Fatal error:  {body}"))
}

/// `isset($a[k])` — see `ops::INDEX_ISSET`.
fn b_index_isset(vm: &mut VM, _: u8) -> Value {
    let key = vm.pop();
    let recv = vm.pop();
    if let Some(r) = array_access_call(vm, &recv, "offsetExists", vec![key.clone()]) {
        return match r {
            Ok(v) => {
                let b = with_host(|h| h.is_truthy(&v));
                bubbled(vm, Value::bool(b))
            }
            Err(e) => fail(vm, e),
        };
    }
    // Everything else: set means "reads as something other than null".
    Value::bool(!matches!(
        with_host(|h| h.index_get(&recv, &key)),
        Value::Undef
    ))
}

fn b_index_set(vm: &mut VM, _: u8) -> Value {
    let val = vm.pop();
    let key = vm.pop();
    let name = pop_name(vm);
    // Writing through an `ArrayAccess` object must NOT replace it with an array.
    let recv = with_host(|h| h.get_var(&name));
    if let Some(r) = array_access_call(vm, &recv, "offsetSet", vec![key.clone(), val.clone()]) {
        if let Err(e) = r {
            return fail(vm, e);
        }
        return bubbled(vm, val);
    }
    with_host(|h| h.index_set_var(&name, &key, val.clone()));
    val
}

fn b_arr_append(vm: &mut VM, _: u8) -> Value {
    let val = vm.pop();
    let name = pop_name(vm);
    // `$o[] = v` is `offsetSet(null, v)` — the null offset is how the object
    // learns the write had no key.
    let recv = with_host(|h| h.get_var(&name));
    if let Some(r) = array_access_call(vm, &recv, "offsetSet", vec![Value::Undef, val.clone()]) {
        if let Err(e) = r {
            return fail(vm, e);
        }
        return bubbled(vm, val);
    }
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
    mark_warn_site(vm);
    with_host(|h| h.index_get_path_warn(&name, &keys))
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
    mark_warn_site(vm);
    let old = with_host(|h| h.index_get_path_warn(&name, &keys));
    if incdec_refused(vm, &old, inc) {
        return Value::Undef;
    }
    with_host(|h| {
        let newv = h.incdec_value(&old, inc);
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
    mark_warn_site(vm);
    // `$x++` on an unset variable reports it, exactly as reading it would.
    let old = with_host(|h| h.get_var_warn(&name));
    if incdec_refused(vm, &old, inc) {
        return Value::Undef;
    }
    with_host(|h| {
        let newv = h.incdec_value(&old, inc);
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
    mark_frame_line(vm);
    match host::new_object(&class, args) {
        Ok(v) => bubbled(vm, v),
        Err(e) => fail(vm, e),
    }
}

/// Run a magic property method (`__get`, `__set`, `__isset`, `__unset`) with the
/// recursion guard held, so an access to the SAME property from inside its own
/// magic method does not re-enter it. The guard is released on every path,
/// including the one where the method throws.
fn call_magic(recv: &Value, name: &str, magic: &'static str, args: Vec<Value>) -> Value {
    let Some(class) = with_host(|h| h.object_class(recv)) else {
        return Value::Undef;
    };
    with_host(|h| h.magic_enter(recv, name, magic));
    let out = host::call_method(&class, magic, Some(recv.clone()), args);
    with_host(|h| h.magic_leave());
    out.unwrap_or(Value::Undef)
}

/// The receiver and property for a property opcode, plus the plan
/// [`host::PhpHost::prop_access`] produced for `magic`. Every property opcode
/// opens the same way, and the `Denied` arm has to reach `throw_php`, which needs
/// the VM — so the shared part stops just short of acting on the plan.
macro_rules! prop_plan {
    ($vm:expr, $recv:expr, $name:expr, $magic:expr) => {{
        mark_frame_line($vm);
        with_host(|h| h.prop_access(&$recv, &$name, $magic))
    }};
}

fn b_prop_get(vm: &mut VM, _: u8) -> Value {
    let name = pop_name(vm);
    let recv = vm.pop();
    match prop_plan!(vm, recv, name, "__get") {
        PropAccess::Direct => {
            mark_warn_site(vm);
            with_host(|h| h.prop_get_warn(&recv, &name))
        }
        PropAccess::Magic => {
            let v = call_magic(&recv, &name, "__get", vec![Value::str(name.clone())]);
            bubbled(vm, v)
        }
        PropAccess::Denied(msg) => throw_php(vm, "Error", &msg),
        PropAccess::Absent => {
            mark_warn_site(vm);
            with_host(|h| h.prop_get_warn(&recv, &name))
        }
    }
}

/// `$o->p` read with no missing-property diagnostic — see `ops::PROP_GET_Q`.
///
/// This is the read `empty()` and `??` compile to. The reference does NOT raise
/// an access error here — an unreachable property is simply "not set" — and it
/// consults the magic methods in a specific order, all three arms of which were
/// read back off it:
///
/// * `__isset` first, when the class has one. A false answer ends the read at
///   null; `__get` is never called, so `$o->p ?? 'd'` on a class whose `__isset`
///   says no is `'d'` without `__get` ever seeing the property.
/// * `__get` for the value, after `__isset` allowed it — or straight away when
///   the class has no `__isset` at all.
/// * A class with `__isset` but no `__get` reads as null even when `__isset`
///   returned true. `isset()` still says true, which is why that question has its
///   own opcode (`ops::PROP_ISSET`) instead of testing this value against null.
fn b_prop_get_q(vm: &mut VM, _: u8) -> Value {
    let name = pop_name(vm);
    let recv = vm.pop();
    prop_quiet_read(vm, &recv, &name, true)
}

/// The read inside `empty($o->p)` — see `ops::PROP_GET_EMPTY`.
fn b_prop_get_empty(vm: &mut VM, _: u8) -> Value {
    let name = pop_name(vm);
    let recv = vm.pop();
    prop_quiet_read(vm, &recv, &name, false)
}

/// The shared body of the two isset-mode property reads. `get_without_isset`
/// is the single arm they differ on: whether a class that has `__get` but no
/// `__isset` may be read through `__get` anyway. `??` says yes, `empty()` says no.
fn prop_quiet_read(vm: &mut VM, recv: &Value, name: &str, get_without_isset: bool) -> Value {
    let recv = recv.clone();
    let name = name.to_string();
    if let PropAccess::Direct = prop_plan!(vm, recv, name, "__isset") {
        return with_host(|h| h.prop_get(&recv, &name));
    }
    // `__isset` gates the read, when there is one to ask.
    let has_isset = matches!(prop_plan!(vm, recv, name, "__isset"), PropAccess::Magic);
    if has_isset {
        let present = call_magic(&recv, &name, "__isset", vec![Value::str(name.clone())]);
        if bubble_throw(vm) {
            return Value::Undef;
        }
        if !with_host(|h| h.is_truthy(&present)) {
            return Value::Undef;
        }
    } else if !get_without_isset {
        return Value::Undef;
    }
    match prop_plan!(vm, recv, name, "__get") {
        PropAccess::Magic => {
            let v = call_magic(&recv, &name, "__get", vec![Value::str(name.clone())]);
            bubbled(vm, v)
        }
        PropAccess::Direct => with_host(|h| h.prop_get(&recv, &name)),
        PropAccess::Denied(_) | PropAccess::Absent => Value::Undef,
    }
}

fn b_prop_set(vm: &mut VM, _: u8) -> Value {
    let val = vm.pop();
    let name = pop_name(vm);
    let recv = vm.pop();
    match prop_plan!(vm, recv, name, "__set") {
        PropAccess::Direct | PropAccess::Absent => {
            if readonly_refused(vm, &recv, &name) {
                return Value::Undef;
            }
            mark_warn_site(vm);
            with_host(|h| h.prop_set_checked(&recv, &name, val.clone()));
            val
        }
        PropAccess::Magic => {
            call_magic(
                &recv,
                &name,
                "__set",
                vec![Value::str(name.clone()), val.clone()],
            );
            bubbled(vm, val)
        }
        PropAccess::Denied(msg) => throw_php(vm, "Error", &msg),
    }
}

/// Screen a source-level write to `$recv->name` against the readonly rule.
///
/// `true` means the chunk has been halted with the `Error` PHP raises and the
/// caller must not write. `false` means the write may go ahead — and, for the
/// one write a readonly property is allowed, that it has now been taken, which
/// is why this must be called by the writer and not by a read path.
fn readonly_refused(vm: &mut VM, recv: &Value, name: &str) -> bool {
    match with_host(|h| h.readonly_write_error(recv, name)) {
        Some(msg) => {
            throw_php(vm, "Error", &msg);
            true
        }
        None => {
            with_host(|h| h.readonly_note_init(recv, name));
            false
        }
    }
}

/// `$o->name = val` for the write half of a compound assignment — see
/// `ops::PROP_SET_RW`. Stack `[recv, name, val]`.
fn b_prop_set_rw(vm: &mut VM, _: u8) -> Value {
    let val = vm.pop();
    let name = pop_name(vm);
    let recv = vm.pop();
    // `__set` runs here only when `__get` supplied the value that was modified.
    // A class with `__set` and NO `__get` does not use it for a read-modify-write
    // at all: the reference reads the property directly (warning that it is
    // undefined) and writes it directly, creating a real one. `__set` is part of
    // the magic PAIR, and half a pair is not enough to divert the write.
    let has_get = with_host(|h| {
        h.object_class(&recv)
            .is_some_and(|c| h.class_has_method(&c, "__get"))
    });
    match prop_plan!(vm, recv, name, "__set") {
        PropAccess::Magic if has_get => {
            call_magic(
                &recv,
                &name,
                "__set",
                vec![Value::str(name.clone()), val.clone()],
            );
            bubbled(vm, val)
        }
        PropAccess::Denied(msg) => throw_php(vm, "Error", &msg),
        // Writing the slot itself — but only ONE of the two paths here announces
        // a dynamic property, because only one of them is the first to know.
        //
        // With `__get` the `PROP_TOUCH` that opened this read-modify-write stayed
        // silent (the read went to `__get`, creating nothing), so this write is
        // what creates the property and the deprecation belongs here — after
        // `__get` has printed, which is the order the reference uses.
        //
        // Without `__get` that touch already announced it, and announcing it a
        // second time would print the deprecation twice.
        _ if has_get => {
            if readonly_refused(vm, &recv, &name) {
                return Value::Undef;
            }
            mark_warn_site(vm);
            with_host(|h| h.prop_set_checked(&recv, &name, val.clone()));
            val
        }
        _ => {
            if readonly_refused(vm, &recv, &name) {
                return Value::Undef;
            }
            with_host(|h| h.prop_set(&recv, &name, val.clone()));
            val
        }
    }
}

/// `isset($o->name)` — see `ops::PROP_ISSET`. Stack `[recv, name]`.
///
/// Asks `__isset` and nothing else. An unreachable property is `false` rather
/// than an error: `isset()` is allowed to ask about anything.
fn b_prop_isset(vm: &mut VM, _: u8) -> Value {
    let name = pop_name(vm);
    let recv = vm.pop();
    match prop_plan!(vm, recv, name, "__isset") {
        PropAccess::Direct => {
            let v = with_host(|h| h.prop_get(&recv, &name));
            Value::bool(!matches!(v, Value::Undef))
        }
        PropAccess::Magic => {
            let present = call_magic(&recv, &name, "__isset", vec![Value::str(name.clone())]);
            if bubble_throw(vm) {
                return Value::Undef;
            }
            Value::bool(with_host(|h| h.is_truthy(&present)))
        }
        PropAccess::Denied(_) | PropAccess::Absent => Value::bool(false),
    }
}

/// `unset($o->name)` — see `ops::PROP_UNSET`. Stack `[recv, name]`.
fn b_prop_unset(vm: &mut VM, _: u8) -> Value {
    let name = pop_name(vm);
    let recv = vm.pop();
    match prop_plan!(vm, recv, name, "__unset") {
        PropAccess::Direct => {
            if let Some(msg) = with_host(|h| h.readonly_unset_error(&recv, &name)) {
                return throw_php(vm, "Error", &msg);
            }
            with_host(|h| h.prop_remove(&recv, &name));
            Value::Undef
        }
        PropAccess::Magic => {
            call_magic(&recv, &name, "__unset", vec![Value::str(name.clone())]);
            bubbled(vm, Value::Undef)
        }
        PropAccess::Denied(msg) => throw_php(vm, "Error", &msg),
        // Unsetting a property that is not there is not an error.
        PropAccess::Absent => Value::Undef,
    }
}

/// `$o->name` fetched for writing — see `ops::PROP_TOUCH`. Raises the
/// dynamic-property deprecation and leaves the receiver on the stack for the
/// read that follows. Stack `[recv, name]` -> `[recv]`.
fn b_prop_touch(vm: &mut VM, _: u8) -> Value {
    let name = pop_name(vm);
    let recv = vm.pop();
    // A class with `__get` does not get a slot here: the read half will call
    // `__get`, and nothing is created until the write half decides what to do.
    // So the deprecation must not fire yet — and when the write does create a
    // property, it fires AFTER `__get` has run, which is the order the reference
    // prints (`[Gq]` then `Creation of dynamic property`).
    if let PropAccess::Magic = prop_plan!(vm, recv, name, "__get") {
        return recv;
    }
    mark_warn_site(vm);
    with_host(|h| h.warn_dynamic_prop(&recv, &name));
    recv
}

/// Vivify `$o->name` into an array and leave its handle on the stack — the pivot
/// for indexing/appending into an array-valued property. Stack `[recv, name]`.
fn b_prop_ensure_array(vm: &mut VM, _: u8) -> Value {
    let name = pop_name(vm);
    let recv = vm.pop();
    // A property out of reach still errors before anything is vivified; the
    // other outcomes all end in a write, which is what this op is for.
    if let PropAccess::Denied(msg) = prop_plan!(vm, recv, name, "__get") {
        return throw_php(vm, "Error", &msg);
    }
    // `$o->tags[] = x` never writes the property itself, so it is not the
    // readonly WRITE rule that stops it — PHP refuses to hand out a modifiable
    // reference into a readonly property at all, with its own wording.
    if let Some(msg) = with_host(|h| h.readonly_indirect_error(&recv, &name)) {
        return throw_php(vm, "Error", &msg);
    }
    mark_warn_site(vm);
    with_host(|h| {
        // `$o->missing[] = 1` vivifies the property, so it too creates a dynamic
        // one when the class never declared it.
        h.warn_dynamic_prop(&recv, &name);
        h.prop_ensure_array(&recv, &name)
    })
}

/// Open an `@expr` suppression region — see `ops::SUPPRESS_PUSH`.
fn b_suppress_push(_: &mut VM, _: u8) -> Value {
    with_host(|h| h.suppress_push());
    Value::Undef
}

/// Close an `@expr` suppression region, passing the operand's value through —
/// see `ops::SUPPRESS_POP`.
fn b_suppress_pop(vm: &mut VM, _: u8) -> Value {
    let v = vm.pop();
    with_host(|h| h.suppress_pop());
    v
}

/// `++`/`--` on `$o->name` — stack `[recv, name, code]`. The `code` bits match
/// `b_incdec` (bit0 = increment, bit1 = prefix).
fn b_prop_incdec(vm: &mut VM, _: u8) -> Value {
    let code = vm.pop().to_int();
    let name = pop_name(vm);
    let recv = vm.pop();
    let inc = code & 1 != 0;
    let prefix = code & 2 != 0;
    match prop_plan!(vm, recv, name, "__get") {
        PropAccess::Denied(msg) => throw_php(vm, "Error", &msg),
        // Same read-modify-write shape as `$o->p += 1`: `__get` supplies the old
        // value, and `__set` takes the new one back only if the class has both.
        PropAccess::Magic => {
            let old = call_magic(&recv, &name, "__get", vec![Value::str(name.clone())]);
            if bubble_throw(vm) {
                return Value::Undef;
            }
            if incdec_refused(vm, &old, inc) {
                return Value::Undef;
            }
            let newv = with_host(|h| h.incdec_value(&old, inc));
            let has_set = with_host(|h| {
                h.object_class(&recv)
                    .is_some_and(|c| h.class_has_method(&c, "__set"))
            });
            if has_set {
                call_magic(
                    &recv,
                    &name,
                    "__set",
                    vec![Value::str(name.clone()), newv.clone()],
                );
            } else {
                if readonly_refused(vm, &recv, &name) {
                    return Value::Undef;
                }
                mark_warn_site(vm);
                with_host(|h| h.prop_set_checked(&recv, &name, newv.clone()));
            }
            bubbled(vm, if prefix { newv } else { old })
        }
        _ => {
            mark_warn_site(vm);
            // PHP raises the dynamic-property deprecation BEFORE the
            // undefined-property warning here: the slot is created, then read.
            let old = with_host(|h| {
                h.warn_dynamic_prop(&recv, &name);
                h.prop_get_warn(&recv, &name)
            });
            if incdec_refused(vm, &old, inc) {
                return Value::Undef;
            }
            if readonly_refused(vm, &recv, &name) {
                return Value::Undef;
            }
            with_host(|h| {
                let newv = h.incdec_value(&old, inc);
                h.prop_set(&recv, &name, newv.clone());
                if prefix {
                    newv
                } else {
                    old
                }
            })
        }
    }
}

fn b_mcall(vm: &mut VM, argc: u8) -> Value {
    let mut args = pop_args(vm, argc as usize);
    let recv = args.remove(0);
    let method = with_host(|h| h.to_str(&args.remove(0)));
    mark_frame_line(vm);
    // `Closure` and `Generator` are built-in objects with no PHP class; dispatch
    // their methods before the ordinary class-method resolution.
    if with_host(|h| h.is_closure(&recv)) {
        return match host::call_closure_method(&recv, &method, args) {
            Ok(v) => bubbled(vm, v),
            Err(e) => fail(vm, e),
        };
    }
    if with_host(|h| h.is_generator_val(&recv)) {
        return match host::call_generator_method(&recv, &method, args) {
            Ok(v) => bubbled(vm, v),
            Err(e) => yield_err(vm, e),
        };
    }
    let class = with_host(|h| h.object_class(&recv));
    match class {
        Some(c) => {
            let magic = match method_plan(vm, &c, &method, true) {
                Ok(m) => m,
                Err(v) => return v,
            };
            let r = if magic {
                host::call_magic_call(&c, &method, Some(recv), args)
            } else {
                host::call_method(&c, &method, Some(recv), args)
            };
            match r {
                Ok(v) => bubbled(vm, v),
                Err(e) => fail(vm, e),
            }
        }
        None => {
            let ty = with_host(|h| receiver_type_name(h, &recv));
            throw_php(
                vm,
                "Error",
                &format!("Call to a member function {method}() on {ty}"),
            )
        }
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
    mark_frame_line(vm);
    let magic = match static_method_plan(vm, &class, &method, &this) {
        Ok(m) => m,
        Err(v) => return v,
    };
    let r = if magic {
        host::call_magic_call(&class, &method, this, args)
    } else {
        host::call_method(&class, &method, this, args)
    };
    match r {
        Ok(v) => bubbled(vm, v),
        Err(e) => fail(vm, e),
    }
}

fn b_sconst(vm: &mut VM, _: u8) -> Value {
    let name = pop_name(vm);
    let class = pop_name(vm);
    match host::class_const(&class, &name) {
        Ok(v) => v,
        Err(e) => fail_or_throw(vm, e),
    }
}

// ── arithmetic builtins (PHP semantics) ──────────────────────────────────────

/// Resolve both operands of an operator compiled as a builtin, halting the chunk
/// with a pending `TypeError` when one has no numeric reading.
///
/// The operand rules run *before* the operator's own checks, which is
/// observable: `"g" / 0` is a `TypeError`, not a `DivisionByZeroError`.
fn arith_args(vm: &mut VM, sym: &str, a: &Value, b: &Value) -> Option<(Value, Value)> {
    mark_warn_site(vm);
    match coerce_arith_pair(sym, a, b) {
        Ok(pair) => Some(pair),
        Err(_) => {
            mark_frame_line(vm);
            vm.ip = vm.chunk.ops.len();
            None
        }
    }
}

fn b_div(vm: &mut VM, _: u8) -> Value {
    let b = vm.pop();
    let a = vm.pop();
    let Some((an, bn)) = arith_args(vm, "/", &a, &b) else {
        return Value::Undef;
    };
    if bn.to_float() == 0.0 {
        return throw_php(vm, "DivisionByZeroError", "Division by zero");
    }
    match (an, bn) {
        (Value::Int(x), Value::Int(y)) if x % y == 0 => Value::int(x / y),
        (an, bn) => Value::float(an.to_float() / bn.to_float()),
    }
}

fn b_mod(vm: &mut VM, _: u8) -> Value {
    let b = vm.pop();
    let a = vm.pop();
    let Some((x, y)) = int_args(vm, "%", &a, &b) else {
        return Value::Undef;
    };
    if y == 0 {
        return throw_php(vm, "DivisionByZeroError", "Modulo by zero");
    }
    Value::int(x % y)
}

/// PHP 8.4 deprecated a zero base raised to a negative exponent; the result is
/// still `INF` (or `-INF`, from a `-0.0` base and an odd exponent), which
/// `powf` already gives, so only the diagnostic is added.
///
/// The test is on the COERCED operands, which is why `"0" ** -1` and
/// `false ** -1` fire too. It is `< 0.0` rather than `<= 0.0` because a negative
/// ZERO exponent is not negative: `0 ** -0.0` is 1.0 and says nothing. `NAN`
/// fails the same comparison, and `0 ** NAN` is likewise silent.
///
/// Both call sites of a PHP power reach this: the `**` operator (and `**=`,
/// which compiles to the same opcode) and the `pow()` library function.
fn deprecate_zero_base_negative_exponent(an: &Value, bn: &Value) {
    if an.to_float() == 0.0 && bn.to_float() < 0.0 {
        with_host(|h| h.deprecated("Power of base 0 and negative exponent is deprecated"));
    }
}

fn b_pow(vm: &mut VM, _: u8) -> Value {
    let b = vm.pop();
    let a = vm.pop();
    let Some((an, bn)) = arith_args(vm, "**", &a, &b) else {
        return Value::Undef;
    };
    deprecate_zero_base_negative_exponent(&an, &bn);
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
    // Two objects are `==` when they are of the SAME class and every property is
    // loose-equal — a different pair of instances can compare equal, which is
    // exactly what separates `==` from `===` on objects. An array is never `==`
    // an object, so the mixed pair is rejected before the element walk.
    if h.is_object(a) || h.is_object(b) {
        let (Some(ca), Some(cb)) = (h.object_class(a), h.object_class(b)) else {
            return false;
        };
        if !ca.eq_ignore_ascii_case(&cb) {
            return false;
        }
        let (pa, pb) = (h.object_props(a), h.object_props(b));
        return pa.len() == pb.len()
            && pa
                .iter()
                .all(|(name, va)| pb.iter().any(|(n, vb)| n == name && loose_eq(h, va, vb)));
    }
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
        (Obj(x), Obj(y)) => {
            // The same heap handle is always identical (same array or object
            // instance) — this also gives enum-case singletons `===` identity.
            if x == y {
                return true;
            }
            // Two distinct array handles are `===` when their keys/values match in
            // order and type; two distinct object handles are never identical.
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

/// PHP ordering: -1 / 0 / 1, following `zend_compare`'s operand-type table.
///
/// The table is applied in this order, which is what the reference interpreter
/// does and is load-bearing — swapping any two arms changes results:
/// 1. `null` vs string — `null` becomes `""` and the two compare as strings
///    (so `null < "0"`, which the bool rule below would call equal).
/// 2. `bool` or `null` on either side — both operands convert to bool
///    (so `null < -1`, because `false < true`).
/// 3. array vs array — fewer elements is smaller, otherwise element-wise over
///    the left operand's keys; a key missing on the right is "uncomparable"
///    and reports greater, matching `zend_hash_compare`.
/// 4. array vs anything else — the array is always greater.
/// 5. everything else — numeric unless both operands are non-numeric strings
///    (then byte comparison).
fn php_compare(h: &host::PhpHost, a: &Value, b: &Value) -> i32 {
    use Value::*;
    match (a, b) {
        // 1. null vs string compares as "" vs the string.
        (Undef, Str(y)) => strcmp_i32("", y),
        (Str(x), Undef) => strcmp_i32(x, ""),
        // 2. A bool or null operand drags both sides down to bool.
        (Bool(_) | Undef, _) | (_, Bool(_) | Undef) => {
            i32::from(h.is_truthy(a)) - i32::from(h.is_truthy(b))
        }
        // 3. Two arrays compare by size, then element-wise.
        (Obj(_), Obj(_)) => compare_arrays(h, a, b),
        // 4. An array outranks every non-array, non-bool, non-null operand.
        (Obj(_), _) => 1,
        (_, Obj(_)) => -1,
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
        _ => cmp_f64(h.to_number(a).to_float(), h.to_number(b).to_float()),
    }
}

/// Order two arrays the way `zend_hash_compare` does: the array with fewer
/// elements is smaller; at equal size, walk the left operand's keys in order and
/// compare the values. A key absent from the right operand makes the pair
/// uncomparable, which the engine reports as "left is greater"
/// (`['a'=>1] <=> ['b'=>1]` is `1`).
fn compare_arrays(h: &host::PhpHost, a: &Value, b: &Value) -> i32 {
    let (Some(pa), Some(pb)) = (h.array_pairs(a), h.array_pairs(b)) else {
        return 0;
    };
    if pa.len() != pb.len() {
        return if pa.len() < pb.len() { -1 } else { 1 };
    }
    for (ka, va) in &pa {
        let Some((_, vb)) = pb.iter().find(|(kb, _)| strict_eq(h, ka, kb)) else {
            return 1;
        };
        let ord = php_compare(h, va, vb);
        if ord != 0 {
            return ord;
        }
    }
    0
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

/// The operator PHP names in `Unsupported operand types: X <sym> Y`.
///
/// `Neg` reports `*` because the reference lowers unary minus to a
/// multiplication by `-1`: `-"g"` says `string * int`, not `string - int`.
fn op_symbol(op: NumOp) -> &'static str {
    match op {
        NumOp::Add => "+",
        NumOp::Sub => "-",
        NumOp::Mul | NumOp::Neg => "*",
        NumOp::Div => "/",
        NumOp::Mod => "%",
        NumOp::Pow => "**",
        _ => "?",
    }
}

/// Record a `TypeError` for `a <sym> b` and return the message to stop the VM.
///
/// The pending exception is the real signal — `run_chunk_on` turns this stop
/// into the clean halt that `catch` can see — so the returned string is only
/// ever the text of a fatal that nothing caught.
pub(crate) fn unsupported_operands(sym: &str, a: &Value, b: &Value) -> String {
    let msg = with_host(|h| {
        format!(
            "Unsupported operand types: {} {sym} {}",
            host::arith_type_name(h, a),
            host::arith_type_name(h, b)
        )
    });
    let _ = pending_php_throw("TypeError", &msg);
    msg
}

/// Coerce one operand of `a <sym> b` under PHP 8's rules, raising the
/// reference's diagnostics on the way.
///
/// Callers must resolve the left operand before the right, because the order is
/// observable: `"5g" + "g"` warns once and *then* throws, while `"g" + "5g"`
/// throws without warning at all.
pub(crate) fn coerce_arith(sym: &str, a: &Value, b: &Value, left: bool) -> Result<Value, String> {
    let v = if left { a } else { b };
    match with_host(|h| host::classify_arith(h, v)) {
        host::ArithOperand::Numeric(n) => Ok(n),
        host::ArithOperand::Leading(n) => {
            with_host(|h| h.warn("A non-numeric value encountered"));
            Ok(n)
        }
        host::ArithOperand::Unsupported => Err(unsupported_operands(sym, a, b)),
    }
}

/// Both operands of a binary arithmetic operator, left resolved first.
pub(crate) fn coerce_arith_pair(sym: &str, a: &Value, b: &Value) -> Result<(Value, Value), String> {
    let an = coerce_arith(sym, a, b, true)?;
    let bn = coerce_arith(sym, a, b, false)?;
    Ok((an, bn))
}

/// Supplies PHP arithmetic for the native `Add`/`Sub`/`Mul`/`Negate` ops when an
/// operand is non-numeric (string/array/bool/null) or an `i64` op overflows.
///
/// `lines` is the running chunk's line table, captured per VM so a warning
/// raised here reports the operator's own line; `ip` indexes it directly.
pub fn numeric_hook_sited(call: NumericCall<'_>, lines: &[u32]) -> Result<Value, String> {
    let (op, a, b) = (call.op, call.a, call.b);
    // `[..] + [..]` is array union, not arithmetic, and must not reach the
    // operand rules below — the reference keeps the left operand's entries and
    // adds only the right's keys that are missing.
    if op == NumOp::Add && with_host(|h| h.is_array(a) && h.is_array(b)) {
        return Ok(array_union(a, b));
    }
    with_host(|h| h.set_warn_line(lines.get(call.ip).copied().unwrap_or(0)));
    let sym = op_symbol(op);
    if op == NumOp::Neg {
        // Unary minus is `$x * -1`, so the reported right-hand type is `int`.
        let an = coerce_arith(sym, a, &Value::Int(-1), true)?;
        return Ok(match an {
            Value::Int(n) => n
                .checked_neg()
                .map(Value::int)
                .unwrap_or(Value::float(-(n as f64))),
            Value::Float(f) => Value::float(-f),
            _ => Value::int(0),
        });
    }
    let (an, bn) = coerce_arith_pair(sym, a, b)?;
    Ok(arith(op, an, bn))
}

/// PHP's `+` on two arrays: the left operand's entries win, and the right
/// contributes only the keys the left does not already have.
fn array_union(a: &Value, b: &Value) -> Value {
    with_host(|h| {
        let pa = h.array_pairs(a).unwrap_or_default();
        let pb = h.array_pairs(b).unwrap_or_default();
        let out = h.new_array();
        for (k, v) in &pa {
            h.arr_set_key(&out, k, v.clone());
        }
        for (k, v) in &pb {
            if !pa.iter().any(|(ka, _)| same_key(ka, k)) {
                h.arr_set_key(&out, k, v.clone());
            }
        }
        out
    })
}

/// Array keys are already normalized to `Int`/`Str`, so identity is a plain
/// comparison of those two shapes.
fn same_key(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        _ => false,
    }
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

/// Library functions whose parameters PHP declares as `string`, so an object
/// argument with a `__toString` method is converted before the call — PHP does
/// this from the function's arginfo, and this list is that arginfo.
///
/// It is an allow-list on purpose. The complement — listing the functions that
/// legitimately *take* an object (`get_class`, `var_dump`, `json_encode`, every
/// callable-taking function, …) — would turn one forgotten entry into an object
/// silently flattened to a string. Here a forgotten entry only means
/// `__toString` is not applied to that function's arguments yet, which shows up
/// as a visible difference rather than a wrong answer.
const STRING_PARAM_BUILTINS: &[&str] = &[
    "addslashes",
    "base64_decode",
    "base64_encode",
    "bin2hex",
    "chunk_split",
    "crc32",
    // `exit`'s `$status` is `string|int`, so a Stringable object satisfies it by
    // converting — `exit($obj)` prints what `__toString` returns.
    "exit",
    "explode",
    "hex2bin",
    "html_entity_decode",
    "htmlentities",
    "htmlspecialchars",
    "htmlspecialchars_decode",
    "lcfirst",
    "levenshtein",
    "ltrim",
    "md5",
    "metaphone",
    "nl2br",
    "ord",
    "preg_quote",
    "printf",
    "quotemeta",
    "rawurldecode",
    "rawurlencode",
    "rtrim",
    "sha1",
    "similar_text",
    "soundex",
    "sprintf",
    "str_contains",
    "str_ends_with",
    "str_pad",
    "str_repeat",
    "str_split",
    "str_starts_with",
    "str_word_count",
    "strcasecmp",
    "strcmp",
    "stripos",
    "stripslashes",
    "stristr",
    "strlen",
    "strnatcasecmp",
    "strnatcmp",
    "strncasecmp",
    "strncmp",
    "strpbrk",
    "strpos",
    "strrchr",
    "strrev",
    "strripos",
    "strrpos",
    "strstr",
    "strtolower",
    "strtoupper",
    "strtr",
    "substr",
    "substr_count",
    "substr_replace",
    "trim",
    "ucfirst",
    "ucwords",
    "urldecode",
    "urlencode",
    "vprintf",
    "vsprintf",
    "wordwrap",
];

/// Apply `__toString` to the arguments of a string-parameter library function.
///
/// Runs before any host borrow is taken: converting calls the object's method,
/// which re-enters the host. Costs one list lookup unless an object argument is
/// actually present.
fn coerce_stringable_args(lname: &str, args: &[Value]) -> Option<Vec<Value>> {
    if !args.iter().any(|a| matches!(a, Value::Obj(_))) || !STRING_PARAM_BUILTINS.contains(&lname) {
        return None;
    }
    Some(
        args.iter()
            .map(|a| {
                let stringable = with_host(|h| {
                    h.object_class(a)
                        .is_some_and(|c| h.class_has_method(&c, "__tostring"))
                });
                if stringable {
                    Value::str(host::to_str_ext(a))
                } else {
                    a.clone()
                }
            })
            .collect(),
    )
}

/// Separates the exception class from the message inside a tagged library error.
/// U+0001 cannot occur in a PHP diagnostic, so a plain error message can never be
/// mistaken for a tagged one.
const THROW_TAG: &str = "\u{1}throw\u{1}";

/// Report a library argument error as the PHP exception it really is.
///
/// A standard-library function that rejects its arguments does not abort the
/// process in PHP — it THROWS, catchably, and the throw carries a stack trace
/// whose `#0` frame is the library call itself. Library functions here return
/// `Result<Value, String>` and have no VM handle, so the class travels back in
/// the error string under [`THROW_TAG`] and [`call_library`] turns it into a real
/// throw at the one place that still knows the function name and its arguments —
/// the two things the trace frame needs.
///
/// ```ignore
/// return Err(throws("ValueError", "range(): Argument #3 ($step) must be …"));
/// ```
pub fn throws(class: &str, message: impl std::fmt::Display) -> String {
    format!("{THROW_TAG}{class}{THROW_TAG}{message}")
}

/// Split a tagged library error back into `(class, message)`; `None` for an
/// ordinary error string, which stays a scaffold-level failure.
pub fn untag_throw(e: &str) -> Option<(&str, &str)> {
    e.strip_prefix(THROW_TAG)?.split_once(THROW_TAG)
}

/// Marker for a throw raised *at* the call site rather than inside the callee —
/// see [`throws_bare`].
const BARE_THROW_TAG: &str = "\u{1}bthrow\u{1}";

/// [`throws`] for a failure the engine reports from the CALLING frame, with no
/// trace entry for the function named in the message.
///
/// "Call to undefined function f()" is the case: there is no `f` to enter, so
/// PHP's trace starts at the caller. Routing it through [`throws`] would invent
/// a `#0 f()` frame the reference does not print.
pub fn throws_bare(class: &str, message: impl std::fmt::Display) -> String {
    format!("{BARE_THROW_TAG}{class}{BARE_THROW_TAG}{message}")
}

/// Split a [`throws_bare`] error back into `(class, message)`.
pub fn untag_bare_throw(e: &str) -> Option<(&str, &str)> {
    e.strip_prefix(BARE_THROW_TAG)?.split_once(BARE_THROW_TAG)
}

/// Dispatch a PHP library function by (case-insensitive) name.
///
/// An argument error comes back as a [`throws`]-tagged `Err`, which
/// `host::call_function` turns into a real, catchable exception thrown from a
/// frame naming this call; every other `Err` is a scaffold-level failure with no
/// PHP equivalent. The decoding deliberately lives in the CALLER, which still
/// owns the argument list the trace frame has to print.
pub fn call_library(name: &str, args: &[Value]) -> Result<Value, String> {
    let lname = name.to_ascii_lowercase();
    let coerced = coerce_stringable_args(&lname, args);
    let args: &[Value] = coerced.as_deref().unwrap_or(args);
    // A builtin with a by-reference OUT parameter publishes its value for the
    // call site to write back (see `BYREF_BUILTINS`); clearing first means a
    // call that writes none cannot be read as having written the last one's.
    with_host(|h| h.byref_out_clear());
    let v = match lname.as_str() {
        // `exit` / `die` — end the request. PHP 8.4 turned both into real
        // functions (`function_exists("exit")` answers true), and the scanner
        // folds `die` onto `exit`, so both spellings arrive here under the one
        // name and every diagnostic quotes `exit()`.
        //
        // `$status` is declared `string|int`: an int becomes the process status,
        // a STRING is printed and the status is 0, a bool and a float narrow to
        // int (a float with a fraction raising the same lost-precision
        // deprecation any other int-only position raises), an explicit null is
        // deprecated and reads as 0, and anything else is a `TypeError`.
        //
        // Nothing is torn down here. Recording the status and unwinding is what
        // the reference does too — an open `ob_start` buffer still flushes on
        // the way out, and only then does the CLI wrapper take the status as the
        // process exit code.
        "exit" | "die" => {
            let status = match args.first() {
                None => 0,
                Some(Value::Undef) => {
                    with_host(|h| {
                        h.deprecated(
                            "exit(): Passing null to parameter #1 ($status) of type string|int \
                             is deprecated",
                        )
                    });
                    0
                }
                Some(Value::Str(s)) => {
                    with_host(|h| h.write_out(s));
                    0
                }
                Some(v @ (Value::Int(_) | Value::Bool(_) | Value::Float(_))) => int_operand(v, v),
                Some(other) => {
                    let t = with_host(|h| crate::stdlib::types::debug_type(h, other));
                    return Err(throws(
                        "TypeError",
                        format!(
                            "exit(): Argument #1 ($status) must be of type string|int, {t} given"
                        ),
                    ));
                }
            };
            // Only the low byte reaches the shell, and that is the reference's
            // arithmetic too: `exit(300)` leaves 44 and `exit(256)` leaves 0.
            host::set_pending_exit((status & 0xFF) as i32);
            Value::Undef
        }
        "strlen" => Value::int(with_host(|h| h.to_str(&arg(args, 0)).len() as i64)),
        // `count` accepts an array or a `Countable`, and NOTHING else: PHP 8
        // rejects a scalar with a TypeError rather than answering 1.
        "count" | "sizeof" => {
            let a = arg(args, 0);
            // `$mode` is COUNT_NORMAL (0) or COUNT_RECURSIVE (1) and nothing
            // else — any other value is a ValueError, checked before the
            // subject so `count(1, 99)` reports the mode.
            let mode = args.get(1).map(|m| m.to_int()).unwrap_or(0);
            if mode != 0 && mode != 1 {
                return Err(throws(
                    "ValueError",
                    "count(): Argument #2 ($mode) must be either COUNT_NORMAL or COUNT_RECURSIVE",
                ));
            }
            if with_host(|h| h.is_array(&a)) {
                Value::int(with_host(|h| {
                    if mode == 1 {
                        count_recursive(h, &a)
                    } else {
                        h.array_len(&a)
                    }
                }))
            } else if let Some(class) = with_host(|h| {
                h.object_class(&a)
                    .filter(|c| h.class_is_a_pub(c, "Countable"))
            }) {
                let n = host::call_method(&class, "count", Some(a), Vec::new())?;
                Value::int(n.to_int())
            } else {
                let t = with_host(|h| crate::stdlib::types::debug_type(h, &a));
                return Err(throws(
                    "TypeError",
                    format!(
                        "count(): Argument #1 ($value) must be of type Countable|array, {t} given"
                    ),
                ));
            }
        }
        "strtoupper" => with_host(|h| Value::str(ascii_upper(&h.to_str(&arg(args, 0))))),
        "strtolower" => with_host(|h| Value::str(ascii_lower(&h.to_str(&arg(args, 0))))),
        "ucfirst" => with_host(|h| Value::str(ucfirst(&h.to_str(&arg(args, 0))))),
        "trim" => with_host(|h| php_trim(h, args, true, true)),
        "ltrim" => with_host(|h| php_trim(h, args, true, false)),
        "rtrim" | "chop" => with_host(|h| php_trim(h, args, false, true)),
        "str_repeat" => {
            let n = arg(args, 1).to_int();
            if n < 0 {
                return Err(throws(
                    "ValueError",
                    "str_repeat(): Argument #2 ($times) must be greater than or equal to 0",
                ));
            }
            with_host(|h| Value::str(h.to_str(&arg(args, 0)).repeat(n as usize)))
        }
        "strrev" => {
            with_host(|h| Value::str(h.to_str(&arg(args, 0)).chars().rev().collect::<String>()))
        }
        "wordwrap" => with_host(|h| php_wordwrap(h, args)),
        "substr" => with_host(|h| Value::str(php_substr(&h.to_str(&arg(args, 0)), args))),
        "strpos" => with_host(|h| php_strpos(h, args)),
        "str_replace" => with_host(|h| php_str_replace(h, args)),
        // The `(array)` / `(object)` casts, which have no PHP-callable spelling.
        "__cast_array" => with_host(|h| php_cast_array(h, &arg(args, 0))),
        "__cast_object" => php_cast_object(&arg(args, 0))?,
        "abs" => with_host(|h| match h.to_number(&arg(args, 0)) {
            Value::Int(n) => Value::int(n.abs()),
            Value::Float(f) => Value::float(f.abs()),
            other => other,
        }),
        "floor" => with_host(|h| Value::float(h.to_number(&arg(args, 0)).to_float().floor())),
        "ceil" => with_host(|h| Value::float(h.to_number(&arg(args, 0)).to_float().ceil())),
        "sqrt" => with_host(|h| Value::float(h.to_number(&arg(args, 0)).to_float().sqrt())),
        "round" => with_host(|h| {
            let x = h.to_number(&arg(args, 0)).to_float();
            let p = args.get(1).map(|v| v.to_int()).unwrap_or(0);
            Value::float(php_round(x, p as i32))
        }),
        "intval" => with_host(|h| {
            let v = arg(args, 0);
            match args.get(1) {
                // A base only applies to strings, and base 10 keeps the ordinary
                // numeric-string reading so `intval("1e3", 10)` is still 1000.
                Some(b)
                    if !matches!(b, Value::Undef)
                        && matches!(v, Value::Str(_))
                        && b.to_int() != 10 =>
                {
                    Value::int(intval_base(&h.to_str(&v), b.to_int()))
                }
                _ => Value::int(h.to_number(&v).to_int()),
            }
        }),
        "floatval" | "doubleval" => {
            with_host(|h| Value::float(h.to_number(&arg(args, 0)).to_float()))
        }
        // `(string)$x` desugars to `strval($x)`, so this is the explicit cast too.
        "strval" => Value::str(host::to_str_ext(&arg(args, 0))),
        "max" => with_host(|h| fold_cmp(h, args, true)),
        "min" => with_host(|h| fold_cmp(h, args, false)),
        "gettype" => with_host(|h| Value::str(h.type_name(&arg(args, 0)).to_string())),
        // `settype($var, $type)` converts IN PLACE through its by-reference first
        // parameter (published for the call site to write back) and returns true.
        "settype" => php_settype(args)?,
        "is_array" => with_host(|h| Value::bool(h.is_array(&arg(args, 0)))),
        "is_int" | "is_integer" | "is_long" => Value::bool(matches!(arg(args, 0), Value::Int(_))),
        "is_float" | "is_double" => Value::bool(matches!(arg(args, 0), Value::Float(_))),
        "is_string" => Value::bool(matches!(arg(args, 0), Value::Str(_))),
        "is_bool" => Value::bool(matches!(arg(args, 0), Value::Bool(_))),
        "is_null" => Value::bool(matches!(arg(args, 0), Value::Undef)),
        "is_numeric" => Value::bool(match arg(args, 0) {
            Value::Int(_) | Value::Float(_) => true,
            Value::Str(s) => host::is_numeric_string(&s),
            _ => false,
        }),
        "implode" | "join" => php_implode(args),
        "explode" => with_host(|h| php_explode(h, args)),
        "in_array" => with_host(|h| php_in_array(h, args)),
        "array_keys" => with_host(|h| h.array_keys(&arg(args, 0))),
        "array_values" => with_host(|h| php_array_values(h, &arg(args, 0))),
        "array_push" => with_host(|h| php_array_push(h, args)),
        "range" => with_host(|h| php_range(h, args))?,
        "sprintf" => with_host(|h| Value::str(php_sprintf(h, args))),
        "printf" => with_host(|h| {
            let s = php_sprintf(h, args);
            h.write_out(&s);
            Value::int(s.len() as i64)
        }),
        "print_r" => with_host(|h| {
            let s = php_print_r(h, &arg(args, 0), 0);
            if args.get(1).map(|v| h.is_truthy(v)).unwrap_or(false) {
                Value::str(s)
            } else {
                h.write_out(&s);
                Value::bool(true)
            }
        }),
        "var_dump" => with_host(|h| {
            for a in args {
                let s = php_var_dump(h, a, 0);
                h.write_out(&s);
            }
            Value::Undef
        }),

        // ── strings ──────────────────────────────────────────────────────
        "str_split" => with_host(|h| php_str_split(h, args)),
        "str_pad" => with_host(|h| Value::str(php_str_pad(h, args))),
        "str_contains" => {
            with_host(|h| Value::bool(h.to_str(&arg(args, 0)).contains(&h.to_str(&arg(args, 1)))))
        }
        "str_starts_with" => with_host(|h| {
            Value::bool(
                h.to_str(&arg(args, 0))
                    .starts_with(&h.to_str(&arg(args, 1))),
            )
        }),
        "str_ends_with" => {
            with_host(|h| Value::bool(h.to_str(&arg(args, 0)).ends_with(&h.to_str(&arg(args, 1)))))
        }
        "ucwords" => with_host(|h| {
            let seps = match args.get(1) {
                Some(v) if !matches!(v, Value::Undef) => h.to_str(v),
                _ => " \t\r\n\x0c\x0b".to_string(),
            };
            Value::str(ucwords(&h.to_str(&arg(args, 0)), &seps))
        }),
        "lcfirst" => with_host(|h| Value::str(lcfirst(&h.to_str(&arg(args, 0))))),
        "number_format" => with_host(|h| Value::str(php_number_format(h, args))),
        "htmlspecialchars" | "htmlentities" => {
            with_host(|h| Value::str(html_special_chars(&h.to_str(&arg(args, 0)))))
        }
        "strcmp" => with_host(|h| {
            let (a, b) = (h.to_str(&arg(args, 0)), h.to_str(&arg(args, 1)));
            Value::int(binary_strcmp(a.as_bytes(), b.as_bytes()))
        }),
        "strcasecmp" => with_host(|h| {
            let a = ascii_lower(&h.to_str(&arg(args, 0)));
            let b = ascii_lower(&h.to_str(&arg(args, 1)));
            Value::int(binary_strcmp(a.as_bytes(), b.as_bytes()))
        }),
        "strncmp" => with_host(|h| {
            let a = h.to_str(&arg(args, 0));
            let b = h.to_str(&arg(args, 1));
            let n = arg(args, 2).to_int().max(0) as usize;
            let (ab, bb) = (a.as_bytes(), b.as_bytes());
            Value::int(binary_strcmp(
                &ab[..n.min(ab.len())],
                &bb[..n.min(bb.len())],
            ))
        }),
        "substr_compare" => with_host(|h| php_substr_compare(h, args)),
        // `str_word_count` is served by `stdlib::misc`, which implements the
        // `$format`/`$characters` arguments; it is intentionally not handled here
        // so the full version wins (this core arm was a count-only stub).
        "chr" => with_host(|h| {
            let n = (h.to_number(&arg(args, 0)).to_int().rem_euclid(256)) as u8;
            Value::str((n as char).to_string())
        }),
        "ord" => {
            with_host(|h| Value::int(h.to_str(&arg(args, 0)).bytes().next().unwrap_or(0) as i64))
        }
        "dechex" => with_host(|h| Value::str(format!("{:x}", h.to_number(&arg(args, 0)).to_int()))),
        "hexdec" => with_host(|h| {
            Value::int(i64::from_str_radix(h.to_str(&arg(args, 0)).trim(), 16).unwrap_or(0))
        }),
        "bin2hex" => with_host(|h| {
            let s = h.to_str(&arg(args, 0));
            Value::str(s.bytes().map(|b| format!("{b:02x}")).collect::<String>())
        }),

        // ── math ─────────────────────────────────────────────────────────
        "pow" => php_pow(args),
        "intdiv" => return php_intdiv(args),
        "fmod" => with_host(|h| {
            Value::float(
                h.to_number(&arg(args, 0)).to_float() % h.to_number(&arg(args, 1)).to_float(),
            )
        }),
        "sin" => with_host(|h| Value::float(h.to_number(&arg(args, 0)).to_float().sin())),
        "cos" => with_host(|h| Value::float(h.to_number(&arg(args, 0)).to_float().cos())),
        "tan" => with_host(|h| Value::float(h.to_number(&arg(args, 0)).to_float().tan())),
        "exp" => with_host(|h| Value::float(h.to_number(&arg(args, 0)).to_float().exp())),
        "log" => with_host(|h| {
            let x = h.to_number(&arg(args, 0)).to_float();
            match args.get(1) {
                Some(b) if !matches!(b, Value::Undef) => {
                    Value::float(x.log(h.to_number(b).to_float()))
                }
                _ => Value::float(x.ln()),
            }
        }),
        "log10" => with_host(|h| Value::float(h.to_number(&arg(args, 0)).to_float().log10())),
        "pi" => Value::float(std::f64::consts::PI),

        // ── arrays ───────────────────────────────────────────────────────
        "array_merge" => with_host(|h| php_array_merge(h, args)),
        "array_map" => return php_array_map(args),
        "array_filter" => return php_array_filter(args),
        "array_reduce" => return php_array_reduce(args),
        "array_slice" => with_host(|h| php_array_slice(h, args)),
        "array_reverse" => with_host(|h| php_array_reverse(h, args)),
        "array_sum" => with_host(|h| php_array_fold(h, &arg(args, 0), false)),
        "array_product" => with_host(|h| php_array_fold(h, &arg(args, 0), true)),
        "array_flip" => with_host(|h| php_array_flip(h, &arg(args, 0))),
        "array_unique" => with_host(|h| php_array_unique(h, &arg(args, 0))),
        "array_key_exists" | "key_exists" => with_host(|h| php_array_key_exists(h, args)),
        "array_search" => with_host(|h| php_array_search(h, args)),
        // The sort family takes an optional `SORT_*` flags argument in position 1.
        "sort" => with_host(|h| php_sort(h, &arg(args, 0), false, sort_flags(args))),
        "rsort" => with_host(|h| php_sort(h, &arg(args, 0), true, sort_flags(args))),
        "asort" => with_host(|h| php_asort(h, &arg(args, 0), false, sort_flags(args))),
        "arsort" => with_host(|h| php_asort(h, &arg(args, 0), true, sort_flags(args))),
        "ksort" => with_host(|h| php_ksort(h, &arg(args, 0), false, sort_flags(args))),
        "krsort" => with_host(|h| php_ksort(h, &arg(args, 0), true, sort_flags(args))),
        "array_fill" => with_host(|h| php_array_fill(h, args)),
        "array_combine" => with_host(|h| php_array_combine(h, args))?,
        "array_diff" => with_host(|h| php_array_diff(h, args, false)),
        "array_intersect" => with_host(|h| php_array_diff(h, args, true)),

        // ── type / util ──────────────────────────────────────────────────
        "boolval" => with_host(|h| Value::bool(h.is_truthy(&arg(args, 0)))),
        "var_export" => with_host(|h| {
            let s = php_var_export(h, &arg(args, 0), 0);
            if args.get(1).map(|v| h.is_truthy(v)).unwrap_or(false) {
                Value::str(s)
            } else {
                h.write_out(&s);
                Value::Undef
            }
        }),
        "json_encode" => {
            // Objects are resolved to plain data FIRST, outside the host borrow:
            // `jsonSerialize()` is PHP code and cannot run while the host is
            // borrowed. See `json_prepare`.
            let prepared = match json_prepare(&arg(args, 0)) {
                Ok(v) => v,
                Err(code) => {
                    crate::stdlib::json::set_last_error(code);
                    return Ok(Value::bool(false));
                }
            };
            with_host(|h| {
                // JSON has no NAN/INF literal, so the encoder bails out entirely
                // and reports JSON_ERROR_INF_OR_NAN rather than emitting invalid
                // JSON.
                if has_nonfinite_float(h, &prepared) {
                    crate::stdlib::json::set_last_error(crate::stdlib::json::JSON_ERROR_INF_OR_NAN);
                    Value::bool(false)
                } else {
                    crate::stdlib::json::set_last_error(0);
                    Value::str(php_json_encode(h, &prepared, arg(args, 1).to_int(), 0))
                }
            })
        }

        // Extended standard library lives in `src/stdlib/*`, one module per
        // category, consulted only for names this core match does not handle.
        _ => {
            return crate::stdlib::dispatch(&lname, args).unwrap_or_else(|| {
                Err(throws_bare(
                    "Error",
                    format!("Call to undefined function {name}()"),
                ))
            })
        }
    };
    Ok(v)
}

/// PHP's case functions are byte-wise and ASCII-only — they never touch a
/// multibyte sequence, so `strtoupper("héllo")` is `"HéLLO"`, not `"HÉLLO"`.
/// (The Unicode-aware behaviour lives in `mb_strtoupper`.)
fn ascii_upper(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii() {
                c.to_ascii_uppercase()
            } else {
                c
            }
        })
        .collect()
}

/// ASCII-only lowercase; see [`ascii_upper`].
fn ascii_lower(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii() {
                c.to_ascii_lowercase()
            } else {
                c
            }
        })
        .collect()
}

fn ucfirst(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        // ASCII-only, so `ucfirst("élan")` leaves the "é" alone.
        Some(first) if first.is_ascii() => std::iter::once(first.to_ascii_uppercase())
            .chain(c)
            .collect(),
        _ => s.to_string(),
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

/// `str_replace($search, $replace, $subject, &$count)`.
///
/// `$search` and `$replace` may each be an array: with two arrays the pairs line
/// up by position (a missing replacement is `""`), and with an array search plus
/// a string replacement every needle maps to that one string. An array
/// `$subject` is processed element-wise and returns an array.
///
/// The searches are applied in sequence to the *running* result, which is why
/// `str_replace(["a", "aa"], ["1", "2"], "aaa")` is `"111"` — by the time `"aa"`
/// is tried, no `a` is left.
/// `(array) $v` — an array stays itself, `null` becomes the empty array, an
/// object yields its properties keyed by name, and any other scalar becomes a
/// one-element list.
/// `intval($string, $base)` for an explicit base.
///
/// Base 0 auto-detects from the prefix (`0x` hex, `0b` binary, a leading `0`
/// octal, else decimal). Bases 16, 2 and 8 also accept — but do not require —
/// their prefix. Parsing takes as many valid digits as it can and stops at the
/// first one that is out of range, so `intval("42abc", 10)` is 42.
fn intval_base(s: &str, base: i64) -> i64 {
    let t = s.trim_start();
    let (neg, t) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let lower = t.to_ascii_lowercase();
    // Strip a prefix when it agrees with the requested base (or picks it, at 0).
    let (base, digits) = match base {
        0 if lower.starts_with("0x") => (16, &t[2..]),
        0 if lower.starts_with("0b") => (2, &t[2..]),
        0 if t.len() > 1 && t.starts_with('0') => (8, &t[1..]),
        0 => (10, t),
        16 if lower.starts_with("0x") => (16, &t[2..]),
        2 if lower.starts_with("0b") => (2, &t[2..]),
        8 if lower.starts_with("0o") => (8, &t[2..]),
        b => (b, t),
    };
    if !(2..=36).contains(&base) {
        return 0;
    }
    let mut acc: i64 = 0;
    for c in digits.chars() {
        let Some(d) = c.to_digit(base as u32) else {
            break;
        };
        acc = acc.saturating_mul(base).saturating_add(d as i64);
    }
    if neg {
        -acc
    } else {
        acc
    }
}

fn php_cast_array(h: &mut host::PhpHost, v: &Value) -> Value {
    if h.is_array(v) {
        return v.clone();
    }
    let out = h.new_array();
    if matches!(v, Value::Undef) {
        return out;
    }
    if h.is_object(v) {
        // Non-public property names are mangled with NUL separators, as PHP's
        // cast does — see `PhpHost::object_props_mangled`.
        for (name, val) in h.object_props_mangled(v) {
            h.arr_set_key(&out, &Value::str(name), val);
        }
        return out;
    }
    h.arr_push_auto(&out, v.clone());
    out
}

/// `settype($var, $type)` — convert `$var` in place to `$type` and return true.
///
/// The conversion is the same one the matching cast performs, so `settype($x,
/// "integer")` and `$x = (int) $x` agree, including on the cases where they
/// differ from arithmetic: a non-numeric string becomes `0`, a scalar becomes a
/// one-element array, and `"null"` clears the value. An unrecognised type name is
/// a `ValueError` in PHP 8.
fn php_settype(args: &[Value]) -> Result<Value, String> {
    let v = arg(args, 0);
    let ty = with_host(|h| h.to_str(&arg(args, 1))).to_ascii_lowercase();
    let out = match ty.as_str() {
        "bool" | "boolean" => Value::bool(with_host(|h| h.is_truthy(&v))),
        "int" | "integer" => Value::int(with_host(|h| h.to_number(&v)).to_int()),
        "float" | "double" => Value::float(with_host(|h| h.to_number(&v)).to_float()),
        "string" => Value::str(host::to_str_ext(&v)),
        "array" => with_host(|h| php_cast_array(h, &v)),
        "object" => php_cast_object(&v)?,
        "null" => Value::Undef,
        other => {
            return Err(format!(
                "settype(): Argument #2 ($type) must be a valid type, \"{other}\" given"
            ))
        }
    };
    with_host(|h| h.byref_out_put(0, out));
    Ok(Value::bool(true))
}

/// `(object) $v` — an array becomes a `stdClass` with the same keys as
/// properties, `null` an empty `stdClass`, and any other scalar a `stdClass`
/// whose single `scalar` property holds the value. An object passes through.
fn php_cast_object(v: &Value) -> Result<Value, String> {
    // `new_object` re-enters the host, so it must run outside `with_host`.
    if with_host(|h| h.is_object(v)) {
        return Ok(v.clone());
    }
    let obj = host::new_object("stdClass", Vec::new())?;
    with_host(|h| {
        if h.is_array(v) {
            for (k, val) in h.array_pairs(v).unwrap_or_default() {
                let name = h.to_str(&k);
                h.prop_set(&obj, &name, val);
            }
        } else if !matches!(v, Value::Undef) {
            h.prop_set(&obj, "scalar", v.clone());
        }
    });
    Ok(obj)
}

fn php_str_replace(h: &mut host::PhpHost, args: &[Value]) -> Value {
    let search = arg(args, 0);
    let replace = arg(args, 1);
    let subject = arg(args, 2);

    // Normalise search/replace into parallel lists.
    let pairs: Vec<(String, String)> = if h.is_array(&search) {
        let searches = h.array_pairs(&search).unwrap_or_default();
        let replaces = if h.is_array(&replace) {
            h.array_pairs(&replace)
                .unwrap_or_default()
                .into_iter()
                .map(|(_, v)| h.to_str(&v))
                .collect()
        } else {
            vec![h.to_str(&replace); searches.len()]
        };
        searches
            .into_iter()
            .enumerate()
            .map(|(i, (_, s))| (h.to_str(&s), replaces.get(i).cloned().unwrap_or_default()))
            .collect()
    } else {
        vec![(h.to_str(&search), h.to_str(&replace))]
    };

    let mut count = 0usize;
    // `$count` is a by-reference OUT parameter: the number of replacements.
    let result = if h.is_array(&subject) {
        let out = h.new_array();
        for (k, v) in h.array_pairs(&subject).unwrap_or_default() {
            let s = replace_all_pairs(&h.to_str(&v), &pairs, &mut count);
            h.arr_set_key(&out, &k, Value::str(s));
        }
        out
    } else {
        Value::str(replace_all_pairs(&h.to_str(&subject), &pairs, &mut count))
    };
    h.byref_out_put(3, Value::int(count as i64));
    result
}

/// Apply each `(search, replace)` pair in order to `subject`, accumulating the
/// number of replacements. An empty needle matches nothing, as in PHP.
fn replace_all_pairs(subject: &str, pairs: &[(String, String)], count: &mut usize) -> String {
    let mut cur = subject.to_string();
    for (needle, rep) in pairs {
        if needle.is_empty() {
            continue;
        }
        *count += cur.matches(needle.as_str()).count();
        cur = cur.replace(needle.as_str(), rep);
    }
    cur
}

fn php_implode(args: &[Value]) -> Value {
    // implode($glue, $array) or implode($array).
    let (glue, arr) = with_host(|h| {
        if h.is_array(&arg(args, 0)) {
            (String::new(), arg(args, 0))
        } else {
            (h.to_str(&arg(args, 0)), arg(args, 1))
        }
    });
    let vals: Vec<Value> = with_host(|h| h.array_pairs(&arr).unwrap_or_default())
        .into_iter()
        .map(|(_, v)| v)
        .collect();
    // Each element is joined *as a string*, so one with `__toString` runs it —
    // which re-enters the host, hence the conversion outside the borrow.
    let parts: Vec<String> = vals.iter().map(host::to_str_ext).collect();
    Value::str(parts.join(&glue))
}

/// The set of characters a `trim` charlist denotes. PHP supports `a..z` ranges
/// inside the list, so `"a..z"` means every lowercase letter rather than the
/// three characters `a`, `.`, `z`.
fn trim_char_set(list: &str) -> Vec<char> {
    let cs: Vec<char> = list.chars().collect();
    let mut out = Vec::with_capacity(cs.len());
    let mut i = 0;
    while i < cs.len() {
        // `x..y` — expand to the inclusive range when it is well formed.
        if i + 3 < cs.len() && cs[i + 1] == '.' && cs[i + 2] == '.' && cs[i + 3] >= cs[i] {
            for c in cs[i]..=cs[i + 3] {
                out.push(c);
            }
            i += 4;
        } else {
            out.push(cs[i]);
            i += 1;
        }
    }
    out
}

/// `trim`/`ltrim`/`rtrim` with PHP's optional `$characters` list. The default set
/// is `" \t\n\r\0\x0B"` — note it does NOT include the form feed `\x0C`, which
/// Rust's `str::trim` would strip.
fn php_trim(h: &host::PhpHost, args: &[Value], start: bool, end: bool) -> Value {
    let s = h.to_str(&arg(args, 0));
    let set = match args.get(1) {
        Some(v) if !matches!(v, Value::Undef) => trim_char_set(&h.to_str(v)),
        _ => vec![' ', '\t', '\n', '\r', '\0', '\x0b'],
    };
    let mut out = s.as_str();
    if start {
        out = out.trim_start_matches(|c| set.contains(&c));
    }
    if end {
        out = out.trim_end_matches(|c| set.contains(&c));
    }
    Value::str(out.to_string())
}

/// `explode($separator, $string, $limit = PHP_INT_MAX)`.
///
/// A positive limit caps the number of parts, the last one keeping the whole
/// remainder; a negative limit drops that many parts off the end; `0` behaves
/// as `1`.
fn php_explode(h: &mut host::PhpHost, args: &[Value]) -> Value {
    let sep = h.to_str(&arg(args, 0));
    let subject = h.to_str(&arg(args, 1));
    let arr = h.new_array();
    if sep.is_empty() {
        h.arr_push_auto(&arr, Value::str(subject));
        return arr;
    }
    let limit = match args.get(2) {
        Some(v) if !matches!(v, Value::Undef) => v.to_int(),
        _ => i64::MAX,
    };
    let mut parts: Vec<String> = if limit > 0 && limit != i64::MAX {
        subject
            .splitn(limit as usize, sep.as_str())
            .map(str::to_string)
            .collect()
    } else {
        subject.split(sep.as_str()).map(str::to_string).collect()
    };
    if limit == 0 {
        // PHP treats a zero limit as 1: everything in a single element.
        parts = vec![subject];
    } else if limit < 0 {
        let drop = (-limit) as usize;
        parts.truncate(parts.len().saturating_sub(drop));
    }
    for part in parts {
        h.arr_push_auto(&arr, Value::str(part));
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

/// `range($start, $end, $step = 1)`: an inclusive sequence. Character ranges
/// (both bounds non-numeric single-byte-anchored strings) walk byte codepoints;
/// numeric ranges are integer unless any bound/step is a float. `$step` is taken
/// as its absolute value (direction comes from `$start` vs `$end`). Like PHP,
/// a `$step` that exceeds the span between distinct bounds is a fatal ValueError.
fn php_range(h: &mut host::PhpHost, args: &[Value]) -> Result<Value, String> {
    const STEP_ERR: &str = "range(): Argument #3 ($step) must be less than the range \
                            spanned by argument #1 ($start) and argument #2 ($end)";
    let a0 = arg(args, 0);
    let a1 = arg(args, 1);
    let arr = h.new_array();

    // Character range: both bounds are non-empty, non-numeric strings.
    if let (Value::Str(s0), Value::Str(s1)) = (&a0, &a1) {
        if !s0.is_empty()
            && !s1.is_empty()
            && !host::is_numeric_string(s0)
            && !host::is_numeric_string(s1)
        {
            // A character range walks BYTES, so only the first byte of each bound
            // is used; PHP warns (per bound, `$start` before `$end`) that the rest
            // is discarded rather than silently truncating.
            for (pos, name, s) in [(1, "start", s0), (2, "end", s1)] {
                if s.len() > 1 {
                    h.warn(format_args!(
                        "range(): Argument #{pos} (${name}) must be a single byte, \
                         subsequent bytes are ignored"
                    ));
                }
            }
            let low = s0.as_bytes()[0] as i64;
            let high = s1.as_bytes()[0] as i64;
            let step = args
                .get(2)
                .map(|v| h.to_number(v).to_int().abs())
                .unwrap_or(1)
                .max(1);
            if low != high && (low - high).abs() < step {
                return Err(throws("ValueError", STEP_ERR));
            }
            let mut c = low;
            if low <= high {
                while c <= high {
                    h.arr_push_auto(&arr, Value::str(((c as u8) as char).to_string()));
                    c += step;
                }
            } else {
                while c >= high {
                    h.arr_push_auto(&arr, Value::str(((c as u8) as char).to_string()));
                    c -= step;
                }
            }
            return Ok(arr);
        }
    }

    // Numeric range. Float if any bound or the step is a float.
    let n0 = h.to_number(&a0);
    let n1 = h.to_number(&a1);
    let nstep = args.get(2).map(|v| h.to_number(v));
    let is_float = matches!(n0, Value::Float(_))
        || matches!(n1, Value::Float(_))
        || matches!(nstep, Some(Value::Float(_)));

    if is_float {
        let low = n0.to_float();
        let high = n1.to_float();
        let mut step = nstep.as_ref().map(|v| v.to_float().abs()).unwrap_or(1.0);
        if step == 0.0 {
            step = 1.0;
        }
        if (low - high).abs() >= f64::EPSILON && (low - high).abs() < step {
            return Err(throws("ValueError", STEP_ERR));
        }
        if (low - high).abs() < f64::EPSILON {
            h.arr_push_auto(&arr, Value::float(low));
        } else if low < high {
            let mut i = 0.0f64;
            loop {
                let v = low + i * step;
                if v > high + f64::EPSILON * high {
                    break;
                }
                h.arr_push_auto(&arr, Value::float(v));
                i += 1.0;
            }
        } else {
            let mut i = 0.0f64;
            loop {
                let v = low - i * step;
                if v < high - f64::EPSILON * high {
                    break;
                }
                h.arr_push_auto(&arr, Value::float(v));
                i += 1.0;
            }
        }
    } else {
        let low = n0.to_int();
        let high = n1.to_int();
        let step = nstep.as_ref().map(|v| v.to_int().abs()).unwrap_or(1).max(1);
        if low != high && (low - high).abs() < step {
            return Err(throws("ValueError", STEP_ERR));
        }
        let mut i = low;
        if low <= high {
            while i <= high {
                h.arr_push_auto(&arr, Value::int(i));
                i += step;
            }
        } else {
            while i >= high {
                h.arr_push_auto(&arr, Value::int(i));
                i -= step;
            }
        }
    }
    Ok(arr)
}

/// A parsed conversion spec `%[argnum$][flags][width][.precision]conv`.
struct FmtSpec {
    argnum: Option<usize>,
    left: bool,
    plus: bool,
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
    let (mut left, mut plus, mut pad) = (false, false, ' ');
    loop {
        match fmt.get(j) {
            Some('-') => left = true,
            Some('+') => plus = true,
            // PHP treats a space as a padding-character flag (pad with spaces),
            // not as C-style sign-space; the last padding flag in the run wins, so
            // `% 05d` pads with zeros while `%0 5d` pads with spaces.
            Some(' ') => pad = ' ',
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
    } else if h.is_object(v) {
        php_print_r_object(h, v, depth)
    } else {
        h.to_str(v)
    }
}

/// `print_r` of an object: the same parenthesised block an array gets, headed by
/// the class name rather than `Array`, and with each non-public property name
/// annotated — `[b:protected]`, `[c:Declaring:private]`.
///
/// An `enum` case heads its block `Suit Enum:string` (a backed enum, naming the
/// backing type) or `Suit Enum` (a pure one), which is why it cannot simply reuse
/// the object path.
fn php_print_r_object(h: &host::PhpHost, v: &Value, depth: usize) -> String {
    let pad = "    ".repeat(depth);
    let inner = "    ".repeat(depth + 1);
    let class = h.object_class(v).unwrap_or_else(|| "stdClass".to_string());
    // An anonymous class prints only the head of its name — see `display_class`.
    let shown = host::display_class(&class);
    let head = match h.enum_case_of(v) {
        Some((_, Some(backing))) => format!("{shown} Enum:{}", enum_backing_type(&backing)),
        Some((_, None)) => format!("{shown} Enum"),
        None => format!("{shown} Object"),
    };
    let mut s = format!("{head}\n{pad}(\n");
    for (name, val) in h.object_props(v) {
        let label = match h.prop_visibility(&class, &name) {
            Some((_, crate::ast::Visibility::Protected)) => format!("{name}:protected"),
            Some((declaring, crate::ast::Visibility::Private)) => {
                format!("{name}:{declaring}:private")
            }
            _ => name,
        };
        s.push_str(&format!(
            "{inner}[{label}] => {}\n",
            php_print_r(h, &val, depth + 2)
        ));
    }
    s.push_str(&format!("{pad})\n"));
    s
}

/// The name `print_r` gives a backed enum's backing type in its header line.
/// Only `int` and `string` can back an enum, so anything else cannot arise.
fn enum_backing_type(backing: &Value) -> &'static str {
    match backing {
        Value::Int(_) => "int",
        _ => "string",
    }
}

/// `var_dump` rendering for scalars and one level of arrays.
fn php_var_dump(h: &host::PhpHost, v: &Value, depth: usize) -> String {
    php_var_dump_ref(h, v, depth, false)
}

/// `var_dump` of one value. `is_ref` marks a slot a `&` binding has turned into a
/// reference: PHP prefixes such a value's type with `&` (`&int(2)`), between the
/// indentation and the type, at every nesting level.
fn php_var_dump_ref(h: &host::PhpHost, v: &Value, depth: usize, is_ref: bool) -> String {
    let pad = "  ".repeat(depth);
    let amp = if is_ref { "&" } else { "" };
    match v {
        Value::Undef => format!("{pad}NULL\n"),
        Value::Bool(b) => format!("{pad}{amp}bool({})\n", if *b { "true" } else { "false" }),
        Value::Int(n) => format!("{pad}{amp}int({n})\n"),
        // var_dump reports floats at serialize_precision, not echo's precision=14.
        Value::Float(f) => format!(
            "{pad}{amp}float({})\n",
            crate::stdlib::types::serialize_float(*f)
        ),
        Value::Str(s) => format!("{pad}{amp}string({}) \"{s}\"\n", s.len()),
        // A class instance prints as `object(Class)#n (count) { ... }` with its
        // properties, not as the bare array its handle would otherwise yield.
        // An `enum` case is one line naming the case, not the two-property
        // object it is implemented as.
        Value::Obj(_) if h.enum_case_of(v).is_some() => {
            let class = h.object_class(v).unwrap_or_default();
            let (case, _) = h.enum_case_of(v).unwrap_or_default();
            format!("{pad}{amp}enum({class}::{case})\n")
        }
        Value::Obj(_) if h.is_object(v) => {
            let props = h.object_props_marked(v);
            let class = h.object_class(v).unwrap_or_else(|| "stdClass".to_string());
            let mut s = format!(
                "{pad}{amp}object({})#{} ({}) {{\n",
                host::display_class(&class),
                h.object_ordinal(v),
                props.len()
            );
            for (name, val, pref) in props {
                // A non-public property carries its visibility in the key, and a
                // private one also names the class that declared it.
                let label = match h.prop_visibility(&class, &name) {
                    Some((_, crate::ast::Visibility::Protected)) => {
                        format!("\"{name}\":protected")
                    }
                    Some((declaring, crate::ast::Visibility::Private)) => {
                        format!("\"{name}\":\"{declaring}\":private")
                    }
                    _ => format!("\"{name}\""),
                };
                s.push_str(&format!("{}  [{label}]=>\n", "  ".repeat(depth)));
                s.push_str(&php_var_dump_ref(h, &val, depth + 1, pref));
            }
            s.push_str(&format!("{pad}}}\n"));
            s
        }
        Value::Obj(_) => {
            let pairs = h.array_pairs_marked(v).unwrap_or_default();
            let mut s = format!("{pad}{amp}array({}) {{\n", pairs.len());
            for (k, val, eref) in pairs {
                let key = match k {
                    Value::Int(n) => format!("{}  [{n}]=>\n", "  ".repeat(depth)),
                    other => format!("{}  [\"{}\"]=>\n", "  ".repeat(depth), h.to_str(&other)),
                };
                s.push_str(&key);
                s.push_str(&php_var_dump_ref(h, &val, depth + 1, eref));
            }
            s.push_str(&format!("{pad}}}\n"));
            s
        }
        _ => format!("{pad}NULL\n"),
    }
}

// ── string helpers (added stdlib wave) ───────────────────────────────────────

/// PHP's `zend_binary_strcmp`, which is what `strcmp` and its relatives return.
///
/// It is *not* a sign: over the shared prefix the result is the difference of the
/// first differing byte (`strcmp("a", "z")` is -25, not -1), because PHP returns
/// `memcmp`'s value unchanged. Only when one string is a prefix of the other does
/// it fall back to a three-way compare of the lengths, which is -1/0/1.
pub(crate) fn binary_strcmp(a: &[u8], b: &[u8]) -> i64 {
    for (x, y) in a.iter().zip(b) {
        if x != y {
            return *x as i64 - *y as i64;
        }
    }
    match a.len().cmp(&b.len()) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

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

/// `ucwords($string, $separators = " \t\r\n\f\v")` — uppercase the first byte and
/// every byte that directly follows a separator.
///
/// The `$separators` argument *replaces* the default set rather than extending
/// it, so `ucwords("hello world-foo bar", "-")` capitalises only after the dash
/// and yields `"Hello world-Foo bar"`. Byte-wise and ASCII-only, like the rest of
/// PHP's case family.
fn ucwords(s: &str, separators: &str) -> String {
    let delims: Vec<u8> = separators.bytes().collect();
    let mut bytes = s.as_bytes().to_vec();
    let mut cap = true;
    for b in &mut bytes {
        if cap {
            b.make_ascii_uppercase();
        }
        cap = delims.contains(b);
    }
    // Only ASCII bytes were altered, so the original UTF-8 structure survives.
    String::from_utf8(bytes).unwrap_or_else(|_| s.to_string())
}

fn lcfirst(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        // ASCII-only, mirroring `ucfirst`.
        Some(first) if first.is_ascii() => std::iter::once(first.to_ascii_lowercase())
            .chain(c)
            .collect(),
        _ => s.to_string(),
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
    // PHP rounds the value with _php_math_round (half away from zero, with
    // pre-rounding) before formatting, so 1.005 becomes "1.01".
    let rounded = php_round(num, dec as i32).abs();
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

/// Port of PHP's `_php_math_round` (default mode: round half away from zero).
/// The pre-rounding step compensates for binary floating-point representation
/// error so decimal half-way values print the way PHP prints them — e.g.
/// `round(1.005, 2)` is `1.01`, not the `1.0` a naive `(x*100).round()/100`
/// yields because the nearest f64 to 1.005 is slightly below it.
pub(crate) fn php_round(value: f64, places: i32) -> f64 {
    if !value.is_finite() || value == 0.0 {
        return value;
    }
    // php_intlog10abs: floor(log10(|value|)).
    let precision_places = 14 - value.abs().log10().floor() as i32;
    let f1 = 10f64.powi(places.abs());
    let tmp_value = if precision_places > places && precision_places - 15 < places {
        let f2 = 10f64.powi(precision_places.abs());
        let mut t = if precision_places >= 0 {
            value * f2
        } else {
            value / f2
        };
        t = t.round();
        let up = places - precision_places;
        let f3 = 10f64.powi(up.abs());
        t = if up >= 0 { t * f3 } else { t / f3 };
        t.round()
    } else {
        let t = if places >= 0 { value * f1 } else { value / f1 };
        if t.abs() >= 1e15 {
            return value;
        }
        t.round()
    };
    if places > 0 {
        tmp_value / f1
    } else {
        tmp_value * f1
    }
}

/// The host borrow is taken for the coercion and RELEASED before the
/// deprecation check, which takes its own — holding one across the other panics
/// the `RefCell`.
fn php_pow(args: &[Value]) -> Value {
    let (an, bn) = with_host(|h| (h.to_number(&arg(args, 0)), h.to_number(&arg(args, 1))));
    deprecate_zero_base_negative_exponent(&an, &bn);
    match (an, bn) {
        (Value::Int(x), Value::Int(y)) if y >= 0 => match checked_ipow(x, y as u32) {
            Some(v) => Value::int(v),
            None => Value::float((x as f64).powf(y as f64)),
        },
        (an, bn) => Value::float(an.to_float().powf(bn.to_float())),
    }
}

fn php_intdiv(args: &[Value]) -> Result<Value, String> {
    let (a, b) = (arg(args, 0), arg(args, 1));
    let (an, bn) = with_host(|h| (h.to_number(&a), h.to_number(&b)));
    let (x, y) = (int_operand(&a, &an), int_operand(&b, &bn));
    if y == 0 {
        return pending_php_throw("DivisionByZeroError", "Division by zero");
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
            if host::unwinding() {
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
            if host::unwinding() {
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
            if host::unwinding() {
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
        if host::unwinding() {
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
/// Compare two values under a `SORT_*` flag from the sort family's second
/// argument.
///
/// `SORT_FLAG_CASE` (8) is a bit that combines with `SORT_STRING` or
/// `SORT_NATURAL` to fold case; the remaining bits select the comparison.
/// Anything unrecognised falls back to `SORT_REGULAR`, PHP's standard
/// comparison.
/// The `SORT_*` flags argument of the sort family (position 1), defaulting to
/// `SORT_REGULAR`.
fn sort_flags(args: &[Value]) -> i64 {
    match args.get(1) {
        Some(v) if !matches!(v, Value::Undef) => v.to_int(),
        _ => 0,
    }
}

fn sort_flag_compare(h: &host::PhpHost, a: &Value, b: &Value, flags: i64) -> i32 {
    const SORT_FLAG_CASE: i64 = 8;
    let fold_case = flags & SORT_FLAG_CASE != 0;
    match flags & !SORT_FLAG_CASE {
        // SORT_NUMERIC — compare both operands as floats.
        1 => cmp_f64(h.to_number(a).to_float(), h.to_number(b).to_float()),
        // SORT_STRING / SORT_LOCALE_STRING — byte comparison of the string forms.
        // There is no locale support, so both behave as the C locale.
        2 | 5 => {
            let (x, y) = (h.to_str(a), h.to_str(b));
            if fold_case {
                strcmp_i32(&ascii_lower(&x), &ascii_lower(&y))
            } else {
                strcmp_i32(&x, &y)
            }
        }
        // SORT_NATURAL — `natsort`'s digit-run-aware ordering.
        6 => sign(crate::stdlib::arrays::nat_cmp(
            &h.to_str(a),
            &h.to_str(b),
            fold_case,
        )) as i32,
        // SORT_REGULAR and anything unknown.
        _ => php_compare(h, a, b),
    }
}

fn php_sort(h: &mut host::PhpHost, arr: &Value, reverse: bool, flags: i64) -> Value {
    let mut vals: Vec<Value> = h
        .array_pairs(arr)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, v)| v)
        .collect();
    // Reverse by inverting the comparator, not by reversing after the fact:
    // PHP sorts are stable, so equal elements must keep their original order in
    // `rsort` too (`rsort([1, "1"])` leaves `1` before `"1"`).
    vals.sort_by(|a, b| {
        if reverse {
            sort_flag_compare(h, b, a, flags)
        } else {
            sort_flag_compare(h, a, b, flags)
        }
        .cmp(&0)
    });
    h.arr_set_reindexed(arr, vals);
    Value::bool(true)
}

/// `asort`/`arsort` — sorts by value in place, preserving keys; returns `true`.
fn php_asort(h: &mut host::PhpHost, arr: &Value, reverse: bool, flags: i64) -> Value {
    let mut pairs = h.array_pairs(arr).unwrap_or_default();
    // Stable reverse: invert the comparator rather than reversing the result.
    pairs.sort_by(|(_, a), (_, b)| {
        if reverse {
            sort_flag_compare(h, b, a, flags)
        } else {
            sort_flag_compare(h, a, b, flags)
        }
        .cmp(&0)
    });
    h.arr_set_pairs(arr, pairs);
    Value::bool(true)
}

/// `ksort`/`krsort` — sorts by key in place, preserving keys; returns `true`.
fn php_ksort(h: &mut host::PhpHost, arr: &Value, reverse: bool, flags: i64) -> Value {
    let mut pairs = h.array_pairs(arr).unwrap_or_default();
    // Stable reverse: invert the comparator rather than reversing the result.
    pairs.sort_by(|(a, _), (b, _)| {
        if reverse {
            sort_flag_compare(h, b, a, flags)
        } else {
            sort_flag_compare(h, a, b, flags)
        }
        .cmp(&0)
    });
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

fn php_array_combine(h: &mut host::PhpHost, args: &[Value]) -> Result<Value, String> {
    let keys = h.array_pairs(&arg(args, 0)).unwrap_or_default();
    let vals = h.array_pairs(&arg(args, 1)).unwrap_or_default();
    if keys.len() != vals.len() {
        return Err(throws(
            "ValueError",
            "array_combine(): Argument #1 ($keys) and argument #2 ($values) \
             must have the same number of elements",
        ));
    }
    let out = h.new_array();
    for ((_, k), (_, v)) in keys.into_iter().zip(vals) {
        h.arr_set_key(&out, &k, v);
    }
    Ok(out)
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
        // var_export prints floats at serialize_precision and guarantees the
        // result reads back as a float, so a whole number gains a ".0" tail
        // (`var_export(1.0, true)` is `"1.0"`, not `"1"`). NAN/INF are exempt.
        Value::Float(f) => {
            let s = crate::stdlib::types::serialize_float(*f);
            if f.is_finite() && !s.contains('.') {
                format!("{s}.0")
            } else {
                s
            }
        }
        Value::Str(s) => format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'")),
        Value::Obj(_) if h.is_object(v) => php_var_export_object(h, v, depth),
        Value::Obj(_) if h.is_array(v) => {
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
                    var_export_item(h, &val, depth + 1)
                ));
            }
            out.push_str(&format!("{pad})"));
            out
        }
        _ => "NULL".to_string(),
    }
}

/// A value in an item position (`key => value`). PHP keeps a scalar on the key's
/// line but breaks BEFORE a nested array or object and starts its block at the
/// item's own indent, which is why the two cannot share one renderer.
fn var_export_item(h: &host::PhpHost, v: &Value, depth: usize) -> String {
    let body = php_var_export(h, v, depth);
    if h.is_array(v) || h.is_object(v) {
        format!("\n{}{body}", "  ".repeat(depth))
    } else {
        body
    }
}

/// `var_export` of an object. The reference emits code that would rebuild it:
/// `\Class::__set_state(array( ... ))` for a declared class, the `(object)` cast
/// of an array for a `stdClass`, and the case constant itself for an `enum`.
///
/// Object bodies indent one space deeper than array bodies — three per level
/// rather than two — which is the engine's own inconsistency, not a typo.
fn php_var_export_object(h: &host::PhpHost, v: &Value, depth: usize) -> String {
    let class = h.object_class(v).unwrap_or_else(|| "stdClass".to_string());
    if let Some((case, _)) = h.enum_case_of(v) {
        return format!("\\{class}::{case}");
    }
    let pad = "  ".repeat(depth);
    let inner = format!("{pad}   ");
    let std = class == "stdClass";
    let mut out = if std {
        "(object) array(\n".to_string()
    } else {
        format!("\\{class}::__set_state(array(\n")
    };
    for (name, val) in h.object_props(v) {
        out.push_str(&format!(
            "{inner}'{}' => {},\n",
            name.replace('\'', "\\'"),
            var_export_item(h, &val, depth + 1)
        ));
    }
    out.push_str(&format!("{pad}{}", if std { ")" } else { "))" }));
    out
}

/// `count($a, COUNT_RECURSIVE)` — every element at every depth. A nested array
/// counts as one element AND contributes its own contents, so `[1, [2, 3]]` is
/// 4 rather than 2.
///
/// Recursion is into ARRAYS only. A `Countable` nested inside counts as a single
/// element and its own `count()` is never consulted — `count([new C], …)` is 1
/// however large `C` says it is — and a plain object is likewise one element.
fn count_recursive(h: &host::PhpHost, v: &Value) -> i64 {
    h.array_pairs(v)
        .unwrap_or_default()
        .iter()
        .map(|(_, val)| {
            if h.is_array(val) {
                1 + count_recursive(h, val)
            } else {
                1
            }
        })
        .sum()
}

/// Whether `v` contains a NAN or INF anywhere, including nested in arrays and
/// object properties.
fn has_nonfinite_float(h: &host::PhpHost, v: &Value) -> bool {
    match v {
        Value::Float(f) => !f.is_finite(),
        Value::Obj(_) if h.is_object(v) => h
            .object_props(v)
            .iter()
            .any(|(_, val)| has_nonfinite_float(h, val)),
        Value::Obj(_) => h
            .array_pairs(v)
            .unwrap_or_default()
            .iter()
            .any(|(_, val)| has_nonfinite_float(h, val)),
        _ => false,
    }
}

/// Resolve every object in `v` down to the plain data `json_encode` can render,
/// recursively. Runs BEFORE the encoder and outside the host borrow, because two
/// of its cases execute PHP.
///
/// The reference's rules, in the order it applies them:
/// * A `JsonSerializable` encodes whatever `jsonSerialize()` returns, and that
///   result is resolved in turn (it may itself contain objects).
/// * A backed `enum` case encodes as its backing value; a pure one has no
///   representation at all and fails the whole encode with
///   `JSON_ERROR_NON_BACKED_ENUM`.
/// * Any other object encodes its PUBLIC properties only, which is why it is
///   rebuilt here as a fresh `stdClass` holding just those — private and
///   protected state never reaches the encoder.
///
/// `Err(code)` is a `JSON_ERROR_*` code: the encode yields `false`.
fn json_prepare(v: &Value) -> Result<Value, i64> {
    if with_host(|h| h.is_array(v)) {
        let pairs = with_host(|h| h.array_pairs(v)).unwrap_or_default();
        let mut out = Vec::with_capacity(pairs.len());
        for (k, val) in pairs {
            out.push((k, json_prepare(&val)?));
        }
        return Ok(with_host(|h| {
            let arr = h.new_array();
            h.arr_set_pairs(&arr, out);
            arr
        }));
    }
    if !with_host(|h| h.is_object(v)) {
        return Ok(v.clone());
    }
    let class = with_host(|h| h.object_class(v)).unwrap_or_default();
    if with_host(|h| h.is_enum_class(&class)) {
        return match with_host(|h| h.enum_case_of(v)) {
            Some((_, Some(backing))) => Ok(backing),
            _ => Err(crate::stdlib::json::JSON_ERROR_NON_BACKED_ENUM),
        };
    }
    if with_host(|h| h.class_is_a_pub(&class, "JsonSerializable")) {
        let produced = host::call_method(&class, "jsonSerialize", Some(v.clone()), Vec::new())
            .map_err(|_| crate::stdlib::json::JSON_ERROR_NON_BACKED_ENUM)?;
        return json_prepare(&produced);
    }
    let mut out = Vec::new();
    for (name, val) in with_host(|h| h.object_props(v)) {
        if with_host(|h| h.prop_visibility(&class, &name)).is_some() {
            continue;
        }
        out.push((name, json_prepare(&val)?));
    }
    Ok(with_host(|h| h.new_transient_object(out)))
}

/// `json_encode` flags this encoder honours.
const JSON_UNESCAPED_SLASHES: i64 = 64;
const JSON_PRETTY_PRINT: i64 = 128;
const JSON_UNESCAPED_UNICODE: i64 = 256;

/// Encode `v` as JSON. `depth` is the current nesting level, used only for
/// `JSON_PRETTY_PRINT` indentation (four spaces per level, as PHP emits).
fn php_json_encode(h: &host::PhpHost, v: &Value, flags: i64, depth: usize) -> String {
    let pretty = flags & JSON_PRETTY_PRINT != 0;
    match v {
        Value::Undef => "null".to_string(),
        Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        Value::Int(n) => n.to_string(),
        // JSON uses serialize_precision too, but spells the exponent lowercase
        // (`1.0e+100`, where var_dump/serialize print `1.0E+100`).
        Value::Float(f) => crate::stdlib::types::serialize_float(*f).replace('E', "e"),
        Value::Str(s) => json_string(s, flags),
        Value::Obj(_) => {
            let pairs = if h.is_object(v) {
                h.object_props(v)
                    .into_iter()
                    .map(|(k, val)| (Value::str(k), val))
                    .collect()
            } else {
                h.array_pairs(v).unwrap_or_default()
            };
            // A real object always encodes as a JSON object, even with no
            // properties — `json_encode(new stdClass())` is `{}`, not `[]`.
            let as_list = is_list(&pairs) && !h.is_object(v);
            let (open, close) = if as_list { ('[', ']') } else { ('{', '}') };
            if pairs.is_empty() {
                return format!("{open}{close}");
            }
            let items: Vec<String> = pairs
                .iter()
                .map(|(k, val)| {
                    let encoded = php_json_encode(h, val, flags, depth + 1);
                    if as_list {
                        encoded
                    } else {
                        let key = json_string(&h.to_str(k), flags);
                        let sep = if pretty { ": " } else { ":" };
                        format!("{key}{sep}{encoded}")
                    }
                })
                .collect();
            if pretty {
                let inner = "    ".repeat(depth + 1);
                let outer = "    ".repeat(depth);
                format!(
                    "{open}\n{inner}{}\n{outer}{close}",
                    items.join(&format!(",\n{inner}"))
                )
            } else {
                format!("{open}{}{close}", items.join(","))
            }
        }
        _ => "null".to_string(),
    }
}

/// JSON-escape `s`. By default PHP escapes `/` as `\/` and every non-ASCII
/// character as a `\uXXXX` sequence (surrogate pairs above the BMP);
/// `JSON_UNESCAPED_SLASHES` and `JSON_UNESCAPED_UNICODE` turn those off.
fn json_string(s: &str, flags: i64) -> String {
    let escape_slashes = flags & JSON_UNESCAPED_SLASHES == 0;
    let escape_unicode = flags & JSON_UNESCAPED_UNICODE == 0;
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '/' if escape_slashes => out.push_str("\\/"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if escape_unicode && !c.is_ascii() => {
                // Above the BMP JSON needs an explicit UTF-16 surrogate pair.
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
