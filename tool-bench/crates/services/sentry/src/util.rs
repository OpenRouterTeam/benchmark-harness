//! Request parsing, Sentry-style errors, cursor pagination and time helpers.

use axum::{
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
    response::Response,
};
use mock_core::{http::json, store::Collection, world::WorldState};
use serde_json::{Value, json as json_value};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::NAME;

/// Query string with repeated keys preserved (`field=a&field=b`).
#[derive(Debug, Clone, Default)]
pub struct Query(Vec<(String, String)>);

impl Query {
    pub fn parse(raw: Option<&str>) -> Self {
        let pairs = raw
            .unwrap_or("")
            .split('&')
            .filter(|p| !p.is_empty())
            .map(|pair| {
                let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                (decode(k), decode(v))
            })
            .collect();
        Self(pairs)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// The value, or `None` when missing or empty.
    pub fn non_empty(&self, key: &str) -> Option<&str> {
        self.get(key).filter(|v| !v.trim().is_empty())
    }

    pub fn all(&self, key: &str) -> Vec<&str> {
        self.0
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    /// The query string without `cursor`, re-encoded, for pagination links.
    pub fn without_cursor(&self) -> String {
        self.0
            .iter()
            .filter(|(k, _)| k != "cursor")
            .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
            .collect::<Vec<_>>()
            .join("&")
    }
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                    out.push(h * 16 + l);
                    i += 3;
                    continue;
                }
                out.push(b'%');
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b':' | b'@') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `http://<host>` as the client addressed us.
pub fn base_url(headers: &HeaderMap) -> String {
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("world:8113");
    format!("{scheme}://{host}")
}

pub fn ok(body: Value) -> Response {
    json(StatusCode::OK, body)
}

pub fn json_status(status: StatusCode, body: Value) -> Response {
    json(status, body)
}

pub fn created(body: Value) -> Response {
    json(StatusCode::CREATED, body)
}

pub fn no_content() -> Response {
    axum::response::IntoResponse::into_response(StatusCode::NO_CONTENT)
}

pub fn detail(status: StatusCode, message: &str) -> Response {
    json(status, json_value!({ "detail": message }))
}

pub fn not_found() -> Response {
    detail(
        StatusCode::NOT_FOUND,
        "The requested resource does not exist",
    )
}

pub fn bad_request(message: &str) -> Response {
    detail(StatusCode::BAD_REQUEST, message)
}

/// DRF field validation error, e.g. `{"name": ["This field is required."]}`.
pub fn field_error(field: &str, message: &str) -> Response {
    json(StatusCode::BAD_REQUEST, json_value!({ field: [message] }))
}

pub fn parse_body(body: &[u8]) -> Result<Value, Response> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(json_value!({}));
    }
    serde_json::from_slice::<Value>(body)
        .ok()
        .filter(Value::is_object)
        .ok_or_else(|| bad_request("JSON parse error - Expecting value: line 1 column 1 (char 0)"))
}

pub struct Page {
    pub items: Vec<Value>,
    pub link: String,
}

/// Sentry cursor pagination (`cursor=<value>:<offset>:<is_prev>`) with a `Link` header that
/// always carries both `previous` and `next` entries and their `results` flags.
pub fn page(
    items: Vec<Value>,
    query: &Query,
    uri: &Uri,
    headers: &HeaderMap,
    default_per_page: usize,
    max_per_page: usize,
) -> Page {
    let per_page = query
        .get("per_page")
        .or_else(|| query.get("limit"))
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default_per_page)
        .clamp(1, max_per_page);
    let offset = query
        .get("cursor")
        .and_then(|c| c.split(':').nth(1))
        .and_then(|o| o.parse::<usize>().ok())
        .unwrap_or(0);
    let total = items.len();
    let items: Vec<Value> = items.into_iter().skip(offset).take(per_page).collect();
    let has_next = offset + per_page < total;
    let has_prev = offset > 0;

    let qs = query.without_cursor();
    let base = format!("{}{}", base_url(headers), uri.path());
    let url = if qs.is_empty() {
        format!("{base}?")
    } else {
        format!("{base}?{qs}&")
    };
    let prev_cursor = format!("0:{}:1", offset.saturating_sub(per_page));
    let next_cursor = format!("0:{}:0", offset + per_page);
    let link = format!(
        "<{url}cursor={prev_cursor}>; rel=\"previous\"; results=\"{has_prev}\"; cursor=\"{prev_cursor}\", \
         <{url}cursor={next_cursor}>; rel=\"next\"; results=\"{has_next}\"; cursor=\"{next_cursor}\""
    );
    Page { items, link }
}

pub fn with_link(mut resp: Response, link: &str) -> Response {
    if let Ok(v) = HeaderValue::from_str(link) {
        resp.headers_mut().insert(header::LINK, v);
    }
    resp
}

pub fn paginate(
    items: Vec<Value>,
    query: &Query,
    uri: &Uri,
    headers: &HeaderMap,
    default_per_page: usize,
    max_per_page: usize,
) -> Response {
    let p = page(items, query, uri, headers, default_per_page, max_per_page);
    with_link(ok(Value::Array(p.items)), &p.link)
}

/// Parses RFC 3339 (with or without fractional seconds) to unix seconds.
pub fn parse_time(s: &str) -> Option<i64> {
    OffsetDateTime::parse(s, &Rfc3339)
        .ok()
        .map(OffsetDateTime::unix_timestamp)
        .or_else(|| {
            // Date only, or a timestamp without a zone (`2026-03-14T09:00:00`).
            let padded = if s.len() == 10 {
                format!("{s}T00:00:00Z")
            } else {
                format!("{s}Z")
            };
            OffsetDateTime::parse(&padded, &Rfc3339)
                .ok()
                .map(OffsetDateTime::unix_timestamp)
        })
}

pub fn value_time(v: &Value) -> Option<i64> {
    match v {
        Value::String(s) => parse_time(s),
        Value::Number(n) => n.as_f64().map(|f| f as i64),
        _ => None,
    }
}

/// `24h`, `14d`, `2w`, `30m`, `90s` to seconds.
pub fn parse_period(s: &str) -> Option<i64> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit())?);
    let n: i64 = num.parse().ok()?;
    let mult = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        "w" => 604_800,
        _ => return None,
    };
    Some(n * mult)
}

/// The `[start, end]` window of a request (`statsPeriod` or `start`/`end`), defaulting to
/// `default_period` before the scenario's current time.
pub fn time_window(query: &Query, now: i64, default_period: &str) -> Result<(i64, i64), Response> {
    if let (Some(start), Some(end)) = (query.non_empty("start"), query.non_empty("end")) {
        let (Some(s), Some(e)) = (parse_time(start), parse_time(end)) else {
            return Err(bad_request(
                "start and end must be valid ISO 8601 date strings",
            ));
        };
        return Ok((s, e));
    }
    let period = query.non_empty("statsPeriod").unwrap_or(default_period);
    let Some(secs) = parse_period(period) else {
        return Err(bad_request(&format!("Invalid statsPeriod: {period}")));
    };
    Ok((now - secs, now))
}

pub fn now(world: &WorldState) -> i64 {
    world.clock.now_unix()
}

pub fn collection<'a>(world: &'a WorldState, name: &str) -> Option<&'a Collection> {
    world.service(NAME).and_then(|s| s.store.collection(name))
}

pub fn docs<'a>(world: &'a WorldState, name: &str) -> Vec<&'a Value> {
    collection(world, name)
        .map(|c| c.values().collect())
        .unwrap_or_default()
}

pub fn str_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Lowercase hex string of `len` characters derived from the service's ID stream.
pub fn hex_id(world: &mut WorldState, kind: &str, len: usize) -> String {
    let raw = world.service_mut(NAME).ids.next_prefixed(kind, "", len * 2);
    raw.bytes()
        .take(len)
        .map(|b| char::from(b"0123456789abcdef"[usize::from(b) % 16]))
        .collect()
}

/// Next numeric id: one more than the largest `id` in the collection (at least `floor`).
pub fn next_numeric_id(world: &WorldState, collection_name: &str, floor: u64) -> u64 {
    docs(world, collection_name)
        .iter()
        .filter_map(|d| match &d["id"] {
            Value::String(s) => s.parse::<u64>().ok(),
            Value::Number(n) => n.as_u64(),
            _ => None,
        })
        .max()
        .map_or(floor, |m| m.max(floor - 1) + 1)
}

/// `name` lowercased with runs of non-alphanumerics replaced by `-`, as Sentry slugifies.
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').chars().take(50).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_decodes_and_keeps_repeats() {
        let q = Query::parse(Some(
            "field=issue&field=count%28%29&query=is%3Aunresolved+level%3Aerror&cursor=0:10:0",
        ));
        assert_eq!(q.all("field"), ["issue", "count()"]);
        assert_eq!(q.get("query"), Some("is:unresolved level:error"));
        assert_eq!(
            q.without_cursor(),
            "field=issue&field=count%28%29&query=is:unresolved%20level:error"
        );
    }

    #[test]
    fn periods_and_times() {
        assert_eq!(parse_period("14d"), Some(14 * 86_400));
        assert_eq!(parse_period("2w"), Some(2 * 604_800));
        assert_eq!(parse_period("x"), None);
        assert_eq!(parse_time("2026-03-14T09:00:00Z"), Some(1_773_478_800));
        assert_eq!(parse_time("2026-03-14T09:00:00.250Z"), Some(1_773_478_800));
        assert_eq!(parse_time("2026-03-14"), Some(1_773_446_400));
    }

    #[test]
    fn slugs() {
        assert_eq!(slugify("Checkout Edge (beta)"), "checkout-edge-beta");
    }
}
