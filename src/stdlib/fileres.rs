//! The stream family: `fopen` and the `php://` wrappers (`memory`, `temp`,
//! `stdin`, `stdout`, `stderr`, `output`), `tmpfile()`, the `STDIN`/`STDOUT`/
//! `STDERR` constants, and the functions that read, write and position a
//! stream. Part of the `stdlib` chain; see `src/stdlib/mod.rs`.
//!
//! A stream is a `PhpObj::Resource` holding a [`Stream`]. A plain file's
//! content is buffered in memory (read modes load the file up front) and every
//! write is flushed to disk immediately, so no data is lost without an explicit
//! `fclose`. Behaviour is ported from php-src 8.5 `main/streams/streams.c`
//! (`_php_stream_read`, `_php_stream_get_line`, `_php_stream_eof`),
//! `main/streams/memory.c` (the memory and temp wrappers) and
//! `ext/standard/file.c` (the userland functions).

use crate::host::with_host;
use crate::stdlib::common::*;
use fusevm::Value;

/// What a stream is backed by. It decides where a write lands and whether a
/// read has anything to pull from outside the buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    /// A file on disk, buffered and flushed on every write.
    Plain,
    /// `php://memory`.
    Memory,
    /// `php://temp` — a memory stream that the reference moves to a temporary
    /// file once it reaches its `maxmemory` threshold.
    Temp,
    /// The process's standard input: `STDIN` and `php://stdin`.
    Stdin,
    /// The process's standard output, bypassing output buffering: `STDOUT`
    /// and `php://stdout`.
    Stdout,
    /// `STDERR` and `php://stderr`.
    Stderr,
    /// `php://output` — the same sink as `echo`, output buffering included.
    Output,
}

/// `php://temp`'s default `maxmemory` (`PHP_STREAM_MAX_MEM`, 2 MiB).
const TEMP_MAX_MEMORY: usize = 2 * 1024 * 1024;

/// One open (or closed) stream.
#[derive(Debug, Clone)]
pub struct Stream {
    /// The resource number: `var_dump`'s `resource(N)`, `(int)$h`,
    /// `get_resource_id`. Assigned by the host from a counter that never
    /// reuses a number, as `zend_list_insert` never does.
    pub id: i64,
    pub kind: StreamKind,
    /// The file a `Plain` stream flushes to; empty for every other kind.
    pub path: String,
    pub buf: Vec<u8>,
    pub pos: usize,
    pub readable: bool,
    pub writable: bool,
    /// `a`/`a+` on a plain file: every write lands at the end whatever the
    /// cursor says (`O_APPEND`), while `ftell` still counts from 0.
    pub append: bool,
    /// Set by a read that wanted more than the stream had; cleared by a seek.
    /// This, not "cursor at end", is what `feof` answers.
    pub eof: bool,
    pub dirty: bool,
    pub closed: bool,
    /// `php://temp`: the size at which the reference spills to a file.
    pub spill_at: Option<usize>,
    /// `Stdin`: the process's standard input has reported end of file.
    pub source_done: bool,
}

impl Stream {
    fn new(kind: StreamKind, readable: bool, writable: bool) -> Self {
        Stream {
            id: 0,
            kind,
            path: String::new(),
            buf: Vec::new(),
            pos: 0,
            readable,
            writable,
            append: false,
            eof: false,
            dirty: false,
            closed: false,
            spill_at: None,
            source_done: false,
        }
    }

    /// The standard streams the CLI opens before the script runs.
    pub fn stdio(kind: StreamKind) -> Self {
        Stream::new(kind, kind == StreamKind::Stdin, kind != StreamKind::Stdin)
    }

    /// Read up to `n` bytes. A short read sets `eof`, as `_php_stream_read`
    /// does when the wrapper hands back less than was asked for.
    fn read(&mut self, n: usize) -> Vec<u8> {
        let start = self.pos.min(self.buf.len());
        let end = start.saturating_add(n).min(self.buf.len());
        if end - start < n {
            self.eof = true;
        }
        if end > start {
            self.pos = end;
        }
        self.buf[start..end].to_vec()
    }

    /// Read one line: through the next `\n`, or `max` bytes, whichever comes
    /// first. `None` when nothing is left. Running out of data before either
    /// stop sets `eof`.
    fn gets(&mut self, max: Option<usize>) -> Option<Vec<u8>> {
        if self.pos >= self.buf.len() {
            self.eof = true;
            return None;
        }
        let cap = max.map_or(self.buf.len(), |m| (self.pos + m).min(self.buf.len()));
        let mut end = self.pos;
        while end < cap && self.buf[end] != b'\n' {
            end += 1;
        }
        if end < cap {
            end += 1; // the newline is part of the line
        } else if end == self.buf.len() && max.map_or(true, |m| end - self.pos < m) {
            self.eof = true;
        }
        let line = self.buf[self.pos..end].to_vec();
        self.pos = end;
        Some(line)
    }

    /// Write at the cursor (or at the end, in append mode), padding with NULs
    /// when the cursor was seeked past the end.
    pub fn write_at_cursor(&mut self, bytes: &[u8]) -> usize {
        let at = if self.append {
            self.buf.len()
        } else {
            self.pos
        };
        if at > self.buf.len() {
            self.buf.resize(at, 0);
        }
        let overlap = (self.buf.len() - at).min(bytes.len());
        self.buf[at..at + overlap].copy_from_slice(&bytes[..overlap]);
        self.buf.extend_from_slice(&bytes[overlap..]);
        self.pos += bytes.len();
        self.dirty = true;
        bytes.len()
    }

    /// `fseek`: a target before the start is refused and leaves the cursor
    /// alone; past the end is allowed. Either way a seek forgets `eof`.
    fn seek(&mut self, offset: i64, whence: i64) -> bool {
        let base = match whence {
            1 => self.pos as i64,
            2 => self.buf.len() as i64,
            _ => 0,
        };
        let Some(target) = base.checked_add(offset).filter(|t| *t >= 0) else {
            return false;
        };
        self.pos = target as usize;
        self.eof = false;
        true
    }

    /// `get_resource_type`: `stream` while open, `Unknown` once freed.
    pub fn type_name(&self) -> &'static str {
        if self.closed {
            "Unknown"
        } else {
            "stream"
        }
    }
}

/// Dispatch a stream function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let v = match name {
        "fopen" => {
            let path = str_arg(args, 0);
            if path.is_empty() {
                return Some(Err(throws("ValueError", "Path must not be empty")));
            }
            fopen(&path, &str_arg(args, 1))
        }
        "tmpfile" => with_host(|h| h.new_stream(Stream::new(StreamKind::Memory, true, true))),
        "fread" => {
            let res = arg(args, 0);
            let n = int_arg(args, 1);
            if n <= 0 {
                return Some(Err(throws(
                    "ValueError",
                    "fread(): Argument #2 ($length) must be greater than 0",
                )));
            }
            match read(name, &res, |s| Some(s.read(n as usize))) {
                Some(b) => bytes_value(b),
                None => Value::bool(false),
            }
        }
        "fgets" => {
            let res = arg(args, 0);
            // fgets($h) reads to the next newline; fgets($h, $len) caps at $len-1.
            let max = if matches!(args.get(1), Some(v) if !matches!(v, Value::Undef)) {
                let len = int_arg(args, 1);
                if len <= 0 {
                    return Some(Err(throws(
                        "ValueError",
                        "fgets(): Argument #2 ($length) must be greater than 0",
                    )));
                }
                Some((len as usize).saturating_sub(1))
            } else {
                None
            };
            match read(name, &res, |s| s.gets(max)) {
                Some(b) => bytes_value(b),
                None => Value::bool(false),
            }
        }
        "fgetc" => {
            let res = arg(args, 0);
            match read(name, &res, |s| Some(s.read(1)).filter(|b| !b.is_empty())) {
                Some(b) => bytes_value(b),
                None => Value::bool(false),
            }
        }
        "stream_get_contents" => {
            let res = arg(args, 0);
            let max = match args.get(1) {
                Some(Value::Undef) | None => -1,
                Some(_) => int_arg(args, 1),
            };
            let offset = match args.get(2) {
                Some(Value::Undef) | None => -1,
                Some(_) => int_arg(args, 2),
            };
            if offset >= 0 {
                with_host(|h| h.stream_mut(&res).map(|s| s.seek(offset, 0)));
            }
            let want = if max < 0 { usize::MAX } else { max as usize };
            match read(name, &res, |s| Some(s.read(want))) {
                Some(b) => bytes_value(b),
                None => Value::str(String::new()),
            }
        }
        "fpassthru" => {
            let res = arg(args, 0);
            match read(name, &res, |s| Some(s.read(usize::MAX))) {
                Some(b) => {
                    let n = b.len();
                    with_host(|h| h.write_out(&String::from_utf8_lossy(&b)));
                    Value::int(n as i64)
                }
                None => Value::int(0),
            }
        }
        "fwrite" | "fputs" => {
            let res = arg(args, 0);
            let mut data = str_arg(args, 1).into_bytes();
            if matches!(args.get(2), Some(v) if !matches!(v, Value::Undef)) {
                let len = int_arg(args, 2).max(0) as usize;
                data.truncate(len);
            }
            match write_bytes(name, &res, &data) {
                Some(n) => Value::int(n as i64),
                None => Value::bool(false),
            }
        }
        "fclose" => {
            let res = arg(args, 0);
            flush(&res);
            Value::bool(with_host(|h| {
                h.stream_mut(&res).is_some_and(|s| {
                    s.closed = true;
                    true
                })
            }))
        }
        "fflush" => {
            let res = arg(args, 0);
            flush(&res);
            Value::bool(true)
        }
        "feof" => {
            let res = arg(args, 0);
            Value::bool(with_host(|h| h.stream(&res).map_or(true, |s| s.eof)))
        }
        "ftell" => {
            let res = arg(args, 0);
            with_host(|h| match h.stream(&res) {
                Some(s) => Value::int(s.pos as i64),
                None => Value::bool(false),
            })
        }
        "rewind" => {
            let res = arg(args, 0);
            Value::bool(with_host(|h| {
                h.stream_mut(&res).is_some_and(|s| s.seek(0, 0))
            }))
        }
        "fseek" => {
            let res = arg(args, 0);
            let offset = int_arg(args, 1);
            let whence = if args.len() > 2 { int_arg(args, 2) } else { 0 };
            // fseek returns 0 on success, -1 on failure.
            let ok = with_host(|h| h.stream_mut(&res).is_some_and(|s| s.seek(offset, whence)));
            Value::int(if ok { 0 } else { -1 })
        }
        "ftruncate" => {
            let res = arg(args, 0);
            let size = int_arg(args, 1);
            if size < 0 {
                return Some(Err(throws(
                    "ValueError",
                    "ftruncate(): Argument #2 ($size) must be greater than or equal to 0",
                )));
            }
            let ok = with_host(|h| {
                h.stream_mut(&res).is_some_and(|s| {
                    if !s.writable {
                        return false;
                    }
                    s.buf.resize(size as usize, 0);
                    s.dirty = true;
                    true
                })
            });
            flush(&res);
            Value::bool(ok)
        }
        "fstat" => fstat(&arg(args, 0)),
        "is_resource" => Value::bool(with_host(|h| h.is_resource(&arg(args, 0)))),
        "get_resource_type" => with_host(|h| match h.stream_any(&arg(args, 0)) {
            Some(s) => Value::str(s.type_name()),
            None => Value::bool(false),
        }),
        "get_resource_id" => with_host(|h| match h.stream_any(&arg(args, 0)) {
            Some(s) => Value::int(s.id),
            None => Value::int(0),
        }),
        _ => return None,
    };
    Some(Ok(v))
}

/// A byte buffer read off a stream, as a PHP string.
fn bytes_value(b: Vec<u8>) -> Value {
    Value::str(String::from_utf8_lossy(&b).into_owned())
}

/// The reference's wording for an I/O failure: `strerror(errno)`, without the
/// `(os error N)` suffix Rust appends.
fn strerror(e: &std::io::Error) -> String {
    let s = e.to_string();
    match s.find(" (os error") {
        Some(i) => s[..i].to_string(),
        None => s,
    }
}

/// Run a read against an open stream, first pulling from the process's
/// standard input when the stream is `STDIN`. A stream opened write-only
/// reports the plain wrapper's `EBADF` notice and yields `None`, which every
/// caller turns into its own failure value.
fn read(
    fname: &str,
    res: &Value,
    op: impl FnOnce(&mut Stream) -> Option<Vec<u8>>,
) -> Option<Vec<u8>> {
    let (kind, readable) = with_host(|h| h.stream(res).map(|s| (s.kind, s.readable)))?;
    if !readable {
        with_host(|h| {
            h.notice(format!(
                "{fname}(): Read of 8192 bytes failed with errno=9 Bad file descriptor"
            ))
        });
        return None;
    }
    if kind == StreamKind::Stdin {
        fill_stdin(fname, res);
    }
    with_host(|h| h.stream_mut(res).and_then(op))
}

/// Top up `STDIN`'s buffer from the process: a line for `fgets`/`fscanf`,
/// everything for a whole-stream read, one chunk for anything else. Reading
/// lazily keeps an interactive prompt-and-read script working.
fn fill_stdin(fname: &str, res: &Value) {
    use std::io::{BufRead, Read};
    let pending = with_host(|h| {
        h.stream(res).map(|s| {
            (
                s.source_done,
                s.buf[s.pos.min(s.buf.len())..].contains(&b'\n'),
            )
        })
    });
    let Some((false, has_line)) = pending else {
        return;
    };
    let mut chunk = Vec::new();
    let stdin = std::io::stdin();
    let mut lock = stdin.lock();
    let got = match fname {
        "fgets" | "fscanf" if has_line => return,
        "fgets" | "fscanf" => lock.read_until(b'\n', &mut chunk),
        "stream_get_contents" | "fpassthru" | "file_get_contents" | "file" | "readfile" => {
            lock.read_to_end(&mut chunk)
        }
        _ => {
            chunk.resize(8192, 0);
            let n = lock.read(&mut chunk);
            chunk.truncate(*n.as_ref().unwrap_or(&0));
            n
        }
    };
    with_host(|h| {
        if let Some(s) = h.stream_mut(res) {
            if !matches!(got, Ok(n) if n > 0) {
                s.source_done = true;
            }
            s.buf.extend_from_slice(&chunk);
        }
    });
}

/// Write to an open stream, routing the standard streams to the process and
/// `php://output` through `echo`'s sink. `None` when the stream refuses the
/// write; a read-only plain file says why first.
pub fn write_bytes(fname: &str, res: &Value, data: &[u8]) -> Option<usize> {
    let (kind, writable) = with_host(|h| h.stream(res).map(|s| (s.kind, s.writable)))?;
    if !writable {
        if kind == StreamKind::Plain {
            with_host(|h| {
                h.notice(format!(
                    "{fname}(): Write of {} bytes failed with errno=9 Bad file descriptor",
                    data.len()
                ))
            });
        }
        return None;
    }
    match kind {
        StreamKind::Stdout => with_host(|h| h.write_stdout_direct(&String::from_utf8_lossy(data))),
        StreamKind::Output => with_host(|h| h.write_out(&String::from_utf8_lossy(data))),
        StreamKind::Stderr => {
            use std::io::Write;
            let _ = std::io::stderr().write_all(data);
        }
        _ => {
            with_host(|h| h.stream_write(res, data));
            flush(res);
        }
    }
    Some(data.len())
}

/// `fopen($path, $mode)`.
fn fopen(path: &str, mode: &str) -> Value {
    if let Some(rest) = path.strip_prefix("php://") {
        return open_php_wrapper("fopen", path, rest, mode);
    }
    let m = mode.replace(['b', 't'], "");
    let plus = m.contains('+');
    let first = m.chars().next().unwrap_or('\0');
    if !matches!(first, 'r' | 'w' | 'a' | 'x' | 'c') {
        with_host(|h| {
            h.warn(format!(
                "fopen({path}): Failed to open stream: `{mode}' is not a valid mode for fopen"
            ))
        });
        return Value::bool(false);
    }
    let fail = |e: std::io::Error| {
        with_host(|h| {
            h.warn(format!(
                "fopen({path}): Failed to open stream: {}",
                strerror(&e)
            ))
        });
        Value::bool(false)
    };
    let p = std::path::Path::new(path);
    let mut s = Stream::new(
        StreamKind::Plain,
        first == 'r' || plus,
        first != 'r' || plus,
    );
    s.path = path.to_string();
    match first {
        'r' => {
            if p.is_dir() {
                // A directory opens for reading and simply has nothing to read.
            } else {
                match std::fs::read(path) {
                    Ok(buf) => s.buf = buf,
                    Err(e) => return fail(e),
                }
            }
        }
        _ => {
            if p.is_dir() {
                return fail(std::io::Error::from_raw_os_error(21));
            }
            if first == 'x' && p.exists() {
                return fail(std::io::Error::from_raw_os_error(17));
            }
            // 'w' truncates; 'a' and 'c' keep what is there.
            s.buf = if first == 'w' {
                Vec::new()
            } else {
                std::fs::read(path).unwrap_or_default()
            };
            s.append = first == 'a';
            // Materialize the file now so it exists (truncated, for 'w') on open.
            let made = if first == 'w' || !p.exists() {
                std::fs::write(path, b"")
            } else {
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(path)
                    .map(|_| ())
            };
            if let Err(e) = made {
                return fail(e);
            }
        }
    }
    with_host(|h| h.new_stream(s))
}

/// `fopen("php://…")`: the wrappers `php_stream_url_wrap_php` recognises.
fn open_php_wrapper(fname: &str, url: &str, rest: &str, mode: &str) -> Value {
    let lower = rest.to_ascii_lowercase();
    // `php://memory` and `php://temp` are read-only only in a pure `r` mode.
    let read_only = !mode.contains(['+', 'w', 'a', 'x', 'c']);
    let s = match lower.as_str() {
        "stdin" => Stream::stdio(StreamKind::Stdin),
        "stdout" => Stream::stdio(StreamKind::Stdout),
        "stderr" => Stream::stdio(StreamKind::Stderr),
        "output" => Stream::stdio(StreamKind::Output),
        "memory" => Stream::new(StreamKind::Memory, true, !read_only),
        _ if lower == "temp" || lower.starts_with("temp/maxmemory:") => {
            let mut s = Stream::new(StreamKind::Temp, true, !read_only);
            s.spill_at = Some(match lower.strip_prefix("temp/maxmemory:") {
                Some(n) => n.parse().unwrap_or(0),
                None => TEMP_MAX_MEMORY,
            });
            s
        }
        _ => {
            with_host(|h| {
                h.warn(format!("{fname}(): Invalid php:// URL specified"));
                h.warn(format!(
                    "{fname}({url}): Failed to open stream: operation failed"
                ));
            });
            return Value::bool(false);
        }
    };
    // `TEMP_STREAM_APPEND`: an `a` mode sends every write to the end.
    let mut s = s;
    s.append = matches!(s.kind, StreamKind::Memory | StreamKind::Temp) && mode.starts_with('a');
    with_host(|h| h.new_stream(s))
}

/// Flush a plain file's buffered content to disk if it is dirty.
pub fn flush(res: &Value) {
    let data = with_host(|h| {
        h.stream_mut(res).and_then(|s| {
            if s.kind == StreamKind::Plain && s.dirty && !s.closed {
                s.dirty = false;
                Some((s.path.clone(), s.buf.clone()))
            } else {
                None
            }
        })
    });
    if let Some((path, buf)) = data {
        let _ = std::fs::write(path, buf);
    }
}

/// One line off an open stream, for `fscanf`.
pub fn read_line(fname: &str, res: &Value) -> Option<String> {
    read(fname, res, |s| s.gets(None)).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// `fstat($stream)`. A plain file and a standard stream report the real
/// `stat(2)` of what they are attached to (a plain file's size is its buffer,
/// which every write has already flushed). A memory or temp stream reports what
/// `php_stream_memory_stat` fabricates: device `0xC`, one link, mode `0100666`
/// (`0100444` read-only), and `-1` where there is no answer.
fn fstat(res: &Value) -> Value {
    use crate::stdlib::fileio::{stat_fields, stat_value};
    let Some((kind, path, size, writable)) = with_host(|h| {
        h.stream(res)
            .map(|s| (s.kind, s.path.clone(), s.buf.len() as i64, s.writable))
    }) else {
        return Value::bool(false);
    };
    let real = |p: &str| match std::fs::metadata(p) {
        Ok(m) => stat_value(&stat_fields(&m)),
        Err(_) => Value::bool(false),
    };
    match kind {
        StreamKind::Plain => match std::fs::metadata(&path) {
            Ok(m) => {
                let mut fields = stat_fields(&m);
                fields[7].1 = size;
                stat_value(&fields)
            }
            Err(_) => Value::bool(false),
        },
        StreamKind::Stdin => real("/dev/stdin"),
        StreamKind::Stdout => real("/dev/stdout"),
        StreamKind::Stderr => real("/dev/stderr"),
        StreamKind::Output => Value::bool(false),
        StreamKind::Memory | StreamKind::Temp => {
            let mode = 0o100000 | if writable { 0o666 } else { 0o444 };
            stat_value(&[
                ("dev", 0xC),
                ("ino", 0),
                ("mode", mode),
                ("nlink", 1),
                ("uid", 0),
                ("gid", 0),
                ("rdev", -1),
                ("size", size),
                ("atime", 0),
                ("mtime", 0),
                ("ctime", 0),
                ("blksize", -1),
                ("blocks", -1),
            ])
        }
    }
}

/// Whether a path names a `php://` wrapper rather than a file.
pub fn is_php_url(path: &str) -> bool {
    path.len() > 6 && path[..6].eq_ignore_ascii_case("php://")
}

/// The whole-stream readers (`file_get_contents`, `file`, `readfile`) on a
/// `php://` URL: open it (spending a resource number, as the reference does),
/// read everything, close it. `None` after the open failed and warned.
pub fn read_url(fname: &str, url: &str) -> Option<Vec<u8>> {
    let res = open_php_wrapper(fname, url, &url[6..], "rb");
    if matches!(res, Value::Bool(false)) {
        return None;
    }
    let data = read(fname, &res, |s| Some(s.read(usize::MAX)));
    with_host(|h| h.stream_mut(&res).map(|s| s.closed = true));
    Some(data.unwrap_or_default())
}

/// `file_put_contents` on a `php://` URL: the byte count written, or `None`
/// after the open failed or the stream refused the write.
pub fn write_url(fname: &str, url: &str, data: &[u8]) -> Option<usize> {
    let res = open_php_wrapper(fname, url, &url[6..], "wb");
    if matches!(res, Value::Bool(false)) {
        return None;
    }
    let n = write_bytes(fname, &res, data);
    with_host(|h| h.stream_mut(&res).map(|s| s.closed = true));
    n
}

/// A whole-file function opened a plain file: the reference spent a resource
/// number on the stream it used, so spend one here too.
pub fn note_file_opened() {
    with_host(|h| h.consume_resource_id());
}

/// A whole-file function could not open `path`: the reference's warning.
pub fn warn_open_failed(fname: &str, path: &str, e: &std::io::Error) {
    with_host(|h| {
        h.warn(format!(
            "{fname}({path}): Failed to open stream: {}",
            strerror(e)
        ))
    });
}
