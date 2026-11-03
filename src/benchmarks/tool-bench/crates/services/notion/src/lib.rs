//! Mock Notion API (`/v1`) as called by `@notionhq/notion-mcp-server` 2.6.1 with
//! `BASE_URL` pointed at the world. Serves the 2025-09-03 data-source model (and the
//! 2026-03-11 page Markdown endpoints): users, search, blocks, pages, page properties,
//! comments, data sources (+ query, templates) and databases, with Notion's error bodies.

pub mod blocks;
pub mod model;
pub mod pages;

use std::collections::HashMap;

use axum::{
    Router,
    body::Bytes,
    extract::{Path, RawQuery, State},
    http::{HeaderMap, StatusCode, header},
    response::Response,
    routing::{get, post},
};
use mock_core::{http::json, service::MockService, world::SharedWorld};
use serde_json::{Value, json as json_value};

use crate::model::{ApiError, Ctx, Res};

pub const NAME: &str = "notion";
pub const PORT: u16 = 8129;

const VERSIONS: &[&str] = &["2022-02-22", "2022-06-28", "2025-09-03", "2026-03-11"];

pub struct Notion;

impl MockService for Notion {
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
            .route("/v1/users/me", get(users_me))
            .route("/v1/users/{id}", get(user))
            .route("/v1/users", get(users))
            .route("/v1/search", post(search))
            .route(
                "/v1/blocks/{id}/children",
                get(block_children).patch(append_children),
            )
            .route(
                "/v1/blocks/{id}",
                get(block).patch(update_block).delete(delete_block),
            )
            .route("/v1/pages", post(create_page))
            .route("/v1/pages/{id}", get(page).patch(patch_page))
            .route("/v1/pages/{id}/properties/{prop}", get(page_property))
            .route("/v1/pages/{id}/move", post(move_page))
            .route(
                "/v1/pages/{id}/markdown",
                get(page_markdown).patch(update_markdown),
            )
            .route("/v1/comments", get(comments).post(create_comment))
            .route("/v1/data_sources", post(create_ds))
            .route("/v1/data_sources/{id}", get(data_source).patch(update_ds))
            .route("/v1/data_sources/{id}/query", post(query_ds))
            .route("/v1/data_sources/{id}/templates", get(templates))
            .route("/v1/databases/{id}", get(database))
            .fallback(fallback)
    }
}

async fn fallback() -> Response {
    json(
        StatusCode::BAD_REQUEST,
        json_value!({"object": "error", "status": 400, "code": "invalid_request_url", "message": "Invalid request URL.", "request_id": "00000000-0000-4000-8000-000000000000"}),
    )
}

fn query_map(raw: Option<&str>) -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for pair in raw.unwrap_or("").split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let k = decode(k).trim_end_matches("[]").to_owned();
        out.entry(k).or_default().push(decode(v));
    }
    out
}

fn decode(text: &str) -> String {
    let bytes = text.replace('+', " ").into_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn first<'a>(q: &'a HashMap<String, Vec<String>>, k: &str) -> Option<&'a str> {
    q.get(k)
        .and_then(|v| v.first())
        .map(String::as_str)
        .filter(|v| !v.is_empty())
}

fn size(q: &HashMap<String, Vec<String>>) -> Option<i64> {
    first(q, "page_size").and_then(|v| v.parse().ok())
}

/// Checks auth and version headers, then runs `f` against the Notion store.
fn run(
    world: &SharedWorld,
    headers: &HeaderMap,
    markdown: bool,
    f: impl FnOnce(&mut Ctx) -> Res,
) -> Response {
    let mut guard = world.lock().expect("world lock");
    let now = guard.clock.now_unix();
    let st = guard.service_mut(NAME);
    let mut ctx = Ctx { st, now };
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let token = auth.strip_prefix("Bearer ").unwrap_or("").trim();
    let outcome = if token.is_empty() {
        Err(ApiError::new(401, "unauthorized", "API token is invalid."))
    } else {
        match headers.get("notion-version").and_then(|v| v.to_str().ok()) {
            None => Err(ApiError::new(
                400,
                "missing_version",
                "Notion-Version header failed validation: Notion-Version header should be defined, instead was `undefined`.",
            )),
            Some(v) if !VERSIONS.contains(&v) => Err(ApiError::validation(format!(
                "Notion-Version header failed validation: Notion-Version header should be one of {}, instead was `\"{v}\"`.",
                VERSIONS
                    .iter()
                    .map(|x| format!("\"{x}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
            Some(v) if markdown && v < "2026-03-11" => Err(ApiError::validation(
                "This endpoint requires Notion-Version 2026-03-11 or later.",
            )),
            Some(_) => f(&mut ctx),
        }
    };
    match outcome {
        Ok(v) => json(StatusCode::OK, v),
        Err(e) => {
            let rid = ctx.request_id();
            e.response(&rid)
        }
    }
}

fn body_json(body: &Bytes) -> Result<Value, ApiError> {
    if body.is_empty() {
        return Ok(json_value!({}));
    }
    serde_json::from_slice(body)
        .map_err(|e| ApiError::new(400, "invalid_json", format!("Error parsing JSON body: {e}")))
}

async fn users_me(State(w): State<SharedWorld>, h: HeaderMap) -> Response {
    run(&w, &h, false, pages::me)
}

async fn user(State(w): State<SharedWorld>, h: HeaderMap, Path(id): Path<String>) -> Response {
    run(&w, &h, false, |c| pages::get_user(c, &id))
}

async fn users(State(w): State<SharedWorld>, h: HeaderMap, RawQuery(q): RawQuery) -> Response {
    let q = query_map(q.as_deref());
    run(&w, &h, false, |c| {
        pages::list_users(c, first(&q, "start_cursor"), size(&q))
    })
}

async fn search(State(w): State<SharedWorld>, h: HeaderMap, body: Bytes) -> Response {
    run(&w, &h, false, |c| pages::search(c, &body_json(&body)?))
}

async fn block_children(
    State(w): State<SharedWorld>,
    h: HeaderMap,
    Path(id): Path<String>,
    RawQuery(q): RawQuery,
) -> Response {
    let q = query_map(q.as_deref());
    run(&w, &h, false, |c| {
        blocks::get_children(c, &id, first(&q, "start_cursor"), size(&q))
    })
}

async fn append_children(
    State(w): State<SharedWorld>,
    h: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    run(&w, &h, false, |c| {
        blocks::patch_children(c, &id, &body_json(&body)?)
    })
}

async fn block(State(w): State<SharedWorld>, h: HeaderMap, Path(id): Path<String>) -> Response {
    run(&w, &h, false, |c| blocks::retrieve(c, &id))
}

async fn update_block(
    State(w): State<SharedWorld>,
    h: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    run(&w, &h, false, |c| {
        blocks::update(c, &id, &body_json(&body)?)
    })
}

async fn delete_block(
    State(w): State<SharedWorld>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    run(&w, &h, false, |c| blocks::delete(c, &id))
}

async fn create_page(State(w): State<SharedWorld>, h: HeaderMap, body: Bytes) -> Response {
    run(&w, &h, false, |c| pages::create_page(c, &body_json(&body)?))
}

async fn page(
    State(w): State<SharedWorld>,
    h: HeaderMap,
    Path(id): Path<String>,
    RawQuery(q): RawQuery,
) -> Response {
    let q = query_map(q.as_deref());
    let filter: Vec<String> = q.get("filter_properties").cloned().unwrap_or_default();
    run(&w, &h, false, |c| pages::retrieve_page(c, &id, &filter))
}

async fn patch_page(
    State(w): State<SharedWorld>,
    h: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    run(&w, &h, false, |c| {
        pages::patch_page(c, &id, &body_json(&body)?)
    })
}

async fn page_property(
    State(w): State<SharedWorld>,
    h: HeaderMap,
    Path((id, prop)): Path<(String, String)>,
    RawQuery(q): RawQuery,
) -> Response {
    let q = query_map(q.as_deref());
    run(&w, &h, false, |c| {
        pages::page_property(c, &id, &prop, first(&q, "start_cursor"), size(&q))
    })
}

async fn move_page(
    State(w): State<SharedWorld>,
    h: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    run(&w, &h, false, |c| {
        pages::move_page(c, &id, &body_json(&body)?)
    })
}

async fn page_markdown(
    State(w): State<SharedWorld>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    run(&w, &h, true, |c| blocks::get_markdown(c, &id))
}

async fn update_markdown(
    State(w): State<SharedWorld>,
    h: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    run(&w, &h, true, |c| {
        blocks::update_markdown(c, &id, &body_json(&body)?)
    })
}

async fn comments(State(w): State<SharedWorld>, h: HeaderMap, RawQuery(q): RawQuery) -> Response {
    let q = query_map(q.as_deref());
    run(&w, &h, false, |c| {
        pages::list_comments(
            c,
            first(&q, "block_id"),
            first(&q, "start_cursor"),
            size(&q),
        )
    })
}

async fn create_comment(State(w): State<SharedWorld>, h: HeaderMap, body: Bytes) -> Response {
    run(&w, &h, false, |c| {
        pages::create_comment(c, &body_json(&body)?)
    })
}

async fn create_ds(State(w): State<SharedWorld>, h: HeaderMap, body: Bytes) -> Response {
    run(&w, &h, false, |c| pages::create_ds(c, &body_json(&body)?))
}

async fn data_source(
    State(w): State<SharedWorld>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    run(&w, &h, false, |c| pages::get_ds(c, &id))
}

async fn update_ds(
    State(w): State<SharedWorld>,
    h: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    run(&w, &h, false, |c| {
        pages::update_ds(c, &id, &body_json(&body)?)
    })
}

async fn query_ds(
    State(w): State<SharedWorld>,
    h: HeaderMap,
    Path(id): Path<String>,
    RawQuery(q): RawQuery,
    body: Bytes,
) -> Response {
    let q = query_map(q.as_deref());
    let filter: Vec<String> = q.get("filter_properties").cloned().unwrap_or_default();
    run(&w, &h, false, |c| {
        pages::query(c, &id, &body_json(&body)?, &filter)
    })
}

async fn templates(
    State(w): State<SharedWorld>,
    h: HeaderMap,
    Path(id): Path<String>,
    RawQuery(q): RawQuery,
) -> Response {
    let q = query_map(q.as_deref());
    run(&w, &h, false, |c| {
        pages::templates(c, &id, first(&q, "start_cursor"), size(&q))
    })
}

async fn database(State(w): State<SharedWorld>, h: HeaderMap, Path(id): Path<String>) -> Response {
    run(&w, &h, false, |c| pages::get_db(c, &id))
}
