//! Mock Supabase: the Management API (`/v1`, `/v2`) that `@supabase/mcp-server-supabase`
//! 0.15.0 calls with `--api-url` pointed at the world, plus the docs Content API
//! (`/docs/api/graphql`).
//!
//! Dataset collections (`scenarios/<id>/dataset/supabase/*.json`):
//!
//! | collection | key | content |
//! |---|---|---|
//! | `organizations` | slug | `{id, name, plan, opt_in_tags, allowed_release_channels}` |
//! | `projects` | ref | Management API project object (+ `branching_enabled`) |
//! | `tables` | `<ref>.<schema>.<table>` | table definition (see [`sql::TableDef`]) |
//! | `db.<ref>.<schema>.<table>` | primary key | rows |
//! | `migrations` | `<ref>/<version>` | `{version, name, statements}` |
//! | `functions` / `function_bodies` | `<ref>/<slug>` | Edge Function metadata / `{files}` |
//! | `branches` | branch id | branch object |
//! | `buckets` | `<ref>/<id>` | storage bucket |
//! | `storage_config` | ref | storage config |
//! | `secrets`, `api_keys` | `<ref>/<name>` | secrets / API keys |
//! | `logs` | id | `{project, source, timestamp, event_message, severity_text, log_attributes}` |
//! | `advisors` | any | `{project, type: security|performance|health, ...lint}` |
//! | `notebooks` | `<ref>/<id>` | notebook with `content` |

#![allow(clippy::result_large_err)]

pub mod db;
pub mod docs;
mod feedback;
pub mod multipart;
pub mod sql;

use std::collections::HashMap;

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::get,
};
use mock_core::{
    http::{GraphqlRequest, graphql_data, json},
    service::MockService,
    world::{ServiceState, SharedWorld, WorldState},
};
use serde_json::{Map, Value, json as json_value};
use sha2::{Digest, Sha256};

pub use docs::CONTENT_API_SCHEMA;

pub const NAME: &str = "supabase";
pub const PORT: u16 = 8118;

/// Free-plan global upload limit (50 MB).
const FREE_FILE_SIZE_LIMIT: u64 = 52_428_800;

pub struct Supabase;

impl MockService for Supabase {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port(&self) -> u16 {
        PORT
    }

    fn hosts(&self) -> &'static [&'static str] {
        &[feedback::HOST]
    }

    fn router(&self) -> Router<SharedWorld> {
        Router::new()
            .route(
                "/docs/api/graphql",
                get(content_api_get).post(content_api_post),
            )
            .route(
                "/rest/v1/rpc/submit_interfaces_feedback",
                axum::routing::post(feedback::submit),
            )
            .route(
                "/rest/v1/interfaces_feedback",
                axum::routing::delete(feedback::delete),
            )
            .fallback(dispatch)
    }
}

// ───────────────────────────── Content API ─────────────────────────────

/// The client sends queries as `GET ?query=...` (and POST is accepted too).
async fn content_api_get(Query(params): Query<HashMap<String, String>>) -> Response {
    let variables = params
        .get("variables")
        .and_then(|v| serde_json::from_str(v).ok())
        .unwrap_or(Value::Null);
    answer(params.get("query").map_or("", String::as_str), &variables)
}

async fn content_api_post(Json(request): Json<GraphqlRequest>) -> Response {
    answer(&request.query, &request.variables)
}

fn answer(query: &str, variables: &Value) -> Response {
    match docs::execute(query, variables) {
        Ok(data) => graphql_data(data),
        Err(messages) => json(
            StatusCode::OK,
            json_value!({
                "errors": messages.iter().map(|m| json_value!({ "message": m, "locations": [{ "line": 1, "column": 1 }] })).collect::<Vec<_>>(),
            }),
        ),
    }
}

// ───────────────────────────── helpers ─────────────────────────────

fn msg(status: StatusCode, message: &str) -> Response {
    json(status, json_value!({ "message": message }))
}

fn not_found() -> Response {
    msg(StatusCode::NOT_FOUND, "Not Found")
}

fn project_not_found() -> Response {
    msg(StatusCode::NOT_FOUND, "Project not found")
}

fn ok(body: Value) -> Response {
    json(StatusCode::OK, body)
}

fn created(body: Value) -> Response {
    json(StatusCode::CREATED, body)
}

fn docs_with_prefix(svc: &ServiceState, collection: &str, prefix: &str) -> Vec<(String, Value)> {
    svc.store
        .collection(collection)
        .map(|c| {
            c.iter()
                .filter(|(k, _)| k.starts_with(prefix))
                .map(|(k, v)| (k.to_owned(), v.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn get_doc(svc: &ServiceState, collection: &str, key: &str) -> Option<Value> {
    svc.store
        .collection(collection)
        .and_then(|c| c.get(key))
        .cloned()
}

const PROJECT_INTERNAL: [&str; 3] = ["branching_enabled", "parent_project_ref", "db_pass"];

fn project_out(doc: &Value) -> Value {
    let mut out = doc.clone();
    if let Some(obj) = out.as_object_mut() {
        for k in PROJECT_INTERNAL {
            obj.shift_remove(k);
        }
    }
    out
}

fn lower_id(svc: &mut ServiceState, kind: &str, len: usize) -> String {
    let mut out = String::new();
    while out.len() < len {
        for b in svc.ids.next_uuid(kind).as_bytes() {
            if out.len() < len {
                out.push((b'a' + b % 26) as char);
            }
        }
    }
    out
}

fn compact_time(rfc3339: &str) -> String {
    rfc3339
        .chars()
        .filter(char::is_ascii_digit)
        .take(14)
        .collect()
}

fn unix_ms(world: &WorldState) -> i64 {
    world.clock.now_unix() * 1000
}

fn b64url(bytes: &[u8]) -> String {
    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
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

fn digest_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn parse_body(body: &Bytes) -> Result<Value, Response> {
    if body.is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_slice(body).map_err(|_| msg(StatusCode::BAD_REQUEST, "Invalid JSON body"))
}

fn query_map(uri: &Uri) -> HashMap<String, String> {
    uri.query()
        .map(|q| {
            q.split('&')
                .map(|kv| {
                    let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
                    (decode(k), decode(v))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                        continue;
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

// ───────────────────────────── dispatch ─────────────────────────────

async fn dispatch(
    State(world): State<SharedWorld>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path().trim_end_matches('/').to_owned();
    if !(path.starts_with("/v1/") || path.starts_with("/v2/")) {
        return not_found();
    }
    let authorized = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("Bearer ") && v.len() > 7);
    if !authorized {
        return msg(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    let segs: Vec<String> = path.split('/').skip(1).map(decode).collect();
    let segs: Vec<&str> = segs.iter().map(String::as_str).collect();
    let q = query_map(&uri);
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    let m = method.as_str();
    match (m, segs.as_slice()) {
        ("GET", ["v1", "profile"]) => profile(w),
        ("GET", ["v1", "organizations"]) => list_organizations(w),
        ("GET", ["v1", "organizations", slug]) => get_organization(w, slug),
        ("GET", ["v1", "organizations", slug, "projects"]) => list_org_projects(w, slug),
        ("GET", ["v1", "projects"]) => list_projects(w),
        ("POST", ["v1", "projects"]) => match parse_body(&body) {
            Ok(b) => create_project(w, &b),
            Err(r) => r,
        },
        ("GET", ["v1", "branches", id]) => get_branch(w, id),
        ("DELETE", ["v1", "branches", id]) => delete_branch(w, id),
        ("POST", ["v1", "branches", id, action]) => match parse_body(&body) {
            Ok(b) => branch_action(w, id, action, &b),
            Err(r) => r,
        },
        (_, ["v1", "projects", r, rest @ ..]) => {
            let r = (*r).to_owned();
            if get_doc(w.service_mut(NAME), "projects", &r).is_none() {
                return project_not_found();
            }
            project_route(w, m, &r, rest, &q, &headers, &body)
        }
        (_, ["v2", "projects", r, rest @ ..]) => {
            let r = (*r).to_owned();
            if get_doc(w.service_mut(NAME), "projects", &r).is_none() {
                return json(
                    StatusCode::NOT_FOUND,
                    json_value!({ "error": { "code": "not_found", "message": "Project not found" } }),
                );
            }
            v2_route(w, m, &r, rest, &q, &body)
        }
        _ => not_found(),
    }
}

fn project_route(
    w: &mut WorldState,
    m: &str,
    r: &str,
    rest: &[&str],
    q: &HashMap<String, String>,
    headers: &HeaderMap,
    body: &Bytes,
) -> Response {
    let body_json = || parse_body(body);
    match (m, rest) {
        ("GET", []) => get_project(w, r),
        ("DELETE", []) => delete_project(w, r),
        ("POST", ["pause"]) => pause_project(w, r),
        ("POST", ["restore"]) => restore_project(w, r),
        ("GET", ["health"]) => project_health(w, r),
        ("POST", ["database", "query"]) => match body_json() {
            Ok(b) => run_query(w, r, &b),
            Err(e) => e,
        },
        ("GET", ["database", "migrations"]) => list_migrations(w, r),
        ("POST", ["database", "migrations"]) => match body_json() {
            Ok(b) => apply_migration(w, r, &b),
            Err(e) => e,
        },
        ("GET", ["analytics", "endpoints", "logs"]) => logs(w, r, q),
        ("GET", ["advisors", kind]) if *kind == "security" || *kind == "performance" => {
            advisors(w, r, kind)
        }
        ("GET", ["api-keys"]) => api_keys(w, r, q.get("reveal").is_some_and(|v| v == "true")),
        ("GET", ["api-keys", "legacy"]) => ok(json_value!({ "enabled": true })),
        ("GET", ["types", "typescript"]) => typescript(w, r),
        ("GET", ["functions"]) => list_functions(w, r),
        ("POST", ["functions", "deploy"]) => deploy_function(w, r, q, headers, body),
        ("GET", ["functions", slug]) => get_function(w, r, slug),
        ("DELETE", ["functions", slug]) => delete_function(w, r, slug),
        ("GET", ["functions", slug, "body"]) => function_body(w, r, slug),
        ("GET", ["branches"]) => list_branches(w, r),
        ("POST", ["branches"]) => match body_json() {
            Ok(b) => create_branch(w, r, &b),
            Err(e) => e,
        },
        ("GET", ["storage", "buckets"]) => list_buckets(w, r),
        ("GET", ["config", "storage"]) => get_storage_config(w, r),
        ("PATCH", ["config", "storage"]) => match body_json() {
            Ok(b) => update_storage_config(w, r, &b),
            Err(e) => e,
        },
        ("GET", ["secrets"]) => list_secrets(w, r),
        ("POST", ["secrets"]) => match body_json() {
            Ok(b) => create_secrets(w, r, &b),
            Err(e) => e,
        },
        ("DELETE", ["secrets"]) => match body_json() {
            Ok(b) => delete_secrets(w, r, &b),
            Err(e) => e,
        },
        _ => not_found(),
    }
}

// ───────────────────────────── account ─────────────────────────────

fn profile(w: &mut WorldState) -> Response {
    let svc = w.service_mut(NAME);
    let p = get_doc(svc, "profile", "profile").unwrap_or_else(|| {
        json_value!({
            "gotrue_id": "7f6e5d4c-3b2a-4190-8a7b-6c5d4e3f2a10",
            "primary_email": "platform@octo-corp.example",
            "username": "octo-platform",
            "first_name": "Octo",
            "last_name": "Platform",
        })
    });
    let mut resp = ok(p.clone());
    if let Some(id) = p["gotrue_id"].as_str()
        && let Ok(v) = HeaderValue::from_str(id)
    {
        resp.headers_mut().insert("x-gotrue-id", v);
    }
    resp
}

fn list_organizations(w: &mut WorldState) -> Response {
    let svc = w.service_mut(NAME);
    let orgs: Vec<Value> = docs_with_prefix(svc, "organizations", "")
        .into_iter()
        .map(|(k, o)| {
            json_value!({
                "id": o.get("id").cloned().unwrap_or(json_value!(k)),
                "slug": o.get("slug").or(o.get("id")).cloned().unwrap_or(json_value!(k)),
                "name": o["name"],
            })
        })
        .collect();
    ok(Value::Array(orgs))
}

fn get_organization(w: &mut WorldState, slug: &str) -> Response {
    let svc = w.service_mut(NAME);
    match get_doc(svc, "organizations", slug) {
        Some(o) => ok(json_value!({
            "id": o.get("id").cloned().unwrap_or(json_value!(slug)),
            "name": o["name"],
            "plan": o.get("plan").cloned().unwrap_or(json_value!("free")),
            "opt_in_tags": o.get("opt_in_tags").cloned().unwrap_or(json_value!([])),
            "allowed_release_channels": o.get("allowed_release_channels").cloned().unwrap_or(json_value!(["ga"])),
        })),
        None => msg(StatusCode::NOT_FOUND, "Organization not found"),
    }
}

fn main_projects(svc: &ServiceState) -> Vec<Value> {
    docs_with_prefix(svc, "projects", "")
        .into_iter()
        .map(|(_, p)| p)
        .filter(|p| p.get("parent_project_ref").is_none_or(Value::is_null))
        .collect()
}

fn list_projects(w: &mut WorldState) -> Response {
    let svc = w.service_mut(NAME);
    ok(Value::Array(
        main_projects(svc).iter().map(project_out).collect(),
    ))
}

fn list_org_projects(w: &mut WorldState, slug: &str) -> Response {
    let svc = w.service_mut(NAME);
    if get_doc(svc, "organizations", slug).is_none() {
        return msg(StatusCode::NOT_FOUND, "Organization not found");
    }
    let projects: Vec<Value> = main_projects(svc)
        .iter()
        .filter(|p| p["organization_id"] == slug || p["organization_slug"] == slug)
        .map(|p| {
            json_value!({
                "ref": p["ref"],
                "name": p["name"],
                "cloud_provider": "AWS",
                "region": p["region"],
                "status": p["status"],
                "inserted_at": p["created_at"],
                "is_branch": false,
                "databases": [],
            })
        })
        .collect();
    ok(
        json_value!({ "projects": projects, "pagination": { "count": projects.len(), "limit": 100, "offset": 0 } }),
    )
}

fn get_project(w: &mut WorldState, r: &str) -> Response {
    let svc = w.service_mut(NAME);
    match get_doc(svc, "projects", r) {
        Some(p) => ok(project_out(&p)),
        None => project_not_found(),
    }
}

fn project_health(w: &mut WorldState, r: &str) -> Response {
    let svc = w.service_mut(NAME);
    let status = get_doc(svc, "projects", r).and_then(|p| p["status"].as_str().map(str::to_owned));
    let healthy = status.as_deref() == Some("ACTIVE_HEALTHY");
    let services: Vec<Value> = ["auth", "db", "pooler", "realtime", "rest", "storage"]
        .iter()
        .map(|s| json_value!({ "name": s, "healthy": healthy, "status": if healthy { "ACTIVE_HEALTHY" } else { "UNHEALTHY" } }))
        .collect();
    ok(Value::Array(services))
}

fn create_project(w: &mut WorldState, b: &Value) -> Response {
    let Some(name) = b["name"].as_str().filter(|n| !n.is_empty()) else {
        return msg(
            StatusCode::BAD_REQUEST,
            "name must be a string between 1 and 256 characters",
        );
    };
    let Some(org) = b["organization_slug"]
        .as_str()
        .or(b["organization_id"].as_str())
    else {
        return msg(StatusCode::BAD_REQUEST, "organization_slug is required");
    };
    let region = b["region"].as_str().unwrap_or("us-east-1").to_owned();
    if b["db_pass"].as_str().is_none_or(|p| p.len() < 8) {
        return msg(
            StatusCode::BAD_REQUEST,
            "db_pass must be at least 8 characters",
        );
    }
    let now = w.clock.now_rfc3339();
    let svc = w.service_mut(NAME);
    if get_doc(svc, "organizations", org).is_none() {
        return msg(StatusCode::NOT_FOUND, "Organization not found");
    }
    let r = lower_id(svc, "project_ref", 20);
    let project = json_value!({
        "id": r,
        "ref": r,
        "organization_id": org,
        "organization_slug": org,
        "name": name,
        "region": region,
        "created_at": now.replace('Z', ".000000Z"),
        "status": "ACTIVE_HEALTHY",
        "database": { "host": format!("db.{r}.supabase.co"), "version": "17.4.1.064", "postgres_engine": "17", "release_channel": "ga" },
    });
    svc.store
        .collection_mut("projects")
        .insert(r.clone(), project.clone());
    let mut response = project_out(&project);
    response["status"] = json_value!("COMING_UP");
    created(response)
}

fn delete_project(w: &mut WorldState, r: &str) -> Response {
    let svc = w.service_mut(NAME);
    let Some(p) = svc.store.collection_mut("projects").remove(r) else {
        return project_not_found();
    };
    ok(json_value!({ "id": p["id"], "ref": p["ref"], "name": p["name"] }))
}

fn set_status(w: &mut WorldState, r: &str, status: &str) {
    if let Some(p) = w
        .service_mut(NAME)
        .store
        .collection_mut("projects")
        .get_mut(r)
    {
        p["status"] = json_value!(status);
    }
}

fn status_of(w: &mut WorldState, r: &str) -> String {
    get_doc(w.service_mut(NAME), "projects", r)
        .and_then(|p| p["status"].as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn pause_project(w: &mut WorldState, r: &str) -> Response {
    match status_of(w, r).as_str() {
        "ACTIVE_HEALTHY" | "ACTIVE_UNHEALTHY" => {
            set_status(w, r, "INACTIVE");
            ok(json_value!({}))
        }
        "INACTIVE" | "PAUSING" => msg(StatusCode::BAD_REQUEST, "Project is already paused."),
        other => msg(
            StatusCode::BAD_REQUEST,
            &format!("Project cannot be paused while its status is {other}."),
        ),
    }
}

fn restore_project(w: &mut WorldState, r: &str) -> Response {
    match status_of(w, r).as_str() {
        "INACTIVE" => {
            set_status(w, r, "ACTIVE_HEALTHY");
            ok(json_value!({}))
        }
        _ => msg(StatusCode::BAD_REQUEST, "Project is not paused."),
    }
}

// ───────────────────────────── database ─────────────────────────────

fn require_active(w: &mut WorldState, r: &str) -> Option<Response> {
    match status_of(w, r).as_str() {
        "ACTIVE_HEALTHY" | "ACTIVE_UNHEALTHY" => None,
        "INACTIVE" => Some(msg(
            StatusCode::BAD_REQUEST,
            "Project is paused. Restore the project before running queries.",
        )),
        other => Some(msg(
            StatusCode::BAD_REQUEST,
            &format!("Project is not ready to accept queries (status: {other})."),
        )),
    }
}

fn load_db(w: &mut WorldState, r: &str) -> sql::Db {
    let now = w.clock.now_rfc3339();
    let svc = w.service_mut(NAME);
    db::load(&svc.store, &svc.ids, r, &now)
}

fn save_db(w: &mut WorldState, r: &str, d: &sql::Db) {
    let svc = w.service_mut(NAME);
    let mut ids = svc.ids.clone();
    db::store(&mut svc.store, &mut ids, r, d);
    svc.ids = ids;
}

/// Runs SQL against project `r`, committing on success (all-or-nothing).
fn run_sql(
    w: &mut WorldState,
    r: &str,
    query: &str,
    params: &[Value],
    read_only: bool,
) -> Result<Vec<Value>, sql::SqlError> {
    let mut d = load_db(w, r);
    let lower = query.to_lowercase();
    if lower.contains("relrowsecurity") && lower.contains("pg_namespace") {
        return Ok(db::introspect_tables(
            &d,
            params,
            lower.contains("where schema not in"),
        ));
    }
    if lower.contains("pg_available_extensions()") {
        return Ok(db::introspect_extensions(&d));
    }
    if lower.contains("supabase_migrations.schema_migrations") {
        let svc = w.service_mut(NAME);
        let rows = docs_with_prefix(svc, "migrations", &format!("{r}/"))
            .into_iter()
            .map(|(_, m)| json_value!({ "version": m["version"], "name": m["name"], "statements": m.get("statements").cloned().unwrap_or(json_value!([])) }))
            .collect();
        return Ok(rows);
    }
    let rows = sql::execute(&mut d, query, params, read_only)?;
    if d.dirty {
        save_db(w, r, &d);
    } else {
        w.service_mut(NAME).ids = d.ids.clone();
    }
    Ok(rows)
}

fn run_query(w: &mut WorldState, r: &str, b: &Value) -> Response {
    if let Some(resp) = require_active(w, r) {
        return resp;
    }
    let Some(query) = b["query"].as_str() else {
        return msg(StatusCode::BAD_REQUEST, "query must be a string");
    };
    let params = b["parameters"].as_array().cloned().unwrap_or_default();
    let read_only = b["read_only"].as_bool().unwrap_or(false);
    match run_sql(w, r, query, &params, read_only) {
        Ok(rows) => created(Value::Array(rows)),
        Err(e) => msg(StatusCode::BAD_REQUEST, &e.api_message()),
    }
}

fn list_migrations(w: &mut WorldState, r: &str) -> Response {
    let svc = w.service_mut(NAME);
    let mut ms: Vec<Value> = docs_with_prefix(svc, "migrations", &format!("{r}/"))
        .into_iter()
        .map(|(_, m)| json_value!({ "version": m["version"], "name": m["name"] }))
        .collect();
    ms.sort_by(|a, b| a["version"].as_str().cmp(&b["version"].as_str()));
    ok(Value::Array(ms))
}

fn apply_migration(w: &mut WorldState, r: &str, b: &Value) -> Response {
    if let Some(resp) = require_active(w, r) {
        return resp;
    }
    let (Some(query), name) = (b["query"].as_str(), b["name"].as_str()) else {
        return msg(StatusCode::BAD_REQUEST, "query must be a string");
    };
    if let Err(e) = run_sql(w, r, query, &[], false) {
        return msg(StatusCode::BAD_REQUEST, &e.api_message());
    }
    let version = compact_time(&w.clock.now_rfc3339());
    let doc = json_value!({ "version": version, "name": name, "statements": [query] });
    w.service_mut(NAME)
        .store
        .collection_mut("migrations")
        .insert(format!("{r}/{version}"), doc);
    ok(json_value!({}))
}

// ───────────────────────────── logs & advisors ─────────────────────────────

fn logs(w: &mut WorldState, r: &str, q: &HashMap<String, String>) -> Response {
    use sql::value::{format_tstz, parse_ts};
    let Some(query) = q.get("sql").filter(|s| !s.trim().is_empty()) else {
        return msg(StatusCode::BAD_REQUEST, "sql is required");
    };
    let now = parse_ts(&w.clock.now_rfc3339()).expect("clock time parses");
    let mut end = q
        .get("iso_timestamp_end")
        .and_then(|s| parse_ts(s))
        .unwrap_or(now);
    let mut start = q
        .get("iso_timestamp_start")
        .and_then(|s| parse_ts(s))
        .unwrap_or(end - time::Duration::days(1));
    // Windows in the world's future (e.g. a wall-clock default) are shifted to end at
    // the world's now, keeping their length.
    if end > now {
        let len = end - start;
        end = now;
        start = now - len;
    }
    let svc = w.service_mut(NAME);
    let entries: Vec<Value> = docs_with_prefix(svc, "logs", "")
        .into_iter()
        .map(|(_, l)| l)
        .filter(|l| l["project"] == r)
        .filter(|l| {
            l["timestamp"]
                .as_str()
                .and_then(parse_ts)
                .is_some_and(|t| t >= start && t <= end)
        })
        .collect();
    let columns = [
        "id",
        "timestamp",
        "event_message",
        "source",
        "severity_text",
        "log_attributes",
    ];
    let def = sql::TableDef {
        id: 1,
        schema: "public".into(),
        name: "logs".into(),
        rls_enabled: false,
        rls_forced: false,
        comment: None,
        columns: columns
            .iter()
            .map(|c| sql::ColumnInfo {
                name: (*c).to_owned(),
                format: if *c == "log_attributes" {
                    "jsonb".into()
                } else {
                    "text".into()
                },
                nullable: true,
                default: None,
                identity: None,
                unique: false,
                enums: Vec::new(),
                check: None,
                comment: None,
            })
            .collect(),
        primary_keys: Vec::new(),
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        policies: Vec::new(),
    };
    let rows = entries
        .iter()
        .map(|l| {
            columns
                .iter()
                .map(|c| match *c {
                    "log_attributes" => l.get(*c).cloned().unwrap_or(json_value!({})),
                    "severity_text" => l.get(*c).cloned().unwrap_or(json_value!("INFO")),
                    _ => l.get(*c).cloned().unwrap_or(Value::Null),
                })
                .collect()
        })
        .collect();
    let mut d = sql::Db {
        tables: vec![sql::Table { def, rows }],
        extensions: Vec::new(),
        now: format_tstz(now),
        ids: svc.ids.clone(),
        next_oid: 2,
        dirty: false,
    };
    match sql::execute(&mut d, query, &[], true) {
        Ok(rows) => ok(json_value!({ "result": rows })),
        Err(e) => ok(json_value!({
            "result": [],
            "error": { "code": 400, "message": e.message, "status": "INVALID_ARGUMENT", "errors": [{ "domain": "global", "reason": "invalidQuery", "message": e.message }] },
        })),
    }
}

fn strip_advisor(mut l: Value) -> Value {
    if let Some(o) = l.as_object_mut() {
        o.shift_remove("project");
        o.shift_remove("type");
    }
    l
}

fn advisors(w: &mut WorldState, r: &str, kind: &str) -> Response {
    if status_of(w, r) == "INACTIVE" {
        return msg(StatusCode::BAD_REQUEST, "Project is paused.");
    }
    let d = load_db(w, r);
    let svc = w.service_mut(NAME);
    let mut lints: Vec<Value> = docs_with_prefix(svc, "advisors", "")
        .into_iter()
        .map(|(_, l)| l)
        .filter(|l| l["project"] == r && l["type"] == kind)
        .map(strip_advisor)
        .collect();
    lints.extend(db::computed_lints(&d, r, kind));
    ok(json_value!({ "lints": lints }))
}

fn v2_route(
    w: &mut WorldState,
    m: &str,
    r: &str,
    rest: &[&str],
    q: &HashMap<String, String>,
    body: &Bytes,
) -> Response {
    let v2_err = |status: StatusCode, code: &str, message: &str| {
        json(
            status,
            json_value!({ "error": { "code": code, "message": message } }),
        )
    };
    match (m, rest) {
        ("POST", ["advisors", "run"]) => {
            let b = parse_body(body).unwrap_or(Value::Null);
            let requested: Vec<String> = b["data"]["attributes"]["lints"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|l| l["name"].as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let active = status_of(w, r) == "ACTIVE_HEALTHY";
            let svc = w.service_mut(NAME);
            let found: Vec<Value> = docs_with_prefix(svc, "advisors", "")
                .into_iter()
                .map(|(_, l)| l)
                .filter(|l| l["project"] == r && l["type"] == "health")
                .map(strip_advisor)
                .collect();
            let mut lints: Vec<Value> = Vec::new();
            if !active {
                lints.push(json_value!({ "name": "project_not_active", "level": "WARN", "status": "skipped", "detail": "Project is not active." }));
            }
            for name in &requested {
                match found.iter().find(|l| l["name"] == name.as_str()) {
                    Some(l) => lints.push(l.clone()),
                    None => lints.push(json_value!({
                        "name": name,
                        "level": "INFO",
                        "status": "pass",
                        "detail": "No issues detected in the last 24 hours.",
                    })),
                }
            }
            ok(
                json_value!({ "data": { "id": format!("{r}-advisors"), "type": "project_advisors", "attributes": { "lints": lints } } }),
            )
        }
        ("GET", ["notebooks"]) => {
            let svc = w.service_mut(NAME);
            let mut all: Vec<Value> = docs_with_prefix(svc, "notebooks", &format!("{r}/"))
                .into_iter()
                .map(|(_, n)| n)
                .collect();
            let size = q
                .get("page[size]")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(25)
                .clamp(1, 100);
            let start = q
                .get("page[after]")
                .and_then(|a| {
                    all.iter()
                        .position(|n| n["id"] == a.as_str())
                        .map(|i| i + 1)
                })
                .unwrap_or(0);
            let total = all.len();
            let page: Vec<Value> = all.drain(..).skip(start).take(size).collect();
            let next = (start + page.len() < total).then(|| {
                format!(
                    "/v2/projects/{r}/notebooks?page[size]={size}&page[after]={}",
                    page.last().and_then(|n| n["id"].as_str()).unwrap_or("")
                )
            });
            let data: Vec<Value> = page
                .iter()
                .map(|n| {
                    let mut attrs = n.clone();
                    if let Some(o) = attrs.as_object_mut() {
                        o.shift_remove("id");
                        o.shift_remove("content");
                    }
                    json_value!({ "id": n["id"], "type": "notebook", "attributes": attrs })
                })
                .collect();
            ok(json_value!({ "data": data, "links": { "next": next } }))
        }
        ("GET", ["notebooks", id]) => {
            let svc = w.service_mut(NAME);
            match get_doc(svc, "notebooks", &format!("{r}/{id}")) {
                Some(n) => {
                    let mut attrs = n.clone();
                    if let Some(o) = attrs.as_object_mut() {
                        o.shift_remove("id");
                    }
                    ok(
                        json_value!({ "data": { "id": n["id"], "type": "notebook", "attributes": attrs } }),
                    )
                }
                None => v2_err(StatusCode::NOT_FOUND, "not_found", "Notebook not found"),
            }
        }
        ("POST", ["notebooks"]) => {
            let b = parse_body(body).unwrap_or(Value::Null);
            let attrs = &b["data"]["attributes"];
            let Some(name) = attrs["name"].as_str() else {
                return v2_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "data.attributes.name is required",
                );
            };
            let now = w.clock.now_rfc3339();
            let svc = w.service_mut(NAME);
            let id = svc.ids.next_uuid("notebook").to_string();
            let cells: Vec<Value> = attrs["cells"]
                .as_array()
                .or(attrs["content"]["cells"].as_array())
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .enumerate()
                .map(|(i, mut c)| {
                    c["id"] = json_value!(format!("cell-{}", i + 1));
                    c
                })
                .collect();
            let owner = json_value!({ "id": 1, "username": "octo-platform" });
            let doc = json_value!({
                "id": id,
                "name": name,
                "description": attrs.get("description").cloned().unwrap_or(Value::Null),
                "favorite": false,
                "inserted_at": now,
                "updated_at": now,
                "owner": owner,
                "updated_by": owner,
                "content": { "schema_version": 1, "cells": cells },
            });
            svc.store
                .collection_mut("notebooks")
                .insert(format!("{r}/{id}"), doc.clone());
            let mut out = doc.clone();
            if let Some(o) = out.as_object_mut() {
                o.shift_remove("id");
            }
            created(json_value!({ "data": { "id": id, "type": "notebook", "attributes": out } }))
        }
        _ => v2_err(StatusCode::NOT_FOUND, "not_found", "Not Found"),
    }
}

// ───────────────────────────── development ─────────────────────────────

fn legacy_jwt(r: &str, role: &str) -> String {
    let header = b64url(br#"{"alg":"HS256","typ":"JWT"}"#);
    let claims = b64url(
        format!(
            r#"{{"iss":"supabase","ref":"{r}","role":"{role}","iat":1735689600,"exp":2051265600}}"#
        )
        .as_bytes(),
    );
    let sig = b64url(&Sha256::digest(
        format!("{header}.{claims}.{role}").as_bytes(),
    ));
    format!("{header}.{claims}.{sig}")
}

fn default_keys(r: &str) -> Vec<Value> {
    let h = digest_hex(r);
    vec![
        json_value!({ "name": "anon", "api_key": legacy_jwt(r, "anon"), "id": "anon", "type": "legacy", "hash": "", "prefix": "", "description": "Legacy anon API key" }),
        json_value!({ "name": "service_role", "api_key": legacy_jwt(r, "service_role"), "id": "service_role", "type": "legacy", "hash": "", "prefix": "", "description": "Legacy service_role API key" }),
        json_value!({ "name": "default", "api_key": format!("sb_publishable_{}", &h[..22]), "id": format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32]), "type": "publishable", "prefix": "sb_publishable_", "description": "Default publishable key" }),
        json_value!({ "name": "default", "api_key": format!("sb_secret_{}", &h[32..54]), "id": format!("{}-{}-{}-{}-{}", &h[32..40], &h[40..44], &h[44..48], &h[48..52], &h[52..64]), "type": "secret", "prefix": "sb_secret_", "description": "Default secret key" }),
    ]
}

fn api_keys(w: &mut WorldState, r: &str, reveal: bool) -> Response {
    let svc = w.service_mut(NAME);
    let stored = docs_with_prefix(svc, "api_keys", &format!("{r}/"));
    let keys: Vec<Value> = if stored.is_empty() {
        default_keys(r)
    } else {
        stored.into_iter().map(|(_, k)| k).collect()
    };
    let keys = keys
        .into_iter()
        .map(|mut k| {
            if k["type"] == "secret"
                && !reveal
                && let Some(s) = k["api_key"].as_str()
            {
                let masked = format!("{}{}", &s[..s.len().min(14)], "\u{2022}".repeat(6));
                k["api_key"] = json_value!(masked);
            }
            k
        })
        .collect();
    ok(Value::Array(keys))
}

fn typescript(w: &mut WorldState, r: &str) -> Response {
    if let Some(resp) = require_active(w, r) {
        return resp;
    }
    let d = load_db(w, r);
    ok(json_value!({ "types": db::typescript(&d) }))
}

// ───────────────────────────── edge functions ─────────────────────────────

fn function_out(r: &str, f: &Value) -> Value {
    let mut out = f.clone();
    let id = f["id"].as_str().unwrap_or_default();
    let version = f["version"].as_i64().unwrap_or(1);
    for key in ["entrypoint_path", "import_map_path"] {
        if let Some(p) = f[key].as_str()
            && !p.starts_with("file://")
        {
            out[key] = json_value!(format!(
                "file:///tmp/user_fn_{r}_{id}_{version}/source/{}",
                p.trim_start_matches("./")
            ));
        }
    }
    out
}

fn list_functions(w: &mut WorldState, r: &str) -> Response {
    let svc = w.service_mut(NAME);
    let fs: Vec<Value> = docs_with_prefix(svc, "functions", &format!("{r}/"))
        .into_iter()
        .map(|(_, f)| function_out(r, &f))
        .collect();
    ok(Value::Array(fs))
}

fn get_function(w: &mut WorldState, r: &str, slug: &str) -> Response {
    let svc = w.service_mut(NAME);
    match get_doc(svc, "functions", &format!("{r}/{slug}")) {
        Some(f) => ok(function_out(r, &f)),
        None => msg(StatusCode::NOT_FOUND, "Function not found"),
    }
}

fn delete_function(w: &mut WorldState, r: &str, slug: &str) -> Response {
    let svc = w.service_mut(NAME);
    let key = format!("{r}/{slug}");
    if svc.store.collection_mut("functions").remove(&key).is_none() {
        return msg(StatusCode::NOT_FOUND, "Function not found");
    }
    svc.store.collection_mut("function_bodies").remove(&key);
    ok(json_value!({}))
}

fn function_body(w: &mut WorldState, r: &str, slug: &str) -> Response {
    let svc = w.service_mut(NAME);
    let key = format!("{r}/{slug}");
    let Some(f) = get_doc(svc, "functions", &key) else {
        return msg(StatusCode::NOT_FOUND, "Function not found");
    };
    let files = get_doc(svc, "function_bodies", &key)
        .and_then(|b| b["files"].as_array().cloned())
        .unwrap_or_default();
    let mut parts = vec![multipart::Part {
        name: "metadata".into(),
        filename: None,
        content_type: Some("application/json".into()),
        data: json_value!({
            "deno2_entrypoint_path": f["entrypoint_path"],
            "entrypoint_path": f["entrypoint_path"],
            "import_map_path": f.get("import_map_path").cloned().unwrap_or(Value::Null),
            "name": f["slug"],
            "verify_jwt": f.get("verify_jwt").cloned().unwrap_or(json_value!(true)),
        })
        .to_string()
        .into_bytes(),
    }];
    for file in files {
        parts.push(multipart::Part {
            name: "file".into(),
            filename: Some(format!(
                "source/{}",
                file["name"].as_str().unwrap_or("index.ts")
            )),
            content_type: Some("application/typescript".into()),
            data: file["content"]
                .as_str()
                .unwrap_or_default()
                .as_bytes()
                .to_vec(),
        });
    }
    let boundary = "----toolbenchEdgeFunctionBoundary";
    let body = multipart::encode(&parts, boundary);
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )],
        Body::from(body),
    )
        .into_response()
}

fn deploy_function(
    w: &mut WorldState,
    r: &str,
    q: &HashMap<String, String>,
    headers: &HeaderMap,
    body: &Bytes,
) -> Response {
    let ct = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let Some(boundary) = multipart::boundary(ct).filter(|_| ct.starts_with("multipart/form-data"))
    else {
        return msg(StatusCode::BAD_REQUEST, "Expected multipart/form-data body");
    };
    let parts = multipart::parse(body, &boundary);
    let Some(meta) = parts
        .iter()
        .find(|p| p.name == "metadata")
        .and_then(|p| serde_json::from_slice::<Value>(&p.data).ok())
    else {
        return msg(StatusCode::BAD_REQUEST, "metadata is required");
    };
    let files: Vec<Value> = parts
        .iter()
        .filter(|p| p.name == "file")
        .map(|p| {
            json_value!({
                "name": p.filename.clone().unwrap_or_else(|| "index.ts".to_owned()),
                "content": String::from_utf8_lossy(&p.data),
            })
        })
        .collect();
    let Some(slug) = q
        .get("slug")
        .cloned()
        .or_else(|| meta["name"].as_str().map(str::to_owned))
    else {
        return msg(StatusCode::BAD_REQUEST, "slug is required");
    };
    let entrypoint = meta["entrypoint_path"]
        .as_str()
        .unwrap_or("index.ts")
        .to_owned();
    if !files.iter().any(|f| f["name"] == entrypoint.as_str()) {
        return msg(
            StatusCode::BAD_REQUEST,
            &format!("Entrypoint path {entrypoint} not found in uploaded files"),
        );
    }
    let ms = unix_ms(w);
    let svc = w.service_mut(NAME);
    let key = format!("{r}/{slug}");
    let existing = get_doc(svc, "functions", &key);
    let id = existing
        .as_ref()
        .and_then(|f| f["id"].as_str().map(str::to_owned))
        .unwrap_or_else(|| svc.ids.next_uuid("function").to_string());
    let version = existing
        .as_ref()
        .and_then(|f| f["version"].as_i64())
        .unwrap_or(0)
        + 1;
    let created_at = existing
        .as_ref()
        .and_then(|f| f["created_at"].as_i64())
        .unwrap_or(ms);
    let all_content: String = files.iter().filter_map(|f| f["content"].as_str()).collect();
    let doc = json_value!({
        "id": id,
        "slug": slug,
        "name": slug,
        "status": "ACTIVE",
        "version": version,
        "created_at": created_at,
        "updated_at": ms,
        "verify_jwt": meta.get("verify_jwt").and_then(Value::as_bool).unwrap_or(true),
        "import_map": meta["import_map_path"].is_string(),
        "entrypoint_path": entrypoint,
        "import_map_path": meta.get("import_map_path").cloned().unwrap_or(Value::Null),
        "ezbr_sha256": digest_hex(&all_content),
    });
    let mut doc = doc;
    if doc["import_map_path"].is_null()
        && let Some(o) = doc.as_object_mut()
    {
        o.shift_remove("import_map_path");
    }
    svc.store
        .collection_mut("functions")
        .insert(key.clone(), doc.clone());
    svc.store
        .collection_mut("function_bodies")
        .insert(key, json_value!({ "files": files }));
    created(function_out(r, &doc))
}

// ───────────────────────────── branching ─────────────────────────────

fn list_branches(w: &mut WorldState, r: &str) -> Response {
    let svc = w.service_mut(NAME);
    let enabled = get_doc(svc, "projects", r)
        .and_then(|p| p["branching_enabled"].as_bool())
        .unwrap_or(false);
    let branches: Vec<Value> = docs_with_prefix(svc, "branches", "")
        .into_iter()
        .map(|(_, b)| b)
        .filter(|b| b["parent_project_ref"] == r)
        .collect();
    if !enabled && branches.is_empty() {
        return msg(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Preview branching is not enabled for this project.",
        );
    }
    ok(Value::Array(branches))
}

fn copy_schema(w: &mut WorldState, from: &str, to: &str, upto: Option<&str>) {
    let now = w.clock.now_rfc3339();
    let svc = w.service_mut(NAME);
    let mut d = db::load(&svc.store, &svc.ids, from, &now);
    for t in &mut d.tables {
        t.rows.clear();
    }
    let mut ids = svc.ids.clone();
    db::store(&mut svc.store, &mut ids, to, &d);
    let migrations: Vec<(String, Value)> = docs_with_prefix(svc, "migrations", &format!("{from}/"));
    let existing: Vec<String> = docs_with_prefix(svc, "migrations", &format!("{to}/"))
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    for k in existing {
        svc.store.collection_mut("migrations").remove(&k);
    }
    for (_, m) in migrations {
        let v = m["version"].as_str().unwrap_or_default().to_owned();
        if upto.is_some_and(|u| v.as_str() > u) {
            continue;
        }
        svc.store
            .collection_mut("migrations")
            .insert(format!("{to}/{v}"), m);
    }
}

fn create_branch(w: &mut WorldState, r: &str, b: &Value) -> Response {
    let Some(name) = b["branch_name"].as_str().filter(|n| !n.is_empty()) else {
        return msg(StatusCode::BAD_REQUEST, "branch_name is required");
    };
    let now = w.clock.now_rfc3339();
    let parent = get_doc(w.service_mut(NAME), "projects", r).unwrap_or(Value::Null);
    if parent
        .get("parent_project_ref")
        .is_some_and(|p| !p.is_null())
    {
        return msg(
            StatusCode::BAD_REQUEST,
            "Cannot create a branch of a branch.",
        );
    }
    let has_branches = docs_with_prefix(w.service_mut(NAME), "branches", "")
        .iter()
        .any(|(_, br)| br["parent_project_ref"] == r);
    if docs_with_prefix(w.service_mut(NAME), "branches", "")
        .iter()
        .any(|(_, br)| br["parent_project_ref"] == r && br["name"] == name)
    {
        return msg(
            StatusCode::CONFLICT,
            &format!("A branch named {name} already exists."),
        );
    }
    if !has_branches {
        // The first branch enables branching: the production project becomes the default branch.
        let svc = w.service_mut(NAME);
        let id = svc.ids.next_uuid("branch").to_string();
        let main = json_value!({
            "id": id, "name": "main", "project_ref": r, "parent_project_ref": r, "is_default": true,
            "persistent": true, "status": "FUNCTIONS_DEPLOYED", "created_at": now, "updated_at": now,
        });
        svc.store.collection_mut("branches").insert(id, main);
    }
    let svc = w.service_mut(NAME);
    let id = svc.ids.next_uuid("branch").to_string();
    let branch_ref = lower_id(svc, "project_ref", 20);
    if let Some(p) = svc.store.collection_mut("projects").get_mut(r) {
        p["branching_enabled"] = json_value!(true);
    }
    let mut project = parent.clone();
    project["id"] = json_value!(branch_ref);
    project["ref"] = json_value!(branch_ref);
    project["name"] = json_value!(name);
    project["created_at"] = json_value!(now);
    project["status"] = json_value!("ACTIVE_HEALTHY");
    project["parent_project_ref"] = json_value!(r);
    if let Some(o) = project.as_object_mut() {
        o.shift_remove("branching_enabled");
    }
    svc.store
        .collection_mut("projects")
        .insert(branch_ref.clone(), project);
    let branch = json_value!({
        "id": id, "name": name, "project_ref": branch_ref, "parent_project_ref": r, "is_default": false,
        "persistent": false, "status": "CREATING_PROJECT", "created_at": now, "updated_at": now,
    });
    svc.store
        .collection_mut("branches")
        .insert(id.clone(), branch.clone());
    copy_schema(w, r, &branch_ref, None);
    if let Some(b) = w
        .service_mut(NAME)
        .store
        .collection_mut("branches")
        .get_mut(&id)
    {
        b["status"] = json_value!("FUNCTIONS_DEPLOYED");
    }
    created(branch)
}

fn find_branch(w: &mut WorldState, id: &str) -> Option<Value> {
    let svc = w.service_mut(NAME);
    get_doc(svc, "branches", id).or_else(|| {
        docs_with_prefix(svc, "branches", "")
            .into_iter()
            .map(|(_, b)| b)
            .find(|b| b["project_ref"] == id)
    })
}

fn get_branch(w: &mut WorldState, id: &str) -> Response {
    match find_branch(w, id) {
        Some(b) => ok(b),
        None => msg(StatusCode::NOT_FOUND, "Branch not found"),
    }
}

fn delete_branch(w: &mut WorldState, id: &str) -> Response {
    let Some(b) = find_branch(w, id) else {
        return msg(StatusCode::NOT_FOUND, "Branch not found");
    };
    if b["is_default"] == true {
        return msg(StatusCode::BAD_REQUEST, "Cannot delete the default branch.");
    }
    let branch_id = b["id"].as_str().unwrap_or_default().to_owned();
    let branch_ref = b["project_ref"].as_str().unwrap_or_default().to_owned();
    let now = w.clock.now_rfc3339();
    let svc = w.service_mut(NAME);
    svc.store.collection_mut("branches").remove(&branch_id);
    svc.store.collection_mut("projects").remove(&branch_ref);
    let empty = sql::Db {
        tables: Vec::new(),
        extensions: Vec::new(),
        now,
        ids: svc.ids.clone(),
        next_oid: 0,
        dirty: false,
    };
    let mut ids = svc.ids.clone();
    db::store(&mut svc.store, &mut ids, &branch_ref, &empty);
    ok(json_value!({ "message": "ok" }))
}

fn branch_action(w: &mut WorldState, id: &str, action: &str, b: &Value) -> Response {
    let Some(branch) = find_branch(w, id) else {
        return msg(StatusCode::NOT_FOUND, "Branch not found");
    };
    let branch_ref = branch["project_ref"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let parent = branch["parent_project_ref"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if branch["is_default"] == true {
        return msg(
            StatusCode::BAD_REQUEST,
            "Cannot run this action on the default branch.",
        );
    }
    let run_missing = |w: &mut WorldState, from: &str, to: &str| -> Result<(), Response> {
        let svc = w.service_mut(NAME);
        let have: Vec<String> = docs_with_prefix(svc, "migrations", &format!("{to}/"))
            .into_iter()
            .map(|(_, m)| m["version"].as_str().unwrap_or_default().to_owned())
            .collect();
        let mut missing: Vec<Value> = docs_with_prefix(svc, "migrations", &format!("{from}/"))
            .into_iter()
            .map(|(_, m)| m)
            .filter(|m| !have.contains(&m["version"].as_str().unwrap_or_default().to_owned()))
            .collect();
        missing.sort_by(|a, b| a["version"].as_str().cmp(&b["version"].as_str()));
        for m in missing {
            for stmt in m["statements"].as_array().cloned().unwrap_or_default() {
                if let Err(e) = run_sql(w, to, stmt.as_str().unwrap_or_default(), &[], false) {
                    return Err(msg(StatusCode::BAD_REQUEST, &e.api_message()));
                }
            }
            let v = m["version"].as_str().unwrap_or_default().to_owned();
            w.service_mut(NAME)
                .store
                .collection_mut("migrations")
                .insert(format!("{to}/{v}"), m);
        }
        Ok(())
    };
    let result = match action {
        "merge" => run_missing(w, &branch_ref, &parent),
        "push" => run_missing(w, &parent, &branch_ref),
        "reset" => {
            copy_schema(w, &parent, &branch_ref, b["migration_version"].as_str());
            Ok(())
        }
        _ => return not_found(),
    };
    if let Err(resp) = result {
        return resp;
    }
    let now = w.clock.now_rfc3339();
    let svc = w.service_mut(NAME);
    let workflow = svc.ids.next_uuid("workflow_run").to_string();
    if let Some(doc) = svc
        .store
        .collection_mut("branches")
        .get_mut(branch["id"].as_str().unwrap_or_default())
    {
        doc["updated_at"] = json_value!(now);
        doc["status"] = json_value!("FUNCTIONS_DEPLOYED");
    }
    let message = match action {
        "merge" => "Branch merged",
        "push" => "Branch pushed",
        _ => "Branch reset",
    };
    created(json_value!({ "workflow_run_id": workflow, "message": message }))
}

// ───────────────────────────── storage & secrets ─────────────────────────────

fn list_buckets(w: &mut WorldState, r: &str) -> Response {
    let svc = w.service_mut(NAME);
    ok(Value::Array(
        docs_with_prefix(svc, "buckets", &format!("{r}/"))
            .into_iter()
            .map(|(_, b)| b)
            .collect(),
    ))
}

fn storage_config(svc: &ServiceState, r: &str) -> Value {
    get_doc(svc, "storage_config", r).unwrap_or_else(|| {
        json_value!({
            "fileSizeLimit": FREE_FILE_SIZE_LIMIT,
            "features": {
                "imageTransformation": { "enabled": false },
                "s3Protocol": { "enabled": true },
            },
        })
    })
}

fn get_storage_config(w: &mut WorldState, r: &str) -> Response {
    let svc = w.service_mut(NAME);
    ok(storage_config(svc, r))
}

fn org_plan(svc: &ServiceState, r: &str) -> String {
    let org = get_doc(svc, "projects", r)
        .and_then(|p| p["organization_id"].as_str().map(str::to_owned))
        .unwrap_or_default();
    get_doc(svc, "organizations", &org)
        .and_then(|o| o["plan"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "free".to_owned())
}

fn update_storage_config(w: &mut WorldState, r: &str, b: &Value) -> Response {
    let svc = w.service_mut(NAME);
    let mut config = storage_config(svc, r);
    let plan = org_plan(svc, r);
    if let Some(limit) = b.get("fileSizeLimit") {
        let Some(n) = limit.as_u64() else {
            return msg(
                StatusCode::BAD_REQUEST,
                "fileSizeLimit must be a positive integer",
            );
        };
        if plan == "free" && n > FREE_FILE_SIZE_LIMIT {
            return msg(
                StatusCode::BAD_REQUEST,
                "The free plan does not allow a global file size limit above 50MB. Upgrade your plan to raise it.",
            );
        }
        config["fileSizeLimit"] = json_value!(n);
    }
    if let Some(features) = b.get("features").and_then(Value::as_object) {
        for (k, v) in features {
            if k == "imageTransformation" && v["enabled"] == true && plan == "free" {
                return msg(
                    StatusCode::BAD_REQUEST,
                    "Image transformations are only available on the Pro plan and above.",
                );
            }
            config["features"][k] = v.clone();
        }
    }
    svc.store
        .collection_mut("storage_config")
        .insert(r.to_owned(), config);
    ok(json_value!({}))
}

fn list_secrets(w: &mut WorldState, r: &str) -> Response {
    let svc = w.service_mut(NAME);
    ok(Value::Array(
        docs_with_prefix(svc, "secrets", &format!("{r}/"))
            .into_iter()
            .map(|(_, s)| s)
            .collect(),
    ))
}

fn create_secrets(w: &mut WorldState, r: &str, b: &Value) -> Response {
    let Some(items) = b.as_array() else {
        return msg(StatusCode::BAD_REQUEST, "Body must be an array of secrets");
    };
    let now = w.clock.now_rfc3339();
    let svc = w.service_mut(NAME);
    for s in items {
        let (Some(name), Some(value)) = (s["name"].as_str(), s["value"].as_str()) else {
            return msg(
                StatusCode::BAD_REQUEST,
                "Each secret needs a name and a value",
            );
        };
        if name.starts_with("SUPABASE_") {
            return msg(
                StatusCode::BAD_REQUEST,
                "Secret names must not start with SUPABASE_",
            );
        }
        svc.store.collection_mut("secrets").insert(
            format!("{r}/{name}"),
            json_value!({ "name": name, "value": digest_hex(value), "updated_at": now }),
        );
    }
    created(json_value!({}))
}

fn delete_secrets(w: &mut WorldState, r: &str, b: &Value) -> Response {
    let Some(names) = b.as_array() else {
        return msg(
            StatusCode::BAD_REQUEST,
            "Body must be an array of secret names",
        );
    };
    let svc = w.service_mut(NAME);
    for n in names.iter().filter_map(Value::as_str) {
        svc.store
            .collection_mut("secrets")
            .remove(&format!("{r}/{n}"));
    }
    ok(json_value!({}))
}
