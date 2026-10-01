//! PHP standard-library `datetime` functions. Part of the `stdlib` chain; see
//! `src/stdlib/mod.rs`. `dispatch` returns `None` for names it does not handle.
//!
//! Date math is delegated to `chrono` (UTC / `NaiveDateTime`). The reference
//! `php` computes `date()` against the process's default timezone; without a tz
//! database (`chrono-tz` is not a dependency) every calculation here runs in UTC,
//! so `date()` and `gmdate()` are equivalent. `date_default_timezone_set/get`
//! still round-trip the configured name, matching PHP's default of "UTC".

use crate::host::with_host;
use crate::stdlib::common::*;
use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Timelike, Utc};
use fusevm::Value;
use std::cell::RefCell;

thread_local! {
    /// Process default timezone name; PHP defaults to "UTC" when unset.
    static TZ: RefCell<String> = RefCell::new("UTC".to_string());
}

/// Dispatch a `datetime`-category PHP function by lowercased name.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let v = match name {
        "time" => Value::int(Utc::now().timestamp()),
        "mktime" | "gmmktime" => php_mktime(args),
        "date" => php_date(args, false),
        // `T` is the ONE format character the two spellings disagree on: with the
        // default timezone at UTC, `date('T')` is `UTC` and `gmdate('T')` is `GMT`.
        "gmdate" => php_date(args, true),
        "checkdate" => php_checkdate(args),
        "idate" => php_idate(args),
        "microtime" => php_microtime(args),
        "strtotime" => php_strtotime(args),
        "getdate" => php_getdate(args),
        // Records the configured name and always returns true. LIMITATION: without
        // a `chrono-tz` database, unknown timezone names are NOT rejected (PHP
        // returns false for those) and setting a non-UTC zone does NOT shift
        // subsequent `date()` output (every calculation here runs in UTC). Low
        // priority; documented deviation.
        "date_default_timezone_set" => {
            let tz = str_arg(args, 0);
            TZ.with(|t| *t.borrow_mut() = tz);
            Value::bool(true)
        }
        "date_default_timezone_get" => Value::str(TZ.with(|t| t.borrow().clone())),
        _ => return None,
    };
    Some(Ok(v))
}

/// Full weekday names, Sunday-first (matches `chrono` `num_days_from_sunday`).
const DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
/// Full month names, 1-based (index 0 unused).
const MONTHS: [&str; 13] = [
    "",
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// UTC `DateTime` for a Unix timestamp; the epoch on any out-of-range value.
fn from_ts(ts: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(ts, 0).unwrap_or_else(|| DateTime::from_timestamp(0, 0).unwrap())
}

/// PHP `time()`-style current timestamp, used as the implicit default.
fn now_ts() -> i64 {
    Utc::now().timestamp()
}

/// `date`/`gmdate`: format a timestamp (default: now) with a PHP format string.
fn php_date(args: &[Value], gm: bool) -> Value {
    let fmt = str_arg(args, 0);
    let ts = if args.len() >= 2 {
        int_arg(args, 1)
    } else {
        now_ts()
    };
    Value::str(php_format_date(fmt.as_bytes(), ts, !gm))
}

/// `php_format_date`: `ts` broken down in the default zone (`date`) or in UTC
/// (`gmdate`), rendered by timelib's `date_format`.
pub fn php_format_date(fmt: &[u8], ts: i64, localtime: bool) -> String {
    use crate::timelib as tl;
    let mut t = tl::Time::default();
    if localtime {
        t.tz_info = Some(default_tz());
        t.zone_type = tl::ZONETYPE_ID;
        tl::unixtime2local(&mut t, ts);
    } else {
        tl::unixtime2gmt(&mut t, ts);
    }
    tl::date_format(fmt, &t, localtime)
}

/// Proleptic-Gregorian leap-year test.
fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Number of days in a given month.
fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 30,
    }
}

/// `mktime`/`gmmktime`: build a timestamp from
/// `(hour, minute, second, month, day, year)`, each defaulting to the current
/// UTC component. Out-of-range fields normalize as PHP's do (month 13 → next
/// January, day 0 → prior month's last day), via signed arithmetic on a base
/// date-time. Returns `false` when the result falls outside chrono's range —
/// e.g. `mktime(0,0,0,1,999999999999,2020)` yields `false` (a non-crashing
/// deviation from PHP, which would produce a far-future timestamp). Every add is
/// checked so huge field values can never panic.
fn php_mktime(args: &[Value]) -> Value {
    let now = Utc::now();
    let comp = |i: usize, default: i64| -> i64 {
        if args.len() > i {
            int_arg(args, i)
        } else {
            default
        }
    };
    let hour = comp(0, now.hour() as i64);
    let minute = comp(1, now.minute() as i64);
    let second = comp(2, now.second() as i64);
    let month = comp(3, now.month() as i64);
    let day = comp(4, now.day() as i64);
    let year = normalize_year(comp(5, now.year() as i64));

    // Fold the (possibly out-of-range) month into a year/month pair, guarding
    // every step so absurd inputs return false instead of panicking.
    let Some(months_total) = year
        .checked_mul(12)
        .and_then(|x| month.checked_sub(1).and_then(|mm| x.checked_add(mm)))
    else {
        return Value::bool(false);
    };
    let y = months_total.div_euclid(12);
    let m = (months_total.rem_euclid(12) + 1) as u32;
    let Ok(y) = i32::try_from(y) else {
        return Value::bool(false);
    };
    let Some(base) = NaiveDate::from_ymd_opt(y, m, 1).and_then(|d| d.and_hms_opt(0, 0, 0)) else {
        return Value::bool(false);
    };
    // Checked signed arithmetic: `Duration::try_*` reject out-of-range spans and
    // `checked_add`/`checked_add_signed` reject overflow of the running total.
    let (Some(dd), Some(dh), Some(dmin), Some(ds)) = (
        day.checked_sub(1).and_then(Duration::try_days),
        Duration::try_hours(hour),
        Duration::try_minutes(minute),
        Duration::try_seconds(second),
    ) else {
        return Value::bool(false);
    };
    let Some(delta) = dd
        .checked_add(&dh)
        .and_then(|x| x.checked_add(&dmin))
        .and_then(|x| x.checked_add(&ds))
    else {
        return Value::bool(false);
    };
    let Some(dt) = base.checked_add_signed(delta) else {
        return Value::bool(false);
    };
    Value::int(Utc.from_utc_datetime(&dt).timestamp())
}

/// PHP's two-digit year fixup: 0-69 → 2000-2069, 70-100 → 1970-2000. Four-digit
/// years pass through untouched.
fn normalize_year(year: i64) -> i64 {
    match year {
        0..=69 => year + 2000,
        70..=100 => year + 1900,
        _ => year,
    }
}

/// `checkdate(month, day, year)`: valid Gregorian date, year 1-32767.
fn php_checkdate(args: &[Value]) -> Value {
    let month = int_arg(args, 0);
    let day = int_arg(args, 1);
    let year = int_arg(args, 2);
    let ok = (1..=12).contains(&month)
        && (1..=32767).contains(&year)
        && day >= 1
        && day <= days_in_month(year as i32, month as u32) as i64;
    Value::bool(ok)
}

/// `microtime(as_float=false)`: with a truthy argument, seconds since the epoch
/// as a float; otherwise the classic `"msec sec"` string.
fn php_microtime(args: &[Value]) -> Value {
    let now = Utc::now();
    let secs = now.timestamp();
    let frac = now.timestamp_subsec_micros() as f64 / 1_000_000.0;
    let as_float = with_host(|h| h.is_truthy(&arg(args, 0)));
    if as_float {
        Value::float(secs as f64 + frac)
    } else {
        Value::str(format!("{frac:.8} {secs}"))
    }
}

/// `strtotime(time, baseTimestamp=now)`: timelib's scanner (`crate::timelib`),
/// holes filled from the base timestamp in the default zone; `false` when the
/// scanner reports an error.
fn php_strtotime(args: &[Value]) -> Value {
    let input = str_arg(args, 0);
    let base = if args.len() >= 2 && !matches!(args[1], Value::Undef) {
        int_arg(args, 1)
    } else {
        now_ts()
    };
    match crate::timelib::strtotime_ts(input.as_bytes(), base, &default_tz()) {
        Some(ts) => Value::int(ts),
        None => Value::bool(false),
    }
}

/// `getdate(timestamp=now)`: the associative/indexed array of date components.
fn php_getdate(args: &[Value]) -> Value {
    let ts = if args.is_empty() {
        now_ts()
    } else {
        int_arg(args, 0)
    };
    let dt = from_ts(ts);
    let dow_sun = dt.weekday().num_days_from_sunday() as usize;
    make_map(vec![
        (Value::str("seconds"), Value::int(dt.second() as i64)),
        (Value::str("minutes"), Value::int(dt.minute() as i64)),
        (Value::str("hours"), Value::int(dt.hour() as i64)),
        (Value::str("mday"), Value::int(dt.day() as i64)),
        (Value::str("wday"), Value::int(dow_sun as i64)),
        (Value::str("mon"), Value::int(dt.month() as i64)),
        (Value::str("year"), Value::int(dt.year() as i64)),
        (Value::str("yday"), Value::int(dt.ordinal0() as i64)),
        (Value::str("weekday"), Value::str(DAYS[dow_sun])),
        (Value::str("month"), Value::str(MONTHS[dt.month() as usize])),
        (Value::int(0), Value::int(ts)),
    ])
}

/// The default time zone as timelib sees it. Only a fixed-offset zone can be
/// resolved here (see `crate::timelib`); anything else is treated as UTC.
pub fn default_tz() -> crate::timelib::TzInfo {
    let name = TZ.with(|t| t.borrow().clone());
    crate::timelib::tz_lookup(&name)
        .unwrap_or_else(|| crate::timelib::tz_lookup("UTC").expect("UTC resolves"))
}

/// `idate(format, timestamp)` — `php_idate`: one field of the timestamp in the
/// default zone, as an int.
fn php_idate(args: &[Value]) -> Value {
    use crate::timelib as tl;
    let format = str_arg(args, 0);
    if format.len() != 1 {
        with_host(|h| h.warn("idate(): idate format is one char"));
        return Value::bool(false);
    }
    let ts = if args.len() >= 2 && !matches!(args[1], Value::Undef) {
        int_arg(args, 1)
    } else {
        now_ts()
    };
    let tz = default_tz();
    let mut t = tl::Time {
        tz_info: Some(tz.clone()),
        zone_type: tl::ZONETYPE_ID,
        ..tl::Time::default()
    };
    tl::unixtime2local(&mut t, ts);
    let (isoweek, isoyear) = tl::isoweek_from_date(t.y, t.m, t.d);
    let r: i64 = match format.as_bytes()[0] {
        b'd' | b'j' => t.d,
        b'N' => tl::iso_day_of_week(t.y, t.m, t.d),
        b'w' => tl::day_of_week(t.y, t.m, t.d),
        b'z' => tl::day_of_year(t.y, t.m, t.d),
        b'W' => isoweek,
        b'm' | b'n' => t.m,
        b't' => tl::days_in_month(t.y, t.m),
        b'L' => tl::is_leap(t.y) as i64,
        b'y' => t.y % 100,
        b'Y' => t.y,
        b'o' => isoyear,
        b'B' => {
            let sse = t.sse;
            let mut r = (sse - (sse - ((sse % 86400) + 3600))) * 10;
            if r < 0 {
                r += 864000;
            }
            (r / 864) % 1000
        }
        b'g' | b'h' => {
            if t.h % 12 != 0 {
                t.h % 12
            } else {
                12
            }
        }
        b'H' | b'G' => t.h,
        b'i' => t.i,
        b's' => t.s,
        b'I' => 0,
        b'Z' => tz.offset,
        b'U' => t.sse,
        _ => -1,
    };
    // The reference returns a C `int`.
    let r = r as i32 as i64;
    if r == -1 {
        with_host(|h| h.warn("idate(): Unrecognized date format token"));
        return Value::bool(false);
    }
    Value::int(r)
}
