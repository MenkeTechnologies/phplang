//! The fopen file-stream family: fopen/fwrite/fread/fgets/feof/ftell/fseek/
//! rewind/fclose/is_resource, backed by an in-memory buffer flushed to disk.

use phplang::eval_capture;
use std::path::PathBuf;

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("phplang_fres_{}_{name}", std::process::id()))
}

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn write_then_read_back() {
    let p = tmp("rw.txt");
    let ps = p.display();
    let src = format!(
        r#"<?php
        $h = fopen("{ps}", "w");
        $n = fwrite($h, "hello world");
        fclose($h);
        $r = fopen("{ps}", "r");
        $data = fread($r, 100);
        fclose($r);
        echo $n, "|", $data;"#
    );
    assert_eq!(run(&src), "11|hello world");
    let _ = std::fs::remove_file(&p);
}

#[test]
fn fgets_reads_lines_and_feof() {
    let p = tmp("lines.txt");
    let ps = p.display();
    std::fs::write(&p, "one\ntwo\nthree\n").unwrap();
    let src = format!(
        r#"<?php
        $h = fopen("{ps}", "r");
        $out = "";
        while (($line = fgets($h)) !== false) {{ $out .= trim($line) . ","; }}
        $eof = feof($h) ? "eof" : "more";
        fclose($h);
        echo $out, $eof;"#
    );
    assert_eq!(run(&src), "one,two,three,eof");
    let _ = std::fs::remove_file(&p);
}

#[test]
fn ftell_seek_rewind() {
    let p = tmp("seek.txt");
    let ps = p.display();
    std::fs::write(&p, "0123456789").unwrap();
    let src = format!(
        r#"<?php
        $h = fopen("{ps}", "r");
        fread($h, 3);
        echo ftell($h);          // 3
        fseek($h, 6, 0);
        echo fread($h, 2);       // "67"
        rewind($h);
        echo fread($h, 1);       // "0"
        fclose($h);"#
    );
    assert_eq!(run(&src), "3670");
    let _ = std::fs::remove_file(&p);
}

#[test]
fn append_mode() {
    let p = tmp("append.txt");
    let ps = p.display();
    std::fs::write(&p, "AAA").unwrap();
    let src = format!(
        r#"<?php
        $h = fopen("{ps}", "a");
        fwrite($h, "BBB");
        fclose($h);
        echo file_get_contents("{ps}");"#
    );
    assert_eq!(run(&src), "AAABBB");
    let _ = std::fs::remove_file(&p);
}

#[test]
fn fopen_missing_read_returns_false_and_is_resource() {
    let p = tmp("nope.txt");
    let ps = p.display();
    let _ = std::fs::remove_file(&p);
    let src = format!(
        r#"<?php
        $h = @fopen("{ps}", "r");
        echo $h === false ? "false" : "handle";
        $w = fopen("{ps}", "w");
        echo "|", is_resource($w) ? "res" : "nores", "|", get_resource_type($w);
        fclose($w);"#
    );
    assert_eq!(run(&src), "false|res|stream");
    let _ = std::fs::remove_file(&p);
}
