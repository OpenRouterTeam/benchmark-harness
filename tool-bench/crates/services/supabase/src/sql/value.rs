//! SQL value semantics over JSON values: NULL handling, comparison, arithmetic with exact
//! decimal scale, casts, and timestamp/interval arithmetic (UTC only).

use std::cmp::Ordering;

use serde_json::{Number, Value};
use time::{
    Date, Duration, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset,
    format_description::well_known::Rfc3339,
};

use super::SqlError;

pub fn is_null(v: &Value) -> bool {
    v.is_null()
}

/// Text form of a value, as `::text` would print it.
pub fn to_text(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Array(items) => Some(format!(
            "{{{}}}",
            items
                .iter()
                .map(|i| to_text(i).unwrap_or_else(|| "NULL".to_owned()))
                .collect::<Vec<_>>()
                .join(",")
        )),
        Value::Object(_) => Some(v.to_string()),
    }
}

pub fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

/// Decimal places of a JSON number (0 for integers).
pub fn scale_of(v: &Value) -> usize {
    match v {
        Value::Number(n) if !n.is_i64() && !n.is_u64() => {
            let s = n.to_string();
            if s.contains('e') || s.contains('E') {
                return 6;
            }
            s.split('.').nth(1).map_or(0, str::len)
        }
        Value::String(s) => s.split('.').nth(1).map_or(0, |d| d.trim().len()),
        _ => 0,
    }
}

/// A number with `scale` decimals; integral results become JSON integers when `scale == 0`.
pub fn number(x: f64, scale: usize) -> Value {
    if !x.is_finite() {
        return Value::Null;
    }
    if scale == 0 && x.fract() == 0.0 && x.abs() < 9.0e15 {
        return Value::from(x as i64);
    }
    let factor = 10f64.powi(scale.min(15) as i32);
    let rounded = (x * factor).round() / factor;
    Number::from_f64(rounded).map_or(Value::Null, Value::Number)
}

fn looks_like_timestamp(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 10
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[7] == b'-'
        && b[8..10].iter().all(u8::is_ascii_digit)
}

/// Comparable form of a date/timestamp string: `YYYY-MM-DDTHH:MM:SS[.ffffff]` in UTC.
fn ts_key(s: &str) -> String {
    match parse_ts(s) {
        Some(dt) => {
            let dt = dt.to_offset(UtcOffset::UTC);
            format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}",
                dt.year(),
                u8::from(dt.month()),
                dt.day(),
                dt.hour(),
                dt.minute(),
                dt.second(),
                dt.microsecond()
            )
        }
        None => s.to_owned(),
    }
}

pub fn compare(a: &Value, b: &Value) -> Option<Ordering> {
    match (a, b) {
        (Value::Null, _) | (_, Value::Null) => None,
        (Value::Number(x), Value::Number(y)) => x.as_f64()?.partial_cmp(&y.as_f64()?),
        (Value::Number(_), Value::String(_)) | (Value::String(_), Value::Number(_)) => {
            as_f64(a)?.partial_cmp(&as_f64(b)?)
        }
        (Value::String(x), Value::String(y)) => {
            if looks_like_timestamp(x) && looks_like_timestamp(y) {
                Some(ts_key(x).cmp(&ts_key(y)))
            } else {
                Some(x.cmp(y))
            }
        }
        (Value::Bool(x), Value::Bool(y)) => Some(x.cmp(y)),
        (Value::Bool(x), Value::String(s)) => parse_bool(s).map(|y| x.cmp(&y)),
        (Value::String(s), Value::Bool(y)) => parse_bool(s).map(|x| x.cmp(y)),
        _ => Some(a.to_string().cmp(&b.to_string())),
    }
}

pub fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_lowercase().as_str() {
        "t" | "true" | "yes" | "y" | "on" | "1" => Some(true),
        "f" | "false" | "no" | "n" | "off" | "0" => Some(false),
        _ => None,
    }
}

/// Total order for ORDER BY: NULLs compare greater than everything (Postgres default).
pub fn sort_cmp(a: &Value, b: &Value) -> Ordering {
    match (a.is_null(), b.is_null()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        _ => compare(a, b).unwrap_or(Ordering::Equal),
    }
}

pub fn equal(a: &Value, b: &Value) -> Option<bool> {
    compare(a, b).map(|o| o == Ordering::Equal)
}

/// Grouping/DISTINCT key: NULLs group together, numbers by value.
pub fn group_key(v: &Value) -> String {
    match v {
        Value::Number(n) => n
            .as_f64()
            .map_or_else(|| n.to_string(), |f| format!("n{f}")),
        Value::String(s) if looks_like_timestamp(s) => format!("s{}", ts_key(s)),
        other => other.to_string(),
    }
}

// ───────────────────────── timestamps ─────────────────────────

pub fn parse_ts(s: &str) -> Option<OffsetDateTime> {
    let s = s.trim();
    if !looks_like_timestamp(s) {
        return None;
    }
    let date = Date::from_calendar_date(
        s[..4].parse().ok()?,
        Month::try_from(s[5..7].parse::<u8>().ok()?).ok()?,
        s[8..10].parse().ok()?,
    )
    .ok()?;
    let rest = &s[10..];
    if rest.is_empty() {
        return Some(PrimitiveDateTime::new(date, Time::MIDNIGHT).assume_utc());
    }
    let rest = rest.trim_start_matches(['T', 't', ' ']);
    // time part up to offset
    let (time_part, offset_part) = match rest.find(['+', '-', 'Z', 'z']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let mut hms = time_part.split(':');
    let h: u8 = hms.next()?.trim().parse().ok()?;
    let m: u8 = hms.next().unwrap_or("0").trim().parse().ok()?;
    let sec_text = hms.next().unwrap_or("0").trim();
    let (sec, micros) = match sec_text.split_once('.') {
        Some((s, f)) => {
            let mut f = f.to_owned();
            f.truncate(6);
            while f.len() < 6 {
                f.push('0');
            }
            (s.parse::<u8>().ok()?, f.parse::<u32>().ok()?)
        }
        None => (sec_text.parse::<u8>().ok()?, 0),
    };
    let time = Time::from_hms_micro(h, m, sec, micros).ok()?;
    let offset = if offset_part.is_empty() || offset_part.eq_ignore_ascii_case("z") {
        UtcOffset::UTC
    } else {
        let sign = if offset_part.starts_with('-') { -1 } else { 1 };
        let digits: String = offset_part[1..]
            .chars()
            .filter(char::is_ascii_digit)
            .collect();
        let oh: i8 = digits.get(..2).unwrap_or("0").parse().ok()?;
        let om: i8 = digits.get(2..4).unwrap_or("0").parse().unwrap_or(0);
        UtcOffset::from_hms(sign * oh, sign * om, 0).ok()?
    };
    Some(PrimitiveDateTime::new(date, time).assume_offset(offset))
}

/// Postgres JSON output for timestamptz: `2026-03-14T09:00:00+00:00` (fraction when present).
pub fn format_tstz(dt: OffsetDateTime) -> String {
    let dt = dt.to_offset(UtcOffset::UTC);
    let frac = if dt.microsecond() == 0 {
        String::new()
    } else {
        let f = format!(".{:06}", dt.microsecond());
        f.trim_end_matches('0').to_owned()
    };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{frac}+00:00",
        dt.year(),
        u8::from(dt.month()),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second()
    )
}

pub fn format_ts(dt: OffsetDateTime) -> String {
    let s = format_tstz(dt);
    s.trim_end_matches("+00:00").to_owned()
}

pub fn format_date(dt: OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        dt.year(),
        u8::from(dt.month()),
        dt.day()
    )
}

/// RFC 3339 clock time to the Postgres timestamptz text form.
pub fn rfc3339_to_tstz(s: &str) -> String {
    OffsetDateTime::parse(s, &Rfc3339).map_or_else(|_| s.to_owned(), format_tstz)
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Interval {
    pub months: i64,
    pub days: i64,
    pub micros: i64,
}

pub fn parse_interval(s: &str) -> Result<Interval, SqlError> {
    let bad = || {
        SqlError::new(
            "22007",
            format!("invalid input syntax for type interval: \"{s}\""),
        )
    };
    let mut iv = Interval::default();
    let tokens: Vec<&str> = s.split_whitespace().collect();
    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i];
        if tok.contains(':') {
            let parts: Vec<i64> = tok
                .split(':')
                .map(|p| p.parse::<i64>().map_err(|_| bad()))
                .collect::<Result<_, _>>()?;
            let (h, m, sec) = (
                parts.first().copied().unwrap_or(0),
                parts.get(1).copied().unwrap_or(0),
                parts.get(2).copied().unwrap_or(0),
            );
            iv.micros += ((h * 60 + m) * 60 + sec) * 1_000_000;
            i += 1;
            continue;
        }
        let n: f64 = tok.parse().map_err(|_| bad())?;
        let unit = tokens.get(i + 1).ok_or_else(bad)?.to_lowercase();
        let unit = unit.trim_end_matches('s');
        match unit {
            "year" | "yr" | "y" => iv.months += (n * 12.0) as i64,
            "month" | "mon" => iv.months += n as i64,
            "week" | "w" => iv.days += (n * 7.0) as i64,
            "day" | "d" => iv.days += n as i64,
            "hour" | "hr" | "h" => iv.micros += (n * 3_600_000_000.0) as i64,
            "minute" | "min" | "m" => iv.micros += (n * 60_000_000.0) as i64,
            "second" | "sec" => iv.micros += (n * 1_000_000.0) as i64,
            _ => return Err(bad()),
        }
        i += 2;
    }
    Ok(iv)
}

pub fn add_interval(dt: OffsetDateTime, iv: Interval, sign: i64) -> OffsetDateTime {
    let mut out = dt;
    if iv.months != 0 {
        let total =
            i64::from(out.year()) * 12 + i64::from(u8::from(out.month())) - 1 + sign * iv.months;
        let year = total.div_euclid(12) as i32;
        let month = Month::try_from((total.rem_euclid(12) + 1) as u8).unwrap_or(Month::January);
        let mut day = out.day();
        loop {
            if let Ok(d) = Date::from_calendar_date(year, month, day) {
                out = out.replace_date(d);
                break;
            }
            day -= 1;
        }
    }
    out + Duration::days(sign * iv.days) + Duration::microseconds(sign * iv.micros)
}

pub fn format_interval(iv: Interval) -> String {
    let mut parts = Vec::new();
    if iv.months != 0 {
        let (y, m) = (iv.months / 12, iv.months % 12);
        if y != 0 {
            parts.push(format!("{y} year{}", if y.abs() == 1 { "" } else { "s" }));
        }
        if m != 0 {
            parts.push(format!("{m} mon{}", if m.abs() == 1 { "" } else { "s" }));
        }
    }
    if iv.days != 0 {
        parts.push(format!(
            "{} day{}",
            iv.days,
            if iv.days.abs() == 1 { "" } else { "s" }
        ));
    }
    if iv.micros != 0 || parts.is_empty() {
        let total = iv.micros / 1_000_000;
        let sign = if total < 0 { "-" } else { "" };
        let t = total.abs();
        parts.push(format!(
            "{sign}{:02}:{:02}:{:02}",
            t / 3600,
            (t / 60) % 60,
            t % 60
        ));
    }
    parts.join(" ")
}

pub fn interval_between(a: OffsetDateTime, b: OffsetDateTime) -> Interval {
    let micros = (a - b).whole_microseconds() as i64;
    let days = micros / 86_400_000_000;
    Interval {
        months: 0,
        days,
        micros: micros - days * 86_400_000_000,
    }
}

pub fn date_trunc(field: &str, dt: OffsetDateTime) -> Option<OffsetDateTime> {
    let dt = dt.to_offset(UtcOffset::UTC);
    let date = dt.date();
    let (date, time) = match field.to_lowercase().as_str() {
        "year" => (
            Date::from_calendar_date(date.year(), Month::January, 1).ok()?,
            Time::MIDNIGHT,
        ),
        "quarter" => {
            let m = (u8::from(date.month()) - 1) / 3 * 3 + 1;
            (
                Date::from_calendar_date(date.year(), Month::try_from(m).ok()?, 1).ok()?,
                Time::MIDNIGHT,
            )
        }
        "month" => (
            Date::from_calendar_date(date.year(), date.month(), 1).ok()?,
            Time::MIDNIGHT,
        ),
        "week" => (
            date - Duration::days(i64::from(date.weekday().number_days_from_monday())),
            Time::MIDNIGHT,
        ),
        "day" => (date, Time::MIDNIGHT),
        "hour" => (date, Time::from_hms(dt.hour(), 0, 0).ok()?),
        "minute" => (date, Time::from_hms(dt.hour(), dt.minute(), 0).ok()?),
        "second" => (
            date,
            Time::from_hms(dt.hour(), dt.minute(), dt.second()).ok()?,
        ),
        _ => return None,
    };
    Some(PrimitiveDateTime::new(date, time).assume_utc())
}

pub fn date_part(field: &str, dt: OffsetDateTime) -> Option<Value> {
    let dt = dt.to_offset(UtcOffset::UTC);
    Some(match field.to_lowercase().as_str() {
        "year" => Value::from(dt.year()),
        "quarter" => Value::from((u8::from(dt.month()) - 1) / 3 + 1),
        "month" => Value::from(u8::from(dt.month())),
        "day" => Value::from(dt.day()),
        "hour" => Value::from(dt.hour()),
        "minute" => Value::from(dt.minute()),
        "second" => Value::from(dt.second()),
        "dow" => Value::from(dt.weekday().number_days_from_sunday()),
        "isodow" => Value::from(dt.weekday().number_from_monday()),
        "doy" => Value::from(dt.ordinal()),
        "week" => Value::from(dt.iso_week()),
        "epoch" => Value::from(dt.unix_timestamp()),
        _ => return None,
    })
}

/// `to_char` for the common date patterns.
pub fn to_char(dt: OffsetDateTime, pattern: &str) -> String {
    let dt = dt.to_offset(UtcOffset::UTC);
    const MONTHS: [&str; 12] = [
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
    let month_name = MONTHS[usize::from(u8::from(dt.month())) - 1];
    let mut out = String::new();
    let mut rest = pattern;
    while !rest.is_empty() {
        let pairs: [(&str, String); 12] = [
            ("YYYY", format!("{:04}", dt.year())),
            ("Month", format!("{month_name:<9}")),
            ("Mon", month_name[..3].to_owned()),
            ("MM", format!("{:02}", u8::from(dt.month()))),
            ("DD", format!("{:02}", dt.day())),
            ("HH24", format!("{:02}", dt.hour())),
            ("HH", format!("{:02}", (dt.hour() + 11) % 12 + 1)),
            ("MI", format!("{:02}", dt.minute())),
            ("SS", format!("{:02}", dt.second())),
            ("YY", format!("{:02}", dt.year() % 100)),
            ("Q", format!("{}", (u8::from(dt.month()) - 1) / 3 + 1)),
            ("D", format!("{}", dt.weekday().number_from_sunday())),
        ];
        if let Some((pat, val)) = pairs.iter().find(|(p, _)| rest.starts_with(p)) {
            out.push_str(val);
            rest = &rest[pat.len()..];
        } else {
            let c = rest.chars().next().unwrap_or_default();
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

// ───────────────────────── casts ─────────────────────────

/// Canonical pg format name for a declared type (`bigint` -> `int8`).
pub fn type_format(ty: &str) -> String {
    let t = ty.trim().to_lowercase();
    let (base, array) = match t.strip_suffix("[]") {
        Some(b) => (b.to_owned(), true),
        None => (t.clone(), false),
    };
    let base = base.split('(').next().unwrap_or("").trim().to_owned();
    let f = match base.as_str() {
        "bigint" | "int8" => "int8",
        "integer" | "int" | "int4" => "int4",
        "smallint" | "int2" => "int2",
        "text" => "text",
        "varchar" | "character varying" => "varchar",
        "char" | "character" | "bpchar" => "bpchar",
        "boolean" | "bool" => "bool",
        "numeric" | "decimal" => "numeric",
        "real" | "float4" => "float4",
        "double precision" | "float8" | "float" => "float8",
        "timestamptz" | "timestamp with time zone" => "timestamptz",
        "timestamp" | "timestamp without time zone" => "timestamp",
        "date" => "date",
        "time" | "time without time zone" => "time",
        "timetz" | "time with time zone" => "timetz",
        "interval" => "interval",
        "uuid" => "uuid",
        "jsonb" => "jsonb",
        "json" => "json",
        "bytea" => "bytea",
        "inet" => "inet",
        "citext" => "citext",
        other => {
            return if array {
                format!("_{other}")
            } else {
                other.to_owned()
            };
        }
    };
    if array { format!("_{f}") } else { f.to_owned() }
}

/// `information_schema` data_type for a pg format.
pub fn data_type_name(format: &str) -> &'static str {
    match format {
        "int8" => "bigint",
        "int4" => "integer",
        "int2" => "smallint",
        "text" => "text",
        "varchar" => "character varying",
        "bpchar" => "character",
        "bool" => "boolean",
        "numeric" => "numeric",
        "float4" => "real",
        "float8" => "double precision",
        "timestamptz" => "timestamp with time zone",
        "timestamp" => "timestamp without time zone",
        "date" => "date",
        "time" => "time without time zone",
        "timetz" => "time with time zone",
        "interval" => "interval",
        "uuid" => "uuid",
        "jsonb" => "jsonb",
        "json" => "json",
        "bytea" => "bytea",
        "inet" => "inet",
        f if f.starts_with('_') => "ARRAY",
        _ => "USER-DEFINED",
    }
}

fn invalid(ty: &str, v: &Value) -> SqlError {
    SqlError::new(
        "22P02",
        format!(
            "invalid input syntax for type {ty}: \"{}\"",
            to_text(v).unwrap_or_default()
        ),
    )
}

/// Casts `v` to the pg type `format` (see [`type_format`]).
pub fn cast(v: &Value, format: &str) -> Result<Value, SqlError> {
    if v.is_null() {
        return Ok(Value::Null);
    }
    Ok(match format {
        "int8" | "int4" | "int2" => match v {
            Value::Number(n) if n.is_i64() || n.is_u64() => v.clone(),
            Value::Number(n) => Value::from(n.as_f64().unwrap_or(0.0).round() as i64),
            Value::String(s) => Value::from(
                s.trim()
                    .parse::<i64>()
                    .map_err(|_| invalid(data_type_name(format), v))?,
            ),
            Value::Bool(b) => Value::from(i64::from(*b)),
            _ => return Err(invalid(data_type_name(format), v)),
        },
        "numeric" | "float8" | "float4" => match v {
            Value::Number(_) => v.clone(),
            Value::String(s) => {
                let t = s.trim();
                let x: f64 = t.parse().map_err(|_| invalid(data_type_name(format), v))?;
                if format == "numeric" {
                    number(x, scale_of(v))
                } else {
                    number(x, 15)
                }
            }
            _ => return Err(invalid(data_type_name(format), v)),
        },
        "bool" => match v {
            Value::Bool(_) => v.clone(),
            Value::String(s) => Value::Bool(parse_bool(s).ok_or_else(|| invalid("boolean", v))?),
            Value::Number(n) => Value::Bool(n.as_f64() != Some(0.0)),
            _ => return Err(invalid("boolean", v)),
        },
        "timestamptz" => {
            let s = to_text(v).unwrap_or_default();
            if s.eq_ignore_ascii_case("now") {
                return Ok(v.clone());
            }
            Value::String(format_tstz(
                parse_ts(&s).ok_or_else(|| invalid("timestamp with time zone", v))?,
            ))
        }
        "timestamp" => {
            let s = to_text(v).unwrap_or_default();
            Value::String(format_ts(
                parse_ts(&s).ok_or_else(|| invalid("timestamp without time zone", v))?,
            ))
        }
        "date" => {
            let s = to_text(v).unwrap_or_default();
            Value::String(format_date(parse_ts(&s).ok_or_else(|| invalid("date", v))?))
        }
        "json" | "jsonb" => match v {
            Value::String(s) => serde_json::from_str(s).map_err(|_| {
                SqlError::new("22P02", "invalid input syntax for type json".to_owned())
            })?,
            other => other.clone(),
        },
        "text" | "varchar" | "bpchar" | "citext" | "uuid" | "name" | "regclass" | "inet" => {
            Value::String(to_text(v).unwrap_or_default())
        }
        "interval" => Value::String(format_interval(parse_interval(
            &to_text(v).unwrap_or_default(),
        )?)),
        f if f.starts_with('_') => match v {
            Value::Array(_) => v.clone(),
            Value::String(s) => {
                let inner = s.trim().trim_start_matches('{').trim_end_matches('}');
                let elem = &f[1..];
                Value::Array(
                    inner
                        .split(',')
                        .filter(|p| !p.is_empty())
                        .map(|p| cast(&Value::String(p.trim().trim_matches('"').to_owned()), elem))
                        .collect::<Result<_, _>>()?,
                )
            }
            other => Value::Array(vec![other.clone()]),
        },
        _ => v.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_compare_across_formats() {
        let a = Value::from("2026-03-05T10:00:00+00:00");
        assert_eq!(
            compare(&a, &Value::from("2026-03-05")),
            Some(Ordering::Greater)
        );
        assert_eq!(
            compare(&a, &Value::from("2026-03-05 10:00:00Z")),
            Some(Ordering::Equal)
        );
        assert_eq!(
            compare(&a, &Value::from("2026-03-06")),
            Some(Ordering::Less)
        );
    }

    #[test]
    fn intervals_and_truncation() {
        let dt = parse_ts("2026-03-31T12:30:00Z").unwrap();
        let iv = parse_interval("1 month").unwrap();
        assert_eq!(
            format_tstz(add_interval(dt, iv, -1)),
            "2026-02-28T12:30:00+00:00"
        );
        assert_eq!(
            format_tstz(date_trunc("month", dt).unwrap()),
            "2026-03-01T00:00:00+00:00"
        );
        assert_eq!(to_char(dt, "YYYY-MM"), "2026-03");
    }

    #[test]
    fn numbers_keep_scale() {
        assert_eq!(number(129.99 + 129.99, 2), serde_json::json!(259.98));
        assert_eq!(number(3.0, 0), serde_json::json!(3));
        assert_eq!(
            cast(&Value::from("12"), "int8").unwrap(),
            serde_json::json!(12)
        );
    }
}
