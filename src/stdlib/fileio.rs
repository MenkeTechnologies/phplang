//! PHP standard-library `fileio` functions. Part of the `stdlib` chain; see
//! `src/stdlib/mod.rs`. `dispatch` returns `None` for names it does not handle.
//!
//! Filesystem access is done with `std::fs`/`std::path`. Failures return PHP's
//! `false` (a `Value::Bool(false)`) rather than raising a warning, matching how
//! these functions behave when warnings are suppressed with `@`.
//!
//! LIMITATION — no stream/resource handles. `fopen`/`fread`/`fwrite`/`fclose`
//! are intentionally NOT implemented: a `resource` needs a persistent
//! handle table on the host (off-limits for this module). Everything here is
//! whole-file (`file_get_contents`/`file_put_contents`/`file`/`readfile`) or a
//! stateless path/stat operation, so no host changes are required.

use crate::host::with_host;
use crate::stdlib::common::*;
use fusevm::Value;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// PHP `false` — the conventional failure result for these functions.
fn f() -> Value {
    Value::bool(false)
}

/// Whether the `i`-th argument was actually supplied (not a missing/`null` slot).
fn provided(args: &[Value], i: usize) -> bool {
    matches!(args.get(i), Some(v) if !matches!(v, Value::Undef))
}

/// PHP `basename`: the trailing name component of a path (a pure string
/// operation, independent of the filesystem). Trailing `/` are ignored; an
/// optional `suffix` is stripped when it is a proper suffix of the result.
fn php_basename(path: &str, suffix: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    let base = match trimmed.rfind('/') {
        Some(i) => &trimmed[i + 1..],
        None => trimmed,
    };
    if !suffix.is_empty() && base != suffix && base.ends_with(suffix) {
        base[..base.len() - suffix.len()].to_string()
    } else {
        base.to_string()
    }
}

/// One level of PHP `dirname`: the parent-directory portion of a path.
fn php_dirname_once(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        // All-slash input ("/", "//") → "/"; an empty string → ".".
        return if path.is_empty() { "." } else { "/" }.to_string();
    }
    match trimmed.rfind('/') {
        Some(0) => "/".to_string(),
        Some(i) => trimmed[..i].to_string(),
        None => ".".to_string(),
    }
}

/// PHP `dirname($path, $levels)` — apply `php_dirname_once` `levels` times.
fn php_dirname(path: &str, levels: i64) -> String {
    let mut cur = path.to_string();
    for _ in 0..levels.max(1) {
        cur = php_dirname_once(&cur);
    }
    cur
}

/// The `(filename, extension)` split of a basename: extension is the text after
/// the last interior `.` (a leading dot, as in `.bashrc`, is not an extension).
fn split_ext(base: &str) -> (&str, Option<&str>) {
    match base.rfind('.') {
        Some(0) | None => (base, None),
        Some(i) => (&base[..i], Some(&base[i + 1..])),
    }
}

/// Whether the append flag (PHP `FILE_APPEND`, integer `8`) is set. As phplang
/// has no constant table, a bareword `FILE_APPEND` reaches us as its name.
fn has_append_flag(v: &Value) -> bool {
    match v {
        Value::Int(n) => n & 8 != 0,
        Value::Str(s) => s.eq_ignore_ascii_case("FILE_APPEND"),
        _ => false,
    }
}

/// Convert a `file()` flags argument to the two behaviors phplang honors:
/// `FILE_IGNORE_NEW_LINES` (2) and `FILE_SKIP_EMPTY_LINES` (4). Bareword
/// constant names are accepted alongside the canonical integers.
fn file_flags(v: &Value) -> (bool, bool) {
    match v {
        Value::Int(n) => (n & 2 != 0, n & 4 != 0),
        Value::Str(s) => {
            let up = s.to_ascii_uppercase();
            (up.contains("IGNORE_NEW_LINES"), up.contains("SKIP_EMPTY_LINES"))
        }
        _ => (false, false),
    }
}

/// Render the `data` argument of `file_put_contents` to bytes: a scalar is
/// string-cast; an array is the concatenation of its string-cast elements
/// (PHP joins array elements with no separator).
fn put_data_string(args: &[Value]) -> String {
    let data = arg(args, 1);
    with_host(|h| {
        if h.is_array(&data) {
            h.array_pairs(&data)
                .unwrap_or_default()
                .into_iter()
                .map(|(_, v)| h.to_str(&v))
                .collect()
        } else {
            h.to_str(&data)
        }
    })
}

/// `filemtime` helper: a file's modification time as whole Unix seconds.
fn mtime_secs(path: &str) -> Option<i64> {
    let m = fs::metadata(path).ok()?;
    let t = m.modified().ok()?;
    let d = t.duration_since(UNIX_EPOCH).ok()?;
    Some(d.as_secs() as i64)
}

/// `pathinfo` single-component selector for the options argument (`PATHINFO_*`),
/// accepting both the canonical integer and the bareword constant name.
fn pathinfo_component(opt: &Value, dir: &str, base: &str, ext: Option<&str>, fname: &str) -> Value {
    let want = match opt {
        Value::Int(n) => *n,
        Value::Str(s) => match s.to_ascii_uppercase().as_str() {
            "PATHINFO_DIRNAME" => 1,
            "PATHINFO_BASENAME" => 2,
            "PATHINFO_EXTENSION" => 4,
            "PATHINFO_FILENAME" => 8,
            _ => 0,
        },
        _ => 0,
    };
    let s = match want {
        1 => dir.to_string(),
        2 => base.to_string(),
        4 => ext.unwrap_or("").to_string(),
        8 => fname.to_string(),
        _ => String::new(),
    };
    Value::str(s)
}

/// Dispatch a `fileio`-category PHP function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let v: Value = match name {
        // ── whole-file read/write ──────────────────────────────────────────
        "file_get_contents" => {
            let path = str_arg(args, 0);
            match fs::read(&path) {
                Ok(bytes) => Value::str(String::from_utf8_lossy(&bytes).into_owned()),
                Err(_) => f(),
            }
        }
        "file_put_contents" => {
            let path = str_arg(args, 0);
            let data = put_data_string(args);
            let append = has_append_flag(&arg(args, 2));
            let res = if append {
                use std::io::Write;
                fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .and_then(|mut fh| fh.write_all(data.as_bytes()))
            } else {
                fs::write(&path, data.as_bytes())
            };
            match res {
                Ok(()) => Value::int(data.len() as i64),
                Err(_) => f(),
            }
        }
        "file" => {
            let path = str_arg(args, 0);
            let (ignore_nl, skip_empty) = file_flags(&arg(args, 1));
            match fs::read(&path) {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    let mut lines: Vec<Value> = Vec::new();
                    for line in text.split_inclusive('\n') {
                        if skip_empty && line.trim_end_matches(['\r', '\n']).is_empty() {
                            continue;
                        }
                        let out = if ignore_nl {
                            line.trim_end_matches('\n').trim_end_matches('\r')
                        } else {
                            line
                        };
                        lines.push(Value::str(out.to_string()));
                    }
                    make_list(lines)
                }
                Err(_) => f(),
            }
        }
        "readfile" => {
            let path = str_arg(args, 0);
            match fs::read(&path) {
                Ok(bytes) => {
                    let s = String::from_utf8_lossy(&bytes).into_owned();
                    with_host(|h| h.write_out(&s));
                    Value::int(bytes.len() as i64)
                }
                Err(_) => f(),
            }
        }

        // ── existence / type / permission predicates ───────────────────────
        "file_exists" => Value::bool(Path::new(&str_arg(args, 0)).exists()),
        "is_file" => Value::bool(Path::new(&str_arg(args, 0)).is_file()),
        "is_dir" => Value::bool(Path::new(&str_arg(args, 0)).is_dir()),
        // A path is readable if it can be stat'd; writable if its metadata says
        // the entry is not read-only. Both are false for a missing path.
        "is_readable" => Value::bool(fs::metadata(str_arg(args, 0)).is_ok()),
        "is_writable" | "is_writeable" => Value::bool(
            fs::metadata(str_arg(args, 0))
                .map(|m| !m.permissions().readonly())
                .unwrap_or(false),
        ),

        // ── mutating filesystem operations ─────────────────────────────────
        "unlink" => Value::bool(fs::remove_file(str_arg(args, 0)).is_ok()),
        "mkdir" => {
            let path = str_arg(args, 0);
            // Signature: mkdir(dir, permissions = 0777, recursive = false).
            let recursive = with_host(|h| h.is_truthy(&arg(args, 2)));
            let res = if recursive {
                fs::create_dir_all(&path)
            } else {
                fs::create_dir(&path)
            };
            Value::bool(res.is_ok())
        }
        "rmdir" => Value::bool(fs::remove_dir(str_arg(args, 0)).is_ok()),
        "rename" => Value::bool(fs::rename(str_arg(args, 0), str_arg(args, 1)).is_ok()),
        "copy" => Value::bool(fs::copy(str_arg(args, 0), str_arg(args, 1)).is_ok()),
        "touch" => {
            let path = str_arg(args, 0);
            // Create the file if absent, then stamp its mtime (the given Unix
            // time, or "now"). PHP's `atime` third argument is not honored:
            // `std::fs::File` exposes only `set_modified`.
            let when = if provided(args, 1) {
                UNIX_EPOCH
                    .checked_add(Duration::from_secs(int_arg(args, 1).max(0) as u64))
                    .unwrap_or_else(SystemTime::now)
            } else {
                SystemTime::now()
            };
            let res = fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(false)
                .open(&path)
                .and_then(|fh| fh.set_modified(when));
            Value::bool(res.is_ok())
        }

        // ── directory listing ──────────────────────────────────────────────
        "scandir" => {
            let path = str_arg(args, 0);
            match fs::read_dir(&path) {
                Ok(rd) => {
                    let mut names: Vec<String> = vec![".".to_string(), "..".to_string()];
                    for entry in rd.flatten() {
                        names.push(entry.file_name().to_string_lossy().into_owned());
                    }
                    names.sort();
                    // Second argument SCANDIR_SORT_DESCENDING (1) reverses.
                    let desc = match arg(args, 1) {
                        Value::Int(n) => n == 1,
                        Value::Str(s) => s.eq_ignore_ascii_case("SCANDIR_SORT_DESCENDING"),
                        _ => false,
                    };
                    if desc {
                        names.reverse();
                    }
                    make_list(names.into_iter().map(Value::str).collect())
                }
                Err(_) => f(),
            }
        }

        // ── stat ───────────────────────────────────────────────────────────
        "filesize" => match fs::metadata(str_arg(args, 0)) {
            Ok(m) => Value::int(m.len() as i64),
            Err(_) => f(),
        },
        "filemtime" => match mtime_secs(&str_arg(args, 0)) {
            Some(secs) => Value::int(secs),
            None => f(),
        },

        // ── path string operations (no filesystem access) ──────────────────
        "basename" => {
            let path = str_arg(args, 0);
            let suffix = if provided(args, 1) {
                str_arg(args, 1)
            } else {
                String::new()
            };
            Value::str(php_basename(&path, &suffix))
        }
        "dirname" => {
            let path = str_arg(args, 0);
            let levels = if provided(args, 1) { int_arg(args, 1) } else { 1 };
            Value::str(php_dirname(&path, levels))
        }
        "pathinfo" => {
            let path = str_arg(args, 0);
            let dir = php_dirname_once(&path);
            let base = php_basename(&path, "");
            let (fname, ext) = split_ext(&base);
            if provided(args, 1) {
                pathinfo_component(&arg(args, 1), &dir, &base, ext, fname)
            } else {
                let mut pairs: Vec<(Value, Value)> = vec![
                    (Value::str("dirname"), Value::str(dir)),
                    (Value::str("basename"), Value::str(base.clone())),
                ];
                if let Some(e) = ext {
                    pairs.push((Value::str("extension"), Value::str(e.to_string())));
                }
                pairs.push((Value::str("filename"), Value::str(fname.to_string())));
                make_map(pairs)
            }
        }

        // ── environment / canonicalization ─────────────────────────────────
        "realpath" => match fs::canonicalize(str_arg(args, 0)) {
            Ok(p) => Value::str(p.to_string_lossy().into_owned()),
            Err(_) => f(),
        },
        "getcwd" => match std::env::current_dir() {
            Ok(p) => Value::str(p.to_string_lossy().into_owned()),
            Err(_) => f(),
        },
        "sys_get_temp_dir" => {
            let p = std::env::temp_dir();
            let s = p.to_string_lossy();
            // PHP returns the temp dir without a trailing separator.
            Value::str(s.trim_end_matches('/').to_string())
        }

        _ => return None,
    };
    Some(Ok(v))
}
