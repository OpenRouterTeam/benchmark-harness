//! Shared helpers: Datadog error envelopes, store access, query-string parsing.

use std::collections::HashMap;

use axum::http::StatusCode;
use mock_core::world::WorldState;
use serde_json::Value;

use crate::NAME;

pub mod errors {
    use axum::{http::StatusCode, response::Response};
    use mock_core::http::json;
    use serde_json::json as json_value;

    /// `{"errors": [message]}` with the given status (Datadog's `APIErrorResponse`).
    pub fn error(status: StatusCode, message: &str) -> Response {
        json(status, json_value!({ "errors": [message] }))
    }

    pub fn not_found() -> Response {
        error(StatusCode::NOT_FOUND, "Not found")
    }

    pub fn forbidden() -> Response {
        error(StatusCode::FORBIDDEN, "Forbidden")
    }

    pub fn bad_request(message: &str) -> Response {
        error(StatusCode::BAD_REQUEST, message)
    }
}

pub const OK: StatusCode = StatusCode::OK;

/// Every document of a collection, in dataset order.
pub fn docs(world: &WorldState, collection: &str) -> Vec<Value> {
    world
        .service(NAME)
        .and_then(|s| s.store.collection(collection))
        .map(|c| c.values().cloned().collect())
        .unwrap_or_default()
}

pub fn doc(world: &WorldState, collection: &str, key: &str) -> Option<Value> {
    world
        .service(NAME)
        .and_then(|s| s.store.collection(collection))
        .and_then(|c| c.get(key))
        .cloned()
}

/// Raw query string parsed into a map (last value wins). Keys keep brackets, e.g. `page[size]`.
pub type Params = HashMap<String, String>;

/// Parses an integer query parameter; `Err` carries Datadog's 400 response.
pub fn int_param(params: &Params, key: &str) -> Result<Option<i64>, axum::response::Response> {
    match params.get(key) {
        None => Ok(None),
        Some(v) if v.is_empty() => Ok(None),
        Some(v) => v
            .parse::<f64>()
            .ok()
            .filter(|f| f.fract() == 0.0)
            .map(|f| Some(f as i64))
            .ok_or_else(|| errors::bad_request(&format!("Invalid value for parameter '{key}'"))),
    }
}

pub fn bool_param(params: &Params, key: &str) -> Option<bool> {
    params.get(key).map(|v| v.eq_ignore_ascii_case("true"))
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Opaque pagination cursor for search endpoints: base64url of `{"o":<offset>}`.
pub fn encode_cursor(offset: usize) -> String {
    let text = format!("{{\"o\":{offset}}}");
    let bytes = text.as_bytes();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(B64[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

pub fn decode_cursor(cursor: &str) -> Option<usize> {
    let mut values = Vec::new();
    for c in cursor.trim_end_matches('=').bytes() {
        values.push(B64.iter().position(|b| *b == c)? as u32);
    }
    let mut bytes = Vec::new();
    for chunk in values.chunks(4) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, v)| acc | (v << (18 - 6 * i)));
        for i in 0..chunk.len().saturating_sub(1) {
            bytes.push((n >> (16 - 8 * i)) as u8);
        }
    }
    let text = String::from_utf8(bytes).ok()?;
    text.strip_prefix("{\"o\":")?
        .strip_suffix('}')?
        .parse()
        .ok()
}

/// Looks up a dotted path (`a.b.c`) in a JSON object.
pub fn path<'a>(value: &'a Value, dotted: &str) -> Option<&'a Value> {
    if let Some(v) = value.get(dotted) {
        return Some(v);
    }
    let mut current = value;
    for part in dotted.split('.') {
        current = current.get(part)?;
    }
    Some(current)
}

/// Decodes `a=b&c%5Bd%5D=e` (percent-encoding and `+`), keeping bracketed keys verbatim.
pub fn parse_query(raw: Option<&str>) -> Params {
    let mut out = Params::new();
    for pair in raw.unwrap_or_default().split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        out.insert(percent_decode(k), percent_decode(v));
    }
    out
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let decoded = (bytes[i] == b'%' && i + 2 < bytes.len())
            .then(|| std::str::from_utf8(&bytes[i + 1..i + 3]).ok())
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (bytes[i], decoded) {
            (_, Some(b)) => {
                out.push(b);
                i += 2;
            }
            (b'+', None) => out.push(b' '),
            (b, None) => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trips() {
        for n in [0, 1, 25, 1000, 123_456] {
            assert_eq!(decode_cursor(&encode_cursor(n)), Some(n));
        }
        assert_eq!(decode_cursor("not a cursor!"), None);
    }

    #[test]
    fn query_strings_decode() {
        let p = parse_query(Some(
            "filter%5Bquery%5D=%40type%3Aview+env%3Aprod&page[limit]=5&x",
        ));
        assert_eq!(p["filter[query]"], "@type:view env:prod");
        assert_eq!(p["page[limit]"], "5");
        assert_eq!(p["x"], "");
        assert_eq!(percent_decode("100%"), "100%");
    }
}
