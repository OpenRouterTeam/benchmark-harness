//! Serves only the Salesforce mock for local checks against the real MCP server:
//! `cargo run -p mock-salesforce --example serve -- <scenario-dir> [port]`.
//! `GET /_state` returns the world snapshot.

use std::path::PathBuf;

use axum::{Router, extract::State, routing::get};
use mock_core::{
    dataset::Dataset,
    serve::{serve, service_app},
    world::{SharedWorld, WorldState, shared},
};
use mock_salesforce::Salesforce;

async fn state(State(world): State<SharedWorld>) -> String {
    world.lock().expect("world lock").snapshot().to_string()
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("scenario dir"));
    let port: u16 = args.next().map_or(18122, |p| p.parse().expect("port"));
    let world =
        shared(WorldState::from_dataset(Dataset::load(&dir).expect("dataset")).expect("world"));
    let app = service_app(world.clone(), &Salesforce)
        .merge(Router::new().route("/_state", get(state)).with_state(world));
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    eprintln!("salesforce mock on :{port}");
    serve(listener, app).await
}
