//! Shared helpers: the tool context, arguments, IDs, times, Python-style reprs and the
//! server's error texts.

use mock_core::world::ServiceState;
use serde_json::Value;

use crate::tz;

pub const USER: &str = "ops@octo-corp.example";

pub struct Ctx<'a> {
    pub st: &'a mut ServiceState,
    pub now: i64,
}

/// Ok: the tool's text result. Err: the error text (already prefixed for the tool).
pub type R = Result<String, String>;

pub fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

pub fn arg<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k)
        .and_then(Value::as_str)
        .filter(|x| !x.trim().is_empty())
}

pub fn arg_bool(a: &Value, k: &str) -> Option<bool> {
    match a.get(k)? {
        Value::Bool(b) => Some(*b),
        Value::String(x) => match x.to_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

pub fn arg_i64(a: &Value, k: &str) -> Option<i64> {
    match a.get(k)? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(x) => x.trim().parse().ok(),
        _ => None,
    }
}

/// A list argument; accepts arrays, JSON-encoded arrays, or comma-separated strings.
pub fn arg_list(a: &Value, k: &str) -> Option<Vec<String>> {
    match a.get(k)? {
        Value::Array(items) => Some(
            items
                .iter()
                .map(|x| {
                    x.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| x.to_string())
                })
                .collect(),
        ),
        Value::String(x) if x.trim().starts_with('[') => {
            serde_json::from_str::<Vec<Value>>(x).ok().map(|v| {
                v.iter()
                    .map(|x| {
                        x.as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| x.to_string())
                    })
                    .collect()
            })
        }
        Value::String(x) if !x.trim().is_empty() => Some(
            x.split(',')
                .map(|p| p.trim().to_owned())
                .filter(|p| !p.is_empty())
                .collect(),
        ),
        _ => None,
    }
}

/// A JSON argument that may arrive as an encoded string.
pub fn arg_json(a: &Value, k: &str) -> Option<Value> {
    match a.get(k)? {
        Value::String(x) => serde_json::from_str(x)
            .ok()
            .or_else(|| Some(Value::String(x.clone()))),
        Value::Null => None,
        other => Some(other.clone()),
    }
}

impl Ctx<'_> {
    pub fn all(&self, coll: &str) -> Vec<Value> {
        self.st
            .store
            .collection(coll)
            .map(|c| c.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn get(&self, coll: &str, id: &str) -> Option<Value> {
        self.st.store.collection(coll)?.get(id).cloned()
    }

    pub fn put(&mut self, coll: &str, doc: Value) {
        let id = s(&doc, "id").to_owned();
        self.st.store.collection_mut(coll).insert(id, doc);
    }

    pub fn remove(&mut self, coll: &str, id: &str) -> Option<Value> {
        self.st.store.collection_mut(coll).remove(id)
    }

    /// Lowercase hex ID of `len` characters, deterministic per kind.
    pub fn hex_id(&mut self, kind: &str, len: usize) -> String {
        let mut out = String::new();
        while out.len() < len {
            let u = self.st.ids.next_uuid(kind);
            out.push_str(&u.simple().to_string());
        }
        out.truncate(len);
        out
    }

    /// Drive-style ID (`1` + 32 base62-ish characters).
    pub fn drive_id(&mut self) -> String {
        let raw = self.st.ids.next_prefixed("drive_file", "1", 43);
        raw.replace(['+', '/'], "_")
    }

    pub fn rfc3339(&self) -> String {
        tz::rfc3339(self.now)
    }

    /// RFC 3339 with milliseconds, as Drive/Tasks report times.
    pub fn rfc3339_ms(&self) -> String {
        format!("{}.000Z", tz::rfc3339(self.now).trim_end_matches('Z'))
    }
}

/// Python `repr()` of a string.
pub fn py_str(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python `repr()` of a JSON value (dicts, lists, strings, numbers, booleans, None).
pub fn py_repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(b) => if *b { "True" } else { "False" }.into(),
        Value::Number(n) => n.to_string(),
        Value::String(x) => py_str(x),
        Value::Array(a) => format!("[{}]", a.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, v)| format!("{}: {}", py_str(k), py_repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `<HttpError 404 when requesting <url> returned "<msg>". Details: "[...]">`
pub fn http_error(status: u16, url: &str, message: &str, reason: &str) -> String {
    format!(
        "<HttpError {status} when requesting {url} returned \"{message}\". Details: \"[{{'message': '{message}', 'domain': 'global', 'reason': '{reason}'}}]\">"
    )
}

pub fn not_found_error(tool: &str, url: &str) -> String {
    format!(
        "API error in {tool}: {}",
        http_error(404, url, "Requested entity was not found.", "notFound")
    )
}

pub fn bad_request(tool: &str, url: &str, message: &str) -> String {
    format!(
        "API error in {tool}: {}",
        http_error(400, url, message, "invalid")
    )
}

/// The weekday name of a unix day.
pub fn weekday_name(days: i64) -> (&'static str, i64) {
    const NAMES: [&str; 7] = [
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
    ];
    // 1970-01-01 was a Thursday (ISO 4).
    let iso = (days + 3).rem_euclid(7) + 1;
    (NAMES[(iso - 1) as usize], iso)
}

/// Parsed RFC 3339 or local time. Returns (unix seconds, had explicit offset).
pub fn parse_datetime(text: &str, zone: Option<&str>) -> Option<(i64, bool)> {
    let t = text.trim();
    if t.len() < 10 {
        return None;
    }
    let (y, m, d) = tz::parse_date(&t[..10])?;
    let days = tz::days_from_civil(y, m, d);
    if t.len() == 10 {
        let local = days * 86_400;
        let off = zone.map(|z| tz::offset_at(z, local).0).unwrap_or(0);
        return Some((local - off, false));
    }
    let rest = t.get(11..)?;
    let hms: Vec<&str> = rest.get(..8.min(rest.len()))?.split(':').collect();
    let h: i64 = hms.first()?.parse().ok()?;
    let mi: i64 = hms.get(1)?.parse().ok()?;
    let sec: i64 = hms
        .get(2)
        .and_then(|x| x.get(..2))
        .and_then(|x| x.parse().ok())
        .unwrap_or(0);
    let local = days * 86_400 + h * 3600 + mi * 60 + sec;
    let tail = rest.get(5..).unwrap_or("");
    let tail = tail.trim_start_matches(|c: char| c == ':' || c.is_ascii_digit() || c == '.');
    if tail.eq_ignore_ascii_case("z") {
        return Some((local, true));
    }
    if let Some(sign) = tail.chars().next().filter(|c| *c == '+' || *c == '-') {
        let parts: Vec<i64> = tail[1..]
            .split(':')
            .filter_map(|x| x.parse().ok())
            .collect();
        let off =
            parts.first().copied().unwrap_or(0) * 3600 + parts.get(1).copied().unwrap_or(0) * 60;
        let off = if sign == '+' { off } else { -off };
        return Some((local - off, true));
    }
    // Local wall-clock time.
    let zone = zone?;
    let guess = local - tz::offset_at(zone, local).0;
    let off = tz::offset_at(zone, guess).0;
    Some((local - off, false))
}

/// `2026-11-03T10:00:00-05:00` in `zone`.
pub fn format_in_zone(secs: i64, zone: &str) -> String {
    let (off, _) = tz::offset_at(zone, secs);
    let local = secs + off;
    let (y, m, d) = tz::civil_from_days(local.div_euclid(86_400));
    let r = local.rem_euclid(86_400);
    let sign = if off < 0 { '-' } else { '+' };
    let a = off.abs();
    if off == 0 && zone == "UTC" {
        return format!(
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
            r / 3600,
            r % 3600 / 60,
            r % 60
        );
    }
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}{sign}{:02}:{:02}",
        r / 3600,
        r % 3600 / 60,
        r % 60,
        a / 3600,
        a % 3600 / 60
    )
}

/// RFC 2822 date header, e.g. `Wed, 29 Oct 2026 10:15:00 -0400`.
pub fn rfc2822(secs: i64, zone: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (off, _) = tz::offset_at(zone, secs);
    let local = secs + off;
    let days = local.div_euclid(86_400);
    let (y, m, d) = tz::civil_from_days(days);
    let (wd, _) = weekday_name(days);
    let r = local.rem_euclid(86_400);
    let sign = if off < 0 { '-' } else { '+' };
    let a = off.abs();
    format!(
        "{}, {d} {} {y} {:02}:{:02}:{:02} {sign}{:02}{:02}",
        &wd[..3],
        MONTHS[(m - 1) as usize],
        r / 3600,
        r % 3600 / 60,
        r % 60,
        a / 3600,
        a % 3600 / 60
    )
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn python_reprs() {
        assert_eq!(
            py_repr(&json!(["Name", "it's", 3])),
            "['Name', \"it's\", 3]"
        );
        assert_eq!(
            py_repr(&json!({"a": null, "b": true})),
            "{'a': None, 'b': True}"
        );
    }

    #[test]
    fn datetimes() {
        let (t, explicit) =
            parse_datetime("2026-11-03T10:00:00", Some("America/New_York")).unwrap();
        assert!(!explicit);
        assert_eq!(
            format_in_zone(t, "America/New_York"),
            "2026-11-03T10:00:00-05:00"
        );
        assert_eq!(tz::rfc3339(t), "2026-11-03T15:00:00Z");
        let (t2, _) = parse_datetime("2026-11-03T15:00:00Z", None).unwrap();
        assert_eq!(t, t2);
        let (t3, _) = parse_datetime("2026-10-29T10:00:00-04:00", None).unwrap();
        assert_eq!(
            format_in_zone(t3, "America/New_York"),
            "2026-10-29T10:00:00-04:00"
        );
        assert_eq!(
            rfc2822(t3, "America/New_York"),
            "Thu, 29 Oct 2026 10:00:00 -0400"
        );
        assert_eq!(
            weekday_name(tz::days_from_civil(2026, 11, 3)),
            ("Tuesday", 2)
        );
    }
}
