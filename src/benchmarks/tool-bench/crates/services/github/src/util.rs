//! Request plumbing and small helpers shared by every endpoint family.

use axum::{
    body::Bytes,
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use mock_core::{
    http::{PageParams, json, link_header, paginate},
    world::ServiceState,
};
use serde_json::{Map, Value, json as json_value};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub const DOCS: &str = "https://docs.github.com/rest";
pub const DOCS_ENTERPRISE: &str = "https://docs.github.com/enterprise-server@3.17/rest";

/// One parsed HTTP request.
pub struct Req {
    pub method: Method,
    pub path: String,
    pub segs: Vec<String>,
    pub query: Vec<(String, String)>,
    pub headers: HeaderMap,
    pub body: Bytes,
    pub base: String,
}

impl Req {
    pub fn new(method: Method, uri: &axum::http::Uri, headers: HeaderMap, body: Bytes) -> Self {
        let path = uri.path().to_owned();
        let segs = path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(percent_decode)
            .collect();
        let query = uri.query().map(parse_query).unwrap_or_default();
        let base = base_url(&headers, uri);
        Self {
            method,
            path,
            segs,
            query,
            headers,
            body,
            base,
        }
    }

    pub fn q(&self, key: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty())
    }

    /// All values of a repeated key (`labels[]=a&labels[]=b` or `fields=1,2`).
    pub fn q_all(&self, key: &str) -> Vec<String> {
        let bracket = format!("{key}[]");
        self.query
            .iter()
            .filter(|(k, _)| k == key || *k == bracket)
            .flat_map(|(_, v)| v.split(',').map(str::to_owned).collect::<Vec<_>>())
            .filter(|v| !v.is_empty())
            .collect()
    }

    pub fn page(&self) -> PageParams {
        PageParams::parse(self.q("page"), self.q("per_page"), 30, 100)
    }

    /// The parsed JSON body, `{}` when empty, or the GitHub 400 for malformed JSON.
    pub fn json(&self) -> Result<Value, Response> {
        if self.body.iter().all(u8::is_ascii_whitespace) {
            return Ok(Value::Object(Map::new()));
        }
        serde_json::from_slice(&self.body).map_err(|_| {
            json(
                StatusCode::BAD_REQUEST,
                json_value!({"message": "Problems parsing JSON", "documentation_url": DOCS, "status": "400"}),
            )
        })
    }

    pub fn api(&self) -> String {
        format!("{}/api/v3", self.base)
    }

    pub fn accept(&self) -> &str {
        self.headers
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
    }
}

pub fn base_url(headers: &HeaderMap, uri: &axum::http::Uri) -> String {
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .or_else(|| uri.authority().map(|a| a.as_str().to_owned()))
        .unwrap_or_else(|| "localhost".to_owned());
    let host = if scheme == "https" {
        host.strip_suffix(":443").unwrap_or(&host).to_owned()
    } else {
        host
    };
    format!("{scheme}://{host}")
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(b) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (
                percent_decode(&k.replace('+', " ")),
                percent_decode(&v.replace('+', " ")),
            )
        })
        .collect()
}

pub fn percent_encode_path(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn percent_encode_query(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// ---------------------------------------------------------------- responses

pub fn ok(body: Value) -> Response {
    json(StatusCode::OK, body)
}

pub fn created(body: Value) -> Response {
    json(StatusCode::CREATED, body)
}

pub fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

pub fn status_json(status: StatusCode, body: Value) -> Response {
    json(status, body)
}

pub fn error(status: StatusCode, message: &str) -> Response {
    json(
        status,
        json_value!({
            "message": message,
            "documentation_url": DOCS,
            "status": status.as_u16().to_string(),
        }),
    )
}

pub fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "Not Found")
}

/// GitHub's 422 "Validation Failed" with one structured error.
pub fn validation(resource: &str, field: &str, code: &str) -> Response {
    json(
        StatusCode::UNPROCESSABLE_ENTITY,
        json_value!({
            "message": "Validation Failed",
            "errors": [{"resource": resource, "field": field, "code": code}],
            "documentation_url": DOCS,
            "status": "422",
        }),
    )
}

/// 422 with a free-form error message (custom code).
pub fn validation_msg(resource: &str, message: &str) -> Response {
    json(
        StatusCode::UNPROCESSABLE_ENTITY,
        json_value!({
            "message": "Validation Failed",
            "errors": [{"resource": resource, "code": "custom", "message": message}],
            "documentation_url": DOCS,
            "status": "422",
        }),
    )
}

pub fn unprocessable(message: &str) -> Response {
    error(StatusCode::UNPROCESSABLE_ENTITY, message)
}

pub fn with_header(mut resp: Response, name: header::HeaderName, value: &str) -> Response {
    if let Ok(v) = HeaderValue::from_str(value) {
        resp.headers_mut().insert(name, v);
    }
    resp
}

/// A page of `items` as a JSON array with GitHub's `Link` header.
pub fn paged(req: &Req, items: Vec<Value>) -> Response {
    let params = req.page();
    let page = paginate(&items, params);
    let resp = ok(Value::Array(page));
    link(req, resp, params, items.len())
}

/// A page of `items` wrapped as `{ "total_count": n, <key>: [...] }`.
pub fn paged_wrapped(req: &Req, key: &str, items: Vec<Value>) -> Response {
    let params = req.page();
    let total = items.len();
    let page = paginate(&items, params);
    let mut body = Map::new();
    body.insert("total_count".into(), json_value!(total));
    body.insert(key.into(), Value::Array(page));
    link(req, ok(Value::Object(body)), params, total)
}

pub fn link(req: &Req, resp: Response, params: PageParams, total: usize) -> Response {
    let others: Vec<String> = req
        .query
        .iter()
        .filter(|(k, _)| k != "page" && k != "per_page")
        .map(|(k, v)| format!("{}={}", percent_encode_query(k), percent_encode_query(v)))
        .collect();
    let mut url = format!("{}{}", req.base, req.path);
    if !others.is_empty() {
        url.push('?');
        url.push_str(&others.join("&"));
    }
    match link_header(&url, params, total) {
        Some(l) => with_header(resp, header::LINK, &l),
        None => resp,
    }
}

// ---------------------------------------------------------------- JSON access

pub fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

pub fn opt_s(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|x| !x.is_empty())
        .map(str::to_owned)
}

pub fn u(v: &Value, key: &str) -> u64 {
    v.get(key)
        .and_then(|x| {
            x.as_u64()
                .or_else(|| x.as_f64().map(|f| f as u64))
                .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(0)
}

pub fn b(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

pub fn arr<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// Login from a user reference stored as `"login"` or `{ "login": ... }`.
pub fn login_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Object(o) => o
            .get("login")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        _ => String::new(),
    }
}

/// Names from a list stored as strings or `{ "name": ... }` objects.
pub fn names_of(list: &[Value]) -> Vec<String> {
    list.iter()
        .map(|v| match v {
            Value::String(s) => s.clone(),
            Value::Object(o) => o
                .get("name")
                .or_else(|| o.get("login"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            _ => String::new(),
        })
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn str_list(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a
            .iter()
            .filter_map(|x| match x {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .collect(),
        Value::String(s) => vec![s.clone()],
        _ => Vec::new(),
    }
}

pub fn as_i64(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_f64().map(|f| f as i64))
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

// ---------------------------------------------------------------- IDs and time

/// `max(field) + 1` over a collection, at least `start`.
pub fn next_id(svc: &ServiceState, collection: &str, field: &str, start: u64) -> u64 {
    svc.store
        .collection(collection)
        .map(|c| c.values().map(|d| u(d, field)).max().unwrap_or(0))
        .unwrap_or(0)
        .max(start - 1)
        + 1
}

const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Deterministic GraphQL node ID for documents that don't carry one.
pub fn node_id(prefix: &str, key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let tail: String = digest
        .iter()
        .take(14)
        .map(|b| BASE62[usize::from(*b) % 62] as char)
        .collect();
    format!("{prefix}_kwDO{tail}")
}

pub fn unix(ts: &str) -> Option<i64> {
    if ts.is_empty() {
        return None;
    }
    if let Ok(t) = OffsetDateTime::parse(ts, &Rfc3339) {
        return Some(t.unix_timestamp());
    }
    // YYYY-MM-DD
    let parts: Vec<&str> = ts.split('-').collect();
    if parts.len() == 3 {
        let y: i32 = parts[0].parse().ok()?;
        let m: u8 = parts[1].parse().ok()?;
        let d: u8 = parts[2].get(..2)?.parse().ok()?;
        let date = time::Date::from_calendar_date(y, time::Month::try_from(m).ok()?, d).ok()?;
        return Some(date.midnight().assume_utc().unix_timestamp());
    }
    None
}

pub fn add_secs(ts: &str, secs: i64) -> String {
    unix(ts).map_or_else(
        || ts.to_owned(),
        |t| mock_core::clock::format_unix(t + secs),
    )
}

// ---------------------------------------------------------------- base64

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// GitHub's contents API wraps base64 at 60 characters with `\n`.
pub fn b64_wrapped(data: &[u8]) -> String {
    let enc = b64_encode(data);
    let mut out = String::new();
    for (i, c) in enc.chars().enumerate() {
        if i > 0 && i % 60 == 0 {
            out.push('\n');
        }
        out.push(c);
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        };
        buf = (buf << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip() {
        for s in ["", "a", "ab", "abc", "hello world\n"] {
            assert_eq!(b64_decode(&b64_encode(s.as_bytes())).unwrap(), s.as_bytes());
        }
        assert_eq!(b64_encode(b"hello"), "aGVsbG8=");
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("a%2Fb%20c"), "a/b c");
        assert_eq!(percent_decode("100%"), "100%");
    }

    #[test]
    fn dates() {
        assert_eq!(unix("1970-01-02"), Some(86400));
        assert_eq!(unix("1970-01-01T00:01:00Z"), Some(60));
    }
}
