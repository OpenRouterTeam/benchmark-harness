//! Mock Firecrawl API v2 (a self-hosted instance, as `firecrawl-mcp` sees it with
//! `FIRECRAWL_API_URL` pointed at the world).
//!
//! The world is "the web as it was at `now`": the dataset defines which pages exist
//! (`pages`, keyed by URL), what the search index returns (`search_index`), extra sitemap
//! URLs (`sitemap`), research papers (`papers`), developer and government indexes
//! (`developer_docs`, `gov_docs`), research-agent answers (`agent_answers`), the team's
//! credits (`team`), and monitors with their check history (`monitors`, `monitor_checks`).
//! Jobs created through the API (`crawls`, `agent_jobs`, `monitors`, `monitor_checks`,
//! `feedback`, `searches`, `scrapes`) are written to the store.
//!
//! Like a self-hosted Firecrawl, the team has no Alexandria (exchange) access: those
//! requests get the real 403, and the MCP server falls back to plain web search.

// Handlers return early with a full `Response` as the error.
#![allow(clippy::result_large_err)]

mod text;

use axum::{
    Router,
    body::Bytes,
    extract::{Path, RawQuery, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
};
use mock_core::{
    clock::format_unix,
    http::json,
    service::MockService,
    world::{SharedWorld, WorldState},
};
use regex::Regex;
use serde_json::{Map, Value, json as j};

use crate::text::{
    SearchQuery, host_matches, host_of, is_http_url, markdown_images, markdown_links,
    markdown_to_html, norm_url, parse_query, path_of, schedule_interval_minutes, summarize,
};

pub const NAME: &str = "firecrawl";
pub const PORT: u16 = 8102;

/// Documents per page of a crawl status response before `next` is set.
const CRAWL_PAGE_SIZE: usize = 10;
const DAY_SECS: i64 = 86_400;
const EXCHANGE_DISABLED: &str = "Exchange is not enabled for this team.";

pub struct Firecrawl;

impl MockService for Firecrawl {
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
            .route("/v2/scrape", post(scrape))
            .route(
                "/v2/scrape/{id}/interact",
                post(interact).delete(interact_stop),
            )
            .route("/v2/map", post(map))
            .route("/v2/search", post(search))
            .route("/v2/search/{id}/feedback", post(search_feedback))
            .route("/v2/search/developer", get(developer_search))
            .route("/v2/search/gov", get(gov_search))
            .route("/v2/search/research/papers", get(papers_search))
            .route("/v2/search/research/papers/{id}", get(paper_get))
            .route(
                "/v2/search/research/papers/{id}/similar",
                get(paper_similar),
            )
            .route("/v2/crawl", post(crawl_start))
            .route("/v2/crawl/active", get(crawl_active))
            .route("/v2/crawl/{id}", get(crawl_status).delete(crawl_cancel))
            .route("/v2/crawl/{id}/errors", get(crawl_errors))
            .route("/v2/agent", post(agent_start).get(agent_list))
            .route("/v2/agent/{id}", get(agent_status).delete(agent_cancel))
            .route("/v2/parse", post(parse))
            .route("/v2/parse/upload-url", post(parse_upload_url))
            .route("/v2/feedback", post(feedback))
            .route("/v2/team/credit-usage", get(credit_usage))
            .route(
                "/v2/team/credit-usage/historical",
                get(credit_usage_historical),
            )
            .route("/v2/monitor", get(monitor_list).post(monitor_create))
            .route(
                "/v2/monitor/{id}",
                get(monitor_get)
                    .patch(monitor_update)
                    .delete(monitor_delete),
            )
            .route("/v2/monitor/{id}/run", post(monitor_run))
            .route("/v2/monitor/{id}/checks", get(monitor_checks))
            .route("/v2/monitor/{id}/checks/{check_id}", get(monitor_check))
            .route("/exchange/discover", get(exchange_disabled))
            .fallback(not_found)
            .layer(middleware::from_fn(require_auth))
    }
}

// ───────────────────────────── helpers ─────────────────────────────

fn ok(body: Value) -> Response {
    json(StatusCode::OK, body)
}

fn err(status: StatusCode, message: &str) -> Response {
    json(status, j!({ "success": false, "error": message }))
}

/// Firecrawl's request-validation error (zod issues under `details`).
fn bad_request(path: &str, message: &str) -> Response {
    json(
        StatusCode::BAD_REQUEST,
        j!({
            "success": false,
            "code": "BAD_REQUEST",
            "error": format!("Bad Request ({path}: {message})"),
            "details": [{ "code": "custom", "path": [path], "message": message }]
        }),
    )
}

async fn not_found() -> Response {
    err(StatusCode::NOT_FOUND, "Not Found")
}

async fn exchange_disabled() -> Response {
    err(StatusCode::FORBIDDEN, EXCHANGE_DISABLED)
}

async fn require_auth(request: Request, next: Next) -> Response {
    let authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|token| !token.trim().is_empty());
    if !authorized {
        return err(StatusCode::UNAUTHORIZED, "Unauthorized: Invalid token");
    }
    next.run(request).await
}

fn body_object(body: &Bytes) -> Result<Map<String, Value>, Response> {
    if body.is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(err(StatusCode::BAD_REQUEST, "Invalid JSON body")),
    }
}

fn base_url(headers: &HeaderMap) -> String {
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

fn docs<'a>(world: &'a WorldState, collection: &str) -> Vec<(&'a str, &'a Value)> {
    world
        .service(NAME)
        .and_then(|s| s.store.collection(collection))
        .map(|c| c.iter().collect())
        .unwrap_or_default()
}

fn doc<'a>(world: &'a WorldState, collection: &str, key: &str) -> Option<&'a Value> {
    world
        .service(NAME)
        .and_then(|s| s.store.collection(collection))
        .and_then(|c| c.get(key))
}

fn insert(world: &mut WorldState, collection: &str, key: &str, value: Value) {
    world
        .service_mut(NAME)
        .store
        .collection_mut(collection)
        .insert(key, value);
}

fn uuid(world: &mut WorldState, kind: &str) -> String {
    world.service_mut(NAME).ids.next_uuid(kind).to_string()
}

fn now_iso(world: &WorldState) -> String {
    world.clock.now_rfc3339()
}

fn iso_after(world: &WorldState, secs: i64) -> String {
    format_unix(world.clock.now_unix() + secs)
}

fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn usize_of(v: Option<&Value>) -> Option<usize> {
    v.and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)))
        .map(|n| n as usize)
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn query_value<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn query_values<'a>(pairs: &'a [(String, String)], key: &str) -> Vec<&'a str> {
    pairs
        .iter()
        .filter(|(k, v)| k == key && !v.is_empty())
        .map(|(_, v)| v.as_str())
        .collect()
}

fn joined_text(v: &Value, fields: &[&str]) -> String {
    let mut out = String::new();
    for field in fields {
        match v.get(*field) {
            Some(Value::String(s)) => out.push_str(s),
            Some(Value::Array(items)) => {
                for item in items {
                    match item {
                        Value::String(s) => out.push_str(s),
                        other => out.push_str(&other.to_string()),
                    }
                    out.push(' ');
                }
            }
            Some(Value::Null) | None => {}
            Some(other) => out.push_str(&other.to_string()),
        }
        out.push(' ');
    }
    out
}

/// Items ranked by query relevance (score desc, dataset order on ties).
fn rank<'a, T>(query: &SearchQuery, items: Vec<(T, String)>) -> Vec<T>
where
    T: 'a,
{
    let mut scored: Vec<(usize, usize, T)> = items
        .into_iter()
        .enumerate()
        .filter_map(|(i, (item, text))| query.score(&text).map(|s| (s, i, item)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, item)| item).collect()
}

// ───────────────────────────── pages and documents ─────────────────────────────

fn find_page<'a>(world: &'a WorldState, url: &str) -> Option<(&'a str, &'a Value)> {
    let wanted = norm_url(url);
    docs(world, "pages")
        .into_iter()
        .find(|(k, _)| norm_url(k) == wanted)
}

fn known_host(world: &WorldState, host: &str) -> bool {
    ["pages", "sitemap", "search_index"].iter().any(|c| {
        docs(world, c).iter().any(|(k, v)| {
            let url = v.get("url").and_then(Value::as_str).unwrap_or(k);
            host_of(url) == host
        })
    })
}

/// Requested formats as `(type, options)`, defaulting to markdown.
fn formats(v: Option<&Value>) -> Vec<(String, Value)> {
    let list: Vec<(String, Value)> = v
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|f| match f {
                    Value::String(s) => Some((s.clone(), Value::Null)),
                    Value::Object(o) => o
                        .get("type")
                        .and_then(Value::as_str)
                        .map(|t| (t.to_owned(), f.clone())),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    if list.is_empty() {
        vec![("markdown".to_owned(), Value::Null)]
    } else {
        list
    }
}

/// The JSON-format result: the page's extracted data, narrowed to the schema's properties
/// when they overlap it.
fn json_extraction(page: &Value, options: &Value) -> Value {
    let Some(data) = page.get("json") else {
        return j!({});
    };
    let props = options
        .get("schema")
        .and_then(|s| s.get("properties"))
        .and_then(Value::as_object);
    match (data, props) {
        (Value::Object(obj), Some(props)) if props.keys().any(|k| obj.contains_key(k)) => {
            let mut out = Map::new();
            for key in props.keys() {
                out.insert(key.clone(), obj.get(key).cloned().unwrap_or(Value::Null));
            }
            Value::Object(out)
        }
        _ => data.clone(),
    }
}

fn page_html(page: &Value) -> String {
    page.get("html")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| markdown_to_html(str_of(page, "markdown")))
}

/// A scraped document in the v2 shape: requested formats, then `metadata`.
fn document(
    url: &str,
    requested: &str,
    page: &Value,
    wanted: &[(String, Value)],
    scrape_id: &str,
    now: &str,
) -> Value {
    let md = str_of(page, "markdown");
    let mut out = Map::new();
    let mut credits = 1;
    for (format, options) in wanted {
        let value = match format.as_str() {
            "markdown" => j!(md),
            "html" => j!(page_html(page)),
            "rawHtml" => page.get("rawHtml").cloned().unwrap_or_else(|| {
                j!(format!(
                    "<!DOCTYPE html><html lang=\"{}\"><head><meta charset=\"utf-8\"><title>{}</title></head><body>{}</body></html>",
                    page.get("language").and_then(Value::as_str).unwrap_or("en"),
                    str_of(page, "title"),
                    page_html(page)
                ))
            }),
            "links" => page
                .get("links")
                .cloned()
                .unwrap_or_else(|| j!(markdown_links(md))),
            "images" => page
                .get("images")
                .cloned()
                .unwrap_or_else(|| j!(markdown_images(md))),
            "summary" => page
                .get("summary")
                .cloned()
                .unwrap_or_else(|| j!(summarize(md))),
            "json" => {
                credits += 4;
                json_extraction(page, options)
            }
            "query" => {
                credits += 4;
                out.insert(
                    "answer".to_owned(),
                    page.get("answer")
                        .cloned()
                        .unwrap_or_else(|| j!(summarize(md))),
                );
                continue;
            }
            "screenshot" => j!(format!(
                "https://storage.googleapis.com/firecrawl-scrape-media/screenshot-{scrape_id}.png"
            )),
            "branding" => page.get("branding").cloned().unwrap_or_else(|| {
                j!({ "colorScheme": "light", "fonts": [], "colors": {}, "images": {} })
            }),
            "changeTracking" => j!({
                "previousScrapeAt": null,
                "changeStatus": "new",
                "visibility": "visible"
            }),
            _ => continue,
        };
        out.insert(format.clone(), value);
    }
    let mut metadata = Map::new();
    for field in ["title", "description", "language"] {
        if let Some(v) = page.get(field) {
            metadata.insert(field.to_owned(), v.clone());
        }
    }
    if let Some(Value::Object(extra)) = page.get("metadata") {
        for (k, v) in extra {
            metadata.insert(k.clone(), v.clone());
        }
    }
    metadata.insert("sourceURL".to_owned(), j!(requested));
    metadata.insert("url".to_owned(), j!(url));
    metadata.insert(
        "statusCode".to_owned(),
        page.get("statusCode").cloned().unwrap_or(j!(200)),
    );
    metadata.insert(
        "contentType".to_owned(),
        page.get("contentType")
            .cloned()
            .unwrap_or(j!("text/html; charset=utf-8")),
    );
    metadata.insert("scrapeId".to_owned(), j!(scrape_id));
    metadata.insert("proxyUsed".to_owned(), j!("basic"));
    metadata.insert("cacheState".to_owned(), j!("hit"));
    metadata.insert(
        "cachedAt".to_owned(),
        page.get("cachedAt").cloned().unwrap_or(j!(now)),
    );
    metadata.insert("creditsUsed".to_owned(), j!(credits));
    out.insert("metadata".to_owned(), Value::Object(metadata));
    Value::Object(out)
}

fn not_found_page(url: &str) -> Value {
    j!({
        "title": "404 Not Found",
        "markdown": format!("# 404 Not Found\n\nThe requested URL {} was not found on this server.", path_of(url)),
        "statusCode": 404,
        "metadata": { "error": "Not Found" }
    })
}

fn dns_error(url: &str) -> Response {
    json(
        StatusCode::BAD_REQUEST,
        j!({
            "success": false,
            "code": "SCRAPE_DNS_RESOLUTION_ERROR",
            "error": format!(
                "DNS resolution failed for hostname: {}. Please check if the domain is valid and accessible.",
                host_of(url)
            )
        }),
    )
}

/// The page for `url`: the dataset's, a 404 page on a known host, or `None` (DNS failure).
fn resolve_page(world: &WorldState, url: &str) -> Option<(String, Value)> {
    match find_page(world, url) {
        Some((key, page)) => Some((key.to_owned(), page.clone())),
        None if known_host(world, &host_of(url)) => Some((url.to_owned(), not_found_page(url))),
        None => None,
    }
}

// ───────────────────────────── scrape / interact ─────────────────────────────

async fn scrape(State(world): State<SharedWorld>, body: Bytes) -> Response {
    let input = match body_object(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if input.contains_key("alexandria") {
        return err(StatusCode::FORBIDDEN, EXCHANGE_DISABLED);
    }
    let Some(url) = input.get("url").and_then(Value::as_str) else {
        return bad_request("url", "Required");
    };
    if !is_http_url(url) {
        return bad_request("url", "Invalid URL");
    }
    if input.get("domainTools") == Some(&Value::Bool(true)) {
        return err(StatusCode::FORBIDDEN, EXCHANGE_DISABLED);
    }
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    let Some((key, page)) = resolve_page(w, url) else {
        return dns_error(url);
    };
    let now = now_iso(w);
    let scrape_id = uuid(w, "scrape");
    let doc = document(
        &key,
        url,
        &page,
        &formats(input.get("formats")),
        &scrape_id,
        &now,
    );
    insert(
        w,
        "scrapes",
        &scrape_id,
        j!({ "id": scrape_id, "url": key, "createdAt": now }),
    );
    ok(j!({ "success": true, "data": doc }))
}

async fn interact(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let input = match body_object(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let code = input.get("code").and_then(Value::as_str);
    let prompt = input.get("prompt").and_then(Value::as_str);
    if code.is_none() && prompt.is_none() {
        return bad_request("code", "Either 'code' or 'prompt' must be provided");
    }
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    let Some(session) = doc(w, "scrapes", &id).cloned() else {
        return err(
            StatusCode::NOT_FOUND,
            "Scrape job not found or its browser session has expired.",
        );
    };
    if session.get("stoppedAt").is_some() {
        return err(
            StatusCode::GONE,
            "The browser session for this scrape has been stopped.",
        );
    }
    let url = str_of(&session, "url").to_owned();
    let page_md = find_page(w, &url)
        .map(|(_, p)| str_of(p, "markdown").to_owned())
        .unwrap_or_default();
    let output = match prompt {
        Some(p) => format!(
            "Completed: {p}\n\nCurrent page: {url}\n\n{}",
            summarize(&page_md)
        ),
        None => String::new(),
    };
    let language = input
        .get("language")
        .and_then(Value::as_str)
        .unwrap_or("node");
    let now = now_iso(w);
    if let Some(s) = w
        .service_mut(NAME)
        .store
        .collection_mut("scrapes")
        .get_mut(&id)
    {
        s["lastInteractionAt"] = j!(now);
    }
    ok(j!({
        "success": true,
        "output": output,
        "stdout": if code.is_some() { format!("[{language}] executed against {url}\n") } else { String::new() },
        "stderr": "",
        "exitCode": 0,
        "liveUrl": format!("https://liveview.firecrawl.dev/{id}")
    }))
}

async fn interact_stop(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    let now = now_iso(w);
    match w
        .service_mut(NAME)
        .store
        .collection_mut("scrapes")
        .get_mut(&id)
    {
        Some(s) => {
            s["stoppedAt"] = j!(now);
            ok(j!({ "success": true, "message": "Interaction session stopped.", "scrapeId": id }))
        }
        None => err(
            StatusCode::NOT_FOUND,
            "Scrape job not found or its browser session has expired.",
        ),
    }
}

// ───────────────────────────── map ─────────────────────────────

/// Every known URL with its title/description: pages first, then extra sitemap entries.
fn site_links(world: &WorldState) -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> = Vec::new();
    for collection in ["pages", "sitemap"] {
        for (k, v) in docs(world, collection) {
            let url = v.get("url").and_then(Value::as_str).unwrap_or(k).to_owned();
            if out.iter().all(|(u, _)| norm_url(u) != norm_url(&url)) {
                out.push((url, v.clone()));
            }
        }
    }
    out
}

async fn map(State(world): State<SharedWorld>, body: Bytes) -> Response {
    let input = match body_object(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(url) = input.get("url").and_then(Value::as_str) else {
        return bad_request("url", "Required");
    };
    if !is_http_url(url) {
        return bad_request("url", "Invalid URL");
    }
    let include_subdomains = input.get("includeSubdomains") != Some(&Value::Bool(false));
    let ignore_query = input.get("ignoreQueryParameters") != Some(&Value::Bool(false));
    let limit = usize_of(input.get("limit"))
        .unwrap_or(5000)
        .clamp(1, 100_000);
    let host = host_of(url);
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    let mut seen: Vec<String> = Vec::new();
    let candidates: Vec<(Value, String)> = site_links(w)
        .into_iter()
        .filter(|(u, _)| {
            let h = host_of(u);
            h == host || (include_subdomains && host_matches(&h, &host))
        })
        .filter(|(u, _)| {
            let key = if ignore_query {
                norm_url(u).split('?').next().unwrap_or("").to_owned()
            } else {
                norm_url(u)
            };
            if seen.contains(&key) {
                false
            } else {
                seen.push(key);
                true
            }
        })
        .map(|(u, v)| {
            let mut link = Map::new();
            link.insert("url".to_owned(), j!(u));
            for field in ["title", "description"] {
                if let Some(Value::String(s)) = v.get(field) {
                    link.insert(field.to_owned(), j!(s));
                }
            }
            let text = format!("{u} {}", joined_text(&v, &["title", "description"]));
            (Value::Object(link), text)
        })
        .collect();
    let links: Vec<Value> = match input.get("search").and_then(Value::as_str) {
        Some(s) if !s.trim().is_empty() => rank(&SearchQuery::parse(s), candidates),
        _ => candidates.into_iter().map(|(l, _)| l).collect(),
    };
    let links: Vec<Value> = links.into_iter().take(limit).collect();
    let id = uuid(w, "map");
    ok(j!({ "success": true, "links": links, "id": id }))
}

// ───────────────────────────── search ─────────────────────────────

fn source_names(v: Option<&Value>) -> Vec<String> {
    let names: Vec<String> = v
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| match s {
                    Value::String(s) => Some(s.clone()),
                    Value::Object(o) => o.get("type").and_then(Value::as_str).map(str::to_owned),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    if names.is_empty() {
        vec!["web".to_owned()]
    } else {
        names
    }
}

fn domain_allowed(url: &str, include: &[String], exclude: &[String], sites: &[String]) -> bool {
    let host = host_of(url);
    let n = norm_url(url);
    (include.is_empty() || include.iter().any(|d| host_matches(&host, d)))
        && !exclude.iter().any(|d| host_matches(&host, d))
        && (sites.is_empty()
            || sites.iter().any(|s| {
                let s = norm_url(s);
                host_matches(&host, &s) || n.starts_with(&s)
            }))
}

fn search_result(kind: &str, item: &Value, category: Option<&str>) -> Value {
    let mut out = Map::new();
    let copy = |out: &mut Map<String, Value>, fields: &[&str]| {
        for f in fields {
            if let Some(v) = item.get(*f) {
                out.insert((*f).to_owned(), v.clone());
            }
        }
    };
    match kind {
        "news" => copy(&mut out, &["title", "url", "snippet", "date", "imageUrl"]),
        "images" => copy(
            &mut out,
            &["title", "imageUrl", "imageWidth", "imageHeight", "url"],
        ),
        _ => copy(&mut out, &["url", "title", "description"]),
    }
    if let Some(c) = category {
        out.insert("category".to_owned(), j!(c));
    }
    Value::Object(out)
}

async fn search(State(world): State<SharedWorld>, body: Bytes) -> Response {
    let input = match body_object(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(query) = input
        .get("query")
        .and_then(Value::as_str)
        .filter(|q| !q.trim().is_empty())
    else {
        return bad_request("query", "Required");
    };
    let sources = source_names(input.get("sources"));
    if sources.iter().any(|s| s == "alexandria" || s == "exchange")
        || input.get("domainTools") == Some(&Value::Bool(true))
    {
        return err(StatusCode::FORBIDDEN, EXCHANGE_DISABLED);
    }
    let include = strings(input.get("includeDomains"));
    let exclude = strings(input.get("excludeDomains"));
    let categories = strings(input.get("categories"));
    let limit = usize_of(input.get("limit")).unwrap_or(5).clamp(1, 100);
    let q = SearchQuery::parse(query);
    let scrape_formats = input
        .get("scrapeOptions")
        .map(|o| formats(o.get("formats")));

    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    let mut data = Map::new();
    let mut total = 0;
    let special = categories
        .iter()
        .find(|c| *c == "developer" || *c == "gov")
        .cloned();
    for source in &sources {
        let kind = source.as_str();
        if !matches!(kind, "web" | "news" | "images") {
            continue;
        }
        let items: Vec<(Value, String)> = match (kind, special.as_deref()) {
            ("web", Some("developer")) => docs(w, "developer_docs")
                .into_iter()
                .filter(|(_, v)| domain_allowed(str_of(v, "url"), &include, &exclude, &q.sites))
                .map(|(_, v)| {
                    let description = v
                        .get("passages")
                        .and_then(Value::as_array)
                        .and_then(|p| p.first())
                        .map(|p| str_of(p, "text").to_owned())
                        .unwrap_or_default();
                    let item =
                        j!({ "url": v["url"], "title": v["title"], "description": description });
                    (
                        search_result("web", &item, Some("developer")),
                        joined_text(v, &["title", "url", "passages", "keywords"]),
                    )
                })
                .collect(),
            ("web", Some("gov")) => docs(w, "gov_docs")
                .into_iter()
                .filter(|(_, v)| domain_allowed(str_of(v, "url"), &include, &exclude, &q.sites))
                .map(|(_, v)| {
                    (
                        search_result("web", v, Some("gov")),
                        joined_text(v, &["title", "url", "description", "keywords", "cites"]),
                    )
                })
                .collect(),
            (_, Some(_)) => Vec::new(),
            _ => docs(w, "search_index")
                .into_iter()
                .filter(|(_, v)| v.get("kind").and_then(Value::as_str).unwrap_or("web") == kind)
                .filter(|(_, v)| {
                    categories.is_empty()
                        || categories
                            .iter()
                            .any(|c| v.get("category").and_then(Value::as_str) == Some(c))
                })
                .filter(|(_, v)| domain_allowed(str_of(v, "url"), &include, &exclude, &q.sites))
                .map(|(_, v)| {
                    let category = v.get("category").and_then(Value::as_str);
                    let mut result =
                        search_result(kind, v, category.filter(|_| !categories.is_empty()));
                    if kind == "web"
                        && let Some(wanted) = &scrape_formats
                        && let Some((key, page)) = find_page(w, str_of(v, "url"))
                        && let Value::Object(fields) =
                            document(key, str_of(v, "url"), page, wanted, "", &now_iso(w))
                        && let Value::Object(r) = &mut result
                    {
                        for (k, val) in fields {
                            if k != "metadata" {
                                r.insert(k, val);
                            }
                        }
                    }
                    (
                        result,
                        joined_text(v, &["title", "description", "snippet", "url", "keywords"]),
                    )
                })
                .collect(),
        };
        let ranked: Vec<Value> = rank(&q, items).into_iter().take(limit).collect();
        total += ranked.len();
        data.insert(kind.to_owned(), Value::Array(ranked));
    }
    let id = uuid(w, "search");
    let now = now_iso(w);
    let credits = 2 * total.div_ceil(10).max(1);
    let urls: Vec<Value> = data
        .values()
        .flat_map(|v| v.as_array().cloned().unwrap_or_default())
        .map(|r| r.get("url").cloned().unwrap_or(Value::Null))
        .collect();
    insert(
        w,
        "searches",
        &id,
        j!({ "id": id, "query": query, "createdAt": now, "resultUrls": urls }),
    );
    ok(j!({ "success": true, "data": data, "creditsUsed": credits, "id": id }))
}

async fn search_feedback(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let input = match body_object(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    if doc(w, "searches", &id).is_none() {
        return json(
            StatusCode::NOT_FOUND,
            j!({ "success": false, "error": "Search not found.", "feedbackErrorCode": "SEARCH_NOT_FOUND" }),
        );
    }
    let rating = input.get("rating").and_then(Value::as_str).unwrap_or("");
    let has = |k: &str| {
        input
            .get(k)
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty())
    };
    let valid = match rating {
        "good" => has("valuableResults") || has("valuableSources"),
        "partial" => has("valuableResults") || has("valuableSources") || has("missingContent"),
        "bad" => has("missingContent") || input.get("querySuggestions").is_some(),
        _ => false,
    };
    if !valid {
        return json(
            StatusCode::BAD_REQUEST,
            j!({
                "success": false,
                "error": "Feedback does not satisfy the rating requirements.",
                "feedbackErrorCode": "INVALID_FEEDBACK"
            }),
        );
    }
    let mut record = input.clone();
    record.remove("origin");
    record.insert("endpoint".to_owned(), j!("search"));
    record.insert("jobId".to_owned(), j!(id));
    record_feedback(w, &id, Value::Object(record))
}

fn record_feedback(w: &mut WorldState, job_id: &str, mut record: Value) -> Response {
    let already = docs(w, "feedback")
        .iter()
        .any(|(_, f)| str_of(f, "jobId") == job_id && !job_id.is_empty());
    let refunded_today = docs(w, "feedback")
        .iter()
        .filter(|(_, f)| f.get("creditsRefunded").and_then(Value::as_u64) == Some(1))
        .count();
    let refund = u64::from(!already);
    let feedback_id = uuid(w, "feedback");
    record["feedbackId"] = j!(feedback_id);
    record["creditsRefunded"] = j!(refund);
    record["createdAt"] = j!(now_iso(w));
    insert(w, "feedback", &feedback_id, record);
    ok(j!({
        "success": true,
        "message": if already { "Feedback recorded. Feedback for this job was already submitted, so no credit was refunded." } else { "Feedback recorded. 1 credit refunded." },
        "feedbackId": feedback_id,
        "creditsRefunded": refund,
        "creditsRefundedToday": refunded_today as u64 + refund,
        "dailyRefundCap": 50,
        "dailyCapReached": false,
        "alreadySubmitted": already
    }))
}

async fn feedback(State(world): State<SharedWorld>, body: Bytes) -> Response {
    let input = match body_object(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let endpoint = input.get("endpoint").and_then(Value::as_str).unwrap_or("");
    if !["search", "scrape", "parse", "map", "alexandria"].contains(&endpoint) {
        return bad_request("endpoint", "Invalid enum value");
    }
    if !matches!(
        input.get("rating").and_then(Value::as_str),
        Some("good" | "bad" | "partial")
    ) {
        return bad_request("rating", "Invalid enum value");
    }
    if endpoint == "alexandria" {
        return json(
            StatusCode::BAD_REQUEST,
            j!({
                "success": false,
                "error": "Alexandria feedback must arrive within 20 minutes of the team's latest Alexandria call; this team has no recent Alexandria activity.",
                "feedbackErrorCode": "ALEXANDRIA_SESSION_NOT_FOUND"
            }),
        );
    }
    let Some(job_id) = input
        .get("jobId")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return bad_request("jobId", "jobId is required for job feedback");
    };
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    let mut record = input.clone();
    record.remove("origin");
    record_feedback(w, &job_id, Value::Object(record))
}

// ───────────────────────────── developer / gov / research ─────────────────────────────

async fn developer_search(State(world): State<SharedWorld>, RawQuery(raw): RawQuery) -> Response {
    let pairs = parse_query(raw.as_deref());
    let Some(query) = query_value(&pairs, "query").filter(|q| !q.trim().is_empty()) else {
        return bad_request("query", "Required");
    };
    let k = query_value(&pairs, "k")
        .and_then(|k| k.parse().ok())
        .unwrap_or(10usize);
    let skills_only = query_value(&pairs, "skills") == Some("only");
    let q = SearchQuery::parse(query);
    let w = world.lock().expect("world lock");
    let items = docs(&w, "developer_docs")
        .into_iter()
        .filter(|(_, v)| !skills_only || str_of(v, "id").starts_with("skill:"))
        .map(|(_, v)| {
            let mut r = v.clone();
            if let Value::Object(o) = &mut r {
                o.remove("keywords");
            }
            (r, joined_text(v, &["title", "url", "passages", "keywords"]))
        })
        .collect();
    let results: Vec<Value> = rank(&q, items).into_iter().take(k).collect();
    ok(j!({ "success": true, "results": results }))
}

async fn gov_search(State(world): State<SharedWorld>, RawQuery(raw): RawQuery) -> Response {
    let pairs = parse_query(raw.as_deref());
    let Some(query) = query_value(&pairs, "query").filter(|q| !q.trim().is_empty()) else {
        return err(StatusCode::BAD_REQUEST, "query is required");
    };
    let k = query_value(&pairs, "k")
        .and_then(|k| k.parse().ok())
        .unwrap_or(10usize);
    let filter = |key: &str| query_value(&pairs, key).map(str::to_lowercase);
    let (jurisdiction, court, court_type, cites, domain) = (
        filter("jurisdiction"),
        filter("court"),
        filter("courtType"),
        filter("cites"),
        filter("domain"),
    );
    let q = SearchQuery::parse(query);
    let w = world.lock().expect("world lock");
    let items = docs(&w, "gov_docs")
        .into_iter()
        .filter(|(_, v)| {
            let eq = |field: &str, want: &Option<String>| {
                want.as_ref()
                    .is_none_or(|x| str_of(v, field).to_lowercase() == *x)
            };
            eq("jurisdiction", &jurisdiction)
                && eq("court", &court)
                && eq("courtType", &court_type)
                && domain
                    .as_ref()
                    .is_none_or(|d| host_matches(&host_of(str_of(v, "url")), d))
                && cites.as_ref().is_none_or(|c| {
                    strings(v.get("cites"))
                        .iter()
                        .any(|x| x.to_lowercase() == *c)
                })
        })
        .map(|(_, v)| {
            (
                j!({ "url": v["url"], "title": v["title"], "description": v["description"] }),
                joined_text(v, &["title", "url", "description", "keywords", "cites"]),
            )
        })
        .collect();
    let web: Vec<Value> = rank(&q, items)
        .into_iter()
        .take(k)
        .enumerate()
        .map(|(i, mut r)| {
            r["position"] = j!(i + 1);
            r
        })
        .collect();
    ok(j!({ "success": true, "data": { "web": web } }))
}

fn paper_summary(paper: &Value) -> Value {
    let mut p = paper.clone();
    if let Value::Object(o) = &mut p {
        o.remove("passages");
        o.remove("related");
        o.remove("keywords");
    }
    p
}

fn find_paper<'a>(world: &'a WorldState, id: &str) -> Option<&'a Value> {
    let id = id.trim().to_lowercase();
    docs(world, "papers")
        .into_iter()
        .find(|(k, v)| {
            k.to_lowercase() == id
                || str_of(v, "primaryId").to_lowercase() == id
                || v.get("ids").and_then(Value::as_object).is_some_and(|ids| {
                    ids.iter().any(|(ns, values)| {
                        strings(Some(values))
                            .iter()
                            .any(|x| format!("{ns}:{x}").to_lowercase() == id)
                    })
                })
        })
        .map(|(_, v)| v)
}

async fn papers_search(State(world): State<SharedWorld>, RawQuery(raw): RawQuery) -> Response {
    let pairs = parse_query(raw.as_deref());
    let Some(query) = query_value(&pairs, "query").filter(|q| !q.trim().is_empty()) else {
        return bad_request("query", "Required");
    };
    let k = query_value(&pairs, "k")
        .and_then(|k| k.parse().ok())
        .unwrap_or(40usize);
    let authors: Vec<String> = query_values(&pairs, "authors")
        .iter()
        .map(|a| a.to_lowercase())
        .collect();
    let categories = query_values(&pairs, "categories");
    let from = query_value(&pairs, "from").unwrap_or("");
    let to = query_value(&pairs, "to").unwrap_or("");
    let q = SearchQuery::parse(query);
    let w = world.lock().expect("world lock");
    let items = docs(&w, "papers")
        .into_iter()
        .filter(|(_, v)| {
            let names = joined_text(v, &["authors"]).to_lowercase();
            let cats = strings(v.get("categories"));
            let date = v
                .get("updateDate")
                .or(v.get("createdDate"))
                .and_then(Value::as_str)
                .unwrap_or("");
            authors.iter().all(|a| names.contains(a.as_str()))
                && categories.iter().all(|c| cats.iter().any(|x| x == c))
                && (from.is_empty() || date >= from)
                && (to.is_empty() || str_of(v, "createdDate") <= to)
        })
        .map(|(_, v)| {
            (
                paper_summary(v),
                joined_text(v, &["title", "abstract", "keywords", "categories"]),
            )
        })
        .collect();
    let results: Vec<Value> = rank(&q, items).into_iter().take(k).collect();
    ok(j!({ "success": true, "results": results }))
}

async fn paper_get(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
) -> Response {
    let pairs = parse_query(raw.as_deref());
    let w = world.lock().expect("world lock");
    let Some(paper) = find_paper(&w, &id) else {
        return err(StatusCode::NOT_FOUND, &format!("Paper not found: {id}"));
    };
    match query_value(&pairs, "query") {
        Some(question) => {
            let k = query_value(&pairs, "k")
                .and_then(|k| k.parse().ok())
                .unwrap_or(4usize);
            let q = SearchQuery::parse(question);
            let passages: Vec<(Value, String)> = paper
                .get("passages")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|p| (p.clone(), joined_text(p, &["text", "section"])))
                        .collect()
                })
                .unwrap_or_default();
            let ranked: Vec<Value> = rank(&q, passages).into_iter().take(k).collect();
            ok(j!({ "success": true, "paperId": paper["paperId"], "passages": ranked }))
        }
        None => ok(j!({ "success": true, "paper": paper_summary(paper) })),
    }
}

async fn paper_similar(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
) -> Response {
    let pairs = parse_query(raw.as_deref());
    let mode = query_value(&pairs, "mode").unwrap_or("similar");
    if !["similar", "citers", "references"].contains(&mode) {
        return err(
            StatusCode::BAD_REQUEST,
            "mode must be similar, citers or references",
        );
    }
    let k = query_value(&pairs, "k")
        .and_then(|k| k.parse().ok())
        .unwrap_or(20usize);
    let w = world.lock().expect("world lock");
    let Some(paper) = find_paper(&w, &id) else {
        return err(StatusCode::NOT_FOUND, &format!("Paper not found: {id}"));
    };
    let ids = strings(paper.get("related").and_then(|r| r.get(mode)));
    let found: Vec<Value> = ids
        .iter()
        .filter_map(|i| find_paper(&w, i))
        .map(|p| {
            (
                paper_summary(p),
                joined_text(p, &["title", "abstract", "keywords"]),
            )
        })
        .map(|(p, _)| p)
        .collect();
    let results: Vec<Value> = match query_value(&pairs, "intent") {
        Some(intent) if !intent.trim().is_empty() => {
            let q = SearchQuery::parse(intent);
            let mut matched: Vec<Value> = rank(
                &q,
                found
                    .iter()
                    .map(|p| (p.clone(), joined_text(p, &["title", "abstract"])))
                    .collect(),
            );
            for p in &found {
                if !matched.contains(p) {
                    matched.push(p.clone());
                }
            }
            matched
        }
        _ => found,
    };
    let pool = results.len() * 25;
    let results: Vec<Value> = results.into_iter().take(k).collect();
    ok(j!({ "success": true, "results": results, "poolSize": pool }))
}

// ───────────────────────────── crawl ─────────────────────────────

fn compile_patterns(v: Option<&Value>, field: &str) -> Result<Vec<Regex>, Response> {
    strings(v)
        .iter()
        .map(|p| {
            Regex::new(p).map_err(|_| {
                err(
                    StatusCode::BAD_REQUEST,
                    &format!("Invalid regular expression in {field}: {p}"),
                )
            })
        })
        .collect()
}

fn path_matches(patterns: &[Regex], path: &str) -> bool {
    let bare = path.trim_start_matches('/');
    patterns
        .iter()
        .any(|re| re.is_match(path) || re.is_match(bare))
}

async fn crawl_start(
    State(world): State<SharedWorld>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let input = match body_object(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(url) = input.get("url").and_then(Value::as_str).map(str::to_owned) else {
        return bad_request("url", "Required");
    };
    if !is_http_url(&url) {
        return bad_request("url", "Invalid URL");
    }
    let include = match compile_patterns(input.get("includePaths"), "includePaths") {
        Ok(p) => p,
        Err(r) => return r,
    };
    let exclude = match compile_patterns(input.get("excludePaths"), "excludePaths") {
        Ok(p) => p,
        Err(r) => return r,
    };
    let entire_domain = input.get("crawlEntireDomain") == Some(&Value::Bool(true));
    let subdomains = input.get("allowSubdomains") == Some(&Value::Bool(true));
    let max_depth = usize_of(input.get("maxDiscoveryDepth"));
    let limit = usize_of(input.get("limit")).unwrap_or(10_000).max(1);
    let wanted = formats(input.get("scrapeOptions").and_then(|o| o.get("formats")));
    let host = host_of(&url);
    let start_path = path_of(&url);

    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    let in_scope = |key: &str| {
        let h = host_of(key);
        let path = path_of(key);
        (h == host || (subdomains && host_matches(&h, &host)))
            && (entire_domain
                || start_path == "/"
                || path == start_path
                || path.starts_with(&format!("{start_path}/")))
            && (include.is_empty() || path_matches(&include, &path))
            && !path_matches(&exclude, &path)
            // Discovery depth counts link hops; every page here is one hop from the
            // start page or its sitemap, so only depth 0 (start page alone) narrows it.
            && (max_depth != Some(0) || path == start_path)
    };
    let start_key = find_page(w, &url).map(|(k, _)| k.to_owned());
    let mut pages: Vec<String> = Vec::new();
    if let Some(k) = &start_key
        && in_scope(k)
    {
        pages.push(k.clone());
    }
    for (k, _) in docs(w, "pages") {
        if Some(k) != start_key.as_deref() && in_scope(k) {
            pages.push(k.to_owned());
        }
    }
    pages.truncate(limit);
    let per_page = if wanted.iter().any(|(f, _)| f == "json" || f == "query") {
        5
    } else {
        1
    };
    let id = uuid(w, "crawl");
    let scrape_ids: Vec<String> = pages.iter().map(|_| uuid(w, "scrape")).collect();
    let mut options = input.clone();
    options.remove("url");
    options.remove("origin");
    let job = j!({
        "id": id,
        "url": url,
        "status": "completed",
        "total": pages.len(),
        "completed": pages.len(),
        "creditsUsed": pages.len() * per_page,
        "createdAt": now_iso(w),
        "expiresAt": iso_after(w, DAY_SECS),
        "options": options,
        "pages": pages,
        "scrapeIds": scrape_ids
    });
    insert(w, "crawls", &id, job);
    ok(j!({
        "success": true,
        "id": id,
        "url": format!("{}/v2/crawl/{id}", base_url(&headers))
    }))
}

async fn crawl_status(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    let pairs = parse_query(raw.as_deref());
    let skip = query_value(&pairs, "skip")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0usize);
    let w = world.lock().expect("world lock");
    let Some(job) = doc(&w, "crawls", &id) else {
        return err(StatusCode::NOT_FOUND, "Job not found");
    };
    let pages = strings(job.get("pages"));
    let scrape_ids = strings(job.get("scrapeIds"));
    let wanted = formats(
        job.get("options")
            .and_then(|o| o.get("scrapeOptions"))
            .and_then(|o| o.get("formats")),
    );
    let now = now_iso(&w);
    let data: Vec<Value> = pages
        .iter()
        .enumerate()
        .skip(skip)
        .take(CRAWL_PAGE_SIZE)
        .map(|(i, key)| {
            let page = find_page(&w, key)
                .map(|(_, p)| p.clone())
                .unwrap_or_else(|| not_found_page(key));
            let sid = scrape_ids.get(i).map_or("", String::as_str);
            document(key, key, &page, &wanted, sid, &now)
        })
        .collect();
    let next = (skip + CRAWL_PAGE_SIZE < pages.len()).then(|| {
        format!(
            "{}/v2/crawl/{id}?skip={}",
            base_url(&headers),
            skip + CRAWL_PAGE_SIZE
        )
    });
    ok(j!({
        "success": true,
        "status": job["status"],
        "completed": job["completed"],
        "total": job["total"],
        "creditsUsed": job["creditsUsed"],
        "expiresAt": job["expiresAt"],
        "next": next,
        "data": data
    }))
}

async fn crawl_cancel(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    let Some(job) = doc(w, "crawls", &id) else {
        return err(StatusCode::NOT_FOUND, "Job not found");
    };
    if job["status"] != "scraping" {
        return err(
            StatusCode::CONFLICT,
            &format!("Crawl job is already {}.", str_of(job, "status")),
        );
    }
    if let Some(j) = w
        .service_mut(NAME)
        .store
        .collection_mut("crawls")
        .get_mut(&id)
    {
        j["status"] = j!("cancelled");
    }
    ok(j!({ "success": true, "status": "cancelled" }))
}

async fn crawl_errors(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let w = world.lock().expect("world lock");
    let Some(job) = doc(&w, "crawls", &id) else {
        return err(StatusCode::NOT_FOUND, "Job not found");
    };
    let errors: Vec<Value> = strings(job.get("pages"))
        .iter()
        .filter(|k| find_page(&w, k).is_none())
        .map(|k| j!({ "id": id, "url": k, "error": "Request failed with status code 404" }))
        .collect();
    ok(j!({ "errors": errors, "robotsBlocked": [] }))
}

async fn crawl_active(State(world): State<SharedWorld>) -> Response {
    let w = world.lock().expect("world lock");
    let crawls: Vec<Value> = docs(&w, "crawls")
        .into_iter()
        .filter(|(_, c)| c["status"] == "scraping")
        .map(|(_, c)| j!({ "id": c["id"], "url": c["url"], "options": c["options"] }))
        .collect();
    ok(j!({ "success": true, "crawls": crawls }))
}

// ───────────────────────────── agent ─────────────────────────────

async fn agent_start(State(world): State<SharedWorld>, body: Bytes) -> Response {
    let input = match body_object(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(prompt) = input
        .get("prompt")
        .and_then(Value::as_str)
        .filter(|p| !p.trim().is_empty())
        .map(str::to_owned)
    else {
        return bad_request("prompt", "Required");
    };
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    let thread_id = match input.get("threadId").and_then(Value::as_str) {
        Some(t) => {
            if !docs(w, "agent_jobs")
                .iter()
                .any(|(_, j)| str_of(j, "threadId") == t)
            {
                return err(StatusCode::NOT_FOUND, "Thread not found");
            }
            t.to_owned()
        }
        None => uuid(w, "thread"),
    };
    let turn = docs(w, "agent_jobs")
        .iter()
        .filter(|(_, j)| str_of(j, "threadId") == thread_id)
        .count()
        + 1;
    let urls = strings(input.get("urls"));
    let q = SearchQuery::parse(&format!("{prompt} {}", urls.join(" ")));
    let answer = rank(
        &q,
        docs(w, "agent_answers")
            .into_iter()
            .map(|(_, a)| (a.clone(), joined_text(a, &["keywords", "title"])))
            .collect(),
    )
    .into_iter()
    .next();
    let max_credits = input
        .get("maxCredits")
        .and_then(Value::as_u64)
        .unwrap_or(2500);
    let id = uuid(w, "agent");
    let mut job = j!({
        "id": id,
        "prompt": prompt,
        "urls": urls,
        "schema": input.get("schema").cloned().unwrap_or(Value::Null),
        "effort": input.get("effort").cloned().unwrap_or(j!("low")),
        "maxCredits": max_credits,
        "mode": input.get("mode").cloned().unwrap_or(j!("extract")),
        "threadId": thread_id,
        "threadTurn": turn,
        "model": "spark-1-mini",
        "createdAt": now_iso(w),
        "expiresAt": iso_after(w, DAY_SECS)
    });
    match answer {
        Some(a) => {
            let cost = a.get("creditsUsed").and_then(Value::as_u64).unwrap_or(150);
            if cost > max_credits {
                job["status"] = j!("failed");
                job["stopReason"] = j!("credit_limit_reached");
                job["creditsUsed"] = j!(max_credits);
                job["partial"] = a.get("data").cloned().unwrap_or(Value::Null);
                job["partialSchemaValid"] = j!(false);
                job["error"] = j!("Agent stopped at its credit limit.");
            } else {
                job["status"] = j!("completed");
                job["data"] = a.get("data").cloned().unwrap_or(Value::Null);
                job["sources"] = a.get("sources").cloned().unwrap_or(j!([]));
                job["creditsUsed"] = j!(cost);
                if let Some(m) = a.get("message") {
                    job["message"] = m.clone();
                }
            }
        }
        None => {
            job["status"] = j!("failed");
            job["error"] =
                j!("The agent could not find information matching this request on the web.");
            job["creditsUsed"] = j!(40.min(max_credits));
        }
    }
    insert(w, "agent_jobs", &id, job);
    ok(
        j!({ "success": true, "id": id, "status": "processing", "threadId": thread_id, "threadTurn": turn }),
    )
}

async fn agent_status(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let w = world.lock().expect("world lock");
    let Some(job) = doc(&w, "agent_jobs", &id) else {
        return err(StatusCode::NOT_FOUND, "Agent job not found");
    };
    let mut out = Map::new();
    out.insert("success".to_owned(), j!(true));
    for field in [
        "id",
        "status",
        "data",
        "partial",
        "partialSchemaValid",
        "stopReason",
        "error",
        "message",
        "sources",
        "creditsUsed",
        "expiresAt",
        "model",
        "mode",
        "threadId",
        "threadTurn",
    ] {
        if let Some(v) = job.get(field) {
            out.insert(field.to_owned(), v.clone());
        }
    }
    ok(Value::Object(out))
}

async fn agent_list(State(world): State<SharedWorld>) -> Response {
    let w = world.lock().expect("world lock");
    let data: Vec<Value> = docs(&w, "agent_jobs")
        .into_iter()
        .rev()
        .map(|(_, j)| {
            j!({ "id": j["id"], "status": j["status"], "prompt": j["prompt"], "createdAt": j["createdAt"], "creditsUsed": j["creditsUsed"] })
        })
        .collect();
    ok(j!({ "success": true, "data": data }))
}

async fn agent_cancel(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let w = world.lock().expect("world lock");
    match doc(&w, "agent_jobs", &id) {
        None => err(StatusCode::NOT_FOUND, "Agent job not found"),
        Some(job) => err(
            StatusCode::CONFLICT,
            &format!("Agent job is already {}.", str_of(job, "status")),
        ),
    }
}

// ───────────────────────────── parse ─────────────────────────────

struct Part {
    name: String,
    filename: Option<String>,
    content_type: Option<String>,
    data: Vec<u8>,
}

fn find_bytes(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= hay.len() {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

fn multipart(content_type: &str, body: &[u8]) -> Vec<Part> {
    let Some(boundary) = content_type
        .split(';')
        .find_map(|p| p.trim().strip_prefix("boundary="))
    else {
        return Vec::new();
    };
    let delim = format!("--{}", boundary.trim_matches('"')).into_bytes();
    let mut parts = Vec::new();
    let mut pos = match find_bytes(body, &delim, 0) {
        Some(p) => p + delim.len(),
        None => return parts,
    };
    while let Some(next) = find_bytes(body, &delim, pos) {
        let chunk = &body[pos..next];
        pos = next + delim.len();
        let Some(split) = find_bytes(chunk, b"\r\n\r\n", 0) else {
            continue;
        };
        let head = String::from_utf8_lossy(&chunk[..split]).into_owned();
        let mut data = chunk[split + 4..].to_vec();
        if data.ends_with(b"\r\n") {
            data.truncate(data.len() - 2);
        }
        let disposition = head
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("content-disposition"))
            .unwrap_or("");
        let attr = |key: &str| {
            disposition
                .split(';')
                .find_map(|p| p.trim().strip_prefix(&format!("{key}=")))
                .map(|v| v.trim().trim_matches('"').to_owned())
        };
        let content_type = head.lines().find_map(|l| {
            l.split_once(':')
                .filter(|(k, _)| k.trim().eq_ignore_ascii_case("content-type"))
                .map(|(_, v)| v.trim().to_owned())
        });
        parts.push(Part {
            name: attr("name").unwrap_or_default(),
            filename: attr("filename"),
            content_type,
            data,
        });
    }
    parts
}

fn html_to_markdown(html: &str) -> String {
    let mut s = html.to_owned();
    for level in 1..=6 {
        let re =
            Regex::new(&format!(r"(?is)<h{level}[^>]*>(.*?)</h{level}>")).expect("valid regex");
        s = re
            .replace_all(&s, format!("\n\n{} $1\n\n", "#".repeat(level)).as_str())
            .into_owned();
    }
    let li = Regex::new(r"(?is)<li[^>]*>(.*?)</li>").expect("valid regex");
    s = li.replace_all(&s, "\n- $1").into_owned();
    let block =
        Regex::new(r"(?i)</?(p|div|br|tr|table|ul|ol|section|article)[^>]*>").expect("valid regex");
    s = block.replace_all(&s, "\n\n").into_owned();
    let script = Regex::new(r"(?is)<(script|style|head)[^>]*>.*?</(script|style|head)>")
        .expect("valid regex");
    s = script.replace_all(&s, "").into_owned();
    let tag = Regex::new(r"(?s)<[^>]+>").expect("valid regex");
    s = tag.replace_all(&s, "").into_owned();
    s = s
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    let lines: Vec<&str> = s.lines().map(str::trim).collect();
    let mut out = String::new();
    let mut blank = true;
    for line in lines {
        if line.is_empty() {
            if !blank {
                out.push('\n');
            }
            blank = true;
        } else {
            out.push_str(line);
            out.push('\n');
            blank = false;
        }
    }
    out.trim().to_owned()
}

async fn parse(State(world): State<SharedWorld>, headers: HeaderMap, body: Bytes) -> Response {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    if content_type.starts_with("application/json") {
        let input = match body_object(&body) {
            Ok(v) => v,
            Err(r) => return r,
        };
        let Some(upload_ref) = input.get("uploadRef").and_then(Value::as_str) else {
            return bad_request("file", "A file upload or uploadRef is required");
        };
        let _ = upload_ref;
        return err(
            StatusCode::NOT_FOUND,
            "Upload reference not found or expired. Request a new upload URL and upload the file again.",
        );
    }
    let parts = multipart(content_type, &body);
    let Some(file) = parts.iter().find(|p| p.name == "file") else {
        return bad_request("file", "A file is required");
    };
    let options: Value = parts
        .iter()
        .find(|p| p.name == "options")
        .and_then(|p| serde_json::from_slice(&p.data).ok())
        .unwrap_or(j!({}));
    let filename = file.filename.clone().unwrap_or_else(|| "upload".to_owned());
    let ctype = file
        .content_type
        .clone()
        .unwrap_or_else(|| "application/octet-stream".to_owned());
    let text = String::from_utf8_lossy(&file.data).into_owned();
    let is_html =
        ctype.contains("html") || filename.ends_with(".html") || filename.ends_with(".htm");
    let is_text = ctype.starts_with("text/") || std::str::from_utf8(&file.data).is_ok();
    if !is_html && !is_text {
        return json(
            StatusCode::UNPROCESSABLE_ENTITY,
            j!({
                "success": false,
                "code": "PARSE_FAILED",
                "error": format!("Could not extract text from {filename}: the document has no text layer or is not a supported format.")
            }),
        );
    }
    let markdown = if is_html {
        html_to_markdown(&text)
    } else {
        text.trim().to_owned()
    };
    let title = markdown
        .lines()
        .find_map(|l| l.strip_prefix("# "))
        .unwrap_or(&filename)
        .to_owned();
    let page = j!({ "title": title, "markdown": markdown, "contentType": ctype });
    let now = now_iso(w);
    let scrape_id = uuid(w, "scrape");
    let doc = document(
        &format!("file://{filename}"),
        &filename,
        &page,
        &formats(options.get("formats")),
        &scrape_id,
        &now,
    );
    ok(j!({ "success": true, "data": doc }))
}

async fn parse_upload_url(State(world): State<SharedWorld>, body: Bytes) -> Response {
    let input = match body_object(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(filename) = input.get("filename").and_then(Value::as_str) else {
        return bad_request("filename", "Required");
    };
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    let upload_ref = uuid(w, "upload");
    ok(j!({
        "success": true,
        "data": {
            "uploadUrl": format!("https://storage.googleapis.com/firecrawl-parse-uploads/{upload_ref}/{filename}"),
            "uploadRef": upload_ref,
            "method": "PUT",
            "headers": { "Content-Type": input.get("contentType").cloned().unwrap_or(j!("application/octet-stream")) },
            "expiresAt": iso_after(w, 900),
            "maxSizeBytes": 52_428_800
        }
    }))
}

// ───────────────────────────── credit usage ─────────────────────────────

fn team(world: &WorldState) -> Value {
    doc(world, "team", "team").cloned().unwrap_or_else(|| {
        j!({
            "remainingCredits": 100_000,
            "planCredits": 100_000,
            "billingPeriodStart": null,
            "billingPeriodEnd": null,
            "history": []
        })
    })
}

async fn credit_usage(State(world): State<SharedWorld>) -> Response {
    let w = world.lock().expect("world lock");
    let t = team(&w);
    ok(j!({
        "success": true,
        "data": {
            "remainingCredits": t["remainingCredits"],
            "planCredits": t["planCredits"],
            "billingPeriodStart": t["billingPeriodStart"],
            "billingPeriodEnd": t["billingPeriodEnd"]
        }
    }))
}

async fn credit_usage_historical(
    State(world): State<SharedWorld>,
    RawQuery(raw): RawQuery,
) -> Response {
    let pairs = parse_query(raw.as_deref());
    let by_endpoint = query_value(&pairs, "byEndpoint") == Some("true");
    let by_key = query_value(&pairs, "byApiKey") == Some("true");
    let w = world.lock().expect("world lock");
    let history = team(&w)["history"].as_array().cloned().unwrap_or_default();
    let mut periods = Vec::new();
    for p in &history {
        let base = |extra: (&str, &Value), credits: &Value| {
            let mut o = Map::new();
            o.insert("startDate".to_owned(), p["startDate"].clone());
            o.insert("endDate".to_owned(), p["endDate"].clone());
            o.insert(extra.0.to_owned(), extra.1.clone());
            o.insert("creditsUsed".to_owned(), credits.clone());
            Value::Object(o)
        };
        let split = if by_endpoint {
            Some(("endpoint", "endpoints"))
        } else if by_key {
            Some(("apiKey", "apiKeys"))
        } else {
            None
        };
        match split {
            Some((label, field)) => {
                if let Some(parts) = p.get(field).and_then(Value::as_object) {
                    for (name, credits) in parts {
                        periods.push(base((label, &j!(name)), credits));
                    }
                }
            }
            None => periods.push(j!({
                "startDate": p["startDate"],
                "endDate": p["endDate"],
                "creditsUsed": p["creditsUsed"]
            })),
        }
    }
    let mut out = j!({ "success": true, "periods": periods });
    if by_endpoint {
        out["bin"] = j!("month");
    }
    ok(out)
}

// ───────────────────────────── monitors ─────────────────────────────

fn normalize_schedule(schedule: &Value) -> Result<Value, Response> {
    let obj = match schedule {
        Value::String(s) => {
            let mut o = Map::new();
            o.insert("text".to_owned(), j!(s));
            o
        }
        Value::Object(o) => o.clone(),
        _ => return Err(bad_request("schedule", "Required")),
    };
    let text = obj.get("text").and_then(Value::as_str);
    let cron = obj.get("cron").and_then(Value::as_str);
    if text.is_none() && cron.is_none() {
        return Err(bad_request(
            "schedule",
            "schedule.text or schedule.cron is required",
        ));
    }
    let interval = text
        .and_then(schedule_interval_minutes)
        .or_else(|| cron.and_then(text::cron_interval_minutes))
        .or_else(|| obj.get("intervalMinutes").and_then(Value::as_u64));
    let Some(interval) = interval else {
        return Err(bad_request(
            "schedule",
            &format!(
                "Could not understand the schedule {:?}. Use a phrase like \"every 30 minutes\", \"daily at 9am\", or a cron expression.",
                text.or(cron).unwrap_or("")
            ),
        ));
    };
    if interval < 5 {
        return Err(bad_request(
            "schedule",
            "Monitors can run at most every 5 minutes.",
        ));
    }
    let mut out = obj;
    out.entry("timezone").or_insert(j!("UTC"));
    out.insert("intervalMinutes".to_owned(), j!(interval));
    Ok(Value::Object(out))
}

fn normalize_targets(targets: Option<&Value>) -> Result<Value, Response> {
    let Some(list) = targets.and_then(Value::as_array).filter(|t| !t.is_empty()) else {
        return Err(bad_request("targets", "At least one target is required"));
    };
    let mut out = Vec::new();
    for (i, t) in list.iter().enumerate() {
        let mut t = t.as_object().cloned().unwrap_or_default();
        if let Some(Value::String(u)) = t.remove("url") {
            t.insert("urls".to_owned(), j!([u]));
        }
        let kind = t
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or(if t.contains_key("queries") {
                "search"
            } else {
                "scrape"
            })
            .to_owned();
        t.insert("type".to_owned(), j!(kind));
        match kind.as_str() {
            "scrape" | "crawl" => {
                let urls = strings(t.get("urls"));
                if urls.is_empty() || urls.iter().any(|u| !is_http_url(u)) {
                    return Err(bad_request(
                        &format!("targets.{i}.urls"),
                        "Expected one or more valid URLs",
                    ));
                }
            }
            "search" => {
                if strings(t.get("queries")).is_empty() {
                    return Err(bad_request(
                        &format!("targets.{i}.queries"),
                        "Expected one or more queries",
                    ));
                }
            }
            _ => {
                return Err(bad_request(
                    &format!("targets.{i}.type"),
                    "Expected 'scrape', 'crawl' or 'search'",
                ));
            }
        }
        out.push(Value::Object(t));
    }
    Ok(Value::Array(out))
}

fn status_from(body: &Map<String, Value>) -> Result<Option<&'static str>, Response> {
    if let Some(s) = body.get("status") {
        return match s.as_str() {
            Some("active") => Ok(Some("active")),
            Some("paused") => Ok(Some("paused")),
            _ => Err(bad_request("status", "Expected 'active' or 'paused'")),
        };
    }
    if let Some(Value::Bool(a)) = body.get("active").or(body.get("enabled")) {
        return Ok(Some(if *a { "active" } else { "paused" }));
    }
    if let Some(Value::Bool(p)) = body.get("paused") {
        return Ok(Some(if *p { "paused" } else { "active" }));
    }
    Ok(None)
}

fn next_run(world: &WorldState, monitor: &Value) -> Value {
    if monitor["status"] != "active" {
        return Value::Null;
    }
    match monitor["schedule"]["intervalMinutes"].as_i64() {
        Some(m) => j!(iso_after(world, m * 60)),
        None => Value::Null,
    }
}

fn monitor_not_found() -> Response {
    err(StatusCode::NOT_FOUND, "Monitor not found")
}

async fn monitor_create(State(world): State<SharedWorld>, body: Bytes) -> Response {
    let input = match body_object(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let targets = match normalize_targets(input.get("targets")) {
        Ok(t) => t,
        Err(r) => return r,
    };
    let schedule = match normalize_schedule(input.get("schedule").unwrap_or(&Value::Null)) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let status = match status_from(&input) {
        Ok(s) => s.unwrap_or("active"),
        Err(r) => return r,
    };
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    let id = uuid(w, "monitor");
    let now = now_iso(w);
    let default_name = targets[0]
        .get("urls")
        .and_then(|u| u.get(0))
        .or(targets[0].get("queries").and_then(|q| q.get(0)))
        .and_then(Value::as_str)
        .map(|s| format!("Monitor {s}"))
        .unwrap_or_else(|| "Monitor".to_owned());
    let mut monitor = Map::new();
    monitor.insert("id".to_owned(), j!(id));
    monitor.insert(
        "name".to_owned(),
        input.get("name").cloned().unwrap_or(j!(default_name)),
    );
    monitor.insert("status".to_owned(), j!(status));
    monitor.insert("schedule".to_owned(), schedule);
    monitor.insert(
        "goal".to_owned(),
        input.get("goal").cloned().unwrap_or(Value::Null),
    );
    monitor.insert("targets".to_owned(), targets);
    for (k, v) in &input {
        if !matches!(
            k.as_str(),
            "id" | "name"
                | "status"
                | "active"
                | "enabled"
                | "paused"
                | "schedule"
                | "goal"
                | "targets"
                | "origin"
                | "integration"
        ) {
            monitor.insert(k.clone(), v.clone());
        }
    }
    monitor.entry("retentionDays").or_insert(j!(30));
    monitor.insert("createdAt".to_owned(), j!(now));
    monitor.insert("updatedAt".to_owned(), j!(now));
    monitor.insert("lastRunAt".to_owned(), Value::Null);
    monitor.insert("lastCheckId".to_owned(), Value::Null);
    let mut monitor = Value::Object(monitor);
    monitor["nextRunAt"] = next_run(w, &monitor);
    insert(w, "monitors", &id, monitor.clone());
    ok(j!({ "success": true, "data": monitor }))
}

async fn monitor_list(State(world): State<SharedWorld>, RawQuery(raw): RawQuery) -> Response {
    let pairs = parse_query(raw.as_deref());
    let limit = query_value(&pairs, "limit")
        .and_then(|l| l.parse().ok())
        .unwrap_or(20usize)
        .clamp(1, 100);
    let offset = query_value(&pairs, "offset")
        .and_then(|o| o.parse().ok())
        .unwrap_or(0usize);
    let w = world.lock().expect("world lock");
    let all = docs(&w, "monitors");
    let data: Vec<Value> = all
        .iter()
        .skip(offset)
        .take(limit)
        .map(|(_, m)| (*m).clone())
        .collect();
    ok(j!({
        "success": true,
        "data": data,
        "pagination": { "limit": limit, "offset": offset, "total": all.len(), "hasMore": offset + limit < all.len() }
    }))
}

async fn monitor_get(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let w = world.lock().expect("world lock");
    match doc(&w, "monitors", &id) {
        Some(m) => ok(j!({ "success": true, "data": m })),
        None => monitor_not_found(),
    }
}

async fn monitor_update(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let input = match body_object(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if input.is_empty() {
        return bad_request("body", "At least one field must be updated");
    }
    let status = match status_from(&input) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let schedule = match input.get("schedule").map(normalize_schedule) {
        Some(Err(r)) => return r,
        Some(Ok(s)) => Some(s),
        None => None,
    };
    let targets = match input.get("targets").map(|t| normalize_targets(Some(t))) {
        Some(Err(r)) => return r,
        Some(Ok(t)) => Some(t),
        None => None,
    };
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    let Some(mut monitor) = doc(w, "monitors", &id).cloned() else {
        return monitor_not_found();
    };
    for (k, v) in &input {
        if !matches!(
            k.as_str(),
            "id" | "status"
                | "active"
                | "enabled"
                | "paused"
                | "schedule"
                | "targets"
                | "origin"
                | "createdAt"
                | "updatedAt"
                | "lastRunAt"
                | "lastCheckId"
                | "nextRunAt"
        ) {
            monitor[k] = v.clone();
        }
    }
    if let Some(s) = status {
        monitor["status"] = j!(s);
    }
    if let Some(s) = schedule {
        monitor["schedule"] = s;
    }
    if let Some(t) = targets {
        monitor["targets"] = t;
    }
    monitor["updatedAt"] = j!(now_iso(w));
    monitor["nextRunAt"] = next_run(w, &monitor);
    insert(w, "monitors", &id, monitor.clone());
    ok(j!({ "success": true, "data": monitor }))
}

async fn monitor_delete(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    match w
        .service_mut(NAME)
        .store
        .collection_mut("monitors")
        .remove(&id)
    {
        Some(_) => ok(j!({ "success": true, "data": { "id": id, "deleted": true } })),
        None => monitor_not_found(),
    }
}

async fn monitor_run(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    let Some(monitor) = doc(w, "monitors", &id).cloned() else {
        return monitor_not_found();
    };
    let mut pages = Vec::new();
    for target in monitor["targets"].as_array().cloned().unwrap_or_default() {
        for url in strings(target.get("urls")) {
            let page = match find_page(w, &url) {
                Some((_, p))
                    if p.get("statusCode").and_then(Value::as_u64).unwrap_or(200) < 400 =>
                {
                    j!({ "url": url, "status": "same" })
                }
                Some(_) | None => j!({
                    "url": url,
                    "status": "error",
                    "error": if known_host(w, &host_of(&url)) { "Page returned HTTP 404" } else { "DNS resolution failed" }
                }),
            };
            pages.push(page);
        }
        for query in strings(target.get("queries")) {
            pages.push(j!({ "url": format!("search:{query}"), "status": "same" }));
        }
    }
    let count = |s: &str| pages.iter().filter(|p| p["status"] == s).count();
    let summary = j!({
        "same": count("same"), "new": count("new"), "changed": count("changed"),
        "removed": count("removed"), "error": count("error")
    });
    let status = if count("error") == 0 {
        "completed"
    } else if count("error") == pages.len() {
        "failed"
    } else {
        "partial"
    };
    let check_id = uuid(w, "monitor_check");
    let now = now_iso(w);
    insert(
        w,
        "monitor_checks",
        &check_id,
        j!({
            "id": check_id, "monitorId": id, "status": status, "trigger": "manual",
            "createdAt": now, "startedAt": now, "completedAt": now,
            "summary": summary, "pages": pages
        }),
    );
    if let Some(m) = w
        .service_mut(NAME)
        .store
        .collection_mut("monitors")
        .get_mut(&id)
    {
        m["lastRunAt"] = j!(now);
        m["lastCheckId"] = j!(check_id);
    }
    ok(j!({
        "success": true,
        "data": { "id": check_id, "monitorId": id, "status": "queued", "trigger": "manual", "createdAt": now }
    }))
}

fn checks_of<'a>(world: &'a WorldState, monitor_id: &str) -> Vec<&'a Value> {
    let mut checks: Vec<(usize, &Value)> = docs(world, "monitor_checks")
        .into_iter()
        .filter(|(_, c)| str_of(c, "monitorId") == monitor_id)
        .map(|(_, c)| c)
        .enumerate()
        .collect();
    checks.sort_by(|a, b| {
        str_of(b.1, "createdAt")
            .cmp(str_of(a.1, "createdAt"))
            .then(b.0.cmp(&a.0))
    });
    checks.into_iter().map(|(_, c)| c).collect()
}

async fn monitor_checks(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
) -> Response {
    let pairs = parse_query(raw.as_deref());
    let limit = query_value(&pairs, "limit")
        .and_then(|l| l.parse().ok())
        .unwrap_or(20usize)
        .clamp(1, 100);
    let offset = query_value(&pairs, "offset")
        .and_then(|o| o.parse().ok())
        .unwrap_or(0usize);
    let status = query_value(&pairs, "status");
    let w = world.lock().expect("world lock");
    if doc(&w, "monitors", &id).is_none() {
        return monitor_not_found();
    }
    let all: Vec<&Value> = checks_of(&w, &id)
        .into_iter()
        .filter(|c| status.is_none_or(|s| str_of(c, "status") == s))
        .collect();
    let data: Vec<Value> = all
        .iter()
        .skip(offset)
        .take(limit)
        .map(|c| {
            let mut c = (*c).clone();
            if let Value::Object(o) = &mut c {
                o.remove("pages");
            }
            c
        })
        .collect();
    ok(j!({
        "success": true,
        "data": data,
        "pagination": { "limit": limit, "offset": offset, "total": all.len(), "hasMore": offset + limit < all.len() }
    }))
}

async fn monitor_check(
    State(world): State<SharedWorld>,
    Path((id, check_id)): Path<(String, String)>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    let pairs = parse_query(raw.as_deref());
    let limit = query_value(&pairs, "limit")
        .and_then(|l| l.parse().ok())
        .unwrap_or(50usize)
        .clamp(1, 500);
    let skip = query_value(&pairs, "skip")
        .and_then(|o| o.parse().ok())
        .unwrap_or(0usize);
    let status = query_value(&pairs, "status");
    let w = world.lock().expect("world lock");
    if doc(&w, "monitors", &id).is_none() {
        return monitor_not_found();
    }
    let Some(check) = doc(&w, "monitor_checks", &check_id).filter(|c| str_of(c, "monitorId") == id)
    else {
        return err(StatusCode::NOT_FOUND, "Check not found");
    };
    let pages: Vec<Value> = check["pages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|p| status.is_none_or(|s| str_of(p, "status") == s))
        .collect();
    let total = pages.len();
    let mut data = check.clone();
    data["pages"] = Value::Array(pages.into_iter().skip(skip).take(limit).collect());
    let next = (skip + limit < total).then(|| {
        format!(
            "{}/v2/monitor/{id}/checks/{check_id}?skip={}&limit={limit}",
            base_url(&headers),
            skip + limit
        )
    });
    ok(j!({
        "success": true,
        "data": data,
        "pagination": { "limit": limit, "skip": skip, "total": total },
        "next": next
    }))
}
