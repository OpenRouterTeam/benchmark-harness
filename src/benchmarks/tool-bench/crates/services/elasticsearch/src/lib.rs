//! Mock Elasticsearch 9 cluster, as called by `elasticsearch-core-mcp-server` 0.4.6
//! (`ES_URL` points at the world). Endpoints:
//!
//! - `GET /` cluster info; `GET /_cluster/health`
//! - `GET /_cat/indices[/{pattern}]`, `GET /_cat/shards[/{index}]` (`format=json` or text, `h=`, `v`)
//! - `GET /{index}/_mapping`, `GET /_mapping`
//! - `GET|POST /{index}/_search`, `/_search` (query DSL: see [`dsl`]; aggregations: see [`aggs`])
//! - `GET|POST /{index}/_count`
//! - `POST /_query` (ES|QL: see [`esql`])
//!
//! Every response carries `X-Elastic-Product: Elasticsearch`. Requests need an
//! `Authorization` header (`ApiKey ...`, `Basic ...` or `Bearer ...`).
//!
//! Dataset collections:
//!
//! | Collection | Key | Content |
//! |---|---|---|
//! | `indices` | index name | `mappings` (`{"properties": {...}}`, every field with a `type`), optional `aliases`, `health`, `status`, `uuid`, `pri`, `rep`, `store_size` |
//! | `docs` | `<index>/<id>` | the document `_source` |
//!
//! Documents are returned in dataset order among equal scores; scoring is a
//! deterministic BM25-style formula, so the same query always ranks the same way.

// Handlers return early with a ready `Response` as the error; boxing it buys nothing here.
#![allow(clippy::result_large_err)]

pub mod aggs;
pub mod dsl;
pub mod error;
pub mod esql;
pub mod model;
pub mod search;

use axum::{
    Router,
    body::Bytes,
    extract::{Path, RawQuery, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use mock_core::{http::json, service::MockService, world::SharedWorld};
use serde_json::{Map, Value, json as json_value};

use crate::{
    dsl::Ctx,
    error::EsError,
    model::{Doc, Index, resolve},
};

pub const NAME: &str = "elasticsearch";
pub const PORT: u16 = 8106;
pub const CLUSTER_NAME: &str = "octo-observability";
pub const CLUSTER_UUID: &str = "Xq3f9P2bT8u1mKc4vLr0Aw";

pub struct Elasticsearch;

impl MockService for Elasticsearch {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port(&self) -> u16 {
        PORT
    }

    fn hosts(&self) -> &'static [&'static str] {
        &[]
    }

    fn router(&self) -> Router<SharedWorld> {
        Router::new()
            .route("/", get(root).head(root))
            .route("/_cluster/health", get(cluster_health))
            .route("/_cat/indices", get(cat_indices_all))
            .route("/_cat/indices/{pattern}", get(cat_indices))
            .route("/_cat/shards", get(cat_shards_all))
            .route("/_cat/shards/{index}", get(cat_shards))
            .route("/_mapping", get(mapping_all))
            .route("/{index}/_mapping", get(mapping))
            .route("/_search", get(search_all).post(search_all))
            .route("/{index}/_search", get(search_index).post(search_index))
            .route("/{index}/_count", get(count_index).post(count_index))
            .route("/_query", post(esql_query))
            .fallback(no_handler)
    }
}

/// JSON with the headers every Elasticsearch response carries.
pub fn respond(status: StatusCode, body: Value) -> Response {
    let mut resp = json(status, body);
    resp.headers_mut().insert(
        "x-elastic-product",
        HeaderValue::from_static("Elasticsearch"),
    );
    resp
}

fn text(status: StatusCode, body: String) -> Response {
    let mut resp = (
        status,
        [(header::CONTENT_TYPE, "text/plain; charset=UTF-8")],
        body,
    )
        .into_response();
    resp.headers_mut().insert(
        "x-elastic-product",
        HeaderValue::from_static("Elasticsearch"),
    );
    resp
}

async fn no_handler(method: Method, uri: axum::http::Uri) -> Response {
    respond(
        StatusCode::BAD_REQUEST,
        json_value!({ "error": format!("no handler found for uri [{}] and method [{method}]", uri.path()), "status": 400 }),
    )
}

fn authorize(headers: &HeaderMap, path: &str) -> Result<(), Response> {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if ["ApiKey ", "Basic ", "Bearer "]
        .iter()
        .any(|p| auth.starts_with(p) && auth.len() > p.len())
    {
        return Ok(());
    }
    let err = EsError::new(
        StatusCode::UNAUTHORIZED,
        "security_exception",
        format!("missing authentication credentials for REST request [{path}]"),
    )
    .with(
        "header",
        json_value!({ "WWW-Authenticate": ["Basic realm=\"security\", charset=\"UTF-8\"", "ApiKey"] }),
    );
    let mut resp = err.response();
    resp.headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("ApiKey"));
    Err(resp)
}

/// Indices (with documents) and the logical time in millis.
fn snapshot(world: &SharedWorld) -> (Vec<Index>, i64) {
    let w = world.lock().expect("world lock");
    let now_ms = w.clock.now_unix() * 1000;
    let Some(store) = w.service(NAME).map(|s| &s.store) else {
        return (Vec::new(), now_ms);
    };
    let mut indices: Vec<Index> = store
        .collection("indices")
        .map(|c| {
            c.iter()
                .map(|(name, meta)| Index {
                    name: name.to_owned(),
                    meta: meta.clone(),
                    docs: Vec::new(),
                })
                .collect()
        })
        .unwrap_or_default();
    if let Some(docs) = store.collection("docs") {
        for (key, source) in docs.iter() {
            let (index, id) = key.rsplit_once('/').unwrap_or((key, key));
            if let Some(i) = indices.iter_mut().find(|i| i.name == index) {
                i.docs.push(Doc {
                    index: index.to_owned(),
                    id: id.to_owned(),
                    source: source.clone(),
                });
            }
        }
    }
    (indices, now_ms)
}

fn params(raw: Option<&str>) -> Vec<(String, String)> {
    raw.unwrap_or_default()
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (decode(k), decode(v))
        })
        .collect()
}

fn param<'a>(ps: &'a [(String, String)], key: &str) -> Option<&'a str> {
    ps.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
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

macro_rules! tryr {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(r) => return r,
        }
    };
}

// ─────────────────────────────── cluster ───────────────────────────────

async fn root(headers: HeaderMap) -> Response {
    tryr!(authorize(&headers, "/"));
    respond(
        StatusCode::OK,
        json_value!({
            "name": "es-hot-01",
            "cluster_name": CLUSTER_NAME,
            "cluster_uuid": CLUSTER_UUID,
            "version": {
                "number": "9.1.5",
                "build_flavor": "default",
                "build_type": "docker",
                "build_hash": "90ee222e7e0136dd8ddbb34015538f3a00c129b7",
                "build_date": "2026-09-23T21:13:57.452791163Z",
                "build_snapshot": false,
                "lucene_version": "10.2.2",
                "minimum_wire_compatibility_version": "8.19.0",
                "minimum_index_compatibility_version": "8.0.0"
            },
            "tagline": "You Know, for Search"
        }),
    )
}

async fn cluster_health(State(world): State<SharedWorld>, headers: HeaderMap) -> Response {
    tryr!(authorize(&headers, "/_cluster/health"));
    let (indices, _) = snapshot(&world);
    let shards: u64 = indices
        .iter()
        .map(|i| i.meta["pri"].as_u64().unwrap_or(1))
        .sum();
    let status = if indices.iter().any(|i| i.meta["health"] == "red") {
        "red"
    } else if indices.iter().any(|i| i.meta["health"] == "yellow") {
        "yellow"
    } else {
        "green"
    };
    respond(
        StatusCode::OK,
        json_value!({
            "cluster_name": CLUSTER_NAME, "status": status, "timed_out": false,
            "number_of_nodes": 3, "number_of_data_nodes": 3,
            "active_primary_shards": shards, "active_shards": shards * 2,
            "relocating_shards": 0, "initializing_shards": 0, "unassigned_shards": 0,
            "delayed_unassigned_shards": 0, "number_of_pending_tasks": 0,
            "number_of_in_flight_fetch": 0, "task_max_waiting_in_queue_millis": 0,
            "active_shards_percent_as_number": 100.0
        }),
    )
}

// ───────────────────────────────── _cat ─────────────────────────────────

/// Renders `_cat` rows as JSON (`format=json`) or an aligned text table (`v` adds headers).
fn cat_response(
    rows: Vec<Vec<(String, Value)>>,
    ps: &[(String, String)],
    default_cols: &[&str],
) -> Response {
    let wanted: Vec<String> = match param(ps, "h") {
        Some(h) if !h.is_empty() => h.split(',').map(|s| s.trim().to_owned()).collect(),
        _ => default_cols.iter().map(|s| (*s).to_owned()).collect(),
    };
    let project = |row: &Vec<(String, Value)>| -> Vec<(String, Value)> {
        wanted
            .iter()
            .map(|c| {
                let v = row
                    .iter()
                    .find(|(k, _)| k == c)
                    .map_or(Value::Null, |(_, v)| v.clone());
                (c.clone(), v)
            })
            .collect()
    };
    let mut rows: Vec<Vec<(String, Value)>> = rows.iter().map(project).collect();
    if let Some(s) = param(ps, "s") {
        for key in s.split(',').rev() {
            let (field, desc) = match key.split_once(':') {
                Some((f, d)) => (f.to_owned(), d == "desc"),
                None => (key.to_owned(), false),
            };
            rows.sort_by(|a, b| {
                let get = |r: &Vec<(String, Value)>| {
                    r.iter()
                        .find(|(k, _)| *k == field)
                        .map(|(_, v)| v.clone())
                        .unwrap_or(Value::Null)
                };
                let (x, y) = (get(a), get(b));
                let o = match (
                    x.as_str().and_then(|s| s.parse::<f64>().ok()),
                    y.as_str().and_then(|s| s.parse::<f64>().ok()),
                ) {
                    (Some(p), Some(q)) => p.total_cmp(&q),
                    _ => model::scalar_string(&x).cmp(&model::scalar_string(&y)),
                };
                if desc { o.reverse() } else { o }
            });
        }
    }
    if param(ps, "format") == Some("json") {
        let out: Vec<Value> = rows
            .into_iter()
            .map(|r| Value::Object(r.into_iter().collect::<Map<_, _>>()))
            .collect();
        return respond(StatusCode::OK, Value::Array(out));
    }
    let mut table: Vec<Vec<String>> = Vec::new();
    if ps.iter().any(|(k, v)| k == "v" && v != "false") {
        table.push(wanted.clone());
    }
    for r in &rows {
        table.push(r.iter().map(|(_, v)| model::scalar_string(v)).collect());
    }
    let widths: Vec<usize> = (0..wanted.len())
        .map(|i| table.iter().map(|r| r[i].len()).max().unwrap_or(0))
        .collect();
    let mut out = String::new();
    for r in &table {
        let line: Vec<String> = r
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{c:<w$}", w = widths[i]))
            .collect();
        out.push_str(line.join(" ").trim_end());
        out.push('\n');
    }
    text(StatusCode::OK, out)
}

fn index_rows(indices: &[&Index]) -> Vec<Vec<(String, Value)>> {
    indices
        .iter()
        .map(|i| {
            let m = &i.meta;
            let s = |k: &str, d: &str| Value::String(m[k].as_str().unwrap_or(d).to_owned());
            let n = |k: &str, d: u64| Value::String(m[k].as_u64().unwrap_or(d).to_string());
            let size = m["store_size"]
                .as_str()
                .map_or_else(|| format!("{}kb", 4 + i.docs.len() * 2), str::to_owned);
            vec![
                ("health".into(), s("health", "green")),
                ("status".into(), s("status", "open")),
                ("index".into(), json_value!(i.name)),
                ("uuid".into(), s("uuid", "_na_")),
                ("pri".into(), n("pri", 1)),
                ("rep".into(), n("rep", 1)),
                ("docs.count".into(), json_value!(i.docs.len().to_string())),
                ("docs.deleted".into(), json_value!("0")),
                ("store.size".into(), json_value!(size)),
                ("pri.store.size".into(), json_value!(size)),
                ("dataset.size".into(), json_value!(size)),
            ]
        })
        .collect()
}

const INDEX_COLS: [&str; 10] = [
    "health",
    "status",
    "index",
    "uuid",
    "pri",
    "rep",
    "docs.count",
    "docs.deleted",
    "store.size",
    "pri.store.size",
];

async fn cat_indices_impl(
    world: SharedWorld,
    headers: HeaderMap,
    pattern: Option<String>,
    raw: Option<String>,
) -> Response {
    let path = format!(
        "/_cat/indices{}",
        pattern
            .as_ref()
            .map(|p| format!("/{p}"))
            .unwrap_or_default()
    );
    tryr!(authorize(&headers, &path));
    let (all, _) = snapshot(&world);
    let ps = params(raw.as_deref());
    let indices = match resolve(&all, pattern.as_deref().unwrap_or("*")) {
        Ok(i) => i,
        Err(e) => return e.response(),
    };
    let mut rows = index_rows(&indices);
    if let Some(h) = param(&ps, "health") {
        rows.retain(|r| r.iter().any(|(k, v)| k == "health" && v == h));
    }
    cat_response(rows, &ps, &INDEX_COLS)
}

async fn cat_indices_all(
    State(world): State<SharedWorld>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    cat_indices_impl(world, headers, None, raw).await
}

async fn cat_indices(
    State(world): State<SharedWorld>,
    Path(pattern): Path<String>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    cat_indices_impl(world, headers, Some(pattern), raw).await
}

const NODES: [&str; 3] = ["es-hot-01", "es-hot-02", "es-hot-03"];

async fn cat_shards_impl(
    world: SharedWorld,
    headers: HeaderMap,
    index: Option<String>,
    raw: Option<String>,
) -> Response {
    let path = format!(
        "/_cat/shards{}",
        index.as_ref().map(|p| format!("/{p}")).unwrap_or_default()
    );
    tryr!(authorize(&headers, &path));
    let (all, _) = snapshot(&world);
    let ps = params(raw.as_deref());
    let indices = match resolve(&all, index.as_deref().unwrap_or("*")) {
        Ok(i) => i,
        Err(e) => return e.response(),
    };
    let mut rows = Vec::new();
    for (n, i) in indices.iter().enumerate() {
        let pri = i.meta["pri"].as_u64().unwrap_or(1);
        let rep = i.meta["rep"].as_u64().unwrap_or(1);
        let unassigned_replicas = i.meta["health"] == "yellow";
        for shard in 0..pri {
            let docs = i.docs.len() as u64 / pri + u64::from(shard < i.docs.len() as u64 % pri);
            for copy in 0..=rep {
                let primary = copy == 0;
                let node_i = (n + shard as usize + copy as usize) % NODES.len();
                let unassigned = !primary && unassigned_replicas;
                rows.push(vec![
                    ("index".into(), json_value!(i.name)),
                    ("shard".into(), json_value!(shard.to_string())),
                    (
                        "prirep".into(),
                        json_value!(if primary { "p" } else { "r" }),
                    ),
                    (
                        "state".into(),
                        json_value!(if unassigned { "UNASSIGNED" } else { "STARTED" }),
                    ),
                    (
                        "docs".into(),
                        if unassigned {
                            Value::Null
                        } else {
                            json_value!(docs.to_string())
                        },
                    ),
                    (
                        "store".into(),
                        if unassigned {
                            Value::Null
                        } else {
                            json_value!(format!("{}kb", 2 + docs * 2))
                        },
                    ),
                    (
                        "dataset".into(),
                        if unassigned {
                            Value::Null
                        } else {
                            json_value!(format!("{}kb", 2 + docs * 2))
                        },
                    ),
                    (
                        "ip".into(),
                        if unassigned {
                            Value::Null
                        } else {
                            json_value!(format!("10.42.0.{}", 11 + node_i))
                        },
                    ),
                    (
                        "node".into(),
                        if unassigned {
                            Value::Null
                        } else {
                            json_value!(NODES[node_i])
                        },
                    ),
                ]);
            }
        }
    }
    cat_response(
        rows,
        &ps,
        &[
            "index", "shard", "prirep", "state", "docs", "store", "dataset", "ip", "node",
        ],
    )
}

async fn cat_shards_all(
    State(world): State<SharedWorld>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    cat_shards_impl(world, headers, None, raw).await
}

async fn cat_shards(
    State(world): State<SharedWorld>,
    Path(index): Path<String>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    cat_shards_impl(world, headers, Some(index), raw).await
}

// ─────────────────────────────── mappings ───────────────────────────────

fn mapping_body(indices: &[&Index]) -> Value {
    let mut out = Map::new();
    for i in indices {
        out.insert(
            i.name.clone(),
            json_value!({ "mappings": i.meta["mappings"] }),
        );
    }
    Value::Object(out)
}

async fn mapping_all(State(world): State<SharedWorld>, headers: HeaderMap) -> Response {
    tryr!(authorize(&headers, "/_mapping"));
    let (all, _) = snapshot(&world);
    let refs: Vec<&Index> = all.iter().collect();
    respond(StatusCode::OK, mapping_body(&refs))
}

async fn mapping(
    State(world): State<SharedWorld>,
    Path(index): Path<String>,
    headers: HeaderMap,
) -> Response {
    tryr!(authorize(&headers, &format!("/{index}/_mapping")));
    let (all, _) = snapshot(&world);
    match resolve(&all, &index) {
        Ok(found) => respond(StatusCode::OK, mapping_body(&found)),
        Err(e) => e.response(),
    }
}

// ──────────────────────────────── search ────────────────────────────────

fn parse_body(body: &Bytes) -> Result<Map<String, Value>, EsError> {
    if body.is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(m)) => Ok(m),
        Ok(_) => Err(EsError::parsing(
            "request body or source parameter is required",
        )),
        Err(e) => Err(EsError::new(
            StatusCode::BAD_REQUEST,
            "x_content_parse_exception",
            format!("[1:{}] Unexpected character: {e}", e.column()),
        )),
    }
}

fn run_search(
    world: &SharedWorld,
    index: &str,
    body: &Bytes,
    raw: Option<&str>,
    count: bool,
) -> Response {
    let (all, now_ms) = snapshot(world);
    let indices = match model::resolve_open(&all, index) {
        Ok(i) => i,
        Err(e) => return e.response(),
    };
    let mut body = match parse_body(body) {
        Ok(b) => b,
        Err(e) => return e.response(),
    };
    let ps = params(raw);
    if let Some(q) = param(&ps, "q") {
        body.insert(
            "query".into(),
            json_value!({ "query_string": { "query": q } }),
        );
    }
    for key in ["size", "from"] {
        if let Some(v) = param(&ps, key).and_then(|v| v.parse::<i64>().ok()) {
            body.insert(key.into(), json_value!(v));
        }
    }
    let ctx = Ctx::new(indices, now_ms);
    let result = if count {
        search::count(&body, &ctx)
    } else {
        search::search(&body, &ctx)
    };
    match result {
        Ok(v) => respond(StatusCode::OK, v),
        Err(e) => wrap_search_error(e, index).response(),
    }
}

/// Shard-level failures surface as `search_phase_execution_exception`, as in real clusters.
fn wrap_search_error(e: EsError, index: &str) -> EsError {
    if e.kind == "parsing_exception"
        || e.kind == "x_content_parse_exception"
        || e.status == StatusCode::NOT_FOUND
    {
        return e;
    }
    let inner = e.body()["error"].clone();
    let root = e.body()["error"]["root_cause"].clone();
    let mut outer = EsError::new(
        e.status,
        "search_phase_execution_exception",
        "all shards failed",
    )
    .with("phase", json_value!("query"))
    .with("grouped", json_value!(true))
    .with(
        "failed_shards",
        json_value!([{ "shard": 0, "index": index, "node": "es-hot-01", "reason": inner }]),
    )
    .with("caused_by", inner);
    outer.root_cause = Some(root);
    outer
}

async fn search_all(
    State(world): State<SharedWorld>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Bytes,
) -> Response {
    tryr!(authorize(&headers, "/_search"));
    run_search(&world, "*", &body, raw.as_deref(), false)
}

async fn search_index(
    State(world): State<SharedWorld>,
    Path(index): Path<String>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Bytes,
) -> Response {
    tryr!(authorize(&headers, &format!("/{index}/_search")));
    run_search(&world, &index, &body, raw.as_deref(), false)
}

async fn count_index(
    State(world): State<SharedWorld>,
    Path(index): Path<String>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Bytes,
) -> Response {
    tryr!(authorize(&headers, &format!("/{index}/_count")));
    run_search(&world, &index, &body, raw.as_deref(), true)
}

// ──────────────────────────────── ES|QL ────────────────────────────────

async fn esql_query(State(world): State<SharedWorld>, headers: HeaderMap, body: Bytes) -> Response {
    tryr!(authorize(&headers, "/_query"));
    let body = match parse_body(&body) {
        Ok(b) => b,
        Err(e) => return e.response(),
    };
    let Some(query) = body.get("query").and_then(Value::as_str) else {
        return EsError::new(
            StatusCode::BAD_REQUEST,
            "action_request_validation_exception",
            "Validation Failed: 1: [query] is required;",
        )
        .response();
    };
    let (all, now_ms) = snapshot(&world);
    match esql::run(query, &all, now_ms) {
        Ok(v) => respond(StatusCode::OK, v),
        Err(e) => e.response(),
    }
}
