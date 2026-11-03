use std::path::Path;

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Request, StatusCode},
    response::Response,
    routing::get,
};
use mock_core::{
    admin::admin_app,
    dataset::Dataset,
    http::json,
    serve::service_app,
    service::MockService,
    world::{SharedWorld, WorldState, shared},
};
use serde_json::{Value, json};
use tower::ServiceExt;

/// Test-only service: GET /now returns the logical time, POST /things stores a thing.
struct Things;

impl MockService for Things {
    fn name(&self) -> &'static str {
        "things"
    }
    fn port(&self) -> u16 {
        0
    }
    fn hosts(&self) -> &'static [&'static str] {
        &["things.example"]
    }
    fn router(&self) -> Router<SharedWorld> {
        Router::new()
            .route("/now", get(now))
            .route("/things", get(now).post(create))
    }
}

async fn now(State(world): State<SharedWorld>) -> Response {
    let now = world.lock().unwrap().clock.now_rfc3339();
    json(StatusCode::OK, json!({ "now": now }))
}

async fn create(State(world): State<SharedWorld>, body: String) -> Response {
    let mut w = world.lock().unwrap();
    let now = w.clock.now_rfc3339();
    let svc = w.service_mut("things");
    let id = svc.ids.next_prefixed("thing", "th_", 8);
    let doc = json!({ "id": id, "body": body, "created_at": now });
    svc.store
        .collection_mut("things")
        .insert(id.clone(), doc.clone());
    json(StatusCode::CREATED, doc)
}

fn world() -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios/example/github/basic");
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

async fn call(app: &Router, method: &str, uri: &str, body: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::from(body.to_owned()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn reads_do_not_tick_and_writes_do() {
    let w = world();
    let app = service_app(w.clone(), &Things);
    assert_eq!(
        call(&app, "GET", "/now", "").await.1["now"],
        "2026-03-14T09:00:00Z"
    );
    let (status, created) = call(&app, "POST", "/things", "hello").await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["created_at"], "2026-03-14T09:00:07Z");
    assert_eq!(
        call(&app, "GET", "/now", "").await.1["now"],
        "2026-03-14T09:00:07Z"
    );
}

#[tokio::test]
async fn journals_every_request_in_order() {
    let w = world();
    let app = service_app(w.clone(), &Things);
    call(&app, "GET", "/things?limit=1", "").await;
    call(&app, "POST", "/things", r#"{"name":"a"}"#).await;
    call(&app, "GET", "/missing", "").await;

    let world = w.lock().unwrap();
    let entries = world.journal.entries();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries.iter().map(|e| e.seq).collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(entries[0].query.as_deref(), Some("limit=1"));
    assert_eq!(entries[0].source, "unknown", "oneshot has no peer address");
    assert_eq!(entries[1].method, "POST");
    assert_eq!(entries[1].request_body, json!({"name": "a"}));
    assert_eq!(entries[1].status, 201);
    assert_eq!(entries[1].service, "things");
    assert_eq!(entries[2].status, 404);
}

#[tokio::test]
async fn admin_exposes_health_journal_state_and_reset() {
    let w = world();
    let app = service_app(w.clone(), &Things);
    let admin = admin_app(w.clone());

    assert_eq!(
        call(&admin, "GET", "/_bench/health", "").await.1,
        json!({"status": "ok", "scenario": "example/github/basic"})
    );

    let (_, created) = call(&app, "POST", "/things", "x").await;
    let id = created["id"].as_str().unwrap().to_owned();

    let (_, journal) = call(&admin, "GET", "/_bench/journal", "").await;
    assert_eq!(journal["entries"].as_array().unwrap().len(), 1);

    let (_, state) = call(&admin, "GET", "/_bench/state", "").await;
    assert_eq!(state["services"]["things"]["things"][&id]["body"], "x");
    assert_eq!(state["now"], "2026-03-14T09:00:07Z");

    assert_eq!(
        call(&admin, "POST", "/_bench/reset", "").await.1,
        json!({"status": "reset"})
    );
    let (_, state) = call(&admin, "GET", "/_bench/state", "").await;
    assert!(state["services"].get("things").is_none());
    assert_eq!(state["now"], "2026-03-14T09:00:00Z");
    let (_, journal) = call(&admin, "GET", "/_bench/journal", "").await;
    assert!(journal["entries"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn same_requests_give_identical_responses() {
    async fn run() -> Vec<Value> {
        let app = service_app(world(), &Things);
        let mut out = Vec::new();
        for (m, u, b) in [
            ("POST", "/things", "a"),
            ("POST", "/things", "b"),
            ("GET", "/now", ""),
        ] {
            out.push(call(&app, m, u, b).await.1);
        }
        out
    }
    assert_eq!(run().await, run().await);
}
