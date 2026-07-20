//! The `fopen` file-stream family. A stream is a `PhpObj::Resource` whose content
//! is buffered in memory (read modes load the file up front; write/append modes
//! accumulate); writes flush to disk immediately so no data is lost without an
//! explicit `fclose`. Part of the `stdlib` chain; see `src/stdlib/mod.rs`.

use crate::host::with_host;
use crate::stdlib::common::*;
use fusevm::Value;

/// Dispatch a file-resource PHP function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let v = match name {
        "fopen" => fopen(&str_arg(args, 0), &str_arg(args, 1)),
        "fread" => {
            let res = arg(args, 0);
            let n = int_arg(args, 1).max(0) as usize;
            with_host(|h| match h.res_read(&res, n) {
                Some(s) => Value::str(s),
                None => Value::bool(false),
            })
        }
        "fgets" => {
            let res = arg(args, 0);
            // fgets($h) reads to the next newline; fgets($h, $len) caps at $len-1.
            let max = if args.len() > 1 {
                Some((int_arg(args, 1).max(1) as usize).saturating_sub(1))
            } else {
                None
            };
            with_host(|h| match h.res_gets(&res, max) {
                Some(s) => Value::str(s),
                None => Value::bool(false),
            })
        }
        "fwrite" | "fputs" => {
            let res = arg(args, 0);
            let mut data = str_arg(args, 1).into_bytes();
            if args.len() > 2 {
                let len = int_arg(args, 2).max(0) as usize;
                data.truncate(len);
            }
            let written = with_host(|h| h.res_write(&res, &data));
            flush(&res);
            match written {
                Some(n) => Value::int(n as i64),
                None => Value::bool(false),
            }
        }
        "fclose" => {
            let res = arg(args, 0);
            flush(&res);
            Value::bool(with_host(|h| h.res_close(&res)))
        }
        "fflush" => {
            let res = arg(args, 0);
            flush(&res);
            Value::bool(true)
        }
        "feof" => {
            let res = arg(args, 0);
            Value::bool(with_host(|h| h.res_eof(&res)))
        }
        "ftell" => {
            let res = arg(args, 0);
            with_host(|h| match h.res_tell(&res) {
                Some(p) => Value::int(p),
                None => Value::bool(false),
            })
        }
        "rewind" => {
            let res = arg(args, 0);
            Value::bool(with_host(|h| h.res_seek(&res, 0, 0)))
        }
        "fseek" => {
            let res = arg(args, 0);
            let offset = int_arg(args, 1);
            let whence = if args.len() > 2 { int_arg(args, 2) } else { 0 };
            // fseek returns 0 on success, -1 on failure.
            Value::int(if with_host(|h| h.res_seek(&res, offset, whence)) {
                0
            } else {
                -1
            })
        }
        "stream_get_contents" => {
            let res = arg(args, 0);
            // Read everything remaining from the cursor.
            with_host(|h| {
                let len = h.res_tell(&res).map(|_| usize::MAX).unwrap_or(0);
                match h.res_read(&res, len) {
                    Some(s) => Value::str(s),
                    None => Value::bool(false),
                }
            })
        }
        "is_resource" => Value::bool(with_host(|h| h.is_resource(&arg(args, 0)))),
        "get_resource_type" => {
            if with_host(|h| h.is_resource(&arg(args, 0))) {
                Value::str("stream".to_string())
            } else {
                Value::bool(false)
            }
        }
        _ => return None,
    };
    Some(Ok(v))
}

/// `fopen($path, $mode)` — open a stream, buffering the file per mode.
fn fopen(path: &str, mode: &str) -> Value {
    let m = mode.trim_end_matches(['b', 't']); // binary/text flags are no-ops here
    let plus = m.contains('+');
    let first = m.chars().next().unwrap_or('r');
    match first {
        'r' => match std::fs::read(path) {
            Ok(buf) => with_host(|h| h.new_resource(path, buf, 0, plus)),
            Err(_) => Value::bool(false),
        },
        'w' | 'x' | 'c' => {
            // 'w' truncates; 'x' fails if the file exists; 'c' keeps existing.
            if first == 'x' && std::path::Path::new(path).exists() {
                return Value::bool(false);
            }
            let buf = if first == 'c' {
                std::fs::read(path).unwrap_or_default()
            } else {
                Vec::new()
            };
            // Materialize the file now (truncate for 'w') so it exists on open.
            if first != 'c' && std::fs::write(path, b"").is_err() {
                return Value::bool(false);
            }
            with_host(|h| h.new_resource(path, buf, 0, true))
        }
        'a' => {
            let buf = std::fs::read(path).unwrap_or_default();
            let pos = buf.len();
            with_host(|h| h.new_resource(path, buf, pos, true))
        }
        _ => Value::bool(false),
    }
}

/// Flush a resource's buffered content to disk if it is dirty.
fn flush(res: &Value) {
    if let Some((path, buf)) = with_host(|h| h.res_flush_data(res)) {
        let _ = std::fs::write(path, buf);
    }
}
