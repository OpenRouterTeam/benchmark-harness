use std::path::Path;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode},
};
use mock_brave::Brave;
use mock_core::{
    dataset::Dataset,
    serve::service_app,
    world::{WorldState, shared},
};
use serde_json::{Value, json};
use tower::ServiceExt;

fn app() -> Router {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture");
    let world = shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap());
    service_app(world, &Brave)
}

async fn get_with(app: &Router, uri: &str, token: Option<&str>) -> (StatusCode, HeaderMap, Value) {
    let mut req = Request::builder()
        .uri(uri)
        .header("host", "api.search.brave.com");
    if let Some(t) = token {
        req = req.header("x-subscription-token", t);
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = to_bytes(body, usize::MAX).await.unwrap();
    (
        parts.status,
        parts.headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
    let (s, _, v) = get_with(app, uri, Some("BSA-toolbench")).await;
    (s, v)
}

fn urls(list: &Value) -> Vec<&str> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|r| r["url"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn token_is_required_and_checked() {
    let app = app();
    let (status, _, body) = get_with(&app, "/res/v1/web/search?q=sla", None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["type"], "ErrorResponse");
    assert_eq!(body["error"]["code"], "VALIDATION");
    assert_eq!(
        body["error"]["meta"]["errors"][0]["loc"],
        json!(["header", "x-subscription-token"])
    );
    let (status, _, body) = get_with(&app, "/res/v1/web/search?q=sla", Some("nope")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "SUBSCRIPTION_TOKEN_INVALID");
}

#[tokio::test]
async fn web_search_shape_ranking_and_rate_limit_headers() {
    let app = app();
    let (status, headers, body) = get_with(
        &app,
        "/res/v1/web/search?q=Ironbark+Hosting+SLA+uptime&result_filter=web,query",
        Some("BSA-x"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.contains_key("x-ratelimit-remaining"));
    assert_eq!(body["type"], "search");
    assert_eq!(body["query"]["original"], "Ironbark Hosting SLA uptime");
    let results = &body["web"]["results"];
    assert_eq!(
        results[0]["url"],
        "https://www.ironbarkhosting.com/legal/sla"
    );
    assert_eq!(results[0]["type"], "search_result");
    assert_eq!(results[0]["age"], "July 1, 2026");
    assert_eq!(results[0]["page_age"], "2026-07-01T00:00:00");
    assert_eq!(results[0]["meta_url"]["path"], "› legal › sla");
    assert!(results[0].get("extra_snippets").is_none());
    assert_eq!(
        body["mixed"]["main"][0],
        json!({"type": "web", "index": 0, "all": false})
    );
    assert!(body.get("news").is_none(), "news not in result_filter");
    assert!(body.get("summarizer").is_none());
}

#[tokio::test]
async fn web_search_paging_snippets_freshness_and_site() {
    let app = app();
    let (_, body) = get(&app, "/res/v1/web/search?q=ironbark&count=2&offset=1").await;
    assert_eq!(body["web"]["results"].as_array().unwrap().len(), 2);
    assert_eq!(body["query"]["more_results_available"], true);
    let (_, body) = get(&app, "/res/v1/web/search?q=ironbark&count=2&offset=2").await;
    assert_eq!(body["web"]["results"].as_array().unwrap().len(), 1);
    assert_eq!(body["query"]["more_results_available"], false);

    let (_, body) = get(
        &app,
        "/res/v1/web/search?q=ironbark+sla&extra_snippets=true&count=1",
    )
    .await;
    assert_eq!(
        body["web"]["results"][0]["extra_snippets"]
            .as_array()
            .unwrap()
            .len(),
        3
    );

    let (_, body) = get(
        &app,
        "/res/v1/web/search?q=sla&freshness=2024-01-01to2024-12-31",
    )
    .await;
    assert_eq!(
        urls(&body["web"]["results"]),
        ["https://www.ironbarkhosting.com/legal/sla-2024"]
    );
    let (_, body) = get(&app, "/res/v1/web/search?q=sla+site%3Aironbankhosting.net").await;
    assert_eq!(
        urls(&body["web"]["results"]),
        ["https://www.ironbankhosting.net/sla"]
    );

    let (status, body) = get(&app, "/res/v1/web/search?q=sla&count=50").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body["error"]["meta"]["errors"][0]["loc"],
        json!(["query", "count"])
    );
    let (status, _) = get(&app, "/res/v1/web/search?q=sla&freshness=lastweek").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, body) = get(&app, "/res/v1/web/search").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body["error"]["meta"]["errors"][0]["loc"],
        json!(["query", "q"])
    );
}

#[tokio::test]
async fn web_search_mixes_news_videos_locations_and_summarizer_key() {
    let app = app();
    let (_, body) = get(&app, "/res/v1/web/search?q=norvik+analytics+series").await;
    assert_eq!(body["web"]["results"], json!([]));
    assert_eq!(
        body["news"]["results"][0]["url"],
        "https://www.techbeat.io/2026/10/27/norvik-analytics-series-b"
    );
    let (_, body) = get(
        &app,
        "/res/v1/web/search?q=catering+portland&result_filter=web,locations",
    )
    .await;
    assert_eq!(body["locations"]["results"][0]["type"], "location_result");
    assert_eq!(
        body["locations"]["results"][0]["postal_address"]["type"],
        "PostalAddress"
    );
    let (_, body) = get(
        &app,
        "/res/v1/web/search?q=postgres+keynote&result_filter=videos",
    )
    .await;
    assert_eq!(body["videos"]["results"][0]["type"], "video_result");
    assert!(body.get("web").is_none());

    let (_, body) = get(&app, "/res/v1/web/search?q=ironbark+sla&summary=true").await;
    let key: Value = serde_json::from_str(body["summarizer"]["key"].as_str().unwrap()).unwrap();
    assert_eq!(key["query"], "ironbark sla");
    let (_, body) = get(&app, "/res/v1/web/search?q=zzqx+nothing&summary=true").await;
    assert!(body.get("summarizer").is_none());
}

#[tokio::test]
async fn news_have_relative_ages_and_freshness() {
    let app = app();
    let (status, body) = get(&app, "/res/v1/news/search?q=norvik&freshness=pw").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["type"], "news");
    let results = body["results"].as_array().unwrap();
    let ages: Vec<&str> = results.iter().map(|r| r["age"].as_str().unwrap()).collect();
    assert_eq!(ages, ["2 days ago", "22 hours ago"]);
    let (_, month) = get(&app, "/res/v1/news/search?q=norvik&freshness=pm").await;
    let ages: Vec<&str> = month["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["age"].as_str().unwrap())
        .collect();
    assert_eq!(ages, ["2 days ago", "1 week ago", "22 hours ago"]);
    assert_eq!(results[0]["source"], "TechBeat");
    assert_eq!(
        results[0]["thumbnail"]["src"],
        "https://imgs.search.brave.com/news/techbeat-norvik.jpg"
    );
    assert!(results[0].get("extra_snippets").is_none());
    let (_, body) = get(
        &app,
        "/res/v1/news/search?q=norvik+series&extra_snippets=true&count=1",
    )
    .await;
    assert_eq!(
        body["results"][0]["extra_snippets"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let (_, body) = get(&app, "/res/v1/news/search?q=zzqx").await;
    assert_eq!(body["results"], json!([]));
}

#[tokio::test]
async fn videos_and_images() {
    let app = app();
    let (_, body) = get(&app, "/res/v1/videos/search?q=Lindqvist+keynote+PGConf").await;
    assert_eq!(body["results"][0]["video"]["duration"], "47:12");
    assert_eq!(body["results"][0]["video"]["creator"], "PGConf Nordic");
    assert_eq!(body["extra"], json!({"might_be_offensive": false}));

    let (_, body) = get(&app, "/res/v1/images/search?q=ironbark+logo&count=1").await;
    let img = &body["results"][0];
    assert_eq!(body["results"].as_array().unwrap().len(), 1);
    assert_eq!(
        img["properties"]["url"],
        "https://www.ironbarkhosting.com/press/ironbark-logo.png"
    );
    assert_eq!(img["properties"]["width"], 1600);
    assert_eq!(img["page_fetched"], "2026-09-02T10:11:12Z");
    assert_eq!(img["confidence"], "high");
}

#[tokio::test]
async fn summarizer_canned_generated_and_missing() {
    let app = app();
    let key = r#"{"query":"ironbark sla","country":"us"}"#;
    let (status, body) = get(
        &app,
        &format!(
            "/res/v1/summarizer/search?key={}&inline_references=true",
            urlencode(key)
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "complete");
    assert_eq!(body["summary"][0]["type"], "token");
    assert!(
        body["summary"][0]["data"]
            .as_str()
            .unwrap()
            .contains("99.95%")
    );
    assert_eq!(
        body["summary"][1]["data"]["url"],
        "https://www.ironbarkhosting.com/legal/sla"
    );

    let key = r#"{"query":"ironbark pricing plans"}"#;
    let (_, body) = get(
        &app,
        &format!("/res/v1/summarizer/search?key={}", urlencode(key)),
    )
    .await;
    assert!(
        body["summary"][0]["data"]
            .as_str()
            .unwrap()
            .contains("Business $59/month")
    );
    assert_eq!(body["summary"].as_array().unwrap().len(), 1);

    let (status, body) = get(&app, "/res/v1/summarizer/search?key=garbage").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "RESOURCE_NOT_FOUND");
}

#[tokio::test]
async fn llm_context_grounding_and_sources() {
    let app = app();
    let (_, body) = get(
        &app,
        "/res/v1/llm/context?q=Ironbark+Business+SLA&maximum_number_of_urls=1&maximum_number_of_snippets_per_url=2",
    )
    .await;
    let generic = body["grounding"]["generic"].as_array().unwrap();
    assert_eq!(generic.len(), 1);
    assert_eq!(
        generic[0]["snippets"],
        json!([
            "# Service Level Agreement",
            "Effective July 1, 2026. Supersedes all previous versions."
        ])
    );
    let source = &body["sources"]["https://www.ironbarkhosting.com/legal/sla"];
    assert_eq!(source["hostname"], "ironbarkhosting.com");
    assert_eq!(source["age"], json!(["July 1, 2026", "2026-07-01"]));
    assert_eq!(body["grounding"]["map"], json!([]));
    assert_eq!(body["grounding"]["poi"], Value::Null);

    let (_, body) = get(
        &app,
        "/res/v1/llm/context?q=juniper+catering&enable_local=true",
    )
    .await;
    assert_eq!(
        body["grounding"]["map"][0]["name"],
        "Juniper & Rye Catering"
    );
    assert_eq!(body["grounding"]["poi"]["url"], "https://juniperandrye.com");
}

#[tokio::test]
async fn local_pois_and_descriptions_by_repeated_ids() {
    let app = app();
    let a = urlencode("loc4CQWMJWLD4VBEBZ62XQLJTGK6YCJEEJDNA3BTZZQ=");
    let b = urlencode("locXR7ZK2M5Q4WTJ3H6YB8D1NFVEUCSAGLPO0IMKQ8=");
    let (_, body) = get(
        &app,
        &format!("/res/v1/local/descriptions?ids={a}&ids={b}&ids=unknown"),
    )
    .await;
    assert_eq!(body["type"], "local_descriptions");
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert!(
        results[1]["description"]
            .as_str()
            .unwrap()
            .contains("Maine")
    );
    let (_, body) = get(&app, &format!("/res/v1/local/pois?ids={b}")).await;
    assert_eq!(
        body["results"][0]["contact"]["telephone"],
        "+1 207-555-0198"
    );
    let (status, _) = get(&app, "/res/v1/local/pois").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn place_search_by_query_location_and_coordinates() {
    let app = app();
    let (_, body) = get(
        &app,
        "/res/v1/local/place_search?q=catering&location=portland+me+united+states",
    )
    .await;
    assert_eq!(body["type"], "locations");
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["title"], "Juniper & Rye Catering Co.");
    assert_eq!(body["location"]["name"], "portland me united states");

    let (_, body) = get(
        &app,
        "/res/v1/local/place_search?latitude=45.528&longitude=-122.6825&count=2",
    )
    .await;
    assert_eq!(body["results"][0]["title"], "Juniper Rye Bakery");
    assert_eq!(body["results"][0]["distance"]["units"], "km");
    assert_eq!(body["location"]["coordinates"], json!([45.528, -122.6825]));

    let (status, _) = get(&app, "/res/v1/local/place_search").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = get(&app, "/res/v1/local/place_search?latitude=abc&longitude=1").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn unknown_paths_are_404() {
    let app = app();
    let (status, body) = get(&app, "/res/v1/spellcheck/search?q=x").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["type"], "ErrorResponse");
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            b' ' => "+".to_owned(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}
