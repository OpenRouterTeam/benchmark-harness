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
use mock_github::GitHub;
use serde_json::{Value, json};
use tower::ServiceExt;

fn world() -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../scenarios/example/github/basic");
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

async fn call(app: &Router, method: &str, uri: &str, body: &str) -> (StatusCode, HeaderMap, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "world:8101")
        .header("content-type", "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = to_bytes(body, usize::MAX).await.unwrap();
    (
        parts.status,
        parts.headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn numbers(v: &Value) -> Vec<u64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|i| i["number"].as_u64().unwrap())
        .collect()
}

#[tokio::test]
async fn get_user_returns_viewer() {
    let app = service_app(world(), &GitHub);
    let (status, _, body) = call(&app, "GET", "/api/v3/user", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["login"], "mona");
}

#[tokio::test]
async fn lists_open_issues_newest_first_by_default() {
    let app = service_app(world(), &GitHub);
    let (status, headers, body) = call(&app, "GET", "/api/v3/repos/octo-org/app/issues", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(numbers(&body), [3, 1]);
    assert!(headers.get("link").is_none());
}

#[tokio::test]
async fn filters_by_state_and_paginates() {
    let app = service_app(world(), &GitHub);
    let (_, _, closed) = call(
        &app,
        "GET",
        "/api/v3/repos/octo-org/app/issues?state=closed",
        "",
    )
    .await;
    assert_eq!(numbers(&closed), [2]);

    let (_, headers, page1) = call(
        &app,
        "GET",
        "/api/v3/repos/octo-org/app/issues?state=all&per_page=2",
        "",
    )
    .await;
    assert_eq!(numbers(&page1), [3, 2]);
    assert_eq!(
        headers["link"],
        "<http://world:8101/api/v3/repos/octo-org/app/issues?state=all&page=2&per_page=2>; rel=\"next\", \
         <http://world:8101/api/v3/repos/octo-org/app/issues?state=all&page=2&per_page=2>; rel=\"last\""
    );
}

#[tokio::test]
async fn creates_issue_with_next_number_and_logical_time() {
    let w = world();
    let app = service_app(w.clone(), &GitHub);
    let (status, headers, issue) = call(
        &app,
        "POST",
        "/api/v3/repos/octo-org/app/issues",
        r#"{"title":"Login page returns 500","body":"Seen after deploy 412."}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(issue["number"], 4);
    assert_eq!(issue["id"], 2004);
    assert_eq!(issue["state"], "open");
    assert_eq!(issue["created_at"], "2026-03-14T09:00:07Z");
    assert_eq!(issue["user"]["login"], "mona");
    assert_eq!(
        issue["url"],
        "http://world:8101/api/v3/repos/octo-org/app/issues/4"
    );
    assert_eq!(
        headers["location"],
        "http://world:8101/api/v3/repos/octo-org/app/issues/4"
    );
    assert!(issue["node_id"].as_str().unwrap().starts_with("I_kwDO"));

    let (_, _, open) = call(&app, "GET", "/api/v3/repos/octo-org/app/issues", "").await;
    assert_eq!(numbers(&open), [4, 3, 1]);
    let state = w.lock().unwrap().snapshot();
    assert_eq!(
        state["services"]["github"]["issues"]["octo-org/app#4"]["title"],
        "Login page returns 500"
    );
}

#[tokio::test]
async fn rejects_bad_input_like_github() {
    let app = service_app(world(), &GitHub);
    let (status, _, body) = call(
        &app,
        "POST",
        "/api/v3/repos/octo-org/app/issues",
        "not json",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["message"], "Problems parsing JSON");

    let (status, _, body) = call(
        &app,
        "POST",
        "/api/v3/repos/octo-org/app/issues",
        r#"{"body":"no title"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body["message"],
        "Invalid request.\n\n\"title\" wasn't supplied."
    );
}

#[tokio::test]
async fn unknown_repos_and_routes_are_github_404s() {
    let app = service_app(world(), &GitHub);
    let expected = json!({"message": "Not Found", "documentation_url": "https://docs.github.com/rest", "status": "404"});
    for (method, uri, body) in [
        ("GET", "/api/v3/repos/octo-org/nope/issues", ""),
        (
            "POST",
            "/api/v3/repos/octo-org/nope/issues",
            r#"{"title":"x"}"#,
        ),
        ("GET", "/api/v3/meta/unknown", ""),
    ] {
        let (status, _, got) = call(&app, method, uri, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
        assert_eq!(got, expected, "{method} {uri}");
    }
}
