//! Request/response plumbing shared by every Okta resource module.

use axum::{
    body::Body,
    http::{HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use mock_core::world::{ServiceState, WorldState};
use serde_json::{Map, Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{NAME, ORG_URL};

/// What a handler answers; turned into an HTTP response with Okta's headers.
#[derive(Debug)]
pub enum Reply {
    Json(StatusCode, Value),
    /// A list page plus its `Link` header value.
    Page(Vec<Value>, String),
    Empty(StatusCode),
}

impl Reply {
    pub fn ok(v: Value) -> Self {
        Reply::Json(StatusCode::OK, v)
    }
    pub fn created(v: Value) -> Self {
        Reply::Json(StatusCode::CREATED, v)
    }
    pub fn no_content() -> Self {
        Reply::Empty(StatusCode::NO_CONTENT)
    }
}

pub type Res = Result<Reply, Reply>;

pub fn into_response(reply: Reply, request_id: &str) -> Response {
    let (status, body, link) = match reply {
        Reply::Json(s, v) => (s, Some(v.to_string()), None),
        Reply::Page(items, link) => (
            StatusCode::OK,
            Some(Value::Array(items).to_string()),
            Some(link),
        ),
        Reply::Empty(s) => (s, None, None),
    };
    let mut resp = match body {
        Some(b) => (status, [(header::CONTENT_TYPE, "application/json")], b).into_response(),
        None => Response::builder()
            .status(status)
            .body(Body::empty())
            .expect("empty response"),
    };
    let h = resp.headers_mut();
    if let Some(link) = link
        && let Ok(v) = HeaderValue::from_str(&link)
    {
        h.insert(header::LINK, v);
    }
    h.insert(
        "x-okta-request-id",
        HeaderValue::from_str(request_id).expect("ascii id"),
    );
    h.insert("x-rate-limit-limit", HeaderValue::from_static("600"));
    h.insert("x-rate-limit-remaining", HeaderValue::from_static("599"));
    h.insert("x-rate-limit-reset", HeaderValue::from_static("4102444800"));
    resp
}

/// Okta error body.
pub fn error(status: StatusCode, code: &str, summary: &str, causes: &[String]) -> Reply {
    let causes: Vec<Value> = causes
        .iter()
        .map(|c| json!({ "errorSummary": c }))
        .collect();
    Reply::Json(
        status,
        json!({
            "errorCode": code,
            "errorSummary": summary,
            "errorLink": code,
            "errorId": "oaetoolbench0000000000",
            "errorCauses": causes,
        }),
    )
}

pub fn not_found(id: &str, kind: &str) -> Reply {
    error(
        StatusCode::NOT_FOUND,
        "E0000007",
        &format!("Not found: Resource not found: {id} ({kind})"),
        &[],
    )
}

pub fn endpoint_not_found() -> Reply {
    error(
        StatusCode::NOT_FOUND,
        "E0000007",
        "Not found: Resource not found: (Endpoint)",
        &[],
    )
}

pub fn validation(causes: &[&str]) -> Reply {
    let fields: Vec<&str> = causes
        .iter()
        .map(|c| c.split(':').next().unwrap_or(c).trim())
        .collect();
    error(
        StatusCode::BAD_REQUEST,
        "E0000001",
        &format!("Api validation failed: {}", fields.join(", ")),
        &causes.iter().map(|c| (*c).to_owned()).collect::<Vec<_>>(),
    )
}

pub fn bad_request(code: &str, summary: &str) -> Reply {
    error(StatusCode::BAD_REQUEST, code, summary, &[])
}

/// One parsed request.
pub struct Req {
    pub method: Method,
    pub segs: Vec<String>,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub body: Value,
    pub raw_body: Vec<u8>,
}

impl Req {
    pub fn q(&self, key: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty())
    }

    pub fn q_bool(&self, key: &str, default: bool) -> bool {
        self.q(key)
            .map_or(default, |v| v.eq_ignore_ascii_case("true"))
    }

    /// The JSON body as an object, or the real 400 for unparseable input.
    pub fn object(&self) -> Result<Map<String, Value>, Reply> {
        match &self.body {
            Value::Object(m) => Ok(m.clone()),
            Value::Null if self.raw_body.is_empty() => Ok(Map::new()),
            _ => Err(error(
                StatusCode::BAD_REQUEST,
                "E0000003",
                "The request body was not well-formed.",
                &[],
            )),
        }
    }
}

/// Okta's timestamp format: millisecond precision, `Z`.
pub fn ts(world: &WorldState) -> String {
    millis(&world.clock.now_rfc3339())
}

pub fn millis(rfc3339: &str) -> String {
    match rfc3339.strip_suffix('Z') {
        Some(base) if !base.contains('.') => format!("{base}.000Z"),
        _ => rfc3339.to_owned(),
    }
}

pub fn parse_ts(s: &str) -> Option<i64> {
    OffsetDateTime::parse(s, &Rfc3339)
        .ok()
        .map(|t| t.unix_timestamp())
}

pub fn format_ts(unix: i64) -> String {
    millis(&mock_core::clock::format_unix(unix))
}

pub fn svc(world: &mut WorldState) -> &mut ServiceState {
    world.service_mut(NAME)
}

/// Documents of a collection (dataset order).
pub fn list(world: &mut WorldState, coll: &str) -> Vec<(String, Value)> {
    svc(world)
        .store
        .collection(coll)
        .map(|c| c.iter().map(|(k, v)| (k.to_owned(), v.clone())).collect())
        .unwrap_or_default()
}

pub fn get(world: &mut WorldState, coll: &str, key: &str) -> Option<Value> {
    svc(world)
        .store
        .collection(coll)
        .and_then(|c| c.get(key))
        .cloned()
}

pub fn put(world: &mut WorldState, coll: &str, key: &str, doc: Value) {
    svc(world).store.collection_mut(coll).insert(key, doc);
}

pub fn remove(world: &mut WorldState, coll: &str, key: &str) -> Option<Value> {
    svc(world).store.collection_mut(coll).remove(key)
}

/// A new Okta-style ID: `prefix` + base62 to 20 characters.
pub fn new_id(world: &mut WorldState, kind: &str, prefix: &str) -> String {
    let len = 20usize.saturating_sub(prefix.len());
    svc(world).ids.next_prefixed(kind, prefix, len)
}

pub fn href(path: &str) -> Value {
    json!({ "href": format!("{ORG_URL}{path}") })
}

pub fn post_href(path: &str) -> Value {
    json!({ "href": format!("{ORG_URL}{path}"), "hints": { "allow": ["POST"] } })
}

/// Deep-merge `patch` into `base` (objects merge, everything else replaces).
pub fn merge(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                match b.get_mut(k) {
                    Some(existing) if existing.is_object() && v.is_object() => merge(existing, v),
                    _ => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, p) => *b = p.clone(),
    }
}

/// Percent-encodes a query component.
pub fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Cursor pagination as Okta does it: `limit` per page, `after` = key of the last item
/// of the previous page, and a `Link` header with `self` and (when more remain) `next`.
pub fn page(
    req: &Req,
    items: Vec<Value>,
    default_limit: usize,
    max_limit: usize,
    key: impl Fn(&Value) -> String,
) -> Reply {
    let limit = req
        .q("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(default_limit)
        .clamp(1, max_limit);
    let start = match req.q("after") {
        Some(after) => match items.iter().position(|i| key(i) == after) {
            Some(i) => i + 1,
            None => items.len(),
        },
        None => 0,
    };
    let slice: Vec<Value> = items.iter().skip(start).take(limit).cloned().collect();
    let other: Vec<String> = req
        .query
        .iter()
        .filter(|(k, _)| k != "after" && k != "limit")
        .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
        .collect();
    let base = format!("{ORG_URL}{}", req.path);
    let with = |extra: &[String]| {
        let all: Vec<String> = other.iter().cloned().chain(extra.iter().cloned()).collect();
        if all.is_empty() {
            base.clone()
        } else {
            format!("{base}?{}", all.join("&"))
        }
    };
    let mut self_extra = Vec::new();
    if let Some(a) = req.q("after") {
        self_extra.push(format!("after={}", enc(a)));
    }
    if req.q("limit").is_some() {
        self_extra.push(format!("limit={limit}"));
    }
    let mut link = format!("<{}>; rel=\"self\"", with(&self_extra));
    if start + slice.len() < items.len()
        && let Some(last) = slice.last()
    {
        link.push_str(&format!(
            ", <{}>; rel=\"next\"",
            with(&[
                format!("after={}", enc(&key(last))),
                format!("limit={limit}")
            ])
        ));
    }
    Reply::Page(slice, link)
}

pub fn id_of(v: &Value) -> String {
    v["id"].as_str().unwrap_or_default().to_owned()
}

/// The API client the MCP server authenticates as; the actor of every event it causes.
pub fn actor() -> Value {
    json!({
        "id": "0oatoolbench",
        "type": "PublicClientApp",
        "alternateId": "0oatoolbench",
        "displayName": "Okta MCP Server",
        "detailEntry": null
    })
}

/// Appends a System Log event for a mutation made through the API.
pub fn log_event(world: &mut WorldState, event_type: &str, display: &str, targets: Vec<Value>) {
    let published = ts(world);
    let uuid = svc(world).ids.next_uuid("log").to_string();
    let txn = svc(world).ids.next_prefixed("txn", "aT", 22);
    let event = json!({
        "actor": actor(),
        "client": {
            "userAgent": { "rawUserAgent": "okta-mcp-server/0.0.1 okta-sdk-python", "os": "Linux", "browser": "UNKNOWN" },
            "zone": "null",
            "device": "Unknown",
            "id": null,
            "ipAddress": "10.0.0.20",
            "geographicalContext": null
        },
        "device": null,
        "authenticationContext": { "authenticationStep": 0, "externalSessionId": "trstoolbench" },
        "displayMessage": display,
        "eventType": event_type,
        "outcome": { "result": "SUCCESS", "reason": null },
        "published": published,
        "securityContext": {},
        "severity": "INFO",
        "debugContext": { "debugData": { "requestUri": "", "threatSuspected": "false" } },
        "legacyEventType": null,
        "transaction": { "type": "WEB", "id": txn, "detail": {} },
        "uuid": uuid,
        "version": "0",
        "request": { "ipChain": [] },
        "target": targets
    });
    put(world, "logs", &uuid, event);
}

pub fn user_target(user: &Value) -> Value {
    let p = &user["profile"];
    json!({
        "id": user["id"],
        "type": "User",
        "alternateId": p["login"],
        "displayName": format!("{} {}", p["firstName"].as_str().unwrap_or(""), p["lastName"].as_str().unwrap_or("")).trim(),
        "detailEntry": null
    })
}

pub fn group_target(group: &Value) -> Value {
    json!({
        "id": group["id"],
        "type": "UserGroup",
        "alternateId": "unknown",
        "displayName": group["profile"]["name"],
        "detailEntry": null
    })
}

pub fn app_target(app: &Value) -> Value {
    json!({
        "id": app["id"],
        "type": "AppInstance",
        "alternateId": app["label"],
        "displayName": app["label"],
        "detailEntry": { "signOnModeType": app["signOnMode"] }
    })
}
