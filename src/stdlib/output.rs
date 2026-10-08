//! Output buffering, ported from `main/output.c`.
//!
//! The stack of `php_output_handler`s lives on the host
//! ([`OutputState`]); a write lands in the top enabled handler's buffer (see
//! `PhpHost::ob_write_below`). Running a handler — `php_output_handler_op` —
//! may call a user callback, which is PHP code, so it happens out here, outside
//! any host borrow: the `ob_*` functions run it directly, the end of the request
//! runs it from [`end_all`], and a write that fills a handler's `chunk_size`
//! marks it due and [`drain_chunks`] runs it at the next builtin boundary.
//!
//! The handler sees the reference's `$phase` bits (`PHP_OUTPUT_HANDLER_START`
//! on its first run, `FLUSH`/`CLEAN`/`FINAL` from the operation), its return
//! value is interpreted as the C does (`false` passes the buffer through and
//! disables the handler, `true` and `""` swallow it), and the userland
//! functions raise the reference's notices with its wording.

use crate::host::{self, with_host};
use crate::stdlib::common::*;
use fusevm::Value;
use std::cell::Cell;

/// `PHP_OUTPUT_HANDLER_*` operation bits — the `$phase` a handler receives.
pub const OP_WRITE: i64 = 0x00;
pub const OP_START: i64 = 0x01;
pub const OP_CLEAN: i64 = 0x02;
pub const OP_FLUSH: i64 = 0x04;
pub const OP_FINAL: i64 = 0x08;

/// Handler type and ability flags.
const HANDLER_USER: i64 = 0x0001;
pub const CLEANABLE: i64 = 0x0010;
pub const FLUSHABLE: i64 = 0x0020;
pub const REMOVABLE: i64 = 0x0040;
pub const STDFLAGS: i64 = 0x0070;

/// Status flags the engine sets on a handler.
pub const STARTED: i64 = 0x1000;
pub const DISABLED: i64 = 0x2000;
pub const PROCESSED: i64 = 0x4000;
pub const PRODUCED_OUTPUT: i64 = 0x8000;

/// `php_output_stack_pop` flags.
const POP_FORCE: u8 = 0x1;
const POP_DISCARD: u8 = 0x2;

/// `PHP_OUTPUT_HANDLER_DEFAULT_SIZE` / `_ALIGNTO_SIZE`.
const DEFAULT_SIZE: usize = 0x4000;
const ALIGNTO_SIZE: usize = 0x1000;

const DEFAULT_HANDLER_NAME: &str = "default output handler";

/// The `func` of the operations [`end_all`] runs: the request is shutting down.
const SHUTDOWN: &str = "PHP Request Shutdown";

/// `PHP_OUTPUT_HANDLER_INITBUF_SIZE`: a chunked handler's buffer is its chunk
/// size rounded up to 4 KiB, any other starts at 16 KiB. Observable through
/// `ob_get_status()["buffer_size"]`.
fn initbuf_size(s: usize) -> usize {
    if s > 0 {
        (s + ALIGNTO_SIZE - 1) & !(ALIGNTO_SIZE - 1)
    } else {
        DEFAULT_SIZE
    }
}

/// One `php_output_handler`.
#[derive(Clone)]
pub struct ObHandler {
    /// Identity across a callback, which may change the stack under it.
    pub id: u64,
    pub name: String,
    /// The user callback; `None` is the internal default handler.
    pub callback: Option<Value>,
    /// `chunk_size`: a buffer this full runs the handler on the next write.
    pub size: usize,
    pub flags: i64,
    pub buf: String,
    /// `buffer.size`, the allocation `ob_get_status` reports.
    pub buf_size: usize,
    /// Position on the stack when started.
    pub level: i64,
}

impl ObHandler {
    pub fn is_disabled(&self) -> bool {
        self.flags & DISABLED != 0
    }

    /// `php_output_handler_append`, minus its chunk test (`chunk_full`).
    pub fn append(&mut self, s: &str) {
        let n = s.len();
        if self.buf_size - self.buf.len() <= n {
            let grow_int = initbuf_size(self.size);
            let grow_buf = initbuf_size(n - (self.buf_size - self.buf.len()));
            self.buf_size += grow_int.max(grow_buf);
        }
        self.buf.push_str(s);
    }

    /// Whether the stored data has reached `chunk_size`.
    pub fn chunk_full(&self) -> bool {
        self.size > 0 && self.buf.len() >= self.size
    }
}

/// `OG(...)`: the handler stack, the handler currently running, and a handler
/// whose chunk a write filled.
#[derive(Default)]
pub struct OutputState {
    pub handlers: Vec<ObHandler>,
    pub running: Option<u64>,
    pub chunk_due: Option<u64>,
    next_id: u64,
    /// Cleared by `php_output_deactivate` after the lock error: no handler can
    /// start any more.
    deactivated: bool,
}

thread_local! {
    /// Set with `OutputState::chunk_due`, so the builtin boundary tests a
    /// `Cell` rather than borrowing the host on every call.
    static CHUNK_DUE: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn mark_chunk_due() {
    CHUNK_DUE.with(|c| c.set(true));
}

/// What `php_output_handler_op` reports.
#[derive(Clone, Copy, PartialEq)]
enum Status {
    Failure,
    NoData,
    Success,
}

fn index_of(h: &host::PhpHost, id: u64) -> Option<usize> {
    h.ob.handlers.iter().position(|x| x.id == id)
}

fn active_id() -> Option<u64> {
    with_host(|h| h.ob.handlers.last().map(|x| x.id))
}

/// `php_output_lock_error`: an output-buffering operation from inside a
/// running handler deactivates output buffering (`php_output_deactivate`
/// drops every handler unrun) and is a fatal error.
fn lock_error(func: &str, op: i64) -> Option<String> {
    let locked = with_host(|h| op != 0 && !h.ob.handlers.is_empty() && h.ob.running.is_some());
    if !locked {
        return None;
    }
    with_host(|h| {
        h.ob.handlers.clear();
        h.ob.running = None;
        h.ob.chunk_due = None;
        h.ob.deactivated = true;
    });
    Some(crate::builtins::fatals(format!(
        "{func}(): Cannot use output buffering in output buffering display handlers"
    )))
}

/// `php_output_handler_op` for a handler whose input is already stored in its
/// buffer: run it with `op` (plus `START` the first time) and answer the status
/// and whatever it hands down. `Err` is an engine error the callback raised.
fn handler_op(func: &str, id: u64, op: i64) -> Result<(Status, Option<String>), String> {
    let snapshot = with_host(|h| {
        let i = index_of(h, id)?;
        let hd = &h.ob.handlers[i];
        Some((
            hd.is_disabled(),
            hd.flags & STARTED != 0,
            hd.callback.clone(),
            hd.buf.clone(),
        ))
    });
    let Some((disabled, started, callback, data)) = snapshot else {
        return Ok((Status::Failure, None));
    };
    if disabled {
        return Ok((Status::Failure, None));
    }
    if let Some(e) = lock_error(func, op) {
        return Err(e);
    }
    let op = if started { op } else { op | OP_START };
    with_host(|h| h.ob.running = Some(id));
    let mut error = None;
    let mut still_have = true;
    let (status, mut out) = match callback {
        None => {
            // `php_output_handler_default_func`: the input passes through.
            let status = if data.is_empty() {
                Status::NoData
            } else {
                Status::Success
            };
            (status, Some(data))
        }
        Some(cb) => {
            let r = host::call_value(cb, vec![Value::str(data), Value::int(op)]);
            // `OG(running)` stays set through the epilogue below: a diagnostic
            // it raises is itself output written while a handler runs.
            match r {
                Err(e) => {
                    error = Some(e);
                    (Status::Failure, None)
                }
                // An exception leaves the call's result UNDEF: a failure.
                Ok(_) if host::unwinding() => (Status::Failure, None),
                Ok(ret) => {
                    still_have = produced_output_check(func, id);
                    match ret {
                        Value::Bool(false) => (Status::Failure, None),
                        Value::Bool(true) => (Status::NoData, None),
                        other => {
                            let s = with_host(|h| h.to_str_diag(&other));
                            if s.is_empty() {
                                (Status::NoData, None)
                            } else {
                                (Status::Success, Some(s))
                            }
                        }
                    }
                }
            }
        }
    };
    with_host(|h| h.ob.running = None);
    if !still_have {
        return error.map_or(Ok((status, out)), Err);
    }
    with_host(|h| {
        let Some(i) = index_of(h, id) else {
            return;
        };
        let hd = &mut h.ob.handlers[i];
        hd.flags |= STARTED;
        match status {
            Status::Failure => {
                // Disabled, and its whole buffer goes down unprocessed.
                hd.flags |= DISABLED;
                out = Some(std::mem::take(&mut hd.buf));
                hd.buf_size = 0;
            }
            Status::NoData | Status::Success => {
                if status == Status::NoData {
                    out = None;
                }
                hd.buf.clear();
                hd.flags |= PROCESSED;
            }
        }
    });
    error.map_or(Ok((status, out)), Err)
}

/// The `PHP_OUTPUT_HANDLER_PRODUCED_OUTPUT` epilogue of a user handler: output
/// written while it ran is deprecated (and lost with its buffer). Answers
/// whether the handler is still on the stack.
fn produced_output_check(func: &str, id: u64) -> bool {
    with_host(|h| {
        let Some(i) = index_of(h, id) else {
            return false;
        };
        let hd = &mut h.ob.handlers[i];
        if hd.flags & PRODUCED_OUTPUT != 0 {
            // Disabled while the deprecation is written, so its text passes to
            // the handler below rather than into the buffer about to be lost.
            hd.flags |= DISABLED;
            hd.flags &= !PRODUCED_OUTPUT;
            let name = hd.name.clone();
            let msg = format!("Producing output from user output handler {name} is deprecated");
            match func {
                SHUTDOWN => h.deprecated_at_shutdown(msg),
                // A chunk filled by a write: the function running the write.
                "" => {
                    let active = h.active_function_name();
                    h.deprecated(format!("{active}: {msg}"));
                }
                _ => h.deprecated(format!("{func}(): {msg}")),
            }
            if let Some(i) = index_of(h, id) {
                h.ob.handlers[i].flags &= !DISABLED;
            }
        }
        true
    })
}

/// Pass a handler's output to the level below it.
fn write_below(id: u64, out: Option<String>) {
    if let Some(out) = out.filter(|s| !s.is_empty()) {
        with_host(|h| {
            let n = index_of(h, id).unwrap_or(h.ob.handlers.len());
            h.ob_write_below(n, &out);
        });
    }
}

/// Run every handler a write filled to its `chunk_size` — the part of
/// `php_output_op` that calls a handler mid-write. Answers whether a callback
/// threw, so the caller can stop the chunk as it does for any throwing builtin.
pub fn drain_chunks() -> bool {
    if !CHUNK_DUE.with(|c| c.replace(false)) {
        return false;
    }
    while let Some(id) = with_host(|h| h.ob.chunk_due.take()) {
        let due = with_host(|h| {
            index_of(h, id).is_some_and(|i| {
                let hd = &h.ob.handlers[i];
                !hd.is_disabled() && hd.chunk_full()
            })
        });
        if !due {
            continue;
        }
        match handler_op("", id, OP_WRITE) {
            Ok((_, out)) => write_below(id, out),
            Err(e) => {
                with_host(|h| h.set_error(e));
                return true;
            }
        }
        if host::unwinding() {
            return true;
        }
    }
    host::unwinding()
}

/// `php_output_stack_pop`: run the top handler one last time (`FINAL`, plus
/// `CLEAN` when discarding), pop it, and write what it produced unless
/// discarding.
fn stack_pop(func: &str, flags: u8) -> Result<bool, String> {
    let top = with_host(|h| h.ob.handlers.last().cloned());
    let discard = flags & POP_DISCARD != 0;
    let verb = if discard { "discard" } else { "send" };
    let Some(orphan) = top else {
        with_host(|h| {
            h.notice(format!(
                "{func}(): Failed to {verb} buffer. No buffer to {verb}"
            ))
        });
        return Ok(false);
    };
    if flags & POP_FORCE == 0 && orphan.flags & REMOVABLE == 0 {
        with_host(|h| {
            h.notice(format!(
                "{func}(): Failed to {verb} buffer of {} ({})",
                orphan.name, orphan.level
            ))
        });
        return Ok(false);
    }
    let mut out = None;
    let mut error = None;
    if !orphan.is_disabled() {
        let op = OP_FINAL | if discard { OP_CLEAN } else { 0 };
        match handler_op(func, orphan.id, op) {
            Ok((_, o)) => out = o,
            Err(e) => error = Some(e),
        }
    }
    with_host(|h| {
        h.ob.handlers.pop();
    });
    if !discard {
        if let Some(out) = out.filter(|s| !s.is_empty()) {
            with_host(|h| h.write_out(&out));
        }
    }
    error.map_or(Ok(true), Err)
}

/// `php_output_end_all`: what the end of the request does — every handler is
/// finalized and its output passed down, innermost first, removable or not.
pub fn end_all() {
    // An `exit` status is parked across the handlers, as across every other
    // shutdown callback, or each would see the run unwinding and not run.
    let exit = with_host(|h| h.pending_exit.take());
    while active_id().is_some() {
        if stack_pop(SHUTDOWN, POP_FORCE).is_err() {
            // A fatal raised by the handler itself: the rest still unwind.
            continue;
        }
    }
    drain_chunks();
    with_host(|h| {
        if h.pending_exit.is_none() {
            h.pending_exit = exit;
        }
    });
}

/// `php_output_flush`: run the top handler with `FLUSH` and pass its output to
/// the level below, keeping the handler.
fn flush(func: &str) -> Result<bool, String> {
    let Some(id) = with_host(|h| {
        h.ob.handlers
            .last()
            .filter(|x| x.flags & FLUSHABLE != 0)
            .map(|x| x.id)
    }) else {
        return Ok(false);
    };
    let (_, out) = handler_op(func, id, OP_FLUSH)?;
    write_below(id, out);
    Ok(true)
}

/// `php_output_clean`: run the top handler with `CLEAN` and discard both its
/// buffer and what it returned.
fn clean(func: &str) -> Result<bool, String> {
    let Some(id) = with_host(|h| {
        h.ob.handlers
            .last()
            .filter(|x| x.flags & CLEANABLE != 0)
            .map(|x| x.id)
    }) else {
        return Ok(false);
    };
    handler_op(func, id, OP_CLEAN)?;
    Ok(true)
}

/// `zend_get_callable_name_ex`: how a handler names its callback in
/// `ob_list_handlers()` and the status array.
fn callable_name(cb: &Value) -> String {
    with_host(|h| {
        if let Some(name) = h.closure_callable_name(cb) {
            return name;
        }
        if h.is_array(cb) {
            let pairs = h.array_pairs(cb).unwrap_or_default();
            let target = pairs.first().map(|p| p.1.clone()).unwrap_or(Value::Undef);
            let method = pairs.get(1).map(|p| h.to_str(&p.1)).unwrap_or_default();
            let class = if h.is_object(&target) {
                h.object_class(&target)
                    .map(|c| h.class_display_name(&c.to_ascii_lowercase()))
                    .unwrap_or_default()
            } else {
                h.to_str(&target)
            };
            return format!("{class}::{method}");
        }
        if h.is_object(cb) {
            let class = h
                .object_class(cb)
                .map(|c| h.class_display_name(&c.to_ascii_lowercase()))
                .unwrap_or_default();
            return format!("{class}::__invoke");
        }
        h.to_str(cb)
    })
}

/// `php_output_handler_status`.
fn status_array(h: &mut host::PhpHost, hd: &ObHandler) -> Value {
    let arr = h.new_array();
    h.arr_set_key(&arr, &Value::str("name"), Value::str(hd.name.clone()));
    let fields = [
        ("type", hd.flags & 0xf),
        ("flags", hd.flags),
        ("level", hd.level),
        ("chunk_size", hd.size as i64),
        ("buffer_size", hd.buf_size as i64),
        ("buffer_used", hd.buf.len() as i64),
    ];
    for (k, v) in fields {
        h.arr_set_key(&arr, &Value::str(k), Value::int(v));
    }
    arr
}

/// `ob_start($callback = null, $chunk_size = 0, $flags = PHP_OUTPUT_HANDLER_STDFLAGS)`.
fn ob_start(args: &[Value]) -> Result<Value, String> {
    let cb = arg(args, 0);
    let chunk = args
        .get(1)
        .map_or(0, |v| with_host(|h| h.to_number(v).to_int()))
        .max(0) as usize;
    let flags = args
        .get(2)
        .map_or(STDFLAGS, |v| with_host(|h| h.to_number(v).to_int()));
    // `PHP_OUTPUT_HANDLER_ABILITY_FLAGS`: only the ability bits are the caller's.
    let ability = flags & !0xf00f;
    let (name, callback, kind) = match &cb {
        Value::Undef => (DEFAULT_HANDLER_NAME.to_string(), None, 0),
        _ => match crate::stdlib::callable::callable_reason(&cb) {
            Some(reason) => {
                with_host(|h| {
                    h.warn(format!("ob_start(): {reason}"));
                    h.notice("ob_start(): Failed to create buffer");
                });
                return Ok(Value::bool(false));
            }
            None => (callable_name(&cb), Some(cb.clone()), HANDLER_USER),
        },
    };
    if let Some(e) = lock_error("ob_start", OP_START) {
        return Err(e);
    }
    let started = with_host(|h| {
        if h.ob.deactivated {
            return false;
        }
        let id = h.ob.next_id;
        h.ob.next_id += 1;
        let level = h.ob.handlers.len() as i64;
        h.ob.handlers.push(ObHandler {
            id,
            name,
            callback,
            size: chunk,
            flags: ability | kind,
            buf: String::new(),
            buf_size: initbuf_size(chunk),
            level,
        });
        true
    });
    if !started {
        with_host(|h| h.notice("ob_start(): Failed to create buffer"));
    }
    Ok(Value::bool(started))
}

/// The output-control functions of `main/output.c`'s userland section,
/// reached through `stdlib::system`'s dispatch.
pub fn call(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let has_active = || with_host(|h| !h.ob.handlers.is_empty());
    let notice = |msg: String| with_host(|h| h.notice(msg));
    let r = match name {
        "ob_start" => ob_start(args),
        "ob_flush" => (|| {
            if !has_active() {
                notice("ob_flush(): Failed to flush buffer. No buffer to flush".into());
                return Ok(Value::bool(false));
            }
            if !flush(name)? {
                let (n, l) = active_name_level();
                notice(format!("ob_flush(): Failed to flush buffer of {n} ({l})"));
                return Ok(Value::bool(false));
            }
            Ok(Value::bool(true))
        })(),
        "ob_clean" => (|| {
            if !has_active() {
                notice("ob_clean(): Failed to delete buffer. No buffer to delete".into());
                return Ok(Value::bool(false));
            }
            if !clean(name)? {
                let (n, l) = active_name_level();
                notice(format!("ob_clean(): Failed to delete buffer of {n} ({l})"));
                return Ok(Value::bool(false));
            }
            Ok(Value::bool(true))
        })(),
        "ob_end_flush" => (|| {
            if !has_active() {
                notice(
                    "ob_end_flush(): Failed to delete and flush buffer. No buffer to delete or flush"
                        .into(),
                );
                return Ok(Value::bool(false));
            }
            Ok(Value::bool(stack_pop(name, 0)?))
        })(),
        "ob_end_clean" => (|| {
            if !has_active() {
                notice("ob_end_clean(): Failed to delete buffer. No buffer to delete".into());
                return Ok(Value::bool(false));
            }
            Ok(Value::bool(stack_pop(name, POP_DISCARD)?))
        })(),
        "ob_get_flush" => (|| {
            let Some(contents) = contents() else {
                notice(
                    "ob_get_flush(): Failed to delete and flush buffer. No buffer to delete or flush"
                        .into(),
                );
                return Ok(Value::bool(false));
            };
            if !stack_pop(name, 0)? {
                let (n, l) = active_name_level();
                notice(format!(
                    "ob_get_flush(): Failed to delete buffer of {n} ({l})"
                ));
            }
            Ok(Value::str(contents))
        })(),
        "ob_get_clean" => (|| {
            let Some(contents) = contents() else {
                return Ok(Value::bool(false));
            };
            if !stack_pop(name, POP_DISCARD)? {
                let (n, l) = active_name_level();
                notice(format!(
                    "ob_get_clean(): Failed to delete buffer of {n} ({l})"
                ));
            }
            Ok(Value::str(contents))
        })(),
        "ob_get_contents" => Ok(contents().map_or(Value::bool(false), Value::str)),
        "ob_get_length" => Ok(with_host(|h| {
            h.ob.handlers
                .last()
                .map_or(Value::bool(false), |x| Value::int(x.buf.len() as i64))
        })),
        "ob_get_level" => Ok(Value::int(with_host(|h| h.ob.handlers.len() as i64))),
        "ob_list_handlers" => {
            let names: Vec<Value> = with_host(|h| {
                h.ob.handlers
                    .iter()
                    .map(|x| Value::str(x.name.clone()))
                    .collect()
            });
            Ok(make_list(names))
        }
        "ob_get_status" => Ok(with_host(|h| {
            let full = args.first().is_some_and(|v| h.is_truthy(v));
            let handlers = h.ob.handlers.clone();
            match handlers.last() {
                None => h.new_array(),
                Some(top) if !full => status_array(h, top),
                Some(_) => {
                    let list = h.new_array();
                    for x in &handlers {
                        let entry = status_array(h, x);
                        h.arr_push_auto(&list, entry);
                    }
                    list
                }
            }
        })),
        // Implicit flushing only changes when the SAPI flushes, which nothing
        // here can observe.
        "ob_implicit_flush" => Ok(Value::Undef),
        "flush" => Ok(Value::Undef),
        _ => return None,
    };
    if r.is_ok() {
        drain_chunks();
    }
    Some(r)
}

fn contents() -> Option<String> {
    with_host(|h| h.ob.handlers.last().map(|x| x.buf.clone()))
}

fn active_name_level() -> (String, i64) {
    with_host(|h| {
        h.ob.handlers
            .last()
            .map_or((String::new(), 0), |x| (x.name.clone(), x.level))
    })
}
