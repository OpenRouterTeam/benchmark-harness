//! Shared plumbing for the GitLab mock: request extraction, GitLab-style errors and
//! pagination, lookups, and renderers for the resources every module touches.
//!
//! # Data model (dataset collections under `dataset/gitlab/`)
//!
//! Documents are stored compactly and rendered into full GitLab API shapes on read.
//! People are referenced by username, projects/groups by full path.
//!
//! | collection | key | notes |
//! |---|---|---|
//! | `instance` | `"instance"` | optional: `current_user` (username), `version`, `revision` |
//! | `users` | username | `id, username, name, email, state, bio, location, job_title, organization, created_at, is_admin, bot` |
//! | `groups` | full path (`acme/platform`) | `id, name, full_path, description, visibility, created_at, members: [{username, access_level}]` |
//! | `projects` | path with namespace (`acme/platform/api`) | `id, name, path_with_namespace, description, default_branch, visibility, created_at, last_activity_at, topics, archived, forked_from, merge_method, members` |
//! | `labels` | `<project-or-group path>~<name>` | `id, name, color, description, priority` |
//! | `milestones` | `<project-or-group path>%<iid>` | `id, iid, title, description, state (active/closed), due_date, start_date, created_at, updated_at` |
//! | `issues` | `<project path>#<iid>` | `id, iid, title, description, state (opened/closed), author, assignees, labels (names), milestone_id, created_at, updated_at, closed_at, closed_by, confidential, due_date, weight, issue_type, discussion_locked` |
//! | `merge_requests` | `<project path>!<iid>` | `id, iid, title, description, state, source_branch, target_branch, author, assignees, reviewers, labels, milestone_id, draft, sha, approvals_required, approved_by, changes, commits, ...` |
//! | `notes` | note id | `id, noteable (issue/MR key), body, author, created_at, updated_at, system, internal, discussion_id, type, resolvable, resolved, resolved_by, position` |
//! | `branches` | `<project path>:<branch>` | `name, commit (sha), base (branch whose files it inherits)` |
//! | `files` | `<project path>:<branch>:<file path>` | `content, deleted` (overlay over the branch's `base` chain) |
//! | `commits` | `<project path>@<sha>` | `id, title, message, author_name, author_email, authored_date, committed_date, parent_ids, branch, diffs` |
//! | `tags` | `<project path>:<tag>` | `name, commit (sha), message, created_at` |
//! | `pipelines`, `jobs`, ... | id | carry a `project` field with the project path |

use std::collections::HashMap;

use axum::{
    body::{Bytes, to_bytes},
    extract::{FromRequest, FromRequestParts, Path, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::Response,
};
use mock_core::{
    store::Store,
    world::{SharedWorld, WorldState},
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::NAME;

/// A synchronous handler over the locked world.
pub type H = fn(&mut WorldState, &Req) -> Response;

/// Turns a sync handler into an axum handler (`rget(wrap(f))`).
pub fn wrap(
    f: H,
) -> impl Fn(State<SharedWorld>, Req) -> std::future::Ready<Response> + Clone + Send + Sync + 'static
{
    move |State(world): State<SharedWorld>, req: Req| {
        let mut guard = world.lock().expect("world lock");
        std::future::ready(f(&mut guard, &req))
    }
}

/// Everything a handler needs from the HTTP request.
#[derive(Debug, Clone)]
pub struct Req {
    pub method: Method,
    /// Percent-decoded path parameters.
    pub path: HashMap<String, String>,
    /// Decoded query pairs in order (repeated keys kept, e.g. `labels[]`).
    pub query: Vec<(String, String)>,
    /// JSON (or form-decoded) body; `Null` when absent or not JSON.
    pub body: Value,
    pub raw_body: Bytes,
    pub headers: HeaderMap,
    /// The raw request path (still percent-encoded), e.g. `/api/v4/projects/acme%2Fapi`.
    pub uri_path: String,
    /// `http://host:port` of the request, for `web_url`s.
    pub base: String,
}

impl FromRequest<SharedWorld> for Req {
    type Rejection = Response;

    async fn from_request(req: Request, state: &SharedWorld) -> Result<Self, Self::Rejection> {
        let (mut parts, body) = req.into_parts();
        let path = Path::<HashMap<String, String>>::from_request_parts(&mut parts, state)
            .await
            .map(|p| p.0)
            .unwrap_or_default();
        let query = parts.uri.query().map(parse_query).unwrap_or_default();
        let raw_body = to_bytes(body, 64 * 1024 * 1024).await.map_err(|_| {
            error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "413 Request Entity Too Large",
            )
        })?;
        let content_type = parts
            .headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let body = if raw_body.is_empty() {
            Value::Null
        } else if let Ok(v) = serde_json::from_slice::<Value>(&raw_body) {
            v
        } else if content_type.starts_with("application/x-www-form-urlencoded") {
            let mut map = Map::new();
            for (k, v) in parse_query(&String::from_utf8_lossy(&raw_body)) {
                map.insert(k, Value::String(v));
            }
            Value::Object(map)
        } else {
            Value::Null
        };
        let scheme = parts
            .headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("http");
        let host = parts
            .headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("localhost");
        Ok(Self {
            method: parts.method.clone(),
            path,
            query,
            body,
            raw_body,
            base: format!("{scheme}://{host}"),
            uri_path: parts.uri.path().to_owned(),
            headers: parts.headers,
        })
    }
}

impl Req {
    /// A path parameter (already percent-decoded).
    pub fn p(&self, name: &str) -> &str {
        self.path.get(name).map_or("", String::as_str)
    }

    /// First query value for `key`.
    pub fn q(&self, key: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty())
    }

    /// A parameter from the JSON body, falling back to the query string.
    pub fn arg(&self, key: &str) -> Option<Value> {
        if let Some(v) = self.body.get(key).filter(|v| !v.is_null()) {
            return Some(v.clone());
        }
        self.q(key).map(|s| Value::String(s.to_owned()))
    }

    /// Parameter as a string (numbers and booleans stringified).
    pub fn s(&self, key: &str) -> Option<String> {
        self.arg(key).and_then(|v| match v {
            Value::String(s) => Some(s),
            Value::Number(n) => Some(n.to_string()),
            Value::Bool(b) => Some(b.to_string()),
            _ => None,
        })
    }

    pub fn has(&self, key: &str) -> bool {
        self.body.get(key).is_some() || self.query.iter().any(|(k, _)| k == key)
    }

    pub fn bool(&self, key: &str) -> Option<bool> {
        self.arg(key).and_then(|v| match v {
            Value::Bool(b) => Some(b),
            Value::Number(n) => Some(n.as_i64() != Some(0)),
            Value::String(s) => match s.as_str() {
                "true" | "1" | "yes" => Some(true),
                "false" | "0" | "no" => Some(false),
                _ => None,
            },
            _ => None,
        })
    }

    pub fn int(&self, key: &str) -> Option<i64> {
        self.arg(key).and_then(|v| match v {
            Value::Number(n) => n.as_i64(),
            Value::String(s) => s.trim().parse().ok(),
            _ => None,
        })
    }

    /// List parameter: JSON array, comma-separated string, or repeated `key[]` query values.
    pub fn list(&self, key: &str) -> Option<Vec<String>> {
        if let Some(v) = self.body.get(key) {
            return Some(value_list(v));
        }
        let bracket = format!("{key}[]");
        let repeated: Vec<String> = self
            .query
            .iter()
            .filter(|(k, _)| *k == bracket)
            .map(|(_, v)| v.clone())
            .collect();
        if !repeated.is_empty() {
            return Some(repeated);
        }
        self.query
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| split_csv(v))
    }
}

pub fn value_list(v: &Value) -> Vec<String> {
    match v {
        Value::Array(items) => items
            .iter()
            .filter_map(|i| match i {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .collect(),
        Value::String(s) => split_csv(s),
        Value::Number(n) => vec![n.to_string()],
        _ => Vec::new(),
    }
}

fn split_csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

pub fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(k), percent_decode(v))
        })
        .collect()
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                match std::str::from_utf8(&bytes[i + 1..i + 3])
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                    }
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn percent_encode(s: &str) -> String {
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

// ---------------------------------------------------------------------------
// Responses

pub fn ok(body: Value) -> Response {
    mock_core::http::json(StatusCode::OK, body)
}

pub fn created(body: Value) -> Response {
    mock_core::http::json(StatusCode::CREATED, body)
}

pub fn accepted(body: Value) -> Response {
    mock_core::http::json(StatusCode::ACCEPTED, body)
}

/// `204 No Content` (GitLab's answer to most DELETEs).
pub fn no_content() -> Response {
    let mut resp = Response::new(axum::body::Body::empty());
    *resp.status_mut() = StatusCode::NO_CONTENT;
    resp
}

pub fn json_status(status: StatusCode, body: Value) -> Response {
    mock_core::http::json(status, body)
}

/// `{"message": msg}` with the given status, GitLab's usual error envelope.
pub fn error(status: StatusCode, msg: &str) -> Response {
    mock_core::http::json(status, json!({ "message": msg }))
}

/// `404 <What> Not Found`, e.g. `not_found("Project")`.
pub fn not_found(what: &str) -> Response {
    error(StatusCode::NOT_FOUND, &format!("404 {what} Not Found"))
}

pub fn project_not_found() -> Response {
    not_found("Project")
}

/// Unknown route: GitLab answers `{"error":"404 Not Found"}`.
pub fn route_not_found() -> Response {
    mock_core::http::json(StatusCode::NOT_FOUND, json!({ "error": "404 Not Found" }))
}

/// Missing required parameter: `400 {"error":"<param> is missing"}`.
pub fn missing(param: &str) -> Response {
    mock_core::http::json(
        StatusCode::BAD_REQUEST,
        json!({ "error": format!("{param} is missing") }),
    )
}

/// `400 {"error": msg}`.
pub fn bad_request(msg: &str) -> Response {
    mock_core::http::json(StatusCode::BAD_REQUEST, json!({ "error": msg }))
}

/// Model validation failure: `400 {"message": {"<field>": ["<msg>"]}}`.
pub fn invalid(field: &str, msg: &str) -> Response {
    mock_core::http::json(
        StatusCode::BAD_REQUEST,
        json!({ "message": { field: [msg] } }),
    )
}

pub fn forbidden() -> Response {
    error(StatusCode::FORBIDDEN, "403 Forbidden")
}

/// Plain-text response (job traces, raw files).
pub fn text(status: StatusCode, content_type: &'static str, body: String) -> Response {
    let mut resp = Response::new(axum::body::Body::from(body));
    *resp.status_mut() = status;
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    resp
}

/// Offset pagination with GitLab's headers (`x-page`, `x-per-page`, `x-total`,
/// `x-total-pages`, `x-next-page`, `x-prev-page`, `Link`). Default 20 per page, max 100.
pub fn paged(r: &Req, items: Vec<Value>) -> Response {
    let page = r
        .q("page")
        .and_then(|p| p.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    let per_page = r
        .q("per_page")
        .and_then(|p| p.parse::<usize>().ok())
        .unwrap_or(20)
        .clamp(1, 100);
    let total = items.len();
    let total_pages = total.div_ceil(per_page).max(1);
    let slice: Vec<Value> = items
        .into_iter()
        .skip((page - 1) * per_page)
        .take(per_page)
        .collect();
    let mut resp = ok(Value::Array(slice));
    let headers = resp.headers_mut();
    let set = |h: &mut HeaderMap, k: &'static str, v: String| {
        h.insert(k, HeaderValue::from_str(&v).expect("ascii header"));
    };
    set(headers, "x-page", page.to_string());
    set(headers, "x-per-page", per_page.to_string());
    set(headers, "x-total", total.to_string());
    set(headers, "x-total-pages", total_pages.to_string());
    set(
        headers,
        "x-next-page",
        if page < total_pages {
            (page + 1).to_string()
        } else {
            String::new()
        },
    );
    set(
        headers,
        "x-prev-page",
        if page > 1 {
            (page - 1).to_string()
        } else {
            String::new()
        },
    );
    // Link header over the request's own path and filters.
    let filters: Vec<String> = r
        .query
        .iter()
        .filter(|(k, _)| k != "page" && k != "per_page")
        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
        .collect();
    let mut prefix = format!("{}{}?", r.base, r.uri_path);
    for f in &filters {
        prefix.push_str(f);
        prefix.push('&');
    }
    let link =
        |p: usize, rel: &str| format!("<{prefix}page={p}&per_page={per_page}>; rel=\"{rel}\"");
    let mut rels = Vec::new();
    if page > 1 {
        rels.push(link(page - 1, "prev"));
    }
    if page < total_pages {
        rels.push(link(page + 1, "next"));
    }
    rels.push(link(1, "first"));
    rels.push(link(total_pages, "last"));
    set(headers, "link", rels.join(", "));
    resp
}

// ---------------------------------------------------------------------------
// Time, ids, hashes

/// Logical "now" in GitLab's format (`2026-03-14T09:00:07.000Z`).
pub fn now(w: &WorldState) -> String {
    gitlab_time(&w.clock.now_rfc3339())
}

/// `2026-03-14T09:00:07Z` -> `2026-03-14T09:00:07.000Z` (other strings unchanged).
pub fn gitlab_time(rfc3339: &str) -> String {
    match rfc3339.strip_suffix('Z') {
        Some(head) if !head.contains('.') => format!("{head}.000Z"),
        _ => rfc3339.to_owned(),
    }
}

/// Date part (`YYYY-MM-DD`) of the logical now.
pub fn today(w: &WorldState) -> String {
    w.clock.now_rfc3339()[..10].to_owned()
}

/// Next numeric id for a collection: max(`id`, highest id ever removed) + 1, at least
/// `floor` (GitLab never reuses ids).
pub fn next_id(store: &Store, collection: &str, floor: u64) -> u64 {
    store
        .collection(collection)
        .map(|c| {
            c.values()
                .filter_map(|v| as_u64(&v["id"]))
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0)
        .max(high_water(store, collection))
        .max(floor.saturating_sub(1))
        + 1
}

fn high_water(store: &Store, name: &str) -> u64 {
    get(store, SEQUENCES, "removed")
        .and_then(|s| as_u64(&s[name]))
        .unwrap_or(0)
}

/// Bookkeeping collection recording the highest removed id/iid per collection.
pub const SEQUENCES: &str = "_sequences";

/// Removes a document, remembering its `id` (and `iid`) so they are never handed out again.
pub fn remove_doc(store: &mut Store, collection: &str, key: &str) -> Option<Value> {
    let doc = store.collection_mut(collection).remove(key)?;
    let mut marks = Vec::new();
    if let Some(id) = as_u64(&doc["id"]) {
        marks.push((collection.to_owned(), id));
    }
    if let Some(iid) = as_u64(&doc["iid"]) {
        marks.push((format!("{collection}:{}", project_of(key)), iid));
    }
    if !marks.is_empty() {
        let seq = store.collection_mut(SEQUENCES);
        if seq.get("removed").is_none() {
            seq.insert("removed", json!({}));
        }
        let entry = seq.get_mut("removed").expect("removed");
        for (k, v) in marks {
            if as_u64(&entry[&k]).unwrap_or(0) < v {
                entry[&k] = json!(v);
            }
        }
    }
    Some(doc)
}

/// Whether an id field (number or string) equals `id`.
pub fn id_is(v: &Value, id: &str) -> bool {
    match v {
        Value::Number(n) => n.to_string() == id,
        Value::String(s) => s == id,
        _ => false,
    }
}

pub fn as_u64(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// Deterministic 40-hex "git sha" derived from `seed`.
pub fn fake_sha(seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    digest.iter().take(20).map(|b| format!("{b:02x}")).collect()
}

pub fn short_sha(sha: &str) -> String {
    sha.chars().take(8).collect()
}

// ---------------------------------------------------------------------------
// Store access

pub fn svc(w: &WorldState) -> Option<&Store> {
    w.service(NAME).map(|s| &s.store)
}

pub fn svc_mut(w: &mut WorldState) -> &mut Store {
    &mut w.service_mut(NAME).store
}

/// Empty store used when the scenario has no GitLab dataset.
pub fn empty_store() -> &'static Store {
    static EMPTY: std::sync::OnceLock<Store> = std::sync::OnceLock::new();
    EMPTY.get_or_init(Store::default)
}

pub fn store(w: &WorldState) -> &Store {
    svc(w).unwrap_or_else(|| empty_store())
}

pub fn get<'a>(store: &'a Store, collection: &str, key: &str) -> Option<&'a Value> {
    store.collection(collection).and_then(|c| c.get(key))
}

/// All `(key, doc)` pairs of a collection in insertion order.
pub fn all<'a>(store: &'a Store, collection: &str) -> Vec<(&'a str, &'a Value)> {
    store
        .collection(collection)
        .map(|c| c.iter().collect())
        .unwrap_or_default()
}

pub fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

// ---------------------------------------------------------------------------
// Instance, users

pub const DEFAULT_VERSION: &str = "18.4.1-ee";

pub fn instance_field<'a>(store: &'a Store, field: &str) -> Option<&'a str> {
    get(store, "instance", "instance").and_then(|i| i.get(field)?.as_str())
}

/// Username of the token owner: `instance.current_user`, else the first user.
pub fn current_username(store: &Store) -> String {
    instance_field(store, "current_user")
        .map(str::to_owned)
        .or_else(|| all(store, "users").first().map(|(k, _)| (*k).to_owned()))
        .unwrap_or_else(|| "root".to_owned())
}

pub fn user<'a>(store: &'a Store, username: &str) -> Option<&'a Value> {
    get(store, "users", username)
}

pub fn user_by_id<'a>(store: &'a Store, id: &str) -> Option<(&'a str, &'a Value)> {
    all(store, "users")
        .into_iter()
        .find(|(_, u)| as_u64(&u["id"]).map(|n| n.to_string()).as_deref() == Some(id))
}

/// Username for an id or username string.
pub fn resolve_username(store: &Store, id_or_name: &str) -> Option<String> {
    if user(store, id_or_name).is_some() {
        return Some(id_or_name.to_owned());
    }
    user_by_id(store, id_or_name).map(|(k, _)| k.to_owned())
}

pub fn avatar_url(base: &str, username: &str) -> String {
    let _ = base;
    format!(
        "https://secure.gravatar.com/avatar/{}?s=80&d=identicon",
        &fake_sha(username)[..32]
    )
}

/// GitLab's basic user (`UserBasic`): id, username, name, state, locked, avatar_url, web_url.
/// Unknown usernames render as a ghost-like user so documents never break.
pub fn user_basic(store: &Store, base: &str, username: &str) -> Value {
    let u = user(store, username);
    let id = u.and_then(|u| as_u64(&u["id"])).unwrap_or(0);
    let name = u
        .and_then(|u| u.get("name")?.as_str())
        .unwrap_or(username)
        .to_owned();
    let state = u
        .and_then(|u| u.get("state")?.as_str())
        .unwrap_or("active")
        .to_owned();
    json!({
        "id": id,
        "username": username,
        "name": name,
        "state": state,
        "locked": false,
        "avatar_url": avatar_url(base, username),
        "web_url": format!("{base}/{username}"),
    })
}

/// `user_basic` for an optional username field (`null` when absent).
pub fn user_opt(store: &Store, base: &str, v: &Value) -> Value {
    match v.as_str() {
        Some(u) if !u.is_empty() => user_basic(store, base, u),
        _ => Value::Null,
    }
}

pub fn users_list(store: &Store, base: &str, v: &Value) -> Value {
    Value::Array(
        value_list(v)
            .iter()
            .map(|u| user_basic(store, base, u))
            .collect(),
    )
}

/// Full user (`/users/:id`, `/user`).
pub fn user_full(store: &Store, base: &str, username: &str) -> Value {
    let mut out = user_basic(store, base, username);
    let u = user(store, username).cloned().unwrap_or(json!({}));
    let field = |k: &str| u.get(k).cloned().unwrap_or(Value::Null);
    let created = u
        .get("created_at")
        .cloned()
        .unwrap_or(json!("2023-01-09T10:00:00.000Z"));
    let extra = json!({
        "created_at": created,
        "bio": u.get("bio").cloned().unwrap_or(json!("")),
        "location": u.get("location").cloned().unwrap_or(json!("")),
        "public_email": u.get("public_email").cloned().unwrap_or(json!("")),
        "linkedin": "",
        "twitter": "",
        "discord": "",
        "website_url": u.get("website_url").cloned().unwrap_or(json!("")),
        "organization": u.get("organization").cloned().unwrap_or(json!("")),
        "job_title": u.get("job_title").cloned().unwrap_or(json!("")),
        "pronouns": field("pronouns"),
        "bot": u.get("bot").cloned().unwrap_or(json!(false)),
        "work_information": field("work_information"),
        "local_time": field("local_time"),
        "followers": 0,
        "following": 0,
        "is_followed": false,
    });
    merge_into(&mut out, extra);
    out
}

pub fn merge_into(target: &mut Value, extra: Value) {
    if let (Some(t), Value::Object(e)) = (target.as_object_mut(), extra) {
        for (k, v) in e {
            t.insert(k, v);
        }
    }
}

// ---------------------------------------------------------------------------
// Groups, namespaces, projects

/// Key of a group by numeric id or full path (case-insensitive path match).
pub fn group_key(store: &Store, id_or_path: &str) -> Option<String> {
    let needle = id_or_path.trim_matches('/');
    all(store, "groups")
        .into_iter()
        .find(|(k, g)| {
            k.eq_ignore_ascii_case(needle)
                || as_u64(&g["id"]).map(|n| n.to_string()).as_deref() == Some(needle)
        })
        .map(|(k, _)| k.to_owned())
}

/// Key of a project by numeric id or `path_with_namespace` (case-insensitive).
pub fn project_key(store: &Store, id_or_path: &str) -> Option<String> {
    let needle = id_or_path.trim_matches('/');
    all(store, "projects")
        .into_iter()
        .find(|(k, p)| {
            k.eq_ignore_ascii_case(needle)
                || as_u64(&p["id"]).map(|n| n.to_string()).as_deref() == Some(needle)
        })
        .map(|(k, _)| k.to_owned())
}

/// Resolves `r.p("id")` to a project key, or the 404 response to return.
pub fn require_project(store: &Store, r: &Req) -> Result<String, Response> {
    project_key(store, r.p("id")).ok_or_else(project_not_found)
}

/// Resolves `r.p("id")` to a group key, or GitLab's group 404.
pub fn require_group(store: &Store, r: &Req) -> Result<String, Response> {
    group_key(store, r.p("id")).ok_or_else(|| not_found("Group"))
}

pub fn project_id(store: &Store, key: &str) -> u64 {
    get(store, "projects", key)
        .and_then(|p| as_u64(&p["id"]))
        .unwrap_or(0)
}

pub fn group_id(store: &Store, key: &str) -> u64 {
    get(store, "groups", key)
        .and_then(|g| as_u64(&g["id"]))
        .unwrap_or(0)
}

/// Project key whose numeric id is `id`.
pub fn project_key_by_id(store: &Store, id: u64) -> Option<String> {
    all(store, "projects")
        .into_iter()
        .find(|(_, p)| as_u64(&p["id"]) == Some(id))
        .map(|(k, _)| k.to_owned())
}

/// Parent path of a full path (`a/b/c` -> `a/b`).
pub fn parent_path(path: &str) -> Option<&str> {
    path.rsplit_once('/').map(|(p, _)| p)
}

pub fn last_segment(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Namespace object for a group key or a username.
pub fn namespace(store: &Store, base: &str, path: &str) -> Value {
    if let Some(g) = get(store, "groups", path) {
        let parent = parent_path(path)
            .map(|p| group_id(store, p))
            .filter(|id| *id > 0);
        json!({
            "id": as_u64(&g["id"]).unwrap_or(0),
            "name": str_of(g, "name"),
            "path": last_segment(path),
            "kind": "group",
            "full_path": path,
            "parent_id": parent,
            "avatar_url": null,
            "web_url": format!("{base}/groups/{path}"),
        })
    } else {
        let u = user(store, path);
        json!({
            "id": u.and_then(|u| as_u64(&u["id"])).unwrap_or(0) + 100_000,
            "name": u.map_or(path, |u| str_of(u, "name")),
            "path": path,
            "kind": "user",
            "full_path": path,
            "parent_id": null,
            "avatar_url": avatar_url(base, path),
            "web_url": format!("{base}/{path}"),
        })
    }
}

/// Full group representation.
pub fn render_group(store: &Store, base: &str, key: &str) -> Value {
    let g = get(store, "groups", key).cloned().unwrap_or(json!({}));
    let parent = parent_path(key).and_then(|p| get(store, "groups", p).map(|_| group_id(store, p)));
    let full_name = full_name_of_group(store, key);
    json!({
        "id": as_u64(&g["id"]).unwrap_or(0),
        "web_url": format!("{base}/groups/{key}"),
        "name": str_of(&g, "name"),
        "path": last_segment(key),
        "description": g.get("description").cloned().unwrap_or(json!("")),
        "visibility": g.get("visibility").cloned().unwrap_or(json!("private")),
        "share_with_group_lock": false,
        "require_two_factor_authentication": false,
        "two_factor_grace_period": 48,
        "project_creation_level": "developer",
        "auto_devops_enabled": null,
        "subgroup_creation_level": "maintainer",
        "emails_disabled": false,
        "emails_enabled": true,
        "mentions_disabled": null,
        "lfs_enabled": true,
        "default_branch": null,
        "request_access_enabled": true,
        "full_name": full_name,
        "full_path": key,
        "created_at": g.get("created_at").cloned().unwrap_or(json!("2023-01-09T10:00:00.000Z")),
        "parent_id": parent,
        "avatar_url": null,
        "file_template_project_id": null,
        "marked_for_deletion_on": null,
    })
}

pub fn full_name_of_group(store: &Store, key: &str) -> String {
    let mut names = Vec::new();
    let mut acc = String::new();
    for seg in key.split('/') {
        if !acc.is_empty() {
            acc.push('/');
        }
        acc.push_str(seg);
        names.push(
            get(store, "groups", &acc)
                .map(|g| str_of(g, "name").to_owned())
                .unwrap_or_else(|| seg.to_owned()),
        );
    }
    names.join(" / ")
}

/// Full project representation (`GET /projects/:id`).
pub fn render_project(store: &Store, base: &str, key: &str) -> Value {
    let p = get(store, "projects", key).cloned().unwrap_or(json!({}));
    let ns_path = parent_path(key).unwrap_or("");
    let ns = namespace(store, base, ns_path);
    let name = p
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_else(|| last_segment(key))
        .to_owned();
    let ns_name = if get(store, "groups", ns_path).is_some() {
        full_name_of_group(store, ns_path)
    } else {
        str_of(&ns, "name").to_owned()
    };
    let host = base
        .split("://")
        .nth(1)
        .unwrap_or("gitlab.local")
        .split(':')
        .next()
        .unwrap_or("gitlab.local")
        .to_owned();
    let created = p
        .get("created_at")
        .cloned()
        .unwrap_or(json!("2024-02-01T10:00:00.000Z"));
    let topics = p.get("topics").cloned().unwrap_or(json!([]));
    let open_issues = all(store, "issues")
        .iter()
        .filter(|(k, i)| {
            k.rsplit_once('#').map(|(p, _)| p) == Some(key) && str_of(i, "state") == "opened"
        })
        .count();
    let default_branch = p.get("default_branch").cloned().unwrap_or(json!("main"));
    let current = current_username(store);
    let access = member_access(store, key, &current);
    let mut out = json!({
        "id": as_u64(&p["id"]).unwrap_or(0),
        "description": p.get("description").cloned().unwrap_or(json!("")),
        "name": name,
        "name_with_namespace": format!("{ns_name} / {name}"),
        "path": last_segment(key),
        "path_with_namespace": key,
        "created_at": created,
        "default_branch": default_branch,
        "tag_list": topics,
        "topics": topics,
        "ssh_url_to_repo": format!("git@{host}:{key}.git"),
        "http_url_to_repo": format!("{base}/{key}.git"),
        "web_url": format!("{base}/{key}"),
        "readme_url": format!("{base}/{key}/-/blob/{}/README.md", default_branch.as_str().unwrap_or("main")),
        "forks_count": p.get("forks_count").cloned().unwrap_or(json!(0)),
        "avatar_url": null,
        "star_count": p.get("star_count").cloned().unwrap_or(json!(0)),
        "last_activity_at": p.get("last_activity_at").cloned().unwrap_or(created),
        "namespace": ns,
        "container_registry_image_prefix": format!("registry.{host}/{key}"),
        "_links": {
            "self": format!("{base}/api/v4/projects/{}", as_u64(&p["id"]).unwrap_or(0)),
            "issues": format!("{base}/api/v4/projects/{}/issues", as_u64(&p["id"]).unwrap_or(0)),
            "merge_requests": format!("{base}/api/v4/projects/{}/merge_requests", as_u64(&p["id"]).unwrap_or(0)),
            "repo_branches": format!("{base}/api/v4/projects/{}/repository/branches", as_u64(&p["id"]).unwrap_or(0)),
            "labels": format!("{base}/api/v4/projects/{}/labels", as_u64(&p["id"]).unwrap_or(0)),
            "events": format!("{base}/api/v4/projects/{}/events", as_u64(&p["id"]).unwrap_or(0)),
            "members": format!("{base}/api/v4/projects/{}/members", as_u64(&p["id"]).unwrap_or(0)),
        },
        "packages_enabled": true,
        "empty_repo": false,
        "archived": p.get("archived").cloned().unwrap_or(json!(false)),
        "visibility": p.get("visibility").cloned().unwrap_or(json!("private")),
        "resolve_outdated_diff_discussions": false,
        "container_registry_enabled": true,
        "container_registry_access_level": "enabled",
        "issues_enabled": p.get("issues_enabled").cloned().unwrap_or(json!(true)),
        "merge_requests_enabled": true,
        "wiki_enabled": p.get("wiki_enabled").cloned().unwrap_or(json!(true)),
        "jobs_enabled": true,
        "snippets_enabled": true,
        "can_create_merge_request_in": true,
        "issues_access_level": "enabled",
        "repository_access_level": "enabled",
        "merge_requests_access_level": "enabled",
        "builds_access_level": "enabled",
        "wiki_access_level": "enabled",
        "snippets_access_level": "enabled",
        "shared_runners_enabled": true,
        "lfs_enabled": true,
        "creator_id": p.get("creator_id").cloned().unwrap_or(json!(1)),
        "open_issues_count": open_issues,
        "ci_default_git_depth": 20,
        "ci_config_path": p.get("ci_config_path").cloned().unwrap_or(json!(null)),
        "public_jobs": true,
        "shared_with_groups": [],
        "only_allow_merge_if_pipeline_succeeds": p.get("only_allow_merge_if_pipeline_succeeds").cloned().unwrap_or(json!(false)),
        "allow_merge_on_skipped_pipeline": null,
        "request_access_enabled": true,
        "only_allow_merge_if_all_discussions_are_resolved": p.get("only_allow_merge_if_all_discussions_are_resolved").cloned().unwrap_or(json!(false)),
        "remove_source_branch_after_merge": true,
        "printing_merge_request_link_enabled": true,
        "merge_method": p.get("merge_method").cloned().unwrap_or(json!("merge")),
        "squash_option": p.get("squash_option").cloned().unwrap_or(json!("default_off")),
        "suggestion_commit_message": null,
        "merge_commit_template": null,
        "squash_commit_template": null,
        "auto_devops_enabled": false,
        "approvals_before_merge": p.get("approvals_before_merge").cloned().unwrap_or(json!(0)),
        "mirror": false,
        "permissions": {
            "project_access": access.map(|a| json!({"access_level": a, "notification_level": 3})),
            "group_access": null,
        },
    });
    if let Some(parent) = p.get("forked_from").and_then(Value::as_str)
        && let Some(pp) = get(store, "projects", parent)
    {
        out["forked_from_project"] = json!({
            "id": as_u64(&pp["id"]).unwrap_or(0),
            "name": pp.get("name").and_then(Value::as_str).unwrap_or_else(|| last_segment(parent)),
            "path": last_segment(parent),
            "path_with_namespace": parent,
            "web_url": format!("{base}/{parent}"),
            "created_at": pp.get("created_at").cloned().unwrap_or(json!(null)),
        });
    }
    if ns.get("kind").and_then(Value::as_str) == Some("user") {
        out["owner"] = user_basic(store, base, ns_path);
    }
    out
}

/// Compact project (`simple=true` and nested `project` objects).
pub fn render_project_simple(store: &Store, base: &str, key: &str) -> Value {
    let full = render_project(store, base, key);
    let mut out = Map::new();
    for k in [
        "id",
        "description",
        "name",
        "name_with_namespace",
        "path",
        "path_with_namespace",
        "created_at",
        "default_branch",
        "tag_list",
        "topics",
        "ssh_url_to_repo",
        "http_url_to_repo",
        "web_url",
        "readme_url",
        "forks_count",
        "avatar_url",
        "star_count",
        "last_activity_at",
        "namespace",
        "visibility",
    ] {
        if let Some(v) = full.get(k) {
            out.insert(k.to_owned(), v.clone());
        }
    }
    Value::Object(out)
}

/// Access level of `username` on a project: explicit member, else inherited from groups.
pub fn member_access(store: &Store, project: &str, username: &str) -> Option<u64> {
    let explicit = get(store, "projects", project)
        .and_then(|p| p.get("members")?.as_array().cloned())
        .and_then(|ms| {
            ms.iter()
                .find(|m| str_of(m, "username") == username)
                .and_then(|m| as_u64(&m["access_level"]))
        });
    explicit.or_else(|| {
        let mut path = parent_path(project);
        while let Some(p) = path {
            if let Some(level) = get(store, "groups", p)
                .and_then(|g| g.get("members")?.as_array().cloned())
                .and_then(|ms| {
                    ms.iter()
                        .find(|m| str_of(m, "username") == username)
                        .and_then(|m| as_u64(&m["access_level"]))
                })
            {
                return Some(level);
            }
            path = parent_path(p);
        }
        None
    })
}

pub fn access_level_description(level: u64) -> &'static str {
    match level {
        0 => "No access",
        5 => "Minimal Access",
        10 => "Guest",
        15 => "Planner",
        20 => "Reporter",
        30 => "Developer",
        40 => "Maintainer",
        50 => "Owner",
        _ => "Custom",
    }
}

/// Project key for an issue/MR key (`a/b#3` / `a/b!3` -> `a/b`).
pub fn project_of(key: &str) -> &str {
    key.rsplit_once(['#', '!', '%', '~', '@'])
        .map_or(key, |(p, _)| p)
}

/// Iid suffix of an issue/MR/milestone key.
pub fn iid_of(key: &str) -> u64 {
    key.rsplit_once(['#', '!', '%'])
        .and_then(|(_, n)| n.parse().ok())
        .unwrap_or(0)
}

/// Next iid for `<project><sep><iid>` keys in a collection.
pub fn next_iid(store: &Store, collection: &str, project: &str, sep: char) -> u64 {
    let prefix = format!("{project}{sep}");
    all(store, collection)
        .iter()
        .filter(|(k, _)| k.starts_with(&prefix))
        .filter_map(|(k, v)| as_u64(&v["iid"]).or_else(|| k[prefix.len()..].parse().ok()))
        .max()
        .unwrap_or(0)
        .max(high_water(store, &format!("{collection}:{project}")))
        + 1
}

/// Sorts newest first by `created_at` then `id` (GitLab's default list order).
pub fn sort_desc_by_created(items: &mut [Value]) {
    items.sort_by(|a, b| {
        let ka = (
            str_of(a, "created_at").to_owned(),
            as_u64(&a["id"]).unwrap_or(0),
        );
        let kb = (
            str_of(b, "created_at").to_owned(),
            as_u64(&b["id"]).unwrap_or(0),
        );
        kb.cmp(&ka)
    });
}

// ---------------------------------------------------------------------------
// Base64 (repository files are transferred base64-encoded)

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
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
    fn base64_round_trips() {
        for s in ["", "a", "ab", "abc", "hello world\n"] {
            assert_eq!(b64_decode(&b64_encode(s.as_bytes())).unwrap(), s.as_bytes());
        }
        assert_eq!(b64_encode(b"hello"), "aGVsbG8=");
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("acme%2Fapi"), "acme/api");
        assert_eq!(percent_decode("a+b%20c"), "a b c");
        assert_eq!(percent_decode("100%"), "100%");
    }

    #[test]
    fn key_helpers() {
        assert_eq!(project_of("acme/api!3"), "acme/api");
        assert_eq!(iid_of("acme/api#12"), 12);
        assert_eq!(
            gitlab_time("2026-03-14T09:00:07Z"),
            "2026-03-14T09:00:07.000Z"
        );
    }
}
