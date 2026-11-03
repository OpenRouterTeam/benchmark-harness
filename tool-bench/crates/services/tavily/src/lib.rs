//! Mock Tavily API (`api.tavily.com`, real-tls), as called by `tavily-mcp` 0.2.22:
//! `POST /search`, `/extract`, `/crawl`, `/map`, `/research` and `GET /research/{id}`.
//!
//! The world is "the web as Tavily indexed it at `now`". Dataset collections:
//!
//! | Collection | Key | Content |
//! |---|---|---|
//! | `pages` | URL | `url`, `title`, `content` (snippet), `raw_content` (markdown), optional `published_date` (`YYYY-MM-DD`), `links` (absolute URLs), `images` (`[{url, description}]`), `favicon`, `keywords` |
//! | `research_reports` | any | canned research output: `match` (terms that must all appear in the input, each optionally `alt1|alt2`), `content`, `sources` (`[{title, url}]`) |
//! | `research` | request id | research requests; created by `POST /research` |
//!
//! Search ranks pages by how many significant query terms they contain; queries that
//! match nothing return an empty `results` list, as Tavily does for obscure queries.

// Handlers return early with a ready `Response` as the error; boxing it buys nothing here.
#![allow(clippy::result_large_err)]

pub mod text;

use std::collections::{BTreeSet, VecDeque};

use axum::{
    Router,
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use mock_core::{
    clock::format_unix,
    http::json,
    service::MockService,
    world::{SharedWorld, WorldState},
};
use regex::Regex;
use serde_json::{Map, Value, json as json_value};
use text::{SearchQuery, host_matches, host_of, norm_url, path_of, tokens};

pub const NAME: &str = "tavily";
pub const PORT: u16 = 8105;

pub struct Tavily;

impl MockService for Tavily {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port(&self) -> u16 {
        PORT
    }

    fn hosts(&self) -> &'static [&'static str] {
        &["api.tavily.com"]
    }

    fn router(&self) -> Router<SharedWorld> {
        Router::new()
            .route("/search", post(search))
            .route("/extract", post(extract))
            .route("/crawl", post(crawl))
            .route("/map", post(map))
            .route("/research", post(research_create))
            .route("/research/{id}", get(research_get))
            .fallback(not_found)
    }
}

// ───────────────────────────── errors & helpers ─────────────────────────────

fn detail(status: StatusCode, message: &str) -> Response {
    json(status, json_value!({ "detail": { "error": message } }))
}

async fn not_found() -> Response {
    json(
        StatusCode::NOT_FOUND,
        json_value!({ "detail": "Not Found" }),
    )
}

fn unauthorized() -> Response {
    detail(
        StatusCode::UNAUTHORIZED,
        "Unauthorized: missing or invalid API key.",
    )
}

/// Parses the body and checks the API key (`Authorization: Bearer tvly-...` or `api_key`).
fn begin(headers: &HeaderMap, body: &Bytes) -> Result<Map<String, Value>, Response> {
    let input: Value = if body.is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_slice(body).map_err(|_| {
            json(
                StatusCode::UNPROCESSABLE_ENTITY,
                json_value!({ "detail": [{ "type": "json_invalid", "loc": ["body"], "msg": "JSON decode error", "input": {} }] }),
            )
        })?
    };
    let Value::Object(input) = input else {
        return Err(detail(
            StatusCode::BAD_REQUEST,
            "Request body must be a JSON object.",
        ));
    };
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim);
    let key = bearer.or_else(|| input.get("api_key").and_then(Value::as_str));
    match key {
        Some(k) if k.starts_with("tvly-") && k.len() > 5 => Ok(input),
        _ => Err(unauthorized()),
    }
}

fn str_arg<'a>(input: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

fn bool_arg(input: &Map<String, Value>, key: &str) -> bool {
    match input.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !s.is_empty() && s != "false",
        _ => false,
    }
}

fn int_arg(input: &Map<String, Value>, key: &str, default: i64) -> i64 {
    input
        .get(key)
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
        .unwrap_or(default)
}

fn str_list(input: &Map<String, Value>, key: &str) -> Vec<String> {
    match input.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Some(Value::String(s)) if !s.is_empty() => vec![s.clone()],
        _ => Vec::new(),
    }
}

fn pages(world: &WorldState) -> Vec<Value> {
    world
        .service(NAME)
        .and_then(|s| s.store.collection("pages"))
        .map(|c| c.values().cloned().collect())
        .unwrap_or_default()
}

fn page_url(page: &Value) -> String {
    page["url"].as_str().unwrap_or_default().to_owned()
}

fn find_page<'a>(pages: &'a [Value], url: &str) -> Option<&'a Value> {
    let wanted = norm_url(url);
    pages.iter().find(|p| norm_url(&page_url(p)) == wanted)
}

fn request_id(world: &mut WorldState) -> String {
    world.service_mut(NAME).ids.next_uuid("request").to_string()
}

/// Markdown to plain text: drops link targets, emphasis, headings and table pipes.
fn plain_text(md: &str) -> String {
    let link = Regex::new(r"!?\[([^\]]*)\]\([^)]*\)").expect("valid regex");
    let text = link.replace_all(md, "$1");
    text.lines()
        .map(|l| {
            l.trim_start_matches('#')
                .replace("**", "")
                .replace('`', "")
                .replace('|', " ")
                .trim()
                .to_owned()
        })
        .filter(|l| !l.chars().all(|c| c == '-' || c == ' ' || c == ':'))
        .collect::<Vec<_>>()
        .join("\n")
}

fn formatted(raw: &str, format: Option<&str>) -> String {
    if format == Some("text") {
        plain_text(raw)
    } else {
        raw.to_owned()
    }
}

/// The `chunks` most relevant paragraphs of `raw` for `query`, in document order,
/// joined the way Tavily joins reranked chunks.
fn chunks_for(raw: &str, query: &str, chunks: usize) -> String {
    let q = SearchQuery::parse(query);
    let paragraphs: Vec<&str> = raw
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let mut scored: Vec<(usize, usize)> = paragraphs
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let toks: BTreeSet<String> = tokens(p).into_iter().collect();
            (i, q.terms.iter().filter(|t| toks.contains(*t)).count())
        })
        .filter(|(_, n)| *n > 0)
        .collect();
    if scored.is_empty() {
        return paragraphs
            .iter()
            .take(chunks)
            .copied()
            .collect::<Vec<_>>()
            .join(" [...] ");
    }
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut keep: Vec<usize> = scored.iter().take(chunks).map(|(i, _)| *i).collect();
    keep.sort_unstable();
    keep.iter()
        .map(|i| paragraphs[*i])
        .collect::<Vec<_>>()
        .join(" [...] ")
}

/// Days since 1970-01-01 of a `YYYY-MM-DD` date (longer timestamps are truncated).
fn date_days(date: &str) -> Option<i64> {
    let d = date.get(..10)?;
    let mut parts = d.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || d.as_bytes()[4] != b'-' {
        return None;
    }
    // Howard Hinnant's days_from_civil.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

fn today_days(world: &WorldState) -> i64 {
    date_days(&format_unix(world.clock.now_unix())).unwrap_or_default()
}

fn round(value: f64, places: i32) -> f64 {
    let f = 10f64.powi(places);
    (value * f).round() / f
}

// ───────────────────────────────── search ─────────────────────────────────

async fn search(State(world): State<SharedWorld>, headers: HeaderMap, body: Bytes) -> Response {
    let input = match begin(&headers, &body) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let Some(query) = str_arg(&input, "query") else {
        return detail(StatusCode::BAD_REQUEST, "Query is missing.");
    };
    if query.chars().count() > 400 {
        return detail(
            StatusCode::BAD_REQUEST,
            "Query is too long. Max query length is 400 characters.",
        );
    }
    let depth = str_arg(&input, "search_depth").unwrap_or("basic");
    if !["basic", "advanced", "fast", "ultra-fast"].contains(&depth) {
        return detail(
            StatusCode::BAD_REQUEST,
            "Invalid search depth. Must be 'basic', 'advanced', 'fast' or 'ultra-fast'.",
        );
    }
    let topic = str_arg(&input, "topic").unwrap_or("general");
    if !["general", "news", "finance"].contains(&topic) {
        return detail(
            StatusCode::BAD_REQUEST,
            "Invalid topic. Must be 'general', 'news' or 'finance'.",
        );
    }
    let max_results = int_arg(&input, "max_results", 5).clamp(0, 20) as usize;
    let include_domains = str_list(&input, "include_domains");
    let exclude_domains = str_list(&input, "exclude_domains");
    let exact = bool_arg(&input, "exact_match");

    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    let today = today_days(world);
    let window = match str_arg(&input, "time_range") {
        Some("day" | "d") => Some(1),
        Some("week" | "w") => Some(7),
        Some("month" | "m") => Some(31),
        Some("year" | "y") => Some(365),
        Some(_) => {
            return detail(
                StatusCode::BAD_REQUEST,
                "Invalid time range. Must be 'day', 'week', 'month' or 'year'.",
            );
        }
        None => None,
    };
    let start = str_arg(&input, "start_date").map(date_days);
    let end = str_arg(&input, "end_date").map(date_days);
    if matches!(start, Some(None)) || matches!(end, Some(None)) {
        return detail(
            StatusCode::BAD_REQUEST,
            "Invalid date format. Dates must be in the format YYYY-MM-DD.",
        );
    }
    let (start, end) = (start.flatten(), end.flatten());

    let mut q = SearchQuery::parse(query);
    if !exact {
        q.phrases.clear();
    }
    let mut hits: Vec<(usize, usize, Value)> = Vec::new();
    for (order, page) in pages(world).into_iter().enumerate() {
        let url = page_url(&page);
        let host = host_of(&url);
        if !include_domains.is_empty() && !include_domains.iter().any(|d| host_matches(&host, d)) {
            continue;
        }
        if exclude_domains.iter().any(|d| host_matches(&host, d)) {
            continue;
        }
        if q.sites.iter().any(|s| !host_matches(&host, s)) {
            continue;
        }
        let published = page["published_date"].as_str().and_then(date_days);
        if window.is_some() || start.is_some() || end.is_some() || topic == "news" {
            let Some(p) = published else { continue };
            if window.is_some_and(|w| p < today - w)
                || start.is_some_and(|s| p < s)
                || end.is_some_and(|e| p > e)
            {
                continue;
            }
        }
        let haystack = format!(
            "{} {} {} {} {}",
            page["title"].as_str().unwrap_or_default(),
            page["content"].as_str().unwrap_or_default(),
            page["raw_content"].as_str().unwrap_or_default(),
            page["keywords"].as_str().unwrap_or_default(),
            url
        );
        if let Some(matched) = q.score(&haystack) {
            hits.push((matched, order, page));
        }
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let terms = q.terms.len().max(1);

    let raw_mode = match input.get("include_raw_content") {
        Some(Value::Bool(true)) => Some("markdown"),
        Some(Value::String(s)) if s == "text" => Some("text"),
        Some(Value::String(s)) if s == "markdown" || s == "true" => Some("markdown"),
        _ => None,
    };
    let favicon = bool_arg(&input, "include_favicon");
    let mut results = Vec::new();
    let mut images = Vec::new();
    for (rank, (matched, _, page)) in hits.iter().take(max_results).enumerate() {
        let score = round(
            0.35 + 0.6 * (*matched as f64 / terms as f64) - 0.01 * rank as f64,
            5,
        );
        let mut r = Map::new();
        r.insert("url".into(), page["url"].clone());
        r.insert("title".into(), page["title"].clone());
        r.insert("content".into(), page["content"].clone());
        r.insert("score".into(), json_value!(score));
        if topic == "news" {
            r.insert("published_date".into(), page["published_date"].clone());
        }
        let raw = page["raw_content"].as_str().unwrap_or_default();
        r.insert(
            "raw_content".into(),
            raw_mode.map_or(Value::Null, |m| Value::String(formatted(raw, Some(m)))),
        );
        if favicon {
            r.insert("favicon".into(), page["favicon"].clone());
        }
        results.push(Value::Object(r));
        if let Some(list) = page["images"].as_array() {
            images.extend(list.iter().cloned());
        }
    }
    let images: Vec<Value> = if bool_arg(&input, "include_images") {
        images
            .into_iter()
            .map(|img| {
                if bool_arg(&input, "include_image_descriptions") {
                    json_value!({ "url": img["url"], "description": img["description"] })
                } else {
                    img["url"].clone()
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    let answer = if bool_arg(&input, "include_answer") {
        results.first().map_or(Value::Null, |r| {
            Value::String(r["content"].as_str().unwrap_or_default().to_owned())
        })
    } else {
        Value::Null
    };
    let response_time = match depth {
        "advanced" => 1.87,
        "fast" => 0.61,
        "ultra-fast" => 0.32,
        _ => 0.92,
    };
    let id = request_id(world);
    json(
        StatusCode::OK,
        json_value!({
            "query": query,
            "follow_up_questions": null,
            "answer": answer,
            "images": images,
            "results": results,
            "response_time": response_time,
            "request_id": id,
        }),
    )
}

// ───────────────────────────────── extract ─────────────────────────────────

async fn extract(State(world): State<SharedWorld>, headers: HeaderMap, body: Bytes) -> Response {
    let input = match begin(&headers, &body) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let urls = str_list(&input, "urls");
    if urls.is_empty() {
        return detail(StatusCode::BAD_REQUEST, "No URLs provided.");
    }
    if urls.len() > 20 {
        return detail(
            StatusCode::BAD_REQUEST,
            "Max 20 URLs are allowed per request.",
        );
    }
    let depth = str_arg(&input, "extract_depth").unwrap_or("basic");
    if !["basic", "advanced"].contains(&depth) {
        return detail(
            StatusCode::BAD_REQUEST,
            "Invalid extract depth. Must be 'basic' or 'advanced'.",
        );
    }
    let format = str_arg(&input, "format");
    if format.is_some_and(|f| f != "markdown" && f != "text") {
        return detail(
            StatusCode::BAD_REQUEST,
            "Invalid format. Must be 'markdown' or 'text'.",
        );
    }
    let query = str_arg(&input, "query");
    let chunks = int_arg(&input, "chunks_per_source", 3).clamp(1, 5) as usize;
    let images = bool_arg(&input, "include_images");
    let favicon = bool_arg(&input, "include_favicon");

    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    let all = pages(world);
    let mut results = Vec::new();
    let mut failed = Vec::new();
    for url in &urls {
        match find_page(&all, url) {
            Some(page) => {
                let raw = page["raw_content"].as_str().unwrap_or_default();
                let raw = match query {
                    Some(q) => chunks_for(raw, q, chunks),
                    None => raw.to_owned(),
                };
                let mut r = Map::new();
                r.insert("url".into(), Value::String(url.clone()));
                r.insert("raw_content".into(), Value::String(formatted(&raw, format)));
                let imgs: Vec<Value> = if images {
                    page["images"]
                        .as_array()
                        .map(|l| l.iter().map(|i| i["url"].clone()).collect())
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                r.insert("images".into(), Value::Array(imgs));
                if favicon {
                    r.insert("favicon".into(), page["favicon"].clone());
                }
                results.push(Value::Object(r));
            }
            None => failed.push(json_value!({ "url": url, "error": "Failed to fetch url" })),
        }
    }
    let id = request_id(world);
    json(
        StatusCode::OK,
        json_value!({
            "results": results,
            "failed_results": failed,
            "response_time": if depth == "advanced" { 2.14 } else { 0.41 },
            "request_id": id,
        }),
    )
}

// ────────────────────────────── crawl and map ──────────────────────────────

struct Walk {
    /// Pages reached, in BFS order (only pages that exist in the dataset).
    pages: Vec<Value>,
    /// Every URL discovered (internal and, if allowed, external), in discovery order.
    urls: Vec<String>,
}

fn compile(patterns: &[String]) -> Result<Vec<Regex>, Response> {
    patterns
        .iter()
        .map(|p| {
            Regex::new(p).map_err(|_| {
                detail(
                    StatusCode::BAD_REQUEST,
                    &format!("Invalid regex pattern: {p}"),
                )
            })
        })
        .collect()
}

/// Breadth-first walk over `links` from `root`, honouring depth, breadth, limit and the
/// path/domain selectors the way Tavily's crawler does.
fn walk(all: &[Value], root: &str, input: &Map<String, Value>) -> Result<Walk, Response> {
    let max_depth = int_arg(input, "max_depth", 1).clamp(1, 5);
    let max_breadth = int_arg(input, "max_breadth", 20).clamp(1, 500) as usize;
    let limit = int_arg(input, "limit", 50).max(1) as usize;
    let select_paths = compile(&str_list(input, "select_paths"))?;
    let exclude_paths = compile(&str_list(input, "exclude_paths"))?;
    let select_domains = compile(&str_list(input, "select_domains"))?;
    let exclude_domains = compile(&str_list(input, "exclude_domains"))?;
    let allow_external = input
        .get("allow_external")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let root_host = host_of(root);

    let in_scope = |url: &str| {
        let host = host_of(url);
        let domain_ok = if select_domains.is_empty() {
            host == root_host
        } else {
            select_domains.iter().any(|r| r.is_match(&host))
        };
        domain_ok && !exclude_domains.iter().any(|r| r.is_match(&host))
    };
    let path_ok = |url: &str| {
        let path = path_of(url);
        (select_paths.is_empty() || select_paths.iter().any(|r| r.is_match(&path)))
            && !exclude_paths.iter().any(|r| r.is_match(&path))
    };

    let mut seen = BTreeSet::new();
    let mut out = Walk {
        pages: Vec::new(),
        urls: Vec::new(),
    };
    // (url, depth, external): external links are listed but never fetched.
    let mut queue = VecDeque::new();
    seen.insert(norm_url(root));
    queue.push_back((root.to_owned(), 0, false));
    while let Some((url, depth, external)) = queue.pop_front() {
        if out.urls.len() >= limit {
            break;
        }
        if external {
            out.urls.push(url);
            continue;
        }
        let page = find_page(all, &url);
        let canonical = page.map_or(url.clone(), page_url);
        if path_ok(&canonical) {
            out.urls.push(canonical);
            if let Some(p) = page {
                out.pages.push(p.clone());
            }
        }
        let Some(page) = page else { continue };
        if depth >= max_depth {
            continue;
        }
        let links: Vec<String> = page["links"]
            .as_array()
            .map(|l| {
                l.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        for link in links.into_iter().take(max_breadth) {
            if !seen.insert(norm_url(&link)) {
                continue;
            }
            if in_scope(&link) {
                queue.push_back((link, depth + 1, false));
            } else if allow_external && path_ok(&link) {
                queue.push_back((link, depth + 1, true));
            }
        }
    }
    Ok(out)
}

fn root_url(input: &Map<String, Value>) -> Result<String, Response> {
    let Some(url) = str_arg(input, "url") else {
        return Err(detail(StatusCode::BAD_REQUEST, "URL is missing."));
    };
    let url = url.trim();
    if url.contains("://") {
        Ok(url.to_owned())
    } else {
        Ok(format!("https://{url}"))
    }
}

async fn crawl(State(world): State<SharedWorld>, headers: HeaderMap, body: Bytes) -> Response {
    let input = match begin(&headers, &body) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let root = match root_url(&input) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let format = str_arg(&input, "format");
    if format.is_some_and(|f| f != "markdown" && f != "text") {
        return detail(
            StatusCode::BAD_REQUEST,
            "Invalid format. Must be 'markdown' or 'text'.",
        );
    }
    let favicon = bool_arg(&input, "include_favicon");
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    let all = pages(world);
    let walked = match walk(&all, &root, &input) {
        Ok(w) => w,
        Err(r) => return r,
    };
    let results: Vec<Value> = walked
        .pages
        .iter()
        .filter(|p| walked.urls.contains(&page_url(p)))
        .map(|p| {
            let mut r = Map::new();
            r.insert("url".into(), p["url"].clone());
            r.insert(
                "raw_content".into(),
                Value::String(formatted(
                    p["raw_content"].as_str().unwrap_or_default(),
                    format,
                )),
            );
            if favicon {
                r.insert("favicon".into(), p["favicon"].clone());
            }
            Value::Object(r)
        })
        .collect();
    let id = request_id(world);
    json(
        StatusCode::OK,
        json_value!({
            "base_url": root,
            "results": results,
            "response_time": round(1.2 + 0.35 * walked.pages.len() as f64, 2),
            "request_id": id,
        }),
    )
}

async fn map(State(world): State<SharedWorld>, headers: HeaderMap, body: Bytes) -> Response {
    let input = match begin(&headers, &body) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let root = match root_url(&input) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    let all = pages(world);
    let walked = match walk(&all, &root, &input) {
        Ok(w) => w,
        Err(r) => return r,
    };
    let id = request_id(world);
    json(
        StatusCode::OK,
        json_value!({
            "base_url": root,
            "results": walked.urls,
            "response_time": round(0.8 + 0.08 * walked.urls.len() as f64, 2),
            "request_id": id,
        }),
    )
}

// ──────────────────────────────── research ────────────────────────────────

/// The report for `input`: a matching canned report from the dataset, else a short
/// synthesis of the best-matching pages.
fn report_for(world: &WorldState, input: &str) -> (String, Vec<Value>) {
    let lower = input.to_lowercase();
    if let Some(reports) = world
        .service(NAME)
        .and_then(|s| s.store.collection("research_reports"))
    {
        for report in reports.values() {
            let terms: Vec<&str> = report["match"]
                .as_array()
                .map(|t| t.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            // Every term must appear; a term may list alternatives as `a|b`.
            let hit = |t: &&str| t.split('|').any(|alt| lower.contains(&alt.to_lowercase()));
            if !terms.is_empty() && terms.iter().all(hit) {
                let sources = report["sources"].as_array().cloned().unwrap_or_default();
                return (
                    report["content"].as_str().unwrap_or_default().to_owned(),
                    sources,
                );
            }
        }
    }
    let q = SearchQuery {
        phrases: Vec::new(),
        ..SearchQuery::parse(input)
    };
    let mut hits: Vec<(usize, usize, Value)> = pages(world)
        .into_iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let hay = format!(
                "{} {} {}",
                p["title"].as_str().unwrap_or_default(),
                p["content"].as_str().unwrap_or_default(),
                p["keywords"].as_str().unwrap_or_default()
            );
            q.score(&hay).map(|m| (m, i, p))
        })
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    if hits.is_empty() {
        return (
            format!(
                "# Research report\n\n**Question:** {input}\n\nNo reliable public sources addressing this question were found. \
                 The topic may be too narrow or too recent to be covered."
            ),
            Vec::new(),
        );
    }
    let mut content = format!("# Research report\n\n**Question:** {input}\n\n## Findings\n");
    let mut sources = Vec::new();
    for (n, (_, _, page)) in hits.iter().take(5).enumerate() {
        content.push_str(&format!(
            "\n{}. **{}**: {} [{}]\n",
            n + 1,
            page["title"].as_str().unwrap_or_default(),
            page["content"].as_str().unwrap_or_default(),
            n + 1
        ));
        sources.push(json_value!({ "title": page["title"], "url": page["url"] }));
    }
    content.push_str("\n## Sources\n");
    for (n, s) in sources.iter().enumerate() {
        content.push_str(&format!(
            "\n[{}] {}: {}",
            n + 1,
            s["title"].as_str().unwrap_or_default(),
            s["url"].as_str().unwrap_or_default()
        ));
    }
    (content, sources)
}

async fn research_create(
    State(world): State<SharedWorld>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let input = match begin(&headers, &body) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let Some(question) = str_arg(&input, "input") else {
        return detail(StatusCode::BAD_REQUEST, "Input is missing.");
    };
    let model = str_arg(&input, "model").unwrap_or("auto");
    if !["mini", "pro", "auto"].contains(&model) {
        return detail(
            StatusCode::BAD_REQUEST,
            "Invalid model. Must be 'mini', 'pro' or 'auto'.",
        );
    }
    let stream = bool_arg(&input, "stream");
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    let now = world.clock.now_rfc3339();
    let (content, sources) = report_for(world, question);
    // Research tasks get their own ID stream, so the N-th task's ID doesn't depend on
    // how many searches came before it.
    let id = world
        .service_mut(NAME)
        .ids
        .next_uuid("research")
        .to_string();
    let doc = json_value!({
        "request_id": id,
        "created_at": now,
        "completed_at": now,
        "status": "completed",
        "input": question,
        "model": model,
        "stream": stream,
        "content": content,
        "sources": sources,
    });
    world
        .service_mut(NAME)
        .store
        .collection_mut("research")
        .insert(id.clone(), doc);
    if stream {
        return sse(&id, &content, &sources);
    }
    json(
        StatusCode::CREATED,
        json_value!({
            "request_id": id,
            "created_at": now,
            "status": "pending",
            "input": question,
            "model": model,
            "response_time": 0.23,
        }),
    )
}

/// Streaming research: OpenAI-style chunk events, then `event: done`.
fn sse(id: &str, content: &str, sources: &[Value]) -> Response {
    let mut out = String::new();
    let chunks: Vec<String> = content.split_inclusive('\n').map(str::to_owned).collect();
    for chunk in chunks {
        let frame = json_value!({
            "id": id,
            "object": "chat.completion.chunk",
            "model": "tavily-research",
            "choices": [{ "index": 0, "delta": { "role": "assistant", "content": chunk } }],
        });
        out.push_str(&format!("data: {frame}\n\n"));
    }
    let frame = json_value!({
        "id": id,
        "object": "chat.completion.chunk",
        "model": "tavily-research",
        "choices": [{ "index": 0, "delta": { "role": "assistant", "sources": sources } }],
    });
    out.push_str(&format!("data: {frame}\n\n"));
    out.push_str("event: done\ndata: {}\n\n");
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/event-stream")],
        Body::from(out),
    )
        .into_response()
}

async fn research_get(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = begin(&headers, &Bytes::new()) {
        return r;
    }
    let world = world.lock().expect("world lock");
    let Some(doc) = world
        .service(NAME)
        .and_then(|s| s.store.collection("research"))
        .and_then(|c| c.get(&id))
    else {
        return detail(StatusCode::NOT_FOUND, "Research task not found");
    };
    let sources: Vec<Value> = doc["sources"]
        .as_array()
        .map(|l| {
            l.iter()
                .map(|s| {
                    json_value!({
                        "title": s["title"],
                        "url": s["url"],
                        "favicon": format!("https://{}/favicon.ico", host_of(s["url"].as_str().unwrap_or_default())),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    json(
        StatusCode::OK,
        json_value!({
            "request_id": doc["request_id"],
            "created_at": doc["created_at"],
            "completed_at": doc["completed_at"],
            "status": doc["status"],
            "content": doc["content"],
            "sources": sources,
            "response_time": 41.7,
        }),
    )
}
