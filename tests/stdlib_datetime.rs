//! Datetime standard-library tests: PHP source in, captured `echo` output out.
//! Every assertion pins an explicit timestamp so results are deterministic and
//! independent of the wall clock / host timezone.
//!
//! FOUR assertions pin this engine rather than the reference. They are listed
//! because a reader is entitled to assume the rest were captured from `php`, and
//! these were re-verified against `php 8.5.9`:
//!
//!   * two OVERFLOW cases — `mktime`/`gmmktime` with a 12-digit field — answer
//!     `false` here where the reference returns a (wrapped) timestamp.
//!   * two `date_default_timezone_set("Not/AZone")` cases: this engine accepts
//!     and stores any name, while the reference emits `Notice: …Timezone ID
//!     'Not/AZone' is invalid`, returns false, and keeps UTC. Already documented
//!     as a LIMITATION in `src/stdlib/datetime.rs`.

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
    // strtotime does the reference's own arithmetic on a huge relative offset
    // (php -r 'var_dump(strtotime("+999999999999 days", 0));').
    assert_eq!(
        run(r#"<?php echo strtotime("+999999999999 days", 0);"#),
        "86399999999913600"
    );
    assert_eq!(
        run(r#"<?php echo strtotime("+999999999999 months", 0);"#),
        "2629745999997235200"
    );
}

#[test]
fn strtotime_empty_string_is_false() {
    // An empty string is a parse failure; an all-blank one is not empty to
    // timelib (its trim keeps the last character) and reads as "now".
    assert_eq!(run(r#"<?php echo strtotime("") === false ? "F":"?";"#), "F");
    assert_eq!(run(r#"<?php echo strtotime("   ", 42);"#), "42");
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

// ── the timezone and expanded-year format characters ─────────────────────────
//
// Ten of `date()`'s format characters used to fall through to the "unknown
// character, emit it verbatim" arm, so `date("T")` answered `"T"`. Nothing
// caught it: every existing assertion in this file names a character that WAS
// implemented, and an unimplemented one is silently its own name rather than an
// error. Each expectation below is the verbatim output of the same call under
// the reference `php` 8.5.9.

#[test]
fn timezone_format_characters_are_the_utc_constants() {
    // php -r 'echo date("e|T|I|Z|O|P|p", 1234567890);'
    assert_eq!(
        run(r#"<?php echo date("e|T|I|Z|O|P|p", 1234567890);"#),
        "UTC|UTC|0|0|+0000|+00:00|Z"
    );
}

#[test]
fn gmdate_names_the_zone_gmt_where_date_names_it_utc() {
    // The single format character on which the two spellings disagree.
    //
    // php -r 'echo gmdate("T"), "|", date("T");'   →  GMT|UTC
    assert_eq!(
        run(r#"<?php echo gmdate("T", 0), "|", date("T", 0);"#),
        "GMT|UTC"
    );
    // `e` agrees: both name the identifier, not the abbreviation.
    assert_eq!(
        run(r#"<?php echo gmdate("e", 0), "|", date("e", 0);"#),
        "UTC|UTC"
    );
}

#[test]
fn expanded_year_signs_differ_between_x_and_lowercase_x() {
    // `X` always carries a sign; `x` carries one only outside 0000-9999. Both
    // pad to four digits. A test using only a modern year would pass with the
    // two implemented identically, so the out-of-range years are the point.
    //
    // php -r 'echo date("X|x", mktime(0,0,0,1,1,2009));'      →  +2009|2009
    // php -r 'echo date("X|x", mktime(0,0,0,1,1,-500));'      →  -0500|-0500
    // php -r 'echo date("X|x", mktime(0,0,0,1,1,12345));'     →  +12345|+12345
    assert_eq!(
        run(r#"<?php echo date("X|x", mktime(0,0,0,1,1,2009));"#),
        "+2009|2009"
    );
    assert_eq!(
        run(r#"<?php echo date("X|x", mktime(0,0,0,1,1,-500));"#),
        "-0500|-0500"
    );
    assert_eq!(
        run(r#"<?php echo date("X|x", mktime(0,0,0,1,1,12345));"#),
        "+12345|+12345"
    );
}

#[test]
fn swatch_internet_time_counts_beats_from_midnight_in_utc_plus_one() {
    // 1000 beats a day, offset one hour ahead of UTC, so it wraps within the
    // day rather than tracking it — 86399 and 0 give the same beat.
    //
    // php -r 'echo date("B",0), "|", date("B",43200), "|", date("B",1234567890), "|", date("B",1700000000);'
    assert_eq!(
        run(
            r#"<?php echo date("B",0), "|", date("B",43200), "|", date("B",1234567890), "|", date("B",1700000000);"#
        ),
        "041|541|021|967"
    );
}

#[test]
fn strtotime_honours_a_trailing_timezone_it_can_apply_exactly() {
    // The absolute formats are exact, so a string carrying a zone matched none
    // of them and the call answered `false`. Values cross-checked against
    // reference PHP 8.5 with TZ=UTC.
    assert_eq!(
        run(r#"<?php var_dump(strtotime('1970-01-02 UTC'));"#),
        "int(86400)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(strtotime('2020-01-01 12:00:00 UTC'));"#),
        "int(1577880000)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(strtotime('2020-01-01 12:00:00 GMT'));"#),
        "int(1577880000)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(strtotime('2020-01-01 12:00:00 Z'));"#),
        "int(1577880000)\n"
    );
    // A numeric offset SHIFTS the result; east of UTC is an earlier instant.
    assert_eq!(
        run(r#"<?php var_dump(strtotime('2020-01-01 12:00:00 +0200'));"#),
        "int(1577872800)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(strtotime('2020-01-01 12:00:00 -05:00'));"#),
        "int(1577898000)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(strtotime('2020-01-01 +03'));"#),
        "int(1577826000)\n"
    );
    // No zone, and the relative/`@` forms, are untouched.
    assert_eq!(
        run(r#"<?php var_dump(strtotime('2020-01-01 12:00:00'));"#),
        "int(1577880000)\n"
    );
    assert_eq!(
        run(r#"<?php var_dump(strtotime('+1 day', 0));"#),
        "int(86400)\n"
    );
    assert_eq!(run(r#"<?php var_dump(strtotime('@500'));"#), "int(500)\n");
    assert_eq!(
        run(r#"<?php var_dump(strtotime('garbage'));"#),
        "bool(false)\n"
    );
}

#[test]
fn strtotime_is_timelibs_scanner() {
    // Each expectation is `strtotime($s, BASE)` under the reference (2024-03-01
    // 01:02:03 UTC, a Friday): absolute formats, zones and abbreviations,
    // weekday and "of" relatives, weekday counting, and the scanner's refusals.
    const BASE: i64 = 1709254923;
    let cases: &[(&str, &str)] = &[
        ("2024/01/15", "1705276800"),
        ("01/15/2024", "1705276800"),
        ("15-01-2024", "1705276800"),
        ("15.01.24", "1709305284"),
        ("Jan 15 2024", "1705276800"),
        ("15 January 2024", "1705276800"),
        ("January 15th, 2024", "1705276800"),
        ("2024-W03-2", "1705363200"),
        ("2016.121", "1461974400"),
        ("20240115", "1705276800"),
        ("20160430T175213", "1462038733"),
        ("30/Apr/2016:17:52:13 +0000", "1462038733"),
        ("Sat, 30 Apr 2016 17:52:13 GMT", "1462038733"),
        ("2024-03-01T10:00:00.5+05:30", "1709267400"),
        ("2024-03-01 10:00 EST", "1709305200"),
        ("10:00 GMT+2", "1709280000"),
        ("2024-03-01 10:00 Etc/GMT+5", "1709287200"),
        ("10:30pm", "1709332200"),
        ("12am", "1709251200"),
        ("12:00 a.m.", "1709251200"),
        ("back of 7pm", "1709320500"),
        ("front of 7", "1709275500"),
        ("@1709251200.123", "1709251200"),
        ("monday", "1709510400"),
        ("next monday", "1709510400"),
        ("last friday", "1708646400"),
        ("sunday this week", "1709424000"),
        ("tuesday next week", "1709596800"),
        ("first monday of january 2025", "1736121600"),
        ("last friday of this month", "1711670400"),
        ("last day of next month", "1714438923"),
        ("first day of last month", "1706749323"),
        ("+1 week 2 days", "1710032523"),
        ("3 days ago", "1708995723"),
        ("1 hour ago 30 min", "1709253123"),
        ("+5 weekdays", "1709859723"),
        ("3 weekdays ago", "1708995723"),
        ("next year", "1740790923"),
        ("this week", "1708909323"),
        ("tomorrow noon", "1709380800"),
        ("midnight +1 hour", "1709254800"),
        ("+500 ms", "1709254923"),
        ("-1 week 2 days 4 hours 2 seconds", "1708837325"),
        ("1 jan 2024 10:00 +1 month", "1706781600"),
        ("2024-01-31 +1 month", "1709337600"),
        ("2024-03-31 -1 month", "1709337600"),
        ("foo", "false"),
        ("2024-01-01 xyz", "false"),
        ("10:00 10:00", "false"),
        ("2024-02-30", "1709251200"),
        ("24:00", "1709337600"),
    ];
    for (s, want) in cases {
        let src = format!("<?php $r = strtotime({s:?}, {BASE}); echo $r === false ? 'false' : $r;");
        assert_eq!(run(&src), *want, "strtotime({s:?})");
    }
}
