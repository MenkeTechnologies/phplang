//! Tests for the `stdlib::strings` category. Every expected value mirrors the
//! output of the reference PHP 8 function of the same name. These run headless
//! (no `php` on PATH required).

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn substr_count_and_replace() {
    assert_eq!(run(r#"<?php echo substr_count("hello world", "o");"#), "2");
    assert_eq!(run(r#"<?php echo substr_count("ababab", "ab");"#), "3");
    // Non-overlapping, like PHP.
    assert_eq!(run(r#"<?php echo substr_count("aaa", "aa");"#), "1");
    assert_eq!(run(r#"<?php echo substr_count("hello world", "o", 5);"#), "1");
    assert_eq!(run(r#"<?php echo substr_replace("Hello", "World", 0);"#), "World");
    assert_eq!(run(r#"<?php echo substr_replace("Hello World", "PHP", 6);"#), "Hello PHP");
    assert_eq!(run(r#"<?php echo substr_replace("abcdef", "X", 1, 2);"#), "aXdef");
    // Negative offset counts from the end.
    assert_eq!(run(r#"<?php echo substr_replace("abcdef", "-", -1, 1);"#), "abcde-");
}

#[test]
fn strtr_both_forms() {
    assert_eq!(run(r#"<?php echo strtr("Hi All", "ia", "eu");"#), "He All");
    // Truncates to the shorter of from/to.
    assert_eq!(run(r#"<?php echo strtr("abc", "abc", "xy");"#), "xyc");
    assert_eq!(
        run(r#"<?php echo strtr("Hello World", ["Hello" => "Hi", "World" => "Earth"]);"#),
        "Hi Earth"
    );
    // Longest key wins at a given position.
    assert_eq!(
        run(r#"<?php echo strtr("XY", ["X" => "1", "XY" => "2"]);"#),
        "2"
    );
}

#[test]
fn strstr_family() {
    assert_eq!(run(r#"<?php echo strstr("user@example.com", "@");"#), "@example.com");
    assert_eq!(run(r#"<?php echo strstr("user@example.com", "@", true);"#), "user");
    assert_eq!(run(r#"<?php var_dump(strstr("abc", "x"));"#), "bool(false)\n");
    assert_eq!(run(r#"<?php echo stristr("HELLO", "ell");"#), "ELLO");
    assert_eq!(run(r#"<?php echo strrchr("a/b/c", "/");"#), "/c");
    assert_eq!(run(r#"<?php echo strpbrk("This is a test", "st");"#), "s is a test");
}

#[test]
fn span_functions() {
    assert_eq!(run(r#"<?php echo strspn("42 is the answer", "1234567890");"#), "2");
    assert_eq!(run(r#"<?php echo strcspn("Hello World", "Wd");"#), "6");
    assert_eq!(run(r#"<?php echo strspn("aaabbb", "a");"#), "3");
}

#[test]
fn case_insensitive_positions() {
    assert_eq!(run(r#"<?php echo stripos("Hello World", "o", 5);"#), "7");
    assert_eq!(run(r#"<?php var_dump(stripos("abc", "X"));"#), "bool(false)\n");
    assert_eq!(run(r#"<?php echo strrpos("hello world hello", "hello");"#), "12");
    assert_eq!(run(r#"<?php echo strripos("Hello WORLD", "o");"#), "7");
    assert_eq!(run(r#"<?php echo strncasecmp("Hello", "HELP", 3);"#), "0");
    assert!(run(r#"<?php echo strncasecmp("Hello", "HELP", 4);"#).starts_with('-'));
}

#[test]
fn ireplace_and_escaping() {
    assert_eq!(run(r#"<?php echo str_ireplace("WORLD", "PHP", "Hello world");"#), "Hello PHP");
    assert_eq!(
        run(r#"<?php echo str_ireplace(["a", "B"], ["1", "2"], "AbAb");"#),
        "1212"
    );
    assert_eq!(run(r#"<?php echo quotemeta("1+1=2");"#), r"1\+1=2");
    assert_eq!(run(r#"<?php echo addslashes("O'Reilly");"#), r"O\'Reilly");
    assert_eq!(run(r#"<?php echo stripslashes("O\\'Reilly");"#), "O'Reilly");
    assert_eq!(run(r#"<?php echo str_rot13("Hello");"#), "Uryyb");
    // rot13 is its own inverse.
    assert_eq!(run(r#"<?php echo str_rot13(str_rot13("Sphinx"));"#), "Sphinx");
}

#[test]
fn nl2br_chunk_tags() {
    assert_eq!(run("<?php echo nl2br(\"a\nb\");"), "a<br />\nb");
    assert_eq!(run(r#"<?php echo chunk_split("abcdefgh", 3, "-");"#), "abc-def-gh-");
    assert_eq!(run(r#"<?php echo htmlspecialchars_decode("&lt;a&gt; &amp; &quot;b&quot;");"#), r#"<a> & "b""#);
    assert_eq!(run(r#"<?php echo strip_tags("<b>hi</b> <i>there</i>");"#), "hi there");
}

#[test]
fn distance_metrics() {
    assert_eq!(run(r#"<?php echo similar_text("World", "word");"#), "3");
    assert_eq!(run(r#"<?php echo similar_text("Hello", "Hello");"#), "5");
    assert_eq!(run(r#"<?php echo levenshtein("kitten", "sitting");"#), "3");
    assert_eq!(run(r#"<?php echo levenshtein("", "abc");"#), "3");
    assert_eq!(run(r#"<?php echo levenshtein("abc", "abc");"#), "0");
}

#[test]
fn sprintf_family_and_scan() {
    assert_eq!(run(r#"<?php echo vsprintf("%s has %d apples", ["Bob", 3]);"#), "Bob has 3 apples");
    assert_eq!(run(r#"<?php vprintf("%d-%d", [4, 5]);"#), "4-5");
    assert_eq!(run(r#"<?php $r = sscanf("age:42", "age:%d"); echo $r[0];"#), "42");
    assert_eq!(run(r#"<?php $r = sscanf("x 3.5", "%s %f"); echo $r[0], "|", $r[1];"#), "x|3.5");
}

#[test]
fn multibyte() {
    assert_eq!(run(r#"<?php echo mb_strlen("héllo");"#), "5");
    // Byte strlen counts the 2-byte é as two.
    assert_eq!(run(r#"<?php echo mb_strtoupper("héllo");"#), "HÉLLO");
    assert_eq!(run(r#"<?php echo mb_strtolower("HÉLLO");"#), "héllo");
    assert_eq!(run(r#"<?php echo mb_substr("héllo", 1, 3);"#), "éll");
    assert_eq!(run(r#"<?php echo mb_substr("héllo", -2);"#), "lo");
}
