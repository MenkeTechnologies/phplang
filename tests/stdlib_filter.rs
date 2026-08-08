//! End-to-end tests for the `filter` stdlib category: `filter_var` validation
//! and sanitization filters plus `filter_var_array`. Expected values were
//! cross-checked against PHP 8.5 `php -r`; the assertions here run headless
//! without `php`.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

// ── FILTER_VALIDATE_INT ──────────────────────────────────────────────────────

#[test]
fn validate_int_basic_and_typing() {
    // Success returns a real int (no quotes under json_encode); whitespace and a
    // leading sign are allowed.
    assert_eq!(
        run(r#"<?php echo json_encode(filter_var("123", FILTER_VALIDATE_INT));"#),
        "123"
    );
    assert_eq!(
        run(r#"<?php echo filter_var(" 42 ", FILTER_VALIDATE_INT);"#),
        "42"
    );
    assert_eq!(
        run(r#"<?php echo filter_var("-5", FILTER_VALIDATE_INT);"#),
        "-5"
    );
    // Leading zeros ("007") are rejected as false, matching PHP.
    assert_eq!(
        run(r#"<?php var_export(filter_var("007", FILTER_VALIDATE_INT));"#),
        "false"
    );
    assert_eq!(
        run(r#"<?php var_export(filter_var("12.5", FILTER_VALIDATE_INT));"#),
        "false"
    );
    assert_eq!(
        run(r#"<?php var_export(filter_var("abc", FILTER_VALIDATE_INT));"#),
        "false"
    );
}

#[test]
fn validate_int_i64_min_boundary() {
    // The magnitude of i64::MIN overflows a positive i64; parsing the full signed
    // token (not sign-then-magnitude) keeps this valid, matching PHP.
    assert_eq!(
        run(r#"<?php echo filter_var("-9223372036854775808", FILTER_VALIDATE_INT);"#),
        "-9223372036854775808"
    );
    // i64::MAX validates; one past it overflows to false.
    assert_eq!(
        run(r#"<?php echo filter_var("9223372036854775807", FILTER_VALIDATE_INT);"#),
        "9223372036854775807"
    );
    assert_eq!(
        run(r#"<?php var_export(filter_var("9223372036854775808", FILTER_VALIDATE_INT));"#),
        "false"
    );
}

#[test]
fn validate_int_signed_and_zero_forms() {
    // "0", "+0", "-0" all validate to 0 (a lone zero is not a leading-zero error).
    assert_eq!(
        run(r#"<?php echo filter_var("0", FILTER_VALIDATE_INT);"#),
        "0"
    );
    assert_eq!(
        run(r#"<?php echo filter_var("+0", FILTER_VALIDATE_INT);"#),
        "0"
    );
    assert_eq!(
        run(r#"<?php echo filter_var("-0", FILTER_VALIDATE_INT);"#),
        "0"
    );
    assert_eq!(
        run(r#"<?php echo filter_var("+42", FILTER_VALIDATE_INT);"#),
        "42"
    );
    // A non-ASCII whitespace prefix (NBSP) is NOT trimmed by PHP and must fail.
    assert_eq!(
        run("<?php var_export(filter_var(\"\u{a0}42\", FILTER_VALIDATE_INT));"),
        "false"
    );
}

#[test]
fn validate_int_hex_octal_flags() {
    // ALLOW_HEX accepts an unsigned 0x prefix; a signed hex form falls through to
    // the decimal parser and fails (PHP checks the raw first byte).
    assert_eq!(
        run(r#"<?php echo filter_var("0x1A", FILTER_VALIDATE_INT, FILTER_FLAG_ALLOW_HEX);"#),
        "26"
    );
    assert_eq!(
        run(
            r#"<?php var_export(filter_var("-0x1A", FILTER_VALIDATE_INT, FILTER_FLAG_ALLOW_HEX));"#
        ),
        "false"
    );
    // Bare "0x" has no digits.
    assert_eq!(
        run(r#"<?php var_export(filter_var("0x", FILTER_VALIDATE_INT, FILTER_FLAG_ALLOW_HEX));"#),
        "false"
    );
    // ALLOW_OCTAL: "010" is octal 8; a digit outside 0-7 fails.
    assert_eq!(
        run(r#"<?php echo filter_var("010", FILTER_VALIDATE_INT, FILTER_FLAG_ALLOW_OCTAL);"#),
        "8"
    );
    assert_eq!(
        run(r#"<?php var_export(filter_var("08", FILTER_VALIDATE_INT, FILTER_FLAG_ALLOW_OCTAL));"#),
        "false"
    );
}

#[test]
fn validate_int_min_max_range() {
    let ok = r#"<?php echo filter_var("5", FILTER_VALIDATE_INT, ["options"=>["min_range"=>1,"max_range"=>10]]);"#;
    assert_eq!(run(ok), "5");
    let bad = r#"<?php var_export(filter_var("50", FILTER_VALIDATE_INT, ["options"=>["min_range"=>1,"max_range"=>10]]));"#;
    assert_eq!(run(bad), "false");
}

#[test]
fn validate_int_null_on_failure() {
    // FILTER_NULL_ON_FAILURE swaps the false sentinel for null.
    assert_eq!(
        run(r#"<?php var_dump(filter_var("nope", FILTER_VALIDATE_INT, FILTER_NULL_ON_FAILURE));"#),
        "NULL\n"
    );
}

// ── FILTER_VALIDATE_FLOAT ────────────────────────────────────────────────────

#[test]
fn validate_float_and_thousand_flag() {
    assert_eq!(
        run(r#"<?php echo filter_var("12.5", FILTER_VALIDATE_FLOAT);"#),
        "12.5"
    );
    // The thousands separator is only accepted with FILTER_FLAG_ALLOW_THOUSAND.
    assert_eq!(
        run(
            r#"<?php echo filter_var("1,234.5", FILTER_VALIDATE_FLOAT, FILTER_FLAG_ALLOW_THOUSAND);"#
        ),
        "1234.5"
    );
    assert_eq!(
        run(r#"<?php var_export(filter_var("1,234.5", FILTER_VALIDATE_FLOAT));"#),
        "false"
    );
    assert_eq!(
        run(r#"<?php var_export(filter_var("x", FILTER_VALIDATE_FLOAT));"#),
        "false"
    );
    // Scientific notation is always accepted (no flag needed), matching PHP.
    assert_eq!(
        run(r#"<?php echo filter_var("1.5e3", FILTER_VALIDATE_FLOAT);"#),
        "1500"
    );
    // "inf"/"nan" look numeric to Rust's parser but PHP's float filter rejects them.
    assert_eq!(
        run(r#"<?php var_export(filter_var("inf", FILTER_VALIDATE_FLOAT));"#),
        "false"
    );
    assert_eq!(
        run(r#"<?php var_export(filter_var("nan", FILTER_VALIDATE_FLOAT));"#),
        "false"
    );
    // A lone thousands separator collapses to empty and must fail, not parse "".
    assert_eq!(
        run(
            r#"<?php var_export(filter_var(",", FILTER_VALIDATE_FLOAT, FILTER_FLAG_ALLOW_THOUSAND));"#
        ),
        "false"
    );
}

// ── FILTER_VALIDATE_BOOLEAN ──────────────────────────────────────────────────

#[test]
fn validate_boolean_recognized_values() {
    assert_eq!(
        run(r#"<?php var_export(filter_var("yes", FILTER_VALIDATE_BOOLEAN));"#),
        "true"
    );
    assert_eq!(
        run(r#"<?php var_export(filter_var("on", FILTER_VALIDATE_BOOLEAN));"#),
        "true"
    );
    assert_eq!(
        run(r#"<?php var_export(filter_var("1", FILTER_VALIDATE_BOOLEAN));"#),
        "true"
    );
    assert_eq!(
        run(r#"<?php var_export(filter_var("no", FILTER_VALIDATE_BOOLEAN));"#),
        "false"
    );
    assert_eq!(
        run(r#"<?php var_export(filter_var("off", FILTER_VALIDATE_BOOLEAN));"#),
        "false"
    );
    // The BOOL alias resolves to the same filter.
    assert_eq!(
        run(r#"<?php var_export(filter_var("true", FILTER_VALIDATE_BOOL));"#),
        "true"
    );
}

#[test]
fn validate_boolean_unrecognized_and_null_on_failure() {
    // Unrecognized is false by default, null under FILTER_NULL_ON_FAILURE.
    assert_eq!(
        run(r#"<?php var_export(filter_var("maybe", FILTER_VALIDATE_BOOLEAN));"#),
        "false"
    );
    assert_eq!(
        run(
            r#"<?php var_dump(filter_var("maybe", FILTER_VALIDATE_BOOLEAN, FILTER_NULL_ON_FAILURE));"#
        ),
        "NULL\n"
    );
}

// ── FILTER_VALIDATE_EMAIL / URL / IP / DOMAIN / REGEXP ───────────────────────

#[test]
fn validate_email() {
    assert_eq!(
        run(r#"<?php echo filter_var("foo.bar@sub.example.co.uk", FILTER_VALIDATE_EMAIL);"#),
        "foo.bar@sub.example.co.uk"
    );
    // A dotless domain and a malformed address are both rejected (PHP 8.5).
    assert_eq!(
        run(r#"<?php var_export(filter_var("foo@localhost", FILTER_VALIDATE_EMAIL));"#),
        "false"
    );
    assert_eq!(
        run(r#"<?php var_export(filter_var("not-an-email", FILTER_VALIDATE_EMAIL));"#),
        "false"
    );
}

#[test]
fn validate_url() {
    assert_eq!(
        run(r#"<?php echo filter_var("http://example.com/path?q=1", FILTER_VALIDATE_URL);"#),
        "http://example.com/path?q=1"
    );
    // A schemeless URL is rejected.
    assert_eq!(
        run(r#"<?php var_export(filter_var("example.com", FILTER_VALIDATE_URL));"#),
        "false"
    );
}

#[test]
fn validate_ip_families() {
    assert_eq!(
        run(r#"<?php echo filter_var("127.0.0.1", FILTER_VALIDATE_IP);"#),
        "127.0.0.1"
    );
    assert_eq!(
        run(r#"<?php echo filter_var("::1", FILTER_VALIDATE_IP, FILTER_FLAG_IPV6);"#),
        "::1"
    );
    // An IPv6 address is rejected when only IPv4 is requested.
    assert_eq!(
        run(r#"<?php var_export(filter_var("::1", FILTER_VALIDATE_IP, FILTER_FLAG_IPV4));"#),
        "false"
    );
    assert_eq!(
        run(r#"<?php var_export(filter_var("999.1.1.1", FILTER_VALIDATE_IP));"#),
        "false"
    );
    // Leading zeros in an octet are rejected (matches PHP + std::net).
    assert_eq!(
        run(r#"<?php var_export(filter_var("127.0.0.01", FILTER_VALIDATE_IP));"#),
        "false"
    );
}

// ── FILTER_VALIDATE_MAC ──────────────────────────────────────────────────────

#[test]
fn validate_mac_accepted_forms() {
    assert_eq!(
        run(r#"<?php echo filter_var("00:11:22:33:44:55", FILTER_VALIDATE_MAC);"#),
        "00:11:22:33:44:55"
    );
    assert_eq!(
        run(r#"<?php echo filter_var("aa-bb-cc-dd-ee-ff", FILTER_VALIDATE_MAC);"#),
        "aa-bb-cc-dd-ee-ff"
    );
    // Cisco dotted-quad (three groups of four hex digits).
    assert_eq!(
        run(r#"<?php echo filter_var("0011.2233.4455", FILTER_VALIDATE_MAC);"#),
        "0011.2233.4455"
    );
}

#[test]
fn validate_mac_rejects_malformed() {
    // Mixed separators fail.
    assert_eq!(
        run(r#"<?php var_export(filter_var("00:11-22:33:44:55", FILTER_VALIDATE_MAC));"#),
        "false"
    );
    // A non-hex digit fails.
    assert_eq!(
        run(r#"<?php var_export(filter_var("00:11:22:33:44:GG", FILTER_VALIDATE_MAC));"#),
        "false"
    );
    // Wrong length (no separators) fails.
    assert_eq!(
        run(r#"<?php var_export(filter_var("001122334455", FILTER_VALIDATE_MAC));"#),
        "false"
    );
    assert_eq!(
        run(r#"<?php var_export(filter_var("", FILTER_VALIDATE_MAC));"#),
        "false"
    );
}

#[test]
fn validate_domain_hostname_flag() {
    assert_eq!(
        run(r#"<?php echo filter_var("example.com", FILTER_VALIDATE_DOMAIN);"#),
        "example.com"
    );
    assert_eq!(
        run(r#"<?php echo filter_var("good.com", FILTER_VALIDATE_DOMAIN, FILTER_FLAG_HOSTNAME);"#),
        "good.com"
    );
    // A leading hyphen is only rejected under FILTER_FLAG_HOSTNAME.
    assert_eq!(
        run(
            r#"<?php var_export(filter_var("-bad.com", FILTER_VALIDATE_DOMAIN, FILTER_FLAG_HOSTNAME));"#
        ),
        "false"
    );
}

#[test]
fn validate_regexp() {
    let ok = r#"<?php echo filter_var("abc123", FILTER_VALIDATE_REGEXP, ["options"=>["regexp"=>"/^[a-z]+[0-9]+$/"]]);"#;
    assert_eq!(run(ok), "abc123");
    let bad = r#"<?php var_export(filter_var("ABC", FILTER_VALIDATE_REGEXP, ["options"=>["regexp"=>"/^[a-z]+$/"]]));"#;
    assert_eq!(run(bad), "false");
    // The `i` modifier is honored.
    let ci = r#"<?php echo filter_var("ABC", FILTER_VALIDATE_REGEXP, ["options"=>["regexp"=>"/^[a-z]+$/i"]]);"#;
    assert_eq!(run(ci), "ABC");
}

#[test]
fn validate_regexp_multibyte_delimiter_no_panic() {
    // A multi-byte first char is an invalid delimiter; the compiler must reject
    // it (return false) rather than panic slicing mid-codepoint.
    let src = r#"<?php var_export(filter_var("x", FILTER_VALIDATE_REGEXP, ["options"=>["regexp"=>"é^x$é"]]));"#;
    assert_eq!(run(src), "false");
}

// ── sanitizers ───────────────────────────────────────────────────────────────

#[test]
fn sanitize_full_special_chars() {
    assert_eq!(
        run(r#"<?php echo filter_var("<b>hi</b>", FILTER_SANITIZE_FULL_SPECIAL_CHARS);"#),
        "&lt;b&gt;hi&lt;/b&gt;"
    );
    assert_eq!(
        run(r#"<?php echo filter_var("a&\"b", FILTER_SANITIZE_FULL_SPECIAL_CHARS);"#),
        "a&amp;&quot;b"
    );
}

#[test]
fn sanitize_special_chars_numeric_entities() {
    assert_eq!(
        run(r#"<?php echo filter_var("a<b>&", FILTER_SANITIZE_SPECIAL_CHARS);"#),
        "a&#60;b&#62;&#38;"
    );
}

#[test]
fn sanitize_string_strips_tags() {
    // strip_tags then encode quotes.
    assert_eq!(
        run(r#"<?php echo filter_var("<b>hi</b>", FILTER_SANITIZE_STRING);"#),
        "hi"
    );
    assert_eq!(
        run(r#"<?php echo filter_var("say \"x\"", FILTER_SANITIZE_STRING);"#),
        "say &#34;x&#34;"
    );
}

#[test]
fn sanitize_email_and_url() {
    // Spaces and disallowed chars are stripped, allowed specials kept.
    assert_eq!(
        run(r#"<?php echo filter_var("a b<c>@ex!ample.com", FILTER_SANITIZE_EMAIL);"#),
        "abc@ex!ample.com"
    );
    assert_eq!(
        run(r#"<?php echo filter_var("http://a b.com/pa th<>", FILTER_SANITIZE_URL);"#),
        "http://ab.com/path<>"
    );
}

#[test]
fn sanitize_number_int_and_float() {
    assert_eq!(
        run(r#"<?php echo filter_var("a-1b2c+3", FILTER_SANITIZE_NUMBER_INT);"#),
        "-12+3"
    );
    // Default float sanitize keeps only digits and signs.
    assert_eq!(
        run(r#"<?php echo filter_var("a1.5e3,2x", FILTER_SANITIZE_NUMBER_FLOAT);"#),
        "1532"
    );
    // With ALLOW_FRACTION the decimal point survives.
    assert_eq!(
        run(
            r#"<?php echo filter_var("a1.5e3,2x", FILTER_SANITIZE_NUMBER_FLOAT, FILTER_FLAG_ALLOW_FRACTION);"#
        ),
        "1.532"
    );
}

#[test]
fn filter_default_passthrough() {
    // FILTER_DEFAULT does no filtering.
    assert_eq!(
        run(r#"<?php echo filter_var("hello world", FILTER_DEFAULT);"#),
        "hello world"
    );
    // No filter argument also means FILTER_DEFAULT.
    assert_eq!(run(r#"<?php echo filter_var("plain");"#), "plain");
}

// ── filter_var_array ─────────────────────────────────────────────────────────

#[test]
fn filter_var_array_per_field_specs() {
    let src = r#"<?php echo json_encode(filter_var_array(
        ["age"=>"25","email"=>"x@y.com","name"=>"<b>bob</b>"],
        ["age"=>FILTER_VALIDATE_INT,"email"=>FILTER_VALIDATE_EMAIL,"name"=>FILTER_SANITIZE_FULL_SPECIAL_CHARS]
    ));"#;
    assert_eq!(
        run(src),
        // json_encode escapes "/" as "\/", matching the reference interpreter.
        r#"{"age":25,"email":"x@y.com","name":"&lt;b&gt;bob&lt;\/b&gt;"}"#
    );
}

#[test]
fn filter_var_array_single_filter_for_all() {
    assert_eq!(
        run(
            r#"<?php echo json_encode(filter_var_array(["a"=>"1","b"=>"2"], FILTER_VALIDATE_INT));"#
        ),
        r#"{"a":1,"b":2}"#
    );
}

#[test]
fn filter_var_array_missing_field_is_null() {
    // A field named in the definition but absent from the data is null, not false.
    assert_eq!(
        run(
            r#"<?php echo json_encode(filter_var_array(["a"=>"1"], ["a"=>FILTER_VALIDATE_INT,"b"=>FILTER_VALIDATE_INT]));"#
        ),
        r#"{"a":1,"b":null}"#
    );
}

#[test]
fn filter_var_array_config_array_spec() {
    // A per-field config array carries its own filter + options.
    let src = r#"<?php echo json_encode(filter_var_array(
        ["n"=>"50"],
        ["n"=>["filter"=>FILTER_VALIDATE_INT,"options"=>["min_range"=>1,"max_range"=>10]]]
    ));"#;
    assert_eq!(run(src), r#"{"n":false}"#);
}

// ── filter_has_var ───────────────────────────────────────────────────────────

#[test]
fn filter_has_var_always_false() {
    // No request superglobals exist in a standalone runtime, so nothing is ever
    // present regardless of input type or name.
    assert_eq!(
        run(r#"<?php var_export(filter_has_var(INPUT_GET, "q"));"#),
        "false"
    );
    assert_eq!(
        run(r#"<?php var_export(filter_has_var(INPUT_POST, "token"));"#),
        "false"
    );
}
