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
    p.push(format!(
        "phplang_fileio_{tag}_{}_{}",
        std::process::id(),
        nanos
    ));
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
    assert_eq!(
        run("<?php echo basename('/foo/bar/baz.txt', '.txt');"),
        "baz"
    );
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
    // Pinned against an explicitly-set TMPDIR rather than the ambient one, which
    // made the outcome depend on the machine: a TMPDIR already ending in `/`
    // failed the test, and one that did not made it pass without exercising the
    // stripping at all.
    //
    // The reference strips exactly ONE trailing separator:
    //
    // ```text
    // $ TMPDIR=/tmp/   php -r 'var_dump(sys_get_temp_dir());'  => string(4) "/tmp"
    // $ TMPDIR=/tmp/// php -r 'var_dump(sys_get_temp_dir());'  => string(6) "/tmp//"
    // ```
    for (tmpdir, expected) in [("/tmp", "/tmp"), ("/tmp/", "/tmp"), ("/tmp///", "/tmp//")] {
        // SAFETY: single-threaded test process.
        unsafe { std::env::set_var("TMPDIR", tmpdir) };
        assert_eq!(
            run("<?php echo sys_get_temp_dir();"),
            expected,
            "TMPDIR={tmpdir:?}"
        );
    }
    unsafe { std::env::remove_var("TMPDIR") };
}

#[test]
fn getcwd_returns_a_path() {
    // Compared against the directory the test process is actually in, not merely
    // tested for `!== false` — the old form accepted the empty string, which is
    // not a path.
    let expected = std::env::current_dir()
        .expect("cwd")
        .to_string_lossy()
        .into_owned();
    let out = run("<?php $d = getcwd(); echo $d === false ? 'FALSE' : $d;");
    assert_eq!(out, expected);
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
    let src = format!("<?php touch('{file}', 1000000000); echo filemtime('{file}');");
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

// ── pathinfo leading-dot (HARDEN: PHP treats `.htaccess` as extension) ───────

#[test]
fn pathinfo_leading_dot_is_extension() {
    // PHP: pathinfo('/x/.htaccess') → extension "htaccess", filename "".
    assert_eq!(
        run("<?php $p = pathinfo('/x/.htaccess'); echo $p['basename'], '|', $p['extension'], '|', $p['filename'], '|', strlen($p['filename']);"),
        ".htaccess|htaccess||0"
    );
    // Single-component selectors agree with the array form.
    assert_eq!(
        run("<?php echo pathinfo('.bashrc', PATHINFO_EXTENSION), '/', pathinfo('.bashrc', PATHINFO_FILENAME);"),
        "bashrc/"
    );
}

// ── glob ────────────────────────────────────────────────────────────────────

#[test]
fn glob_star_matches_and_sorts() {
    let dir = unique_dir("glob");
    let dirs = dir.to_string_lossy().into_owned();
    let a = child(&dir, "a.txt");
    let b = child(&dir, "b.txt");
    let c = child(&dir, "c.md");
    let src = format!(
        "<?php file_put_contents('{b}', 'x'); file_put_contents('{a}', 'x'); file_put_contents('{c}', 'x');
         $g = glob('{dirs}/*.txt');
         echo count($g), ':', basename($g[0]), ',', basename($g[1]);"
    );
    // Only the two .txt files, sorted ascending by full path.
    assert_eq!(run(&src), "2:a.txt,b.txt");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn glob_question_and_bracket() {
    let dir = unique_dir("globq");
    let dirs = dir.to_string_lossy().into_owned();
    for name in ["a1", "a2", "b1", "zz"] {
        std::fs::write(dir.join(name), "x").unwrap();
    }
    // `a?` matches a1,a2; `[ab]1` matches a1,b1.
    let src = format!("<?php echo count(glob('{dirs}/a?')), ':', count(glob('{dirs}/[ab]1'));");
    assert_eq!(run(&src), "2:2");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn glob_only_dir_flag() {
    let dir = unique_dir("globdir");
    let dirs = dir.to_string_lossy().into_owned();
    std::fs::write(dir.join("file.txt"), "x").unwrap();
    std::fs::create_dir(dir.join("subdir")).unwrap();
    let src =
        format!("<?php $g = glob('{dirs}/*', GLOB_ONLYDIR); echo count($g), ':', basename($g[0]);");
    assert_eq!(run(&src), "1:subdir");
    // By VALUE, not by name. `GLOB_ONLYDIR` used to be undefined: the bareword
    // resolved to the string "GLOB_ONLYDIR" and was matched by substring, so the
    // bit the matcher tested was never exercised — and it was `0x2000` (glibc's)
    // while the reference on this platform reports 1073741824.
    let by_value =
        format!("<?php $g = glob('{dirs}/*', 1073741824); echo count($g), ':', basename($g[0]);");
    assert_eq!(run(&by_value), "1:subdir");
    // No flags: both entries come back, so the filter is really the flag's doing.
    let no_flag = format!("<?php echo count(glob('{dirs}/*'));");
    assert_eq!(run(&no_flag), "2");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn glob_excludes_dotfiles_unless_pattern_starts_with_dot() {
    let dir = unique_dir("globdot");
    let dirs = dir.to_string_lossy().into_owned();
    std::fs::write(dir.join(".hidden"), "x").unwrap();
    std::fs::write(dir.join("shown.txt"), "x").unwrap();
    // `*` skips the dotfile; `.*` includes it (and never `.`/`..`).
    let src = format!("<?php echo count(glob('{dirs}/*')), ':', count(glob('{dirs}/.*'));");
    assert_eq!(run(&src), "1:1");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn glob_no_match_is_empty_array() {
    let dir = unique_dir("globempty");
    let dirs = dir.to_string_lossy().into_owned();
    let src = format!(
        "<?php $g = glob('{dirs}/*.nope'); echo is_array($g) ? 'arr' : 'no', ':', count($g);"
    );
    assert_eq!(run(&src), "arr:0");
    std::fs::remove_dir_all(&dir).unwrap();
}

// ── fnmatch ─────────────────────────────────────────────────────────────────

#[test]
fn fnmatch_wildcards() {
    assert_eq!(
        run("<?php echo fnmatch('*.txt', 'foo.txt') ? '1' : '0';"),
        "1"
    );
    assert_eq!(
        run("<?php echo fnmatch('*.txt', 'foo.md') ? '1' : '0';"),
        "0"
    );
    assert_eq!(run("<?php echo fnmatch('f?o', 'foo') ? '1' : '0';"), "1");
    assert_eq!(run("<?php echo fnmatch('f?o', 'fooo') ? '1' : '0';"), "0");
    assert_eq!(
        run("<?php echo fnmatch('[a-c]at', 'bat') ? '1' : '0';"),
        "1"
    );
    assert_eq!(
        run("<?php echo fnmatch('[!a-c]at', 'bat') ? '1' : '0';"),
        "0"
    );
    assert_eq!(
        run("<?php echo fnmatch('[!a-c]at', 'rat') ? '1' : '0';"),
        "1"
    );
}

#[test]
fn fnmatch_casefold_flag() {
    // `FNM_CASEFOLD` is a seeded constant (16, as the reference reports) and
    // folds ASCII case. It used to resolve to the STRING "FNM_CASEFOLD" under
    // the pre-PHP-8 bareword fallback and was matched by name, so this passed
    // without the flag's value ever being read.
    assert_eq!(run("<?php echo fnmatch('FOO', 'foo') ? '1' : '0';"), "0");
    assert_eq!(
        run("<?php echo fnmatch('FOO', 'foo', FNM_CASEFOLD) ? '1' : '0';"),
        "1"
    );
    // The value, not just the name, is what selects the behaviour now.
    assert_eq!(
        run("<?php echo fnmatch('FOO', 'foo', 16) ? '1' : '0';"),
        "1"
    );
    assert_eq!(run("<?php echo fnmatch('FOO', 'foo', 0) ? '1' : '0';"), "0");
}

// ── stat / lstat / fileperms / filetype ─────────────────────────────────────

#[test]
fn stat_reports_size_under_numeric_and_named_keys() {
    let dir = unique_dir("stat");
    let file = child(&dir, "s.txt");
    let src = format!(
        "<?php file_put_contents('{file}', 'hello');
         $s = stat('{file}');
         echo $s['size'], ',', $s[7], ',', ($s['mtime'] > 0 ? 'mt' : 'no'), ',', ($s['ino'] === $s[1] ? 'dup' : 'x');"
    );
    // size==5 both keys, mtime positive, numeric/named views agree.
    assert_eq!(run(&src), "5,5,mt,dup");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn stat_missing_returns_false() {
    let dir = unique_dir("statmiss");
    let file = child(&dir, "absent");
    let src = format!("<?php var_dump(stat('{file}'));");
    assert_eq!(run(&src), "bool(false)\n");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn lstat_and_filetype_on_symlink() {
    let dir = unique_dir("lstat");
    let target = dir.join("target.txt");
    std::fs::write(&target, "x").unwrap();
    let link = dir.join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let link_s = link.to_string_lossy().into_owned();
    let src = format!(
        "<?php echo filetype('{link_s}'), ',', is_link('{link_s}') ? '1' : '0', ',', (stat('{link_s}') !== false ? 'follows' : 'no');"
    );
    // lstat-based filetype is 'link'; is_link true; stat() follows to the file.
    assert_eq!(run(&src), "link,1,follows");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn fileperms_returns_mode_int() {
    let dir = unique_dir("perms");
    let file = child(&dir, "p.txt");
    let src = format!(
        "<?php file_put_contents('{file}', 'x');
         $m = fileperms('{file}');
         // Full st_mode carries the regular-file type bit: (mode & S_IFMT) ==
         // S_IFREG, i.e. (mode & 61440) == 32768 in decimal.
         echo is_int($m) ? '1' : '0', ':', (($m & 61440) === 32768) ? 'reg' : 'other';"
    );
    assert_eq!(run(&src), "1:reg");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn fileperms_missing_returns_false() {
    let dir = unique_dir("permsmiss");
    let file = child(&dir, "absent");
    let src = format!("<?php var_dump(fileperms('{file}'));");
    assert_eq!(run(&src), "bool(false)\n");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn filetype_dir_and_file() {
    let dir = unique_dir("ftype");
    let dirs = dir.to_string_lossy().into_owned();
    let file = child(&dir, "f.txt");
    let src = format!(
        "<?php file_put_contents('{file}', 'x'); echo filetype('{dirs}'), ',', filetype('{file}');"
    );
    assert_eq!(run(&src), "dir,file");
    std::fs::remove_dir_all(&dir).unwrap();
}

// ── is_executable ───────────────────────────────────────────────────────────

#[test]
fn is_executable_respects_mode_bits() {
    use std::os::unix::fs::PermissionsExt;
    let dir = unique_dir("exec");
    let plain = dir.join("plain.txt");
    std::fs::write(&plain, "x").unwrap();
    std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o644)).unwrap();
    let exe = dir.join("run.sh");
    std::fs::write(&exe, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let plain_s = plain.to_string_lossy().into_owned();
    let exe_s = exe.to_string_lossy().into_owned();
    let src = format!(
        "<?php echo is_executable('{plain_s}') ? '1' : '0'; echo is_executable('{exe_s}') ? '1' : '0';"
    );
    // 0644 → not executable; 0755 → executable. (Linux grants X_OK to root only
    // when an execute bit is set, so this holds under CI's root as well.)
    assert_eq!(run(&src), "01");
    std::fs::remove_dir_all(&dir).unwrap();
}

// ── disk space ──────────────────────────────────────────────────────────────

#[test]
fn disk_space_is_positive_float() {
    let dir = unique_dir("disk");
    let dirs = dir.to_string_lossy().into_owned();
    let src = format!(
        "<?php $t = disk_total_space('{dirs}'); $f = disk_free_space('{dirs}');
         echo (is_float($t) && $t > 0) ? '1' : '0';
         echo (is_float($f) && $f > 0 && $f <= $t) ? '1' : '0';
         echo disk_free_space('/definitely/not/a/directory/xyz') === false ? '1' : '0';"
    );
    // `$f >= 0` was trivially true; a filesystem with a real free-space figure
    // reports more than zero. The third digit is the negative control: a path
    // that does not exist must NOT answer with a number.
    assert_eq!(run(&src), "111");
    std::fs::remove_dir_all(&dir).unwrap();
}

// ── clearstatcache / tempnam ────────────────────────────────────────────────

#[test]
fn clearstatcache_returns_null() {
    assert_eq!(run("<?php var_dump(clearstatcache());"), "NULL\n");
}

#[test]
fn tempnam_creates_a_unique_writable_file() {
    let dir = unique_dir("tempnam");
    let dirs = dir.to_string_lossy().into_owned();
    let src = format!(
        "<?php $a = tempnam('{dirs}', 'pre'); $b = tempnam('{dirs}', 'pre');
         echo (is_string($a) && file_exists($a)) ? '1' : '0';
         echo ($a !== $b) ? '1' : '0';
         echo (strpos(basename($a), 'pre') === 0) ? '1' : '0';
         file_put_contents($a, 'data'); echo file_get_contents($a);"
    );
    // File exists, two calls differ, name carries the prefix, and it is writable.
    assert_eq!(run(&src), "111data");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn tempnam_bad_dir_falls_back_to_temp() {
    let dir = unique_dir("tempnamfallback");
    let missing = child(&dir, "no_such_dir");
    // A non-directory `dir` argument → PHP falls back to the system temp dir.
    let src = format!(
        "<?php $t = tempnam('{missing}', 'fb'); echo (is_string($t) && file_exists($t)) ? '1' : '0'; if (is_string($t)) unlink($t);"
    );
    assert_eq!(run(&src), "1");
    std::fs::remove_dir_all(&dir).unwrap();
}
