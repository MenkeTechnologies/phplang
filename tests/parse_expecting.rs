//! Syntax errors and compile-time refusals for one stray operand inserted into
//! an otherwise valid construct: which token the reference names and the
//! `expecting …` list its grammar tables attach, if any.
//!
//! Every pair is the `Parse error:` / `Fatal error:` line of `php -r` under the
//! reference `php` 8.5.11. The qualified-name cases hang on whitespace: `A \ B`
//! is three tokens to the reference and `A\B` is one.

use std::process::{Command, Stdio};

const CASES: &[(&str, &str)] = &[
    ("! ;", "Parse error: syntax error, unexpected token \";\""),
    ("@ ;", "Parse error: syntax error, unexpected token \";\""),
    ("++ 1 ;", "Parse error: syntax error, unexpected integer \"1\""),
    ("$a -> b ( 1 2 ) ;", "Parse error: syntax error, unexpected integer \"2\", expecting \")\""),
    ("$a ? 1 : 2 3 ;", "Parse error: syntax error, unexpected integer \"3\""),
    ("$a ? 1 2 ;", "Parse error: syntax error, unexpected integer \"2\""),
    ("$a [ 1 2 ] ;", "Parse error: syntax error, unexpected integer \"2\", expecting \"]\""),
    ("$a ++ ++ ;", "Parse error: syntax error, unexpected token \"++\""),
    ("$a = [ 'k' => ] ;", "Parse error: syntax error, unexpected token \"]\""),
    ("$a = [ 'k' => 1 'j' => 2 ] ;", "Parse error: syntax error, unexpected single-quoted string \"j\", expecting \"]\""),
    ("$a = [ 1 , 2 3 ] ;", "Parse error: syntax error, unexpected integer \"3\", expecting \"]\""),
    ("$a = [ 1 2 ] ;", "Parse error: syntax error, unexpected integer \"2\", expecting \"]\""),
    ("$a = & 1 ;", "Parse error: syntax error, unexpected integer \"1\""),
    ("$a = <<< 1 ;", "Parse error: syntax error, unexpected token \"<<\""),
    ("$a = 1 2 ;", "Parse error: syntax error, unexpected integer \"2\""),
    ("$a = array ( 1 2 ) ;", "Parse error: syntax error, unexpected integer \"2\", expecting \")\""),
    ("$a = fn 1 ;", "Parse error: syntax error, unexpected integer \"1\", expecting \"(\""),
    ("$a = new 1 ;", "Parse error: syntax error, unexpected integer \"1\", expecting \"class\""),
    ("$a = static 1 ;", "Parse error: syntax error, unexpected integer \"1\", expecting \"::\""),
    ("$a 1 ;", "Parse error: syntax error, unexpected integer \"1\""),
    ("$f = fn ( $a ) 1 $a;", "Parse error: syntax error, unexpected integer \"1\", expecting \"=>\""),
    ("$f = function ( $a ) 1 { };", "Parse error: syntax error, unexpected integer \"1\", expecting \"{\""),
    ("$f = function ( $a ) use ( $b , 1 ) { };", "Parse error: syntax error, unexpected integer \"1\", expecting \")\""),
    ("$f = function ( $a ) use ( $b 1 ) { };", "Parse error: syntax error, unexpected integer \"1\", expecting \")\""),
    ("$f = function ( $a ) use ( 1 $b ) { };", "Parse error: syntax error, unexpected integer \"1\", expecting variable or \"&\" or token \"&\""),
    ("$f = function ( $a ) use 1 ( $b ) { };", "Parse error: syntax error, unexpected integer \"1\", expecting \"(\""),
    ("$x = $a -> 1 m ( 1 );", "Parse error: syntax error, unexpected integer \"1\", expecting identifier or variable or \"{\" or \"$\""),
    ("$x = $a :: 1 ;", "Parse error: syntax error, unexpected integer \"1\""),
    ("$x = $a ?-> 1 m ( 1 );", "Parse error: syntax error, unexpected integer \"1\", expecting identifier or variable or \"{\" or \"$\""),
    ("$x = A :: 's' ;", "Parse error: syntax error, unexpected single-quoted string \"s\""),
    ("$x = A :: 1 m ( 1 );", "Parse error: syntax error, unexpected integer \"1\""),
    ("$x = array 1 ( 1 );", "Parse error: syntax error, unexpected integer \"1\", expecting \"(\""),
    ("$x = match ( $a ) { 1 => 2 3 };", "Parse error: syntax error, unexpected integer \"3\", expecting \"}\""),
    ("$x = match ( $a ) { 1 2 => 2 };", "Parse error: syntax error, unexpected integer \"2\", expecting \"=>\""),
    ("$x = match ( $a ) 1 { 1 => 2 };", "Parse error: syntax error, unexpected integer \"1\", expecting \"{\""),
    ("$x = match 1 ( $a ) { 1 => 2 };", "Parse error: syntax error, unexpected integer \"1\", expecting \"(\""),
    ("1 2 ;", "Parse error: syntax error, unexpected integer \"2\""),
    ("abstract 1 class A { }", "Parse error: syntax error, unexpected integer \"1\", expecting \"abstract\" or \"final\" or \"readonly\" or \"class\""),
    ("abstract final readonly 1 class A { }", "Fatal error: Cannot use the final modifier on an abstract class"),
    ("class A { const 1 = 1; }", "Parse error: syntax error, unexpected integer \"1\""),
    ("class A { const X = 1 2; }", "Parse error: syntax error, unexpected integer \"2\", expecting \",\" or \";\""),
    ("class A { const X 1; }", "Parse error: syntax error, unexpected integer \"1\""),
    ("class A { function 1 () { } }", "Parse error: syntax error, unexpected integer \"1\""),
    ("class A { function f ( ) : 1 { } }", "Parse error: syntax error, unexpected integer \"1\""),
    ("class A { function f ( ) 1 { } }", "Parse error: syntax error, unexpected integer \"1\", expecting \";\" or \"{\""),
    ("class A { function f ( 1 ) { } }", "Parse error: syntax error, unexpected integer \"1\", expecting variable"),
    ("class A { function f 1 () { } }", "Parse error: syntax error, unexpected integer \"1\", expecting \"(\""),
    ("class A { public 1 $a; }", "Parse error: syntax error, unexpected integer \"1\", expecting variable"),
    ("class A { public int 1 $a; }", "Parse error: syntax error, unexpected integer \"1\", expecting variable"),
    ("class A { use 1; }", "Parse error: syntax error, unexpected integer \"1\""),
    ("class A { use T 1 }", "Parse error: syntax error, unexpected integer \"1\", expecting \",\" or \";\" or \"{\""),
    ("class A 1 { }", "Parse error: syntax error, unexpected integer \"1\", expecting \"{\""),
    ("class A extends 1 { }", "Parse error: syntax error, unexpected integer \"1\""),
    ("class A implements 1 { }", "Parse error: syntax error, unexpected integer \"1\""),
    ("class X extends A \\ B {}", "Parse error: syntax error, unexpected token \"\\\", expecting \"{\""),
    ("clone 1 2 ;", "Parse error: syntax error, unexpected integer \"2\""),
    ("const 1 = 1 ;", "Parse error: syntax error, unexpected integer \"1\", expecting identifier"),
    ("const A 1 ;", "Parse error: syntax error, unexpected integer \"1\", expecting \"=\""),
    ("declare ( ticks = 1 2 ) ;", "Parse error: syntax error, unexpected integer \"2\", expecting \",\" or \")\""),
    ("do { } 1 while ( 1 );", "Parse error: syntax error, unexpected integer \"1\", expecting \"while\""),
    ("do { } while ( 1 ) 2 ;", "Parse error: syntax error, unexpected integer \"2\", expecting \";\""),
    ("do { } while 1 ( 1 );", "Parse error: syntax error, unexpected integer \"1\", expecting \"(\""),
    ("echo \"x\" instanceof A \\ B;", "Parse error: syntax error, unexpected token \"\\\", expecting \",\" or \";\""),
    ("echo \\ strlen(\"a\");", "Parse error: syntax error, unexpected token \"\\\""),
    ("echo \\A\\B C;", "Parse error: syntax error, unexpected identifier \"C\", expecting \",\" or \";\""),
    ("echo 1 2 ;", "Parse error: syntax error, unexpected integer \"2\", expecting \",\" or \";\""),
    ("echo A \\ B::C;", "Parse error: syntax error, unexpected token \"\\\", expecting \",\" or \";\""),
    ("echo A \\B;", "Parse error: syntax error, unexpected fully qualified name \"\\B\", expecting \",\" or \";\""),
    ("echo A\\B C;", "Parse error: syntax error, unexpected identifier \"C\", expecting \",\" or \";\""),
    ("echo namespace \\ strlen(\"a\");", "Parse error: syntax error, unexpected token \"namespace\""),
    ("empty ( $a 1 ) ;", "Parse error: syntax error, unexpected integer \"1\""),
    ("eval ( 1 2 ) ;", "Parse error: syntax error, unexpected integer \"2\""),
    ("exit ( 1 2 ) ;", "Parse error: syntax error, unexpected integer \"2\", expecting \")\""),
    ("f ( 1 , 2 3 ) ;", "Parse error: syntax error, unexpected integer \"3\", expecting \")\""),
    ("f ( 1 2 ) ;", "Parse error: syntax error, unexpected integer \"2\", expecting \")\""),
    ("final 1 class A { }", "Parse error: syntax error, unexpected integer \"1\", expecting \"abstract\" or \"final\" or \"readonly\" or \"class\""),
    ("for ( ; ; 1 2 ) { }", "Parse error: syntax error, unexpected integer \"2\", expecting \")\""),
    ("for 1 ( ; ; ) { }", "Parse error: syntax error, unexpected integer \"1\", expecting \"(\""),
    ("foreach ( $a 1 $b ) { }", "Parse error: syntax error, unexpected integer \"1\""),
    ("foreach ( $a as $b => 1 ) { }", "Parse error: syntax error, unexpected integer \"1\""),
    ("foreach ( $a as $b 1 ) { }", "Parse error: syntax error, unexpected integer \"1\", expecting \"->\" or \"?->\" or \"[\""),
    ("foreach 1 ( $a as $b ) { }", "Parse error: syntax error, unexpected integer \"1\", expecting \"(\""),
    ("function 1 () { }", "Parse error: syntax error, unexpected integer \"1\", expecting \"(\""),
    ("function f ( ) 1 { }", "Parse error: syntax error, unexpected integer \"1\", expecting \"{\""),
    ("function f ( $a 1 ) { }", "Parse error: syntax error, unexpected integer \"1\", expecting \")\""),
    ("function f ( 1 ) { }", "Parse error: syntax error, unexpected integer \"1\", expecting variable"),
    ("function f 1 () { }", "Parse error: syntax error, unexpected integer \"1\", expecting \"(\""),
    ("function f(A \\ B $x){} echo 1;", "Parse error: syntax error, unexpected token \"\\\", expecting variable"),
    ("global 1 ;", "Parse error: syntax error, unexpected integer \"1\", expecting variable or \"$\""),
    ("goto 1 ;", "Parse error: syntax error, unexpected integer \"1\", expecting identifier"),
    ("if ( 1 ) 2 3 ;", "Parse error: syntax error, unexpected integer \"3\""),
    ("if 1 ( 1 ) { }", "Parse error: syntax error, unexpected integer \"1\", expecting \"(\""),
    ("include 1 2 ;", "Parse error: syntax error, unexpected integer \"2\""),
    ("interface I 1 { }", "Parse error: syntax error, unexpected integer \"1\", expecting \"{\""),
    ("isset ( $a 1 ) ;", "Parse error: syntax error, unexpected integer \"1\", expecting \")\""),
    ("list ( $a ) 1 ;", "Parse error: syntax error, unexpected integer \"1\", expecting \"=\""),
    ("list 1 ( $a ) = $b ;", "Parse error: syntax error, unexpected integer \"1\", expecting \"(\""),
    ("namespace 1 ;", "Parse error: syntax error, unexpected integer \"1\", expecting \"{\""),
    ("namespace A \\ B;", "Parse error: syntax error, unexpected token \"\\\", expecting \"{\""),
    ("namespace A 1 ;", "Parse error: syntax error, unexpected integer \"1\", expecting \"{\""),
    ("namespace A; class C{} $x = new A \\ C;", "Parse error: syntax error, unexpected token \"\\\""),
    ("namespace A; function f(){return 1;} echo A \\ f();", "Parse error: syntax error, unexpected token \"\\\", expecting \",\" or \";\""),
    ("new 1 ;", "Parse error: syntax error, unexpected integer \"1\", expecting \"class\""),
    ("new A 1 ;", "Parse error: syntax error, unexpected integer \"1\""),
    ("new class 1 { } ;", "Parse error: syntax error, unexpected integer \"1\", expecting \"{\""),
    ("print 1 2 ;", "Parse error: syntax error, unexpected integer \"2\""),
    ("readonly 1 class A { }", "Parse error: syntax error, unexpected integer \"1\", expecting \"abstract\" or \"final\" or \"readonly\" or \"class\""),
    ("return 1 2 ;", "Parse error: syntax error, unexpected integer \"2\", expecting \";\""),
    ("static $a = 1 , 1 $b ;", "Parse error: syntax error, unexpected integer \"1\", expecting variable"),
    ("static $a 1 ;", "Parse error: syntax error, unexpected integer \"1\", expecting \",\" or \";\""),
    ("static 1 $a = 1 ;", "Parse error: syntax error, unexpected integer \"1\", expecting \"::\""),
    ("switch ( $a ) { 1 }", "Parse error: syntax error, unexpected integer \"1\", expecting \"case\" or \"default\" or \"}\""),
    ("switch ( $a ) { case 1 ; 2 }", "Parse error: syntax error, unexpected token \"}\""),
    ("switch ( $a ) { case 1 2 : }", "Parse error: syntax error, unexpected integer \"2\""),
    ("switch ( $a ) 1 { }", "Parse error: syntax error, unexpected integer \"1\", expecting \":\" or \"{\""),
    ("switch 1 ( $a ) { }", "Parse error: syntax error, unexpected integer \"1\", expecting \"(\""),
    ("throw 1 2 ;", "Parse error: syntax error, unexpected integer \"2\""),
    ("trait T 1 { }", "Parse error: syntax error, unexpected integer \"1\", expecting \"{\""),
    ("try { } 1 catch ( E $e ) { }", "Parse error: syntax error, unexpected token \"catch\""),
    ("try { } catch ( 1 $e ) { }", "Parse error: syntax error, unexpected integer \"1\""),
    ("try { } catch ( E $e ) 1 { }", "Parse error: syntax error, unexpected integer \"1\", expecting \"{\""),
    ("try { } catch ( E 1 ) { }", "Parse error: syntax error, unexpected integer \"1\", expecting \")\""),
    ("try { } catch 1 ( E $e ) { }", "Parse error: syntax error, unexpected integer \"1\", expecting \"(\""),
    ("try { } finally 1 { }", "Parse error: syntax error, unexpected integer \"1\", expecting \"{\""),
    ("try{}catch(A \\ B $e){}", "Parse error: syntax error, unexpected token \"\\\", expecting \")\""),
    ("unset ( $a , 1 ) ;", "Parse error: syntax error, unexpected integer \"1\", expecting \")\""),
    ("unset ( $a 1 ) ;", "Parse error: syntax error, unexpected integer \"1\", expecting \"->\" or \"?->\" or \"[\""),
    ("unset ( 1 $a ) ;", "Parse error: syntax error, unexpected integer \"1\""),
    ("unset 1 ;", "Parse error: syntax error, unexpected integer \"1\", expecting \"(\""),
    ("use A \\ $z B ;", "Parse error: syntax error, unexpected variable \"$z\", expecting \"{\""),
    ("use A \\ 1 B ;", "Parse error: syntax error, unexpected integer \"1\", expecting \"{\""),
    ("use A \\ B ;", "Parse error: syntax error, unexpected identifier \"B\", expecting \"{\""),
    ("use A \\ B ; $z", "Parse error: syntax error, unexpected identifier \"B\", expecting \"{\""),
    ("use A \\ B $z ;", "Parse error: syntax error, unexpected identifier \"B\", expecting \"{\""),
    ("use A \\ B 1 ;", "Parse error: syntax error, unexpected identifier \"B\", expecting \"{\""),
    ("use A 1 ;", "Parse error: syntax error, unexpected integer \"1\", expecting \",\" or \";\""),
    ("use A\\{}; echo 4;", "Parse error: syntax error, unexpected token \"}\", expecting identifier or namespaced name or \"function\" or \"const\""),
    ("use A\\{B \\ C};", "Parse error: syntax error, unexpected token \"\\\", expecting \"}\""),
    ("use A\\B \\ C;", "Parse error: syntax error, unexpected identifier \"C\", expecting \"{\""),
    ("use function 1 ;", "Parse error: syntax error, unexpected integer \"1\", expecting identifier or fully qualified name or namespaced name"),
    ("use function A \\ f;", "Parse error: syntax error, unexpected identifier \"f\", expecting \"{\""),
    ("while 1 ( 1 ) { }", "Parse error: syntax error, unexpected integer \"1\", expecting \"(\""),
];

fn first_diagnostic(src: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_php"))
        .arg("-r")
        .arg(src)
        .stderr(Stdio::null())
        .output()
        .expect("spawn php -r");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let line = text
        .lines()
        .find(|l| l.starts_with("Parse error: ") || l.starts_with("Fatal error: "))
        .unwrap_or("");
    line.trim_end_matches(" in Command line code on line 1")
        .to_string()
}

#[test]
fn a_stray_operand_is_named_with_the_references_expecting_list() {
    let mut wrong = Vec::new();
    for (src, want) in CASES {
        let got = first_diagnostic(src);
        if got != *want {
            wrong.push(format!("{src}\n    want: {want}\n    got:  {got}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} case(s) differ:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}
