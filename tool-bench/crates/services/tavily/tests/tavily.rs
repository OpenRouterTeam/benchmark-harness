use std::path::Path;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode},
};
use mock_core::{
    dataset::Dataset,
    serve::service_app,
    world::{SharedWorld, WorldState, shared},
};
use mock_tavily::Tavily;
use serde_json::{Value, json};
use tower::ServiceExt;

const KEY: &str = "Bearer tvly-toolbench";

fn world() -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture");
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    auth: Option<&str>,
    body: Value,
) -> (StatusCode, HeaderMap, String) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "api.tavily.com")
        .header("content-type", "application/json");
    if let Some(a) = auth {
        req = req.header("authorization", a);
    }
    let body = if body.is_null() {
        Body::empty()
    } else {
        Body::from(body.to_string())
    };
    let resp = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = to_bytes(body, usize::MAX).await.unwrap();
    (
        parts.status,
        parts.headers,
        String::from_utf8(bytes.to_vec()).unwrap(),
    )
}

async fn post(app: &Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let (status, _, text) = send(app, "POST", uri, Some(KEY), body).await;
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

fn urls(v: &Value) -> Vec<String> {
    v["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["url"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn missing_or_bad_key_is_401() {
    let app = service_app(world(), &Tavily);
    let (status, _, text) = send(&app, "POST", "/search", None, json!({"query": "acme"})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap(),
        json!({"detail": {"error": "Unauthorized: missing or invalid API key."}})
    );
    let (status, _, _) = send(
        &app,
        "POST",
        "/search",
        Some("Bearer nope"),
        json!({"query": "acme"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // The key in the body is accepted too.
    let (status, _, _) = send(
        &app,
        "POST",
        "/search",
        None,
        json!({"query": "acme", "api_key": "tvly-abc"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn search_ranks_by_matched_terms_and_has_tavily_shape() {
    let app = service_app(world(), &Tavily);
    let (status, body) = post(&app, "/search", json!({"query": "Acme API rate limits"})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(urls(&body)[0], "https://docs.acme.dev/api/limits");
    assert_eq!(body["query"], "Acme API rate limits");
    assert_eq!(body["answer"], Value::Null);
    assert_eq!(body["follow_up_questions"], Value::Null);
    assert_eq!(body["images"], json!([]));
    let first = &body["results"][0];
    assert_eq!(first["raw_content"], Value::Null);
    assert!(first["score"].as_f64().unwrap() > 0.5);
    assert!(body["request_id"].as_str().unwrap().len() == 36);
    assert!(body["response_time"].is_number());
}

#[tokio::test]
async fn search_filters_domains_dates_and_limits() {
    let app = service_app(world(), &Tavily);
    let (_, body) = post(
        &app,
        "/search",
        json!({"query": "acme", "include_domains": ["news.example.com"]}),
    )
    .await;
    assert_eq!(urls(&body), ["https://news.example.com/acme-raises"]);

    let (_, body) = post(
        &app,
        "/search",
        json!({"query": "acme", "exclude_domains": ["docs.acme.dev"]}),
    )
    .await;
    assert_eq!(urls(&body), ["https://news.example.com/acme-raises"]);

    // now = 2026-10-30: only pages published in the last week.
    let (_, body) = post(
        &app,
        "/search",
        json!({"query": "acme", "time_range": "week", "max_results": 10}),
    )
    .await;
    assert_eq!(
        urls(&body),
        [
            "https://docs.acme.dev/guides/billing",
            "https://news.example.com/acme-raises"
        ]
    );
    let (_, body) = post(
        &app,
        "/search",
        json!({"query": "acme", "start_date": "2026-01-01", "end_date": "2026-06-30"}),
    )
    .await;
    assert_eq!(
        urls(&body),
        ["https://docs.acme.dev/guides/billing/refunds"]
    );

    let (_, body) = post(&app, "/search", json!({"query": "acme docs"})).await;
    assert_eq!(body["results"].as_array().unwrap().len(), 5);
    let (_, body) = post(&app, "/search", json!({"query": "acme", "max_results": 2})).await;
    assert_eq!(body["results"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn search_raw_content_images_and_favicon() {
    let app = service_app(world(), &Tavily);
    let (_, body) = post(
        &app,
        "/search",
        json!({"query": "billing invoices", "include_raw_content": true, "include_images": true,
               "include_image_descriptions": true, "include_favicon": true, "max_results": 1}),
    )
    .await;
    let r = &body["results"][0];
    assert!(
        r["raw_content"]
            .as_str()
            .unwrap()
            .starts_with("# Billing guide")
    );
    assert!(r.get("favicon").is_some());
    assert_eq!(
        body["images"],
        json!([{"url": "https://docs.acme.dev/img/invoice.png", "description": "A sample Acme invoice"}])
    );
    let (_, body) = post(
        &app,
        "/search",
        json!({"query": "billing invoices", "include_raw_content": "text", "max_results": 1}),
    )
    .await;
    assert!(
        body["results"][0]["raw_content"]
            .as_str()
            .unwrap()
            .starts_with("Billing guide")
    );
}

#[tokio::test]
async fn unknown_queries_return_empty_results() {
    let app = service_app(world(), &Tavily);
    let (status, body) = post(
        &app,
        "/search",
        json!({"query": "zebra migration patterns"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["results"], json!([]));
}

#[tokio::test]
async fn search_validation_errors() {
    let app = service_app(world(), &Tavily);
    let (status, body) = post(&app, "/search", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, json!({"detail": {"error": "Query is missing."}}));
    let (status, _) = post(
        &app,
        "/search",
        json!({"query": "a", "search_depth": "deep"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = post(
        &app,
        "/search",
        json!({"query": "a", "time_range": "decade"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = post(
        &app,
        "/search",
        json!({"query": "a", "start_date": "10/01/2026"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn extract_returns_content_and_failed_results() {
    let app = service_app(world(), &Tavily);
    let (status, body) = post(
        &app,
        "/extract",
        json!({"urls": ["https://docs.acme.dev/api/limits/", "https://nope.example.org/x"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["results"][0]["url"],
        "https://docs.acme.dev/api/limits/"
    );
    assert!(
        body["results"][0]["raw_content"]
            .as_str()
            .unwrap()
            .contains("**500** requests")
    );
    assert_eq!(
        body["failed_results"],
        json!([{"url": "https://nope.example.org/x", "error": "Failed to fetch url"}])
    );

    let (_, body) = post(
        &app,
        "/extract",
        json!({"urls": ["https://docs.acme.dev/api/limits"], "format": "text"}),
    )
    .await;
    let text = body["results"][0]["raw_content"].as_str().unwrap();
    assert!(
        text.contains("500 requests per minute per key. See errors."),
        "{text}"
    );

    let (_, body) = post(
        &app,
        "/extract",
        json!({"urls": ["https://docs.acme.dev/guides/billing"], "query": "refunds", "chunks_per_source": 1}),
    )
    .await;
    assert_eq!(
        body["results"][0]["raw_content"],
        "Refunds are processed within 5 business days."
    );

    let many: Vec<String> = (0..21).map(|i| format!("https://a.example/{i}")).collect();
    let (status, _) = post(&app, "/extract", json!({ "urls": many })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = post(&app, "/extract", json!({"urls": []})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn crawl_follows_internal_links_by_depth_and_paths() {
    let app = service_app(world(), &Tavily);
    let (status, body) = post(&app, "/crawl", json!({"url": "https://docs.acme.dev"})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["base_url"], "https://docs.acme.dev");
    assert_eq!(
        urls(&body),
        [
            "https://docs.acme.dev/",
            "https://docs.acme.dev/guides/billing",
            "https://docs.acme.dev/api/limits"
        ]
    );
    let (_, body) = post(
        &app,
        "/crawl",
        json!({"url": "docs.acme.dev", "max_depth": 2, "select_paths": ["^/guides/.*"]}),
    )
    .await;
    assert_eq!(
        urls(&body),
        [
            "https://docs.acme.dev/guides/billing",
            "https://docs.acme.dev/guides/billing/refunds"
        ]
    );
    let (_, body) = post(
        &app,
        "/crawl",
        json!({"url": "https://docs.acme.dev", "max_depth": 3, "limit": 2}),
    )
    .await;
    assert_eq!(body["results"].as_array().unwrap().len(), 2);
    let (status, _) = post(
        &app,
        "/crawl",
        json!({"url": "https://docs.acme.dev", "select_paths": ["("]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn map_lists_discovered_urls_including_external() {
    let app = service_app(world(), &Tavily);
    let (_, body) = post(&app, "/map", json!({"url": "https://docs.acme.dev"})).await;
    assert_eq!(
        body["results"],
        json!([
            "https://docs.acme.dev/",
            "https://docs.acme.dev/guides/billing",
            "https://docs.acme.dev/api/limits",
            "https://docs.acme.dev/missing",
            "https://github.com/acme/sdk"
        ])
    );
    let (_, body) = post(
        &app,
        "/map",
        json!({"url": "https://docs.acme.dev", "allow_external": false}),
    )
    .await;
    assert!(
        !body["results"]
            .as_array()
            .unwrap()
            .contains(&json!("https://github.com/acme/sdk"))
    );
}

#[tokio::test]
async fn research_is_created_stored_and_completed() {
    let w = world();
    let app = service_app(w.clone(), &Tavily);
    let (status, created) = post(
        &app,
        "/research",
        json!({"input": "Summarise Acme's funding history", "model": "mini"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["status"], "pending");
    assert_eq!(created["created_at"], "2026-10-30T12:00:07Z");
    let id = created["request_id"].as_str().unwrap().to_owned();

    let (status, _, text) = send(
        &app,
        "GET",
        &format!("/research/{id}"),
        Some(KEY),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let done: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(done["status"], "completed");
    assert!(done["content"].as_str().unwrap().contains("Series C: $80M"));
    assert_eq!(
        done["sources"][0]["url"],
        "https://news.example.com/acme-raises"
    );

    let state = w.lock().unwrap().snapshot();
    let doc = &state["services"]["tavily"]["research"][&id];
    assert_eq!(doc["model"], "mini");
    assert_eq!(doc["input"], "Summarise Acme's funding history");

    // No canned report: a synthesis of matching pages.
    let (_, created) = post(&app, "/research", json!({"input": "Acme billing invoices"})).await;
    let id2 = created["request_id"].as_str().unwrap();
    assert_eq!(created["model"], "auto");
    let (_, _, text) = send(
        &app,
        "GET",
        &format!("/research/{id2}"),
        Some(KEY),
        Value::Null,
    )
    .await;
    let done: Value = serde_json::from_str(&text).unwrap();
    assert!(
        done["content"]
            .as_str()
            .unwrap()
            .contains("Billing guide | Acme Docs")
    );
}

#[tokio::test]
async fn research_errors_and_streaming() {
    let app = service_app(world(), &Tavily);
    let (status, body) = post(&app, "/research", json!({"model": "mini"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, json!({"detail": {"error": "Input is missing."}}));
    let (status, _) = post(&app, "/research", json!({"input": "x", "model": "max"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, text) = send(&app, "GET", "/research/unknown", Some(KEY), Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap(),
        json!({"detail": {"error": "Research task not found"}})
    );

    let (status, headers, text) = send(
        &app,
        "POST",
        "/research",
        Some(KEY),
        json!({"input": "Acme funding", "stream": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "text/event-stream");
    assert!(
        text.contains(
            "\"delta\":{\"role\":\"assistant\",\"content\":\"# Acme funding history\\n\"}"
        )
    );
    assert!(text.ends_with("event: done\ndata: {}\n\n"));
}

#[tokio::test]
async fn unknown_routes_are_404() {
    let app = service_app(world(), &Tavily);
    let (status, _, text) = send(&app, "GET", "/usage", Some(KEY), Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(text, r#"{"detail":"Not Found"}"#);
}
