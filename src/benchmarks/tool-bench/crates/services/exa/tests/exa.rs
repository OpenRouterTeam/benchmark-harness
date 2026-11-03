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
use mock_exa::Exa;
use serde_json::{Value, json};
use tower::ServiceExt;

fn world() -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture");
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Value,
) -> (StatusCode, HeaderMap, String) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "api.exa.ai")
        .header("content-type", "application/json");
    for (k, v) in headers {
        req = req.header(*k, *v);
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

const KEY: (&str, &str) = ("x-api-key", "exa-toolbench");

async fn call(app: &Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let (s, _, text) = send(app, method, uri, &[KEY], body).await;
    (s, serde_json::from_str(&text).unwrap_or(Value::Null))
}

fn urls(v: &Value) -> Vec<&str> {
    v["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["url"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn missing_api_key_is_401_with_exa_error_shape() {
    let app = service_app(world(), &Exa);
    let (status, _, text) = send(&app, "POST", "/search", &[], json!({"query": "x"})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["tag"], "INVALID_API_KEY");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .starts_with("Invalid API key")
    );
    assert_eq!(body["requestId"].as_str().unwrap().len(), 32);
    // Bearer auth works too.
    let (status, _, _) = send(
        &app,
        "POST",
        "/search",
        &[("authorization", "Bearer abc")],
        json!({"query": "driftwood"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn search_ranks_and_returns_highlights_text_and_cost() {
    let app = service_app(world(), &Exa);
    let (status, body) = call(
        &app,
        "POST",
        "/search",
        json!({"query": "Driftwood Python SDK v4 timeout option", "numResults": 2, "contents": {"highlights": true}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["resolvedSearchType"], "neural");
    assert_eq!(
        urls(&body),
        [
            "https://docs.driftwood.dev/python/v4/configuration",
            "https://docs.driftwood.dev/python/v3/configuration"
        ]
    );
    let first = &body["results"][0];
    assert_eq!(first["id"], first["url"]);
    assert_eq!(first["publishedDate"], "2026-08-19T00:00:00.000Z");
    assert!(first.get("text").is_none());
    let highlights = first["highlights"].as_array().unwrap();
    assert!(!highlights.is_empty() && highlights.len() <= 3);
    assert_eq!(
        first["highlightScores"].as_array().unwrap().len(),
        highlights.len()
    );
    assert_eq!(body["costDollars"]["total"], 0.007);

    let (_, body) = call(
        &app,
        "POST",
        "/search",
        json!({"query": "driftwood timeout", "type": "fast", "contents": {"text": {"maxCharacters": 20}, "summary": true}}),
    )
    .await;
    assert_eq!(body["resolvedSearchType"], "fast");
    assert_eq!(
        body["results"][0]["text"].as_str().unwrap().chars().count(),
        20
    );
    assert!(body["results"][0]["summary"].is_string());
}

#[tokio::test]
async fn search_filters() {
    let app = service_app(world(), &Exa);
    let (_, body) = call(
        &app,
        "POST",
        "/search",
        json!({"query": "driftwood sdk", "category": "github"}),
    )
    .await;
    assert_eq!(
        urls(&body),
        ["https://github.com/driftwood-io/driftwood-python"]
    );
    let (_, body) = call(
        &app,
        "POST",
        "/search",
        json!({"query": "driftwood timeout", "includeDomains": ["stackoverflow.com"]}),
    )
    .await;
    assert_eq!(urls(&body).len(), 1);
    let (_, body) = call(
        &app,
        "POST",
        "/search",
        json!({"query": "driftwood timeout", "excludeDomains": ["driftwood.dev", "github.com"], "startPublishedDate": "2024-06-01"}),
    )
    .await;
    assert_eq!(
        urls(&body),
        ["https://stackoverflow.com/questions/78123456/driftwood-python-client-times-out"]
    );
    let (_, body) = call(
        &app,
        "POST",
        "/search",
        json!({"query": "driftwood configuration", "includeText": ["timeoutMs"]}),
    )
    .await;
    assert_eq!(
        urls(&body),
        ["https://docs.driftwood.dev/node/v4/configuration"]
    );
    let (_, body) = call(
        &app,
        "POST",
        "/search",
        json!({"query": "driftwood configuration", "excludeText": ["legacy"], "endPublishedDate": "2025-01-01"}),
    )
    .await;
    assert_eq!(
        urls(&body),
        ["https://stackoverflow.com/questions/78123456/driftwood-python-client-times-out"]
    );
}

#[tokio::test]
async fn unknown_query_is_empty_and_bad_input_is_400() {
    let app = service_app(world(), &Exa);
    let (status, body) = call(&app, "POST", "/search", json!({"query": "zebra migration"})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["results"], json!([]));
    let (status, body) = call(&app, "POST", "/search", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["tag"], "INVALID_REQUEST_BODY");
    let (status, _) = call(
        &app,
        "POST",
        "/search",
        json!({"query": "x", "numResults": 500}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        &app,
        "POST",
        "/search",
        json!({"query": "x", "type": "magic"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn deep_search_answers_with_citations_and_structured_output() {
    let app = service_app(world(), &Exa);
    let (_, body) = call(
        &app,
        "POST",
        "/search",
        json!({"query": "driftwood v4 timeout default", "type": "deep"}),
    )
    .await;
    assert_eq!(body["resolvedSearchType"], "deep");
    assert_eq!(
        body["output"]["content"],
        "In Driftwood Python SDK v4, use timeout_seconds (default 30 seconds)."
    );
    assert_eq!(
        body["output"]["grounding"][0]["citations"][0]["url"],
        "https://docs.driftwood.dev/python/v4/configuration"
    );
    let (_, body) = call(
        &app,
        "POST",
        "/search",
        json!({"query": "driftwood v4 timeout default", "type": "deep", "outputSchema": {"type": "object"}}),
    )
    .await;
    assert_eq!(body["output"]["content"]["defaultSeconds"], 30);
    // No canned answer: synthesized from the hits.
    let (_, body) = call(
        &app,
        "POST",
        "/search",
        json!({"query": "driftwood node sdk configuration", "type": "deep-reasoning"}),
    )
    .await;
    assert!(
        body["output"]["content"]
            .as_str()
            .unwrap()
            .contains("Node.js SDK v4")
    );
}

#[tokio::test]
async fn context_and_subpages() {
    let app = service_app(world(), &Exa);
    let (_, body) = call(
        &app,
        "POST",
        "/search",
        json!({"query": "driftwood-python github", "category": "github", "contents": {"text": true, "context": {"maxCharacters": 50}, "subpages": 1, "subpageTarget": ["node"]}}),
    )
    .await;
    assert_eq!(body["context"].as_str().unwrap().chars().count(), 50);
    let subs = body["results"][0]["subpages"].as_array().unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(
        subs[0]["url"],
        "https://docs.driftwood.dev/node/v4/configuration"
    );
    assert_eq!(
        body["results"][0]["image"],
        "https://opengraph.githubassets.com/driftwood-python.png"
    );
}

#[tokio::test]
async fn contents_returns_text_and_per_url_statuses() {
    let app = service_app(world(), &Exa);
    let (status, body) = call(
        &app,
        "POST",
        "/contents",
        json!({"urls": ["https://docs.driftwood.dev/python/v4/configuration/", "https://nope.example.com/x"], "text": {"maxCharacters": 40}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["results"].as_array().unwrap().len(), 1);
    assert_eq!(
        body["results"][0]["id"],
        "https://docs.driftwood.dev/python/v4/configuration/"
    );
    assert_eq!(
        body["results"][0]["text"].as_str().unwrap().chars().count(),
        40
    );
    assert_eq!(
        body["statuses"],
        json!([
            {"id": "https://docs.driftwood.dev/python/v4/configuration/", "status": "success", "source": "cached"},
            {"id": "https://nope.example.com/x", "status": "error", "error": {"tag": "CRAWL_NOT_FOUND", "httpStatusCode": 404}, "source": "crawl"}
        ])
    );
    let (status, _) = call(&app, "POST", "/contents", json!({"urls": []})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn research_tasks_are_stored_and_completed() {
    let w = world();
    let app = service_app(w.clone(), &Exa);
    let (status, created) = call(
        &app,
        "POST",
        "/research/v1",
        json!({"instructions": "Bidirectional depot chargers in Germany: vendors and prices", "model": "exa-research-pro", "outputSchema": {"type": "object"}}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["status"], "pending");
    let id = created["researchId"].as_str().unwrap().to_owned();
    assert!(id.starts_with("r_01k"));
    assert_eq!(created["createdAt"], 1_793_181_607_000_i64);

    let (status, done) = call(&app, "GET", &format!("/research/v1/{id}"), Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(done["status"], "completed");
    assert!(
        done["output"]["content"]
            .as_str()
            .unwrap()
            .contains("VoltHaven")
    );
    assert_eq!(done["output"]["parsed"]["leaderPriceEur"], 6900);
    assert_eq!(done["costDollars"]["total"], 0.31);

    let state = w.lock().unwrap().snapshot();
    assert_eq!(
        state["services"]["exa"]["research"][&id]["model"],
        "exa-research-pro"
    );

    let (_, list) = call(&app, "GET", "/research/v1?limit=5", Value::Null).await;
    assert_eq!(list["data"][0]["researchId"], id.as_str());
    assert_eq!(list["hasMore"], false);

    let (status, body) = call(&app, "GET", "/research/v1/r_missing", Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["tag"], "NOT_FOUND");
    let (status, _) = call(
        &app,
        "POST",
        "/research/v1",
        json!({"instructions": "x", "model": "gpt"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        &app,
        "POST",
        "/research/v1",
        json!({"model": "exa-research"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn agent_runs_stream_events_and_persist() {
    let w = world();
    let app = service_app(w.clone(), &Exa);
    let body = json!({
        "query": "Find HQ city and headcount",
        "input": {"data": [{"companyName": "Corvane Robotics"}, {"companyName": "Lumetric Health"}, {"companyName": "Bramblewood Foods"}]},
        "effort": "low"
    });
    let (status, headers, text) = send(
        &app,
        "POST",
        "/agent/runs",
        &[KEY, ("accept", "text/event-stream")],
        body,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "text/event-stream");
    let events: Vec<&str> = text
        .lines()
        .filter_map(|l| l.strip_prefix("event: "))
        .collect();
    assert_eq!(
        events,
        [
            "agent_run.created",
            "agent_run.started",
            "agent_run.completed"
        ]
    );
    let last = text
        .lines()
        .rev()
        .find_map(|l| l.strip_prefix("data: "))
        .unwrap();
    let run: Value = serde_json::from_str(last).unwrap();
    let id = run["id"].as_str().unwrap().to_owned();
    assert!(id.starts_with("agent_run_"));
    assert_eq!(
        run["output"]["structured"]["companies"][2]["hqCity"],
        "Leeds"
    );

    let (_, got) = call(&app, "GET", &format!("/agent/runs/{id}"), Value::Null).await;
    assert_eq!(got, run);
    let (_, events) = call(
        &app,
        "GET",
        &format!("/agent/runs/{id}/events"),
        Value::Null,
    )
    .await;
    assert_eq!(events["data"].as_array().unwrap().len(), 3);
    let (status, _) = call(
        &app,
        "POST",
        &format!("/agent/runs/{id}/cancel"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Non-streaming create returns the queued run.
    let (status, queued) = call(
        &app,
        "POST",
        "/agent/runs",
        json!({"query": "Corvane Robotics headcount"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(queued["status"], "queued");
    let id2 = queued["id"].as_str().unwrap().to_owned();
    let (_, list) = call(&app, "GET", "/agent/runs?limit=1", Value::Null).await;
    assert_eq!(list["data"][0]["id"], id2.as_str());
    assert_eq!(list["hasMore"], true);
    assert_eq!(list["nextCursor"], "1");

    let state = w.lock().unwrap().snapshot();
    assert_eq!(
        state["services"]["exa"]["agent_runs"][&id]["input"]["data"][0]["companyName"],
        "Corvane Robotics"
    );

    let (status, deleted) = call(&app, "DELETE", &format!("/agent/runs/{id2}"), Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(deleted["deleted"], true);
    let (status, _) = call(&app, "GET", &format!("/agent/runs/{id2}"), Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        &app,
        "POST",
        "/agent/runs",
        json!({"query": "x", "effort": "max"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        &app,
        "POST",
        "/agent/runs",
        json!({"query": "x", "previousRunId": "agent_run_nope"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_routes_are_404() {
    let app = service_app(world(), &Exa);
    let (status, body) = call(&app, "POST", "/websets/v0/websets", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["tag"], "NOT_FOUND");
}
