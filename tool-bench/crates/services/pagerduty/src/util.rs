//! Request plumbing shared by every PagerDuty handler: query parsing (`key[]=a&key[]=b`),
//! the error and list envelopes, the per-request context over the service store,
//! references, deterministic IDs and time parsing.

use axum::{
    body::{Bytes, to_bytes},
    extract::{FromRequest, Request},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use mock_core::{clock::format_unix, http::json, ids::IdGen, store::Store, world::SharedWorld};
use serde_json::{Map, Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::NAME;

/// Classic pagination: `offset + limit` may not exceed this.
const ITERATION_LIMIT: usize = 10_000;

// ---------------------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------------------

/// Everything a handler needs from the request besides path parameters.
pub struct Req {
    pub query: Query,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl<S: Send + Sync> FromRequest<S> for Req {
    type Rejection = Response;

    async fn from_request(req: Request, _state: &S) -> Result<Self, Response> {
        let (parts, body) = req.into_parts();
        let body = to_bytes(body, usize::MAX)
            .await
            .map_err(|_| ApiErr::invalid("Request body could not be read").into_response())?;
        Ok(Self {
            query: Query::parse(parts.uri.query().unwrap_or("")),
            headers: parts.headers,
            body,
        })
    }
}

impl Req {
    /// The JSON body, or PagerDuty's 400 for unparseable input. An empty body is `{}`.
    pub fn json(&self) -> Result<Value, ApiErr> {
        if self.body.iter().all(u8::is_ascii_whitespace) {
            return Ok(json!({}));
        }
        serde_json::from_slice(&self.body).map_err(|_| ApiErr::invalid("Invalid JSON body"))
    }
}

/// Decoded query string, keeping repeated keys in order.
#[derive(Debug, Clone, Default)]
pub struct Query(Vec<(String, String)>);

impl Query {
    pub fn parse(raw: &str) -> Self {
        Self(
            raw.split('&')
                .filter(|p| !p.is_empty())
                .map(|pair| {
                    let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                    (decode(k), decode(v))
                })
                .collect(),
        )
    }

    /// First value of `key` (also accepts `key[]`).
    pub fn get(&self, key: &str) -> Option<&str> {
        let bracket = format!("{key}[]");
        self.0
            .iter()
            .find(|(k, _)| k == key || *k == bracket)
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty())
    }

    /// All values of `key[]` (or `key`), splitting comma-separated values.
    pub fn list(&self, key: &str) -> Vec<String> {
        let bracket = format!("{key}[]");
        self.0
            .iter()
            .filter(|(k, _)| k == key || *k == bracket)
            .flat_map(|(_, v)| v.split(',').map(str::trim).map(str::to_owned))
            .filter(|v| !v.is_empty())
            .collect()
    }

    pub fn flag(&self, key: &str) -> bool {
        self.get(key)
            .is_some_and(|v| v.eq_ignore_ascii_case("true"))
    }

    pub fn number(&self, key: &str) -> Result<Option<usize>, ApiErr> {
        match self.get(key) {
            None => Ok(None),
            Some(v) => v
                .parse::<usize>()
                .map(Some)
                .map_err(|_| ApiErr::invalid(format!("{key} must be a non-negative integer"))),
        }
    }

    /// A time parameter, or 400 if present but unparseable.
    pub fn time(&self, key: &str) -> Result<Option<i64>, ApiErr> {
        match self.get(key) {
            None => Ok(None),
            Some(v) => parse_time(v)
                .map(Some)
                .ok_or_else(|| ApiErr::invalid(format!("{key} is not a valid date/time"))),
        }
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
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
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

// ---------------------------------------------------------------------------------------
// Responses and errors
// ---------------------------------------------------------------------------------------

#[derive(Debug)]
pub enum ApiErr {
    NotFound,
    Invalid(Vec<String>),
    Status(StatusCode, Value),
}

impl ApiErr {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(vec![message.into()])
    }

    /// 400 whose `error.message` itself carries the explanation (schedule v2/v3 redirects).
    pub fn bad_request(message: &str) -> Self {
        Self::Status(
            StatusCode::BAD_REQUEST,
            json!({ "error": { "message": message, "code": 2001 } }),
        )
    }
}

impl IntoResponse for ApiErr {
    fn into_response(self) -> Response {
        match self {
            Self::NotFound => not_found(),
            Self::Invalid(errors) => json(
                StatusCode::BAD_REQUEST,
                json!({ "error": { "message": "Invalid Input Provided", "code": 2001, "errors": errors } }),
            ),
            Self::Status(status, body) => json(status, body),
        }
    }
}

pub type Res = Result<Response, ApiErr>;

pub fn not_found() -> Response {
    json(
        StatusCode::NOT_FOUND,
        json!({ "error": { "message": "Not Found", "code": 2100 } }),
    )
}

pub fn ok(body: Value) -> Res {
    Ok(json(StatusCode::OK, body))
}

pub fn created(body: Value) -> Res {
    Ok(json(StatusCode::CREATED, body))
}

pub fn no_content() -> Res {
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Classic offset pagination envelope: `{"<wrapper>": [...], "limit", "offset", "more", "total"}`.
pub fn classic(q: &Query, wrapper: &str, items: Vec<Value>) -> Res {
    let limit = q.number("limit")?.unwrap_or(25).clamp(1, 100);
    let offset = q.number("offset")?.unwrap_or(0);
    if offset + limit > ITERATION_LIMIT {
        return Err(ApiErr::invalid(format!(
            "Offset must be less than {}",
            ITERATION_LIMIT - limit + 1
        )));
    }
    let total = items.len();
    let page: Vec<Value> = items.into_iter().skip(offset).take(limit).collect();
    let more = offset + page.len() < total;
    let mut body = Map::new();
    body.insert(wrapper.to_owned(), Value::Array(page));
    body.insert("limit".into(), json!(limit));
    body.insert("offset".into(), json!(offset));
    body.insert("more".into(), json!(more));
    body.insert(
        "total".into(),
        if q.flag("total") {
            json!(total)
        } else {
            Value::Null
        },
    );
    ok(Value::Object(body))
}

// ---------------------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------------------

/// Kinds of top-level resources that other objects reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    User,
    Team,
    Service,
    EscalationPolicy,
    Schedule,
    ScheduleV3,
    Priority,
    Incident,
    BusinessService,
}

impl Kind {
    pub fn collection(self) -> &'static str {
        match self {
            Self::User => "users",
            Self::Team => "teams",
            Self::Service => "services",
            Self::EscalationPolicy => "escalation_policies",
            Self::Schedule => "schedules",
            Self::ScheduleV3 => "schedules_v3",
            Self::Priority => "priorities",
            Self::Incident => "incidents",
            Self::BusinessService => "business_services",
        }
    }

    fn api_path(self) -> &'static str {
        match self {
            Self::ScheduleV3 => "v3/schedules",
            other => other.collection(),
        }
    }

    fn web_path(self) -> &'static str {
        match self {
            Self::EscalationPolicy => "escalation_policies#",
            Self::ScheduleV3 => "schedules",
            Self::Service => "service-directory",
            Self::BusinessService => "business-services",
            other => other.collection(),
        }
    }

    pub fn object_type(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Team => "team",
            Self::Service => "service",
            Self::EscalationPolicy => "escalation_policy",
            Self::Schedule | Self::ScheduleV3 => "schedule",
            Self::Priority => "priority",
            Self::Incident => "incident",
            Self::BusinessService => "business_service",
        }
    }

    pub fn reference_type(self) -> &'static str {
        match self {
            Self::User => "user_reference",
            Self::Team => "team_reference",
            Self::Service => "service_reference",
            Self::EscalationPolicy => "escalation_policy_reference",
            Self::Schedule => "schedule_reference",
            Self::ScheduleV3 => "schedule_v3_reference",
            Self::Priority => "priority_reference",
            Self::Incident => "incident_reference",
            Self::BusinessService => "business_service_reference",
        }
    }
}

/// One request's view of the PagerDuty service state.
pub struct Ctx<'a> {
    pub store: &'a mut Store,
    pub ids: &'a mut IdGen,
    /// Logical now (unix seconds).
    pub now: i64,
    /// API base URL for `self` links, e.g. `http://world:8111`.
    pub api: String,
    /// Web UI base URL for `html_url`, e.g. `https://tidewater.pagerduty.com`.
    pub web: String,
    /// The `From` header (acting user's email), if sent.
    pub from: Option<String>,
}

/// Runs `f` against the locked world and turns its result into a response.
pub fn run(world: &SharedWorld, req: &Req, f: impl FnOnce(&mut Ctx) -> Res) -> Response {
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    let now = world.clock.now_unix();
    let service = world.service_mut(NAME);
    let subdomain = service
        .store
        .collection("account")
        .and_then(|c| c.get("account"))
        .and_then(|a| a["subdomain"].as_str())
        .unwrap_or("acme")
        .to_owned();
    let mut ctx = Ctx {
        store: &mut service.store,
        ids: &mut service.ids,
        now,
        api: base_url(&req.headers),
        web: format!("https://{subdomain}.pagerduty.com"),
        from: req
            .headers
            .get("from")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned),
    };
    match f(&mut ctx) {
        Ok(response) => response,
        Err(err) => err.into_response(),
    }
}

fn base_url(headers: &HeaderMap) -> String {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    format!("http://{host}")
}

impl Ctx<'_> {
    pub fn now_str(&self) -> String {
        format_unix(self.now)
    }

    pub fn docs(&self, collection: &str) -> Vec<Value> {
        self.store
            .collection(collection)
            .map(|c| c.values().cloned().collect())
            .unwrap_or_default()
    }

    /// `(key, doc)` pairs of a collection, in order.
    pub fn entries(&self, collection: &str) -> Vec<(String, Value)> {
        self.store
            .collection(collection)
            .map(|c| c.iter().map(|(k, v)| (k.to_owned(), v.clone())).collect())
            .unwrap_or_default()
    }

    pub fn get(&self, collection: &str, key: &str) -> Option<Value> {
        self.store
            .collection(collection)
            .and_then(|c| c.get(key))
            .cloned()
    }

    pub fn exists(&self, collection: &str, key: &str) -> bool {
        self.store
            .collection(collection)
            .is_some_and(|c| c.contains(key))
    }

    pub fn put(&mut self, collection: &str, key: &str, doc: Value) {
        self.store.collection_mut(collection).insert(key, doc);
    }

    pub fn remove(&mut self, collection: &str, key: &str) -> Option<Value> {
        self.store.collection_mut(collection).remove(key)
    }

    /// Children of `parent` in a collection keyed `<parent>/<n>`.
    pub fn children(&self, collection: &str, parent: &str) -> Vec<(String, Value)> {
        let prefix = format!("{parent}/");
        self.entries(collection)
            .into_iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .collect()
    }

    /// Next `<parent>/<n>` key in a child collection.
    pub fn next_child_key(&self, collection: &str, parent: &str) -> String {
        let prefix = format!("{parent}/");
        let n = self
            .children(collection, parent)
            .iter()
            .filter_map(|(k, _)| k[prefix.len()..].parse::<u64>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        format!("{prefix}{n}")
    }

    /// Finds a child document by its `id` field.
    pub fn find_child(&self, collection: &str, parent: &str, id: &str) -> Option<(String, Value)> {
        self.children(collection, parent)
            .into_iter()
            .find(|(_, v)| v["id"] == id)
    }

    /// A new PagerDuty-style ID: `P` plus six upper-case alphanumerics, unused in `collection`.
    pub fn new_id(&mut self, collection: &str) -> String {
        self.new_id_len(collection, 'P', 6)
    }

    /// Incident IDs are longer: `Q` plus thirteen characters.
    pub fn new_incident_id(&mut self) -> String {
        self.new_id_len("incidents", 'Q', 13)
    }

    fn new_id_len(&mut self, collection: &str, first: char, len: usize) -> String {
        loop {
            let raw = self.ids.next_prefixed(collection, "", len);
            let id = format!("{first}{}", raw.to_ascii_uppercase());
            if !self.exists(collection, &id)
                && !self.docs(collection).iter().any(|d| d["id"] == id.as_str())
            {
                return id;
            }
        }
    }

    /// Lower-case hex string from the ID stream (orchestration rule IDs, versions, secrets).
    pub fn new_hex(&mut self, kind: &str, len: usize) -> String {
        let raw = self.ids.next_prefixed(kind, "", len);
        raw.bytes()
            .map(|b| char::from(b"0123456789abcdef"[usize::from(b) % 16]))
            .collect()
    }

    // ----- users -----

    /// The user that owns the API token (`/users/me`): `account.user_id`, else the first user.
    pub fn me(&self) -> Value {
        let users = self.docs("users");
        let owner = self
            .get("account", "account")
            .and_then(|a| a["user_id"].as_str().map(str::to_owned));
        owner
            .and_then(|id| users.iter().find(|u| u["id"] == id.as_str()).cloned())
            .or_else(|| users.first().cloned())
            .unwrap_or_else(default_user)
    }

    /// The user named by the `From` header.
    pub fn user_from_header(&self) -> Option<Value> {
        let email = self.from.as_deref()?.trim().to_ascii_lowercase();
        self.docs("users")
            .into_iter()
            .find(|u| u["email"].as_str().map(str::to_ascii_lowercase) == Some(email.clone()))
    }

    /// The acting user for writes that PagerDuty requires a valid `From` header for.
    pub fn require_from(&self) -> Result<Value, ApiErr> {
        match self.from.as_deref() {
            None | Some("") => Err(ApiErr::invalid(
                "You must specify a user's email address in the \"From\" header to perform this action",
            )),
            Some(_) => self.user_from_header().ok_or_else(|| {
                ApiErr::invalid(
                    "The email address in the \"From\" header does not belong to a user",
                )
            }),
        }
    }

    /// The acting user if `From` names one, else the token owner.
    pub fn actor(&self) -> Value {
        self.user_from_header().unwrap_or_else(|| self.me())
    }

    // ----- references -----

    pub fn lookup(&self, kind: Kind, id: &str) -> Option<Value> {
        self.get(kind.collection(), id).or_else(|| {
            self.docs(kind.collection())
                .into_iter()
                .find(|d| d["id"] == id)
        })
    }

    pub fn api_url(&self, kind: Kind, id: &str) -> String {
        format!("{}/{}/{id}", self.api, kind.api_path())
    }

    pub fn html_url(&self, kind: Kind, id: &str) -> String {
        match kind {
            Kind::EscalationPolicy => format!("{}/escalation_policies#{id}", self.web),
            Kind::Priority => format!("{}/account/incident_priorities", self.web),
            other => format!("{}/{}/{id}", self.web, other.web_path()),
        }
    }

    /// Full reference object (`id`, `type`, `summary`, `self`, `html_url`).
    pub fn reference(&self, kind: Kind, id: &str) -> Value {
        let summary = self
            .lookup(kind, id)
            .map(|d| summary_of(kind, &d))
            .unwrap_or(Value::Null);
        let mut r = json!({
            "id": id,
            "type": kind.reference_type(),
            "summary": summary,
            "self": self.api_url(kind, id),
            "html_url": self.html_url(kind, id),
        });
        if kind == Kind::Priority {
            r["html_url"] = Value::Null;
        }
        r
    }

    /// Replaces a stored `{"id": ...}` reference with the full reference; null stays null.
    pub fn fix_ref(&self, value: &mut Value, kind: Kind) {
        if let Some(id) = value.get("id").and_then(Value::as_str) {
            *value = self.reference(kind, id);
        }
    }

    pub fn fix_refs(&self, value: &mut Value, kind: Kind) {
        if let Some(items) = value.as_array_mut() {
            for item in items {
                self.fix_ref(item, kind);
            }
        }
    }

    /// Adds `type`, `summary`, `self` and `html_url` when the stored document lacks them.
    pub fn base_fields(&self, doc: &mut Value, kind: Kind) {
        let id = doc["id"].as_str().unwrap_or_default().to_owned();
        let summary = summary_of(kind, doc);
        let api = self.api_url(kind, &id);
        let html = self.html_url(kind, &id);
        let Some(obj) = doc.as_object_mut() else {
            return;
        };
        obj.entry("type")
            .or_insert_with(|| json!(kind.object_type()));
        obj.entry("summary").or_insert(summary);
        obj.entry("self").or_insert_with(|| json!(api));
        obj.entry("html_url").or_insert_with(|| json!(html));
    }
}

/// Server-generated summary: the name, or `[#N] title` for incidents.
pub fn summary_of(kind: Kind, doc: &Value) -> Value {
    if let Some(s) = doc.get("summary").filter(|s| s.is_string()) {
        return s.clone();
    }
    match kind {
        Kind::Incident => json!(format!(
            "[#{}] {}",
            doc["incident_number"],
            doc["title"].as_str().unwrap_or_default()
        )),
        _ => doc.get("name").cloned().unwrap_or(Value::Null),
    }
}

/// Used when the dataset has no users at all, so `/users/me` still answers.
pub fn default_user() -> Value {
    json!({
        "id": "PTBOWNR",
        "name": "Account Owner",
        "email": "owner@example.com",
        "role": "owner",
        "time_zone": "Etc/UTC",
        "teams": []
    })
}

/// Inserts `value` under `key` when absent.
pub fn default(doc: &mut Value, key: &str, value: Value) {
    if let Some(obj) = doc.as_object_mut() {
        obj.entry(key).or_insert(value);
    }
}

/// Copies the listed fields from `src` into `dst` when present and not null.
pub fn merge_fields(dst: &mut Value, src: &Value, fields: &[&str]) {
    for f in fields {
        if let Some(v) = src.get(*f).filter(|v| !v.is_null()) {
            dst[*f] = v.clone();
        }
    }
}

/// Minimal stored reference `{"id", "type"}`.
pub fn stored_ref(kind: Kind, id: &str) -> Value {
    json!({ "id": id, "type": kind.reference_type() })
}

pub fn ref_id(value: &Value) -> Option<&str> {
    value.get("id").and_then(Value::as_str)
}

/// Case-insensitive substring match used by `query` filters.
pub fn matches_query(q: Option<&str>, fields: &[&Value]) -> bool {
    let Some(q) = q else { return true };
    let q = q.to_ascii_lowercase();
    fields
        .iter()
        .filter_map(|f| f.as_str())
        .any(|f| f.to_ascii_lowercase().contains(&q))
}

// ---------------------------------------------------------------------------------------
// Time and derived values
// ---------------------------------------------------------------------------------------

/// Parses RFC 3339 times, also accepting a missing offset (UTC) and bare dates.
pub fn parse_time(s: &str) -> Option<i64> {
    let s = s.trim();
    let parse = |t: &str| {
        OffsetDateTime::parse(t, &Rfc3339)
            .ok()
            .map(|d| d.unix_timestamp())
    };
    parse(s)
        .or_else(|| parse(&format!("{s}Z")))
        .or_else(|| parse(&format!("{}Z", s.replacen(' ', "T", 1))))
        .or_else(|| {
            (s.len() == 10)
                .then(|| parse(&format!("{s}T00:00:00Z")))
                .flatten()
        })
}

pub fn time_of(doc: &Value, field: &str) -> Option<i64> {
    doc.get(field).and_then(Value::as_str).and_then(parse_time)
}

/// The value as a normalized UTC timestamp string, if it parses.
pub fn norm_time(value: &Value) -> Option<String> {
    value.as_str().and_then(parse_time).map(format_unix)
}

pub fn fmt(ts: i64) -> String {
    format_unix(ts)
}

/// FNV-1a 64-bit hash.
pub fn fnv(seed: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in seed.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Stable PagerDuty-style ID derived from a seed (for objects the dataset leaves unnamed).
pub fn derived_id(seed: &str) -> String {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut h = fnv(seed);
    let mut out = String::from("P");
    for _ in 0..6 {
        out.push(char::from(ALPHABET[(h % 36) as usize]));
        h /= 36;
    }
    out
}

pub fn derived_hex(seed: &str, len: usize) -> String {
    let mut out = String::new();
    let mut n = 0u64;
    while out.len() < len {
        out.push_str(&format!("{:016x}", fnv(&format!("{seed}/{n}"))));
        n += 1;
    }
    out.truncate(len);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_handles_brackets_and_encoding() {
        let q = Query::parse(
            "statuses%5B%5D=triggered&statuses%5B%5D=acknowledged&since=2026-10-30T10%3A00%3A00%2B00%3A00&query=a+b",
        );
        assert_eq!(q.list("statuses"), ["triggered", "acknowledged"]);
        assert_eq!(q.get("since"), Some("2026-10-30T10:00:00+00:00"));
        assert_eq!(q.get("query"), Some("a b"));
        assert_eq!(q.get("missing"), None);
    }

    #[test]
    fn parses_lenient_times() {
        let z = parse_time("2026-10-30T10:00:00Z").unwrap();
        assert_eq!(parse_time("2026-10-30T10:00:00+00:00"), Some(z));
        assert_eq!(parse_time("2026-10-30T10:00:00"), Some(z));
        assert_eq!(parse_time("2026-10-30T12:00:00+02:00"), Some(z));
        assert_eq!(parse_time("2026-10-30T10:00:00.123456"), Some(z));
        assert_eq!(parse_time("2026-10-30"), Some(z - 36_000));
        assert_eq!(parse_time("soon"), None);
    }

    #[test]
    fn derived_ids_are_stable() {
        assert_eq!(derived_id("a"), derived_id("a"));
        assert_ne!(derived_id("a"), derived_id("b"));
        assert_eq!(derived_id("a").len(), 7);
        assert_eq!(derived_hex("x", 20).len(), 20);
    }
}
