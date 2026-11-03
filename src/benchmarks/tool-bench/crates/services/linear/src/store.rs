//! The Linear workspace store and helpers shared by every tool family.
//!
//! Documents keep Linear's field names and reference other entities by ID (`teamId`,
//! `stateId`, ...). Collections are keyed by UUID, except `issues` (keyed by identifier,
//! e.g. `ENG-42`) and `comments` (keyed `<target>#<n>`, e.g. `ENG-42#3`), so the grader can
//! address created records without knowing generated IDs.

use mock_core::{clock::format_unix, world::ServiceState};
use serde_json::{Map, Value, json};

pub type R = Result<Value, String>;

pub struct Ctx<'a> {
    pub st: &'a mut ServiceState,
    pub now: i64,
}

pub fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// `value` unless null/missing.
pub fn opt(v: &Value, k: &str) -> Option<Value> {
    v.get(k).filter(|x| !x.is_null()).cloned()
}

/// A non-empty string argument.
pub fn arg<'a>(args: &'a Value, k: &str) -> Option<&'a str> {
    args.get(k)
        .and_then(Value::as_str)
        .filter(|x| !x.trim().is_empty())
}

pub fn arg_i64(args: &Value, k: &str) -> Option<i64> {
    args.get(k).and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_f64().map(|f| f as i64))
            .or_else(|| v.as_str().and_then(|x| x.parse().ok()))
    })
}

pub fn arg_bool(args: &Value, k: &str) -> Option<bool> {
    args.get(k).and_then(Value::as_bool)
}

pub fn arg_strs(args: &Value, k: &str) -> Option<Vec<String>> {
    args.get(k).and_then(Value::as_array).map(|a| {
        a.iter()
            .filter_map(|x| x.as_str().map(str::to_owned))
            .collect()
    })
}

/// Builds an object the way the Linear SDK serializes models: `null` fields are dropped
/// (the SDK maps them to `undefined`, which `JSON.stringify` omits).
pub fn sdk(fields: Vec<(&str, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in fields {
        if !v.is_null() {
            m.insert(k.to_owned(), v);
        }
    }
    Value::Object(m)
}

/// Builds an object keeping explicit nulls (normalizers that use `?? null`).
pub fn obj(fields: Vec<(&str, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in fields {
        m.insert(k.to_owned(), v);
    }
    Value::Object(m)
}

pub fn get_or_null(v: &Value, k: &str) -> Value {
    v.get(k).cloned().unwrap_or(Value::Null)
}

pub fn not_found(kind: &str) -> String {
    format!("Entity not found: {kind} - Could not find referenced {kind}.")
}

pub fn slugify(text: &str) -> String {
    let mut out = String::new();
    for c in text.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-')
        .chars()
        .take(60)
        .collect::<String>()
        .trim_end_matches('-')
        .to_owned()
}

impl Ctx<'_> {
    pub fn all(&self, coll: &str) -> Vec<Value> {
        self.st
            .store
            .collection(coll)
            .map(|c| c.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn entries(&self, coll: &str) -> Vec<(String, Value)> {
        self.st
            .store
            .collection(coll)
            .map(|c| c.iter().map(|(k, v)| (k.to_owned(), v.clone())).collect())
            .unwrap_or_default()
    }

    /// By key, then by `id` field.
    pub fn get(&self, coll: &str, id: &str) -> Option<Value> {
        let c = self.st.store.collection(coll)?;
        if let Some(v) = c.get(id) {
            return Some(v.clone());
        }
        c.values().find(|v| s(v, "id") == id).cloned()
    }

    pub fn key_of(&self, coll: &str, id: &str) -> Option<String> {
        let c = self.st.store.collection(coll)?;
        if c.contains(id) {
            return Some(id.to_owned());
        }
        c.iter()
            .find(|(_, v)| s(v, "id") == id)
            .map(|(k, _)| k.to_owned())
    }

    pub fn put(&mut self, coll: &str, key: &str, doc: Value) {
        self.st
            .store
            .collection_mut(coll)
            .insert(key.to_owned(), doc);
    }

    /// Replaces the document with this `id` (keeping its key), or inserts it under `id`.
    pub fn save(&mut self, coll: &str, doc: Value) {
        let id = s(&doc, "id").to_owned();
        let key = self.key_of(coll, &id).unwrap_or(id);
        self.put(coll, &key, doc);
    }

    pub fn remove(&mut self, coll: &str, id: &str) -> Option<Value> {
        let key = self.key_of(coll, id)?;
        self.st.store.collection_mut(coll).remove(&key)
    }

    pub fn need(&self, coll: &str, id: &str, kind: &str) -> R {
        self.get(coll, id).ok_or_else(|| not_found(kind))
    }

    pub fn iso_now(&self) -> String {
        iso(self.now)
    }

    /// A v4-shaped UUID, deterministic per kind.
    pub fn uuid(&mut self, kind: &str) -> String {
        let u = self.st.ids.next_uuid(kind);
        let mut b = *u.as_bytes();
        b[6] = (b[6] & 0x0f) | 0x40;
        b[8] = (b[8] & 0x3f) | 0x80;
        let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// 12-hex-character slug ID like Linear's `slugId`.
    pub fn slug_id(&mut self, kind: &str) -> String {
        let u = self.uuid(kind);
        u.replace('-', "")[..12].to_owned()
    }

    pub fn org(&self) -> Value {
        self.get("organization", "organization").unwrap_or_else(|| {
            json!({"id": "b1c8a4f0-2d3e-4f56-8a9b-0c1d2e3f4a5b", "name": "Octo Corp", "urlKey": "octo"})
        })
    }

    pub fn url_key(&self) -> String {
        match s(&self.org(), "urlKey") {
            "" => "octo".to_owned(),
            k => k.to_owned(),
        }
    }

    pub fn viewer_id(&self) -> String {
        let org = self.org();
        match s(&org, "viewerId") {
            "" => self
                .all("users")
                .first()
                .map(|u| s(u, "id").to_owned())
                .unwrap_or_default(),
            v => v.to_owned(),
        }
    }

    pub fn user_ref(&self, id: &str) -> Value {
        match self.get("users", id) {
            Some(u) => json!({"id": s(&u, "id"), "name": s(&u, "name")}),
            None => Value::Null,
        }
    }

    pub fn user_ref_email(&self, id: &str) -> Value {
        match self.get("users", id) {
            Some(u) => json!({"id": s(&u, "id"), "name": s(&u, "name"), "email": s(&u, "email")}),
            None => Value::Null,
        }
    }

    pub fn simple_user(&self, id: &str) -> Value {
        match self.get("users", id) {
            Some(u) => obj(vec![
                ("id", json!(s(&u, "id"))),
                ("name", json!(s(&u, "name"))),
                ("email", opt(&u, "email").unwrap_or(Value::Null)),
                ("displayName", opt(&u, "displayName").unwrap_or(Value::Null)),
            ]),
            None => Value::Null,
        }
    }

    pub fn named_ref(&self, coll: &str, id: &str) -> Value {
        match self.get(coll, id) {
            Some(x) => json!({"id": s(&x, "id"), "name": get_or_null(&x, "name")}),
            None => Value::Null,
        }
    }

    pub fn team_ref(&self, id: &str) -> Value {
        match self.get("teams", id) {
            Some(t) => json!({"id": s(&t, "id"), "name": s(&t, "name")}),
            None => Value::Null,
        }
    }

    pub fn team_ref_key(&self, id: &str) -> Value {
        match self.get("teams", id) {
            Some(t) => json!({"id": s(&t, "id"), "name": s(&t, "name"), "key": s(&t, "key")}),
            None => Value::Null,
        }
    }

    pub fn issue_url(&self, issue: &Value) -> String {
        format!(
            "https://linear.app/{}/issue/{}/{}",
            self.url_key(),
            s(issue, "identifier"),
            slugify(s(issue, "title"))
        )
    }

    pub fn project_url(&self, p: &Value) -> String {
        format!(
            "https://linear.app/{}/project/{}-{}",
            self.url_key(),
            slugify(s(p, "name")),
            s(p, "slugId")
        )
    }

    /// Issue by UUID or identifier (case-insensitive).
    pub fn find_issue(&self, id: &str) -> Option<(String, Value)> {
        let coll = self.st.store.collection("issues")?;
        if let Some(v) = coll.get(id) {
            return Some((id.to_owned(), v.clone()));
        }
        let upper = id.to_uppercase();
        coll.iter()
            .find(|(_, v)| s(v, "id") == id || s(v, "identifier").to_uppercase() == upper)
            .map(|(k, v)| (k.to_owned(), v.clone()))
    }

    pub fn need_issue(&self, id: &str) -> Result<(String, Value), String> {
        self.find_issue(id).ok_or_else(|| not_found("Issue"))
    }

    pub fn issue_ref(&self, id: &str) -> Value {
        match self.find_issue(id) {
            Some((_, i)) => {
                json!({"id": s(&i, "id"), "identifier": s(&i, "identifier"), "title": s(&i, "title")})
            }
            None => Value::Null,
        }
    }

    /// Records an issue history event.
    pub fn history(&mut self, issue_id: &str, kind: &str, from: Value, to: Value) {
        let id = self.uuid("history");
        let doc = json!({
            "id": id, "issueId": issue_id, "createdAt": self.iso_now(), "actorId": self.viewer_id(),
            "type": kind, "from": from, "to": to,
        });
        self.put("issue_history", &id, doc);
    }
}

/// `2026-10-29T15:00:07.000Z`
pub fn iso(secs: i64) -> String {
    let t = format_unix(secs);
    format!("{}.000Z", t.trim_end_matches('Z'))
}

/// Parses RFC 3339 (with optional fraction) or `YYYY-MM-DD` to unix seconds.
pub fn parse_time(text: &str) -> Option<i64> {
    let t = text.trim();
    let date = t.get(0..10)?;
    let parts: Vec<i64> = date.split('-').filter_map(|x| x.parse().ok()).collect();
    if parts.len() != 3 {
        return None;
    }
    let days = days_from_civil(parts[0], parts[1], parts[2]);
    let mut secs = days * 86_400;
    if t.len() > 10 {
        let rest = &t[11..];
        let hms: Vec<i64> = rest
            .get(0..8)
            .unwrap_or("00:00:00")
            .split(':')
            .filter_map(|x| x.parse().ok())
            .collect();
        if hms.len() == 3 {
            secs += hms[0] * 3600 + hms[1] * 60 + hms[2];
        }
        if let Some(pos) = rest.rfind(['+', '-'])
            && pos >= 8
        {
            let sign = if &rest[pos..pos + 1] == "+" { 1 } else { -1 };
            let off: Vec<i64> = rest[pos + 1..]
                .split(':')
                .filter_map(|x| x.parse().ok())
                .collect();
            if off.len() == 2 {
                secs -= sign * (off[0] * 3600 + off[1] * 60);
            }
        }
    }
    Some(secs)
}

pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Normalizes an input timestamp to Linear's `...sss Z` form.
pub fn norm_time(text: &str) -> Option<String> {
    parse_time(text).map(iso)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_round_trip() {
        assert_eq!(
            norm_time("2026-10-29T10:00:00-05:00").unwrap(),
            "2026-10-29T15:00:00.000Z"
        );
        assert_eq!(norm_time("2026-10-29").unwrap(), "2026-10-29T00:00:00.000Z");
        assert_eq!(
            norm_time("2026-10-29T15:00:00.123Z").unwrap(),
            "2026-10-29T15:00:00.000Z"
        );
        assert_eq!(
            slugify("Checkout fails: 3DS (EU)!"),
            "checkout-fails-3ds-eu"
        );
    }
}
