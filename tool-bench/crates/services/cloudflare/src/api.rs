//! The mock Cloudflare REST API (`/client/v4`), shared by the HTTP listener and the
//! in-process `cloudflare.request()` binding of the `execute` tool.

use mock_core::world::WorldState;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::{NAME, accounts, d1, kv, r2, workers, zones};

/// A request below the API base, e.g. `GET /zones/<id>/dns_records?type=A`.
#[derive(Debug, Clone)]
pub struct ApiRequest {
    pub method: String,
    /// Path below `/client/v4`, starting with `/`, without the query string.
    pub path: String,
    pub query: Vec<(String, String)>,
    pub content_type: Option<String>,
    pub body: Body,
}

#[derive(Debug, Clone)]
pub enum Body {
    None,
    Bytes(Vec<u8>),
    Form(Vec<Part>),
}

/// One `multipart/form-data` part.
#[derive(Debug, Clone)]
pub struct Part {
    pub name: String,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub bytes: Vec<u8>,
}

impl Part {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

#[derive(Debug, Clone)]
pub struct ApiResponse {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

impl ApiResponse {
    pub fn json(status: u16, value: &Value) -> Self {
        Self {
            status,
            content_type: "application/json".to_owned(),
            body: value.to_string().into_bytes(),
        }
    }

    pub fn raw(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            content_type: content_type.to_owned(),
            body: body.into(),
        }
    }

    pub fn json_value(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

pub type ApiResult = Result<ApiResponse, ApiResponse>;

impl ApiRequest {
    /// Splits `path?query` and lets `extra` override (as `URL.searchParams.set` does).
    pub fn new(method: &str, path_and_query: &str, extra: Vec<(String, String)>) -> Self {
        let (path, query) = path_and_query
            .split_once('?')
            .unwrap_or((path_and_query, ""));
        let mut pairs = parse_query(query);
        for (k, v) in extra {
            pairs.retain(|(key, _)| *key != k);
            pairs.push((k, v));
        }
        Self {
            method: method.to_ascii_uppercase(),
            path: path.to_owned(),
            query: pairs,
            content_type: None,
            body: Body::None,
        }
    }

    pub fn q(&self, key: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn query_string(&self) -> Option<String> {
        (!self.query.is_empty()).then(|| {
            self.query
                .iter()
                .map(|(k, v)| format!("{}={}", encode_component(k), encode_component(v)))
                .collect::<Vec<_>>()
                .join("&")
        })
    }

    pub fn media_type(&self) -> String {
        self.content_type
            .as_deref()
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase()
    }

    pub fn bytes(&self) -> &[u8] {
        match &self.body {
            Body::Bytes(b) => b,
            _ => &[],
        }
    }

    /// The JSON body, or Cloudflare's malformed-JSON error.
    pub fn json(&self) -> Result<Value, ApiResponse> {
        match &self.body {
            Body::Bytes(b) if !b.is_empty() => serde_json::from_slice(b)
                .map_err(|_| fail(400, &[(6007, "Malformed JSON in request body")])),
            Body::Form(parts) => {
                let mut obj = Map::new();
                for p in parts {
                    let text = p.text();
                    let v = serde_json::from_str(&text).unwrap_or(Value::String(text));
                    obj.insert(p.name.clone(), v);
                }
                Ok(Value::Object(obj))
            }
            _ => Err(fail(400, &[(6007, "Malformed JSON in request body")])),
        }
    }

    pub fn json_object(&self) -> Result<Map<String, Value>, ApiResponse> {
        match self.json()? {
            Value::Object(o) => Ok(o),
            _ => Err(fail(400, &[(6007, "Malformed JSON in request body")])),
        }
    }

    /// The request body as the journal records it.
    pub fn journal_body(&self) -> Value {
        match &self.body {
            Body::None => Value::Null,
            Body::Bytes(b) => mock_core::journal::body_value(b),
            Body::Form(parts) => json!({
                "multipart": parts.iter().map(|p| json!({
                    "name": p.name,
                    "filename": p.filename,
                    "content_type": p.content_type,
                    "text": p.text(),
                })).collect::<Vec<_>>()
            }),
        }
    }
}

pub fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (decode_component(k), decode_component(v))
        })
        .collect()
}

pub fn decode_component(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                match std::str::from_utf8(&bytes[i + 1..i + 3])
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                    .ok_or(())
                {
                    Ok(b) => {
                        out.push(b);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Percent-decoding for path segments (`+` stays a plus).
pub fn decode_path_segment(s: &str) -> String {
    decode_component(&s.replace('+', "%2B"))
}

pub fn encode_component(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// ── Envelopes ────────────────────────────────────────────────────────────────

pub fn ok(result: Value) -> ApiResult {
    Ok(ApiResponse::json(
        200,
        &json!({ "result": result, "success": true, "errors": [], "messages": [] }),
    ))
}

pub fn ok_with_info(result: Value, info: Value) -> ApiResult {
    Ok(ApiResponse::json(
        200,
        &json!({ "result": result, "success": true, "errors": [], "messages": [], "result_info": info }),
    ))
}

pub fn fail(status: u16, errors: &[(u32, &str)]) -> ApiResponse {
    let errors: Vec<Value> = errors
        .iter()
        .map(|(code, message)| json!({ "code": code, "message": message }))
        .collect();
    ApiResponse::json(
        status,
        &json!({ "result": null, "success": false, "errors": errors, "messages": [] }),
    )
}

pub fn fail_chain(status: u16, code: u32, message: &str, chain: &[(u32, &str)]) -> ApiResponse {
    let chain: Vec<Value> = chain
        .iter()
        .map(|(c, m)| json!({ "code": c, "message": m }))
        .collect();
    ApiResponse::json(
        status,
        &json!({
            "result": null,
            "success": false,
            "errors": [{ "code": code, "message": message, "error_chain": chain }],
            "messages": []
        }),
    )
}

/// Cloudflare's answer for a path it can't route (including unknown identifiers).
pub fn no_route(path: &str) -> ApiResponse {
    fail(
        400,
        &[
            (
                7003,
                &format!("Could not route to {path}, perhaps your object identifier is invalid?"),
            ),
            (7000, "No route for that URI"),
        ],
    )
}

pub fn method_not_allowed() -> ApiResponse {
    fail(405, &[(10000, "Method not allowed")])
}

/// `page`/`per_page` paging with Cloudflare's `result_info`.
pub fn paged(
    req: &ApiRequest,
    items: Vec<Value>,
    default_per_page: usize,
    max: usize,
) -> ApiResult {
    let page = req
        .q("page")
        .and_then(|p| p.parse::<f64>().ok())
        .map_or(1, |p| p.max(1.0) as usize);
    let per_page = req
        .q("per_page")
        .and_then(|p| p.parse::<f64>().ok())
        .map_or(default_per_page, |p| (p.max(1.0) as usize).min(max));
    let total = items.len();
    let slice: Vec<Value> = items
        .into_iter()
        .skip((page - 1) * per_page)
        .take(per_page)
        .collect();
    let info = json!({
        "page": page,
        "per_page": per_page,
        "count": slice.len(),
        "total_count": total,
        "total_pages": total.div_ceil(per_page).max(1),
    });
    ok_with_info(Value::Array(slice), info)
}

/// Orders by `order` (a field name) and `direction` when given.
pub fn sort_by_query(req: &ApiRequest, items: &mut [Value], allowed: &[&str]) {
    if let Some(order) = req.q("order").filter(|o| allowed.contains(o)) {
        items.sort_by(|a, b| {
            sort_key(pointer_dotted(a, order)).cmp(&sort_key(pointer_dotted(b, order)))
        });
    }
    if req.q("direction") == Some("desc") {
        items.reverse();
    }
}

fn pointer_dotted<'a>(v: &'a Value, dotted: &str) -> &'a Value {
    let mut cur = v;
    for part in dotted.split('.') {
        cur = cur.get(part).unwrap_or(&Value::Null);
    }
    cur
}

fn sort_key(v: &Value) -> String {
    match v {
        Value::String(s) => s.to_lowercase(),
        Value::Number(n) => format!("{:>20}", n.as_f64().unwrap_or(0.0) as i64),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

// ── Store access ─────────────────────────────────────────────────────────────

pub fn all(w: &WorldState, collection: &str) -> Vec<(String, Value)> {
    w.service(NAME)
        .and_then(|s| s.store.collection(collection))
        .map(|c| c.iter().map(|(k, v)| (k.to_owned(), v.clone())).collect())
        .unwrap_or_default()
}

pub fn values(w: &WorldState, collection: &str) -> Vec<Value> {
    all(w, collection).into_iter().map(|(_, v)| v).collect()
}

pub fn get(w: &WorldState, collection: &str, key: &str) -> Option<Value> {
    w.service(NAME)
        .and_then(|s| s.store.collection(collection))
        .and_then(|c| c.get(key))
        .cloned()
}

pub fn put(w: &mut WorldState, collection: &str, key: &str, doc: Value) {
    w.service_mut(NAME)
        .store
        .collection_mut(collection)
        .insert(key.to_owned(), doc);
}

pub fn remove(w: &mut WorldState, collection: &str, key: &str) -> Option<Value> {
    w.service_mut(NAME)
        .store
        .collection_mut(collection)
        .remove(key)
}

/// Deterministic 32-hex identifier from the scenario seed and `parts`.
pub fn hex_id(w: &WorldState, parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(w.meta.seed.as_bytes());
    for p in parts {
        hasher.update([0]);
        hasher.update(p.as_bytes());
    }
    hasher
        .finalize()
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Deterministic UUID (8-4-4-4-12) from the scenario seed and `parts`.
pub fn uuid_id(w: &WorldState, parts: &[&str]) -> String {
    let h = hex_id(w, parts);
    format!(
        "{}-{}-4{}-a{}-{}",
        &h[0..8],
        &h[8..12],
        &h[13..16],
        &h[17..20],
        &h[20..32]
    )
}

/// Logical time with Cloudflare's microsecond precision, e.g. `2026-03-14T09:00:00.000000Z`.
pub fn now(w: &WorldState) -> String {
    let t = w.clock.now_rfc3339();
    format!("{}.000000Z", t.trim_end_matches('Z'))
}

pub fn is_hex32(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn str_field<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The session's accounts (the dataset's `accounts`).
pub fn session_accounts(w: &WorldState) -> Vec<Value> {
    values(w, "accounts")
}

/// The account for an account-scoped path, or Cloudflare's refusal.
pub fn account(w: &WorldState, req: &ApiRequest, id: &str) -> Result<Value, ApiResponse> {
    if !is_hex32(id) {
        return Err(no_route(&req.path));
    }
    get(w, "accounts", id)
        .ok_or_else(|| fail(403, &[(9109, "Unauthorized to access requested resource")]))
}

// ── Dispatch ─────────────────────────────────────────────────────────────────

/// Answers one API request against the world.
pub fn handle(w: &mut WorldState, req: &ApiRequest) -> ApiResponse {
    let segments: Vec<String> = req
        .path
        .trim_end_matches('/')
        .split('/')
        .skip(1)
        .map(decode_path_segment)
        .collect();
    let segs: Vec<&str> = segments.iter().map(String::as_str).collect();
    if !req.path.starts_with('/') || segs.is_empty() {
        return fail(400, &[(7000, "No route for that URI")]);
    }
    let result = match segs[0] {
        "user" | "memberships" => accounts::route(w, req, &segs),
        "zones" => zones::route(w, req, &segs),
        "accounts" => match segs.get(2).copied() {
            None | Some("members" | "tokens") => accounts::route(w, req, &segs),
            Some("workers") => workers::route(w, req, &segs),
            Some("storage") => kv::route(w, req, &segs),
            Some("r2") => r2::route(w, req, &segs),
            Some("d1") => d1::route(w, req, &segs),
            Some(_) => match account(w, req, segs[1]) {
                Err(e) => Err(e),
                Ok(_) => Err(fail(400, &[(7000, "No route for that URI")])),
            },
        },
        _ => Err(fail(400, &[(7000, "No route for that URI")])),
    };
    result.unwrap_or_else(|e| e)
}

/// The mutating methods, which advance the logical clock.
pub fn is_mutating(method: &str) -> bool {
    !matches!(method, "GET" | "HEAD" | "OPTIONS")
}
