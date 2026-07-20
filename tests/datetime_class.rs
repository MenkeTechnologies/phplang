//! The DateTime / DateTimeImmutable / DateInterval prelude classes (UTC, built
//! over the date/strtotime/mktime functions). DateInterval month/year are
//! approximate (30/365 days) rather than calendar-accurate — a documented
//! simplification.

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
