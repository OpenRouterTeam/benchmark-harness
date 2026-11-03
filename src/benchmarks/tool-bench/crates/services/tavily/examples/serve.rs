//! Serves only the Tavily mock for one scenario, for driving the real MCP server
//! against it during development:
//!
//! ```text
//! cargo run -p mock-tavily --example serve -- scenarios/search/tavily/<slug> [port]
//! ```
//!
//! `GET /_bench/journal` and `/_bench/state` on the same port expose what happened.

use std::path::PathBuf;

use axum::{Json, Router, extract::State, routing::get};
use mock_core::{
    dataset::Dataset,
    serve::{serve, service_app},
    world::{SharedWorld, WorldState, shared},
};
use mock_tavily::Tavily;
use serde_json::{Value, json};

async fn journal(State(world): State<SharedWorld>) -> Json<Value> {
    Json(json!({ "entries": world.lock().expect("world lock").journal.entries() }))
}

async fn state(State(world): State<SharedWorld>) -> Json<Value> {
    Json(world.lock().expect("world lock").snapshot())
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("usage: serve <scenario-dir> [port]"));
    let port: u16 = args.next().map_or(18105, |p| p.parse().expect("port"));
    let world = shared(
        WorldState::from_dataset(Dataset::load(&dir).expect("dataset")).expect("world state"),
    );
    let admin = Router::new()
        .route("/_bench/journal", get(journal))
        .route("/_bench/state", get(state))
        .with_state(world.clone());
    let app = admin.merge(service_app(world, &Tavily));
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .expect("bind");
    eprintln!("tavily mock for {} on :{port}", dir.display());
    serve(listener, app).await.expect("serve");
}
