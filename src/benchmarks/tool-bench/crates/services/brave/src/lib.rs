//! Mock Brave Search API (`api.search.brave.com`, real-tls), as called by
//! `brave-search-mcp-server` 2.1.4. Every endpoint is a `GET` under `/res/v1`:
//! `web/search`, `news/search`, `videos/search`, `images/search`, `summarizer/search`,
//! `llm/context`, `local/pois`, `local/descriptions` and `local/place_search`.
//!
//! The world is "the web as Brave indexed it at `now`". Dataset collections (all keyed
//! by URL unless noted):
//!
//! | Collection | Content |
//! |---|---|
//! | `web` | `url`, `title`, `description`, `page_age` (ISO date), `extra_snippets`, `content` (markdown, used by LLM context), `faq` (`[{question, answer}]`), `keywords` |
//! | `news` | `url`, `title`, `description`, `page_age`, `source`, `breaking`, `thumbnail`, `extra_snippets`, `keywords` |
//! | `videos` | `url`, `title`, `description`, `page_age`, `duration`, `views`, `creator`, `publisher`, `tags`, `thumbnail`, `keywords` |
//! | `images` | `url` (page), `title`, `source`, `image_url`, `width`, `height`, `confidence`, `page_fetched`, `keywords` |
//! | `pois` | keyed by POI id: `id`, `title`, `url`, `categories`, `postal_address`, `coordinates` (`[lat, lon]`), `contact`, `opening_hours`, `rating`, `price_range`, `timezone`, `description` (served by `local/descriptions`), `keywords` |
//! | `summaries` | any key: `match` (terms that must all appear in the query, each optionally `a|b`), `title`, `text`, `references` (URLs) |
//!
//! Relevance is the number of significant query terms a document contains; queries that
//! match nothing return empty result lists, as Brave does for obscure queries.

// Handlers return early with a ready `Response` as the error; boxing it buys nothing here.
#![allow(clippy::result_large_err)]

pub mod text;

use axum::{
    Router,
    extract::{RawQuery, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::Response,
    routing::get,
};
use mock_core::{clock::format_unix, http::json, service::MockService, world::SharedWorld};
use serde_json::{Map, Value, json as json_value};
use text::{SearchQuery, host_matches, host_of, path_of, tokens};

pub const NAME: &str = "brave";
pub const PORT: u16 = 8103;

pub struct Brave;

impl MockService for Brave {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port(&self) -> u16 {
        PORT
    }

    fn hosts(&self) -> &'static [&'static str] {
        &["api.search.brave.com"]
    }

    fn router(&self) -> Router<SharedWorld> {
        Router::new()
            .route("/res/v1/web/search", get(web_search))
            .route("/res/v1/news/search", get(news_search))
            .route("/res/v1/videos/search", get(video_search))
            .route("/res/v1/images/search", get(image_search))
            .route("/res/v1/summarizer/search", get(summarizer))
            .route("/res/v1/llm/context", get(llm_context))
            .route("/res/v1/local/pois", get(local_pois))
            .route("/res/v1/local/descriptions", get(local_descriptions))
            .route("/res/v1/local/place_search", get(place_search))
            .fallback(not_found)
    }
}

// ───────────────────────────── request plumbing ─────────────────────────────

/// Decoded query string, keeping repeated keys (`ids=a&ids=b`).
struct Params(Vec<(String, String)>);

impl Params {
    fn parse(raw: Option<&str>) -> Self {
        Self(
            raw.unwrap_or_default()
                .split('&')
                .filter(|p| !p.is_empty())
                .map(|p| {
                    let (k, v) = p.split_once('=').unwrap_or((p, ""));
                    (decode(k), decode(v))
                })
                .collect(),
        )
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn all(&self, key: &str) -> Vec<&str> {
        self.0
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    fn flag(&self, key: &str) -> bool {
        matches!(self.get(key), Some("true" | "1"))
    }

    fn int(&self, key: &str) -> Result<Option<i64>, Response> {
        match self.get(key) {
            None | Some("") => Ok(None),
            Some(v) => v
                .parse::<i64>()
                .map(Some)
                .map_err(|_| invalid(key, v, "Input should be a valid integer")),
        }
    }

    fn float(&self, key: &str) -> Result<Option<f64>, Response> {
        match self.get(key) {
            None | Some("") => Ok(None),
            Some(v) => v
                .parse::<f64>()
                .map(Some)
                .map_err(|_| invalid(key, v, "Input should be a valid number")),
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

fn error_body(status: StatusCode, code: &str, detail: &str, meta: Value) -> Response {
    json(
        status,
        json_value!({
            "type": "ErrorResponse",
            "error": {
                "id": "6c1f0ab3-2b4e-4f0e-9d0e-3b6a7d5c9e11",
                "status": status.as_u16(),
                "code": code,
                "detail": detail,
                "meta": meta,
            },
            "time": 1_793_404_800,
        }),
    )
}

fn invalid(key: &str, value: &str, msg: &str) -> Response {
    error_body(
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION",
        "Unable to validate request parameter(s)",
        json_value!({ "errors": [{ "type": "value_error", "loc": ["query", key], "msg": msg, "input": value }] }),
    )
}

async fn not_found() -> Response {
    error_body(
        StatusCode::NOT_FOUND,
        "RESOURCE_NOT_FOUND",
        "Resource not found.",
        json_value!({}),
    )
}

fn authorize(headers: &HeaderMap) -> Result<(), Response> {
    match headers
        .get("x-subscription-token")
        .and_then(|v| v.to_str().ok())
    {
        None | Some("") => Err(error_body(
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION",
            "Unable to validate request parameter(s)",
            json_value!({ "errors": [{ "type": "missing", "loc": ["header", "x-subscription-token"], "msg": "Field required", "input": null }] }),
        )),
        Some(token) if token.starts_with("BSA") => Ok(()),
        Some(_) => Err(error_body(
            StatusCode::UNAUTHORIZED,
            "SUBSCRIPTION_TOKEN_INVALID",
            "The provided subscription token is invalid.",
            json_value!({ "component": "authentication" }),
        )),
    }
}

fn ok(body: Value) -> Response {
    let mut resp = json(StatusCode::OK, body);
    let h = resp.headers_mut();
    h.insert(
        "x-ratelimit-limit",
        HeaderValue::from_static("50, 20000000"),
    );
    h.insert(
        "x-ratelimit-policy",
        HeaderValue::from_static("50;w=1, 20000000;w=2678400"),
    );
    h.insert(
        "x-ratelimit-remaining",
        HeaderValue::from_static("49, 19999000"),
    );
    h.insert("x-ratelimit-reset", HeaderValue::from_static("1, 2419200"));
    resp
}

/// Validated `q`: present, at most 400 characters and 50 words.
fn query_param(p: &Params, required: bool) -> Result<Option<String>, Response> {
    let q = p.get("q").map(str::trim).filter(|q| !q.is_empty());
    match q {
        None if required => Err(error_body(
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION",
            "Unable to validate request parameter(s)",
            json_value!({ "errors": [{ "type": "missing", "loc": ["query", "q"], "msg": "Field required", "input": null }] }),
        )),
        None => Ok(None),
        Some(q) if q.chars().count() > 400 || q.split_whitespace().count() > 50 => {
            Err(invalid("q", q, "Query too long"))
        }
        Some(q) => Ok(Some(q.to_owned())),
    }
}

fn collection(world: &SharedWorld, name: &str) -> Vec<Value> {
    world
        .lock()
        .expect("world lock")
        .service(NAME)
        .and_then(|s| s.store.collection(name))
        .map(|c| c.values().cloned().collect())
        .unwrap_or_default()
}

fn now_unix(world: &SharedWorld) -> i64 {
    world.lock().expect("world lock").clock.now_unix()
}

// ─────────────────────────────── dates & ages ───────────────────────────────

/// Unix seconds of an ISO date or `YYYY-MM-DDTHH:MM:SS[Z]` timestamp.
fn parse_time(s: &str) -> Option<i64> {
    let date = s.get(..10)?;
    let mut parts = date.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    let mut secs = days * 86_400;
    if let Some(t) = s.get(11..19) {
        let mut hms = t.split(':').map(|x| x.parse::<i64>().unwrap_or(0));
        secs +=
            hms.next().unwrap_or(0) * 3600 + hms.next().unwrap_or(0) * 60 + hms.next().unwrap_or(0);
    }
    Some(secs)
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Brave's human `age`: "3 hours ago", "2 days ago", "1 week ago", else "Month D, YYYY".
fn age(page_age: &str, now: i64) -> Option<String> {
    let t = parse_time(page_age)?;
    let diff = now - t;
    let plural = |n: i64, unit: &str| {
        if n == 1 {
            format!("1 {unit} ago")
        } else {
            format!("{n} {unit}s ago")
        }
    };
    if (0..86_400).contains(&diff) {
        return Some(plural((diff / 3600).max(1), "hour"));
    }
    if (0..7 * 86_400).contains(&diff) {
        return Some(plural(diff / 86_400, "day"));
    }
    if (0..28 * 86_400).contains(&diff) {
        return Some(plural(diff / (7 * 86_400), "week"));
    }
    let iso = format_unix(t);
    let month: usize = iso[5..7].parse().ok()?;
    let day: u32 = iso[8..10].parse().ok()?;
    Some(format!("{} {day}, {}", MONTHS[month - 1], &iso[..4]))
}

/// `page_age` in Brave's format (`YYYY-MM-DDTHH:MM:SS`, no zone).
fn page_age(doc: &Value) -> Value {
    doc["page_age"]
        .as_str()
        .and_then(parse_time)
        .map_or(Value::Null, |t| {
            Value::String(format_unix(t).trim_end_matches('Z').to_owned())
        })
}

/// Freshness filter: `pd`, `pw`, `pm`, `py` or `YYYY-MM-DDtoYYYY-MM-DD`.
fn freshness(p: &Params, now: i64) -> Result<Option<(i64, i64)>, Response> {
    let Some(f) = p.get("freshness").filter(|f| !f.is_empty()) else {
        return Ok(None);
    };
    let window = |days: i64| Ok(Some((now - days * 86_400, now)));
    match f {
        "pd" => window(1),
        "pw" => window(7),
        "pm" => window(31),
        "py" => window(365),
        range => {
            let parsed = range
                .split_once("to")
                .and_then(|(a, b)| Some((parse_time(a)?, parse_time(b)? + 86_399)));
            parsed
                .map(Some)
                .ok_or_else(|| invalid("freshness", range, "Invalid freshness value"))
        }
    }
}

fn fresh(doc: &Value, window: Option<(i64, i64)>) -> bool {
    match window {
        None => true,
        Some((from, to)) => doc["page_age"]
            .as_str()
            .and_then(parse_time)
            .is_some_and(|t| t >= from && t <= to),
    }
}

// ───────────────────────────────── ranking ─────────────────────────────────

fn haystack(doc: &Value) -> String {
    let mut s = String::new();
    for key in [
        "title",
        "description",
        "keywords",
        "url",
        "source",
        "creator",
        "publisher",
    ] {
        if let Some(v) = doc[key].as_str() {
            s.push_str(v);
            s.push(' ');
        }
    }
    for key in ["extra_snippets", "tags", "categories"] {
        if let Some(list) = doc[key].as_array() {
            for v in list.iter().filter_map(Value::as_str) {
                s.push_str(v);
                s.push(' ');
            }
        }
    }
    if let Some(addr) = doc["postal_address"].as_object() {
        for v in addr.values().filter_map(Value::as_str) {
            s.push_str(v);
            s.push(' ');
        }
    }
    s
}

/// Documents matching `query`, best first (ties keep dataset order).
fn rank(docs: Vec<Value>, query: &str, window: Option<(i64, i64)>) -> Vec<Value> {
    let q = SearchQuery::parse(query);
    let mut hits: Vec<(usize, usize, Value)> = docs
        .into_iter()
        .enumerate()
        .filter(|(_, d)| fresh(d, window))
        .filter(|(_, d)| {
            let host = host_of(d["url"].as_str().unwrap_or_default());
            q.sites.iter().all(|s| host_matches(&host, s))
        })
        .filter_map(|(i, d)| q.score(&haystack(&d)).map(|m| (m, i, d)))
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    hits.into_iter().map(|(_, _, d)| d).collect()
}

fn meta_url(url: &str) -> Value {
    let scheme = url.split("://").next().unwrap_or("https");
    let netloc = url
        .split("://")
        .nth(1)
        .and_then(|r| r.split(['/', '?']).next())
        .unwrap_or_default();
    let path = path_of(url);
    let crumbs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let path = if crumbs.is_empty() {
        String::new()
    } else {
        format!("› {}", crumbs.join(" › "))
    };
    json_value!({
        "scheme": scheme,
        "netloc": netloc,
        "hostname": netloc.to_ascii_lowercase(),
        "favicon": format!("https://imgs.search.brave.com/favicon/{}", netloc.to_ascii_lowercase()),
        "path": path,
    })
}

fn query_block(q: &str, p: &Params, more: bool) -> Value {
    json_value!({
        "original": q,
        "show_strict_warning": false,
        "is_navigational": false,
        "is_news_breaking": false,
        "spellcheck_off": !p.get("spellcheck").is_none_or(|s| s != "false"),
        "country": p.get("country").unwrap_or("us").to_ascii_lowercase(),
        "bad_results": false,
        "should_fallback": false,
        "postal_code": "",
        "city": "",
        "header_country": "",
        "more_results_available": more,
        "state": "",
    })
}

fn page(
    items: Vec<Value>,
    p: &Params,
    default_count: i64,
    max_count: i64,
) -> Result<(Vec<Value>, bool), Response> {
    let count = p.int("count")?.unwrap_or(default_count);
    if !(1..=max_count).contains(&count) {
        return Err(invalid(
            "count",
            &count.to_string(),
            &format!("Input should be less than or equal to {max_count}"),
        ));
    }
    let offset = p.int("offset")?.unwrap_or(0);
    if !(0..=9).contains(&offset) {
        return Err(invalid(
            "offset",
            &offset.to_string(),
            "Input should be less than or equal to 9",
        ));
    }
    let start = (offset * count) as usize;
    let more = items.len() > start + count as usize;
    Ok((
        items.into_iter().skip(start).take(count as usize).collect(),
        more,
    ))
}

// ─────────────────────────────── result shapes ───────────────────────────────

fn web_result(doc: &Value, now: i64, extra: bool) -> Value {
    let url = doc["url"].as_str().unwrap_or_default();
    let host = host_of(url);
    let mut r = Map::new();
    r.insert("title".into(), doc["title"].clone());
    r.insert("url".into(), doc["url"].clone());
    r.insert("is_source_local".into(), json_value!(false));
    r.insert("is_source_both".into(), json_value!(false));
    r.insert("description".into(), doc["description"].clone());
    if let Some(pa) = doc["page_age"].as_str() {
        r.insert("page_age".into(), page_age(doc));
        if let Some(a) = age(pa, now) {
            r.insert("age".into(), Value::String(a));
        }
    }
    r.insert(
        "profile".into(),
        json_value!({
            "name": doc["site_name"].as_str().map_or(host.clone(), str::to_owned),
            "url": format!("https://{}", url.split("://").nth(1).and_then(|r| r.split('/').next()).unwrap_or_default()),
            "long_name": host,
            "img": format!("https://imgs.search.brave.com/favicon/{host}"),
        }),
    );
    r.insert(
        "language".into(),
        doc.get("language").cloned().unwrap_or(json_value!("en")),
    );
    r.insert("family_friendly".into(), json_value!(true));
    r.insert("type".into(), json_value!("search_result"));
    r.insert(
        "subtype".into(),
        doc.get("subtype")
            .cloned()
            .unwrap_or(json_value!("generic")),
    );
    r.insert("is_live".into(), json_value!(false));
    r.insert("meta_url".into(), meta_url(url));
    if extra && let Some(snips) = doc.get("extra_snippets") {
        r.insert("extra_snippets".into(), snips.clone());
    }
    Value::Object(r)
}

fn news_result(doc: &Value, now: i64, extra: bool) -> Value {
    let url = doc["url"].as_str().unwrap_or_default();
    let mut r = Map::new();
    r.insert("type".into(), json_value!("news_result"));
    r.insert("title".into(), doc["title"].clone());
    r.insert("url".into(), doc["url"].clone());
    r.insert("description".into(), doc["description"].clone());
    r.insert(
        "age".into(),
        doc["page_age"]
            .as_str()
            .and_then(|pa| age(pa, now))
            .map_or(Value::Null, Value::String),
    );
    r.insert("page_age".into(), page_age(doc));
    r.insert(
        "source".into(),
        doc.get("source")
            .cloned()
            .unwrap_or_else(|| Value::String(host_of(url))),
    );
    r.insert("meta_url".into(), meta_url(url));
    r.insert(
        "breaking".into(),
        doc.get("breaking").cloned().unwrap_or(json_value!(false)),
    );
    r.insert("is_live".into(), json_value!(false));
    if let Some(t) = doc["thumbnail"].as_str() {
        r.insert("thumbnail".into(), json_value!({ "src": t, "original": t }));
    }
    if extra && let Some(snips) = doc.get("extra_snippets") {
        r.insert("extra_snippets".into(), snips.clone());
    }
    Value::Object(r)
}

fn video_result(doc: &Value, now: i64) -> Value {
    let url = doc["url"].as_str().unwrap_or_default();
    let mut video = Map::new();
    for key in ["duration", "views", "creator", "publisher", "tags"] {
        if let Some(v) = doc.get(key) {
            video.insert(key.into(), v.clone());
        }
    }
    let mut r = Map::new();
    r.insert("type".into(), json_value!("video_result"));
    r.insert("url".into(), doc["url"].clone());
    r.insert("title".into(), doc["title"].clone());
    r.insert("description".into(), doc["description"].clone());
    if let Some(a) = doc["page_age"].as_str().and_then(|pa| age(pa, now)) {
        r.insert("age".into(), Value::String(a));
    }
    r.insert("page_age".into(), page_age(doc));
    r.insert("video".into(), Value::Object(video));
    r.insert("meta_url".into(), meta_url(url));
    if let Some(t) = doc["thumbnail"].as_str() {
        r.insert("thumbnail".into(), json_value!({ "src": t, "original": t }));
    }
    Value::Object(r)
}

fn location_result(doc: &Value) -> Value {
    let mut r = Map::new();
    r.insert("type".into(), json_value!("location_result"));
    r.insert("id".into(), doc["id"].clone());
    r.insert(
        "title".into(),
        doc.get("title").cloned().unwrap_or(json_value!("")),
    );
    r.insert(
        "url".into(),
        doc.get("url").cloned().unwrap_or(json_value!("")),
    );
    r.insert(
        "provider_url".into(),
        doc.get("provider_url")
            .cloned()
            .or_else(|| doc.get("url").cloned())
            .unwrap_or(json_value!("")),
    );
    r.insert("is_source_local".into(), json_value!(true));
    r.insert("is_source_both".into(), json_value!(false));
    if let Some(d) = doc.get("snippet") {
        r.insert("description".into(), d.clone());
    }
    r.insert("zoom_level".into(), json_value!(17));
    for key in [
        "coordinates",
        "postal_address",
        "opening_hours",
        "contact",
        "price_range",
        "rating",
        "categories",
        "serves_cuisine",
        "timezone",
        "thumbnail",
        "profiles",
        "reviews",
    ] {
        if let Some(v) = doc.get(key) {
            let v = if key == "postal_address" {
                let mut addr = v.as_object().cloned().unwrap_or_default();
                addr.insert("type".into(), json_value!("PostalAddress"));
                Value::Object(addr)
            } else {
                v.clone()
            };
            r.insert(key.into(), v);
        }
    }
    Value::Object(r)
}

// ───────────────────────────────── handlers ─────────────────────────────────

fn begin(headers: &HeaderMap, raw: Option<&str>) -> Result<Params, Response> {
    authorize(headers)?;
    Ok(Params::parse(raw))
}

macro_rules! tryr {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(r) => return r,
        }
    };
}

/// Pseudo-random-looking but deterministic hex digest (FNV-1a 64).
fn digest(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn summarizer_key(q: &str, p: &Params) -> String {
    json_value!({
        "query": q,
        "country": p.get("country").unwrap_or("us").to_ascii_lowercase(),
        "language": p.get("search_lang").unwrap_or("en"),
        "safesearch": p.get("safesearch").unwrap_or("moderate"),
        "results_hash": digest(q),
    })
    .to_string()
}

async fn web_search(
    State(world): State<SharedWorld>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    let p = tryr!(begin(&headers, raw.as_deref()));
    let q = tryr!(query_param(&p, true)).unwrap_or_default();
    let now = now_unix(&world);
    let window = tryr!(freshness(&p, now));
    let filter: Option<Vec<String>> = p
        .get("result_filter")
        .filter(|f| !f.is_empty())
        .map(|f| f.split(',').map(|s| s.trim().to_owned()).collect());
    let wants = |kind: &str| filter.as_ref().is_none_or(|f| f.iter().any(|k| k == kind));
    let extra = p.flag("extra_snippets");

    let web_hits = rank(collection(&world, "web"), &q, window);
    let (web_page, more) = tryr!(page(web_hits.clone(), &p, 20, 20));

    let mut body = Map::new();
    body.insert("type".into(), json_value!("search"));
    if wants("query") || filter.is_none() {
        body.insert("query".into(), query_block(&q, &p, more));
    }
    let mut main = Vec::new();
    if wants("web") {
        let results: Vec<Value> = web_page.iter().map(|d| web_result(d, now, extra)).collect();
        for i in 0..results.len() {
            main.push(json_value!({ "type": "web", "index": i, "all": false }));
        }
        body.insert(
            "web".into(),
            json_value!({ "type": "search", "results": results, "family_friendly": true }),
        );
    }
    if wants("faq") {
        let faqs: Vec<Value> = web_hits
            .iter()
            .flat_map(|d| {
                d["faq"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(move |f| {
                        json_value!({
                            "question": f["question"],
                            "answer": f["answer"],
                            "title": d["title"],
                            "url": d["url"],
                            "meta_url": meta_url(d["url"].as_str().unwrap_or_default()),
                        })
                    })
            })
            .collect();
        if !faqs.is_empty() {
            main.push(json_value!({ "type": "faq", "all": true }));
            body.insert(
                "faq".into(),
                json_value!({ "type": "faq", "results": faqs }),
            );
        }
    }
    if wants("news") {
        let news: Vec<Value> = rank(collection(&world, "news"), &q, window)
            .iter()
            .take(3)
            .map(|d| news_result(d, now, extra))
            .collect();
        if !news.is_empty() {
            main.push(json_value!({ "type": "news", "all": true }));
            body.insert(
                "news".into(),
                json_value!({ "type": "news", "results": news, "mutated_by_goggles": false }),
            );
        }
    }
    if wants("videos") {
        let videos: Vec<Value> = rank(collection(&world, "videos"), &q, window)
            .iter()
            .take(3)
            .map(|d| video_result(d, now))
            .collect();
        if !videos.is_empty() {
            main.push(json_value!({ "type": "videos", "all": true }));
            body.insert(
                "videos".into(),
                json_value!({ "type": "videos", "results": videos, "mutated_by_goggles": false }),
            );
        }
    }
    if wants("locations") {
        let pois: Vec<Value> = rank(collection(&world, "pois"), &q, None)
            .iter()
            .take(20)
            .map(location_result)
            .collect();
        if !pois.is_empty() {
            body.insert(
                "locations".into(),
                json_value!({ "type": "locations", "results": pois }),
            );
        }
    }
    if p.flag("summary") && !web_hits.is_empty() {
        body.insert(
            "summarizer".into(),
            json_value!({ "type": "summarizer", "key": summarizer_key(&q, &p) }),
        );
    }
    body.insert(
        "mixed".into(),
        json_value!({ "type": "mixed", "main": main, "top": [], "side": [] }),
    );
    ok(Value::Object(body))
}

async fn news_search(
    State(world): State<SharedWorld>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    let p = tryr!(begin(&headers, raw.as_deref()));
    let q = tryr!(query_param(&p, true)).unwrap_or_default();
    let now = now_unix(&world);
    let window = tryr!(freshness(&p, now));
    let hits = rank(collection(&world, "news"), &q, window);
    let (items, more) = tryr!(page(hits, &p, 20, 50));
    let extra = p.flag("extra_snippets");
    ok(json_value!({
        "type": "news",
        "query": { "original": q, "spellcheck_off": false, "show_strict_warning": false, "more_results_available": more },
        "results": items.iter().map(|d| news_result(d, now, extra)).collect::<Vec<_>>(),
    }))
}

async fn video_search(
    State(world): State<SharedWorld>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    let p = tryr!(begin(&headers, raw.as_deref()));
    let q = tryr!(query_param(&p, true)).unwrap_or_default();
    let now = now_unix(&world);
    let window = tryr!(freshness(&p, now));
    let hits = rank(collection(&world, "videos"), &q, window);
    let (items, more) = tryr!(page(hits, &p, 20, 50));
    ok(json_value!({
        "type": "videos",
        "query": { "original": q, "spellcheck_off": false, "show_strict_warning": false, "more_results_available": more },
        "results": items.iter().map(|d| video_result(d, now)).collect::<Vec<_>>(),
        "extra": { "might_be_offensive": false },
    }))
}

async fn image_search(
    State(world): State<SharedWorld>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    let p = tryr!(begin(&headers, raw.as_deref()));
    let q = tryr!(query_param(&p, true)).unwrap_or_default();
    let count = tryr!(p.int("count")).unwrap_or(50);
    if !(1..=200).contains(&count) {
        return invalid(
            "count",
            &count.to_string(),
            "Input should be less than or equal to 200",
        );
    }
    let hits = rank(collection(&world, "images"), &q, None);
    let results: Vec<Value> = hits
        .iter()
        .take(count as usize)
        .map(|d| {
            let page_url = d["url"].as_str().unwrap_or_default();
            let image = d["image_url"].as_str().unwrap_or_default();
            let thumb = format!("https://imgs.search.brave.com/{}/rs:fit:500:0:0:0/g:ce/{}", digest(image), digest(page_url));
            json_value!({
                "type": "image_result",
                "title": d["title"],
                "url": d["url"],
                "source": d.get("source").cloned().unwrap_or_else(|| Value::String(host_of(page_url))),
                "page_fetched": d.get("page_fetched").cloned().unwrap_or(json_value!("2026-01-01T00:00:00Z")),
                "thumbnail": { "src": thumb, "width": 500, "height": 281 },
                "properties": {
                    "url": image,
                    "placeholder": format!("https://imgs.search.brave.com/{}/rs:fit:32:32:1:0/g:ce/", digest(image)),
                    "width": d.get("width").cloned().unwrap_or(json_value!(1200)),
                    "height": d.get("height").cloned().unwrap_or(json_value!(675)),
                },
                "meta_url": meta_url(page_url),
                "confidence": d.get("confidence").cloned().unwrap_or(json_value!("high")),
            })
        })
        .collect();
    ok(json_value!({
        "type": "images",
        "query": { "original": q, "spellcheck_off": false, "show_strict_warning": false },
        "results": results,
        "extra": { "might_be_offensive": false },
    }))
}

async fn summarizer(
    State(world): State<SharedWorld>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    let p = tryr!(begin(&headers, raw.as_deref()));
    let Some(key) = p.get("key").filter(|k| !k.is_empty()) else {
        return error_body(
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION",
            "Unable to validate request parameter(s)",
            json_value!({ "errors": [{ "type": "missing", "loc": ["query", "key"], "msg": "Field required", "input": null }] }),
        );
    };
    let query = serde_json::from_str::<Value>(key)
        .ok()
        .and_then(|k| k["query"].as_str().map(str::to_owned));
    let Some(query) = query else {
        return error_body(
            StatusCode::NOT_FOUND,
            "RESOURCE_NOT_FOUND",
            "Unable to find summarizer for the given key.",
            json_value!({}),
        );
    };
    let lower = query.to_lowercase();
    let canned = collection(&world, "summaries").into_iter().find(|s| {
        let terms: Vec<&str> = s["match"]
            .as_array()
            .map(|t| t.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        !terms.is_empty()
            && terms
                .iter()
                .all(|t| t.split('|').any(|alt| lower.contains(&alt.to_lowercase())))
    });
    let inline = p.flag("inline_references");
    let (title, text, refs) = match canned {
        Some(s) => (
            s["title"].as_str().unwrap_or(&query).to_owned(),
            s["text"].as_str().unwrap_or_default().to_owned(),
            s["references"]
                .as_array()
                .map(|r| {
                    r.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
        ),
        None => {
            let hits = rank(collection(&world, "web"), &query, None);
            if hits.is_empty() {
                return error_body(
                    StatusCode::NOT_FOUND,
                    "RESOURCE_NOT_FOUND",
                    "Unable to find summarizer for the given key.",
                    json_value!({}),
                );
            }
            let top: Vec<&Value> = hits.iter().take(3).collect();
            let text = top
                .iter()
                .filter_map(|d| d["description"].as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let refs = top
                .iter()
                .filter_map(|d| d["url"].as_str().map(str::to_owned))
                .collect::<Vec<String>>();
            (query.clone(), text, refs)
        }
    };
    let mut summary = vec![json_value!({ "type": "token", "data": text })];
    if inline {
        for (n, url) in refs.iter().enumerate() {
            summary.push(json_value!({
                "type": "inline_reference",
                "data": { "type": "inline_reference", "url": url, "start_index": 0, "end_index": text.len(), "number": n + 1 },
            }));
        }
    }
    let mut body = json_value!({
        "type": "summarizer",
        "status": "complete",
        "title": title,
        "summary": summary,
        "enrichments": {
            "raw": text,
            "context": refs.iter().map(|u| json_value!({ "title": u, "url": u, "meta_url": meta_url(u) })).collect::<Vec<_>>(),
        },
        "followups": [],
    });
    if p.flag("entity_info") {
        body["entities_infos"] = json_value!({});
    }
    ok(body)
}

/// Paragraph-sized chunks of a page's markdown content (or its description).
fn snippets(doc: &Value, max: usize) -> Vec<String> {
    let content = doc["content"]
        .as_str()
        .or_else(|| doc["description"].as_str())
        .unwrap_or_default();
    content
        .split("\n\n")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .take(max)
        .map(str::to_owned)
        .collect()
}

async fn llm_context(
    State(world): State<SharedWorld>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    let p = tryr!(begin(&headers, raw.as_deref()));
    let q = tryr!(query_param(&p, true)).unwrap_or_default();
    let now = now_unix(&world);
    let window = tryr!(freshness(&p, now));
    let count = tryr!(p.int("count")).unwrap_or(20).clamp(1, 50) as usize;
    let max_urls = tryr!(p.int("maximum_number_of_urls"))
        .unwrap_or(20)
        .clamp(1, 50) as usize;
    let per_url = tryr!(p.int("maximum_number_of_snippets_per_url"))
        .unwrap_or(50)
        .clamp(1, 100) as usize;
    let max_snippets = tryr!(p.int("maximum_number_of_snippets"))
        .unwrap_or(50)
        .clamp(1, 256) as usize;
    let hits = rank(collection(&world, "web"), &q, window);
    let mut generic = Vec::new();
    let mut sources = Map::new();
    let mut used = 0;
    for doc in hits.iter().take(count).take(max_urls) {
        if used >= max_snippets {
            break;
        }
        let snips: Vec<String> = snippets(doc, per_url.min(max_snippets - used));
        used += snips.len();
        let url = doc["url"].as_str().unwrap_or_default();
        generic.push(json_value!({ "url": url, "title": doc["title"], "snippets": snips }));
        let mut ages = Vec::new();
        if let Some(pa) = doc["page_age"].as_str() {
            if let Some(a) = age(pa, now) {
                ages.push(Value::String(a));
            }
            ages.push(Value::String(pa.get(..10).unwrap_or(pa).to_owned()));
        }
        let mut meta = Map::new();
        meta.insert("title".into(), doc["title"].clone());
        meta.insert("hostname".into(), Value::String(host_of(url)));
        meta.insert("age".into(), Value::Array(ages));
        if p.flag("enable_source_metadata") {
            meta.insert("site_name".into(), Value::String(host_of(url)));
            meta.insert(
                "favicon".into(),
                Value::String(format!(
                    "https://imgs.search.brave.com/favicon/{}",
                    host_of(url)
                )),
            );
        }
        sources.insert(url.to_owned(), Value::Object(meta));
    }
    let mut map = Vec::new();
    let mut poi = Value::Null;
    if p.flag("enable_local") {
        let pois = rank(collection(&world, "pois"), &q, None);
        for (i, doc) in pois.iter().take(5).enumerate() {
            let item = json_value!({
                "name": doc["title"],
                "url": doc.get("url").cloned().unwrap_or(json_value!("")),
                "title": doc["title"],
                "snippets": [doc.get("description").and_then(Value::as_str).unwrap_or_default()],
            });
            if i == 0
                && doc
                    .get("url")
                    .and_then(Value::as_str)
                    .is_some_and(|u| !u.is_empty())
            {
                poi = item.clone();
            }
            map.push(item);
        }
    }
    ok(json_value!({
        "grounding": { "generic": generic, "poi": poi, "map": map },
        "sources": sources,
    }))
}

fn pois_by_ids(world: &SharedWorld, p: &Params) -> Result<Vec<Value>, Response> {
    let ids = p.all("ids");
    if ids.is_empty() {
        return Err(error_body(
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION",
            "Unable to validate request parameter(s)",
            json_value!({ "errors": [{ "type": "missing", "loc": ["query", "ids"], "msg": "Field required", "input": null }] }),
        ));
    }
    if ids.len() > 20 {
        return Err(invalid(
            "ids",
            &ids.len().to_string(),
            "List should have at most 20 items",
        ));
    }
    let pois = collection(world, "pois");
    Ok(ids
        .iter()
        .filter_map(|id| pois.iter().find(|poi| poi["id"] == *id).cloned())
        .collect())
}

async fn local_pois(
    State(world): State<SharedWorld>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    let p = tryr!(begin(&headers, raw.as_deref()));
    let pois = tryr!(pois_by_ids(&world, &p));
    ok(json_value!({
        "type": "local_pois",
        "results": pois.iter().map(location_result).collect::<Vec<_>>(),
    }))
}

async fn local_descriptions(
    State(world): State<SharedWorld>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    let p = tryr!(begin(&headers, raw.as_deref()));
    let pois = tryr!(pois_by_ids(&world, &p));
    let results: Vec<Value> = pois
        .iter()
        .map(|poi| {
            json_value!({
                "type": "local_description",
                "id": poi["id"],
                "description": poi.get("description").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();
    ok(json_value!({ "type": "local_descriptions", "results": results }))
}

/// Great-circle distance in metres.
fn distance_m(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, lo1, la2, lo2) = (
        a.0.to_radians(),
        a.1.to_radians(),
        b.0.to_radians(),
        b.1.to_radians(),
    );
    let h = ((la2 - la1) / 2.0).sin().powi(2)
        + la1.cos() * la2.cos() * ((lo2 - lo1) / 2.0).sin().powi(2);
    2.0 * 6_371_000.0 * h.sqrt().asin()
}

fn coords(doc: &Value) -> Option<(f64, f64)> {
    let c = doc["coordinates"].as_array()?;
    Some((c.first()?.as_f64()?, c.get(1)?.as_f64()?))
}

async fn place_search(
    State(world): State<SharedWorld>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    let p = tryr!(begin(&headers, raw.as_deref()));
    let q = tryr!(query_param(&p, false));
    let lat = tryr!(p.float("latitude"));
    let lon = tryr!(p.float("longitude"));
    let location = p.get("location").filter(|l| !l.trim().is_empty());
    if q.is_none() && location.is_none() && (lat.is_none() || lon.is_none()) {
        return error_body(
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION",
            "Unable to validate request parameter(s)",
            json_value!({ "errors": [{ "type": "value_error", "loc": ["query"], "msg": "Either 'q', 'location' or 'latitude' and 'longitude' must be provided", "input": null }] }),
        );
    }
    let count = tryr!(p.int("count")).unwrap_or(20).clamp(1, 50) as usize;
    let mut pois = collection(&world, "pois");
    // `location` scopes to POIs whose address mentions its leading word(s), e.g. the city.
    if let Some(loc) = location {
        let loc_tokens = tokens(loc);
        pois.retain(|poi| {
            let addr = haystack(&json_value!({ "postal_address": poi["postal_address"] }));
            let addr: Vec<String> = tokens(&addr);
            loc_tokens.iter().filter(|t| addr.contains(t)).count() * 2 >= loc_tokens.len()
        });
    }
    let mut ranked = match &q {
        Some(q) => rank(pois, q, None),
        None => pois,
    };
    let anchor = lat.zip(lon);
    if let Some(a) = anchor {
        ranked.sort_by(|x, y| {
            let dx = coords(x).map_or(f64::MAX, |c| distance_m(a, c));
            let dy = coords(y).map_or(f64::MAX, |c| distance_m(a, c));
            dx.total_cmp(&dy)
        });
    }
    let results: Vec<Value> = ranked
        .iter()
        .take(count)
        .map(|poi| {
            let mut r = location_result(poi);
            if let (Some(a), Some(c)) = (anchor, coords(poi)) {
                let km = (distance_m(a, c) / 100.0).round() / 10.0;
                r["distance"] = json_value!({ "value": km, "units": "km" });
            }
            r
        })
        .collect();
    let mut body = Map::new();
    body.insert("type".into(), json_value!("locations"));
    if let Some(q) = &q {
        body.insert(
            "query".into(),
            json_value!({ "original": q, "spellcheck_off": true }),
        );
    }
    body.insert("results".into(), Value::Array(results.clone()));
    let area = anchor
        .map(|(la, lo)| json_value!({ "coordinates": [la, lo] }))
        .or_else(|| {
            results
                .first()
                .and_then(|r| r.get("coordinates").cloned())
                .map(|c| json_value!({ "coordinates": c }))
        });
    if let Some(mut area) = area {
        if let Some(loc) = location {
            area["name"] = Value::String(loc.to_owned());
        }
        body.insert("location".into(), area);
    }
    ok(Value::Object(body))
}
