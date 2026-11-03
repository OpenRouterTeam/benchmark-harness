//! Mock Exa API (`api.exa.ai`, real-tls), as called by `exa-mcp-server` 3.4.2 (bundled
//! exa-js, base URL fixed to `https://api.exa.ai`) with all 12 tools enabled:
//!
//! - `POST /search`: every search tool (web, advanced, deep, code, company, people, LinkedIn)
//! - `POST /contents`: `web_fetch_exa` / `crawling_exa`
//! - `POST /research/v1`, `GET /research/v1/{id}`, `GET /research/v1`: deep researcher
//! - `POST /agent/runs` (JSON, or SSE when `Accept: text/event-stream`), `GET /agent/runs`,
//!   `GET /agent/runs/{id}`, `GET /agent/runs/{id}/events`, `POST /agent/runs/{id}/cancel`,
//!   `DELETE /agent/runs/{id}`: `agent_run`
//!
//! The world is "the web as Exa indexed it at `now`". Dataset collections:
//!
//! | Collection | Key | Content |
//! |---|---|---|
//! | `documents` | URL | `url`, `title`, `author`, `publishedDate` (ISO date), `category` (`company`, `people`, `news`, `github`, `pdf`, `publication`, `personal site`, `financial report`), `text`, `summary`, `image`, `favicon`, `keywords`, `subpages` (URLs) |
//! | `answers` | any | deep-search answers: `match` (terms that must all appear in the query, each optionally `a\|b`), `content`, `structured`, `citations` (`[{url, title}]`) |
//! | `research_reports` | any | research output: `match`, `content`, `parsed`, `citations` |
//! | `agent_outputs` | any | agent run output: `match` (against the query plus input rows), `output` |
//! | `research` | research id | research tasks, created by `POST /research/v1` |
//! | `agent_runs` | run id | agent runs, created by `POST /agent/runs` |
//!
//! Relevance is the number of significant query terms a document contains; queries that
//! match nothing return an empty `results` list.

// Handlers return early with a ready `Response` as the error; boxing it buys nothing here.
#![allow(clippy::result_large_err)]

pub mod text;

use std::collections::BTreeSet;

use axum::{
    Router,
    body::{Body, Bytes},
    extract::{Path, RawQuery, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use mock_core::{
    http::json,
    service::MockService,
    world::{SharedWorld, WorldState},
};
use serde_json::{Map, Value, json as json_value};
use text::{SearchQuery, host_matches, host_of, norm_url, tokens};

pub const NAME: &str = "exa";
pub const PORT: u16 = 8104;

pub struct Exa;

impl MockService for Exa {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port(&self) -> u16 {
        PORT
    }

    fn hosts(&self) -> &'static [&'static str] {
        &["api.exa.ai"]
    }

    fn router(&self) -> Router<SharedWorld> {
        Router::new()
            .route("/search", post(search))
            .route("/contents", post(contents))
            .route("/research/v1", post(research_create).get(research_list))
            .route("/research/v1/{id}", get(research_get))
            .route("/agent/runs", post(agent_create).get(agent_list))
            .route("/agent/runs/{id}", get(agent_get).delete(agent_delete))
            .route("/agent/runs/{id}/events", get(agent_events))
            .route("/agent/runs/{id}/cancel", post(agent_cancel))
            .fallback(not_found)
    }
}

// ───────────────────────────── errors & helpers ─────────────────────────────

fn error(world: &mut WorldState, status: StatusCode, tag: &str, message: &str) -> Response {
    let request_id = request_id(world);
    json(
        status,
        json_value!({ "requestId": request_id, "error": message, "tag": tag }),
    )
}

async fn not_found(State(world): State<SharedWorld>) -> Response {
    let mut w = world.lock().expect("world lock");
    error(&mut w, StatusCode::NOT_FOUND, "NOT_FOUND", "Not found")
}

fn request_id(world: &mut WorldState) -> String {
    let id = world.service_mut(NAME).ids.next_uuid("request");
    id.simple().to_string()
}

/// Checks the API key (`x-api-key` or `Authorization: Bearer`) and parses a JSON body.
fn begin(
    world: &mut WorldState,
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<Map<String, Value>, Response> {
    let key = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .or_else(|| {
            headers
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
        })
        .unwrap_or_default()
        .trim();
    if key.is_empty() {
        return Err(error(
            world,
            StatusCode::UNAUTHORIZED,
            "INVALID_API_KEY",
            "Invalid API key. Provide a valid key using 'Authorization: Bearer <key>' or 'x-api-key: <key>'. Create a key at https://dashboard.exa.ai/api-keys",
        ));
    }
    if body.is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(m)) => Ok(m),
        _ => Err(error(
            world,
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST_BODY",
            "Invalid request body: expected a JSON object",
        )),
    }
}

macro_rules! tryr {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(r) => return r,
        }
    };
}

fn collection(world: &WorldState, name: &str) -> Vec<Value> {
    world
        .service(NAME)
        .and_then(|s| s.store.collection(name))
        .map(|c| c.values().cloned().collect())
        .unwrap_or_default()
}

fn str_list(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Some(Value::String(s)) if !s.is_empty() => vec![s.clone()],
        _ => Vec::new(),
    }
}

fn round(value: f64, places: i32) -> f64 {
    let f = 10f64.powi(places);
    (value * f).round() / f
}

/// `YYYY-MM-DD` prefix as a sortable string, or `None` if malformed.
fn date_prefix(s: &str) -> Option<String> {
    let d = s.get(..10)?;
    let ok = d.len() == 10
        && d.as_bytes()[4] == b'-'
        && d.as_bytes()[7] == b'-'
        && d.chars()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
    ok.then(|| d.to_owned())
}

fn iso_date(doc: &Value) -> Value {
    doc["publishedDate"]
        .as_str()
        .and_then(date_prefix)
        .map_or(Value::Null, |d| Value::String(format!("{d}T00:00:00.000Z")))
}

/// Every `match` term (each optionally `a|b`) appears in `text`.
fn matches_terms(entry: &Value, text: &str) -> bool {
    let lower = text.to_lowercase();
    let terms: Vec<&str> = entry["match"]
        .as_array()
        .map(|t| t.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    !terms.is_empty()
        && terms
            .iter()
            .all(|t| t.split('|').any(|alt| lower.contains(&alt.to_lowercase())))
}

fn canned(world: &WorldState, name: &str, text: &str) -> Option<Value> {
    collection(world, name)
        .into_iter()
        .find(|e| matches_terms(e, text))
}

// ───────────────────────────── ranking & contents ─────────────────────────────

fn haystack(doc: &Value) -> String {
    let mut s = String::new();
    for key in ["title", "author", "text", "summary", "keywords", "url"] {
        if let Some(v) = doc[key].as_str() {
            s.push_str(v);
            s.push(' ');
        }
    }
    s
}

struct Filters {
    category: Option<String>,
    include_domains: Vec<String>,
    exclude_domains: Vec<String>,
    start_published: Option<String>,
    end_published: Option<String>,
    start_crawl: Option<String>,
    end_crawl: Option<String>,
    include_text: Vec<String>,
    exclude_text: Vec<String>,
}

impl Filters {
    fn parse(input: &Map<String, Value>) -> Self {
        let date = |k: &str| input.get(k).and_then(Value::as_str).and_then(date_prefix);
        Self {
            category: input
                .get("category")
                .and_then(Value::as_str)
                .map(str::to_lowercase),
            include_domains: str_list(input.get("includeDomains")),
            exclude_domains: str_list(input.get("excludeDomains")),
            start_published: date("startPublishedDate"),
            end_published: date("endPublishedDate"),
            start_crawl: date("startCrawlDate"),
            end_crawl: date("endCrawlDate"),
            include_text: str_list(input.get("includeText")),
            exclude_text: str_list(input.get("excludeText")),
        }
    }

    fn accepts(&self, doc: &Value) -> bool {
        let host = host_of(doc["url"].as_str().unwrap_or_default());
        if let Some(c) = &self.category {
            let doc_cat = doc["category"].as_str().unwrap_or_default().to_lowercase();
            let same = doc_cat == *c
                || (c == "publication" && doc_cat == "research paper")
                || (c == "research paper" && doc_cat == "publication");
            if !same {
                return false;
            }
        }
        if !self.include_domains.is_empty()
            && !self.include_domains.iter().any(|d| host_matches(&host, d))
        {
            return false;
        }
        if self.exclude_domains.iter().any(|d| host_matches(&host, d)) {
            return false;
        }
        let published = doc["publishedDate"].as_str().and_then(date_prefix);
        let crawled = doc["crawlDate"]
            .as_str()
            .and_then(date_prefix)
            .or_else(|| published.clone());
        let in_range = |d: &Option<String>, from: &Option<String>, to: &Option<String>| {
            if from.is_none() && to.is_none() {
                return true;
            }
            d.as_ref().is_some_and(|d| {
                from.as_ref().is_none_or(|f| d >= f) && to.as_ref().is_none_or(|t| d <= t)
            })
        };
        if !in_range(&published, &self.start_published, &self.end_published)
            || !in_range(&crawled, &self.start_crawl, &self.end_crawl)
        {
            return false;
        }
        let text = haystack(doc).to_lowercase();
        self.include_text
            .iter()
            .all(|t| text.contains(&t.to_lowercase()))
            && !self
                .exclude_text
                .iter()
                .any(|t| text.contains(&t.to_lowercase()))
    }
}

/// Documents for `queries` (the first is the main query), best first, de-duplicated.
fn rank(world: &WorldState, queries: &[String], filters: &Filters) -> Vec<Value> {
    let docs = collection(world, "documents");
    let mut best: Vec<(usize, usize, Value)> = Vec::new();
    for (order, doc) in docs.into_iter().enumerate() {
        if !filters.accepts(&doc) {
            continue;
        }
        let hay = haystack(&doc);
        let score = queries
            .iter()
            .filter_map(|q| {
                let mut sq = SearchQuery::parse(q);
                sq.phrases.clear();
                let host = host_of(doc["url"].as_str().unwrap_or_default());
                if sq.sites.iter().any(|s| !host_matches(&host, s)) {
                    return None;
                }
                sq.score(&hay)
            })
            .max();
        if let Some(s) = score {
            best.push((s, order, doc));
        }
    }
    best.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    best.into_iter().map(|(_, _, d)| d).collect()
}

/// Sentences of `text`, split on `. `, `? `, `! ` and newlines.
fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut cur = String::new();
        let chars: Vec<char> = line.chars().collect();
        for (i, c) in chars.iter().enumerate() {
            cur.push(*c);
            if matches!(c, '.' | '?' | '!') && chars.get(i + 1).is_none_or(|n| *n == ' ') {
                let s = cur.trim().to_owned();
                if !s.is_empty() {
                    out.push(s);
                }
                cur.clear();
            }
        }
        let s = cur.trim().to_owned();
        if !s.is_empty() {
            out.push(s);
        }
    }
    out
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        text.chars().take(max).collect()
    }
}

/// The most relevant sentences for `query` (document order), within `max_chars` total.
fn highlights(doc: &Value, query: &str, max_chars: usize) -> (Vec<String>, Vec<f64>) {
    let q = SearchQuery::parse(query);
    let text = doc["text"].as_str().unwrap_or_default();
    let sents = sentences(text);
    let mut scored: Vec<(usize, usize)> = sents
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let toks: BTreeSet<String> = tokens(s).into_iter().collect();
            (i, q.terms.iter().filter(|t| toks.contains(*t)).count())
        })
        .collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut chosen: Vec<(usize, usize)> = Vec::new();
    let mut used = 0;
    for (i, n) in scored {
        if chosen.len() >= 3 || (n == 0 && !chosen.is_empty()) {
            break;
        }
        let len = sents[i].chars().count();
        if used + len > max_chars && !chosen.is_empty() {
            continue;
        }
        used += len;
        chosen.push((i, n));
    }
    chosen.sort_by_key(|(i, _)| *i);
    let terms = q.terms.len().max(1) as f64;
    (
        chosen
            .iter()
            .map(|(i, _)| truncate(&sents[*i], max_chars))
            .collect(),
        chosen
            .iter()
            .map(|(_, n)| round(0.25 + 0.7 * (*n as f64 / terms), 4))
            .collect(),
    )
}

fn summary_of(doc: &Value) -> String {
    doc["summary"].as_str().map_or_else(
        || {
            sentences(doc["text"].as_str().unwrap_or_default())
                .into_iter()
                .take(2)
                .collect::<Vec<_>>()
                .join(" ")
        },
        str::to_owned,
    )
}

/// What `contents` asked for.
#[derive(Default)]
struct Wanted {
    text: Option<usize>,
    highlights: Option<(usize, Option<String>)>,
    summary: bool,
    subpages: usize,
    subpage_target: Vec<String>,
}

impl Wanted {
    fn parse(contents: &Map<String, Value>) -> Self {
        let max_of = |v: &Value, default: usize| {
            v.get("maxCharacters")
                .and_then(Value::as_u64)
                .map_or(default, |m| m as usize)
        };
        let text = match contents.get("text") {
            Some(Value::Bool(true)) => Some(10_000),
            Some(v @ Value::Object(_)) => Some(max_of(v, 10_000)),
            _ => None,
        };
        let highlights = match contents.get("highlights") {
            Some(Value::Bool(true)) => Some((2_000, None)),
            Some(v @ Value::Object(_)) => {
                let sentences = v
                    .get("numSentences")
                    .and_then(Value::as_u64)
                    .map(|n| n as usize * 1_333);
                Some((
                    v.get("maxCharacters")
                        .and_then(Value::as_u64)
                        .map(|m| m as usize)
                        .or(sentences)
                        .unwrap_or(2_000),
                    v.get("query").and_then(Value::as_str).map(str::to_owned),
                ))
            }
            _ => None,
        };
        Self {
            text,
            highlights,
            summary: matches!(
                contents.get("summary"),
                Some(Value::Bool(true) | Value::Object(_))
            ),
            subpages: contents
                .get("subpages")
                .and_then(Value::as_u64)
                .map_or(0, |n| n.min(10) as usize),
            subpage_target: str_list(contents.get("subpageTarget")),
        }
    }

    fn cost(&self, n: usize) -> f64 {
        let per = [self.text.is_some(), self.highlights.is_some(), self.summary]
            .iter()
            .filter(|b| **b)
            .count();
        n as f64 * per as f64 * 0.001
    }
}

fn result_of(world: &WorldState, doc: &Value, query: &str, wanted: &Wanted, depth: u8) -> Value {
    let mut r = Map::new();
    r.insert("id".into(), doc["url"].clone());
    r.insert(
        "title".into(),
        doc.get("title").cloned().unwrap_or(Value::Null),
    );
    r.insert("url".into(), doc["url"].clone());
    let date = iso_date(doc);
    if !date.is_null() {
        r.insert("publishedDate".into(), date);
    }
    if let Some(a) = doc.get("author") {
        r.insert("author".into(), a.clone());
    }
    if let Some(max) = wanted.text {
        r.insert(
            "text".into(),
            Value::String(truncate(doc["text"].as_str().unwrap_or_default(), max)),
        );
    }
    if let Some((max, hq)) = &wanted.highlights {
        let (h, scores) = highlights(doc, hq.as_deref().unwrap_or(query), *max);
        r.insert("highlights".into(), json_value!(h));
        r.insert("highlightScores".into(), json_value!(scores));
    }
    if wanted.summary {
        r.insert("summary".into(), Value::String(summary_of(doc)));
    }
    for key in ["image", "favicon"] {
        if let Some(v) = doc.get(key) {
            r.insert(key.into(), v.clone());
        }
    }
    if wanted.subpages > 0 && depth == 0 {
        let docs = collection(world, "documents");
        let subs: Vec<Value> = str_list(doc.get("subpages"))
            .iter()
            .filter_map(|u| {
                docs.iter()
                    .find(|d| norm_url(d["url"].as_str().unwrap_or_default()) == norm_url(u))
            })
            .filter(|d| {
                wanted.subpage_target.is_empty() || {
                    let hay = haystack(d).to_lowercase();
                    wanted
                        .subpage_target
                        .iter()
                        .any(|t| hay.contains(&t.to_lowercase()))
                }
            })
            .take(wanted.subpages)
            .map(|d| result_of(world, d, query, wanted, 1))
            .collect();
        r.insert("subpages".into(), Value::Array(subs));
    }
    Value::Object(r)
}

// ───────────────────────────────── /search ─────────────────────────────────

async fn search(State(world): State<SharedWorld>, headers: HeaderMap, body: Bytes) -> Response {
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    let input = tryr!(begin(w, &headers, &body));
    let Some(query) = input
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(str::to_owned)
    else {
        return error(
            w,
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST_BODY",
            "Invalid request body: query is required",
        );
    };
    let kind = input
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("auto")
        .to_owned();
    let resolved = match kind.as_str() {
        "auto" | "neural" => "neural",
        "fast" => "fast",
        "instant" => "instant",
        "keyword" => "keyword",
        "deep" => "deep",
        "deep-reasoning" => "deep-reasoning",
        other => {
            let msg = format!(
                "Invalid search type '{other}'. Expected one of: auto, neural, fast, instant, keyword, deep, deep-reasoning"
            );
            return error(w, StatusCode::BAD_REQUEST, "INVALID_REQUEST_BODY", &msg);
        }
    };
    let num = input
        .get("numResults")
        .and_then(Value::as_u64)
        .unwrap_or(10);
    if !(1..=100).contains(&num) {
        return error(
            w,
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST_BODY",
            "Invalid request body: numResults must be between 1 and 100",
        );
    }
    let mut queries = vec![query.clone()];
    queries.extend(str_list(input.get("additionalQueries")));
    let filters = Filters::parse(&input);
    let contents = input
        .get("contents")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let wanted = Wanted::parse(&contents);
    let hits: Vec<Value> = rank(w, &queries, &filters)
        .into_iter()
        .take(num as usize)
        .collect();
    let results: Vec<Value> = hits
        .iter()
        .map(|d| result_of(w, d, &query, &wanted, 0))
        .collect();

    let deep = resolved.starts_with("deep");
    let search_cost = if deep { 0.015 } else { 0.005 };
    let mut body = Map::new();
    body.insert("requestId".into(), Value::String(request_id(w)));
    body.insert("resolvedSearchType".into(), json_value!(resolved));
    if let Some(ctx) = contents.get("context") {
        let max = ctx
            .get("maxCharacters")
            .and_then(Value::as_u64)
            .map_or(10_000, |m| m as usize);
        let joined = hits
            .iter()
            .map(|d| {
                format!(
                    "Title: {}\nURL: {}\n{}",
                    d["title"].as_str().unwrap_or_default(),
                    d["url"].as_str().unwrap_or_default(),
                    d["text"].as_str().unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        body.insert("context".into(), Value::String(truncate(&joined, max)));
    }
    if deep {
        body.insert(
            "output".into(),
            deep_output(w, &query, &hits, input.get("outputSchema")),
        );
    }
    body.insert("results".into(), Value::Array(results));
    body.insert(
        "searchTime".into(),
        json_value!(match resolved {
            "instant" => 187.3,
            "fast" => 412.7,
            "deep" => 6840.2,
            "deep-reasoning" => 21_377.9,
            _ => 1203.5,
        }),
    );
    let contents_cost = wanted.cost(hits.len());
    body.insert(
        "costDollars".into(),
        json_value!({
            "total": round(search_cost + contents_cost, 4),
            "search": { resolved: search_cost },
            "contents": { "total": round(contents_cost, 4) },
        }),
    );
    json(StatusCode::OK, Value::Object(body))
}

/// Deep-search answer: a canned answer from the dataset, else a synthesis of the top hits.
fn deep_output(w: &WorldState, query: &str, hits: &[Value], schema: Option<&Value>) -> Value {
    let structured = schema.is_some_and(|s| s["type"] == "object");
    if let Some(answer) = canned(w, "answers", query) {
        let content = if structured {
            answer
                .get("structured")
                .cloned()
                .unwrap_or_else(|| json_value!({ "answer": answer["content"] }))
        } else {
            answer["content"].clone()
        };
        let citations = answer["citations"].as_array().cloned().unwrap_or_default();
        return json_value!({
            "content": content,
            "grounding": [{ "field": "content", "citations": citations, "confidence": "high" }],
        });
    }
    let top: Vec<&Value> = hits.iter().take(3).collect();
    let text = if top.is_empty() {
        "No sources were found that answer this question.".to_owned()
    } else {
        top.iter()
            .map(|d| summary_of(d))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let citations: Vec<Value> = top
        .iter()
        .map(|d| json_value!({ "url": d["url"], "title": d["title"] }))
        .collect();
    let content = if structured {
        json_value!({ "answer": text })
    } else {
        Value::String(text)
    };
    json_value!({
        "content": content,
        "grounding": [{ "field": "content", "citations": citations, "confidence": if top.is_empty() { "low" } else { "medium" } }],
    })
}

// ──────────────────────────────── /contents ────────────────────────────────

async fn contents(State(world): State<SharedWorld>, headers: HeaderMap, body: Bytes) -> Response {
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    let input = tryr!(begin(w, &headers, &body));
    let mut urls = str_list(input.get("urls"));
    urls.extend(str_list(input.get("ids")));
    if urls.is_empty() {
        return error(
            w,
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST_BODY",
            "Invalid request body: either urls or ids must be provided",
        );
    }
    if urls.len() > 100 {
        return error(
            w,
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST_BODY",
            "Invalid request body: at most 100 urls are allowed",
        );
    }
    let mut wanted = Wanted::parse(&input);
    if wanted.text.is_none() && wanted.highlights.is_none() && !wanted.summary {
        wanted.text = Some(10_000);
    }
    let docs = collection(w, "documents");
    let mut results = Vec::new();
    let mut statuses = Vec::new();
    for url in &urls {
        match docs
            .iter()
            .find(|d| norm_url(d["url"].as_str().unwrap_or_default()) == norm_url(url))
        {
            Some(doc) => {
                let title = doc["title"].as_str().unwrap_or_default().to_owned();
                let mut r = result_of(w, doc, &title, &wanted, 0);
                r["id"] = Value::String(url.clone());
                results.push(r);
                statuses.push(json_value!({ "id": url, "status": "success", "source": "cached" }));
            }
            None => statuses.push(json_value!({
                "id": url,
                "status": "error",
                "error": { "tag": "CRAWL_NOT_FOUND", "httpStatusCode": 404 },
                "source": "crawl",
            })),
        }
    }
    let cost = wanted.cost(results.len());
    let id = request_id(w);
    json(
        StatusCode::OK,
        json_value!({
            "requestId": id,
            "results": results,
            "statuses": statuses,
            "costDollars": { "total": round(cost, 4), "contents": { "total": round(cost, 4) } },
            "searchTime": 318.4,
        }),
    )
}

// ──────────────────────────────── research ────────────────────────────────

const RESEARCH_MODELS: [&str; 3] = ["exa-research-fast", "exa-research", "exa-research-pro"];

fn millis(w: &WorldState) -> i64 {
    w.clock.now_unix() * 1000
}

/// The report for `instructions`: a canned report, else a synthesis of matching documents.
fn report_for(w: &WorldState, instructions: &str) -> (Value, Option<Value>, Vec<Value>) {
    if let Some(r) = canned(w, "research_reports", instructions) {
        return (
            r["content"].clone(),
            r.get("parsed").cloned(),
            r["citations"].as_array().cloned().unwrap_or_default(),
        );
    }
    let hits = rank(w, &[instructions.to_owned()], &Filters::parse(&Map::new()));
    if hits.is_empty() {
        return (
            json_value!(format!(
                "# Research report\n\n{instructions}\n\nNo reliable sources addressing these instructions were found."
            )),
            None,
            Vec::new(),
        );
    }
    let mut content = format!("# Research report\n\n{instructions}\n\n## Findings\n");
    let mut citations = Vec::new();
    for (i, d) in hits.iter().take(5).enumerate() {
        content.push_str(&format!(
            "\n{}. **{}**: {} [{}]\n",
            i + 1,
            d["title"].as_str().unwrap_or_default(),
            summary_of(d),
            i + 1
        ));
        citations.push(json_value!({ "id": d["url"], "url": d["url"], "title": d["title"] }));
    }
    (Value::String(content), None, citations)
}

async fn research_create(
    State(world): State<SharedWorld>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    let input = tryr!(begin(w, &headers, &body));
    let Some(instructions) = input
        .get("instructions")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
    else {
        return error(
            w,
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST_BODY",
            "Invalid request body: instructions is required",
        );
    };
    if instructions.chars().count() > 4096 {
        return error(
            w,
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST_BODY",
            "Invalid request body: instructions must be at most 4096 characters",
        );
    }
    let model = input
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("exa-research")
        .to_owned();
    if !RESEARCH_MODELS.contains(&model.as_str()) {
        let msg = format!(
            "Invalid request body: model must be one of {}",
            RESEARCH_MODELS.join(", ")
        );
        return error(w, StatusCode::BAD_REQUEST, "INVALID_REQUEST_BODY", &msg);
    }
    let schema = input.get("outputSchema").cloned();
    let (content, parsed, citations) = report_for(w, &instructions);
    let created = millis(w);
    let id = w
        .service_mut(NAME)
        .ids
        .next_prefixed("research", "r_01k", 21);
    let (searches, pages) = (6 + citations.len() as u64, 10 + 2 * citations.len() as u64);
    let price = match model.as_str() {
        "exa-research-pro" => 0.31,
        "exa-research" => 0.17,
        _ => 0.08,
    };
    let mut output = json_value!({ "content": content });
    if schema.is_some() {
        output["parsed"] = parsed.unwrap_or_else(|| json_value!({}));
    }
    let doc = json_value!({
        "researchId": id,
        "createdAt": created,
        "finishedAt": created + 41_000,
        "model": model,
        "instructions": instructions,
        "outputSchema": schema,
        "status": "completed",
        "output": output,
        "citations": citations,
        "costDollars": { "total": price, "numSearches": searches, "numPages": pages, "reasoningTokens": 18_432 },
    });
    w.service_mut(NAME)
        .store
        .collection_mut("research")
        .insert(id.clone(), doc);
    let mut resp = json_value!({
        "researchId": id,
        "createdAt": created,
        "model": model,
        "instructions": instructions,
        "status": "pending",
    });
    if let Some(s) = schema {
        resp["outputSchema"] = s;
    }
    json(StatusCode::CREATED, resp)
}

async fn research_get(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    tryr!(begin(w, &headers, &Bytes::new()));
    let doc = w
        .service(NAME)
        .and_then(|s| s.store.collection("research"))
        .and_then(|c| c.get(&id))
        .cloned();
    match doc {
        Some(mut d) => {
            if d["outputSchema"].is_null()
                && let Some(o) = d.as_object_mut()
            {
                o.shift_remove("outputSchema");
            }
            json(StatusCode::OK, d)
        }
        None => error(
            w,
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            &format!("Research task {id} not found"),
        ),
    }
}

fn query_param(raw: Option<&str>, key: &str) -> Option<String> {
    raw?.split('&')
        .filter_map(|p| p.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.to_owned())
}

/// Cursor pagination over a collection, newest first (`data`, `hasMore`, `nextCursor`).
fn list_page(w: &WorldState, name: &str, raw: Option<&str>) -> Value {
    let mut docs = collection(w, name);
    docs.reverse();
    let limit = query_param(raw, "limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(10)
        .clamp(1, 50);
    let start = query_param(raw, "cursor")
        .and_then(|c| c.parse::<usize>().ok())
        .unwrap_or(0);
    let page: Vec<Value> = docs.iter().skip(start).take(limit).cloned().collect();
    let more = docs.len() > start + limit;
    json_value!({
        "data": page,
        "hasMore": more,
        "nextCursor": if more { Value::String((start + limit).to_string()) } else { Value::Null },
    })
}

async fn research_list(
    State(world): State<SharedWorld>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    tryr!(begin(w, &headers, &Bytes::new()));
    json(StatusCode::OK, list_page(w, "research", raw.as_deref()))
}

// ──────────────────────────────── agent runs ────────────────────────────────

const EFFORTS: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "auto"];

fn agent_output(w: &WorldState, query: &str, input: Option<&Value>) -> Value {
    let probe = format!(
        "{query} {}",
        input.map(Value::to_string).unwrap_or_default()
    );
    if let Some(o) = canned(w, "agent_outputs", &probe) {
        return o["output"].clone();
    }
    let hits = rank(w, &[query.to_owned()], &Filters::parse(&Map::new()));
    let text = if hits.is_empty() {
        "I could not find sources that satisfy this request.".to_owned()
    } else {
        hits.iter()
            .take(3)
            .map(|d| {
                format!(
                    "- {} ({}): {}",
                    d["title"].as_str().unwrap_or_default(),
                    d["url"].as_str().unwrap_or_default(),
                    summary_of(d)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    json_value!({ "content": text })
}

fn agent_events_of(run: &Value) -> Vec<(String, Value)> {
    let mut queued = run.clone();
    queued["status"] = json_value!("queued");
    queued["output"] = Value::Null;
    let mut running = queued.clone();
    running["status"] = json_value!("running");
    vec![
        ("agent_run.created".to_owned(), queued),
        ("agent_run.started".to_owned(), running),
        (
            format!(
                "agent_run.{}",
                run["status"].as_str().unwrap_or("completed")
            ),
            run.clone(),
        ),
    ]
}

async fn agent_create(
    State(world): State<SharedWorld>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    let input = tryr!(begin(w, &headers, &body));
    let Some(query) = input
        .get("query")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
    else {
        return error(
            w,
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST_BODY",
            "Invalid request body: query is required",
        );
    };
    let effort = input
        .get("effort")
        .and_then(Value::as_str)
        .unwrap_or("low")
        .to_owned();
    if !EFFORTS.contains(&effort.as_str()) {
        return error(
            w,
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST_BODY",
            "Invalid request body: effort must be one of minimal, low, medium, high, xhigh, auto",
        );
    }
    if let Some(prev) = input.get("previousRunId").and_then(Value::as_str) {
        let known = w
            .service(NAME)
            .and_then(|s| s.store.collection("agent_runs"))
            .is_some_and(|c| c.contains(prev));
        if !known {
            return error(
                w,
                StatusCode::NOT_FOUND,
                "NOT_FOUND",
                &format!("Agent run {prev} not found"),
            );
        }
    }
    let now = w.clock.now_rfc3339();
    let output = agent_output(w, &query, input.get("input"));
    let id = w
        .service_mut(NAME)
        .ids
        .next_prefixed("agent_run", "agent_run_", 24);
    let mut run = Map::new();
    run.insert("id".into(), json_value!(id));
    run.insert("object".into(), json_value!("agent_run"));
    run.insert("status".into(), json_value!("completed"));
    run.insert("query".into(), json_value!(query));
    run.insert("effort".into(), json_value!(effort));
    for key in [
        "systemPrompt",
        "outputSchema",
        "input",
        "dataSources",
        "previousRunId",
    ] {
        if let Some(v) = input.get(key) {
            run.insert(key.into(), v.clone());
        }
    }
    run.insert("output".into(), output);
    run.insert(
        "usage".into(),
        json_value!({ "searches": 4, "pagesRead": 9, "inputTokens": 48_210, "outputTokens": 2_340 }),
    );
    run.insert(
        "costDollars".into(),
        json_value!({ "total": match effort.as_str() { "minimal" => 0.02, "low" => 0.05, "medium" => 0.12, "high" => 0.3, "xhigh" => 0.6, _ => 0.08 } }),
    );
    run.insert("createdAt".into(), json_value!(now));
    run.insert("completedAt".into(), json_value!(now));
    let run = Value::Object(run);
    w.service_mut(NAME)
        .store
        .collection_mut("agent_runs")
        .insert(id.clone(), run.clone());

    let wants_stream = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.contains("text/event-stream"));
    if wants_stream {
        let mut out = String::new();
        for (n, (event, data)) in agent_events_of(&run).into_iter().enumerate() {
            out.push_str(&format!("id: {}\nevent: {event}\ndata: {data}\n\n", n + 1));
        }
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/event-stream")],
            Body::from(out),
        )
            .into_response();
    }
    let mut queued = run;
    queued["status"] = json_value!("queued");
    queued["output"] = Value::Null;
    json(StatusCode::CREATED, queued)
}

fn find_run(w: &WorldState, id: &str) -> Option<Value> {
    w.service(NAME)
        .and_then(|s| s.store.collection("agent_runs"))
        .and_then(|c| c.get(id))
        .cloned()
}

fn run_not_found(w: &mut WorldState, id: &str) -> Response {
    error(
        w,
        StatusCode::NOT_FOUND,
        "NOT_FOUND",
        &format!("Agent run {id} not found"),
    )
}

async fn agent_get(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    tryr!(begin(w, &headers, &Bytes::new()));
    match find_run(w, &id) {
        Some(run) => json(StatusCode::OK, run),
        None => run_not_found(w, &id),
    }
}

async fn agent_list(
    State(world): State<SharedWorld>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    tryr!(begin(w, &headers, &Bytes::new()));
    json(StatusCode::OK, list_page(w, "agent_runs", raw.as_deref()))
}

async fn agent_events(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    tryr!(begin(w, &headers, &Bytes::new()));
    let Some(run) = find_run(w, &id) else {
        return run_not_found(w, &id);
    };
    let events: Vec<Value> = agent_events_of(&run)
        .into_iter()
        .enumerate()
        .map(|(n, (event, data))| json_value!({ "id": (n + 1).to_string(), "event": event, "data": data }))
        .collect();
    json(
        StatusCode::OK,
        json_value!({ "data": events, "hasMore": false, "nextCursor": null }),
    )
}

async fn agent_cancel(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    tryr!(begin(w, &headers, &Bytes::new()));
    let Some(run) = find_run(w, &id) else {
        return run_not_found(w, &id);
    };
    let status = run["status"].as_str().unwrap_or_default().to_owned();
    if ["completed", "failed", "cancelled"].contains(&status.as_str()) {
        return error(
            w,
            StatusCode::CONFLICT,
            "INVALID_STATE",
            &format!("Agent run {id} is already {status} and cannot be cancelled"),
        );
    }
    json(StatusCode::OK, run)
}

async fn agent_delete(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    tryr!(begin(w, &headers, &Bytes::new()));
    let removed = w
        .service_mut(NAME)
        .store
        .collection_mut("agent_runs")
        .remove(&id);
    match removed {
        Some(_) => json(
            StatusCode::OK,
            json_value!({ "id": id, "object": "agent_run", "deleted": true }),
        ),
        None => run_not_found(w, &id),
    }
}
