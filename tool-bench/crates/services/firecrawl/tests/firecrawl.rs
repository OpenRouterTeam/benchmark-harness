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
use mock_firecrawl::Firecrawl;
use serde_json::{Value, json};
use tower::ServiceExt;

fn world_at(rel: &str) -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

fn world() -> SharedWorld {
    world_at("tests/fixture")
}

fn app(w: &SharedWorld) -> Router {
    service_app(w.clone(), &Firecrawl)
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    body: Body,
    content_type: &str,
    auth: bool,
) -> (StatusCode, HeaderMap, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "world:8102")
        .header("content-type", content_type);
    if auth {
        req = req.header("authorization", "Bearer fc-test");
    }
    let resp = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = to_bytes(body, usize::MAX).await.unwrap();
    (
        parts.status,
        parts.headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn call(app: &Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let body = if body.is_null() {
        Body::empty()
    } else {
        Body::from(body.to_string())
    };
    let (s, _, v) = send(app, method, uri, body, "application/json", true).await;
    (s, v)
}

fn state(w: &SharedWorld) -> Value {
    w.lock().unwrap().snapshot()["services"]["firecrawl"].clone()
}

fn urls(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|r| r["url"].as_str().unwrap().to_owned())
        .collect()
}

// ───────────── auth and errors ─────────────

#[tokio::test]
async fn requires_a_bearer_token_and_404s_unknown_routes() {
    let w = world();
    let app = app(&w);
    let (status, _, body) = send(
        &app,
        "POST",
        "/v2/scrape",
        Body::from(r#"{"url":"https://acme.dev"}"#),
        "application/json",
        false,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        body,
        json!({"success": false, "error": "Unauthorized: Invalid token"})
    );
    let (status, body) = call(&app, "GET", "/v2/nothing", Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["success"], false);
}

// ───────────── scrape ─────────────

#[tokio::test]
async fn scrapes_known_pages_in_requested_formats() {
    let w = world();
    let app = app(&w);
    let (status, body) = call(
        &app,
        "POST",
        "/v2/scrape",
        json!({"url": "http://www.acme.dev/pricing/", "formats": ["markdown", "links", {"type": "json", "schema": {"type": "object", "properties": {"pro": {"type": "number"}}}}, {"type": "query", "prompt": "pro price?"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    let doc = &body["data"];
    assert_eq!(doc["markdown"], "# Pricing\n\nPro is $20/month.");
    assert_eq!(doc["json"], json!({"pro": 20}));
    assert_eq!(doc["answer"], "Pro costs $20/month.");
    assert_eq!(doc["links"], json!([]));
    assert_eq!(doc["metadata"]["url"], "https://acme.dev/pricing");
    assert_eq!(doc["metadata"]["sourceURL"], "http://www.acme.dev/pricing/");
    assert_eq!(doc["metadata"]["statusCode"], 200);
    assert_eq!(doc["metadata"]["creditsUsed"], 9);
    let scrape_id = doc["metadata"]["scrapeId"].as_str().unwrap();
    assert_eq!(
        state(&w)["scrapes"][scrape_id]["url"],
        "https://acme.dev/pricing"
    );

    let (_, home) = call(
        &app,
        "POST",
        "/v2/scrape",
        json!({"url": "https://acme.dev", "formats": ["links", "images", "html"]}),
    )
    .await;
    assert_eq!(home["data"]["links"], json!(["https://acme.dev/docs"]));
    assert_eq!(home["data"]["images"], json!(["https://acme.dev/logo.png"]));
    assert!(
        home["data"]["html"]
            .as_str()
            .unwrap()
            .starts_with("<h1>Acme</h1>")
    );
}

#[tokio::test]
async fn scrape_errors_match_firecrawl() {
    let w = world();
    let app = app(&w);
    let (status, body) = call(
        &app,
        "POST",
        "/v2/scrape",
        json!({"url": "https://acme.dev/missing"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["metadata"]["statusCode"], 404);

    let (status, body) = call(
        &app,
        "POST",
        "/v2/scrape",
        json!({"url": "https://no-such-host.example/"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "SCRAPE_DNS_RESOLUTION_ERROR");

    let (status, body) = call(&app, "POST", "/v2/scrape", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["details"][0]["path"], json!(["url"]));

    let (status, body) = call(
        &app,
        "POST",
        "/v2/scrape",
        json!({"alexandria": {"provider": "fred", "capability": "series"}}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "Exchange is not enabled for this team.");
    let (status, _) = call(&app, "GET", "/exchange/discover", Value::Null).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn interact_sessions_follow_scrapes() {
    let w = world();
    let app = app(&w);
    let (status, _) = call(
        &app,
        "POST",
        "/v2/scrape/nope/interact",
        json!({"prompt": "click"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, scraped) = call(
        &app,
        "POST",
        "/v2/scrape",
        json!({"url": "https://acme.dev/pricing"}),
    )
    .await;
    let id = scraped["data"]["metadata"]["scrapeId"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, body) = call(
        &app,
        "POST",
        &format!("/v2/scrape/{id}/interact"),
        json!({"prompt": "open the Pro plan"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["output"]
            .as_str()
            .unwrap()
            .contains("open the Pro plan")
    );
    assert_eq!(body["exitCode"], 0);
    let (status, body) = call(
        &app,
        "DELETE",
        &format!("/v2/scrape/{id}/interact"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    let (status, _) = call(
        &app,
        "POST",
        &format!("/v2/scrape/{id}/interact"),
        json!({"code": "1+1"}),
    )
    .await;
    assert_eq!(status, StatusCode::GONE);
}

// ───────────── map / search ─────────────

#[tokio::test]
async fn maps_a_site_with_search_and_limit() {
    let w = world();
    let app = app(&w);
    let (_, all) = call(&app, "POST", "/v2/map", json!({"url": "https://acme.dev"})).await;
    let links = urls(&all["links"]);
    assert_eq!(links.len(), 16);
    assert!(links.contains(&"https://acme.dev/careers".to_owned()));
    assert!(!links.iter().any(|u| u.contains("other.org")));

    let (_, found) = call(
        &app,
        "POST",
        "/v2/map",
        json!({"url": "https://acme.dev", "search": "pricing"}),
    )
    .await;
    assert_eq!(urls(&found["links"]), ["https://acme.dev/pricing"]);
    assert_eq!(found["links"][0]["title"], "Pricing | Acme");

    let (_, limited) = call(
        &app,
        "POST",
        "/v2/map",
        json!({"url": "https://acme.dev", "limit": 3}),
    )
    .await;
    assert_eq!(limited["links"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn searches_the_index_by_relevance_and_source() {
    let w = world();
    let app = app(&w);
    let (status, body) = call(
        &app,
        "POST",
        "/v2/search",
        json!({"query": "acme pricing", "sources": ["web", "news", "images"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        urls(&body["data"]["web"]),
        ["https://acme.dev/pricing", "https://reviews.example/acme"]
    );
    assert_eq!(body["data"]["news"][0]["snippet"], "Acme pricing to change");
    assert_eq!(
        body["data"]["images"][0]["imageUrl"],
        "https://acme.dev/pricing.png"
    );
    let id = body["id"].as_str().unwrap();
    assert_eq!(state(&w)["searches"][id]["query"], "acme pricing");

    let (_, filtered) = call(
        &app,
        "POST",
        "/v2/search",
        json!({"query": "acme pricing", "excludeDomains": ["acme.dev"]}),
    )
    .await;
    assert_eq!(
        urls(&filtered["data"]["web"]),
        ["https://reviews.example/acme"]
    );
    let (_, site) = call(
        &app,
        "POST",
        "/v2/search",
        json!({"query": "pricing site:acme.dev", "scrapeOptions": {"formats": ["markdown"]}}),
    )
    .await;
    assert_eq!(
        site["data"]["web"][0]["markdown"],
        "# Pricing\n\nPro is $20/month."
    );
    let (_, none) = call(
        &app,
        "POST",
        "/v2/search",
        json!({"query": "submarine recipes"}),
    )
    .await;
    assert_eq!(none["data"], json!({"web": []}));

    let (status, body) = call(
        &app,
        "POST",
        "/v2/search",
        json!({"query": "acme", "sources": ["web", "alexandria"], "domainTools": true}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "Exchange is not enabled for this team.");
    let (status, _) = call(&app, "POST", "/v2/search", json!({"query": ""})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn search_categories_reach_developer_and_gov_indexes() {
    let w = world();
    let app = app(&w);
    let (_, dev) = call(
        &app,
        "POST",
        "/v2/search",
        json!({"query": "sdk retry 429", "categories": ["developer"]}),
    )
    .await;
    assert_eq!(dev["data"]["web"][0]["category"], "developer");
    assert_eq!(
        dev["data"]["web"][0]["url"],
        "https://github.com/acme/sdk/issues/12"
    );
    let (_, gov) = call(
        &app,
        "POST",
        "/v2/search",
        json!({"query": "consumer privacy california", "categories": ["gov"]}),
    )
    .await;
    assert_eq!(
        urls(&gov["data"]["web"]),
        [
            "https://leginfo.legislature.ca.gov/ccpa",
            "https://www.law.cornell.edu/uscode/text/15/6501"
        ]
    );
    let (_, research) = call(
        &app,
        "POST",
        "/v2/search",
        json!({"query": "acme pricing", "categories": ["research"]}),
    )
    .await;
    assert_eq!(
        urls(&research["data"]["web"]),
        ["https://reviews.example/acme"]
    );
}

#[tokio::test]
async fn search_and_job_feedback_are_recorded() {
    let w = world();
    let app = app(&w);
    let (status, body) = call(
        &app,
        "POST",
        "/v2/search/00000000-0000-0000-0000-000000000000/feedback",
        json!({"rating": "good"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["feedbackErrorCode"], "SEARCH_NOT_FOUND");

    let (_, search) = call(&app, "POST", "/v2/search", json!({"query": "acme pricing"})).await;
    let id = search["id"].as_str().unwrap();
    let path = format!("/v2/search/{id}/feedback");
    let (status, body) = call(&app, "POST", &path, json!({"rating": "good"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["feedbackErrorCode"], "INVALID_FEEDBACK");
    let (status, body) = call(
        &app,
        "POST",
        &path,
        json!({"rating": "good", "valuableResults": [{"source": "web", "position": 1}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["creditsRefunded"], 1);
    assert_eq!(body["alreadySubmitted"], false);
    let feedback_id = body["feedbackId"].as_str().unwrap();
    assert_eq!(state(&w)["feedback"][feedback_id]["jobId"], id);

    let (_, again) = call(
        &app,
        "POST",
        "/v2/feedback",
        json!({"endpoint": "search", "jobId": id, "rating": "partial", "note": "ok"}),
    )
    .await;
    assert_eq!(again["alreadySubmitted"], true);
    assert_eq!(again["creditsRefunded"], 0);
    let (status, _) = call(
        &app,
        "POST",
        "/v2/feedback",
        json!({"endpoint": "scrape", "rating": "good"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = call(
        &app,
        "POST",
        "/v2/feedback",
        json!({"endpoint": "alexandria", "rating": "good"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], false);
}

// ───────────── crawl ─────────────

#[tokio::test]
async fn crawls_are_scoped_paginated_and_stored() {
    let w = world();
    let app = app(&w);
    let (status, started) = call(
        &app,
        "POST",
        "/v2/crawl",
        json!({"url": "https://acme.dev/docs", "excludePaths": ["page-12$"], "scrapeOptions": {"formats": ["markdown"]}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let id = started["id"].as_str().unwrap().to_owned();
    assert_eq!(started["url"], format!("http://world:8102/v2/crawl/{id}"));
    let job = &state(&w)["crawls"][&id];
    assert_eq!(job["total"], 11);
    assert_eq!(job["status"], "completed");

    let (_, page1) = call(&app, "GET", &format!("/v2/crawl/{id}"), Value::Null).await;
    assert_eq!(page1["status"], "completed");
    assert_eq!(page1["total"], 11);
    assert_eq!(page1["data"].as_array().unwrap().len(), 10);
    assert_eq!(
        page1["next"],
        format!("http://world:8102/v2/crawl/{id}?skip=10")
    );
    assert_eq!(
        page1["data"][0]["metadata"]["sourceURL"],
        "https://acme.dev/docs/page-01"
    );
    let (_, page2) = call(&app, "GET", &format!("/v2/crawl/{id}?skip=10"), Value::Null).await;
    assert_eq!(page2["data"].as_array().unwrap().len(), 1);
    assert_eq!(page2["next"], Value::Null);

    let (_, included) = call(
        &app,
        "POST",
        "/v2/crawl",
        json!({"url": "https://acme.dev", "includePaths": ["^/blog/"], "limit": 5}),
    )
    .await;
    let other = included["id"].as_str().unwrap();
    assert_eq!(
        state(&w)["crawls"][other]["pages"],
        json!(["https://acme.dev/blog/launch"])
    );

    let (status, body) = call(
        &app,
        "POST",
        "/v2/crawl",
        json!({"url": "https://acme.dev", "includePaths": ["("]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("includePaths"));
    let (status, _) = call(&app, "GET", "/v2/crawl/missing", Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&app, "DELETE", &format!("/v2/crawl/{id}"), Value::Null).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (_, errors) = call(&app, "GET", &format!("/v2/crawl/{id}/errors"), Value::Null).await;
    assert_eq!(errors, json!({"errors": [], "robotsBlocked": []}));
    let (_, active) = call(&app, "GET", "/v2/crawl/active", Value::Null).await;
    assert_eq!(active, json!({"success": true, "crawls": []}));
}

// ───────────── agent ─────────────

#[tokio::test]
async fn agent_jobs_resolve_from_the_dataset() {
    let w = world();
    let app = app(&w);
    let (_, started) = call(
        &app,
        "POST",
        "/v2/agent",
        json!({"prompt": "List Acme's competitors and their pricing"}),
    )
    .await;
    assert_eq!(started["status"], "processing");
    assert_eq!(started["threadTurn"], 1);
    let id = started["id"].as_str().unwrap();
    let (_, done) = call(&app, "GET", &format!("/v2/agent/{id}"), Value::Null).await;
    assert_eq!(done["status"], "completed");
    assert_eq!(done["data"]["competitors"][0]["name"], "Beta");
    assert_eq!(done["creditsUsed"], 180);

    let thread = started["threadId"].as_str().unwrap();
    let (_, follow) = call(
        &app,
        "POST",
        "/v2/agent",
        json!({"prompt": "Acme competitors pricing again", "threadId": thread, "maxCredits": 100}),
    )
    .await;
    assert_eq!(follow["threadTurn"], 2);
    let (_, limited) = call(
        &app,
        "GET",
        &format!("/v2/agent/{}", follow["id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    assert_eq!(limited["status"], "failed");
    assert_eq!(limited["stopReason"], "credit_limit_reached");
    assert_eq!(limited["creditsUsed"], 100);

    let (_, unknown) = call(
        &app,
        "POST",
        "/v2/agent",
        json!({"prompt": "the airspeed of an unladen swallow"}),
    )
    .await;
    let (_, failed) = call(
        &app,
        "GET",
        &format!("/v2/agent/{}", unknown["id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    assert_eq!(failed["status"], "failed");
    let (status, _) = call(&app, "GET", "/v2/agent/missing", Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&app, "POST", "/v2/agent", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        state(&w)["agent_jobs"].as_object().unwrap().len(),
        3,
        "three jobs stored"
    );
}

// ───────────── research / developer / gov ─────────────

#[tokio::test]
async fn research_paper_endpoints() {
    let w = world();
    let app = app(&w);
    let (_, hits) = call(
        &app,
        "GET",
        "/v2/search/research/papers?query=transformer+attention&authors=vaswani",
        Value::Null,
    )
    .await;
    assert_eq!(hits["results"].as_array().unwrap().len(), 1);
    assert_eq!(hits["results"][0]["title"], "Attention Is All You Need");
    assert!(hits["results"][0].get("passages").is_none());

    let (_, paper) = call(
        &app,
        "GET",
        "/v2/search/research/papers/doi%3A10.48550%2FarXiv.1706.03762",
        Value::Null,
    )
    .await;
    assert_eq!(paper["paper"]["paperId"], "arxiv:1706.03762");
    let (_, passages) = call(
        &app,
        "GET",
        "/v2/search/research/papers/arxiv%3A1706.03762?query=how+many+attention+heads&k=1",
        Value::Null,
    )
    .await;
    assert_eq!(
        passages["passages"],
        json!([{"text": "The Transformer uses multi-head attention with h = 8 heads.", "section": "Model"}])
    );
    let (_, similar) = call(
        &app,
        "GET",
        "/v2/search/research/papers/arxiv%3A1706.03762/similar?intent=bidirectional&mode=citers",
        Value::Null,
    )
    .await;
    assert_eq!(similar["results"][0]["paperId"], "arxiv:1810.04805");
    assert_eq!(similar["poolSize"], 25);
    let (status, _) = call(
        &app,
        "GET",
        "/v2/search/research/papers/arxiv%3A0000.00000",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn developer_and_gov_indexes() {
    let w = world();
    let app = app(&w);
    let (_, dev) = call(
        &app,
        "GET",
        "/v2/search/developer?query=retry+429+rate+limit&k=5",
        Value::Null,
    )
    .await;
    assert_eq!(dev["results"][0]["id"], "issue:acme/sdk#12");
    assert!(dev["results"][0].get("keywords").is_none());
    let (_, gov) = call(
        &app,
        "GET",
        "/v2/search/gov?query=privacy&jurisdiction=us",
        Value::Null,
    )
    .await;
    assert_eq!(
        gov["data"]["web"],
        json!([{"url": "https://www.law.cornell.edu/uscode/text/15/6501", "title": "15 U.S. Code 6501 - Definitions (COPPA)", "description": "Children's online privacy protection definitions.", "position": 1}])
    );
    let (status, _) = call(&app, "GET", "/v2/search/gov", Value::Null).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ───────────── credits ─────────────

#[tokio::test]
async fn credit_usage_current_and_historical() {
    let w = world();
    let app = app(&w);
    let (_, current) = call(&app, "GET", "/v2/team/credit-usage", Value::Null).await;
    assert_eq!(current["data"]["remainingCredits"], 41250);
    assert_eq!(current["data"]["planCredits"], 50000);
    let (_, hist) = call(&app, "GET", "/v2/team/credit-usage/historical", Value::Null).await;
    assert_eq!(hist["periods"][1]["creditsUsed"], 8750);
    let (_, by_endpoint) = call(
        &app,
        "GET",
        "/v2/team/credit-usage/historical?byEndpoint=true&bin=month",
        Value::Null,
    )
    .await;
    assert_eq!(by_endpoint["bin"], "month");
    assert_eq!(by_endpoint["periods"].as_array().unwrap().len(), 4);
    assert_eq!(by_endpoint["periods"][0]["endpoint"], "scrape");
}

// ───────────── monitors ─────────────

#[tokio::test]
async fn monitor_crud_and_validation() {
    let w = world();
    let app = app(&w);
    let (status, body) = call(&app, "POST", "/v2/monitor", json!({"name": "x"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("targets"));
    let (status, _) = call(
        &app,
        "POST",
        "/v2/monitor",
        json!({"targets": [{"type": "scrape", "urls": ["https://acme.dev/pricing"]}], "schedule": {"text": "whenever"}}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, created) = call(
        &app,
        "POST",
        "/v2/monitor",
        json!({
            "name": "Acme docs",
            "schedule": {"text": "every day at 9am", "timezone": "UTC"},
            "goal": "Track docs changes",
            "targets": [{"type": "scrape", "urls": ["https://acme.dev/docs/page-01"]}],
            "notification": {"email": {"enabled": true, "recipients": ["a@b.example"]}}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let m = &created["data"];
    let id = m["id"].as_str().unwrap().to_owned();
    assert_eq!(m["status"], "active");
    assert_eq!(m["schedule"]["intervalMinutes"], 1440);
    // Two rejected POSTs ticked the clock before this one.
    assert_eq!(m["createdAt"], "2026-10-01T12:00:21Z");
    assert_eq!(m["nextRunAt"], "2026-10-02T12:00:21Z");
    assert_eq!(state(&w)["monitors"][&id]["name"], "Acme docs");

    let (_, list) = call(&app, "GET", "/v2/monitor?limit=2&offset=1", Value::Null).await;
    assert_eq!(list["pagination"]["total"], 3);
    assert_eq!(list["pagination"]["hasMore"], false);
    assert_eq!(list["data"][0]["id"], "m-2");
    assert_eq!(list["data"][1]["id"], id);

    let (_, paused) = call(
        &app,
        "PATCH",
        &format!("/v2/monitor/{id}"),
        json!({"active": false, "name": "Acme docs (paused)"}),
    )
    .await;
    assert_eq!(paused["data"]["status"], "paused");
    assert_eq!(paused["data"]["nextRunAt"], Value::Null);
    assert_eq!(state(&w)["monitors"][&id]["name"], "Acme docs (paused)");
    let (status, _) = call(
        &app,
        "PATCH",
        "/v2/monitor/m-1",
        json!({"status": "sleeping"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (_, deleted) = call(&app, "DELETE", &format!("/v2/monitor/{id}"), Value::Null).await;
    assert_eq!(deleted["data"], json!({"id": id, "deleted": true}));
    assert!(state(&w)["monitors"].get(&id).is_none());
    for (method, uri) in [
        ("GET", format!("/v2/monitor/{id}")),
        ("DELETE", format!("/v2/monitor/{id}")),
        ("POST", format!("/v2/monitor/{id}/run")),
        ("GET", format!("/v2/monitor/{id}/checks")),
    ] {
        let (status, body) = call(&app, method, &uri, Value::Null).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
        assert_eq!(body["error"], "Monitor not found");
    }
}

#[tokio::test]
async fn monitor_runs_and_checks() {
    let w = world();
    let app = app(&w);
    let (_, run) = call(&app, "POST", "/v2/monitor/m-1/run", Value::Null).await;
    assert_eq!(run["data"]["status"], "queued");
    let check_id = run["data"]["id"].as_str().unwrap().to_owned();
    let s = state(&w);
    assert_eq!(s["monitor_checks"][&check_id]["status"], "completed");
    assert_eq!(s["monitors"]["m-1"]["lastCheckId"], check_id);

    let (_, checks) = call(&app, "GET", "/v2/monitor/m-1/checks", Value::Null).await;
    let ids: Vec<&str> = checks["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [check_id.as_str(), "c-2", "c-1"]);
    assert!(checks["data"][0].get("pages").is_none());
    let (_, partial) = call(
        &app,
        "GET",
        "/v2/monitor/m-1/checks?status=partial",
        Value::Null,
    )
    .await;
    assert_eq!(partial["pagination"]["total"], 1);

    let (_, check) = call(
        &app,
        "GET",
        "/v2/monitor/m-1/checks/c-2?limit=1",
        Value::Null,
    )
    .await;
    assert_eq!(check["data"]["pages"][0]["status"], "changed");
    assert_eq!(
        check["next"],
        "http://world:8102/v2/monitor/m-1/checks/c-2?skip=1&limit=1"
    );
    let (_, errors) = call(
        &app,
        "GET",
        "/v2/monitor/m-1/checks/c-2?status=error",
        Value::Null,
    )
    .await;
    assert_eq!(errors["data"]["pages"][0]["url"], "https://acme.dev/old");
    assert_eq!(errors["next"], Value::Null);
    let (status, body) = call(&app, "GET", "/v2/monitor/m-2/checks/c-2", Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "Check not found");
}

// ───────────── parse ─────────────

#[tokio::test]
async fn parses_uploaded_html() {
    let w = world();
    let app = app(&w);
    let boundary = "XyZ";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"report.html\"\r\nContent-Type: text/html\r\n\r\n<html><head><title>t</title></head><body><h1>Q3 report</h1><p>Revenue grew &amp; churn fell.</p><ul><li>One</li></ul></body></html>\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"options\"\r\n\r\n{{\"formats\":[\"markdown\"]}}\r\n--{boundary}--\r\n"
    );
    let (status, _, parsed) = send(
        &app,
        "POST",
        "/v2/parse",
        Body::from(body),
        &format!("multipart/form-data; boundary={boundary}"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        parsed["data"]["markdown"],
        "# Q3 report\n\nRevenue grew & churn fell.\n\n- One"
    );
    assert_eq!(parsed["data"]["metadata"]["title"], "Q3 report");
    assert_eq!(parsed["data"]["metadata"]["sourceURL"], "report.html");

    let (status, _) = call(&app, "POST", "/v2/parse", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, upload) = call(
        &app,
        "POST",
        "/v2/parse/upload-url",
        json!({"filename": "a.pdf", "contentType": "application/pdf"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(upload["data"]["method"], "PUT");
}

// ───────────── determinism ─────────────

#[tokio::test]
async fn same_requests_give_identical_responses() {
    async fn run() -> Vec<Value> {
        let w = world();
        let app = app(&w);
        let mut out = Vec::new();
        for (method, uri, body) in [
            ("POST", "/v2/search", json!({"query": "acme pricing"})),
            (
                "POST",
                "/v2/scrape",
                json!({"url": "https://acme.dev/pricing"}),
            ),
            ("POST", "/v2/crawl", json!({"url": "https://acme.dev/docs"})),
            (
                "POST",
                "/v2/monitor",
                json!({"targets": [{"url": "https://acme.dev"}], "schedule": "hourly"}),
            ),
        ] {
            out.push(call(&app, method, uri, body).await.1);
        }
        out.push(state(&w));
        out
    }
    assert_eq!(run().await, run().await);
}

#[tokio::test]
async fn scenario_datasets_load_and_answer() {
    let w = world_at("../../../scenarios/search/firecrawl/northwind-team-pricing");
    let app = app(&w);
    let (_, found) = call(
        &app,
        "POST",
        "/v2/search",
        json!({"query": "Northwind Analytics pricing Team plan"}),
    )
    .await;
    assert_eq!(
        found["data"]["web"][0]["url"],
        "https://www.northwind-analytics.com/pricing"
    );
}
