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
use std::ffi::CString;
use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
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
/// the last `.`. PHP splits on the last dot even when it is the leading
/// character, so `.htaccess` has extension `htaccess` and filename `""` (this
/// mirrors `zend_memrchr` in `php_pathinfo`).
fn split_ext(base: &str) -> (&str, Option<&str>) {
    match base.rfind('.') {
        None => (base, None),
        Some(i) => (&base[..i], Some(&base[i + 1..])),
    }
}

/// Whether the append flag (PHP `FILE_APPEND`, integer `8`) is set. The constant
/// table is seeded, so this normally arrives as the real int `8`; an unseeded
/// bareword `FILE_APPEND` is still accepted as its name for robustness.
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
            (
                up.contains("IGNORE_NEW_LINES"),
                up.contains("SKIP_EMPTY_LINES"),
            )
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

// ── glob / fnmatch pattern matching ─────────────────────────────────────────

/// Case-sensitive-or-folded comparison of two `char`s (ASCII fold only, matching
/// PHP's `FNM_CASEFOLD` which is ASCII).
fn ch_eq(a: char, b: char, casefold: bool) -> bool {
    if casefold {
        a.eq_ignore_ascii_case(&b)
    } else {
        a == b
    }
}

/// Whether `c` falls in the inclusive `lo..=hi` range of a bracket expression.
fn in_range(lo: char, hi: char, c: char, casefold: bool) -> bool {
    if casefold {
        (lo..=hi).contains(&c)
            || (lo..=hi).contains(&c.to_ascii_lowercase())
            || (lo..=hi).contains(&c.to_ascii_uppercase())
    } else {
        (lo..=hi).contains(&c)
    }
}

/// Match a single `char` against a `[...]` bracket expression at the head of
/// `p` (`p[0] == '['`). Returns `(chars_consumed, matched)`, or `None` when the
/// bracket is unterminated (in which case `[` is a literal character).
fn match_bracket(p: &[char], c: char, casefold: bool) -> Option<(usize, bool)> {
    let mut i = 1;
    let negate = matches!(p.get(i), Some('!') | Some('^'));
    if negate {
        i += 1;
    }
    let mut matched = false;
    let mut closed = false;
    let mut first = true;
    while i < p.len() {
        if p[i] == ']' && !first {
            i += 1;
            closed = true;
            break;
        }
        first = false;
        // `a-b` range, but a trailing `-` (before `]`) is a literal dash.
        if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
            if in_range(p[i], p[i + 2], c, casefold) {
                matched = true;
            }
            i += 3;
        } else {
            if ch_eq(p[i], c, casefold) {
                matched = true;
            }
            i += 1;
        }
    }
    if !closed {
        return None;
    }
    Some((i, if negate { !matched } else { matched }))
}

/// Glob-style wildcard match (`*`, `?`, `[...]`) over full strings. Iterative
/// with single-star backtracking; `*` matches any run including `/` (PHP's
/// `fnmatch` without `FNM_PATHNAME`, and glob within one directory level).
fn wildcard_match(pattern: &str, text: &str, casefold: bool) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        let mut advanced = false;
        if pi < p.len() {
            match p[pi] {
                '*' => {
                    star = Some((pi, ti));
                    pi += 1;
                    continue;
                }
                '?' => {
                    pi += 1;
                    ti += 1;
                    continue;
                }
                '[' => match match_bracket(&p[pi..], t[ti], casefold) {
                    Some((consumed, true)) => {
                        pi += consumed;
                        ti += 1;
                        advanced = true;
                    }
                    Some((_, false)) => {}
                    None => {
                        if ch_eq('[', t[ti], casefold) {
                            pi += 1;
                            ti += 1;
                            advanced = true;
                        }
                    }
                },
                c => {
                    if ch_eq(c, t[ti], casefold) {
                        pi += 1;
                        ti += 1;
                        advanced = true;
                    }
                }
            }
        }
        if advanced {
            continue;
        }
        // Mismatch: backtrack to just past the last `*`, extending its match.
        match star {
            Some((sp, st)) => {
                pi = sp + 1;
                ti = st + 1;
                star = Some((sp, st + 1));
            }
            None => return false,
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

// The `glob`/`fnmatch` flag bits, and the values the host seeds the matching
// constants with — one definition, so the two cannot disagree.
//
// PHP takes these from libc, so they are PLATFORM-dependent; every value here
// was read off the reference `php` on macOS. The matcher below is hand-rolled
// rather than a libc `glob(3)` call, so it only needs to agree with the
// constants phplang itself seeds — which it now does by construction. A script
// hardcoding a numeric literal from a different platform's libc is the one case
// that would still differ.
//
// The bits were previously written inline and were a MIXTURE of two platforms'
// values (`ONLYDIR` was glibc's `0x2000`, `NOSORT` glibc's `4`, `MARK` macOS's
// `8`), matching neither. Nothing caught it because an unseeded bareword
// resolved to its own NAME under the pre-PHP-8 fallback and took the string
// branch of `has_flag`, so the int branch was never reached.
pub(crate) const GLOB_ERR: i64 = 4;
pub(crate) const GLOB_MARK: i64 = 8;
pub(crate) const GLOB_NOCHECK: i64 = 16;
pub(crate) const GLOB_NOSORT: i64 = 32;
pub(crate) const GLOB_BRACE: i64 = 128;
pub(crate) const GLOB_NOESCAPE: i64 = 4096;
pub(crate) const GLOB_ONLYDIR: i64 = 1073741824;
pub(crate) const GLOB_AVAILABLE_FLAGS: i64 = 1073746108;
pub(crate) const FNM_NOESCAPE: i64 = 1;
pub(crate) const FNM_PATHNAME: i64 = 2;
pub(crate) const FNM_PERIOD: i64 = 4;
pub(crate) const FNM_CASEFOLD: i64 = 16;

/// Whether a `glob`/`fnmatch` flag argument requests the behavior named by
/// `name` (matched against the seeded int bit `bit`, or a literal string
/// argument — a bareword no longer reaches here, since an undefined constant is
/// an `Error` in PHP 8).
fn has_flag(v: &Value, name: &str, bit: i64) -> bool {
    match v {
        Value::Int(n) => n & bit != 0,
        Value::Str(s) => s.to_ascii_uppercase().contains(name),
        _ => false,
    }
}

// ── stat helpers ────────────────────────────────────────────────────────────

/// PHP's 26-entry `stat`/`lstat` array: the 13 fields under numeric keys `0..12`
/// followed by the same values under their named keys, in PHP's order.
fn stat_array(m: &fs::Metadata) -> Value {
    let fields: [(&str, i64); 13] = [
        ("dev", m.dev() as i64),
        ("ino", m.ino() as i64),
        ("mode", m.mode() as i64),
        ("nlink", m.nlink() as i64),
        ("uid", m.uid() as i64),
        ("gid", m.gid() as i64),
        ("rdev", m.rdev() as i64),
        ("size", m.size() as i64),
        ("atime", m.atime()),
        ("mtime", m.mtime()),
        ("ctime", m.ctime()),
        ("blksize", m.blksize() as i64),
        ("blocks", m.blocks() as i64),
    ];
    let mut pairs: Vec<(Value, Value)> = Vec::with_capacity(26);
    for (i, (_, val)) in fields.iter().enumerate() {
        pairs.push((Value::int(i as i64), Value::int(*val)));
    }
    for (key, val) in fields.iter() {
        pairs.push((Value::str(*key), Value::int(*val)));
    }
    make_map(pairs)
}

/// PHP `filetype` name for a `FileType`. Uses the type as observed (callers pass
/// `symlink_metadata`, so a symlink reports `"link"`, matching PHP's `lstat`).
fn filetype_str(ft: fs::FileType) -> &'static str {
    if ft.is_dir() {
        "dir"
    } else if ft.is_file() {
        "file"
    } else if ft.is_symlink() {
        "link"
    } else if ft.is_fifo() {
        "fifo"
    } else if ft.is_char_device() {
        "char"
    } else if ft.is_block_device() {
        "block"
    } else if ft.is_socket() {
        "socket"
    } else {
        "unknown"
    }
}

/// `access(2)` check (`R_OK`/`W_OK`/`X_OK`) — the honest "can the current user do
/// this" test PHP's `is_executable` etc. rely on. `false` for a missing path or
/// a path containing an interior NUL.
fn access_ok(path: &str, mode: libc::c_int) -> bool {
    match CString::new(path) {
        Ok(c) => unsafe { libc::access(c.as_ptr(), mode) == 0 },
        Err(_) => false,
    }
}

/// `statvfs(2)` of `path`, or `None` on failure.
fn statvfs_of(path: &str) -> Option<libc::statvfs> {
    let c = CString::new(path).ok()?;
    // SAFETY: `buf` is fully written by `statvfs` on success (rc == 0); we only
    // read it in that case.
    let mut buf: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut buf) };
    if rc == 0 {
        Some(buf)
    } else {
        None
    }
}

/// Fundamental block size for disk-space math: `f_frsize`, falling back to
/// `f_bsize` when the kernel reports `f_frsize == 0`.
fn frsize(s: &libc::statvfs) -> f64 {
    if s.f_frsize != 0 {
        s.f_frsize as f64
    } else {
        s.f_bsize as f64
    }
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
        // Executable per `access(X_OK)` for the current user (false if missing).
        "is_executable" => Value::bool(access_ok(&str_arg(args, 0), libc::X_OK)),
        // A symlink? `lstat`-based, so it does not follow the link.
        "is_link" => Value::bool(
            fs::symlink_metadata(str_arg(args, 0))
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false),
        ),
        "filetype" => match fs::symlink_metadata(str_arg(args, 0)) {
            Ok(m) => Value::str(filetype_str(m.file_type())),
            Err(_) => f(),
        },

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
        // Single-level shell glob: the last path component is the wildcard, the
        // rest is a literal directory. Dotfiles are excluded unless the pattern
        // itself starts with a dot (matching libc glob / PHP). No match → empty
        // array (PHP only returns false on a read error, rare here).
        "glob" => {
            let pattern = str_arg(args, 0);
            let flags = arg(args, 1);
            let only_dir = has_flag(&flags, "ONLYDIR", GLOB_ONLYDIR);
            let mark = has_flag(&flags, "MARK", GLOB_MARK);
            let no_sort = has_flag(&flags, "NOSORT", GLOB_NOSORT);
            let (dir_read, prefix, name_pat) = match pattern.rfind('/') {
                Some(i) => {
                    let dir = &pattern[..i];
                    let read = if dir.is_empty() { "/" } else { dir };
                    (
                        read.to_string(),
                        pattern[..=i].to_string(),
                        pattern[i + 1..].to_string(),
                    )
                }
                None => (".".to_string(), String::new(), pattern.clone()),
            };
            let pat_dot = name_pat.starts_with('.');
            let mut out: Vec<String> = Vec::new();
            if let Ok(rd) = fs::read_dir(&dir_read) {
                for entry in rd.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if !pat_dot && name.starts_with('.') {
                        continue;
                    }
                    if !wildcard_match(&name_pat, &name, false) {
                        continue;
                    }
                    let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    if only_dir && !is_dir {
                        continue;
                    }
                    let mut full = format!("{prefix}{name}");
                    if mark && is_dir {
                        full.push('/');
                    }
                    out.push(full);
                }
            }
            if !no_sort {
                out.sort();
            }
            make_list(out.into_iter().map(Value::str).collect())
        }
        // fnmatch(pattern, string, flags): FNM_CASEFOLD (0x10) honored; the other
        // flags do not change single-string matching here.
        "fnmatch" => {
            let pattern = str_arg(args, 0);
            let subject = str_arg(args, 1);
            let casefold = has_flag(&arg(args, 2), "CASEFOLD", FNM_CASEFOLD);
            Value::bool(wildcard_match(&pattern, &subject, casefold))
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
        // Full `st_mode` (permission bits AND file-type bits), matching PHP.
        "fileperms" => match fs::metadata(str_arg(args, 0)) {
            Ok(m) => Value::int(m.mode() as i64),
            Err(_) => f(),
        },
        // `stat` follows symlinks; `lstat` reports on the link itself.
        "stat" => match fs::metadata(str_arg(args, 0)) {
            Ok(m) => stat_array(&m),
            Err(_) => f(),
        },
        "lstat" => match fs::symlink_metadata(str_arg(args, 0)) {
            Ok(m) => stat_array(&m),
            Err(_) => f(),
        },
        // Bytes available to an unprivileged process (`f_bavail`).
        "disk_free_space" | "diskfreespace" => match statvfs_of(&str_arg(args, 0)) {
            Some(s) => Value::float(s.f_bavail as f64 * frsize(&s)),
            None => f(),
        },
        "disk_total_space" => match statvfs_of(&str_arg(args, 0)) {
            Some(s) => Value::float(s.f_blocks as f64 * frsize(&s)),
            None => f(),
        },
        // No stat cache exists here, so invalidation is a no-op returning null.
        "clearstatcache" => Value::Undef,

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
            let levels = if provided(args, 1) {
                int_arg(args, 1)
            } else {
                1
            };
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
        // Create a unique file in `dir` (falling back to the system temp dir when
        // `dir` is not a usable directory, as PHP does) and return its path, or
        // false on failure. The file is created (0600 by `create_new`) so callers
        // can write to it immediately, matching PHP's `tempnam`.
        "tempnam" => {
            let dir = str_arg(args, 0);
            let base_dir = if Path::new(&dir).is_dir() {
                dir
            } else {
                std::env::temp_dir().to_string_lossy().into_owned()
            };
            // PHP uses only the basename of the prefix and caps its length.
            let prefix = php_basename(&str_arg(args, 1), "");
            let prefix: String = prefix.chars().take(60).collect();
            let pid = std::process::id();
            let mut result = f();
            for attempt in 0u128..10_000 {
                let nanos = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
                    .wrapping_add(attempt);
                let candidate = Path::new(&base_dir).join(format!("{prefix}{pid:x}{nanos:x}"));
                match fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&candidate)
                {
                    Ok(_) => {
                        result = Value::str(candidate.to_string_lossy().into_owned());
                        break;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(_) => break,
                }
            }
            result
        }

        _ => return None,
    };
    Some(Ok(v))
}
