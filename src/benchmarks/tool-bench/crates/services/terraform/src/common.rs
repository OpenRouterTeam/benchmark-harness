//! Shared plumbing: query parsing, JSON:API envelopes and errors, pagination, store access.

use axum::{
    body::Bytes,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use mock_core::{http::jsonapi, store::Store, world::WorldState};
use serde_json::{Map, Value, json};

use crate::NAME;

/// Query string and caller base URL of a request.
pub struct Ctx {
    pub q: Query,
    pub base: String,
}

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for Ctx {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self {
            q: Query::parse(parts.uri.query()),
            base: base_url(&parts.headers),
        })
    }
}

/// Decoded query string pairs in request order (`page[number]` style keys kept verbatim).
#[derive(Debug, Clone, Default)]
pub struct Query(pub Vec<(String, String)>);

impl Query {
    pub fn parse(raw: Option<&str>) -> Self {
        let pairs = raw
            .unwrap_or("")
            .split('&')
            .filter(|p| !p.is_empty())
            .map(|pair| {
                let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                (percent_decode(k), percent_decode(v))
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

    /// Non-empty value for `key`.
    pub fn value(&self, key: &str) -> Option<&str> {
        self.get(key).filter(|v| !v.is_empty())
    }

    /// Every value for `key`, also splitting comma-separated lists.
    pub fn list(&self, key: &str) -> Vec<String> {
        self.0
            .iter()
            .filter(|(k, _)| k == key)
            .flat_map(|(_, v)| v.split(','))
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect()
    }

    pub fn has_include(&self, name: &str) -> bool {
        self.list("include").iter().any(|i| i == name)
    }
}

pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b);
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

/// `scheme://host` the caller used, honouring the TLS proxy's `x-forwarded-proto`.
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

pub fn data(status: StatusCode, data: Value) -> Response {
    jsonapi(status, json!({ "data": data }))
}

pub fn data_with_included(data: Value, included: Vec<Value>) -> Response {
    let mut body = json!({ "data": data });
    if !included.is_empty() {
        body["included"] = Value::Array(included);
    }
    jsonapi(StatusCode::OK, body)
}

pub fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

pub fn error(status: StatusCode, title: &str, detail: Option<&str>) -> Response {
    let mut err = json!({ "status": status.as_u16().to_string(), "title": title });
    if let Some(detail) = detail {
        err["detail"] = json!(detail);
    }
    jsonapi(status, json!({ "errors": [err] }))
}

pub fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "not found", None)
}

pub fn unauthorized() -> Response {
    error(StatusCode::UNAUTHORIZED, "unauthorized", None)
}

pub fn conflict(title: &str, detail: &str) -> Response {
    error(StatusCode::CONFLICT, title, Some(detail))
}

/// 422 with a JSON:API source pointer, like Rails validation errors from HCP Terraform.
pub fn invalid(detail: &str, attribute: &str) -> Response {
    jsonapi(
        StatusCode::UNPROCESSABLE_ENTITY,
        json!({ "errors": [{
            "status": "422",
            "title": "invalid attribute",
            "detail": detail,
            "source": { "pointer": format!("/data/attributes/{attribute}") }
        }] }),
    )
}

pub fn bad_request(detail: &str) -> Response {
    error(StatusCode::BAD_REQUEST, "bad request", Some(detail))
}

/// `{"data": ...}` document of a request body, or a 400 response.
#[allow(clippy::result_large_err)]
pub fn body_json(body: &Bytes) -> Result<Value, Response> {
    if body.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(body).map_err(|_| bad_request("Request body is not valid JSON"))
}

pub fn attributes(doc: &Value) -> Map<String, Value> {
    doc.pointer("/data/attributes")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// `data.relationships.<name>.data.id` of a request body.
pub fn relationship_id(doc: &Value, name: &str) -> Option<String> {
    doc.pointer(&format!("/data/relationships/{name}/data/id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// IDs of a `{"data": [{"type": ..., "id": ...}, ...]}` relationship body.
pub fn data_ids(doc: &Value) -> Vec<String> {
    doc.get("data")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|i| i.get("id").and_then(Value::as_str))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

pub fn rel(kind: &str, id: Option<&str>) -> Value {
    match id {
        Some(id) if !id.is_empty() => json!({ "data": { "id": id, "type": kind } }),
        _ => json!({ "data": null }),
    }
}

pub fn rel_many<'a>(kind: &str, ids: impl IntoIterator<Item = &'a str>) -> Value {
    let data: Vec<Value> = ids
        .into_iter()
        .map(|id| json!({ "id": id, "type": kind }))
        .collect();
    json!({ "data": data })
}

pub fn str_field<'a>(doc: &'a Value, key: &str) -> &'a str {
    doc.get(key).and_then(Value::as_str).unwrap_or("")
}

pub fn opt_str<'a>(doc: &'a Value, key: &str) -> Option<&'a str> {
    doc.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

pub fn str_list(doc: &Value, key: &str) -> Vec<String> {
    doc.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// `defaults` overlaid with the document's fields, minus `skip` (relations and internal keys).
pub fn merge_attributes(defaults: Value, doc: &Value, skip: &[&str]) -> Value {
    let mut out = defaults.as_object().cloned().unwrap_or_default();
    if let Some(obj) = doc.as_object() {
        for (k, v) in obj {
            if k == "id" || skip.contains(&k.as_str()) {
                continue;
            }
            out.insert(k.clone(), v.clone());
        }
    }
    Value::Object(out)
}

/// Read-only view of the terraform service's store (empty when the scenario has no dataset).
pub struct Db<'a> {
    store: Option<&'a Store>,
}

impl<'a> Db<'a> {
    pub fn new(world: &'a WorldState) -> Self {
        Self {
            store: world.service(NAME).map(|s| &s.store),
        }
    }

    pub fn get(&self, collection: &str, key: &str) -> Option<&'a Value> {
        self.store?.collection(collection)?.get(key)
    }

    pub fn all(&self, collection: &str) -> Vec<&'a Value> {
        self.store
            .and_then(|s| s.collection(collection))
            .map(|c| c.values().collect())
            .unwrap_or_default()
    }

    pub fn has_collection(&self, collection: &str) -> bool {
        self.store.and_then(|s| s.collection(collection)).is_some()
    }

    pub fn filter(&self, collection: &str, f: impl Fn(&Value) -> bool) -> Vec<&'a Value> {
        self.all(collection).into_iter().filter(|d| f(d)).collect()
    }

    pub fn org(&self, name: &str) -> Option<&'a Value> {
        self.get("organizations", name).or_else(|| {
            self.all("organizations")
                .into_iter()
                .find(|o| str_field(o, "name").eq_ignore_ascii_case(name))
        })
    }

    /// Workspace by organization and name (names are case-insensitive in HCP Terraform).
    pub fn workspace_by_name(&self, org: &str, name: &str) -> Option<&'a Value> {
        self.all("workspaces").into_iter().find(|w| {
            str_field(w, "organization").eq_ignore_ascii_case(org)
                && str_field(w, "name").eq_ignore_ascii_case(name)
        })
    }

    /// The user the API token belongs to.
    pub fn current_user(&self) -> Option<&'a Value> {
        let users = self.all("users");
        users
            .iter()
            .copied()
            .find(|u| u.get("current").and_then(Value::as_bool) == Some(true))
            .or_else(|| users.first().copied())
    }
}

/// Page number and size from `page[number]` / `page[size]` (HCP Terraform default 20, max 100).
pub fn page_params(q: &Query) -> (usize, usize) {
    let number = q
        .get("page[number]")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    let size = q
        .get("page[size]")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(20)
        .clamp(1, 100);
    (number, size)
}

/// A paginated JSON:API list document with `links` and `meta.pagination`.
pub fn list_response(
    base: &str,
    path: &str,
    q: &Query,
    items: Vec<Value>,
    included: Vec<Value>,
) -> Response {
    let (number, size) = page_params(q);
    let total = items.len();
    let total_pages = total.div_ceil(size).max(1);
    let page: Vec<Value> = items
        .into_iter()
        .skip((number - 1) * size)
        .take(size)
        .collect();
    let others: Vec<String> =
        q.0.iter()
            .filter(|(k, _)| k != "page[number]" && k != "page[size]")
            .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
            .collect();
    let link = |n: usize| {
        let mut parts = vec![
            format!("page%5Bnumber%5D={n}"),
            format!("page%5Bsize%5D={size}"),
        ];
        parts.extend(others.iter().cloned());
        format!("{base}{path}?{}", parts.join("&"))
    };
    let prev = (number > 1).then(|| number - 1);
    let next = (number < total_pages).then(|| number + 1);
    let mut body = json!({
        "data": page,
        "links": {
            "self": link(number),
            "first": link(1),
            "prev": prev.map(link),
            "next": next.map(link),
            "last": link(total_pages),
        },
        "meta": { "pagination": {
            "current-page": number,
            "page-size": size,
            "prev-page": prev,
            "next-page": next,
            "total-pages": total_pages,
            "total-count": total,
        } },
    });
    if !included.is_empty() {
        body["included"] = Value::Array(dedup(included));
    }
    jsonapi(StatusCode::OK, body)
}

/// Included resources without duplicates (same type and id), first occurrence wins.
pub fn dedup(included: Vec<Value>) -> Vec<Value> {
    let mut seen = std::collections::BTreeSet::new();
    included
        .into_iter()
        .filter(|v| seen.insert(format!("{}/{}", str_field(v, "type"), str_field(v, "id"))))
        .collect()
}

pub fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn redirect(location: &str) -> Response {
    let mut resp = StatusCode::TEMPORARY_REDIRECT.into_response();
    if let Ok(v) = HeaderValue::from_str(location) {
        resp.headers_mut().insert(header::LOCATION, v);
    }
    resp
}

/// First 40 hex characters of SHA-256, used for version IDs and checksums.
pub fn hex_digest(input: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(input.as_bytes());
    digest.iter().take(20).map(|b| format!("{b:02x}")).collect()
}

pub fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// Next free ID for `prefix` that does not collide with documents already in `collection`.
pub fn new_id(world: &mut WorldState, collection: &str, kind: &str, prefix: &str) -> String {
    loop {
        let service = world.service_mut(NAME);
        let id = service.ids.next_prefixed(kind, prefix, 16);
        if !service
            .store
            .collection(collection)
            .is_some_and(|c| c.contains(&id))
        {
            return id;
        }
    }
}
