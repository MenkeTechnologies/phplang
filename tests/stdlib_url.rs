//! End-to-end tests for the `url` stdlib category: percent-encoding, query
//! assembly/parsing, and URL decomposition. Expected values were cross-checked
//! against PHP 8.5 `php -r`; the assertions here run headless without `php`.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn urlencode_vs_rawurlencode_space_and_tilde() {
    // urlencode: space -> '+', tilde escaped; rawurlencode: space -> %20, tilde kept.
    assert_eq!(run(r#"<?php echo urlencode("a b");"#), "a+b");
    assert_eq!(run(r#"<?php echo rawurlencode("a b");"#), "a%20b");
    assert_eq!(
        run(r#"<?php echo urlencode("a b~c!.*");"#),
        "a+b%7Ec%21.%2A"
    );
    assert_eq!(
        run(r#"<?php echo rawurlencode("a b~c!.*");"#),
        "a%20b~c%21.%2A"
    );
    // Unreserved set A-Za-z0-9-_. passes through untouched.
    assert_eq!(run(r#"<?php echo urlencode("A_z-9.x");"#), "A_z-9.x");
}

#[test]
fn urldecode_vs_rawurldecode_plus() {
    // '+' becomes a space only for urldecode; both decode %XX.
    assert_eq!(run(r#"<?php echo urldecode("a+b%20c");"#), "a b c");
    assert_eq!(run(r#"<?php echo rawurldecode("a+b%20c");"#), "a+b c");
    // A stray '%' with no valid hex pair is emitted verbatim.
    assert_eq!(
        run(r#"<?php echo rawurldecode("100%25 done");"#),
        "100% done"
    );
    // Round-trips.
    assert_eq!(
        run(r#"<?php echo urldecode(urlencode("x y&z=1"));"#),
        "x y&z=1"
    );
    assert_eq!(
        run(r#"<?php echo rawurldecode(rawurlencode("/a b/?q"));"#),
        "/a b/?q"
    );
}

#[test]
fn http_build_query_basic_bool_and_null() {
    // true->1, false->0, null skipped, space via '+'.
    assert_eq!(
        run(r#"<?php echo http_build_query(["a"=>"1 2","b"=>true,"c"=>false,"d"=>null]);"#),
        "a=1+2&b=1&c=0"
    );
}

#[test]
fn http_build_query_nested_and_separator() {
    // Nested arrays produce url-encoded bracket keys.
    assert_eq!(
        run(r#"<?php echo http_build_query(["x"=>["a","b"]]);"#),
        "x%5B0%5D=a&x%5B1%5D=b"
    );
    assert_eq!(
        run(r#"<?php echo http_build_query(["n"=>["a"=>"b","c"=>"d"]]);"#),
        "n%5Ba%5D=b&n%5Bc%5D=d"
    );
    // Custom argument separator (3rd arg).
    assert_eq!(
        run(r#"<?php echo http_build_query(["a"=>1,"b"=>2], "", ";");"#),
        "a=1;b=2"
    );
}

#[test]
fn http_build_query_numeric_prefix() {
    // Integer top-level keys get the numeric prefix prepended.
    assert_eq!(
        run(r#"<?php echo http_build_query(["a","b"], "p_");"#),
        "p_0=a&p_1=b"
    );
}

#[test]
fn parse_url_full_authority() {
    let src = r#"<?php $u = parse_url("https://u:p@host:8080/path?q=1#frag");
        echo $u["scheme"],"|",$u["host"],"|",$u["port"],"|",$u["user"],"|",
             $u["pass"],"|",$u["path"],"|",$u["query"],"|",$u["fragment"];"#;
    assert_eq!(run(src), "https|host|8080|u|p|/path|q=1|frag");
}

#[test]
fn parse_url_port_is_integer() {
    // The port key must be an int, not a string, so json_encode has no quotes.
    assert_eq!(
        run(r#"<?php echo json_encode(parse_url("http://h:80/"));"#),
        r#"{"scheme":"http","host":"h","port":80,"path":"\/"}"#
    );
}

#[test]
fn parse_url_ipv6_host() {
    let src = r#"<?php $u = parse_url("http://[::1]:8080/p");
        echo $u["host"],"|",$u["port"],"|",$u["path"];"#;
    assert_eq!(run(src), "[::1]|8080|/p");
}

#[test]
fn parse_url_scheme_only_and_mailto() {
    assert_eq!(
        run(r#"<?php echo json_encode(parse_url("mailto:foo@bar.com"));"#),
        r#"{"scheme":"mailto","path":"foo@bar.com"}"#
    );
    assert_eq!(
        run(r#"<?php echo json_encode(parse_url("//example.com"));"#),
        r#"{"host":"example.com"}"#
    );
}

#[test]
fn parse_url_relative_and_empty_path() {
    assert_eq!(
        run(r#"<?php echo json_encode(parse_url("path/only"));"#),
        r#"{"path":"path\/only"}"#
    );
    // Empty input yields an empty path (not false).
    assert_eq!(
        run(r#"<?php echo json_encode(parse_url(""));"#),
        r#"{"path":""}"#
    );
}

#[test]
fn parse_url_malformed_returns_false() {
    // Empty host with a port is rejected as false by PHP.
    assert_eq!(
        run(r#"<?php var_export(parse_url("http://:80"));"#),
        "false"
    );
}

#[test]
fn parse_url_component_selector() {
    // PHP_URL_PORT == 2 -> the port int; PHP_URL_QUERY == 6, absent -> null.
    assert_eq!(
        run(r#"<?php echo parse_url("https://h:8080/p", 2);"#),
        "8080"
    );
    assert_eq!(
        run(r#"<?php var_dump(parse_url("https://h/p", 6));"#),
        "NULL\n"
    );
    assert_eq!(run(r#"<?php echo parse_url("https://h/p?a=b", 6);"#), "a=b");
}

#[test]
fn parse_str_arrays_and_key_mangling() {
    // '[]' appends, '[k]' nests, '.'/' ' in the base key become '_'.
    let src = r#"<?php parse_str("a=1&b[]=2&b[]=3&c.d=4", $r);
        echo $r["a"],"|",$r["b"][0],"|",$r["b"][1],"|",$r["c_d"];"#;
    assert_eq!(run(src), "1|2|3|4");
    let src2 = r#"<?php parse_str("a b=c d&x[y]=z", $r);
        echo $r["a_b"],"|",$r["x"]["y"];"#;
    assert_eq!(run(src2), "c d|z");
}

#[test]
fn parse_str_decodes_percent_and_plus() {
    let src = r#"<?php parse_str("q=a%20b+c&e=", $r);
        echo $r["q"],"|",$r["e"],"|",strlen($r["e"]);"#;
    assert_eq!(run(src), "a b c||0");
}

#[test]
fn parse_url_port_with_trailing_junk() {
    // strtol semantics: a port with trailing non-digits keeps the leading digits
    // ("80abc" -> 80) instead of rejecting the whole URL as false.
    assert_eq!(
        run(r#"<?php echo json_encode(parse_url("http://host:80abc/"));"#),
        r#"{"scheme":"http","host":"host","port":80,"path":"\/"}"#
    );
}

#[test]
fn http_build_query_explicit_empty_separator() {
    // An explicit empty separator concatenates pairs with no delimiter; only a
    // missing/null separator falls back to "&".
    assert_eq!(
        run(r#"<?php echo http_build_query(["a"=>1,"b"=>2], "", "");"#),
        "a=1b=2"
    );
    // Default (missing) separator still uses "&".
    assert_eq!(
        run(r#"<?php echo http_build_query(["a"=>1,"b"=>2]);"#),
        "a=1&b=2"
    );
}

#[test]
fn parse_str_strips_leading_plus_and_space() {
    // A leading '+' (decodes to a space) is stripped from the base key, not
    // mangled to '_': "+a" -> "a".
    let src = r#"<?php parse_str("+a=1", $r); echo json_encode($r);"#;
    assert_eq!(run(src), r#"{"a":"1"}"#);
    // A leading literal space is likewise stripped; interior space still mangles.
    let src2 = r#"<?php parse_str("%20x y=2", $r); echo json_encode($r);"#;
    assert_eq!(run(src2), r#"{"x_y":"2"}"#);
}

#[test]
fn parse_str_nested_deep() {
    let src = r#"<?php parse_str("a[b][c]=1", $r);
        echo json_encode($r);"#;
    assert_eq!(run(src), r#"{"a":{"b":{"c":"1"}}}"#);
}

// ── PHP_URL_* component selectors ────────────────────────────────────────────

/// The eight `$component` selectors, each pulled off one fully-populated URL.
///
/// The ordinals were always honoured; the CONSTANTS naming them were never
/// seeded, so `parse_url($u, PHP_URL_HOST)` died on `Undefined constant
/// "PHP_URL_HOST"` — the spelling every PHP program actually uses.
#[test]
fn parse_url_component_constants() {
    let u = "https://user:pw@host:8080/path?q=1#frag";
    let one = |c: &str| run(&format!(r#"<?php var_dump(parse_url("{u}", {c}));"#));
    assert_eq!(one("PHP_URL_SCHEME"), "string(5) \"https\"\n");
    assert_eq!(one("PHP_URL_HOST"), "string(4) \"host\"\n");
    assert_eq!(one("PHP_URL_PORT"), "int(8080)\n");
    assert_eq!(one("PHP_URL_USER"), "string(4) \"user\"\n");
    assert_eq!(one("PHP_URL_PASS"), "string(2) \"pw\"\n");
    assert_eq!(one("PHP_URL_PATH"), "string(5) \"/path\"\n");
    assert_eq!(one("PHP_URL_QUERY"), "string(3) \"q=1\"\n");
    assert_eq!(one("PHP_URL_FRAGMENT"), "string(4) \"frag\"\n");
    // A component the URL does not carry reads back as null, not "".
    assert_eq!(
        run(
            r#"<?php var_dump(parse_url("http://h/p", PHP_URL_PORT), parse_url("http://h/p", PHP_URL_QUERY));"#
        ),
        "NULL\nNULL\n"
    );
}

/// Only a component ABOVE `PHP_URL_FRAGMENT` is out of range. The reference
/// branches on `key > -1`, so every negative value — not just the documented
/// `-1` — asks for the whole array.
#[test]
fn parse_url_component_range() {
    assert_eq!(
        run(
            r#"<?php try { parse_url("http://h/p", 8); } catch (ValueError $e) { echo $e->getMessage(); }"#
        ),
        "parse_url(): Argument #2 ($component) must be a valid URL component identifier, 8 given"
    );
    assert_eq!(
        run(r#"<?php echo implode(",", array_keys(parse_url("http://h/p", -2)));"#),
        "scheme,host,path"
    );
    // The range check runs BEFORE the parse, so a bad component beats a bad URL.
    assert_eq!(
        run(r#"<?php try { parse_url("::", 9); } catch (ValueError $e) { echo "caught"; }"#),
        "caught"
    );
}
