//! The engine behind the `DateTime` / `DateTimeImmutable` / `DateTimeZone` /
//! `DateInterval` prelude classes and the procedural `date_*` / `timezone_*`
//! functions: ports of the `php_date.c` routines over `crate::timelib`.
//!
//! An object's whole state lives in the properties the reference SHOWS for it
//! (`date_object_to_hash` and friends), so `var_dump`, `print_r`, `(array)`,
//! `json_encode`, `serialize` and `clone` need nothing special:
//!
//! * a date: `date` (`x-m-d H:i:s.u`, local wall time), `timezone_type` and
//!   `timezone` — from which the timelib time is rebuilt exactly;
//! * a zone: `timezone_type` and `timezone`;
//! * an interval: `y m d h i s f invert days from_string`, or — for one made
//!   from a relative string — only `from_string` and `date_string`, re-read
//!   when it is used, as the reference re-creates such an interval.
//!
//! The prelude methods call the `__phplang_*` helpers here; a failure inside
//! one is raised as from an internal method (see
//! [`host::throw_as_internal_method`]).

use crate::host::{self, with_host};
use crate::stdlib::common::*;
use crate::timelib::{
    self as tl, RelTime, Time, TzInfo, UNSET, ZONETYPE_ABBR, ZONETYPE_ID, ZONETYPE_OFFSET,
};
use fusevm::Value;
use std::cell::RefCell;

thread_local! {
    /// `DATEG(last_errors)`: the warnings and errors of the last parse that
    /// had any, for `date_get_last_errors()` / `DateTime::getLastErrors()`.
    static LAST_ERRORS: RefCell<Option<tl::Errors>> = const { RefCell::new(None) };
}

fn update_errors_warnings(e: &tl::Errors) {
    LAST_ERRORS.with(|l| {
        *l.borrow_mut() = (!e.warnings.is_empty() || !e.errors.is_empty()).then(|| e.clone());
    });
}

/// A `DateTimeZone`'s state (`php_timezone_obj`).
#[derive(Clone, Debug)]
pub enum Zone {
    Id(TzInfo),
    Offset(i64),
    Abbr { z: i64, dst: i64, abbr: String },
}

impl Zone {
    fn kind(&self) -> i64 {
        match self {
            Zone::Id(_) => ZONETYPE_ID as i64,
            Zone::Offset(_) => ZONETYPE_OFFSET as i64,
            Zone::Abbr { .. } => ZONETYPE_ABBR as i64,
        }
    }

    /// `php_timezone_to_string`.
    fn name(&self) -> String {
        match self {
            Zone::Id(tz) => tz.name.clone(),
            Zone::Offset(off) => tl::offset_str(*off),
            Zone::Abbr { abbr, .. } => abbr.clone(),
        }
    }

    /// The zone a time carries — `set_timezone_from_timelib_time`.
    fn of(t: &Time) -> Option<Zone> {
        match t.zone_type {
            ZONETYPE_ID => t.tz_info.clone().map(Zone::Id),
            ZONETYPE_OFFSET => Some(Zone::Offset(t.z)),
            ZONETYPE_ABBR => Some(Zone::Abbr {
                z: t.z,
                dst: t.dst,
                abbr: t.tz_abbr.clone().unwrap_or_default(),
            }),
            _ => None,
        }
    }

    /// Rebuild a zone from its two properties.
    fn from_props(kind: i64, name: &str) -> Option<Zone> {
        match kind {
            3 => tl::tz_lookup(name).map(Zone::Id),
            1 => parse_offset(name).map(Zone::Offset),
            2 => {
                let (dst, off) = tl::abbreviation(name).unwrap_or((0, 0));
                Some(Zone::Abbr {
                    z: off - dst * 3600,
                    dst,
                    abbr: name.to_string(),
                })
            }
            _ => None,
        }
    }
}

/// `+05:30` / `-04:00:15` back to seconds east.
fn parse_offset(s: &str) -> Option<i64> {
    let (sign, rest) = match s.as_bytes().first()? {
        b'+' => (1, &s[1..]),
        b'-' => (-1, &s[1..]),
        _ => return None,
    };
    let mut parts = rest.split(':').map(|p| p.parse::<i64>().ok());
    let h = parts.next()??;
    let m = parts.next().flatten().unwrap_or(0);
    let sec = parts.next().flatten().unwrap_or(0);
    Some(sign * (h * 3600 + m * 60 + sec))
}

// ── object state ─────────────────────────────────────────────────────────────

fn prop(obj: &Value, name: &str) -> Value {
    with_host(|h| h.prop_get(obj, name))
}

fn set_prop(obj: &Value, name: &str, v: Value) {
    with_host(|h| h.prop_set(obj, name, v));
}

fn class_of(obj: &Value) -> String {
    with_host(|h| h.object_class(obj)).unwrap_or_default()
}

fn is_a(obj: &Value, class: &str) -> bool {
    with_host(|h| {
        h.object_class(obj)
            .is_some_and(|c| h.class_is_a_pub(&c.to_ascii_lowercase(), class))
    })
}

/// The timelib time a date object holds, or `None` before its constructor
/// ran.
pub fn load_date(obj: &Value) -> Option<Time> {
    with_host(|h| load_date_in(h, obj))
}

/// [`load_date`] for a caller already holding the host.
pub fn load_date_in(h: &host::PhpHost, obj: &Value) -> Option<Time> {
    let date = match h.prop_get(obj, "date") {
        Value::Str(s) => s.to_string(),
        _ => return None,
    };
    let kind = h.to_number(&h.prop_get(obj, "timezone_type")).to_int();
    let zone_name = h.to_str(&h.prop_get(obj, "timezone"));
    let zone = Zone::from_props(kind, &zone_name).unwrap_or_else(|| Zone::Id(utc()));
    // `x-m-d H:i:s.u`: an optional sign, the year, then fixed fields.
    let b = date.as_bytes();
    let neg = b.first() == Some(&b'-');
    let start = usize::from(matches!(b.first(), Some(b'-' | b'+')));
    let dash = date[start..].find('-')? + start;
    let year: i64 = date[start..dash].parse().ok()?;
    let rest = &date[dash + 1..];
    let num = |r: std::ops::Range<usize>| rest.get(r).and_then(|s| s.parse::<i64>().ok());
    let mut t = Time {
        y: if neg { -year } else { year },
        m: num(0..2)?,
        d: num(3..5)?,
        h: num(6..8)?,
        i: num(9..11)?,
        s: num(12..14)?,
        us: num(15..21).unwrap_or(0),
        ..Time::default()
    };
    apply_zone(&mut t, &zone);
    tl::update_ts(&mut t, None);
    Some(t)
}

fn apply_zone(t: &mut Time, zone: &Zone) {
    match zone {
        Zone::Id(tz) => tl::set_timezone(t, tz),
        Zone::Offset(off) => tl::set_timezone_from_offset(t, *off),
        Zone::Abbr { z, dst, abbr } => tl::set_timezone_from_abbr(t, *z, *dst, abbr),
    }
    t.is_localtime = true;
}

/// `date_object_to_hash`: write the three properties.
pub fn store_date(obj: &Value, t: &Time) {
    set_prop(
        obj,
        "date",
        Value::str(tl::date_format(b"x-m-d H:i:s.u", t, true)),
    );
    if t.is_localtime {
        if let Some(zone) = Zone::of(t) {
            set_prop(obj, "timezone_type", Value::int(zone.kind()));
            set_prop(obj, "timezone", Value::str(zone.name()));
        }
    }
}

fn load_zone(obj: &Value) -> Option<Zone> {
    let kind = match prop(obj, "timezone_type") {
        Value::Int(k) => k,
        _ => return None,
    };
    let name = with_host(|h| h.to_str(&h.prop_get(obj, "timezone")));
    Zone::from_props(kind, &name)
}

fn store_zone(obj: &Value, zone: &Zone) {
    set_prop(obj, "timezone_type", Value::int(zone.kind()));
    set_prop(obj, "timezone", Value::str(zone.name()));
}

/// The relative time a `DateInterval` holds.
pub fn load_interval(obj: &Value) -> Option<RelTime> {
    let truthy = |v: &Value| with_host(|h| h.is_truthy(v));
    if truthy(&prop(obj, "from_string")) {
        let s = with_host(|h| h.to_str(&h.prop_get(obj, "date_string")));
        let (t, _) = tl::strtotime(s.as_bytes());
        return Some(t.relative);
    }
    let int = |n: &str| with_host(|h| h.to_number(&h.prop_get(obj, n)).to_int());
    if !with_host(|h| h.obj_has_prop(obj, "y")) {
        return None;
    }
    let f = with_host(|h| h.to_number(&h.prop_get(obj, "f")).to_float());
    let days = match prop(obj, "days") {
        Value::Bool(false) | Value::Undef => UNSET,
        v => with_host(|h| h.to_number(&v).to_int()),
    };
    Some(RelTime {
        y: int("y"),
        m: int("m"),
        d: int("d"),
        h: int("h"),
        i: int("i"),
        s: int("s"),
        us: (f * 1_000_000.0).round() as i64,
        invert: int("invert") != 0,
        days,
        ..RelTime::default()
    })
}

/// `date_interval_object_to_hash` for an interval with real fields.
fn store_interval(obj: &Value, rt: &RelTime) {
    set_prop(obj, "y", Value::int(rt.y));
    set_prop(obj, "m", Value::int(rt.m));
    set_prop(obj, "d", Value::int(rt.d));
    set_prop(obj, "h", Value::int(rt.h));
    set_prop(obj, "i", Value::int(rt.i));
    set_prop(obj, "s", Value::int(rt.s));
    set_prop(obj, "f", Value::float(rt.us as f64 / 1_000_000.0));
    set_prop(obj, "invert", Value::int(rt.invert as i64));
    set_prop(
        obj,
        "days",
        if rt.days != UNSET {
            Value::int(rt.days)
        } else {
            Value::bool(false)
        },
    );
    set_prop(obj, "from_string", Value::bool(false));
}

fn new_interval(rt: &RelTime) -> Value {
    let obj = with_host(|h| h.new_object_bare("DateInterval", Vec::new()));
    store_interval(&obj, rt);
    obj
}

fn utc() -> TzInfo {
    tl::tz_lookup("UTC").expect("UTC resolves")
}

fn default_tz() -> TzInfo {
    crate::stdlib::datetime::default_tz()
}

/// `php_date_get_current_time_with_fraction`.
fn now_with_fraction() -> (i64, i64) {
    let now = chrono::Utc::now();
    (now.timestamp(), now.timestamp_subsec_micros() as i64)
}

fn parse_failure(subject: &str, e: &tl::Message) -> String {
    let c = if e.character == 0 {
        ' '
    } else {
        e.character as char
    };
    format!(
        "Failed to parse time string ({subject}) at position {} ({c}): {}",
        e.position, e.message
    )
}

/// `php_date_initialize`: the time `time_str` names (or `format` reads), in
/// `zone` or the default. `Err` is the first parse error, worded for the
/// constructor's exception.
pub fn initialize(
    time_str: &str,
    format: Option<&str>,
    zone: Option<&Zone>,
) -> Result<Time, String> {
    let subject = if format.is_none() && time_str.is_empty() {
        "now"
    } else {
        time_str
    };
    let (mut t, errors) = match format {
        Some(f) => tl::parse_from_format(f.as_bytes(), time_str.as_bytes()),
        None => tl::strtotime(subject.as_bytes()),
    };
    update_errors_warnings(&errors);
    if let Some(e) = errors.errors.first() {
        return Err(parse_failure(subject, e));
    }
    let (kind, tzi) = match zone {
        Some(Zone::Id(tz)) => (ZONETYPE_ID, Some(tz.clone())),
        Some(z) => (z.kind() as u8, None),
        None => (
            ZONETYPE_ID,
            Some(t.tz_info.clone().unwrap_or_else(default_tz)),
        ),
    };
    let mut now = Time {
        zone_type: kind,
        ..Time::default()
    };
    match zone {
        Some(Zone::Offset(off)) => now.z = *off,
        Some(Zone::Abbr { z, dst, abbr }) => {
            now.z = *z;
            now.dst = *dst;
            now.tz_abbr = Some(abbr.clone());
        }
        _ => now.tz_info = tzi.clone(),
    }
    let (sec, usec) = now_with_fraction();
    tl::unixtime2local(&mut now, sec);
    now.us = usec;
    if format.is_none() && subject == "now" {
        return Ok(now);
    }
    tl::fill_holes(&mut t, &now, format.is_some());
    tl::update_ts(&mut t, tzi.as_ref());
    tl::update_from_sse(&mut t);
    t.have_relative = false;
    Ok(t)
}

/// `php_date_modify`: `Err` carries the warning text.
pub fn modify(t: &mut Time, s: &str) -> Result<(), String> {
    let (tmp, errors) = tl::strtotime(s.as_bytes());
    update_errors_warnings(&errors);
    if let Some(e) = errors.errors.first() {
        return Err(parse_failure(s, e));
    }
    t.relative = tmp.relative.clone();
    t.have_relative = tmp.have_relative;
    if tmp.y != UNSET {
        t.y = tmp.y;
    }
    if tmp.m != UNSET {
        t.m = tmp.m;
    }
    if tmp.d != UNSET {
        t.d = tmp.d;
    }
    if tmp.h != UNSET {
        t.h = tmp.h;
        if tmp.i != UNSET {
            t.i = tmp.i;
            t.s = if tmp.s != UNSET { tmp.s } else { 0 };
        } else {
            t.i = 0;
            t.s = 0;
        }
    }
    if tmp.us != UNSET {
        t.us = tmp.us;
    }
    if tmp.y == 1970
        && tmp.m == 1
        && tmp.d == 1
        && tmp.h == 0
        && tmp.i == 0
        && tmp.s == 0
        && tmp.us == 0
        && tmp.have_zone != 0
        && tmp.zone_type == ZONETYPE_OFFSET
        && tmp.z == 0
        && tmp.dst == 0
    {
        tl::set_timezone_from_offset(t, 0);
    }
    tl::update_ts(t, None);
    tl::update_from_sse(t);
    t.have_relative = false;
    t.relative = RelTime::default();
    Ok(())
}

const SUB_SPECIAL: &str =
    "Only non-special relative time specifications are supported for subtraction";

// ── the prelude's helpers ────────────────────────────────────────────────────

fn uninitialized(obj: &Value) -> Result<Value, String> {
    let class = class_of(obj);
    host::throw_as_internal_method(
        "Error",
        &format!("The {class} object has not been correctly initialized by its constructor"),
    )
}

/// The calling method's name as a diagnostic prefixes it: `DateTime::modify`.
fn method_name(obj: &Value, method: &str) -> String {
    let base = if is_a(obj, "datetimeimmutable") {
        "DateTimeImmutable"
    } else {
        "DateTime"
    };
    format!("{base}::{method}")
}

fn zone_arg(v: &Value) -> Option<Zone> {
    match v {
        Value::Obj(_) if is_a(v, "datetimezone") => load_zone(v),
        _ => None,
    }
}

/// `DateTimeZone` from a string — `timezone_initialize`.
fn timezone_initialize(s: &str) -> Result<Zone, String> {
    let (t, z, not_found, read) = tl::parse_zone_str(s.as_bytes());
    if !(-100 * 3600 + 1..100 * 3600).contains(&z) {
        return Err(format!("Timezone offset is out of range ({s})"));
    }
    if not_found || read < s.len() {
        return Err(format!("Unknown or bad timezone ({s})"));
    }
    let mut t = t;
    t.z = z;
    Zone::of(&t).ok_or_else(|| format!("Unknown or bad timezone ({s})"))
}

/// `date_interval_initialize`.
fn interval_initialize(spec: &str) -> Result<RelTime, String> {
    tl::parse_iso_interval(spec.as_bytes())
}

fn date_interval_format(fmt: &[u8], t: &RelTime) -> String {
    let mut out = String::new();
    let mut spec = false;
    for (i, &c) in fmt.iter().enumerate() {
        if !spec {
            if c == b'%' {
                spec = true;
            } else if !(0x80..0xC0).contains(&c) {
                let len = match c {
                    0..=0x7F => 1,
                    0xC0..=0xDF => 2,
                    0xE0..=0xEF => 3,
                    _ => 4,
                };
                if let Some(s) = fmt
                    .get(i..i + len)
                    .and_then(|b| std::str::from_utf8(b).ok())
                {
                    out.push_str(s);
                }
            }
            continue;
        }
        spec = false;
        let s = match c {
            b'Y' => format!("{:02}", t.y as i32),
            b'y' => (t.y as i32).to_string(),
            b'M' => format!("{:02}", t.m as i32),
            b'm' => (t.m as i32).to_string(),
            b'D' => format!("{:02}", t.d as i32),
            b'd' => (t.d as i32).to_string(),
            b'H' => format!("{:02}", t.h as i32),
            b'h' => (t.h as i32).to_string(),
            b'I' => format!("{:02}", t.i as i32),
            b'i' => (t.i as i32).to_string(),
            b'S' => format!("{:02}", t.s),
            b's' => t.s.to_string(),
            b'F' => format!("{:06}", t.us),
            b'f' => t.us.to_string(),
            b'a' => {
                if t.days as i32 != UNSET as i32 {
                    (t.days as i32).to_string()
                } else {
                    "(unknown)".to_string()
                }
            }
            b'r' => if t.invert { "-" } else { "" }.to_string(),
            b'R' => if t.invert { "-" } else { "+" }.to_string(),
            b'%' => "%".to_string(),
            _ => {
                let mut s = String::from("%");
                if c < 0x80 {
                    s.push(c as char);
                }
                s
            }
        };
        out.push_str(&s);
    }
    out
}

fn bool_v(b: bool) -> Value {
    Value::bool(b)
}

/// `php_date_do_return_parsed_time`.
fn parsed_time_array(t: &Time, e: &tl::Errors) -> Value {
    let elem = |v: i64| {
        if v == UNSET {
            Value::bool(false)
        } else {
            Value::int(v)
        }
    };
    let msgs = |list: &[tl::Message]| {
        make_map(
            list.iter()
                .map(|m| (Value::int(m.position as i64), Value::str(m.message)))
                .collect(),
        )
    };
    let mut pairs = vec![
        (Value::str("year"), elem(t.y)),
        (Value::str("month"), elem(t.m)),
        (Value::str("day"), elem(t.d)),
        (Value::str("hour"), elem(t.h)),
        (Value::str("minute"), elem(t.i)),
        (Value::str("second"), elem(t.s)),
        (
            Value::str("fraction"),
            if t.us == UNSET {
                Value::bool(false)
            } else {
                Value::float(t.us as f64 / 1_000_000.0)
            },
        ),
        (
            Value::str("warning_count"),
            Value::int(e.warnings.len() as i64),
        ),
        (Value::str("warnings"), msgs(&e.warnings)),
        (Value::str("error_count"), Value::int(e.errors.len() as i64)),
        (Value::str("errors"), msgs(&e.errors)),
        (Value::str("is_localtime"), bool_v(t.is_localtime)),
    ];
    if t.is_localtime {
        pairs.push((Value::str("zone_type"), elem(t.zone_type as i64)));
        match t.zone_type {
            ZONETYPE_OFFSET => {
                pairs.push((Value::str("zone"), elem(t.z)));
                pairs.push((Value::str("is_dst"), bool_v(t.dst != 0)));
            }
            ZONETYPE_ID => {
                if let Some(a) = &t.tz_abbr {
                    pairs.push((Value::str("tz_abbr"), Value::str(a.clone())));
                }
                if let Some(tz) = &t.tz_info {
                    pairs.push((Value::str("tz_id"), Value::str(tz.name.clone())));
                }
            }
            ZONETYPE_ABBR => {
                pairs.push((Value::str("zone"), elem(t.z)));
                pairs.push((Value::str("is_dst"), bool_v(t.dst != 0)));
                pairs.push((
                    Value::str("tz_abbr"),
                    Value::str(t.tz_abbr.clone().unwrap_or_default()),
                ));
            }
            _ => {}
        }
    }
    if t.have_relative {
        let r = &t.relative;
        let mut rel = vec![
            (Value::str("year"), Value::int(r.y)),
            (Value::str("month"), Value::int(r.m)),
            (Value::str("day"), Value::int(r.d)),
            (Value::str("hour"), Value::int(r.h)),
            (Value::str("minute"), Value::int(r.i)),
            (Value::str("second"), Value::int(r.s)),
        ];
        if r.have_weekday_relative {
            rel.push((Value::str("weekday"), Value::int(r.weekday)));
        }
        if r.have_special_relative && r.special.kind == 1 {
            rel.push((Value::str("weekdays"), Value::int(r.special.amount)));
        }
        if r.first_last_day_of != 0 {
            let k = if r.first_last_day_of == 1 {
                "first_day_of_month"
            } else {
                "last_day_of_month"
            };
            rel.push((Value::str(k), Value::bool(true)));
        }
        pairs.push((Value::str("relative"), make_map(rel)));
    }
    make_map(pairs)
}

fn last_errors_array() -> Value {
    LAST_ERRORS.with(|l| match &*l.borrow() {
        None => Value::bool(false),
        Some(e) => {
            let msgs = |list: &[tl::Message]| {
                make_map(
                    list.iter()
                        .map(|m| (Value::int(m.position as i64), Value::str(m.message)))
                        .collect(),
                )
            };
            make_map(vec![
                (
                    Value::str("warning_count"),
                    Value::int(e.warnings.len() as i64),
                ),
                (Value::str("warnings"), msgs(&e.warnings)),
                (Value::str("error_count"), Value::int(e.errors.len() as i64)),
                (Value::str("errors"), msgs(&e.errors)),
            ])
        }
    })
}

/// Make a fresh object of `class` holding `t`.
fn new_date(class: &str, t: &Time) -> Value {
    let obj = with_host(|h| h.new_object_bare(class, Vec::new()));
    store_date(&obj, t);
    obj
}

fn new_zone(zone: &Zone) -> Value {
    let obj = with_host(|h| h.new_object_bare("DateTimeZone", Vec::new()));
    store_zone(&obj, zone);
    obj
}

/// `date_object_compare_date`: two `DateTimeInterface` objects order by the
/// instant they stand for — `<`, `==`, `<=>`, `max()` — whatever their zones.
/// `None` when either operand is not one, so the ordinary object comparison
/// applies.
pub fn compare_dates(h: &host::PhpHost, a: &Value, b: &Value) -> Option<i32> {
    let is_date = |v: &Value| {
        h.object_class(v)
            .is_some_and(|c| h.class_is_a_pub(&c.to_ascii_lowercase(), "datetimeinterface"))
    };
    if !is_date(a) || !is_date(b) {
        return None;
    }
    let (x, y) = (load_date_in(h, a)?, load_date_in(h, b)?);
    Some(match tl::time_compare(&x, &y) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    })
}

/// Dispatch the procedural functions, and the prelude's helpers.
pub fn dispatch(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    if name.starts_with("__phplang_") {
        return prelude_helper(name, args);
    }
    let a = |i: usize| arg(args, i);
    let r: Result<Value, String> = match name {
        // ── procedural ────────────────────────────────────────────────────
        "date_create" | "date_create_immutable" => {
            let class = if name == "date_create" {
                "DateTime"
            } else {
                "DateTimeImmutable"
            };
            let s = if args.is_empty() {
                "now".to_string()
            } else {
                str_arg(args, 0)
            };
            Ok(match initialize(&s, None, zone_arg(&a(1)).as_ref()) {
                Ok(t) => new_date(class, &t),
                Err(_) => Value::bool(false),
            })
        }
        "date_create_from_format" | "date_create_immutable_from_format" => {
            let class = if name == "date_create_from_format" {
                "DateTime"
            } else {
                "DateTimeImmutable"
            };
            let (f, s) = (str_arg(args, 0), str_arg(args, 1));
            Ok(match initialize(&s, Some(&f), zone_arg(&a(2)).as_ref()) {
                Ok(t) => new_date(class, &t),
                Err(_) => Value::bool(false),
            })
        }
        "date_format" => match load_date(&a(0)) {
            Some(t) => Ok(Value::str(tl::date_format(
                str_arg(args, 1).as_bytes(),
                &t,
                t.is_localtime,
            ))),
            None => Ok(Value::bool(false)),
        },
        "date_modify" => {
            let obj = a(0);
            match load_date(&obj) {
                None => Ok(Value::bool(false)),
                Some(mut t) => match modify(&mut t, &str_arg(args, 1)) {
                    Ok(()) => {
                        store_date(&obj, &t);
                        Ok(obj)
                    }
                    Err(msg) => {
                        with_host(|h| h.warn(format!("date_modify(): {msg}")));
                        Ok(Value::bool(false))
                    }
                },
            }
        }
        "date_add" | "date_sub" => {
            let (obj, iv) = (a(0), a(1));
            let sub = name == "date_sub";
            if let (Some(t), Some(rt)) = (load_date(&obj), load_interval(&iv)) {
                if sub && (rt.have_weekday_relative || rt.have_special_relative) {
                    with_host(|h| h.warn(format!("date_sub(): {SUB_SPECIAL}")));
                } else {
                    store_date(&obj, &tl::add_wall(&t, &rt, sub));
                }
            }
            Ok(obj)
        }
        "date_diff" => match (load_date(&a(0)), load_date(&a(1))) {
            (Some(x), Some(y)) => {
                let mut rt = tl::diff(&x, &y);
                if with_host(|h| h.is_truthy(&a(2))) {
                    rt.invert = false;
                }
                Ok(new_interval(&rt))
            }
            _ => Ok(Value::bool(false)),
        },
        "date_timestamp_get" => {
            Ok(load_date(&a(0)).map_or(Value::bool(false), |t| Value::int(t.sse)))
        }
        "date_offset_get" => {
            Ok(load_date(&a(0)).map_or(Value::bool(false), |t| Value::int(offset_of(&t))))
        }
        "date_timestamp_set" | "date_date_set" | "date_isodate_set" | "date_time_set" => {
            let what = match name {
                "date_timestamp_set" => "timestamp",
                "date_date_set" => "date",
                "date_isodate_set" => "isodate",
                _ => "time",
            };
            let mut call = vec![a(0), Value::str(what)];
            call.extend(args.iter().skip(1).cloned());
            if what == "isodate" && call.len() < 5 {
                call.push(Value::int(1));
            }
            while call.len() < 6 {
                call.push(Value::int(0));
            }
            return dispatch("__phplang_date_set", &call);
        }
        "date_timezone_get" => match load_date(&a(0)) {
            Some(t) if t.is_localtime => {
                Ok(Zone::of(&t).map_or(Value::bool(false), |z| new_zone(&z)))
            }
            _ => Ok(Value::bool(false)),
        },
        "date_timezone_set" => return dispatch("__phplang_date_timezone_set", args),
        "date_interval_format" => match load_interval(&a(0)) {
            Some(rt) => Ok(Value::str(date_interval_format(
                str_arg(args, 1).as_bytes(),
                &rt,
            ))),
            None => Ok(Value::bool(false)),
        },
        "date_interval_create_from_date_string" => interval_from_string(&str_arg(args, 0), false),
        "date_parse" => {
            let (t, e) = tl::strtotime(str_arg(args, 0).as_bytes());
            Ok(parsed_time_array(&t, &e))
        }
        "date_parse_from_format" => {
            let (t, e) =
                tl::parse_from_format(str_arg(args, 0).as_bytes(), str_arg(args, 1).as_bytes());
            Ok(parsed_time_array(&t, &e))
        }
        "date_get_last_errors" => Ok(last_errors_array()),
        "timezone_open" => match timezone_initialize(&str_arg(args, 0)) {
            Ok(zone) => Ok(new_zone(&zone)),
            Err(msg) => {
                with_host(|h| h.warn(format!("timezone_open(): {msg}")));
                Ok(Value::bool(false))
            }
        },
        "timezone_name_get" => {
            Ok(load_zone(&a(0)).map_or(Value::bool(false), |z| Value::str(z.name())))
        }
        "timezone_offset_get" => return dispatch("__phplang_tz_offset", args),
        _ => return None,
    };
    Some(r)
}

/// The `__phplang_*` helpers the prelude classes' methods call. They are not
/// library functions: `function_exists` does not see them (it reads the corpus).
fn prelude_helper(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let a = |i: usize| arg(args, i);
    let r: Result<Value, String> = match name {
        "__phplang_date_init" => {
            let obj = a(0);
            let zone = zone_arg(&a(2));
            let format = match a(3) {
                Value::Undef => None,
                v => Some(with_host(|h| h.to_str(&v))),
            };
            match initialize(&str_arg(args, 1), format.as_deref(), zone.as_ref()) {
                Ok(t) => {
                    store_date(&obj, &t);
                    Ok(Value::bool(true))
                }
                Err(msg) if format.is_none() => {
                    host::throw_as_internal_method("DateMalformedStringException", &msg)
                }
                Err(_) => Ok(Value::bool(false)),
            }
        }
        "__phplang_date_format" => match load_date(&a(0)) {
            Some(t) => Ok(Value::str(tl::date_format(
                str_arg(args, 1).as_bytes(),
                &t,
                t.is_localtime,
            ))),
            None => uninitialized(&a(0)),
        },
        "__phplang_date_modify" => {
            let obj = a(0);
            match load_date(&obj) {
                None => uninitialized(&obj),
                Some(mut t) => match modify(&mut t, &str_arg(args, 1)) {
                    Ok(()) => {
                        store_date(&obj, &t);
                        Ok(obj)
                    }
                    Err(msg) => {
                        let m = format!("{}(): {msg}", method_name(&obj, "modify"));
                        host::throw_as_internal_method("DateMalformedStringException", &m)
                    }
                },
            }
        }
        "__phplang_date_add" | "__phplang_date_sub" => {
            let (obj, iv) = (a(0), a(1));
            let sub = name.ends_with("sub");
            match (load_date(&obj), load_interval(&iv)) {
                (None, _) => uninitialized(&obj),
                (_, None) => uninitialized(&iv),
                (Some(t), Some(rt)) => {
                    if sub && (rt.have_weekday_relative || rt.have_special_relative) {
                        let m = format!("{}(): {SUB_SPECIAL}", method_name(&obj, "sub"));
                        host::throw_as_internal_method("DateInvalidOperationException", &m)
                    } else {
                        store_date(&obj, &tl::add_wall(&t, &rt, sub));
                        Ok(obj)
                    }
                }
            }
        }
        "__phplang_date_diff" => match (load_date(&a(0)), load_date(&a(1))) {
            (Some(x), Some(y)) => {
                let mut rt = tl::diff(&x, &y);
                if with_host(|h| h.is_truthy(&a(2))) {
                    rt.invert = false;
                }
                Ok(new_interval(&rt))
            }
            (None, _) => uninitialized(&a(0)),
            (_, None) => uninitialized(&a(1)),
        },
        "__phplang_date_timestamp" => match load_date(&a(0)) {
            Some(t) => Ok(Value::int(t.sse)),
            None => uninitialized(&a(0)),
        },
        "__phplang_date_microsecond" => match load_date(&a(0)) {
            Some(t) => Ok(Value::int(t.us)),
            None => uninitialized(&a(0)),
        },
        "__phplang_date_offset" => match load_date(&a(0)) {
            Some(t) => Ok(Value::int(offset_of(&t))),
            None => uninitialized(&a(0)),
        },
        "__phplang_date_set" => {
            // (obj, what, x, y, z, w)
            let obj = a(0);
            let Some(mut t) = load_date(&obj) else {
                return Some(uninitialized(&obj));
            };
            let n = |i: usize| int_arg(args, i);
            match str_arg(args, 1).as_str() {
                "date" => {
                    t.y = n(2);
                    t.m = n(3);
                    t.d = n(4);
                    tl::update_ts(&mut t, None);
                }
                "isodate" => {
                    t.y = n(2);
                    t.m = 1;
                    t.d = 1;
                    t.relative = RelTime {
                        d: tl::daynr_from_weeknr(n(2), n(3), n(4)),
                        ..RelTime::default()
                    };
                    t.have_relative = true;
                    tl::update_ts(&mut t, None);
                }
                "time" => {
                    t.h = n(2);
                    t.i = n(3);
                    t.s = n(4);
                    t.us = n(5);
                    tl::update_ts(&mut t, None);
                    tl::update_from_sse(&mut t);
                }
                "timestamp" => {
                    tl::unixtime2local(&mut t, n(2));
                    tl::update_ts(&mut t, None);
                    t.us = 0;
                }
                "microsecond" => {
                    t.us = n(2);
                }
                _ => {}
            }
            store_date(&obj, &t);
            Ok(obj)
        }
        "__phplang_date_timezone_get" => match load_date(&a(0)) {
            Some(t) if t.is_localtime => {
                Ok(Zone::of(&t).map_or(Value::bool(false), |z| new_zone(&z)))
            }
            Some(_) => Ok(Value::bool(false)),
            None => uninitialized(&a(0)),
        },
        "__phplang_date_timezone_set" => {
            let obj = a(0);
            match (load_date(&obj), zone_arg(&a(1))) {
                (Some(mut t), Some(zone)) => {
                    let sse = t.sse;
                    apply_zone(&mut t, &zone);
                    tl::unixtime2local(&mut t, sse);
                    store_date(&obj, &t);
                    Ok(obj)
                }
                (None, _) => uninitialized(&obj),
                (_, None) => uninitialized(&a(1)),
            }
        }
        "__phplang_date_copy" => match load_date(&a(1)) {
            Some(t) => {
                store_date(&a(0), &t);
                Ok(a(0))
            }
            None => uninitialized(&a(1)),
        },
        "__phplang_date_from_timestamp" => {
            // (obj, int|float) — `php_date_initialize_from_ts_*`.
            let obj = a(0);
            let (sec, usec) = match a(1) {
                Value::Float(f) => {
                    let sec = f.trunc() as i64;
                    let mut usec = ((f % 1.0) * 1_000_000.0).round() as i64;
                    let mut sec = sec;
                    if usec.abs() == 1_000_000 {
                        sec += usec.signum();
                        usec = 0;
                    }
                    if usec < 0 {
                        sec -= 1;
                        usec += 1_000_000;
                    }
                    (sec, usec)
                }
                v => (with_host(|h| h.to_number(&v).to_int()), 0),
            };
            let mut t = Time {
                zone_type: ZONETYPE_OFFSET,
                ..Time::default()
            };
            tl::unixtime2gmt(&mut t, sec);
            tl::update_ts(&mut t, None);
            t.us = usec;
            t.is_localtime = true;
            t.have_zone = 1;
            store_date(&obj, &t);
            Ok(obj)
        }
        "__phplang_date_last_errors" => Ok(last_errors_array()),
        "__phplang_period_init" => {
            let rest: Vec<Value> = with_host(|h| h.array_pairs(&a(1)))
                .unwrap_or_default()
                .into_iter()
                .map(|(_, v)| v)
                .collect();
            period_init(&a(0), &rest)
        }
        "__phplang_period_list" => period_list(&a(0)),
        "__phplang_date_new" => {
            let class = str_arg(args, 0);
            Ok(with_host(|h| h.new_object_bare(&class, Vec::new())))
        }
        "__phplang_tz_init" => match timezone_initialize(&str_arg(args, 1)) {
            Ok(zone) => {
                store_zone(&a(0), &zone);
                Ok(Value::bool(true))
            }
            Err(msg) => host::throw_as_internal_method(
                "DateInvalidTimeZoneException",
                &format!("DateTimeZone::__construct(): {msg}"),
            ),
        },
        "__phplang_tz_name" => match load_zone(&a(0)) {
            Some(z) => Ok(Value::str(z.name())),
            None => uninitialized(&a(0)),
        },
        "__phplang_tz_offset" => match (load_zone(&a(0)), load_date(&a(1))) {
            (Some(z), Some(_)) => Ok(Value::int(match z {
                Zone::Id(tz) => tz.offset,
                Zone::Offset(off) => off,
                Zone::Abbr { z, dst, .. } => z + dst * 3600,
            })),
            (None, _) => uninitialized(&a(0)),
            (_, None) => uninitialized(&a(1)),
        },
        "__phplang_interval_init" => match interval_initialize(&str_arg(args, 1)) {
            Ok(rt) => {
                store_interval(&a(0), &rt);
                Ok(Value::bool(true))
            }
            Err(msg) => {
                host::throw_as_internal_method("DateMalformedIntervalStringException", &msg)
            }
        },
        "__phplang_interval_format" => match load_interval(&a(0)) {
            Some(rt) => Ok(Value::str(date_interval_format(
                str_arg(args, 1).as_bytes(),
                &rt,
            ))),
            None => uninitialized(&a(0)),
        },
        "__phplang_interval_from_string" => interval_from_string(&str_arg(args, 0), true),
        "__phplang_interval_get" => {
            // `date_interval_read_property` for an interval made from a string,
            // whose fields are not properties.
            let obj = a(0);
            let field = str_arg(args, 1);
            let rt = load_interval(&obj);
            let v = rt.and_then(|rt| match field.as_str() {
                "y" => Some(Value::int(rt.y)),
                "m" => Some(Value::int(rt.m)),
                "d" => Some(Value::int(rt.d)),
                "h" => Some(Value::int(rt.h)),
                "i" => Some(Value::int(rt.i)),
                "s" => Some(Value::int(rt.s)),
                "f" => Some(Value::float(rt.us as f64 / 1_000_000.0)),
                "invert" => Some(Value::int(rt.invert as i64)),
                "days" => Some(if rt.days == UNSET {
                    Value::bool(false)
                } else {
                    Value::int(rt.days)
                }),
                _ => None,
            });
            match v {
                Some(v) => Ok(v),
                None => {
                    host::warn_as_internal_method(&format!(
                        "Undefined property: DateInterval::${field}"
                    ));
                    Ok(Value::Undef)
                }
            }
        }

        _ => return None,
    };
    Some(r)
}

/// `date_offset_get`.
fn offset_of(t: &Time) -> i64 {
    if !t.is_localtime {
        return 0;
    }
    match t.zone_type {
        ZONETYPE_ID => t.tz_info.as_ref().map_or(0, |z| z.offset),
        ZONETYPE_OFFSET => t.z,
        ZONETYPE_ABBR => t.z + 3600 * t.dst,
        _ => 0,
    }
}

/// `DateInterval::createFromDateString` (`throwing`) and
/// `date_interval_create_from_date_string`.
fn interval_from_string(s: &str, throwing: bool) -> Result<Value, String> {
    let (t, e) = tl::strtotime(s.as_bytes());
    let fail = |msg: String| {
        if throwing {
            host::throw_as_internal_method("DateMalformedIntervalStringException", &msg)
        } else {
            with_host(|h| h.warn(format!("date_interval_create_from_date_string(): {msg}")));
            Ok(Value::bool(false))
        }
    };
    if let Some(m) = e.errors.first() {
        let c = if m.character == 0 {
            ' '
        } else {
            m.character as char
        };
        return fail(format!(
            "Unknown or bad format ({s}) at position {} ({c}): {}",
            m.position, m.message
        ));
    }
    if t.have_date || t.have_time != 0 || t.have_zone != 0 {
        return fail(format!("String '{s}' contains non-relative elements"));
    }
    let obj = with_host(|h| h.new_object_bare("DateInterval", Vec::new()));
    set_prop(&obj, "from_string", Value::bool(true));
    set_prop(&obj, "date_string", Value::str(s.to_string()));
    Ok(obj)
}

/// `DatePeriod::__construct` (`date_period_init_finish` included): `args` are
/// the constructor's own arguments.
fn period_init(obj: &Value, args: &[Value]) -> Result<Value, String> {
    let a = |i: usize| arg(args, i);
    let int = |v: &Value| matches!(v, Value::Int(_));
    let date = |v: &Value| is_a(v, "datetimeinterface");
    let interval = |v: &Value| is_a(v, "dateinterval");
    let (start, iv, end, recurrences, options) =
        if date(&a(0)) && interval(&a(1)) && int(&a(2)) && args.len() <= 4 {
            (
                a(0),
                a(1),
                None,
                int_arg(args, 2),
                if args.len() > 3 { int_arg(args, 3) } else { 0 },
            )
        } else if date(&a(0)) && interval(&a(1)) && date(&a(2)) && args.len() <= 4 {
            (
                a(0),
                a(1),
                Some(a(2)),
                0,
                if args.len() > 3 { int_arg(args, 3) } else { 0 },
            )
        } else {
            return host::throw_as_internal_method(
            "TypeError",
            "DatePeriod::__construct() accepts (DateTimeInterface, DateInterval, int [, int]), or \
             (DateTimeInterface, DateInterval, DateTime [, int]), or (string [, int]) as arguments",
        );
        };
    let (Some(st), Some(rt)) = (load_date(&start), load_interval(&iv)) else {
        return uninitialized(&start);
    };
    const MAX: i64 = i32::MAX as i64 - 8;
    if end.is_none() && !(1..=MAX).contains(&recurrences) {
        return host::throw_as_internal_method(
            "DateMalformedPeriodStringException",
            &format!(
                "DatePeriod::__construct(): Recurrence count must be greater or equal to 1 and lower than {}",
                MAX + 1
            ),
        );
    }
    let include_start = options & 1 == 0;
    let include_end = options & 2 != 0;
    let start_class = class_of(&start);
    set_prop(obj, "start", new_date(&start_class, &st));
    set_prop(obj, "current", Value::Undef);
    let end_v = match &end {
        Some(e) => match load_date(e) {
            Some(et) => new_date(&start_class, &et),
            None => return uninitialized(e),
        },
        None => Value::Undef,
    };
    set_prop(obj, "end", end_v);
    set_prop(obj, "interval", new_interval(&rt));
    set_prop(
        obj,
        "recurrences",
        Value::int(recurrences + include_start as i64 + include_end as i64),
    );
    set_prop(obj, "include_start_date", Value::bool(include_start));
    set_prop(obj, "include_end_date", Value::bool(include_end));
    Ok(Value::Undef)
}

/// Every date a `DatePeriod` yields, in order — its iterator's
/// rewind / has_more / current / move_forward run to the end.
fn period_list(obj: &Value) -> Result<Value, String> {
    let start = prop(obj, "start");
    let (Some(st), Some(rt)) = (load_date(&start), load_interval(&prop(obj, "interval"))) else {
        return uninitialized(obj);
    };
    let end = load_date(&prop(obj, "end"));
    let truthy = |n: &str| with_host(|h| h.is_truthy(&h.prop_get(obj, n)));
    let (include_start, include_end) = (truthy("include_start_date"), truthy("include_end_date"));
    let recurrences = with_host(|h| h.to_number(&h.prop_get(obj, "recurrences")).to_int());
    // `get_base_date_class`: a subclass of DateTime yields plain DateTimes.
    let class = if is_a(&start, "datetimeimmutable") {
        "DateTimeImmutable"
    } else {
        "DateTime"
    };
    let advance = |t: &mut Time| {
        t.have_relative = true;
        t.relative = rt.clone();
        tl::update_ts(t, None);
        tl::update_from_sse(t);
    };
    let mut cur = st;
    if !include_start {
        advance(&mut cur);
    }
    let mut out = Vec::new();
    let mut index = 0;
    loop {
        let more = match &end {
            Some(e) if cur.sse == e.sse => {
                if include_end {
                    cur.us <= e.us
                } else {
                    cur.us < e.us
                }
            }
            Some(e) => cur.sse < e.sse,
            None => index < recurrences,
        };
        if !more {
            break;
        }
        out.push(new_date(class, &cur));
        advance(&mut cur);
        index += 1;
    }
    set_prop(obj, "current", new_date(&class_of(&start), &cur));
    Ok(make_list(out))
}
