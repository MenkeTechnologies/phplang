//! A port of the parts of timelib (the date library bundled with the reference
//! interpreter, `ext/date/lib`, version 2022.17) that `strtotime`, `date_parse`
//! and the `DateTime` classes stand on: the free-form date scanner
//! (`parse_date.re`), `timelib_parse_from_format`, the relative-time
//! resolution of `tm2unixtime.c`, the calendar helpers of `dow.c` and
//! `unixtime2tm.c`, and the interval arithmetic of `interval.c`.
//!
//! The scanner is re2c in the original: a DFA over every token rule at once,
//! taking the LONGEST match at the cursor and, between rules matching the same
//! length, the one written first. Here every rule is a pattern of one
//! multi-pattern lazy DFA built with `MatchKind::All`; an anchored overlapping
//! search reports every (rule, end) pair at the cursor and the same choice is
//! made. Rule bodies are ported line for line and named after the rule.
//!
//! Time zones: the reference resolves identifiers against its bundled tz
//! database. phplang has none, so an identifier is found only when its offset
//! is fixed — `UTC` and its aliases and the `Etc/GMT±N` zones. Abbreviations
//! (`EST`, `CEST`) come from timelib's own table and need no database.

use regex_automata::hybrid::dfa::{Cache, OverlappingState, DFA};
use regex_automata::{Anchored, Input, MatchKind};
use std::cell::RefCell;
use std::sync::OnceLock;

/// `TIMELIB_UNSET`.
pub const UNSET: i64 = -9_999_999;

pub const ZONETYPE_NONE: u8 = 0;
pub const ZONETYPE_OFFSET: u8 = 1;
pub const ZONETYPE_ABBR: u8 = 2;
pub const ZONETYPE_ID: u8 = 3;

const SPECIAL_WEEKDAY: i64 = 1;
const SPECIAL_DAY_OF_WEEK_IN_MONTH: i64 = 2;
const SPECIAL_LAST_DAY_OF_WEEK_IN_MONTH: i64 = 3;
const SPECIAL_FIRST_DAY_OF_MONTH: i64 = 1;
const SPECIAL_LAST_DAY_OF_MONTH: i64 = 2;

const SECS_PER_DAY: i64 = 86_400;
const SECS_PER_HOUR: i64 = 3_600;
const DAYS_PER_ERA: i64 = 146_097;
const YEARS_PER_ERA: i64 = 400;
const DAYS_PER_YEAR: i64 = 365;
const HINNANT_EPOCH_SHIFT: i64 = 719_468;

/// A time zone with a fixed offset — what `tz_info` can point at here.
#[derive(Debug, Clone, PartialEq)]
pub struct TzInfo {
    /// The identifier as the database spells it (`UTC`, `Etc/GMT+5`).
    pub name: String,
    /// Seconds east of UTC.
    pub offset: i64,
    /// The abbreviation `date('T')` shows.
    pub abbr: String,
}

/// `timelib_special`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Special {
    pub kind: i64,
    pub amount: i64,
}

/// `timelib_rel_time`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RelTime {
    pub y: i64,
    pub m: i64,
    pub d: i64,
    pub h: i64,
    pub i: i64,
    pub s: i64,
    pub us: i64,
    pub weekday: i64,
    pub weekday_behavior: i64,
    pub first_last_day_of: i64,
    pub invert: bool,
    pub days: i64,
    pub special: Special,
    pub have_weekday_relative: bool,
    pub have_special_relative: bool,
}

/// `timelib_time`.
#[derive(Debug, Clone, PartialEq)]
pub struct Time {
    pub y: i64,
    pub m: i64,
    pub d: i64,
    pub h: i64,
    pub i: i64,
    pub s: i64,
    pub us: i64,
    pub z: i64,
    pub tz_abbr: Option<String>,
    pub tz_info: Option<TzInfo>,
    pub dst: i64,
    pub relative: RelTime,
    pub sse: i64,
    pub have_time: i64,
    pub have_date: bool,
    pub have_zone: i64,
    pub have_relative: bool,
    pub is_localtime: bool,
    pub zone_type: u8,
}

impl Default for Time {
    /// `timelib_time_ctor`: everything zeroed.
    fn default() -> Self {
        Time {
            y: 0,
            m: 0,
            d: 0,
            h: 0,
            i: 0,
            s: 0,
            us: 0,
            z: 0,
            tz_abbr: None,
            tz_info: None,
            dst: 0,
            relative: RelTime::default(),
            sse: 0,
            have_time: 0,
            have_date: false,
            have_zone: 0,
            have_relative: false,
            is_localtime: false,
            zone_type: ZONETYPE_NONE,
        }
    }
}

/// `timelib_error_message`.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub position: usize,
    pub character: u8,
    pub message: &'static str,
}

/// `timelib_error_container`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Errors {
    pub warnings: Vec<Message>,
    pub errors: Vec<Message>,
}

// ── tables ───────────────────────────────────────────────────────────────────

const TIMELIB_MICROSEC: u8 = 9;
const TIMELIB_SECOND: u8 = 1;
const TIMELIB_MINUTE: u8 = 2;
const TIMELIB_HOUR: u8 = 3;
const TIMELIB_DAY: u8 = 4;
const TIMELIB_MONTH: u8 = 5;
const TIMELIB_YEAR: u8 = 6;
const TIMELIB_WEEKDAY: u8 = 7;
const TIMELIB_SPECIAL: u8 = 8;

/// `timelib_relunit_lookup`: (name, unit, multiplier).
const RELUNITS: &[(&[u8], u8, i64)] = &[
    (b"ms", TIMELIB_MICROSEC, 1000),
    (b"msec", TIMELIB_MICROSEC, 1000),
    (b"msecs", TIMELIB_MICROSEC, 1000),
    (b"millisecond", TIMELIB_MICROSEC, 1000),
    (b"milliseconds", TIMELIB_MICROSEC, 1000),
    ("µs".as_bytes(), TIMELIB_MICROSEC, 1),
    (b"usec", TIMELIB_MICROSEC, 1),
    (b"usecs", TIMELIB_MICROSEC, 1),
    ("µsec".as_bytes(), TIMELIB_MICROSEC, 1),
    ("µsecs".as_bytes(), TIMELIB_MICROSEC, 1),
    (b"microsecond", TIMELIB_MICROSEC, 1),
    (b"microseconds", TIMELIB_MICROSEC, 1),
    (b"sec", TIMELIB_SECOND, 1),
    (b"secs", TIMELIB_SECOND, 1),
    (b"second", TIMELIB_SECOND, 1),
    (b"seconds", TIMELIB_SECOND, 1),
    (b"min", TIMELIB_MINUTE, 1),
    (b"mins", TIMELIB_MINUTE, 1),
    (b"minute", TIMELIB_MINUTE, 1),
    (b"minutes", TIMELIB_MINUTE, 1),
    (b"hour", TIMELIB_HOUR, 1),
    (b"hours", TIMELIB_HOUR, 1),
    (b"day", TIMELIB_DAY, 1),
    (b"days", TIMELIB_DAY, 1),
    (b"week", TIMELIB_DAY, 7),
    (b"weeks", TIMELIB_DAY, 7),
    (b"fortnight", TIMELIB_DAY, 14),
    (b"fortnights", TIMELIB_DAY, 14),
    (b"forthnight", TIMELIB_DAY, 14),
    (b"forthnights", TIMELIB_DAY, 14),
    (b"month", TIMELIB_MONTH, 1),
    (b"months", TIMELIB_MONTH, 1),
    (b"year", TIMELIB_YEAR, 1),
    (b"years", TIMELIB_YEAR, 1),
    (b"mondays", TIMELIB_WEEKDAY, 1),
    (b"monday", TIMELIB_WEEKDAY, 1),
    (b"mon", TIMELIB_WEEKDAY, 1),
    (b"tuesdays", TIMELIB_WEEKDAY, 2),
    (b"tuesday", TIMELIB_WEEKDAY, 2),
    (b"tue", TIMELIB_WEEKDAY, 2),
    (b"wednesdays", TIMELIB_WEEKDAY, 3),
    (b"wednesday", TIMELIB_WEEKDAY, 3),
    (b"wed", TIMELIB_WEEKDAY, 3),
    (b"thursdays", TIMELIB_WEEKDAY, 4),
    (b"thursday", TIMELIB_WEEKDAY, 4),
    (b"thu", TIMELIB_WEEKDAY, 4),
    (b"fridays", TIMELIB_WEEKDAY, 5),
    (b"friday", TIMELIB_WEEKDAY, 5),
    (b"fri", TIMELIB_WEEKDAY, 5),
    (b"saturdays", TIMELIB_WEEKDAY, 6),
    (b"saturday", TIMELIB_WEEKDAY, 6),
    (b"sat", TIMELIB_WEEKDAY, 6),
    (b"sundays", TIMELIB_WEEKDAY, 0),
    (b"sunday", TIMELIB_WEEKDAY, 0),
    (b"sun", TIMELIB_WEEKDAY, 0),
    (b"weekday", TIMELIB_SPECIAL, SPECIAL_WEEKDAY),
    (b"weekdays", TIMELIB_SPECIAL, SPECIAL_WEEKDAY),
];

/// `timelib_reltext_lookup`: (name, behavior, value).
const RELTEXT: &[(&[u8], i64, i64)] = &[
    (b"first", 0, 1),
    (b"next", 0, 1),
    (b"second", 0, 2),
    (b"third", 0, 3),
    (b"fourth", 0, 4),
    (b"fifth", 0, 5),
    (b"sixth", 0, 6),
    (b"seventh", 0, 7),
    (b"eight", 0, 8),
    (b"eighth", 0, 8),
    (b"ninth", 0, 9),
    (b"tenth", 0, 10),
    (b"eleventh", 0, 11),
    (b"twelfth", 0, 12),
    (b"last", 0, -1),
    (b"previous", 0, -1),
    (b"this", 1, 0),
];

/// `timelib_month_lookup`.
const MONTHS: &[(&[u8], i64)] = &[
    (b"jan", 1),
    (b"feb", 2),
    (b"mar", 3),
    (b"apr", 4),
    (b"may", 5),
    (b"jun", 6),
    (b"jul", 7),
    (b"aug", 8),
    (b"sep", 9),
    (b"sept", 9),
    (b"oct", 10),
    (b"nov", 11),
    (b"dec", 12),
    (b"i", 1),
    (b"ii", 2),
    (b"iii", 3),
    (b"iv", 4),
    (b"v", 5),
    (b"vi", 6),
    (b"vii", 7),
    (b"viii", 8),
    (b"ix", 9),
    (b"x", 10),
    (b"xi", 11),
    (b"xii", 12),
    (b"january", 1),
    (b"february", 2),
    (b"march", 3),
    (b"april", 4),
    (b"may", 5),
    (b"june", 6),
    (b"july", 7),
    (b"august", 8),
    (b"september", 9),
    (b"october", 10),
    (b"november", 11),
    (b"december", 12),
];

/// `timezonemap.h`, reduced to the FIRST row for each abbreviation: a lookup
/// made without an offset (`abbr_search(word, -1, 0)`, the only kind the
/// scanner makes) returns the first row whose name matches. (name, dst, offset)
const ABBREVIATIONS: &[(&str, i64, i64)] = &include!("timelib_abbr.in");

/// `MAX_ABBR_LEN` (`_POSIX_TZNAME_MAX`, 6 on the platforms the reference is
/// built for): only a word SHORTER than this is looked up as an abbreviation.
const MAX_ABBR_LEN: usize = 6;

fn eq_ci(a: &[u8], b: &[u8]) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// `abbr_search(word, -1, 0)`.
fn abbr_search(word: &[u8]) -> Option<(i64, i64)> {
    if eq_ci(word, b"utc") || eq_ci(word, b"gmt") {
        return Some((0, 0));
    }
    ABBREVIATIONS
        .iter()
        .find(|(n, _, _)| eq_ci(word, n.as_bytes()))
        .map(|(_, dst, off)| (*dst, *off))
}

/// The zones whose offset never changes, which is every zone phplang can
/// resolve without a tz database: (identifier, offset east, abbreviation).
fn fixed_zone(name: &str) -> Option<TzInfo> {
    const ALIASES: &[&str] = &[
        "UTC",
        "Etc/UTC",
        "Etc/UCT",
        "UCT",
        "Universal",
        "Etc/Universal",
        "Zulu",
        "Etc/Zulu",
        "GMT",
        "Etc/GMT",
        "GMT0",
        "Etc/GMT0",
        "GMT+0",
        "GMT-0",
        "Etc/GMT+0",
        "Etc/GMT-0",
        "Greenwich",
        "Etc/Greenwich",
    ];
    if let Some(id) = ALIASES.iter().find(|a| a.eq_ignore_ascii_case(name)) {
        let abbr = if id.contains("GMT") || id.contains("Greenwich") {
            "GMT"
        } else {
            "UTC"
        };
        return Some(TzInfo {
            name: id.to_string(),
            offset: 0,
            abbr: abbr.to_string(),
        });
    }
    // `Etc/GMT+5` is FIVE HOURS WEST — POSIX sign convention.
    name.get(..7)
        .filter(|p| p.eq_ignore_ascii_case("Etc/GMT"))?;
    let tail = &name[7..];
    let (sign, digits) = match tail.as_bytes().first()? {
        b'+' => (-1, &tail[1..]),
        b'-' => (1, &tail[1..]),
        _ => return None,
    };
    let hours: i64 = digits
        .parse()
        .ok()
        .filter(|_| digits.bytes().all(|b| b.is_ascii_digit()))?;
    let max = if sign < 0 { 12 } else { 14 };
    if !(1..=max).contains(&hours) {
        return None;
    }
    let off = sign * hours * 3600;
    Some(TzInfo {
        name: format!("Etc/GMT{}{}", if sign < 0 { '+' } else { '-' }, hours),
        offset: off,
        abbr: format!("{}{:02}", if off < 0 { '-' } else { '+' }, hours),
    })
}

/// Look a zone identifier up the way `php_date_parse_tzfile` does
/// (case-insensitively), within what can be resolved here.
pub fn tz_lookup(name: &str) -> Option<TzInfo> {
    fixed_zone(name)
}

// ── calendar helpers (dow.c, unixtime2tm.c, tm2unixtime.c) ───────────────────

pub fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

const ML_COMMON: [i64; 13] = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
const ML_LEAP: [i64; 13] = [0, 31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
/// tm2unixtime.c's tables, indexed 0 = December of the year before.
const DIM_LEAP: [i64; 13] = [31, 31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
const DIM: [i64; 13] = [31, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

pub fn days_in_month(y: i64, m: i64) -> i64 {
    if is_leap(y) {
        ML_LEAP[m as usize]
    } else {
        ML_COMMON[m as usize]
    }
}

fn positive_mod(x: i64, y: i64) -> i64 {
    let t = x % y;
    if t < 0 {
        t + y
    } else {
        t
    }
}

fn day_of_week_ex(y: i64, m: i64, d: i64, iso: bool) -> i64 {
    const M_COMMON: [i64; 13] = [-1, 0, 3, 3, 6, 1, 4, 6, 2, 5, 0, 3, 5];
    const M_LEAP: [i64; 13] = [-1, 6, 2, 3, 6, 1, 4, 6, 2, 5, 0, 3, 5];
    let c1 = 6 - positive_mod(positive_mod(y, 400) / 100, 4) * 2;
    let y1 = positive_mod(y, 100);
    let m1 = if is_leap(y) {
        M_LEAP[m as usize]
    } else {
        M_COMMON[m as usize]
    };
    let dow = positive_mod(c1 + y1 + m1 + (y1 / 4) + d, 7);
    if iso && dow == 0 {
        7
    } else {
        dow
    }
}

/// `timelib_day_of_week`: 0 = Sunday.
pub fn day_of_week(y: i64, m: i64, d: i64) -> i64 {
    day_of_week_ex(y, m, d, false)
}

/// `timelib_iso_day_of_week`: 7 = Sunday.
pub fn iso_day_of_week(y: i64, m: i64, d: i64) -> i64 {
    day_of_week_ex(y, m, d, true)
}

/// `timelib_day_of_year`: 0-based.
pub fn day_of_year(y: i64, m: i64, d: i64) -> i64 {
    const D_COMMON: [i64; 13] = [0, 0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    const D_LEAP: [i64; 13] = [0, 0, 31, 60, 91, 121, 152, 182, 213, 244, 274, 305, 335];
    (if is_leap(y) {
        D_LEAP[m as usize]
    } else {
        D_COMMON[m as usize]
    }) + d
        - 1
}

/// `timelib_isoweek_from_date`: (iso week, iso year).
pub fn isoweek_from_date(y: i64, m: i64, d: i64) -> (i64, i64) {
    let y_leap = is_leap(y) as i64;
    let prev_y_leap = is_leap(y - 1);
    let mut doy = day_of_year(y, m, d) + 1;
    if y_leap == 1 && m > 2 {
        doy += 1;
    }
    let mut jan1weekday = day_of_week(y, 1, 1);
    let mut weekday = day_of_week(y, m, d);
    if weekday == 0 {
        weekday = 7;
    }
    if jan1weekday == 0 {
        jan1weekday = 7;
    }
    let (mut iw, mut iy) = (0, y);
    if doy <= (8 - jan1weekday) && jan1weekday > 4 {
        iy = y - 1;
        iw = if jan1weekday == 5 || (jan1weekday == 6 && prev_y_leap) {
            53
        } else {
            52
        };
    }
    if iy == y {
        let i = if y_leap == 1 { 366 } else { 365 };
        if (i - (doy - y_leap)) < (4 - weekday) {
            return (1, y + 1);
        }
    }
    if iy == y {
        let j = doy + (7 - weekday) + (jan1weekday - 1);
        iw = j / 7;
        if jan1weekday > 4 {
            iw -= 1;
        }
    }
    (iw, iy)
}

/// `timelib_daynr_from_weeknr`.
pub fn daynr_from_weeknr(iy: i64, iw: i64, id: i64) -> i64 {
    let dow = day_of_week(iy, 1, 1);
    let day = -(if dow > 4 { dow - 7 } else { dow });
    day + ((iw - 1) * 7) + id
}

/// `timelib_date_from_isodate`.
pub fn date_from_isodate(iy: i64, iw: i64, id: i64) -> (i64, i64, i64) {
    let mut daynr = daynr_from_weeknr(iy, iw, id) + 1;
    let mut y = iy;
    let mut leap = is_leap(y);
    while daynr <= 0 {
        y -= 1;
        leap = is_leap(y);
        daynr += if leap { 366 } else { 365 };
    }
    while daynr > if leap { 366 } else { 365 } {
        daynr -= if leap { 366 } else { 365 };
        y += 1;
        leap = is_leap(y);
    }
    let table = if leap { &ML_LEAP } else { &ML_COMMON };
    let mut m = 1;
    while daynr > table[m] {
        daynr -= table[m];
        m += 1;
    }
    (y, m as i64, daynr)
}

pub fn valid_time(h: i64, i: i64, s: i64) -> bool {
    (0..=23).contains(&h) && (0..=59).contains(&i) && (0..=59).contains(&s)
}

pub fn valid_date(y: i64, m: i64, d: i64) -> bool {
    (1..=12).contains(&m) && d >= 1 && d <= days_in_month(y, m)
}

/// `timelib_date_from_epoch_days`.
pub fn date_from_epoch_days(epoch_days: i64) -> (i64, i64, i64) {
    let days = epoch_days + HINNANT_EPOCH_SHIFT;
    let era = (if days >= 0 {
        days
    } else {
        days - DAYS_PER_ERA + 1
    }) / DAYS_PER_ERA;
    let day_of_era = days - era * DAYS_PER_ERA;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / DAYS_PER_YEAR;
    let mut y = year_of_era + era * YEARS_PER_ERA;
    let day_of_year =
        day_of_era - (DAYS_PER_YEAR * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_portion = (5 * day_of_year + 2) / 153;
    let d = day_of_year - (153 * month_portion + 2) / 5 + 1;
    let m = month_portion + if month_portion < 10 { 3 } else { -9 };
    if m <= 2 {
        y += 1;
    }
    (y, m, d)
}

/// `timelib_epoch_days_from_time`.
pub fn epoch_days_from_ymd(y: i64, m: i64, d: i64) -> i64 {
    let y = y - (m <= 2) as i64;
    let era = (if y >= 0 { y } else { y - 399 }) / YEARS_PER_ERA;
    let year_of_era = y - era * YEARS_PER_ERA;
    let day_of_year = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let day_of_era =
        year_of_era * DAYS_PER_YEAR + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * DAYS_PER_ERA + day_of_era - HINNANT_EPOCH_SHIFT
}

/// `timelib_unixtime2gmt`.
pub fn unixtime2gmt(tm: &mut Time, ts: i64) {
    let mut epoch_days = ts / SECS_PER_DAY;
    if ts % SECS_PER_DAY < 0 {
        epoch_days -= 1;
    }
    let (y, m, d) = date_from_epoch_days(epoch_days);
    tm.y = y;
    tm.m = m;
    tm.d = d;
    let mut rem = ts % SECS_PER_DAY;
    if rem < 0 {
        rem += SECS_PER_DAY;
    }
    tm.h = rem / 3600;
    tm.i = (rem - tm.h * 3600) / 60;
    tm.s = rem % 60;
    tm.z = 0;
    tm.dst = 0;
    tm.sse = ts;
    tm.is_localtime = false;
}

/// `timelib_unixtime2local`.
pub fn unixtime2local(tm: &mut Time, ts: i64) {
    match tm.zone_type {
        ZONETYPE_ABBR | ZONETYPE_OFFSET => {
            let (z, dst) = (tm.z, tm.dst);
            unixtime2gmt(tm, ts + z + dst * 3600);
            tm.sse = ts;
            tm.z = z;
            tm.dst = dst;
        }
        ZONETYPE_ID => {
            let tz = tm.tz_info.clone().expect("an ID zone carries its tz_info");
            unixtime2gmt(tm, ts + tz.offset);
            tm.sse = ts;
            tm.dst = 0;
            tm.z = tz.offset;
            tm.tz_info = Some(tz.clone());
            tm.tz_abbr = Some(tz.abbr);
        }
        _ => {
            unixtime2gmt(tm, ts);
            return;
        }
    }
    tm.is_localtime = true;
    tm.have_zone = 1;
}

/// `timelib_update_from_sse`.
pub fn update_from_sse(tm: &mut Time) {
    let (sse, z, dst) = (tm.sse, tm.z, tm.dst);
    match tm.zone_type {
        ZONETYPE_ABBR | ZONETYPE_OFFSET => unixtime2gmt(tm, sse + z + dst * 3600),
        ZONETYPE_ID => {
            let off = tm.tz_info.as_ref().map_or(0, |t| t.offset);
            unixtime2gmt(tm, sse + off);
        }
        _ => unixtime2gmt(tm, sse),
    }
    tm.sse = sse;
    tm.is_localtime = true;
    tm.have_zone = 1;
    tm.z = z;
    tm.dst = dst;
}

/// `timelib_set_timezone`.
pub fn set_timezone(t: &mut Time, tz: &TzInfo) {
    t.z = tz.offset;
    t.dst = 0;
    t.tz_info = Some(tz.clone());
    t.tz_abbr = Some(tz.abbr.clone());
    t.have_zone = 1;
    t.zone_type = ZONETYPE_ID;
}

fn do_range_limit(start: i64, end: i64, adj: i64, a: &mut i64, b: &mut i64) {
    if *a < start {
        let a_plus_1 = *a + 1;
        *b -= (start - a_plus_1) / adj + 1;
        *a += adj * ((start - a_plus_1) / adj);
        *a += adj;
    }
    if *a >= end {
        *b += *a / adj;
        *a -= adj * (*a / adj);
    }
}

fn do_range_limit_days_relative(
    base_y: &mut i64,
    base_m: &mut i64,
    m: &mut i64,
    d: &mut i64,
    invert: bool,
) {
    do_range_limit(1, 13, 12, base_m, base_y);
    let (mut year, mut month) = (*base_y, *base_m);
    if !invert {
        while *d < 0 {
            month -= 1;
            if month < 1 {
                month += 12;
                year -= 1;
            }
            let days = if is_leap(year) {
                DIM_LEAP[month as usize]
            } else {
                DIM[month as usize]
            };
            *d += days;
            *m -= 1;
        }
    } else {
        while *d < 0 {
            let days = if is_leap(year) {
                DIM_LEAP[month as usize]
            } else {
                DIM[month as usize]
            };
            *d += days;
            *m -= 1;
            month += 1;
            if month > 12 {
                month -= 12;
                year += 1;
            }
        }
    }
}

fn do_range_limit_days(y: &mut i64, m: &mut i64, d: &mut i64) -> bool {
    if *d >= DAYS_PER_ERA || *d <= -DAYS_PER_ERA {
        *y += YEARS_PER_ERA * (*d / DAYS_PER_ERA);
        *d -= DAYS_PER_ERA * (*d / DAYS_PER_ERA);
    }
    do_range_limit(1, 13, 12, m, y);
    let current = if is_leap(*y) { &DIM_LEAP } else { &DIM };
    let mut retval = false;
    while *d <= 0 && *m > 0 {
        let (pm, py) = if *m - 1 < 1 {
            (*m - 1 + 12, *y - 1)
        } else {
            (*m - 1, *y)
        };
        *d += if is_leap(py) {
            DIM_LEAP[pm as usize]
        } else {
            DIM[pm as usize]
        };
        *m -= 1;
        retval = true;
    }
    while *d > 0 && *m <= 12 && *d > current[*m as usize] {
        *d -= current[*m as usize];
        *m += 1;
        retval = true;
    }
    retval
}

fn do_adjust_for_weekday(time: &mut Time) {
    let current_dow = day_of_week(time.y, time.m, time.d);
    let rel = &mut time.relative;
    if rel.weekday_behavior == 2 {
        if current_dow == 0 && rel.weekday != 0 {
            rel.weekday -= 7;
        }
        if rel.weekday == 0 && current_dow != 0 {
            rel.weekday = 7;
        }
        time.d -= current_dow;
        time.d += rel.weekday;
        return;
    }
    let mut difference = rel.weekday - current_dow;
    if (rel.d < 0 && difference < 0) || (rel.d >= 0 && difference <= -rel.weekday_behavior) {
        difference += 7;
    }
    if rel.weekday >= 0 {
        time.d += difference;
    } else {
        time.d -= 7 - (rel.weekday.abs() - current_dow);
    }
    rel.have_weekday_relative = false;
}

/// `timelib_do_rel_normalize`. `base` is (y, m) of the base time, which the
/// reference normalizes in place too.
pub fn do_rel_normalize(base_y: &mut i64, base_m: &mut i64, rt: &mut RelTime) {
    do_range_limit(0, 1_000_000, 1_000_000, &mut rt.us, &mut rt.s);
    do_range_limit(0, 60, 60, &mut rt.s, &mut rt.i);
    do_range_limit(0, 60, 60, &mut rt.i, &mut rt.h);
    do_range_limit(0, 24, 24, &mut rt.h, &mut rt.d);
    do_range_limit(0, 12, 12, &mut rt.m, &mut rt.y);
    do_range_limit_days_relative(base_y, base_m, &mut rt.m, &mut rt.d, rt.invert);
    do_range_limit(0, 12, 12, &mut rt.m, &mut rt.y);
}

/// `timelib_do_normalize`.
pub fn do_normalize(t: &mut Time) {
    if t.us != UNSET {
        do_range_limit(0, 1_000_000, 1_000_000, &mut t.us, &mut t.s);
    }
    if t.s != UNSET {
        do_range_limit(0, 60, 60, &mut t.s, &mut t.i);
        do_range_limit(0, 60, 60, &mut t.i, &mut t.h);
        do_range_limit(0, 24, 24, &mut t.h, &mut t.d);
    }
    do_range_limit(1, 13, 12, &mut t.m, &mut t.y);
    if t.y == 1970 && t.m == 1 {
        let (y, m, d) = date_from_epoch_days(t.d - 1);
        t.y = y;
        t.m = m;
        t.d = d;
        return;
    }
    while do_range_limit_days(&mut t.y, &mut t.m, &mut t.d) {}
    do_range_limit(1, 13, 12, &mut t.m, &mut t.y);
}

fn do_adjust_relative(t: &mut Time) {
    if t.relative.have_weekday_relative {
        do_adjust_for_weekday(t);
    }
    do_normalize(t);
    if t.have_relative {
        t.us += t.relative.us;
        t.s += t.relative.s;
        t.i += t.relative.i;
        t.h += t.relative.h;
        t.d += t.relative.d;
        t.m += t.relative.m;
        t.y += t.relative.y;
    }
    match t.relative.first_last_day_of {
        SPECIAL_FIRST_DAY_OF_MONTH => t.d = 1,
        SPECIAL_LAST_DAY_OF_MONTH => {
            t.d = 0;
            t.m += 1;
        }
        _ => {}
    }
    do_normalize(t);
}

fn do_adjust_special_weekday(t: &mut Time) {
    let count = t.relative.special.amount;
    let dow = day_of_week(t.y, t.m, t.d);
    t.d += (count / 5) * 7;
    let rem = count % 5;
    if count > 0 {
        if rem == 0 {
            if dow == 0 {
                t.d -= 2;
            } else if dow == 6 {
                t.d -= 1;
            }
        } else if dow == 6 {
            t.d += 1;
        } else if dow + rem > 5 {
            t.d += 2;
        }
    } else if rem == 0 {
        if dow == 6 {
            t.d += 2;
        } else if dow == 0 {
            t.d += 1;
        }
    } else if dow == 0 {
        t.d -= 1;
    } else if dow + rem < 1 {
        t.d -= 2;
    }
    t.d += rem;
}

fn do_adjust_special(t: &mut Time) {
    if t.relative.have_special_relative && t.relative.special.kind == SPECIAL_WEEKDAY {
        do_adjust_special_weekday(t);
    }
    do_normalize(t);
    t.relative.special = Special::default();
}

fn do_adjust_special_early(t: &mut Time) {
    if t.relative.have_special_relative {
        match t.relative.special.kind {
            SPECIAL_DAY_OF_WEEK_IN_MONTH => {
                t.d = 1;
                t.m += t.relative.m;
                t.relative.m = 0;
            }
            SPECIAL_LAST_DAY_OF_WEEK_IN_MONTH => {
                t.d = 1;
                t.m += t.relative.m + 1;
                t.relative.m = 0;
            }
            _ => {}
        }
    }
    match t.relative.first_last_day_of {
        SPECIAL_FIRST_DAY_OF_MONTH => t.d = 1,
        SPECIAL_LAST_DAY_OF_MONTH => {
            t.d = 0;
            t.m += 1;
        }
        _ => {}
    }
    do_normalize(t);
}

fn do_adjust_timezone(t: &mut Time, tzi: Option<&TzInfo>) {
    match t.zone_type {
        ZONETYPE_OFFSET => {
            t.is_localtime = true;
            t.sse -= t.z;
        }
        ZONETYPE_ABBR => {
            t.is_localtime = true;
            t.sse += -t.z - t.dst * SECS_PER_HOUR;
        }
        _ => {
            let tz = if t.zone_type == ZONETYPE_ID {
                t.tz_info.clone()
            } else {
                tzi.cloned()
            };
            let Some(tz) = tz else { return };
            t.is_localtime = true;
            t.sse -= tz.offset;
            set_timezone(t, &tz);
        }
    }
}

/// `timelib_update_ts`.
pub fn update_ts(t: &mut Time, tzi: Option<&TzInfo>) {
    do_adjust_special_early(t);
    do_adjust_relative(t);
    do_adjust_special(t);
    t.sse = t.h * SECS_PER_HOUR + t.i * 60 + t.s;
    let days = epoch_days_from_ymd(t.y, t.m, t.d);
    t.sse = t.sse.wrapping_add(days.wrapping_mul(SECS_PER_DAY / 2));
    t.sse = t.sse.wrapping_add(days.wrapping_mul(SECS_PER_DAY / 2));
    do_adjust_timezone(t, tzi);
    t.have_relative = false;
    t.relative.have_weekday_relative = false;
    t.relative.have_special_relative = false;
    t.relative.first_last_day_of = 0;
}

/// `timelib_fill_holes`. `override_time` is `TIMELIB_OVERRIDE_TIME`.
pub fn fill_holes(parsed: &mut Time, now: &Time, override_time: bool) {
    if !override_time && parsed.have_date && parsed.have_time == 0 {
        parsed.h = 0;
        parsed.i = 0;
        parsed.s = 0;
        parsed.us = 0;
    }
    if parsed.y != UNSET
        || parsed.m != UNSET
        || parsed.d != UNSET
        || parsed.h != UNSET
        || parsed.i != UNSET
        || parsed.s != UNSET
    {
        if parsed.us == UNSET {
            parsed.us = 0;
        }
    } else if parsed.us == UNSET {
        parsed.us = if now.us != UNSET { now.us } else { 0 };
    }
    let pick = |p: &mut i64, n: i64| {
        if *p == UNSET {
            *p = if n != UNSET { n } else { 0 };
        }
    };
    pick(&mut parsed.y, now.y);
    pick(&mut parsed.m, now.m);
    pick(&mut parsed.d, now.d);
    pick(&mut parsed.h, now.h);
    pick(&mut parsed.i, now.i);
    pick(&mut parsed.s, now.s);
    if parsed.tz_info.is_none() {
        parsed.tz_info = now.tz_info.clone();
        pick(&mut parsed.z, now.z);
        pick(&mut parsed.dst, now.dst);
        if parsed.tz_abbr.is_none() {
            parsed.tz_abbr = now.tz_abbr.clone();
        }
    }
    if parsed.zone_type == ZONETYPE_NONE && now.zone_type != ZONETYPE_NONE {
        parsed.zone_type = now.zone_type;
        parsed.is_localtime = true;
    }
}

// ── interval.c ───────────────────────────────────────────────────────────────

fn hmsf_to_decimal_hour(h: i64, i: i64, s: i64, us: i64) -> f64 {
    let (h, i, s, us) = (h as f64, i as f64, s as f64, us as f64);
    if h >= 0.0 {
        (h + i / 60.0 + s / 3600.0) + us / 3_600_000_000.0
    } else {
        (h - i / 60.0 - s / 3600.0) - us / 3_600_000_000.0
    }
}

/// `timelib_time_compare`.
pub fn time_compare(a: &Time, b: &Time) -> std::cmp::Ordering {
    (a.sse, a.us).cmp(&(b.sse, b.us))
}

/// `timelib_same_timezone`.
pub fn same_timezone(a: &Time, b: &Time) -> bool {
    if a.zone_type != b.zone_type {
        return false;
    }
    match a.zone_type {
        ZONETYPE_ABBR | ZONETYPE_OFFSET => a.z + a.dst * 3600 == b.z + b.dst * 3600,
        ZONETYPE_ID => a.tz_info.as_ref().map(|t| &t.name) == b.tz_info.as_ref().map(|t| &t.name),
        _ => false,
    }
}

/// `timelib_diff_days`.
pub fn diff_days(one: &Time, two: &Time) -> i64 {
    if same_timezone(one, two) {
        let (earliest, latest) = if time_compare(one, two).is_lt() {
            (one, two)
        } else {
            (two, one)
        };
        let et = hmsf_to_decimal_hour(earliest.h, earliest.i, earliest.s, earliest.us);
        let lt = hmsf_to_decimal_hour(latest.h, latest.i, latest.s, latest.us);
        let mut days = (epoch_days_from_ymd(one.y, one.m, one.d)
            - epoch_days_from_ymd(two.y, two.m, two.d))
        .abs();
        if lt < et && days > 0 {
            days -= 1;
        }
        days
    } else {
        ((one.sse - two.sse) as f64 / 86400.0).abs() as i64
    }
}

fn sort_old_to_new<'a>(one: &'a Time, two: &'a Time, rt: &mut RelTime) -> (&'a Time, &'a Time) {
    let same_id = one.zone_type == ZONETYPE_ID
        && two.zone_type == ZONETYPE_ID
        && one.tz_info.as_ref().map(|t| &t.name) == two.tz_info.as_ref().map(|t| &t.name);
    let swap = if same_id {
        (one.y, one.m, one.d, one.h, one.i, one.s, one.us)
            > (two.y, two.m, two.d, two.h, two.i, two.s, two.us)
    } else {
        (one.sse, one.us) > (two.sse, two.us)
    };
    if swap {
        rt.invert = true;
        (two, one)
    } else {
        (one, two)
    }
}

/// `timelib_diff`. Every zone here is fixed, so the DST corrections of
/// `timelib_diff_with_tzid` never apply; its fall-back branch is kept.
pub fn diff(one: &Time, two: &Time) -> RelTime {
    let mut rt = RelTime::default();
    let same_id = one.zone_type == ZONETYPE_ID
        && two.zone_type == ZONETYPE_ID
        && one.tz_info.as_ref().map(|t| &t.name) == two.tz_info.as_ref().map(|t| &t.name);
    let (one, two) = sort_old_to_new(one, two, &mut rt);
    rt.y = two.y - one.y;
    rt.m = two.m - one.m;
    rt.d = two.d - one.d;
    rt.h = two.h - one.h;
    rt.i = two.i - one.i;
    rt.s = two.s - one.s;
    rt.us = two.us - one.us;
    if same_id {
        let dst_corr = two.z - one.z;
        rt.days = diff_days(one, two);
        if two.sse < one.sse {
            let flipped = ((rt.i * 60) + rt.s - dst_corr).abs();
            rt.h = flipped / SECS_PER_HOUR;
            rt.i = (flipped - rt.h * SECS_PER_HOUR) / 60;
            rt.s = flipped % 60;
            rt.invert = !rt.invert;
        }
    } else {
        if one.zone_type != ZONETYPE_ID {
            rt.h += one.dst;
        }
        if two.zone_type != ZONETYPE_ID {
            rt.h -= two.dst;
        }
        rt.s = two.s - one.s - two.z + one.z;
        rt.days = diff_days(one, two);
    }
    let base = if rt.invert { one } else { two };
    let (mut by, mut bm) = (base.y, base.m);
    do_rel_normalize(&mut by, &mut bm, &mut rt);
    rt
}

fn interval_range_limit(start: i64, end: i64, adj: i64, a: &mut i64, b: &mut i64) {
    if *a < start {
        *b -= (start - *a - 1) / adj + 1;
        *a += adj * ((start - *a - 1) / adj + 1);
    }
    if *a >= end {
        *b += *a / adj;
        *a -= adj * (*a / adj);
    }
}

/// `timelib_add_wall` (`sub` = `timelib_sub_wall`).
pub fn add_wall(old: &Time, interval: &RelTime, sub: bool) -> Time {
    let mut t = old.clone();
    t.have_relative = true;
    let neg = if sub { -1 } else { 1 };
    if interval.have_weekday_relative || interval.have_special_relative {
        t.relative = interval.clone();
        update_ts(&mut t, None);
        update_from_sse(&mut t);
    } else {
        let bias = if interval.invert { -1 } else { 1 };
        t.relative = RelTime {
            y: neg * interval.y * bias,
            m: neg * interval.m * bias,
            d: neg * interval.d * bias,
            ..RelTime::default()
        };
        if t.relative.y != 0 || t.relative.m != 0 || t.relative.d != 0 {
            update_ts(&mut t, None);
        }
        if interval.us == 0 {
            t.sse += neg * bias * (interval.h * SECS_PER_HOUR + interval.i * 60 + interval.s);
            update_from_sse(&mut t);
        } else {
            let mut tmp = interval.clone();
            interval_range_limit(0, 1_000_000, 1_000_000, &mut tmp.us, &mut tmp.s);
            t.sse += neg * bias * (tmp.h * SECS_PER_HOUR + tmp.i * 60 + tmp.s);
            update_from_sse(&mut t);
            t.us += neg * tmp.us * bias;
            do_normalize(&mut t);
            update_ts(&mut t, None);
        }
        do_normalize(&mut t);
    }
    if t.zone_type == ZONETYPE_ID {
        if let Some(tz) = t.tz_info.clone() {
            set_timezone(&mut t, &tz);
        }
    }
    t.have_relative = false;
    t
}

// ── the scanner (parse_date.re) ──────────────────────────────────────────────

/// What a token rule does, one per re2c action block.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Rule {
    Yesterday,
    Now,
    Noon,
    MidnightToday,
    Tomorrow,
    Timestamp,
    TimestampMs,
    FirstLastDayOf,
    BackFrontOf,
    WeekdayOf,
    Time12,
    MssqlTime,
    Time24,
    GnuNoColon,
    Iso8601NoColon,
    American,
    Iso8601Date4,
    Iso8601Date2,
    Iso8601DateX,
    GnuDateShorter,
    GnuDateShort,
    DateFull,
    PointedDate4,
    PointedDate2,
    DateNoDay,
    DateNoDayRev,
    DateTextual,
    DateNoYearRev,
    DateNoColon,
    XmlRpc,
    PgYdotd,
    IsoWeekday,
    IsoWeek,
    PgTextShort,
    PgTextReverse,
    Clf,
    Year4,
    Ago,
    DayText,
    RelativeTextWeek,
    RelativeText,
    MonthText,
    Timezone,
    DateShortWithTime12,
    DateShortWithTime24,
    Relative,
    Separator,
    Space,
    Nul,
    Any,
}

/// The named sub-expressions of `parse_date.re`, in definition order, as
/// byte-regex syntax: `'x'` (re2c's case-insensitive literal) is `(?i:x)`.
fn definitions() -> Vec<(&'static str, &'static str)> {
    vec![
        ("nbsp", r"\xC2\xA0"),
        ("nnbsp", r"\xE2\x80\xAF"),
        ("space", r"(?:[ \t]+|(?:{nbsp})+|(?:{nnbsp})+)"),
        ("frac", r"\.[0-9]+"),
        ("ago", r"(?i:ago)"),
        ("hour24", r"(?:[01]?[0-9]|2[0-4])"),
        ("hour24lz", r"(?:[01][0-9]|2[0-4])"),
        ("hour12", r"(?:0?[1-9]|1[0-2])"),
        ("minute", r"(?:[0-5]?[0-9])"),
        ("minutelz", r"(?:[0-5][0-9])"),
        ("second", r"(?:{minute}|60)"),
        ("secondlz", r"(?:{minutelz}|60)"),
        ("meridian", r"(?:[AaPp]\.?[Mm]\.?)[\x00\t ]"),
        (
            "tz",
            r"(?:\(?[A-Za-z]{1,6}\)?|[A-Z][a-z]+(?:[_/\-][A-Za-z]+)+)",
        ),
        (
            "tzcorrection",
            r"(?:GMT)?[+\-](?:(?:{hour24}(?::?{minute})?)|(?:{hour24lz}{minutelz}{secondlz})|(?:{hour24lz}:{minutelz}:{secondlz}))",
        ),
        ("daysuf", r"(?:st|nd|rd|th)"),
        ("month", r"(?:0?[0-9]|1[0-2])"),
        ("day", r"(?:(?:[0-2]?[0-9]|3[01]){daysuf}?)"),
        ("year", r"[0-9]{1,4}"),
        ("year2", r"[0-9]{2}"),
        ("year4", r"[0-9]{4}"),
        ("year4withsign", r"[+\-]?[0-9]{4}"),
        ("yearx", r"[+\-][0-9]{5,19}"),
        (
            "dayofyear",
            r"(?:00[1-9]|0[1-9][0-9]|[1-2][0-9][0-9]|3[0-5][0-9]|36[0-6])",
        ),
        ("weekofyear", r"(?:0[1-9]|[1-4][0-9]|5[0-3])"),
        ("monthlz", r"(?:0[0-9]|1[0-2])"),
        ("daylz", r"(?:0[0-9]|[1-2][0-9]|3[01])"),
        (
            "dayfulls",
            r"(?i:sundays|mondays|tuesdays|wednesdays|thursdays|fridays|saturdays)",
        ),
        (
            "dayfull",
            r"(?i:sunday|monday|tuesday|wednesday|thursday|friday|saturday)",
        ),
        ("dayabbr", r"(?i:sun|mon|tue|wed|thu|fri|sat)"),
        ("dayspecial", r"(?i:weekday|weekdays)"),
        (
            "daytext",
            r"(?:{dayfulls}|{dayfull}|{dayabbr}|{dayspecial})",
        ),
        (
            "monthfull",
            r"(?i:january|february|march|april|may|june|july|august|september|october|november|december)",
        ),
        (
            "monthabbr",
            r"(?i:jan|feb|mar|apr|may|jun|jul|aug|sep|sept|oct|nov|dec)",
        ),
        ("monthroman", r"(?:I|II|III|IV|V|VI|VII|VIII|IX|X|XI|XII)"),
        ("monthtext", r"(?:{monthfull}|{monthabbr}|{monthroman})"),
        ("timetiny12", r"{hour12}{space}?{meridian}"),
        ("timeshort12", r"{hour12}[:.]{minutelz}{space}?{meridian}"),
        (
            "timelong12",
            r"{hour12}[:.]{minute}[:.]{secondlz}{space}?{meridian}",
        ),
        ("timetiny24", r"(?i:t){hour24}"),
        ("timeshort24", r"(?i:t)?{hour24}[:.]{minute}"),
        ("timelong24", r"(?i:t)?{hour24}[:.]{minute}[:.]{second}"),
        (
            "iso8601long",
            r"(?i:t)?{hour24}[:.]{minute}[:.]{second}{frac}",
        ),
        (
            "iso8601normtz",
            r"(?i:t)?{hour24}[:.]{minute}[:.]{secondlz}{space}?(?:{tzcorrection}|{tz})",
        ),
        ("gnunocolon", r"(?i:t)?{hour24lz}{minutelz}"),
        ("iso8601nocolon", r"(?i:t)?{hour24lz}{minutelz}{secondlz}"),
        ("americanshort", r"{month}/{day}"),
        ("american", r"{month}/{day}/{year}"),
        ("iso8601dateslash", r"{year4}/{monthlz}/{daylz}/?"),
        ("dateslash", r"{year4}/{month}/{day}"),
        ("iso8601date4", r"{year4withsign}-{monthlz}-{daylz}"),
        ("iso8601date2", r"{year2}-{monthlz}-{daylz}"),
        ("iso8601datex", r"{yearx}-{monthlz}-{daylz}"),
        ("gnudateshorter", r"{year4}-{month}"),
        ("gnudateshort", r"{year}-{month}-{day}"),
        ("pointeddate4", r"{day}[.\t\-]{month}[.\-]{year4}"),
        ("pointeddate2", r"{day}[.\t]{month}\.{year2}"),
        ("datefull", r"{day}[ \t.\-]*{monthtext}[ \t.\-]*{year}"),
        ("datenoday", r"{monthtext}[ .\t\-]*{year4}"),
        ("datenodayrev", r"{year4}[ .\t\-]*{monthtext}"),
        (
            "datetextual",
            r"{monthtext}[ .\t\-]*{day}[,.stndrh\t ]+{year}",
        ),
        (
            "datenoyear",
            r"{monthtext}[ .\t\-]*{day}(?:[,.stndrh\t ]+|\x00)",
        ),
        ("datenoyearrev", r"{day}[ .\t\-]*{monthtext}"),
        ("datenocolon", r"{year4}{monthlz}{daylz}"),
        (
            "soap",
            r"{year4}-{monthlz}-{daylz}T{hour24lz}:{minutelz}:{secondlz}{frac}{tzcorrection}?",
        ),
        (
            "xmlrpc",
            r"{year4}{monthlz}{daylz}T{hour24}:{minutelz}:{secondlz}",
        ),
        (
            "xmlrpcnocolon",
            r"{year4}{monthlz}{daylz}(?i:t){hour24}{minutelz}{secondlz}",
        ),
        ("wddx", r"{year4}-{month}-{day}T{hour24}:{minute}:{second}"),
        ("pgydotd", r"{year4}[.\-]?{dayofyear}"),
        ("pgtextshort", r"{monthabbr}-{daylz}-{year}"),
        ("pgtextreverse", r"{year}-{monthabbr}-{daylz}"),
        (
            "mssqltime",
            r"{hour12}:{minutelz}:{secondlz}[:.][0-9]+{meridian}",
        ),
        ("isoweekday", r"{year4}-?W{weekofyear}-?[0-7]"),
        ("isoweek", r"{year4}-?W{weekofyear}"),
        (
            "exif",
            r"{year4}:{monthlz}:{daylz} {hour24lz}:{minutelz}:{secondlz}",
        ),
        ("firstdayof", r"(?i:first day of)"),
        ("lastdayof", r"(?i:last day of)"),
        ("backof", r"(?i:back of ){hour24}(?:{space}?{meridian})?"),
        ("frontof", r"(?i:front of ){hour24}(?:{space}?{meridian})?"),
        (
            "clf",
            r"{day}/{monthabbr}/{year4}:{hour24lz}:{minutelz}:{secondlz}{space}{tzcorrection}",
        ),
        ("timestamp", r"@-?[0-9]+"),
        ("timestampms", r"@-?[0-9]+\.[0-9]{0,6}"),
        ("dateshortwithtimeshort12", r"{datenoyear}{timeshort12}"),
        ("dateshortwithtimelong12", r"{datenoyear}{timelong12}"),
        ("dateshortwithtimeshort", r"{datenoyear}{timeshort24}"),
        ("dateshortwithtimelong", r"{datenoyear}{timelong24}"),
        ("dateshortwithtimelongtz", r"{datenoyear}{iso8601normtz}"),
        (
            "reltextnumber",
            r"(?i:first|second|third|fourth|fifth|sixth|seventh|eight|eighth|ninth|tenth|eleventh|twelfth)",
        ),
        ("reltexttext", r"(?i:next|last|previous|this)"),
        (
            "reltextunit",
            r"(?:(?i:ms)|\xC2\xB5(?i:s)|(?:(?:(?i:msec|millisecond)|\xC2\xB5(?i:sec)|(?i:microsecond|usec|sec|second|min|minute|hour|day|fortnight|forthnight|month|year))(?i:s)?)|(?i:weeks)|{daytext})",
        ),
        ("relnumber", r"(?:[+\-]*[ \t]*[0-9]{1,13})"),
        (
            "relative",
            r"{relnumber}{space}?(?:{reltextunit}|(?i:week))",
        ),
        (
            "relativetext",
            r"(?:{reltextnumber}|{reltexttext}){space}{reltextunit}",
        ),
        ("relativetextweek", r"{reltexttext}{space}(?i:week)"),
        (
            "weekdayof",
            r"(?:{reltextnumber}|{reltexttext}){space}(?:{dayfulls}|{dayfull}|{dayabbr}){space}(?i:of)",
        ),
    ]
}

/// The token rules in the order `parse_date.re` writes them — which is the
/// tie-break between rules that match the same length.
fn rules() -> Vec<(Rule, &'static str)> {
    use Rule::*;
    vec![
        (Yesterday, "(?i:yesterday)"),
        (Now, "(?i:now)"),
        (Noon, "(?i:noon)"),
        (MidnightToday, "(?i:midnight)|(?i:today)"),
        (Tomorrow, "(?i:tomorrow)"),
        (Timestamp, "{timestamp}"),
        (TimestampMs, "{timestampms}"),
        (FirstLastDayOf, "{firstdayof}|{lastdayof}"),
        (BackFrontOf, "{backof}|{frontof}"),
        (WeekdayOf, "{weekdayof}"),
        (Time12, "{timetiny12}|{timeshort12}|{timelong12}"),
        (MssqlTime, "{mssqltime}"),
        (
            Time24,
            "{timetiny24}|{timeshort24}|{timelong24}|{iso8601long}",
        ),
        (GnuNoColon, "{gnunocolon}"),
        (Iso8601NoColon, "{iso8601nocolon}"),
        (American, "{americanshort}|{american}"),
        (
            Iso8601Date4,
            "{iso8601date4}|{iso8601dateslash}|{dateslash}",
        ),
        (Iso8601Date2, "{iso8601date2}"),
        (Iso8601DateX, "{iso8601datex}"),
        (GnuDateShorter, "{gnudateshorter}"),
        (GnuDateShort, "{gnudateshort}"),
        (DateFull, "{datefull}"),
        (PointedDate4, "{pointeddate4}"),
        (PointedDate2, "{pointeddate2}"),
        (DateNoDay, "{datenoday}"),
        (DateNoDayRev, "{datenodayrev}"),
        (DateTextual, "{datetextual}|{datenoyear}"),
        (DateNoYearRev, "{datenoyearrev}"),
        (DateNoColon, "{datenocolon}"),
        (XmlRpc, "{xmlrpc}|{xmlrpcnocolon}|{soap}|{wddx}|{exif}"),
        (PgYdotd, "{pgydotd}"),
        (IsoWeekday, "{isoweekday}"),
        (IsoWeek, "{isoweek}"),
        (PgTextShort, "{pgtextshort}"),
        (PgTextReverse, "{pgtextreverse}"),
        (Clf, "{clf}"),
        (Year4, "{year4}"),
        (Ago, "{ago}"),
        (DayText, "{daytext}"),
        (RelativeTextWeek, "{relativetextweek}"),
        (RelativeText, "{relativetext}"),
        (MonthText, "{monthfull}|{monthabbr}"),
        (Timezone, "{tzcorrection}|{tz}"),
        (
            DateShortWithTime12,
            "{dateshortwithtimeshort12}|{dateshortwithtimelong12}",
        ),
        (
            DateShortWithTime24,
            "{dateshortwithtimeshort}|{dateshortwithtimelong}|{dateshortwithtimelongtz}",
        ),
        (Relative, "{relative}"),
        (Separator, "[.,]"),
        (Space, "{space}"),
        (Nul, r"\x00|\n"),
        (Any, r"(?s-u:.)"),
    ]
}

/// Expand `{name}` references against the definitions made so far.
fn expand(pat: &str, defs: &[(String, String)]) -> String {
    let mut out = String::with_capacity(pat.len() * 4);
    let mut rest = pat;
    while let Some(open) = rest.find('{') {
        // `{1,6}`-style counted repetition is not a reference.
        let close = rest[open..]
            .find('}')
            .map(|c| open + c)
            .expect("balanced braces");
        let name = &rest[open + 1..close];
        out.push_str(&rest[..open]);
        if name.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
            let (_, body) = defs
                .iter()
                .find(|(n, _)| n == name)
                .unwrap_or_else(|| panic!("undefined {name}"));
            out.push_str("(?:");
            out.push_str(body);
            out.push(')');
        } else {
            out.push_str(&rest[open..=close]);
        }
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

struct Scanner {
    dfa: DFA,
    rules: Vec<Rule>,
}

fn scanner() -> &'static Scanner {
    static S: OnceLock<Scanner> = OnceLock::new();
    S.get_or_init(|| {
        let mut defs: Vec<(String, String)> = Vec::new();
        for (name, pat) in definitions() {
            let body = expand(pat, &defs);
            defs.push((name.to_string(), body));
        }
        let rules = rules();
        let pats: Vec<String> = rules
            .iter()
            .map(|(_, p)| format!("(?-u:{})", expand(p, &defs)))
            .collect();
        let dfa = DFA::builder()
            .configure(
                DFA::config()
                    .match_kind(MatchKind::All)
                    .cache_capacity(16 * (1 << 20)),
            )
            .syntax(
                regex_automata::util::syntax::Config::new()
                    .unicode(false)
                    .utf8(false),
            )
            .thompson(regex_automata::nfa::thompson::Config::new().utf8(false))
            .build_many(&pats)
            .expect("timelib token rules compile");
        Scanner {
            dfa,
            rules: rules.into_iter().map(|(r, _)| r).collect(),
        }
    })
}

thread_local! {
    static CACHE: RefCell<Option<Cache>> = const { RefCell::new(None) };
}

/// The longest rule match at `pos` (re2c semantics): (rule, end).
fn longest_match(buf: &[u8], pos: usize) -> (Rule, usize) {
    let sc = scanner();
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        let cache = c.get_or_insert_with(|| sc.dfa.create_cache());
        let input = Input::new(buf).range(pos..).anchored(Anchored::Yes);
        let mut state = OverlappingState::start();
        let mut best: Option<(usize, usize)> = None;
        loop {
            if sc
                .dfa
                .try_search_overlapping_fwd(cache, &input, &mut state)
                .is_err()
            {
                break;
            }
            let Some(hm) = state.get_match() else { break };
            let (end, pid) = (hm.offset(), hm.pattern().as_usize());
            if end > pos && best.map_or(true, |(be, bp)| end > be || (end == be && pid < bp)) {
                best = Some((end, pid));
            }
        }
        let (end, pid) = best.unwrap_or((pos + 1, sc.rules.len() - 1));
        (sc.rules[pid], end)
    })
}

/// A token's text as the C actions see it: a NUL-terminated string, so a NUL
/// a rule consumed ends it. `at` reads past the end as NUL.
struct Tok {
    b: Vec<u8>,
}

impl Tok {
    fn at(&self, p: usize) -> u8 {
        self.b.get(p).copied().unwrap_or(0)
    }
}

fn is_digit(c: u8) -> bool {
    c.is_ascii_digit()
}

/// `strtoll` of an all-digit (optionally signed) run, saturating.
fn strtoll(s: &[u8]) -> (i64, bool) {
    let (neg, digits) = match s.first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let mut v: i128 = 0;
    let mut overflow = false;
    for &c in digits.iter().take_while(|c| c.is_ascii_digit()) {
        v = v * 10 + (c - b'0') as i128;
        if v > i64::MAX as i128 + 1 {
            overflow = true;
            v = i64::MAX as i128 + 1;
        }
    }
    let v = if neg { -v } else { v };
    if v > i64::MAX as i128 {
        (i64::MAX, true)
    } else if v < i64::MIN as i128 {
        (i64::MIN, true)
    } else {
        (v as i64, overflow && !(neg && v == i64::MIN as i128))
    }
}

/// `timelib_get_nr_ex`: (value, digits scanned).
fn get_nr_ex(t: &Tok, p: &mut usize, max: usize) -> (i64, usize) {
    while !is_digit(t.at(*p)) {
        if t.at(*p) == 0 {
            return (UNSET, 0);
        }
        *p += 1;
    }
    let begin = *p;
    while is_digit(t.at(*p)) && *p - begin < max {
        *p += 1;
    }
    (strtoll(&t.b[begin..*p]).0, *p - begin)
}

fn get_nr(t: &Tok, p: &mut usize, max: usize) -> i64 {
    get_nr_ex(t, p, max).0
}

fn skip_day_suffix(t: &Tok, p: &mut usize) {
    if t.at(*p).is_ascii_whitespace() {
        return;
    }
    let two = [
        t.at(*p).to_ascii_lowercase(),
        t.at(*p + 1).to_ascii_lowercase(),
    ];
    if matches!(&two, b"nd" | b"rd" | b"st" | b"th") {
        *p += 2;
    }
}

/// `timelib_get_frac_nr`.
fn get_frac_nr(t: &Tok, p: &mut usize) -> i64 {
    while t.at(*p) != b'.' && t.at(*p) != b':' && !is_digit(t.at(*p)) {
        if t.at(*p) == 0 {
            return UNSET;
        }
        *p += 1;
    }
    let begin = *p;
    while t.at(*p) == b'.' || t.at(*p) == b':' || is_digit(t.at(*p)) {
        *p += 1;
    }
    let s = &t.b[begin + 1..*p];
    // strtod of the run after its first character: digits, then an optional
    // fraction; it stops at anything else.
    let int_end = s
        .iter()
        .position(|c| !c.is_ascii_digit())
        .unwrap_or(s.len());
    let mut end = int_end;
    if s.get(int_end) == Some(&b'.') {
        end = int_end
            + 1
            + s[int_end + 1..]
                .iter()
                .take_while(|c| c.is_ascii_digit())
                .count();
    }
    let v: f64 = std::str::from_utf8(&s[..end])
        .ok()
        .and_then(|x| x.parse().ok())
        .unwrap_or(0.0);
    (v * 10f64.powi(7 - (*p - begin) as i32)) as i64
}

fn lookup_relunit(t: &Tok, p: &mut usize) -> Option<(u8, i64)> {
    let begin = *p;
    while !matches!(
        t.at(*p),
        0 | b' ' | b',' | b'\t' | b';' | b':' | b'/' | b'.' | b'-' | b'(' | b')'
    ) {
        *p += 1;
    }
    let word = &t.b[begin..*p];
    RELUNITS
        .iter()
        .find(|(n, _, _)| eq_ci(word, n))
        .map(|(_, u, m)| (*u, *m))
}

fn lookup_month(t: &Tok, p: &mut usize) -> i64 {
    let begin = *p;
    while t.at(*p).is_ascii_alphabetic() {
        *p += 1;
    }
    let word = &t.b[begin..*p];
    MONTHS
        .iter()
        .rev()
        .find(|(n, _)| eq_ci(word, n))
        .map_or(0, |(_, v)| *v)
}

fn get_month(t: &Tok, p: &mut usize) -> i64 {
    while matches!(t.at(*p), b' ' | b'\t' | b'-' | b'.' | b'/') {
        *p += 1;
    }
    lookup_month(t, p)
}

fn eat_spaces(t: &Tok, p: &mut usize) {
    loop {
        match (t.at(*p), t.at(*p + 1), t.at(*p + 2)) {
            (b' ' | b'\t', _, _) => *p += 1,
            (0xE2, 0x80, 0xAF) => *p += 3,
            (0xC2, 0xA0, _) => *p += 2,
            _ => break,
        }
    }
}

/// `timelib_meridian`.
fn meridian(t: &Tok, p: &mut usize, h: i64) -> i64 {
    while !matches!(t.at(*p), b'A' | b'a' | b'P' | b'p' | 0) {
        *p += 1;
    }
    let mut r = 0;
    if matches!(t.at(*p), b'a' | b'A') {
        if h == 12 {
            r = -12;
        }
    } else if h != 12 {
        r = 12;
    }
    *p += 1;
    if t.at(*p) == b'.' {
        *p += 1;
    }
    if matches!(t.at(*p), b'M' | b'm') {
        *p += 1;
    }
    if t.at(*p) == b'.' {
        *p += 1;
    }
    r
}

/// `timelib_meridian_with_check`.
fn meridian_with_check(t: &Tok, p: &mut usize, h: i64) -> i64 {
    while t.at(*p) != 0 && !matches!(t.at(*p), b'A' | b'a' | b'P' | b'p') {
        *p += 1;
    }
    if t.at(*p) == 0 {
        return UNSET;
    }
    let mut r = 0;
    if matches!(t.at(*p), b'a' | b'A') {
        if h == 12 {
            r = -12;
        }
    } else if h != 12 {
        r = 12;
    }
    *p += 1;
    if t.at(*p) == b'.' {
        *p += 1;
        if !matches!(t.at(*p), b'm' | b'M') {
            return UNSET;
        }
        *p += 1;
        if t.at(*p) != b'.' {
            return UNSET;
        }
        *p += 1;
    } else if matches!(t.at(*p), b'm' | b'M') {
        *p += 1;
    } else {
        return UNSET;
    }
    r
}

fn process_year(y: &mut i64, len: usize) {
    if *y == UNSET || len >= 4 {
        return;
    }
    if *y < 100 {
        *y += if *y < 70 { 2000 } else { 1900 };
    }
}

struct State {
    time: Time,
    errors: Errors,
    tok: usize,
    tok_char: u8,
}

impl State {
    fn error(&mut self, msg: &'static str) {
        self.errors.errors.push(Message {
            position: self.tok,
            character: self.tok_char,
            message: msg,
        });
    }

    fn warning(&mut self, msg: &'static str) {
        self.errors.warnings.push(Message {
            position: self.tok,
            character: self.tok_char,
            message: msg,
        });
    }

    /// `TIMELIB_HAVE_TIME`; `false` means the action returned `TIMELIB_ERROR`.
    fn have_time(&mut self) -> bool {
        if self.time.have_time != 0 {
            self.error("Double time specification");
            return false;
        }
        self.time.have_time = 1;
        self.unhave_time_fields();
        true
    }

    fn unhave_time_fields(&mut self) {
        self.time.h = 0;
        self.time.i = 0;
        self.time.s = 0;
        self.time.us = 0;
    }

    fn unhave_time(&mut self) {
        self.time.have_time = 0;
        self.unhave_time_fields();
    }

    fn have_date(&mut self) -> bool {
        if self.time.have_date {
            self.error("Double date specification");
            return false;
        }
        self.time.have_date = true;
        true
    }

    fn unhave_date(&mut self) {
        self.time.have_date = false;
        self.time.d = 0;
        self.time.m = 0;
        self.time.y = 0;
    }

    fn have_relative(&mut self) {
        self.time.have_relative = true;
    }

    fn have_weekday_relative(&mut self) {
        self.time.have_relative = true;
        self.time.relative.have_weekday_relative = true;
    }

    fn have_special_relative(&mut self) {
        self.time.have_relative = true;
        self.time.relative.have_special_relative = true;
    }

    fn have_tz(&mut self) -> bool {
        if self.time.have_zone != 0 {
            if self.time.have_zone > 1 {
                self.error("Double timezone specification");
            } else {
                self.warning("Double timezone specification");
            }
            self.time.have_zone += 1;
            return false;
        }
        self.time.have_zone += 1;
        true
    }

    /// `timelib_get_signed_nr`.
    fn get_signed_nr(&mut self, t: &Tok, p: &mut usize, max: usize) -> i64 {
        while !is_digit(t.at(*p)) && t.at(*p) != b'+' && t.at(*p) != b'-' {
            if t.at(*p) == 0 {
                self.error("Found unexpected data");
                return 0;
            }
            *p += 1;
        }
        let mut neg = false;
        while t.at(*p) == b'+' || t.at(*p) == b'-' {
            if t.at(*p) == b'-' {
                neg = !neg;
            }
            *p += 1;
        }
        while !is_digit(t.at(*p)) {
            if t.at(*p) == 0 {
                self.error("Found unexpected data");
                return 0;
            }
            *p += 1;
        }
        let begin = *p;
        while is_digit(t.at(*p)) && *p - begin < max {
            *p += 1;
        }
        let mut s = vec![if neg { b'-' } else { b'+' }];
        s.extend_from_slice(&t.b[begin..*p]);
        let (v, overflow) = strtoll(&s);
        if overflow {
            self.error("Number out of range");
            return 0;
        }
        v
    }

    fn add_with_overflow(&mut self, which: fn(&mut RelTime) -> &mut i64, amount: i64, mult: i64) {
        let e = which(&mut self.time.relative);
        let (v, o) = e.overflowing_add(amount.wrapping_mul(mult));
        *e = v;
        if o {
            self.error("Number out of range");
        }
    }

    /// `timelib_set_relative`.
    fn set_relative(
        &mut self,
        t: &Tok,
        p: &mut usize,
        amount: i64,
        behavior: i64,
        keep_time: bool,
    ) {
        let Some((unit, mult)) = lookup_relunit(t, p) else {
            return;
        };
        match unit {
            TIMELIB_MICROSEC => self.add_with_overflow(|r| &mut r.us, amount, mult),
            TIMELIB_SECOND => self.add_with_overflow(|r| &mut r.s, amount, mult),
            TIMELIB_MINUTE => self.add_with_overflow(|r| &mut r.i, amount, mult),
            TIMELIB_HOUR => self.add_with_overflow(|r| &mut r.h, amount, mult),
            TIMELIB_DAY => self.add_with_overflow(|r| &mut r.d, amount, mult),
            TIMELIB_MONTH => self.add_with_overflow(|r| &mut r.m, amount, mult),
            TIMELIB_YEAR => self.add_with_overflow(|r| &mut r.y, amount, mult),
            TIMELIB_WEEKDAY => {
                self.have_weekday_relative();
                if !keep_time {
                    self.unhave_time();
                }
                let r = &mut self.time.relative;
                r.d += (if amount > 0 { amount - 1 } else { amount }) * 7;
                r.weekday = mult;
                r.weekday_behavior = behavior;
            }
            TIMELIB_SPECIAL => {
                self.have_special_relative();
                if !keep_time {
                    self.unhave_time();
                }
                self.time.relative.special.kind = mult;
                self.time.relative.special.amount = amount;
            }
            _ => {}
        }
    }

    /// `timelib_parse_zone`: the zone's offset, and whether it was NOT found.
    fn parse_zone(&mut self, t: &Tok, p: &mut usize) -> (i64, bool) {
        parse_zone(&mut self.time, t, p)
    }

    fn zone_into_time(&mut self, t: &Tok, p: &mut usize) {
        let (z, not_found) = self.parse_zone(t, p);
        self.time.z = z;
        if not_found {
            self.error("The timezone could not be found in the database");
        }
    }
}

/// `timelib_parse_tz_cor`: (offset, not found).
fn parse_tz_cor(t: &Tok, p: &mut usize) -> (i64, bool) {
    let begin = *p;
    while is_digit(t.at(*p)) || t.at(*p) == b':' {
        *p += 1;
    }
    let s = &t.b[begin..*p];
    let num = |from: usize| strtoll(&s[from.min(s.len())..]).0;
    let hour = |v: i64| v * 3600;
    let min = |v: i64| v * 60;
    match s.len() {
        1 | 2 => (hour(num(0)), false),
        3 | 4 => {
            if s[1] == b':' {
                (hour(num(0)) + min(num(2)), false)
            } else if s[2] == b':' {
                (hour(num(0)) + min(num(3)), false)
            } else {
                let v = num(0);
                (hour(v / 100) + min(v % 100), false)
            }
        }
        5 if s[2] == b':' => (hour(num(0)) + min(num(3)), false),
        6 => {
            let v = num(0);
            (hour(v / 10000) + min((v / 100) % 100) + (v % 100), false)
        }
        8 if s[2] == b':' && s[5] == b':' => (hour(num(0)) + min(num(3)) + num(6), false),
        _ => (0, true),
    }
}

/// `timelib_parse_zone`.
fn parse_zone(time: &mut Time, t: &Tok, p: &mut usize) -> (i64, bool) {
    let mut paren = 0;
    while matches!(t.at(*p), b' ' | b'\t' | b'(') {
        if t.at(*p) == b'(' {
            paren += 1;
        }
        *p += 1;
    }
    if t.at(*p) == b'G'
        && t.at(*p + 1) == b'M'
        && t.at(*p + 2) == b'T'
        && matches!(t.at(*p + 3), b'+' | b'-')
    {
        *p += 3;
    }
    let (retval, not_found);
    if t.at(*p) == b'+' || t.at(*p) == b'-' {
        let sign = if t.at(*p) == b'-' { -1 } else { 1 };
        *p += 1;
        time.is_localtime = true;
        time.zone_type = ZONETYPE_OFFSET;
        time.dst = 0;
        let (v, nf) = parse_tz_cor(t, p);
        retval = sign * v;
        not_found = nf;
    } else {
        time.is_localtime = true;
        // timelib_lookup_abbr
        let begin = *p;
        while t.at(*p).is_ascii_alphanumeric() || matches!(t.at(*p), b'/' | b'_' | b'-' | b'+') {
            *p += 1;
        }
        let word = &t.b[begin..*p];
        let mut found = 0;
        let mut offset = 0;
        if word.len() < MAX_ABBR_LEN {
            if let Some((dst, off)) = abbr_search(word) {
                offset = off - dst * 3600;
                time.dst = dst;
                found = 1;
                time.zone_type = ZONETYPE_ABBR;
                time.tz_abbr = Some(String::from_utf8_lossy(word).to_ascii_uppercase());
            }
        }
        if found == 0 || word == b"UTC" {
            if let Some(tz) = std::str::from_utf8(word).ok().and_then(tz_lookup) {
                time.tz_info = Some(tz);
                time.zone_type = ZONETYPE_ID;
                found += 1;
            }
        }
        not_found = found == 0;
        retval = offset;
    }
    while paren > 0 && t.at(*p) == b')' {
        *p += 1;
        paren -= 1;
    }
    (retval, not_found)
}

fn lookup_relative_text(t: &Tok, p: &mut usize, behavior: &mut i64) -> i64 {
    let begin = *p;
    while t.at(*p).is_ascii_alphabetic() {
        *p += 1;
    }
    let word = &t.b[begin..*p];
    let mut value = 0;
    for (n, b, v) in RELTEXT {
        if eq_ci(word, n) {
            value = *v;
            *behavior = *b;
        }
    }
    value
}

fn get_relative_text(t: &Tok, p: &mut usize, behavior: &mut i64) -> i64 {
    while matches!(t.at(*p), b' ' | b'\t' | b'-' | b'/') {
        *p += 1;
    }
    lookup_relative_text(t, p, behavior)
}

/// Run one rule's action over its token.
fn act(st: &mut State, rule: Rule, t: &Tok) {
    use Rule::*;
    let mut p = 0usize;
    let p = &mut p;
    match rule {
        Yesterday => {
            st.have_relative();
            st.unhave_time();
            st.time.relative.d = -1;
        }
        Now => {}
        Noon => {
            st.unhave_time();
            if !st.have_time() {
                return;
            }
            st.time.h = 12;
        }
        MidnightToday => st.unhave_time(),
        Tomorrow => {
            st.have_relative();
            st.unhave_time();
            st.time.relative.d = 1;
        }
        Timestamp | TimestampMs => {
            st.have_relative();
            st.unhave_date();
            st.unhave_time();
            if !st.have_tz() {
                return;
            }
            let is_negative = t.at(1) == b'-';
            let i = st.get_signed_nr(t, p, 24);
            let mut us = 0i64;
            if rule == TimestampMs {
                let before = *p;
                let v = st.get_signed_nr(t, p, 6);
                us = (v as f64 * 10f64.powi(7 - (*p - before) as i32)) as i64;
                if is_negative {
                    us = -us;
                }
            }
            let time = &mut st.time;
            time.y = 1970;
            time.m = 1;
            time.d = 1;
            time.h = 0;
            time.i = 0;
            time.s = 0;
            time.us = 0;
            time.relative.s += i;
            if rule == TimestampMs {
                time.relative.us = us;
            }
            time.is_localtime = true;
            time.zone_type = ZONETYPE_OFFSET;
            time.z = 0;
            time.dst = 0;
        }
        FirstLastDayOf => {
            st.have_relative();
            st.time.relative.first_last_day_of = if matches!(t.at(0), b'l' | b'L') {
                SPECIAL_LAST_DAY_OF_MONTH
            } else {
                SPECIAL_FIRST_DAY_OF_MONTH
            };
        }
        BackFrontOf => {
            st.unhave_time();
            if !st.have_time() {
                return;
            }
            if t.at(0) == b'b' {
                st.time.h = get_nr(t, p, 2);
                st.time.i = 15;
            } else {
                st.time.h = get_nr(t, p, 2) - 1;
                st.time.i = 45;
            }
            if t.at(*p) != 0 {
                eat_spaces(t, p);
                st.time.h += meridian(t, p, st.time.h);
            }
        }
        WeekdayOf => {
            st.have_relative();
            st.have_special_relative();
            let mut behavior = 0;
            let i = get_relative_text(t, p, &mut behavior);
            eat_spaces(t, p);
            if i > 0 {
                st.time.relative.special.kind = SPECIAL_DAY_OF_WEEK_IN_MONTH;
                st.set_relative(t, p, i, 1, false);
            } else {
                st.time.relative.special.kind = SPECIAL_LAST_DAY_OF_WEEK_IN_MONTH;
                st.set_relative(t, p, i, behavior, false);
            }
        }
        Time12 => {
            if !st.have_time() {
                return;
            }
            st.time.h = get_nr(t, p, 2);
            if matches!(t.at(*p), b':' | b'.') {
                st.time.i = get_nr(t, p, 2);
                if matches!(t.at(*p), b':' | b'.') {
                    st.time.s = get_nr(t, p, 2);
                }
            }
            eat_spaces(t, p);
            st.time.h += meridian(t, p, st.time.h);
        }
        MssqlTime => {
            if !st.have_time() {
                return;
            }
            st.time.h = get_nr(t, p, 2);
            st.time.i = get_nr(t, p, 2);
            if matches!(t.at(*p), b':' | b'.') {
                st.time.s = get_nr(t, p, 2);
                if matches!(t.at(*p), b':' | b'.') {
                    st.time.us = get_frac_nr(t, p);
                }
            }
            eat_spaces(t, p);
            st.time.h += meridian(t, p, st.time.h);
        }
        Time24 => {
            if !st.have_time() {
                return;
            }
            st.time.h = get_nr(t, p, 2);
            if matches!(t.at(*p), b':' | b'.') {
                st.time.i = get_nr(t, p, 2);
                if matches!(t.at(*p), b':' | b'.') {
                    st.time.s = get_nr(t, p, 2);
                    if t.at(*p) == b'.' {
                        st.time.us = get_frac_nr(t, p);
                    }
                }
            }
            if t.at(*p) != 0 {
                st.zone_into_time(t, p);
            }
        }
        GnuNoColon => {
            match st.time.have_time {
                0 => {
                    st.time.h = get_nr(t, p, 2);
                    st.time.i = get_nr(t, p, 2);
                    st.time.s = 0;
                }
                1 => st.time.y = get_nr(t, p, 4),
                _ => {
                    st.error("Double time specification");
                    return;
                }
            }
            st.time.have_time += 1;
        }
        Iso8601NoColon => {
            if !st.have_time() {
                return;
            }
            st.time.h = get_nr(t, p, 2);
            st.time.i = get_nr(t, p, 2);
            st.time.s = get_nr(t, p, 2);
            if t.at(*p) != 0 {
                st.zone_into_time(t, p);
            }
        }
        American => {
            if !st.have_date() {
                return;
            }
            st.time.m = get_nr(t, p, 2);
            st.time.d = get_nr(t, p, 2);
            if t.at(*p) == b'/' {
                let (y, len) = get_nr_ex(t, p, 4);
                st.time.y = y;
                process_year(&mut st.time.y, len);
            }
        }
        Iso8601Date4 | Iso8601DateX => {
            if !st.have_date() {
                return;
            }
            st.time.y = st.get_signed_nr(t, p, if rule == Iso8601Date4 { 4 } else { 19 });
            st.time.m = get_nr(t, p, 2);
            st.time.d = get_nr(t, p, 2);
        }
        Iso8601Date2 | GnuDateShort => {
            if !st.have_date() {
                return;
            }
            let (y, len) = get_nr_ex(t, p, 4);
            st.time.y = y;
            st.time.m = get_nr(t, p, 2);
            st.time.d = get_nr(t, p, 2);
            process_year(&mut st.time.y, len);
        }
        GnuDateShorter => {
            if !st.have_date() {
                return;
            }
            let (y, len) = get_nr_ex(t, p, 4);
            st.time.y = y;
            st.time.m = get_nr(t, p, 2);
            st.time.d = 1;
            process_year(&mut st.time.y, len);
        }
        DateFull => {
            if !st.have_date() {
                return;
            }
            st.time.d = get_nr(t, p, 2);
            skip_day_suffix(t, p);
            st.time.m = get_month(t, p);
            let (y, len) = get_nr_ex(t, p, 4);
            st.time.y = y;
            process_year(&mut st.time.y, len);
        }
        PointedDate4 => {
            if !st.have_date() {
                return;
            }
            st.time.d = get_nr(t, p, 2);
            st.time.m = get_nr(t, p, 2);
            st.time.y = get_nr(t, p, 4);
        }
        PointedDate2 => {
            if !st.have_date() {
                return;
            }
            st.time.d = get_nr(t, p, 2);
            st.time.m = get_nr(t, p, 2);
            let (y, len) = get_nr_ex(t, p, 2);
            st.time.y = y;
            process_year(&mut st.time.y, len);
        }
        DateNoDay => {
            if !st.have_date() {
                return;
            }
            st.time.m = get_month(t, p);
            let (y, len) = get_nr_ex(t, p, 4);
            st.time.y = y;
            st.time.d = 1;
            process_year(&mut st.time.y, len);
        }
        DateNoDayRev => {
            if !st.have_date() {
                return;
            }
            let (y, len) = get_nr_ex(t, p, 4);
            st.time.y = y;
            st.time.m = get_month(t, p);
            st.time.d = 1;
            process_year(&mut st.time.y, len);
        }
        DateTextual | PgTextShort => {
            if !st.have_date() {
                return;
            }
            st.time.m = get_month(t, p);
            st.time.d = get_nr(t, p, 2);
            let (y, len) = get_nr_ex(t, p, 4);
            st.time.y = y;
            process_year(&mut st.time.y, len);
        }
        DateNoYearRev => {
            if !st.have_date() {
                return;
            }
            st.time.d = get_nr(t, p, 2);
            skip_day_suffix(t, p);
            st.time.m = get_month(t, p);
        }
        DateNoColon => {
            if !st.have_date() {
                return;
            }
            st.time.y = get_nr(t, p, 4);
            st.time.m = get_nr(t, p, 2);
            st.time.d = get_nr(t, p, 2);
        }
        XmlRpc => {
            if !st.have_time() || !st.have_date() {
                return;
            }
            st.time.y = get_nr(t, p, 4);
            st.time.m = get_nr(t, p, 2);
            st.time.d = get_nr(t, p, 2);
            st.time.h = get_nr(t, p, 2);
            st.time.i = get_nr(t, p, 2);
            st.time.s = get_nr(t, p, 2);
            if t.at(*p) == b'.' {
                st.time.us = get_frac_nr(t, p);
                if t.at(*p) != 0 {
                    st.zone_into_time(t, p);
                }
            }
        }
        PgYdotd => {
            if !st.have_date() {
                return;
            }
            let (y, len) = get_nr_ex(t, p, 4);
            st.time.y = y;
            st.time.d = get_nr(t, p, 3);
            st.time.m = 1;
            process_year(&mut st.time.y, len);
        }
        IsoWeekday | IsoWeek => {
            if !st.have_date() {
                return;
            }
            st.have_relative();
            st.time.y = get_nr(t, p, 4);
            let w = get_nr(t, p, 2);
            let d = if rule == IsoWeekday {
                get_nr(t, p, 1)
            } else {
                1
            };
            st.time.m = 1;
            st.time.d = 1;
            st.time.relative.d = daynr_from_weeknr(st.time.y, w, d);
        }
        PgTextReverse => {
            if !st.have_date() {
                return;
            }
            let (y, len) = get_nr_ex(t, p, 4);
            st.time.y = y;
            st.time.m = get_month(t, p);
            st.time.d = get_nr(t, p, 2);
            process_year(&mut st.time.y, len);
        }
        Clf => {
            if !st.have_time() || !st.have_date() {
                return;
            }
            st.time.d = get_nr(t, p, 2);
            st.time.m = get_month(t, p);
            st.time.y = get_nr(t, p, 4);
            st.time.h = get_nr(t, p, 2);
            st.time.i = get_nr(t, p, 2);
            st.time.s = get_nr(t, p, 2);
            eat_spaces(t, p);
            st.zone_into_time(t, p);
        }
        Year4 => st.time.y = get_nr(t, p, 4),
        Ago => {
            let r = &mut st.time.relative;
            r.y = -r.y;
            r.m = -r.m;
            r.d = -r.d;
            r.h = -r.h;
            r.i = -r.i;
            r.s = -r.s;
            r.weekday = -r.weekday;
            if r.weekday == 0 {
                r.weekday = -7;
            }
            if r.have_special_relative && r.special.kind == SPECIAL_WEEKDAY {
                r.special.amount = -r.special.amount;
            }
        }
        DayText => {
            st.have_relative();
            st.have_weekday_relative();
            st.unhave_time();
            let (_, mult) = lookup_relunit(t, p).expect("a daytext token names a unit");
            st.time.relative.weekday = mult;
            if st.time.relative.weekday_behavior != 2 {
                st.time.relative.weekday_behavior = 1;
            }
        }
        RelativeTextWeek | RelativeText => {
            st.have_relative();
            let mut behavior = 0;
            while t.at(*p) != 0 {
                let before = *p;
                let i = get_relative_text(t, p, &mut behavior);
                eat_spaces(t, p);
                st.set_relative(t, p, i, behavior, false);
                if rule == RelativeTextWeek {
                    st.time.relative.weekday_behavior = 2;
                    if !st.time.relative.have_weekday_relative {
                        st.have_weekday_relative();
                        st.time.relative.weekday = 1;
                    }
                }
                if *p == before {
                    break;
                }
            }
        }
        MonthText => {
            if !st.have_date() {
                return;
            }
            st.time.m = lookup_month(t, p);
        }
        Timezone => {
            if !st.have_tz() {
                return;
            }
            eat_spaces(t, p);
            st.zone_into_time(t, p);
        }
        DateShortWithTime12 => {
            if !st.have_date() {
                return;
            }
            st.time.m = get_month(t, p);
            st.time.d = get_nr(t, p, 2);
            if !st.have_time() {
                return;
            }
            st.time.h = get_nr(t, p, 2);
            st.time.i = get_nr(t, p, 2);
            if matches!(t.at(*p), b':' | b'.') {
                st.time.s = get_nr(t, p, 2);
                if t.at(*p) == b'.' {
                    st.time.us = get_frac_nr(t, p);
                }
            }
            st.time.h += meridian(t, p, st.time.h);
        }
        DateShortWithTime24 => {
            if !st.have_date() {
                return;
            }
            st.time.m = get_month(t, p);
            st.time.d = get_nr(t, p, 2);
            if !st.have_time() {
                return;
            }
            st.time.h = get_nr(t, p, 2);
            st.time.i = get_nr(t, p, 2);
            if t.at(*p) == b':' {
                st.time.s = get_nr(t, p, 2);
                if t.at(*p) == b'.' {
                    st.time.us = get_frac_nr(t, p);
                }
            }
            if t.at(*p) != 0 {
                st.zone_into_time(t, p);
            }
        }
        Relative => {
            st.have_relative();
            while t.at(*p) != 0 {
                let before = *p;
                let i = st.get_signed_nr(t, p, 24);
                eat_spaces(t, p);
                st.set_relative(t, p, i, 1, true);
                if *p == before {
                    break;
                }
            }
        }
        Separator | Space | Nul => {}
        Any => st.error("Unexpected character"),
    }
}

/// `timelib_strtotime`.
pub fn strtotime(input: &[u8]) -> (Time, Errors) {
    // The trim of the shipped parse_date.c (`s < e`; the .re beside it still
    // reads `s <= e`): it never trims the LAST character away, so an
    // all-blank string is one blank, not empty.
    let mut s = 0;
    let mut e = input.len();
    if !input.is_empty() {
        let mut last = input.len() - 1;
        while input[s].is_ascii_whitespace_c() && s < last {
            s += 1;
        }
        while input[last].is_ascii_whitespace_c() && last > s {
            last -= 1;
        }
        e = last + 1;
    }
    let mut st = State {
        time: Time::default(),
        errors: Errors::default(),
        tok: 0,
        tok_char: 0,
    };
    if e == s {
        st.error("Empty string");
        let t = &mut st.time;
        t.y = UNSET;
        t.d = UNSET;
        t.m = UNSET;
        t.h = UNSET;
        t.i = UNSET;
        t.s = UNSET;
        t.us = UNSET;
        t.dst = UNSET;
        t.z = UNSET;
        return (st.time, st.errors);
    }
    let len = e - s;
    // The scanner's buffer: the text and YYMAXFILL NULs, which a rule may read
    // into (a meridian or a year-less date ends on one).
    let mut buf = input[s..e].to_vec();
    buf.extend_from_slice(&[0u8; 32]);
    {
        let t = &mut st.time;
        t.y = UNSET;
        t.d = UNSET;
        t.m = UNSET;
        t.h = UNSET;
        t.i = UNSET;
        t.s = UNSET;
        t.us = UNSET;
        t.z = UNSET;
        t.dst = UNSET;
        t.relative.days = UNSET;
    }
    let mut pos = 0;
    while pos <= len {
        let (rule, end) = longest_match(&buf, pos);
        st.tok = pos;
        st.tok_char = buf[pos];
        let text = &buf[pos..end];
        let cut = text.iter().position(|&c| c == 0).unwrap_or(text.len());
        let tok = Tok {
            b: text[..cut].to_vec(),
        };
        act(&mut st, rule, &tok);
        pos = end;
    }
    // At end of input the token start is where the scanner stood when it ran
    // out: one past the terminating NUL.
    st.tok = pos;
    st.tok_char = 0;
    if st.time.have_time != 0 && !valid_time(st.time.h, st.time.i, st.time.s) {
        st.warning("The parsed time was invalid");
    }
    if st.time.have_date && !valid_date(st.time.y, st.time.m, st.time.d) {
        st.warning("The parsed date was invalid");
    }
    (st.time, st.errors)
}

trait CIsSpace {
    fn is_ascii_whitespace_c(&self) -> bool;
}

impl CIsSpace for u8 {
    /// C `isspace` in the "C" locale: also `\v`, which Rust's test leaves out.
    fn is_ascii_whitespace_c(&self) -> bool {
        matches!(*self, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
    }
}

/// The default-zone "now" every relative parse fills its holes from: `ts` in
/// `tz`, as `timelib_unixtime2local` breaks it down.
pub fn now_in(tz: &TzInfo, ts: i64) -> Time {
    let mut now = Time {
        tz_info: Some(tz.clone()),
        zone_type: ZONETYPE_ID,
        ..Time::default()
    };
    unixtime2local(&mut now, ts);
    now
}

/// `strtotime($s, $base)` against the default zone `tz`: the timestamp, or
/// `None` when the scanner reported an error.
pub fn strtotime_ts(input: &[u8], base: i64, tz: &TzInfo) -> Option<i64> {
    if input.is_empty() {
        return None;
    }
    let now = now_in(tz, base);
    let (mut t, errors) = strtotime(input);
    if !errors.errors.is_empty() {
        return None;
    }
    fill_holes(&mut t, &now, false);
    update_ts(&mut t, Some(tz));
    Some(t.sse)
}

// ── timelib_parse_from_format ────────────────────────────────────────────────

/// `timelib_parse_from_format` with the default format map.
pub fn parse_from_format(format: &[u8], string: &[u8]) -> (Time, Errors) {
    let mut st = State {
        time: Time::default(),
        errors: Errors::default(),
        tok: 0,
        tok_char: 0,
    };
    {
        let t = &mut st.time;
        t.y = UNSET;
        t.d = UNSET;
        t.m = UNSET;
        t.h = UNSET;
        t.i = UNSET;
        t.s = UNSET;
        t.us = UNSET;
        t.z = UNSET;
        t.dst = UNSET;
    }
    let tok = Tok {
        b: string.iter().copied().take_while(|&c| c != 0).collect(),
    };
    let t = &tok;
    let f = Tok {
        b: format.iter().copied().take_while(|&c| c != 0).collect(),
    };
    let mut fp = 0usize;
    let mut p = 0usize;
    let mut allow_extra = false;
    fn pbf(st: &mut State, errors: bool, msg: &'static str, t: &Tok, at: usize) {
        let m = Message {
            position: at,
            character: t.at(at),
            message: msg,
        };
        if errors {
            st.errors.errors.push(m);
        } else {
            st.errors.warnings.push(m);
        }
    }
    let is_sep = |c: u8| matches!(c, b';' | b':' | b'/' | b'.' | b',' | b'-' | b'(' | b')');
    while f.at(fp) != 0 && t.at(p) != 0 {
        let begin = p;
        let fc = f.at(fp);
        let check_number = |st: &mut State, p: usize| {
            if !is_digit(t.at(p)) {
                pbf(st, true, "Unexpected data found.", t, begin);
            }
        };
        match fc {
            b'D' | b'l' => match lookup_relunit(t, &mut p) {
                None => pbf(&mut st, true, "A textual day could not be found", t, begin),
                Some((_, mult)) => {
                    st.time.have_relative = true;
                    st.time.relative.have_weekday_relative = true;
                    st.time.relative.weekday = mult;
                    st.time.relative.weekday_behavior = 1;
                }
            },
            b'j' | b'd' => {
                check_number(&mut st, p);
                st.time.d = get_nr(t, &mut p, 2);
                if st.time.d == UNSET {
                    pbf(
                        &mut st,
                        true,
                        "A two digit day could not be found",
                        t,
                        begin,
                    );
                } else {
                    st.time.have_date = true;
                }
            }
            b'S' => skip_day_suffix(t, &mut p),
            b'z' => {
                check_number(&mut st, p);
                if st.time.y == UNSET {
                    pbf(
                        &mut st,
                        true,
                        "A 'day of year' can only come after a year has been found",
                        t,
                        begin,
                    );
                }
                let tmp = get_nr(t, &mut p, 3);
                if tmp == UNSET {
                    pbf(
                        &mut st,
                        true,
                        "A three digit day-of-year could not be found",
                        t,
                        begin,
                    );
                } else if st.time.y != UNSET {
                    st.time.have_date = true;
                    st.time.m = 1;
                    st.time.d = tmp + 1;
                    do_normalize(&mut st.time);
                }
            }
            b'n' | b'm' => {
                check_number(&mut st, p);
                st.time.m = get_nr(t, &mut p, 2);
                if st.time.m == UNSET {
                    pbf(
                        &mut st,
                        true,
                        "A two digit month could not be found",
                        t,
                        begin,
                    );
                } else {
                    st.time.have_date = true;
                }
            }
            b'M' | b'F' => {
                let tmp = lookup_month(t, &mut p);
                if tmp == 0 {
                    pbf(
                        &mut st,
                        true,
                        "A textual month could not be found",
                        t,
                        begin,
                    );
                } else {
                    st.time.have_date = true;
                    st.time.m = tmp;
                }
            }
            b'y' => {
                check_number(&mut st, p);
                let (y, len) = get_nr_ex(t, &mut p, 2);
                st.time.y = y;
                if y == UNSET {
                    pbf(
                        &mut st,
                        true,
                        "A two digit year could not be found",
                        t,
                        begin,
                    );
                } else {
                    st.time.have_date = true;
                    process_year(&mut st.time.y, len);
                }
            }
            b'Y' => {
                check_number(&mut st, p);
                st.time.y = get_nr(t, &mut p, 4);
                if st.time.y == UNSET {
                    pbf(
                        &mut st,
                        true,
                        "A four digit year could not be found",
                        t,
                        begin,
                    );
                } else {
                    st.time.have_date = true;
                }
            }
            b'x' | b'X' => {
                if !matches!(t.at(p), b'+' | b'-') && !is_digit(t.at(p)) {
                    pbf(&mut st, true, "Unexpected data found.", t, begin);
                }
                st.time.y = st.get_signed_nr(t, &mut p, 19);
                st.time.have_date = true;
            }
            b'h' | b'g' => {
                check_number(&mut st, p);
                st.time.h = get_nr(t, &mut p, 2);
                if st.time.h == UNSET {
                    pbf(
                        &mut st,
                        true,
                        "A two digit hour could not be found",
                        t,
                        begin,
                    );
                } else if st.time.h > 12 {
                    pbf(&mut st, true, "Hour cannot be higher than 12", t, begin);
                } else {
                    st.time.have_time = 1;
                }
            }
            b'H' | b'G' => {
                check_number(&mut st, p);
                st.time.h = get_nr(t, &mut p, 2);
                if st.time.h == UNSET {
                    pbf(
                        &mut st,
                        true,
                        "A two digit hour could not be found",
                        t,
                        begin,
                    );
                } else {
                    st.time.have_time = 1;
                }
            }
            b'a' | b'A' => {
                if st.time.h == UNSET {
                    pbf(
                        &mut st,
                        true,
                        "Meridian can only come after an hour has been found",
                        t,
                        begin,
                    );
                }
                let tmp = meridian_with_check(t, &mut p, st.time.h);
                if tmp == UNSET {
                    pbf(&mut st, true, "A meridian could not be found", t, begin);
                } else {
                    st.time.have_time = 1;
                    if st.time.h != UNSET {
                        st.time.h += tmp;
                    }
                }
            }
            b'i' => {
                check_number(&mut st, p);
                let (min, len) = get_nr_ex(t, &mut p, 2);
                if min == UNSET || len != 2 {
                    pbf(
                        &mut st,
                        true,
                        "A two digit minute could not be found",
                        t,
                        begin,
                    );
                } else {
                    st.time.have_time = 1;
                    st.time.i = min;
                }
            }
            b's' => {
                check_number(&mut st, p);
                let (sec, len) = get_nr_ex(t, &mut p, 2);
                if sec == UNSET || len != 2 {
                    pbf(
                        &mut st,
                        true,
                        "A two digit second could not be found",
                        t,
                        begin,
                    );
                } else {
                    st.time.have_time = 1;
                    st.time.s = sec;
                }
            }
            b'u' | b'v' => {
                check_number(&mut st, p);
                let tp = p;
                let digits = if fc == b'u' { 6 } else { 3 };
                let f = get_nr(t, &mut p, digits);
                if f == UNSET || p - tp < 1 {
                    pbf(
                        &mut st,
                        true,
                        if fc == b'u' {
                            "A six digit microsecond could not be found"
                        } else {
                            "A three digit millisecond could not be found"
                        },
                        t,
                        begin,
                    );
                } else if fc == b'u' {
                    st.time.us = (f as f64 * 10f64.powi(6 - (p - tp) as i32)) as i64;
                } else {
                    st.time.us = (f as f64 * 10f64.powi(3 - (p - tp) as i32) * 1000.0) as i64;
                }
            }
            b' ' => eat_spaces(t, &mut p),
            b'U' => {
                if !matches!(t.at(p), b'+' | b'-') && !is_digit(t.at(p)) {
                    pbf(&mut st, true, "Unexpected data found.", t, begin);
                }
                let tmp = st.get_signed_nr(t, &mut p, 24);
                let tm = &mut st.time;
                tm.have_zone = 1;
                tm.sse = tmp;
                tm.is_localtime = true;
                tm.zone_type = ZONETYPE_OFFSET;
                tm.z = 0;
                tm.dst = 0;
                update_from_sse(tm);
            }
            b'#' => {
                if is_sep(t.at(p)) {
                    p += 1;
                } else {
                    pbf(
                        &mut st,
                        true,
                        "The separation symbol ([;:/.,-]) could not be found",
                        t,
                        begin,
                    );
                }
            }
            b';' | b':' | b'/' | b'.' | b',' | b'-' | b'(' | b')' => {
                if t.at(p) != fc {
                    pbf(
                        &mut st,
                        true,
                        "The separation symbol could not be found",
                        t,
                        begin,
                    );
                } else {
                    p += 1;
                }
            }
            b'!' => reset_fields(&mut st.time),
            b'|' => reset_unset_fields(&mut st.time),
            b'?' => p += 1,
            b'\\' => {
                if f.at(fp + 1) == 0 {
                    pbf(&mut st, true, "Escaped character expected", t, begin);
                } else {
                    fp += 1;
                    if t.at(p) != f.at(fp) {
                        pbf(
                            &mut st,
                            true,
                            "The escaped character could not be found",
                            t,
                            begin,
                        );
                    } else {
                        p += 1;
                    }
                }
            }
            b'*' => {
                p += 1;
                while !matches!(
                    t.at(p),
                    b' ' | b'\t' | b'.' | b',' | b':' | b';' | b'/' | b'-' | b'0'..=b'9' | 0
                ) {
                    p += 1;
                }
            }
            b'+' => allow_extra = true,
            b'e' | b'P' | b'p' | b'T' | b'O' => {
                let (z, not_found) = parse_zone(&mut st.time, t, &mut p);
                st.time.z = z;
                if not_found {
                    pbf(
                        &mut st,
                        true,
                        "The timezone could not be found in the database",
                        t,
                        begin,
                    );
                } else {
                    st.time.have_zone = 1;
                }
            }
            _ => {
                if fc != t.at(p) {
                    pbf(
                        &mut st,
                        true,
                        "The format separator does not match",
                        t,
                        begin,
                    );
                }
                p += 1;
            }
        }
        fp += 1;
    }
    if t.at(p) != 0 {
        pbf(&mut st, !allow_extra, "Trailing data", t, p);
    }
    while f.at(fp) != 0 {
        match f.at(fp) {
            b'!' => reset_fields(&mut st.time),
            b'|' => reset_unset_fields(&mut st.time),
            b'+' => {}
            _ => {
                pbf(
                    &mut st,
                    true,
                    "Not enough data available to satisfy format",
                    t,
                    p,
                );
                break;
            }
        }
        fp += 1;
    }
    let tm = &mut st.time;
    if tm.h != UNSET || tm.i != UNSET || tm.s != UNSET || tm.us != UNSET {
        for v in [&mut tm.h, &mut tm.i, &mut tm.s, &mut tm.us] {
            if *v == UNSET {
                *v = 0;
            }
        }
    }
    if st.time.h != UNSET
        && st.time.i != UNSET
        && st.time.s != UNSET
        && !valid_time(st.time.h, st.time.i, st.time.s)
    {
        pbf(&mut st, false, "The parsed time was invalid", t, p);
    }
    if st.time.y != UNSET
        && st.time.m != UNSET
        && st.time.d != UNSET
        && !valid_date(st.time.y, st.time.m, st.time.d)
    {
        pbf(&mut st, false, "The parsed date was invalid", t, p);
    }
    (st.time, st.errors)
}

fn reset_fields(t: &mut Time) {
    t.y = 1970;
    t.m = 1;
    t.d = 1;
    t.h = 0;
    t.i = 0;
    t.s = 0;
    t.us = 0;
    t.tz_info = None;
}

fn reset_unset_fields(t: &mut Time) {
    for (v, d) in [
        (&mut t.y, 1970),
        (&mut t.m, 1),
        (&mut t.d, 1),
        (&mut t.h, 0),
        (&mut t.i, 0),
        (&mut t.s, 0),
        (&mut t.us, 0),
    ] {
        if *v == UNSET {
            *v = d;
        }
    }
}
