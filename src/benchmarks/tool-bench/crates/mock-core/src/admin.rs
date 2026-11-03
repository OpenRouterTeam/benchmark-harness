use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use serde_json::{Value, json};

use crate::world::SharedWorld;

/// Benchmark control plane, bound only on the admin port.
pub fn admin_app(world: SharedWorld) -> Router {
    Router::new()
        .route("/_bench/health", get(health))
        .route("/_bench/journal", get(journal))
        .route("/_bench/state", get(state))
        .route("/_bench/reset", post(reset))
        .with_state(world)
}

async fn health(State(world): State<SharedWorld>) -> Json<Value> {
    let scenario = world.lock().expect("world lock").meta.id.clone();
    Json(json!({ "status": "ok", "scenario": scenario }))
}

async fn journal(State(world): State<SharedWorld>) -> Json<Value> {
    let world = world.lock().expect("world lock");
    Json(json!({ "entries": world.journal.entries() }))
}

async fn state(State(world): State<SharedWorld>) -> Json<Value> {
    Json(world.lock().expect("world lock").snapshot())
}

async fn reset(State(world): State<SharedWorld>) -> Json<Value> {
    world.lock().expect("world lock").reset();
    Json(json!({ "status": "reset" }))
}
