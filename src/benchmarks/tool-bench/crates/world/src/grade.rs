//! Verifier endpoints. The task's `tests/test.sh` posts the agent's answer, then asks the
//! world to grade itself against `tests/expect.json`; the reply is Harbor's `reward.json`.

use std::collections::HashMap;

use axum::{
    Json, Router,
    extract::{Query, State},
    routing::post,
};
use mock_core::{
    grading::{Expect, GradeInput, grade},
    world::SharedWorld,
};
use serde_json::{Value, json};

/// Compose service name of the tools container; its IPs are the legitimate API callers.
pub const TOOLS_HOST: &str = "tools";

pub fn routes(world: SharedWorld) -> Router {
    Router::new()
        .route("/_bench/answer", post(answer))
        .route("/_bench/grade", post(grade_trial))
        .with_state(world)
}

async fn answer(State(world): State<SharedWorld>, body: String) -> Json<Value> {
    world.lock().expect("world lock").answer = Some(body);
    Json(json!({ "status": "stored" }))
}

/// Query: `context_overflow=1` when the agent logs show a length rejection;
/// `tools_host` overrides [`TOOLS_HOST`] (tests).
async fn grade_trial(
    State(world): State<SharedWorld>,
    Query(query): Query<HashMap<String, String>>,
    Json(expect): Json<Expect>,
) -> Json<Value> {
    let host = query.get("tools_host").map_or(TOOLS_HOST, String::as_str);
    let tools_ips = match tokio::net::lookup_host((host, 0)).await {
        Ok(addrs) => addrs.map(|a| a.ip().to_string()).collect(),
        Err(_) => Vec::new(),
    };
    let world = world.lock().expect("world lock");
    let result = grade(&GradeInput {
        expect: &expect,
        state: world.snapshot(),
        journal: world.journal.entries(),
        answer: world.answer.as_deref(),
        tools_ips,
        context_overflow: query.get("context_overflow").is_some_and(|v| v == "1"),
    });
    Json(json!(result.metrics()))
}
