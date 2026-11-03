//! Shared test helpers: load a fixture world and call the mock in-process.
#![allow(dead_code)]

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
use mock_gitlab::Gitlab;
use serde_json::Value;
use tower::ServiceExt;

/// World loaded from `tests/fixtures/<name>`.
pub fn world(fixture: &str) -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(fixture);
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

pub fn app(world: &SharedWorld) -> Router {
    service_app(world.clone(), &Gitlab)
}

pub struct Resp {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub text: String,
    pub json: Value,
}

pub async fn call(app: &Router, method: &str, uri: &str, body: &str) -> Resp {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "world:8107")
        .header("content-type", "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = to_bytes(body, usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Resp {
        status: parts.status,
        headers: parts.headers,
        json: serde_json::from_str(&text).unwrap_or(Value::Null),
        text,
    }
}

pub async fn get(app: &Router, uri: &str) -> Resp {
    call(app, "GET", uri, "").await
}

/// The `gitlab` service's state snapshot.
pub fn state(world: &SharedWorld) -> Value {
    world.lock().unwrap().snapshot()["services"]["gitlab"].clone()
}
