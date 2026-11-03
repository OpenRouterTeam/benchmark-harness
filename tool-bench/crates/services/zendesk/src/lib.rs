//! Mock Zendesk Support (replica mode, `octo-corp.zendesk.com`).
//!
//! - `/_mcp/tools/call` answers the 7 tools of `reminia/zendesk-mcp-server` @806d1ca
//!   (`servers/snapshots/zendesk.json`) with the exact text that server prints.
//! - `/api/v2/...` serves the matching REST API (tickets, comments, audits, users,
//!   organizations, attachments, Help Center) from the same store; see [`model`] for the
//!   collections and keys.

pub mod model;
pub mod pyjson;
mod rest;
pub mod text;
mod tools;

use axum::{Json, Router, extract::State, response::Response, routing::post};
use mock_core::{
    http::json,
    mcp::{TOOLS_CALL_PATH, ToolCall},
    service::MockService,
    world::SharedWorld,
};
use serde_json::Value;

pub const NAME: &str = "zendesk";
pub const PORT: u16 = 8124;

pub struct Zendesk;

impl MockService for Zendesk {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port(&self) -> u16 {
        PORT
    }

    fn hosts(&self) -> &'static [&'static str] {
        &[]
    }

    fn router(&self) -> Router<SharedWorld> {
        rest::routes(Router::new().route(TOOLS_CALL_PATH, post(tools_call))).fallback(not_found)
    }
}

async fn not_found() -> Response {
    let e = model::ApiError::invalid_endpoint();
    json(e.status, e.body)
}

async fn tools_call(State(world): State<SharedWorld>, Json(call): Json<ToolCall>) -> Json<Value> {
    let mut world = world.lock().expect("world lock");
    Json(tools::call(&mut world, &call.name, &call.arguments))
}
