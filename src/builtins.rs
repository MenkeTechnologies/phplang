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
    vm.register_builtin(ops::MKARRAY, b_mkarray);
    vm.register_builtin(ops::INDEX_GET, b_index_get);
    vm.register_builtin(ops::INDEX_SET, b_index_set);
    vm.register_builtin(ops::ARR_APPEND, b_arr_append);
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
        Ok(v) => v,
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
                match x.cmp(y) {
                    std::cmp::Ordering::Less => -1,
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Greater => 1,
                }
            }
        }
        (Obj(_), Obj(_)) => cmp_f64(h.array_len(a) as f64, h.array_len(b) as f64),
        _ => cmp_f64(h.to_number(a).to_float(), h.to_number(b).to_float()),
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
                Value::Int(n) => n.checked_neg().map(Value::int).unwrap_or(Value::float(-(n as f64))),
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

fn arg(args: &[Value], i: usize) -> Value {
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
        "strrev" => with_host(|h| Value::str(h.to_str(&arg(&args, 0)).chars().rev().collect::<String>())),
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
        "max" => with_host(|h| fold_cmp(h, &args, true)),
        "min" => with_host(|h| fold_cmp(h, &args, false)),
        "gettype" => with_host(|h| Value::str(h.type_name(&arg(&args, 0)).to_string())),
        "is_array" => with_host(|h| Value::bool(h.is_array(&arg(&args, 0)))),
        "is_int" | "is_integer" | "is_long" => {
            Value::bool(matches!(arg(&args, 0), Value::Int(_)))
        }
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
        _ => return Err(format!("call to undefined function {name}()")),
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
        h.array_pairs(&args[0]).unwrap_or_default().into_iter().map(|(_, v)| v).collect()
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
    chars[start..(start + count).min(chars.len())].iter().collect()
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
    let found = h.array_pairs(&hay).unwrap_or_default().iter().any(|(_, v)| {
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

/// A small `sprintf`: supports `%s %d %i %f %b %% ` (no width/precision flags).
fn php_sprintf(h: &host::PhpHost, args: &[Value]) -> String {
    let fmt = h.to_str(&arg(args, 0));
    let mut out = String::new();
    let mut ai = 1;
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some('s') => {
                out.push_str(&h.to_str(&arg(args, ai)));
                ai += 1;
            }
            Some('d') | Some('i') => {
                out.push_str(&h.to_number(&arg(args, ai)).to_int().to_string());
                ai += 1;
            }
            Some('f') | Some('F') => {
                out.push_str(&format!("{:.6}", h.to_number(&arg(args, ai)).to_float()));
                ai += 1;
            }
            Some('b') => {
                out.push_str(&format!("{:b}", h.to_number(&arg(args, ai)).to_int()));
                ai += 1;
            }
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

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
