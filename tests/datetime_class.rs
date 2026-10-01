//! The DateTime / DateTimeImmutable / DateTimeZone / DateInterval / DatePeriod
//! prelude classes, over the timelib port in `phplang::timelib` and the
//! `php_date.c` routines in `stdlib::datefn`.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn construct_and_format() {
    let src = r#"<?php
        $d = new DateTime("2020-06-15 12:30:45");
        echo $d->format("Y-m-d H:i:s");"#;
    assert_eq!(run(src), "2020-06-15 12:30:45");
}

#[test]
fn get_timestamp() {
    let src = r#"<?php $d = new DateTime("1970-01-02 00:00:00"); echo $d->getTimestamp();"#;
    assert_eq!(run(src), "86400");
}

#[test]
fn modify_mutates() {
    let src = r#"<?php
        $d = new DateTime("2020-01-01");
        $d->modify("+1 day");
        echo $d->format("Y-m-d");"#;
    assert_eq!(run(src), "2020-01-02");
}

#[test]
fn set_date_and_time() {
    let src = r#"<?php
        $d = new DateTime("2020-01-01 00:00:00");
        $d->setDate(1999, 12, 31)->setTime(23, 59, 59);
        echo $d->format("Y-m-d H:i:s");"#;
    assert_eq!(run(src), "1999-12-31 23:59:59");
}

#[test]
fn add_interval_seconds() {
    // PT3600S = 3600 seconds = one hour.
    let src = r#"<?php
        $d = new DateTime("2020-01-01 00:00:00");
        $d->add(new DateInterval("PT1H"));
        echo $d->format("H:i:s");"#;
    assert_eq!(run(src), "01:00:00");
}

#[test]
fn diff_days_and_format() {
    let src = r#"<?php
        $a = new DateTime("2020-01-01");
        $b = new DateTime("2020-01-11");
        $i = $a->diff($b);
        echo $i->days, "|", $i->format("%R%d days");"#;
    assert_eq!(run(src), "10|+10 days");
}

#[test]
fn immutable_returns_new_instance() {
    let src = r#"<?php
        $d = new DateTimeImmutable("2020-01-01");
        $d2 = $d->modify("+1 day");
        echo $d->format("Y-m-d"), "|", $d2->format("Y-m-d");"#;
    // The original is unchanged; modify returns a new object.
    assert_eq!(run(src), "2020-01-01|2020-01-02");
}

#[test]
fn interval_parses_iso_spec() {
    let src = r#"<?php
        $i = new DateInterval("P1Y2M10DT2H30M");
        echo $i->y, ",", $i->m, ",", $i->d, ",", $i->h, ",", $i->i;"#;
    assert_eq!(run(src), "1,2,10,2,30");
}

// ── timelib-backed arithmetic, zones and errors ──────────────────────────────
// Every expectation below is the reference's output for the same program.

#[test]
fn calendar_arithmetic_is_timelibs() {
    // A month added to Jan 31 overflows into March; a diff borrows days
    // against the calendar; an Immutable's leap day plus a year is Mar 1.
    let src = r#"<?php
        $d = new DateTime("2024-01-31 10:00:00");
        $d->add(new DateInterval("P1M"));
        echo $d->format("Y-m-d"), "|";
        $i = (new DateTime("2024-01-01"))->diff(new DateTime("2024-03-15 10:00"));
        echo $i->y, $i->m, " ", $i->d, " ", $i->h, " ", $i->days, " ", $i->format("%R%a %H:%I"), "|";
        echo (new DateTime("2024-03-31"))->sub(new DateInterval("P1M"))->format("Y-m-d"), "|";
        echo (new DateTime("2024-01-01"))->diff(new DateTime("2023-12-25"))->format("%R%a %d"), "|";
        $im = new DateTimeImmutable("2020-02-29");
        echo $im->add(new DateInterval("P1Y"))->format("Y-m-d"), " ",
             $im->modify("last day of next month")->format("Y-m-d"), " ", $im->format("Y-m-d");"#;
    assert_eq!(
        run(src),
        "2024-03-02|02 14 10 74 +74 10:00|2024-03-02|-7 7|2021-03-01 2020-03-31 2020-02-29"
    );
}

#[test]
fn zones_offsets_and_abbreviations() {
    let src = r#"<?php
        $z = new DateTime("2024-06-01 12:00:00", new DateTimeZone("+05:30"));
        echo $z->format(DATE_ATOM), " ", $z->getTimestamp(), " ", $z->getOffset(), " ", $z->getTimezone()->getName(), "|";
        $z->setTimezone(new DateTimeZone("UTC"));
        echo $z->format("c T e"), "|";
        $e = new DateTime("2024-01-01 10:00 EST");
        echo $e->format("c T e Z"), "|";
        print_r($e);"#;
    assert_eq!(
        run(src),
        "2024-06-01T12:00:00+05:30 1717223400 19800 +05:30|2024-06-01T06:30:00+00:00 UTC UTC|\
         2024-01-01T10:00:00-05:00 EST EST -18000|DateTime Object\n(\n    [date] => \
         2024-01-01 10:00:00.000000\n    [timezone_type] => 2\n    [timezone] => EST\n)\n"
    );
}

#[test]
fn refusals_are_the_reference_exceptions() {
    let src = r#"<?php
        try { new DateTime("not a date"); } catch (DateMalformedStringException $ex) { echo $ex->getMessage(), " @", $ex->getLine(), "|"; }
        try { (new DateTime("2020-01-01"))->modify("foo"); } catch (DateMalformedStringException $ex) { echo $ex->getMessage(), "|"; }
        try { new DateInterval("bogus"); } catch (DateMalformedIntervalStringException $ex) { echo $ex->getMessage(), "|"; }
        try { new DateTimeZone("Nowhere/Nothing"); } catch (DateInvalidTimeZoneException $ex) { echo $ex->getMessage(), "|"; }
        var_dump(date_create("garbage"));"#;
    assert_eq!(
        run(src),
        "Failed to parse time string (not a date) at position 0 (n): The timezone could not be found \
         in the database @2|DateTime::modify(): Failed to parse time string (foo) at position 0 (f): \
         The timezone could not be found in the database|Unknown or bad format (bogus)|\
         DateTimeZone::__construct(): Unknown or bad timezone (Nowhere/Nothing)|bool(false)\n"
    );
}

#[test]
fn dates_compare_by_instant_and_periods_iterate() {
    let src = r#"<?php
        $a = new DateTime("2024-01-01 05:00 +05:00");
        $b = new DateTimeImmutable("2024-01-01 00:00 UTC");
        var_dump($a == $b, $a < new DateTime("2024-01-02"), max($a, new DateTime("2023-01-01")) === $a);
        $p = new DatePeriod(new DateTimeImmutable("2024-01-30"), new DateInterval("P1M"),
                            new DateTime("2024-05-01"), DatePeriod::EXCLUDE_START_DATE);
        foreach ($p as $k => $d) echo $k, get_class($d), $d->format(" Y-m-d"), "|";
        echo DateTime::createFromFormat("!d/m/Y", "05/06/2024")->format("c"), "|",
             DateInterval::createFromDateString("3 days")->d;"#;
    assert_eq!(
        run(src),
        "bool(true)\nbool(true)\nbool(true)\n0DateTimeImmutable 2024-03-01|1DateTimeImmutable 2024-04-01|\
         2024-06-05T00:00:00+00:00|3"
    );
}
