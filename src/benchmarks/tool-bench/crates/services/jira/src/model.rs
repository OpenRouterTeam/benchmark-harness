//! Request context, request parsing, response helpers and shared Jira representations.

use std::collections::HashMap;

use axum::http::{Method, StatusCode};
use mock_core::world::ServiceState;
use serde_json::{Map, Value, json};

pub struct Ctx<'a> {
    pub st: &'a mut ServiceState,
    pub now: i64,
    pub base: String,
}

pub enum Out {
    Json(StatusCode, Value),
    Empty(StatusCode),
    Bytes(String, Vec<u8>),
}

pub fn ok(v: Value) -> Out {
    Out::Json(StatusCode::OK, v)
}

pub fn created(v: Value) -> Out {
    Out::Json(StatusCode::CREATED, v)
}

pub fn no_content() -> Out {
    Out::Empty(StatusCode::NO_CONTENT)
}

/// Jira's `{"errorMessages": [...], "errors": {}}` body.
pub fn error(status: u16, msg: &str) -> Out {
    Out::Json(
        StatusCode::from_u16(status).expect("status"),
        json!({"errorMessages": [msg], "errors": {}}),
    )
}

pub fn field_errors(errors: Value) -> Out {
    Out::Json(
        StatusCode::BAD_REQUEST,
        json!({"errorMessages": [], "errors": errors}),
    )
}

pub fn not_found_page() -> Out {
    Out::Json(
        StatusCode::NOT_FOUND,
        json!({"message": "null for uri", "status-code": 404}),
    )
}

pub fn issue_missing() -> Out {
    error(404, "Issue Does Not Exist")
}

pub struct Req {
    pub method: Method,
    pub path: String,
    pub query: HashMap<String, Vec<String>>,
    pub body: Value,
    pub raw: Vec<u8>,
    pub content_type: String,
}

pub fn decode(text: &str) -> String {
    let bytes = text.replace('+', " ").into_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl Req {
    pub fn new(
        method: Method,
        path: &str,
        query: Option<&str>,
        body: &[u8],
        content_type: &str,
    ) -> Self {
        let mut q: HashMap<String, Vec<String>> = HashMap::new();
        for pair in query.unwrap_or("").split('&').filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            q.entry(decode(k)).or_default().push(decode(v));
        }
        let parsed = if content_type.contains("json")
            || body.first().is_some_and(|b| *b == b'{' || *b == b'[')
        {
            serde_json::from_slice(body).unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        Req {
            method,
            path: path.to_owned(),
            query: q,
            body: parsed,
            raw: body.to_vec(),
            content_type: content_type.to_owned(),
        }
    }

    pub fn q(&self, k: &str) -> Option<&str> {
        self.query
            .get(k)
            .and_then(|v| v.first())
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    pub fn q_i64(&self, k: &str) -> Option<i64> {
        self.q(k).and_then(|v| v.parse().ok())
    }

    pub fn q_list(&self, k: &str) -> Vec<String> {
        self.query
            .get(k)
            .map(|v| {
                v.iter()
                    .flat_map(|x| x.split(','))
                    .map(|x| x.trim().to_owned())
                    .filter(|x| !x.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }
}

pub fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

impl Ctx<'_> {
    pub fn all(&self, coll: &str) -> Vec<Value> {
        self.st
            .store
            .collection(coll)
            .map(|c| c.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn get(&self, coll: &str, key: &str) -> Option<Value> {
        self.st.store.collection(coll)?.get(key).cloned()
    }

    pub fn put(&mut self, coll: &str, key: &str, doc: Value) {
        self.st
            .store
            .collection_mut(coll)
            .insert(key.to_owned(), doc);
    }

    pub fn remove(&mut self, coll: &str, key: &str) -> Option<Value> {
        self.st.store.collection_mut(coll).remove(key)
    }

    /// Next numeric ID: one more than the largest `id` in the collection (at least `floor`).
    pub fn next_id(&self, coll: &str, floor: i64) -> String {
        let max = self
            .all(coll)
            .iter()
            .filter_map(|d| id_num(&d["id"]))
            .max()
            .unwrap_or(floor - 1);
        (max.max(floor - 1) + 1).to_string()
    }

    /// `2026-10-29T15:00:00.000+0000`
    pub fn stamp(&self) -> String {
        stamp(self.now)
    }

    pub fn api(&self, path: &str) -> String {
        format!("{}/rest/api/2/{path}", self.base)
    }

    pub fn me(&self) -> Value {
        self.all("users")
            .into_iter()
            .find(|u| u["me"].as_bool().unwrap_or(false))
            .or_else(|| self.all("users").into_iter().next())
            .unwrap_or(json!({"name": "toolbench", "displayName": "Tool Bench"}))
    }

    pub fn me_name(&self) -> String {
        s(&self.me(), "name").to_owned()
    }

    pub fn user_doc(&self, name: &str) -> Option<Value> {
        self.get("users", name).or_else(|| {
            self.all("users")
                .into_iter()
                .find(|u| s(u, "key") == name || s(u, "emailAddress").eq_ignore_ascii_case(name))
        })
    }

    /// Server user representation.
    pub fn user(&self, name: &str) -> Value {
        let Some(u) = self.user_doc(name) else {
            return Value::Null;
        };
        let n = s(&u, "name");
        json!({
            "self": self.api(&format!("user?username={n}")),
            "key": if s(&u, "key").is_empty() { format!("JIRAUSER{}", 10000 + n.len()) } else { s(&u, "key").to_owned() },
            "name": n,
            "emailAddress": s(&u, "emailAddress"),
            "avatarUrls": avatars(&self.base, n),
            "displayName": s(&u, "displayName"),
            "active": u["active"].as_bool().unwrap_or(true),
            "deleted": false,
            "timeZone": if s(&u, "timeZone").is_empty() { "America/New_York" } else { s(&u, "timeZone") },
            "locale": "en_US",
        })
    }

    pub fn user_or_null(&self, v: &Value) -> Value {
        match v.as_str() {
            Some(n) if !n.is_empty() => self.user(n),
            _ => Value::Null,
        }
    }

    /// Resolves a user reference from a request body (`{"name": ..}`, `{"key": ..}`, a string).
    pub fn resolve_user(&self, v: &Value) -> Option<String> {
        let raw = match v {
            Value::String(x) => x.clone(),
            Value::Object(_) => [
                s(v, "name"),
                s(v, "key"),
                s(v, "accountId"),
                s(v, "emailAddress"),
            ]
            .into_iter()
            .find(|x| !x.is_empty())?
            .to_owned(),
            _ => return None,
        };
        self.user_doc(&raw).map(|u| s(&u, "name").to_owned())
    }
}

pub fn id_num(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(x) => x.parse().ok(),
        _ => None,
    }
}

pub fn avatars(base: &str, name: &str) -> Value {
    let mut m = Map::new();
    for (k, size) in [("48x48", 48), ("24x24", 24), ("16x16", 16), ("32x32", 32)] {
        m.insert(
            k.into(),
            json!(format!(
                "{base}/secure/useravatar?size={size}&ownerId={name}"
            )),
        );
    }
    Value::Object(m)
}

pub fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn stamp(t: i64) -> String {
    let (y, m, d) = civil(t.div_euclid(86_400));
    let r = t.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000+0000",
        r / 3600,
        r % 3600 / 60,
        r % 60
    )
}

pub fn iso_z(t: i64) -> String {
    let (y, m, d) = civil(t.div_euclid(86_400));
    let r = t.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z",
        r / 3600,
        r % 3600 / 60,
        r % 60
    )
}

/// Unix seconds of a Jira/ISO timestamp or date (`2026-10-29`, `2026-10-29 10:00`,
/// `2026-10-29T15:00:00.000+0000`, `...Z`).
pub fn parse_time(text: &str) -> Option<i64> {
    let t = text.trim().trim_matches('"').trim_matches('\'');
    if t.len() < 10 {
        return None;
    }
    let parts: Vec<&str> = t[..10].split(['-', '/']).collect();
    if parts.len() != 3 {
        return None;
    }
    let (y, m, d): (i64, i64, i64) = (
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    );
    let mut secs = days_from_civil(y, m, d) * 86_400;
    let rest = t[10..].trim_start_matches(['T', ' ']);
    if rest.len() >= 5 {
        let h: i64 = rest[..2].parse().ok()?;
        let mi: i64 = rest[3..5].parse().ok()?;
        let sec: i64 = rest.get(6..8).and_then(|x| x.parse().ok()).unwrap_or(0);
        secs += h * 3600 + mi * 60 + sec;
        let tail = rest
            .get(5..)
            .unwrap_or("")
            .trim_start_matches(|c: char| c == ':' || c == '.' || c.is_ascii_digit());
        if let Some(sign) = tail.chars().next().filter(|c| *c == '+' || *c == '-') {
            let digits: String = tail[1..].chars().filter(char::is_ascii_digit).collect();
            if digits.len() >= 4 {
                let off = digits[..2].parse::<i64>().ok()? * 3600
                    + digits[2..4].parse::<i64>().ok()? * 60;
                secs -= if sign == '+' { off } else { -off };
            }
        }
    }
    Some(secs)
}

/// Seconds of a Jira duration (`3h 30m`, `1d`, `2w`; 8h days, 5d weeks).
pub fn duration_secs(text: &str) -> Option<i64> {
    let mut total = 0;
    let mut any = false;
    for part in text.split_whitespace() {
        let (num, unit) = part.split_at(part.find(|c: char| c.is_ascii_alphabetic())?);
        let n: f64 = num.parse().ok()?;
        let mult = match unit {
            "w" => 5 * 8 * 3600,
            "d" => 8 * 3600,
            "h" => 3600,
            "m" => 60,
            _ => return None,
        };
        total += (n * f64::from(mult)) as i64;
        any = true;
    }
    any.then_some(total)
}

pub fn format_duration(secs: i64) -> String {
    let mut parts = Vec::new();
    let mut r = secs;
    for (unit, size) in [("w", 5 * 8 * 3600), ("d", 8 * 3600), ("h", 3600), ("m", 60)] {
        if r >= size {
            parts.push(format!("{}{unit}", r / size));
            r %= size;
        }
    }
    if parts.is_empty() {
        "0m".into()
    } else {
        parts.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_and_durations() {
        let t = parse_time("2026-10-29T15:00:00.000+0000").unwrap();
        assert_eq!(stamp(t), "2026-10-29T15:00:00.000+0000");
        assert_eq!(parse_time("2026-10-29T11:00:00.000-0400"), Some(t));
        assert_eq!(parse_time("2026-10-29 15:00"), Some(t));
        assert_eq!(duration_secs("1d 2h 30m"), Some(8 * 3600 + 2 * 3600 + 1800));
        assert_eq!(format_duration(37800), "1d 2h 30m");
    }
}
