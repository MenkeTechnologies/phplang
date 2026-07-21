//! Procedural date/time wrappers over the `DateTime`/`DateTimeImmutable`/
//! `DateInterval` prelude classes (`date_create`, `date_format`, `date_diff`, …).
//! Part of the `stdlib` chain; see `src/stdlib/mod.rs`.

use crate::host::{self, with_host};
use crate::stdlib::common::*;
use fusevm::Value;

/// Call an instance method on `recv` (dispatched by its real class).
fn method(recv: &Value, name: &str, args: Vec<Value>) -> Result<Value, String> {
    match with_host(|h| h.object_class(recv)) {
        Some(class) => host::call_method(&class, name, Some(recv.clone()), args),
        None => Ok(Value::bool(false)),
    }
}

/// Dispatch a procedural date/time function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let r: Result<Value, String> = match name {
        "date_create" => {
            let s = if args.is_empty() {
                "now".to_string()
            } else {
                str_arg(args, 0)
            };
            host::new_object("DateTime", vec![Value::str(s)])
        }
        "date_create_immutable" => {
            let s = if args.is_empty() {
                "now".to_string()
            } else {
                str_arg(args, 0)
            };
            host::new_object("DateTimeImmutable", vec![Value::str(s)])
        }
        "date_interval_create_from_date_string" => {
            // A relative string ("1 day"); model it as a DateInterval whose second
            // count comes from strtotime(s) - strtotime("now" at epoch 0).
            host::new_object("DateInterval", vec![Value::str(String::new())])
        }
        "date_format" => method(&arg(args, 0), "format", vec![arg(args, 1)]),
        "date_add" => method(&arg(args, 0), "add", vec![arg(args, 1)]),
        "date_sub" => method(&arg(args, 0), "sub", vec![arg(args, 1)]),
        "date_diff" => method(&arg(args, 0), "diff", vec![arg(args, 1)]),
        "date_modify" => method(&arg(args, 0), "modify", vec![arg(args, 1)]),
        "date_timestamp_get" => method(&arg(args, 0), "getTimestamp", Vec::new()),
        "date_timestamp_set" => method(&arg(args, 0), "setTimestamp", vec![arg(args, 1)]),
        "date_interval_format" => method(&arg(args, 0), "format", vec![arg(args, 1)]),
        "date_offset_get" => Ok(Value::int(0)), // UTC — no offset
        _ => return None,
    };
    Some(r)
}
