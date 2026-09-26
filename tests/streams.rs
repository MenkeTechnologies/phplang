//! Stream resources: numbering, every rendering of a resource, the `php://`
//! wrappers, `STDIN`/`STDOUT`/`STDERR`, end-of-file semantics, seeking past the
//! end, append modes, and the argument checks the stream functions run.
//!
//! Every expectation is the verbatim stdout of the same `-r` program under the
//! reference `php` 8.5.11 with `-d log_errors=0`, `LC_ALL=C` and `TZ=UTC`,
//! captured by running it. The two `stdin` cases feed the same four-line input
//! to both.

use std::io::Write;
use std::process::{Command, Stdio};

/// The input the `stdin` cases pipe in.
const STDIN_TEXT: &str = "line one\n12 abc\nxyzrest\nmore";

/// Run `code` through the crate's binary with `stdin` piped in; return stdout.
fn run(code: &str, stdin: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_php"))
        .args(["-d", "log_errors=0", "-r", code])
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn php");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait php");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn resource_numbers_and_every_rendering() {
    let code = r#"$a=fopen("php://memory","r+"); $t=fopen("php://temp","r+"); $m=fopen("php://memory","r"); $f=tmpfile(); var_dump($a,$t,$m,$f,STDIN,STDOUT,STDERR); echo $t, "|", (int)$t, "|", gettype($t), "|", get_debug_type($t), "\n"; print_r([$t]); var_export($t); echo "\n", serialize([$t]), "\n"; var_dump(json_encode($t), json_last_error_msg());"#;
    let expected = r#"resource(5) of type (stream)
resource(6) of type (stream)
resource(8) of type (stream)
resource(9) of type (stream)
resource(1) of type (stream)
resource(2) of type (stream)
resource(3) of type (stream)
Resource id #6|6|resource|resource (stream)
Array
(
    [0] => Resource id #6
)
NULL
a:1:{i:0;i:0;}
bool(false)
string(21) "Type is not supported"
"#;
    assert_eq!(run(code, ""), expected);
}

#[test]
fn a_closed_stream_is_still_a_resource() {
    let code = r#"$a=fopen("php://memory","r+"); fclose($a); var_dump($a, gettype($a), get_debug_type($a), get_resource_type($a), get_resource_id($a), is_resource($a)); echo $a, "\n";"#;
    let expected = r#"resource(5) of type (Unknown)
string(17) "resource (closed)"
string(17) "resource (closed)"
string(7) "Unknown"
int(5)
bool(false)
Resource id #5
"#;
    assert_eq!(run(code, ""), expected);
}

#[test]
fn temp_takes_two_numbers_and_a_spill_a_third() {
    let code = r#"$e=fopen("php://temp/maxmemory:4","r+"); var_dump($e); fwrite($e,"ab"); var_dump(fopen("php://memory","r")); fwrite($e,"cd"); var_dump(fopen("php://memory","r")); fwrite($e,"e"); var_dump(fopen("php://memory","r")); rewind($e); var_dump(fread($e, 10));"#;
    let expected = r#"resource(5) of type (stream)
resource(7) of type (stream)
resource(9) of type (stream)
resource(10) of type (stream)
string(5) "abcde"
"#;
    assert_eq!(run(code, ""), expected);
}

#[test]
fn feof_needs_a_read_that_came_up_short() {
    let code = r#"$f=fopen("php://memory","w+"); fwrite($f,"a\nb\n"); rewind($f); var_dump(feof($f)); fgets($f); fgets($f); var_dump(feof($f)); var_dump(fgets($f), feof($f)); rewind($f); var_dump(feof($f), fread($f,4), feof($f), fread($f,1), feof($f)); rewind($f); var_dump(fread($f,100), feof($f)); fseek($f,1); var_dump(feof($f), fgetc($f), fgetc($f), fgetc($f), fgetc($f), fgetc($f), feof($f));"#;
    let expected = r#"bool(false)
bool(false)
bool(false)
bool(true)
bool(false)
string(4) "a
b
"
bool(false)
string(0) ""
bool(true)
string(4) "a
b
"
bool(true)
bool(false)
string(1) "
"
string(1) "b"
string(1) "
"
bool(false)
bool(false)
bool(true)
"#;
    assert_eq!(run(code, ""), expected);
}

#[test]
fn seek_write_and_truncate() {
    let code = r#"$m=fopen("php://memory","w+"); fwrite($m,"hello world"); var_dump(fseek($m,-100), ftell($m), fseek($m,100), ftell($m), fread($m,1), feof($m)); fseek($m, 13); fwrite($m,"Z"); rewind($m); var_dump(bin2hex(stream_get_contents($m))); var_dump(ftruncate($m,5), ftell($m)); rewind($m); var_dump(fread($m,100)); ftruncate($m,7); var_dump(bin2hex(stream_get_contents($m, -1, 0))); var_dump(stream_get_contents($m, 3, 1), ftell($m)); fseek($m, -2, SEEK_END); var_dump(bin2hex(fread($m, 5))); fseek($m, 1); fseek($m, 1, SEEK_CUR); var_dump(ftell($m));"#;
    let expected = r#"int(-1)
int(11)
int(0)
int(100)
string(0) ""
bool(true)
string(28) "68656c6c6f20776f726c6400005a"
bool(true)
int(14)
string(5) "hello"
string(14) "68656c6c6f0000"
string(3) "ell"
int(4)
string(4) "0000"
int(2)
"#;
    assert_eq!(run(code, ""), expected);
}

#[test]
fn argument_one_must_be_an_open_stream() {
    let code = r#"$f=fopen("php://memory","r+"); fclose($f); foreach (["fread"=>[$f,"y"],"fwrite"=>[$f,"x"],"fclose"=>[$f],"feof"=>[$f],"fgets"=>[$f],"fgetc"=>[$f],"ftell"=>[$f],"rewind"=>[$f],"fseek"=>[$f,0],"ftruncate"=>[$f,0],"fflush"=>[$f],"fstat"=>[$f],"fpassthru"=>[$f],"stream_get_contents"=>[$f],"fprintf"=>[$f,"%d",1],"fread "=>["x",1],"fwrite "=>[null,"x"],"fclose "=>[new stdClass],"get_resource_type"=>["x"],"get_resource_id"=>[1]] as $fn=>$args) { try { trim($fn)(...$args); echo "no error\n"; } catch (TypeError $e) { echo $e->getMessage(), "\n"; } }"#;
    let expected = r#"fread(): Argument #1 ($stream) must be an open stream resource
fwrite(): Argument #1 ($stream) must be an open stream resource
fclose(): Argument #1 ($stream) must be an open stream resource
feof(): Argument #1 ($stream) must be an open stream resource
fgets(): Argument #1 ($stream) must be an open stream resource
fgetc(): Argument #1 ($stream) must be an open stream resource
ftell(): Argument #1 ($stream) must be an open stream resource
rewind(): Argument #1 ($stream) must be an open stream resource
fseek(): Argument #1 ($stream) must be an open stream resource
ftruncate(): Argument #1 ($stream) must be an open stream resource
fflush(): Argument #1 ($stream) must be an open stream resource
fstat(): Argument #1 ($stream) must be an open stream resource
fpassthru(): Argument #1 ($stream) must be an open stream resource
stream_get_contents(): Argument #1 ($stream) must be an open stream resource
fprintf(): Argument #1 ($stream) must be an open stream resource
fread(): Argument #1 ($stream) must be of type resource, string given
fwrite(): Argument #1 ($stream) must be of type resource, null given
fclose(): Argument #1 ($stream) must be of type resource, stdClass given
get_resource_type(): Argument #1 ($resource) must be of type resource, string given
get_resource_id(): Argument #1 ($resource) must be of type resource, int given
"#;
    assert_eq!(run(code, ""), expected);
}

#[test]
fn length_arguments_below_one_are_value_errors() {
    let code = r#"$f=fopen("php://memory","r+"); foreach ([fn() => fread($f, 0), fn() => fgets($f, 0), fn() => ftruncate($f, -1), fn() => fopen("", "r")] as $c) { try { $c(); } catch (ValueError $e) { echo $e->getMessage(), "\n"; } }"#;
    let expected = r#"fread(): Argument #2 ($length) must be greater than 0
fgets(): Argument #2 ($length) must be greater than 0
ftruncate(): Argument #2 ($size) must be greater than or equal to 0
Path must not be empty
"#;
    assert_eq!(run(code, ""), expected);
}

#[test]
fn open_failures_warn_and_return_false() {
    let code = r#"var_dump(fopen("/nonexistent/x", "r")); var_dump(fopen("/etc/hosts", "q")); var_dump(fopen("/tmp", "w")); var_dump(fopen("php://nope", "r")); var_dump(fopen("/etc/hosts", "x"));"#;
    let expected = r#"
Warning: fopen(/nonexistent/x): Failed to open stream: No such file or directory in Command line code on line 1
bool(false)

Warning: fopen(/etc/hosts): Failed to open stream: `q' is not a valid mode for fopen in Command line code on line 1
bool(false)

Warning: fopen(/tmp): Failed to open stream: Is a directory in Command line code on line 1
bool(false)

Warning: fopen(): Invalid php:// URL specified in Command line code on line 1

Warning: fopen(php://nope): Failed to open stream: operation failed in Command line code on line 1
bool(false)

Warning: fopen(/etc/hosts): Failed to open stream: File exists in Command line code on line 1
bool(false)
"#;
    assert_eq!(run(code, ""), expected);
}

#[test]
fn a_stream_refuses_the_direction_it_was_not_opened_for() {
    let code = r#"$p = sys_get_temp_dir() . "/phplang_st_dir.txt"; $w = fopen($p, "w"); var_dump(fread($w, 3), fgets($w), fgetc($w)); fclose($w); $r = fopen($p, "r"); var_dump(fwrite($r, "x")); fclose($r); $m = fopen("php://memory", "r"); var_dump(fwrite($m, "x")); $t = fopen("php://temp", "rb"); var_dump(fwrite($t, "x")); unlink($p);"#;
    let expected = r#"
Notice: fread(): Read of 8192 bytes failed with errno=9 Bad file descriptor in Command line code on line 1

Notice: fgets(): Read of 8192 bytes failed with errno=9 Bad file descriptor in Command line code on line 1

Notice: fgetc(): Read of 8192 bytes failed with errno=9 Bad file descriptor in Command line code on line 1
bool(false)
bool(false)
bool(false)

Notice: fwrite(): Write of 1 bytes failed with errno=9 Bad file descriptor in Command line code on line 1
bool(false)
bool(false)
bool(false)
"#;
    assert_eq!(run(code, ""), expected);
}

#[test]
fn append_mode_writes_at_the_end_and_counts_from_zero() {
    let code = r#"$p = sys_get_temp_dir() . "/phplang_st_app.txt"; file_put_contents($p, "abc"); $w = fopen($p, "a+"); var_dump(ftell($w)); fwrite($w, "d"); var_dump(ftell($w)); fseek($w, 0); fwrite($w, "e"); rewind($w); var_dump(fread($w, 10)); fclose($w); var_dump(file_get_contents($p)); $c = fopen($p, "c+"); var_dump(fread($c, 2)); fwrite($c, "Z"); fclose($c); var_dump(file_get_contents($p)); $m = fopen("php://memory", "a"); fwrite($m, "ab"); rewind($m); fwrite($m, "Z"); rewind($m); var_dump(stream_get_contents($m)); unlink($p);"#;
    let expected = r#"int(0)
int(1)
string(5) "abcde"
string(5) "abcde"
string(2) "ab"
string(5) "abZde"
string(3) "abZ"
"#;
    assert_eq!(run(code, ""), expected);
}

#[test]
fn stdout_bypasses_output_buffering_and_output_does_not() {
    let code = r#"ob_start(); echo "buffered\n"; fwrite(STDOUT, "direct\n"); $o = fopen("php://output", "w"); fwrite($o, "out\n"); $b = ob_get_clean(); echo "[", $b, "]\n"; fprintf(STDOUT, "%05d\n", 42); var_dump(file_put_contents("php://stdout", "hi\n"), file_put_contents("php://output", "o\n"));"#;
    let expected = r#"direct
[buffered
out
]
00042
hi
o
int(3)
int(2)
"#;
    assert_eq!(run(code, ""), expected);
}

#[test]
fn resources_compare_by_number() {
    let code = r#"$a=fopen("php://memory","r"); $d=fopen("php://memory","r"); var_dump($a == $a, $a === $d, $a == 5, $a != $d, $a < $d, $a == "5", $a == true, $a <=> $d, $a == null, [$a] == [$a], $a == [], max($a, 3), in_array(5, [$a]));"#;
    let expected = r#"bool(true)
bool(false)
bool(true)
bool(true)
bool(true)
bool(true)
bool(true)
int(-1)
bool(false)
bool(true)
bool(false)
resource(5) of type (stream)
bool(true)
"#;
    assert_eq!(run(code, ""), expected);
}

#[test]
fn a_resource_key_is_its_number() {
    let code = r#"$a=[]; $a[STDERR]="x"; var_dump($a, isset($a[STDERR]));"#;
    let expected = r#"
Warning: Resource ID#3 used as offset, casting to integer (3) in Command line code on line 1

Warning: Resource ID#3 used as offset, casting to integer (3) in Command line code on line 1
array(1) {
  [3]=>
  string(1) "x"
}
bool(true)
"#;
    assert_eq!(run(code, ""), expected);
}

#[test]
fn arithmetic_on_a_resource_is_a_type_error() {
    let code = r#"function f(int $x) {} try { f(STDIN); } catch (TypeError $e) { echo $e->getMessage(), "\n", $e->getTraceAsString(), "\n"; } $x = 1; try { $x = STDIN * 2; } catch (TypeError $e) { echo $e->getMessage(), "\n"; }"#;
    let expected = r#"f(): Argument #1 ($x) must be of type int, resource given, called in Command line code on line 1
#0 Command line code(1): f(Resource id #1)
#1 {main}
Unsupported operand types: resource * int
"#;
    assert_eq!(run(code, ""), expected);
}

#[test]
fn fstat_of_a_memory_stream() {
    let code = r#"$m=fopen("php://memory","w+"); fwrite($m,"abc"); print_r(fstat($m)); $r=fopen("php://memory","r"); var_dump(fstat($r)["mode"], fstat(fopen("php://output","w")));"#;
    let expected = r#"Array
(
    [0] => 12
    [1] => 0
    [2] => 33206
    [3] => 1
    [4] => 0
    [5] => 0
    [6] => -1
    [7] => 3
    [8] => 0
    [9] => 0
    [10] => 0
    [11] => -1
    [12] => -1
    [dev] => 12
    [ino] => 0
    [mode] => 33206
    [nlink] => 1
    [uid] => 0
    [gid] => 0
    [rdev] => -1
    [size] => 3
    [atime] => 0
    [mtime] => 0
    [ctime] => 0
    [blksize] => -1
    [blocks] => -1
)
int(33060)
bool(false)
"#;
    assert_eq!(run(code, ""), expected);
}

#[test]
fn whole_file_functions_spend_a_number_and_warn() {
    let code = r#"file_get_contents("/etc/hosts"); var_dump(fopen("php://memory","r")); file("/etc/hosts"); readfile("/dev/null"); var_dump(fopen("php://memory","r")); var_dump(@file_get_contents("/nope")); var_dump(fopen("php://memory","r")); var_dump(file_get_contents("/nope"), file("/nope"), readfile("/nope"), file_put_contents("/nope/x","a"), file_get_contents("php://bogus"), file_put_contents("php://memory", "x"), fopen("php://memory","r"));"#;
    let expected = r#"resource(6) of type (stream)
resource(9) of type (stream)
bool(false)
resource(10) of type (stream)

Warning: file_get_contents(/nope): Failed to open stream: No such file or directory in Command line code on line 1

Warning: file(/nope): Failed to open stream: No such file or directory in Command line code on line 1

Warning: readfile(/nope): Failed to open stream: No such file or directory in Command line code on line 1

Warning: file_put_contents(/nope/x): Failed to open stream: No such file or directory in Command line code on line 1

Warning: file_get_contents(): Invalid php:// URL specified in Command line code on line 1

Warning: file_get_contents(php://bogus): Failed to open stream: operation failed in Command line code on line 1
bool(false)
bool(false)
bool(false)
bool(false)
bool(false)
int(1)
resource(12) of type (stream)
"#;
    assert_eq!(run(code, ""), expected);
}

#[test]
fn stdin_is_read_lazily() {
    let code = r#"var_dump(fgets(STDIN)); var_dump(fscanf(STDIN, "%d %s")); var_dump(fread(STDIN, 3)); var_dump(feof(STDIN)); var_dump(stream_get_contents(STDIN)); var_dump(feof(STDIN)); var_dump(fgets(STDIN), feof(STDIN));"#;
    let expected = r#"string(9) "line one
"
array(2) {
  [0]=>
  int(12)
  [1]=>
  string(3) "abc"
}
string(3) "xyz"
bool(false)
string(9) "rest
more"
bool(true)
bool(false)
bool(true)
"#;
    assert_eq!(run(code, STDIN_TEXT), expected);
}

#[test]
fn php_stdin_url_reads_everything() {
    let code =
        r#"var_dump(file_get_contents("php://stdin")); var_dump(fopen("php://memory", "r"));"#;
    let expected = r#"string(28) "line one
12 abc
xyzrest
more"
resource(6) of type (stream)
"#;
    assert_eq!(run(code, STDIN_TEXT), expected);
}
