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
        "date" | "gmdate" => php_date(args),
        "checkdate" => php_checkdate(args),
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
fn php_date(args: &[Value]) -> Value {
    let fmt = str_arg(args, 0);
    let ts = if args.len() >= 2 {
        int_arg(args, 1)
    } else {
        now_ts()
    };
    Value::str(format_php(&fmt, from_ts(ts)))
}

/// Render `dt` per PHP `date()` format characters. `\` escapes the next char.
fn format_php(fmt: &str, dt: DateTime<Utc>) -> String {
    let mut out = String::with_capacity(fmt.len() * 2);
    let mut chars = fmt.chars();
    let dow_sun = dt.weekday().num_days_from_sunday() as usize; // 0=Sun..6=Sat
    let dow_iso = dt.weekday().number_from_monday(); // 1=Mon..7=Sun
    let day = dt.day();
    let month = dt.month();
    let hour24 = dt.hour();
    let hour12 = match hour24 % 12 {
        0 => 12,
        h => h,
    };
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            }
            'd' => out.push_str(&format!("{day:02}")),
            'j' => out.push_str(&day.to_string()),
            'D' => out.push_str(&DAYS[dow_sun][..3]),
            'l' => out.push_str(DAYS[dow_sun]),
            'N' => out.push_str(&dow_iso.to_string()),
            'w' => out.push_str(&dow_sun.to_string()),
            'S' => out.push_str(ordinal_suffix(day)),
            'z' => out.push_str(&dt.ordinal0().to_string()),
            'W' => out.push_str(&format!("{:02}", dt.iso_week().week())),
            'F' => out.push_str(MONTHS[month as usize]),
            'M' => out.push_str(&MONTHS[month as usize][..3]),
            'm' => out.push_str(&format!("{month:02}")),
            'n' => out.push_str(&month.to_string()),
            't' => out.push_str(&days_in_month(dt.year(), month).to_string()),
            'L' => out.push_str(if is_leap(dt.year()) { "1" } else { "0" }),
            'Y' => out.push_str(&dt.year().to_string()),
            'y' => out.push_str(&format!("{:02}", dt.year().rem_euclid(100))),
            'a' => out.push_str(if hour24 < 12 { "am" } else { "pm" }),
            'A' => out.push_str(if hour24 < 12 { "AM" } else { "PM" }),
            'g' => out.push_str(&hour12.to_string()),
            'G' => out.push_str(&hour24.to_string()),
            'h' => out.push_str(&format!("{hour12:02}")),
            'H' => out.push_str(&format!("{hour24:02}")),
            'i' => out.push_str(&format!("{:02}", dt.minute())),
            's' => out.push_str(&format!("{:02}", dt.second())),
            'U' => out.push_str(&dt.timestamp().to_string()),
            // ISO-8601 week-numbering year (differs from `Y` near Jan 1 / Dec 31).
            'o' => out.push_str(&dt.iso_week().year().to_string()),
            // Microseconds / milliseconds. Timestamps here carry no sub-second
            // component, so these render as zeros, matching PHP integer-second input.
            'u' => out.push_str(&format!("{:06}", dt.timestamp_subsec_micros())),
            'v' => out.push_str(&format!("{:03}", dt.timestamp_subsec_millis())),
            // ISO 8601 date, e.g. "2004-02-12T15:19:21+00:00" (UTC only here).
            'c' => out.push_str(&format!(
                "{:04}-{month:02}-{day:02}T{hour24:02}:{:02}:{:02}+00:00",
                dt.year(),
                dt.minute(),
                dt.second()
            )),
            // RFC 2822 date, e.g. "Thu, 21 Dec 2000 16:01:07 +0000" (UTC only here).
            'r' => out.push_str(&format!(
                "{}, {day:02} {} {} {hour24:02}:{:02}:{:02} +0000",
                &DAYS[dow_sun][..3],
                &MONTHS[month as usize][..3],
                dt.year(),
                dt.minute(),
                dt.second()
            )),
            other => out.push(other),
        }
    }
    out
}

/// English ordinal suffix for a day-of-month (`1`→"st", `11`→"th", `22`→"nd").
fn ordinal_suffix(day: u32) -> &'static str {
    match (day % 10, day % 100) {
        (1, 11) | (2, 12) | (3, 13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    }
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

/// `strtotime(time, baseTimestamp=now)`: parse a subset of PHP's date/time
/// grammar — "now", "today"/"yesterday"/"tomorrow", absolute `YYYY-MM-DD` and
/// `YYYY-MM-DD HH:MM:SS`, `@<unixts>`, and chained relative offsets such as
/// `"+1 week"`, `"-3 days"`, `"2 months 1 day"`. Returns `false` on parse
/// failure, mirroring PHP.
fn php_strtotime(args: &[Value]) -> Value {
    let input = str_arg(args, 0).trim().to_string();
    let base = if args.len() >= 2 {
        int_arg(args, 1)
    } else {
        now_ts()
    };
    match parse_strtotime(&input, base) {
        Some(ts) => Value::int(ts),
        None => Value::bool(false),
    }
}

/// Core of `strtotime`; `None` signals an unparseable string.
fn parse_strtotime(input: &str, base: i64) -> Option<i64> {
    // PHP 8: an empty string is a parse failure, not the base timestamp.
    if input.is_empty() {
        return None;
    }
    let low = input.to_ascii_lowercase();
    match low.as_str() {
        "now" => return Some(base),
        "today" | "midnight" => return Some(start_of_day(base)),
        "yesterday" => return Some(start_of_day(base) - 86_400),
        "tomorrow" => return Some(start_of_day(base) + 86_400),
        _ => {}
    }
    // "@<timestamp>" — explicit Unix seconds.
    if let Some(rest) = input.strip_prefix('@') {
        return rest.trim().parse::<i64>().ok();
    }
    // Absolute "YYYY-MM-DD HH:MM:SS" then "YYYY-MM-DD".
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(input, "%Y-%m-%d %H:%M:%S") {
        return Some(Utc.from_utc_datetime(&dt).timestamp());
    }
    if let Some(dt) = NaiveDate::parse_from_str(input, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
    {
        return Some(Utc.from_utc_datetime(&dt).timestamp());
    }
    parse_relative(&low, base)
}

/// Midnight (UTC) of the day containing `ts`.
fn start_of_day(ts: i64) -> i64 {
    let d = from_ts(ts).date_naive().and_hms_opt(0, 0, 0).unwrap();
    Utc.from_utc_datetime(&d).timestamp()
}

/// Apply one or more `"[+-]N unit"` offsets (optionally with a trailing "ago")
/// to `base`. Every token must match or the whole parse fails.
fn parse_relative(low: &str, base: i64) -> Option<i64> {
    let re = regex::Regex::new(
        r"([+-]?\d+)\s*(sec(?:ond)?|min(?:ute)?|hour|day|week|month|year)s?(\s+ago)?",
    )
    .ok()?;
    let mut dt = from_ts(base);
    let mut matched_end = 0usize;
    let mut any = false;
    for cap in re.captures_iter(low) {
        let whole = cap.get(0).unwrap();
        // Reject stray characters between tokens (keeps parsing strict).
        let gap = low[matched_end..whole.start()].trim();
        if !gap.is_empty() {
            return None;
        }
        matched_end = whole.end();
        any = true;
        let mut n: i64 = cap[1].parse().ok()?;
        if cap.get(3).is_some() {
            n = -n; // "N unit ago"
        }
        dt = apply_unit(dt, &cap[2], n)?;
    }
    // Require at least one token and no unconsumed trailing text.
    if !any || !low[matched_end..].trim().is_empty() {
        return None;
    }
    Some(dt.timestamp())
}

/// Shift `dt` by `n` of the named unit; month/year use calendar arithmetic.
/// Every add is checked, so huge `n` yields `None` (→ `false`) instead of a panic.
fn apply_unit(dt: DateTime<Utc>, unit: &str, n: i64) -> Option<DateTime<Utc>> {
    let dur = match unit {
        "sec" | "second" => Duration::try_seconds(n)?,
        "min" | "minute" => Duration::try_minutes(n)?,
        "hour" => Duration::try_hours(n)?,
        "day" => Duration::try_days(n)?,
        "week" => Duration::try_weeks(n)?,
        "month" => return add_months_overflow(dt, n),
        "year" => return add_months_overflow(dt, n.checked_mul(12)?),
        _ => return None,
    };
    dt.checked_add_signed(dur)
}

/// Add `months` (may be negative) to `dt`, matching PHP's **overflowing** month
/// arithmetic rather than chrono's clamping `checked_add_months`. PHP keeps the
/// day-of-month and lets it spill into the following month when the target month
/// is shorter: `2011-01-31 +1 month` → `2011-03-03`, not `2011-02-28`. Achieved
/// by landing on the first of the target month and adding `(day-1)` days.
/// Returns `None` on year/day overflow (→ `false`).
fn add_months_overflow(dt: DateTime<Utc>, months: i64) -> Option<DateTime<Utc>> {
    let total = (dt.year() as i64)
        .checked_mul(12)?
        .checked_add(dt.month0() as i64)?
        .checked_add(months)?;
    let ny = i32::try_from(total.div_euclid(12)).ok()?;
    let nm = (total.rem_euclid(12) + 1) as u32;
    let day_off = Duration::try_days(dt.day() as i64 - 1)?;
    let first = NaiveDate::from_ymd_opt(ny, nm, 1)?.and_time(dt.time());
    let shifted = first.checked_add_signed(day_off)?;
    Some(Utc.from_utc_datetime(&shifted))
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
