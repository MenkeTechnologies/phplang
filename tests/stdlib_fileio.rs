//! End-to-end tests for the `fileio` stdlib category. Filesystem tests operate
//! under a unique per-test directory beneath `std::env::temp_dir()` and remove
//! it on the way out, so the suite is safe in a headless CI (no network, no
//! shared fixtures). PHP source embeds the temp paths as single-quoted string
//! literals (temp paths never contain a single quote).

use phplang::eval_capture;
use std::path::{Path, PathBuf};

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

/// A unique, freshly-created directory for one test; caller removes it.
fn unique_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut p = std::env::temp_dir();
    p.push(format!("phplang_fileio_{tag}_{}_{}", std::process::id(), nanos));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Path (as a string) of `name` inside `dir`, for embedding in PHP source.
fn child(dir: &Path, name: &str) -> String {
    dir.join(name).to_string_lossy().into_owned()
}

// ── path string operations (no filesystem) ─────────────────────────────────

#[test]
fn basename_plain_and_suffix() {
    assert_eq!(run("<?php echo basename('/foo/bar/baz.txt');"), "baz.txt");
    assert_eq!(run("<?php echo basename('/foo/bar/baz.txt', '.txt');"), "baz");
    assert_eq!(run("<?php echo basename('/foo/bar/');"), "bar");
    assert_eq!(run("<?php echo basename('solo');"), "solo");
}

#[test]
fn dirname_levels() {
    assert_eq!(run("<?php echo dirname('/foo/bar/baz.txt');"), "/foo/bar");
    assert_eq!(run("<?php echo dirname('/foo');"), "/");
    assert_eq!(run("<?php echo dirname('solo');"), ".");
    assert_eq!(run("<?php echo dirname('/a/b/c/d', 2);"), "/a/b");
}

#[test]
fn pathinfo_full_array() {
    // Keys and their values, rendered in insertion order.
    assert_eq!(
        run("<?php $p = pathinfo('/x/y/file.tar.gz'); echo $p['dirname'], '|', $p['basename'], '|', $p['extension'], '|', $p['filename'];"),
        "/x/y|file.tar.gz|gz|file.tar"
    );
}

#[test]
fn pathinfo_no_extension_omits_key() {
    // No extension → no 'extension' key (isset is false); filename == basename.
    assert_eq!(
        run("<?php $p = pathinfo('/x/README'); echo isset($p['extension']) ? 'yes' : 'no', ':', $p['filename'];"),
        "no:README"
    );
}

#[test]
fn pathinfo_single_component_constant() {
    // A bareword PATHINFO_* constant reaches the function as its name string.
    assert_eq!(
        run("<?php echo pathinfo('/x/y/a.php', PATHINFO_EXTENSION);"),
        "php"
    );
    assert_eq!(
        run("<?php echo pathinfo('/x/y/a.php', PATHINFO_FILENAME);"),
        "a"
    );
}

// ── environment ────────────────────────────────────────────────────────────

#[test]
fn sys_get_temp_dir_no_trailing_slash() {
    let out = run("<?php $d = sys_get_temp_dir(); echo $d === '' ? 'empty' : (substr($d, -1) === '/' ? 'slash' : 'ok');");
    assert_eq!(out, "ok");
}

#[test]
fn getcwd_returns_a_path() {
    let out = run("<?php echo getcwd() === false ? 'false' : 'ok';");
    assert_eq!(out, "ok");
}

// ── whole-file read/write round-trip ───────────────────────────────────────

#[test]
fn put_then_get_contents_round_trip() {
    let dir = unique_dir("roundtrip");
    let file = child(&dir, "data.txt");
    let src = format!(
        "<?php $n = file_put_contents('{file}', 'hello world'); echo $n, ':', file_get_contents('{file}');"
    );
    assert_eq!(run(&src), "11:hello world");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn file_put_contents_append_flag_int_and_name() {
    let dir = unique_dir("append");
    let file = child(&dir, "log.txt");
    // First write, then append with the integer flag 8, then with the bareword.
    let src = format!(
        "<?php file_put_contents('{file}', 'a'); file_put_contents('{file}', 'b', 8); file_put_contents('{file}', 'c', FILE_APPEND); echo file_get_contents('{file}');"
    );
    assert_eq!(run(&src), "abc");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn file_put_contents_array_data_is_concatenated() {
    let dir = unique_dir("arrdata");
    let file = child(&dir, "joined.txt");
    let src = format!(
        "<?php $n = file_put_contents('{file}', ['x', 'y', 'z']); echo $n, ':', file_get_contents('{file}');"
    );
    assert_eq!(run(&src), "3:xyz");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn file_get_contents_missing_returns_false() {
    let dir = unique_dir("missing");
    let file = child(&dir, "nope.txt");
    let src = format!("<?php var_dump(file_get_contents('{file}'));");
    assert_eq!(run(&src), "bool(false)\n");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn file_lines_keep_newlines() {
    let dir = unique_dir("lines");
    let file = child(&dir, "multi.txt");
    let src = format!(
        "<?php file_put_contents('{file}', \"a\\nb\\nc\"); $lines = file('{file}'); echo count($lines), ':', $lines[0], '|', $lines[2];"
    );
    // "a\n", "b\n", "c" — first line keeps its newline, last has none.
    assert_eq!(run(&src), "3:a\n|c");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn readfile_echoes_and_returns_bytes() {
    let dir = unique_dir("readfile");
    let file = child(&dir, "r.txt");
    let src = format!(
        "<?php file_put_contents('{file}', 'echoed'); $n = readfile('{file}'); echo ':', $n;"
    );
    assert_eq!(run(&src), "echoed:6");
    std::fs::remove_dir_all(&dir).unwrap();
}

// ── existence / type / permission ──────────────────────────────────────────

#[test]
fn exists_is_file_is_dir() {
    let dir = unique_dir("types");
    let file = child(&dir, "f.txt");
    let dirs = dir.to_string_lossy().into_owned();
    let src = format!(
        "<?php file_put_contents('{file}', 'x');
         echo file_exists('{file}') ? '1' : '0';
         echo is_file('{file}') ? '1' : '0';
         echo is_dir('{file}') ? '1' : '0';
         echo is_dir('{dirs}') ? '1' : '0';
         echo file_exists('{dirs}/absent') ? '1' : '0';"
    );
    // file_exists=1, is_file=1, is_dir(file)=0, is_dir(dir)=1, exists(absent)=0.
    assert_eq!(run(&src), "11010");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn is_readable_and_writable() {
    let dir = unique_dir("perm");
    let file = child(&dir, "rw.txt");
    let src = format!(
        "<?php file_put_contents('{file}', 'x'); echo is_readable('{file}') ? '1' : '0'; echo is_writable('{file}') ? '1' : '0';"
    );
    assert_eq!(run(&src), "11");
    std::fs::remove_dir_all(&dir).unwrap();
}

// ── mutating operations ────────────────────────────────────────────────────

#[test]
fn unlink_removes_file() {
    let dir = unique_dir("unlink");
    let file = child(&dir, "gone.txt");
    let src = format!(
        "<?php file_put_contents('{file}', 'x'); $ok = unlink('{file}'); echo $ok ? '1' : '0'; echo file_exists('{file}') ? '1' : '0';"
    );
    assert_eq!(run(&src), "10");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn mkdir_recursive_and_rmdir() {
    let dir = unique_dir("mkdir");
    let nested = child(&dir, "a/b/c");
    let leaf = child(&dir, "a/b/c");
    let src = format!(
        "<?php $ok = mkdir('{nested}', 0777, true); echo $ok ? '1' : '0'; echo is_dir('{leaf}') ? '1' : '0'; echo rmdir('{leaf}') ? '1' : '0'; echo is_dir('{leaf}') ? '1' : '0';"
    );
    // mkdir=1, is_dir(leaf)=1, rmdir=1, is_dir after rmdir=0.
    assert_eq!(run(&src), "1110");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn mkdir_non_recursive_missing_parent_fails() {
    let dir = unique_dir("mkdirfail");
    let nested = child(&dir, "missing/child");
    let src = format!("<?php echo mkdir('{nested}') ? '1' : '0';");
    assert_eq!(run(&src), "0");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn rename_and_copy() {
    let dir = unique_dir("movecopy");
    let a = child(&dir, "a.txt");
    let b = child(&dir, "b.txt");
    let c = child(&dir, "c.txt");
    let src = format!(
        "<?php file_put_contents('{a}', 'data');
         echo rename('{a}', '{b}') ? '1' : '0';
         echo file_exists('{a}') ? '1' : '0';
         echo copy('{b}', '{c}') ? '1' : '0';
         echo file_get_contents('{c}');"
    );
    assert_eq!(run(&src), "101data");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn touch_creates_file() {
    let dir = unique_dir("touch");
    let file = child(&dir, "t.txt");
    let src = format!(
        "<?php echo touch('{file}') ? '1' : '0'; echo file_exists('{file}') ? '1' : '0'; echo filesize('{file}');"
    );
    assert_eq!(run(&src), "110");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn touch_sets_explicit_mtime() {
    let dir = unique_dir("touchtime");
    let file = child(&dir, "stamped.txt");
    let src = format!(
        "<?php touch('{file}', 1000000000); echo filemtime('{file}');"
    );
    assert_eq!(run(&src), "1000000000");
    std::fs::remove_dir_all(&dir).unwrap();
}

// ── stat ───────────────────────────────────────────────────────────────────

#[test]
fn filesize_reports_byte_count() {
    let dir = unique_dir("size");
    let file = child(&dir, "s.txt");
    let src = format!("<?php file_put_contents('{file}', 'hello'); echo filesize('{file}');");
    assert_eq!(run(&src), "5");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn filesize_missing_returns_false() {
    let dir = unique_dir("sizemiss");
    let file = child(&dir, "absent.txt");
    let src = format!("<?php var_dump(filesize('{file}'));");
    assert_eq!(run(&src), "bool(false)\n");
    std::fs::remove_dir_all(&dir).unwrap();
}

// ── directory listing ──────────────────────────────────────────────────────

#[test]
fn scandir_sorted_with_dot_entries() {
    let dir = unique_dir("scandir");
    let dirs = dir.to_string_lossy().into_owned();
    let a = child(&dir, "a.txt");
    let b = child(&dir, "b.txt");
    let src = format!(
        "<?php file_put_contents('{b}', 'x'); file_put_contents('{a}', 'x'); echo implode(',', scandir('{dirs}'));"
    );
    // ".", ".." sort first; then a.txt, b.txt ascending.
    assert_eq!(run(&src), ".,..,a.txt,b.txt");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn scandir_descending() {
    let dir = unique_dir("scandirdesc");
    let dirs = dir.to_string_lossy().into_owned();
    let a = child(&dir, "a.txt");
    let b = child(&dir, "b.txt");
    let src = format!(
        "<?php file_put_contents('{a}', 'x'); file_put_contents('{b}', 'x'); echo implode(',', scandir('{dirs}', 1));"
    );
    assert_eq!(run(&src), "b.txt,a.txt,..,.");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn scandir_missing_returns_false() {
    let dir = unique_dir("scandirmiss");
    let missing = child(&dir, "no_such_dir");
    let src = format!("<?php var_dump(scandir('{missing}'));");
    assert_eq!(run(&src), "bool(false)\n");
    std::fs::remove_dir_all(&dir).unwrap();
}

// ── realpath ───────────────────────────────────────────────────────────────

#[test]
fn realpath_of_existing_file_is_truthy() {
    let dir = unique_dir("realpath");
    let file = child(&dir, "real.txt");
    let src = format!(
        "<?php file_put_contents('{file}', 'x'); $r = realpath('{file}'); echo $r === false ? 'false' : (is_string($r) ? 'ok' : 'bad');"
    );
    assert_eq!(run(&src), "ok");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn realpath_missing_returns_false() {
    let dir = unique_dir("realpathmiss");
    let file = child(&dir, "absent.txt");
    let src = format!("<?php var_dump(realpath('{file}'));");
    assert_eq!(run(&src), "bool(false)\n");
    std::fs::remove_dir_all(&dir).unwrap();
}
