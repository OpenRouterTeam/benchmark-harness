//! Error envelopes, time formatting and small request helpers shared by every endpoint.

use axum::{
    http::{HeaderMap, StatusCode, header},
    response::Response,
};
use mock_core::{
    clock::format_unix,
    http::json,
    world::{ServiceState, SharedWorld},
};
use serde_json::{Map, Value, json as json_value};
use time::{Date, Month, OffsetDateTime, format_description::well_known::Rfc3339};

/// Runs `f` on the HubSpot service state with the current logical time (unix seconds).
pub fn with_service<R>(world: &SharedWorld, f: impl FnOnce(&mut ServiceState, i64) -> R) -> R {
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    let now = world.clock.now_unix();
    f(world.service_mut(crate::NAME), now)
}

/// HubSpot's standard error body: `{status, message, correlationId, category}`.
pub fn error(
    svc: &mut ServiceState,
    status: StatusCode,
    category: &str,
    message: &str,
) -> Response {
    let correlation = svc.ids.next_uuid("correlation").to_string();
    json(
        status,
        json_value!({
            "status": "error",
            "message": message,
            "correlationId": correlation,
            "category": category,
        }),
    )
}

pub fn not_found(svc: &mut ServiceState, message: &str) -> Response {
    error(svc, StatusCode::NOT_FOUND, "OBJECT_NOT_FOUND", message)
}

pub fn validation(svc: &mut ServiceState, message: &str) -> Response {
    error(svc, StatusCode::BAD_REQUEST, "VALIDATION_ERROR", message)
}

pub fn bad_json(svc: &mut ServiceState) -> Response {
    validation(
        svc,
        "Invalid input JSON on line 1, column 1: Cannot build request body from the provided input",
    )
}

/// `2026-03-14T09:00:07.000Z` from unix seconds (HubSpot timestamps carry milliseconds).
pub fn iso_secs(secs: i64) -> String {
    iso_ms(secs * 1000)
}

pub fn iso_ms(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let base = format_unix(secs);
    format!("{}.{millis:03}Z", base.trim_end_matches('Z'))
}

/// Milliseconds since the epoch for an RFC 3339 timestamp or a `YYYY-MM-DD` date.
pub fn parse_ms(s: &str) -> Option<i64> {
    if let Ok(t) = OffsetDateTime::parse(s, &Rfc3339) {
        return i64::try_from(t.unix_timestamp_nanos() / 1_000_000).ok();
    }
    let date = parse_date(s)?;
    Some(date.midnight().assume_utc().unix_timestamp() * 1000)
}

fn parse_date(s: &str) -> Option<Date> {
    let mut parts = s.split('-');
    let (y, m, d) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || y.len() != 4 || m.len() != 2 || d.len() != 2 {
        return None;
    }
    let month = Month::try_from(m.parse::<u8>().ok()?).ok()?;
    Date::from_calendar_date(y.parse().ok()?, month, d.parse().ok()?).ok()
}

/// A JSON scalar as HubSpot's string form (`5` -> `"5"`, `true` -> `"true"`).
pub fn scalar_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Numeric view of a property value: a number, or a date/timestamp in epoch milliseconds.
pub fn numeric(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    t.parse::<f64>()
        .ok()
        .or_else(|| parse_ms(t).map(|ms| ms as f64))
}

pub fn base_url(headers: &HeaderMap) -> String {
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    format!("{scheme}://{host}")
}

/// Query values for `key`, accepting both repeated keys and comma-separated lists.
pub fn query_list(query: &[(String, String)], key: &str) -> Vec<String> {
    query
        .iter()
        .filter(|(k, _)| k == key)
        .flat_map(|(_, v)| v.split(','))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

pub fn query_one<'a>(query: &'a [(String, String)], key: &str) -> Option<&'a str> {
    query
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

pub fn query_bool(query: &[(String, String)], key: &str) -> bool {
    query_one(query, key).is_some_and(|v| v.eq_ignore_ascii_case("true"))
}

/// `limit` clamped to `1..=max`, `after` as an offset cursor.
pub fn page_window(query: &[(String, String)], default_limit: usize, max: usize) -> (usize, usize) {
    let limit = query_one(query, "limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default_limit)
        .clamp(1, max);
    let after = query_one(query, "after")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    (limit, after)
}

/// `{"next": {"after": "<n>", "link": "<url>"}}` when more results follow.
pub fn paging(total: usize, after: usize, limit: usize, link_base: Option<&str>) -> Option<Value> {
    let next = after + limit;
    if next >= total {
        return None;
    }
    let mut next_obj = Map::new();
    next_obj.insert("after".into(), Value::String(next.to_string()));
    if let Some(base) = link_base {
        let sep = if base.contains('?') { '&' } else { '?' };
        next_obj.insert(
            "link".into(),
            Value::String(format!("{base}{sep}after={next}")),
        );
    }
    Some(json_value!({ "next": next_obj }))
}

/// Numeric id order (HubSpot ids are numeric strings); non-numeric ids sort after, by text.
pub fn id_order(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        (Ok(_), Err(_)) => std::cmp::Ordering::Less,
        (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
        _ => a.cmp(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_and_parses_times() {
        assert_eq!(iso_secs(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_ms(1_500), "1970-01-01T00:00:01.500Z");
        assert_eq!(parse_ms("2026-11-20"), Some(1_795_132_800_000));
        assert_eq!(
            parse_ms("2026-11-20T00:00:00.000Z"),
            Some(1_795_132_800_000)
        );
        assert_eq!(parse_ms("20-11-2026"), None);
        assert_eq!(numeric("42.5"), Some(42.5));
        assert_eq!(numeric(""), None);
    }

    #[test]
    fn paging_only_when_more_follow() {
        assert_eq!(paging(3, 0, 3, None), None);
        assert_eq!(
            paging(5, 0, 2, Some("http://h/x?limit=2")),
            Some(json_value!({"next": {"after": "2", "link": "http://h/x?limit=2&after=2"}}))
        );
    }
}
