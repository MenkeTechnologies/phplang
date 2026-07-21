//! Datetime standard-library tests: PHP source in, captured `echo` output out.
//! Every assertion pins an explicit timestamp so results are deterministic and
//! independent of the wall clock / host timezone.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn date_basic_fields_at_epoch() {
    // 1970-01-01 00:00:00 UTC was a Thursday.
    assert_eq!(run(r#"<?php echo date("Y-m-d", 0);"#), "1970-01-01");
    assert_eq!(run(r#"<?php echo date("H:i:s", 0);"#), "00:00:00");
    assert_eq!(run(r#"<?php echo date("D", 0);"#), "Thu");
    assert_eq!(run(r#"<?php echo date("l", 0);"#), "Thursday");
    assert_eq!(run(r#"<?php echo date("N", 0);"#), "4"); // ISO Mon=1..Sun=7
    assert_eq!(run(r#"<?php echo date("w", 0);"#), "4"); // Sun=0..Sat=6
    assert_eq!(run(r#"<?php echo date("U", 0);"#), "0");
}

#[test]
fn date_month_day_and_ordinals() {
    assert_eq!(run(r#"<?php echo date("j n", 0);"#), "1 1");
    assert_eq!(run(r#"<?php echo date("F M", 0);"#), "January Jan");
    assert_eq!(run(r#"<?php echo date("S", 0);"#), "st"); // 1st
    assert_eq!(run(r#"<?php echo date("t", 0);"#), "31"); // days in January
    assert_eq!(run(r#"<?php echo date("L", 0);"#), "0"); // 1970 not leap
    assert_eq!(run(r#"<?php echo date("z", 0);"#), "0"); // day of year, 0-based
    assert_eq!(run(r#"<?php echo date("y", 0);"#), "70");
}

#[test]
fn date_twelve_hour_and_meridiem() {
    // Midnight: 12-hour clock reads 12, meridiem AM.
    assert_eq!(run(r#"<?php echo date("g h", 0);"#), "12 12");
    assert_eq!(run(r#"<?php echo date("A a", 0);"#), "AM am");
    // 13:00 UTC -> G=13, g=1, h=01, A=PM. 946731600 = 2000-01-01 13:00:00 UTC.
    assert_eq!(
        run(r#"<?php echo date("G g h A", 946731600);"#),
        "13 1 01 PM"
    );
}

#[test]
fn date_ordinal_suffix_edge_cases() {
    // 11th/12th/13th are all "th" despite ending in 1/2/3.
    // 2000-01-11 00:00:00 UTC = 947548800.
    assert_eq!(run(r#"<?php echo date("jS", 947548800);"#), "11th");
    // 2000-01-21 = 948412800 -> 21st.
    assert_eq!(run(r#"<?php echo date("jS", 948412800);"#), "21st");
    // 2000-01-22 = 948499200 -> 22nd.
    assert_eq!(run(r#"<?php echo date("jS", 948499200);"#), "22nd");
    // 2000-01-23 = 948585600 -> 23rd.
    assert_eq!(run(r#"<?php echo date("jS", 948585600);"#), "23rd");
}

#[test]
fn date_escaping_and_literals() {
    assert_eq!(run(r#"<?php echo date("Y\\Y", 0);"#), "1970Y");
    assert_eq!(run(r#"<?php echo date("H:i", 0);"#), "00:00");
}

#[test]
fn gmdate_matches_date_in_utc() {
    assert_eq!(
        run(r#"<?php echo gmdate("Y-m-d H:i:s", 0);"#),
        "1970-01-01 00:00:00"
    );
    assert_eq!(
        run(r#"<?php echo gmdate("Y-m-d", 946684800);"#),
        "2000-01-01"
    );
}

#[test]
fn mktime_and_gmmktime() {
    // 2000-01-01 00:00:00 UTC.
    assert_eq!(run(r#"<?php echo mktime(0,0,0,1,1,2000);"#), "946684800");
    assert_eq!(run(r#"<?php echo gmmktime(0,0,0,1,1,2000);"#), "946684800");
    // Month overflow: month 13 rolls into the next January (2001-01-01).
    assert_eq!(run(r#"<?php echo mktime(0,0,0,13,1,2000);"#), "978307200");
    // Two-digit-year fixup: 70 -> 1970 -> epoch.
    assert_eq!(run(r#"<?php echo mktime(0,0,0,1,1,70);"#), "0");
    // Round-trips through date().
    assert_eq!(
        run(r#"<?php echo date("Y-m-d H:i:s", mktime(13,30,45,6,15,2020));"#),
        "2020-06-15 13:30:45"
    );
}

#[test]
fn mktime_huge_field_returns_false_no_panic() {
    // Bug 1: absurd field values used to panic via chrono overflow. They must
    // now return false (a documented non-crashing deviation from PHP).
    assert_eq!(
        run(r#"<?php echo mktime(0,0,0,1,999999999999,2020) === false ? "F":"?";"#),
        "F"
    );
    assert_eq!(
        run(r#"<?php echo gmmktime(0,0,0,999999999999,1,2020) === false ? "F":"?";"#),
        "F"
    );
    // A relative strtotime offset large enough to overflow must also return false.
    assert_eq!(
        run(r#"<?php echo strtotime("+999999999999 days", 0) === false ? "F":"?";"#),
        "F"
    );
    assert_eq!(
        run(r#"<?php echo strtotime("+999999999999 months", 0) === false ? "F":"?";"#),
        "F"
    );
}

#[test]
fn strtotime_empty_string_is_false() {
    // Bug 2: PHP 8 treats an empty (or whitespace-only) string as a parse failure.
    assert_eq!(run(r#"<?php echo strtotime("") === false ? "F":"?";"#), "F");
    assert_eq!(
        run(r#"<?php echo strtotime("   ") === false ? "F":"?";"#),
        "F"
    );
    // "now" still resolves to the base timestamp.
    assert_eq!(run(r#"<?php echo strtotime("now", 42);"#), "42");
}

#[test]
fn strtotime_month_year_overflow_matches_php() {
    // Bug 3: PHP overflows the day rather than clamping to the last valid day.
    // 2011-01-31 (mktime -> 1296432000) + 1 month => 2011-03-03, not 2011-02-28.
    assert_eq!(
        run(r#"<?php echo date("Y-m-d", strtotime("+1 month", 1296432000));"#),
        "2011-03-03"
    );
    // Exact timestamp PHP produces for the above.
    assert_eq!(
        run(r#"<?php echo strtotime("+1 month", 1296432000);"#),
        "1299110400"
    );
    // Year overflow: 2000-02-29 (leap) + 1 year => 2001-03-01.
    // mktime(0,0,0,2,29,2000) = 951782400.
    assert_eq!(
        run(r#"<?php echo date("Y-m-d", strtotime("+1 year", 951782400));"#),
        "2001-03-01"
    );
}

#[test]
fn date_iso_rfc_and_subsecond_formats() {
    // Bug 4: c, r, o, u, v were previously emitted as literals.
    assert_eq!(
        run(r#"<?php echo date("c", 0);"#),
        "1970-01-01T00:00:00+00:00"
    );
    assert_eq!(
        run(r#"<?php echo date("r", 0);"#),
        "Thu, 01 Jan 1970 00:00:00 +0000"
    );
    // Integer-second timestamps carry no sub-second part -> zeros.
    assert_eq!(run(r#"<?php echo date("u", 0);"#), "000000");
    assert_eq!(run(r#"<?php echo date("v", 0);"#), "000");
    // ISO-8601 week-numbering year: 2005-01-01 belongs to ISO week 53 of 2004.
    // 2005-01-01 00:00:00 UTC = 1104537600.
    assert_eq!(run(r#"<?php echo date("o", 1104537600);"#), "2004");
    assert_eq!(run(r#"<?php echo date("Y", 1104537600);"#), "2005");
    // gmdate honors the same additions.
    assert_eq!(
        run(r#"<?php echo gmdate("c", 0);"#),
        "1970-01-01T00:00:00+00:00"
    );
}

#[test]
fn timezone_set_accepts_unknown_name() {
    // Bug 5 (documented-only): without chrono-tz, unknown names are NOT rejected
    // (PHP returns false); this impl always returns true and only records the name.
    assert_eq!(
        run(r#"<?php echo date_default_timezone_set("Not/AZone") ? "y":"n";"#),
        "y"
    );
    assert_eq!(
        run(r#"<?php date_default_timezone_set("Not/AZone"); echo date_default_timezone_get();"#),
        "Not/AZone"
    );
}

#[test]
fn checkdate_validity() {
    assert_eq!(run(r#"<?php echo checkdate(2,29,2000) ? "y":"n";"#), "y"); // leap
    assert_eq!(run(r#"<?php echo checkdate(2,29,2001) ? "y":"n";"#), "n"); // non-leap
    assert_eq!(run(r#"<?php echo checkdate(4,31,2020) ? "y":"n";"#), "n"); // Apr has 30
    assert_eq!(run(r#"<?php echo checkdate(13,1,2020) ? "y":"n";"#), "n"); // bad month
    assert_eq!(run(r#"<?php echo checkdate(12,31,2020) ? "y":"n";"#), "y");
}

#[test]
fn strtotime_absolute_and_epoch() {
    assert_eq!(run(r#"<?php echo strtotime("1970-01-01");"#), "0");
    assert_eq!(
        run(r#"<?php echo strtotime("2000-01-01 00:00:00");"#),
        "946684800"
    );
    assert_eq!(run(r#"<?php echo strtotime("@12345");"#), "12345");
    assert_eq!(run(r#"<?php echo strtotime("now", 42);"#), "42");
    // Unparseable -> false -> empty echo.
    assert_eq!(
        run(r#"<?php echo strtotime("not a date") === false ? "F":"?";"#),
        "F"
    );
}

#[test]
fn strtotime_relative_offsets() {
    assert_eq!(run(r#"<?php echo strtotime("+1 day", 0);"#), "86400");
    assert_eq!(run(r#"<?php echo strtotime("-1 week", 604800);"#), "0");
    assert_eq!(run(r#"<?php echo strtotime("+1 hour", 0);"#), "3600");
    assert_eq!(run(r#"<?php echo strtotime("+30 minutes", 0);"#), "1800");
    // 1970-01 has 31 days -> +1 month = 31*86400.
    assert_eq!(run(r#"<?php echo strtotime("+1 month", 0);"#), "2678400");
    // +1 year from epoch = 1971-01-01.
    assert_eq!(run(r#"<?php echo strtotime("+1 year", 0);"#), "31536000");
    // Chained tokens accumulate.
    assert_eq!(
        run(r#"<?php echo strtotime("+1 day +1 hour", 0);"#),
        "90000"
    );
    // "ago" suffix negates.
    assert_eq!(run(r#"<?php echo strtotime("1 day ago", 86400);"#), "0");
}

#[test]
fn strtotime_named_days() {
    // Base 2000-01-02 12:00:00 UTC = 946814400.
    assert_eq!(
        run(r#"<?php echo strtotime("today", 946814400);"#),
        "946771200"
    ); // midnight
    assert_eq!(
        run(r#"<?php echo strtotime("yesterday", 946814400);"#),
        "946684800"
    );
    assert_eq!(
        run(r#"<?php echo strtotime("tomorrow", 946814400);"#),
        "946857600"
    );
}

#[test]
fn microtime_float_flag() {
    assert_eq!(
        run(r#"<?php echo is_float(microtime(true)) ? "y":"n";"#),
        "y"
    );
    // Without the flag, PHP returns a "msec sec" string.
    assert_eq!(run(r#"<?php echo is_string(microtime()) ? "y":"n";"#), "y");
    assert_eq!(
        run(r#"<?php echo is_string(microtime(false)) ? "y":"n";"#),
        "y"
    );
}

#[test]
fn timezone_get_set_roundtrip() {
    assert_eq!(run(r#"<?php echo date_default_timezone_get();"#), "UTC");
    assert_eq!(
        run(
            r#"<?php date_default_timezone_set("America/New_York"); echo date_default_timezone_get();"#
        ),
        "America/New_York"
    );
    assert_eq!(
        run(r#"<?php echo date_default_timezone_set("UTC") ? "y":"n";"#),
        "y"
    );
}

#[test]
fn getdate_components() {
    // 2000-01-02 03:04:05 UTC = 946782245.
    assert_eq!(
        run(
            r#"<?php $g = getdate(946782245); echo $g['year']."-".$g['mon']."-".$g['mday']." ".$g['hours'].":".$g['minutes'].":".$g['seconds'];"#
        ),
        "2000-1-2 3:4:5"
    );
    assert_eq!(
        run(r#"<?php $g = getdate(946782245); echo $g['weekday'];"#),
        "Sunday"
    );
    assert_eq!(run(r#"<?php $g = getdate(0); echo $g[0];"#), "0");
}
