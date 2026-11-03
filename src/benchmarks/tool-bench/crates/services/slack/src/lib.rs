//! Mock Slack workspace (replica mode).
//!
//! - `POST /_mcp/tools/call` answers the 27 tools of the official Slack MCP server
//!   (`mcp.slack.com`, snapshot `servers/snapshots/slack.json`) in its output format.
//! - `GET|POST /api/<method>` serves Slack Web API methods over the same store.
//! - `POST /upload/v1/<token>` receives file bytes for `slack_get_file_upload_url`.
//!
//! The caller is always `U0TOOLBNCH1`. See [`model`] for the store layout.

pub mod canvas;
pub mod lists;
pub mod model;
pub mod search;
pub mod tools;
pub mod tz;
pub mod util;
pub mod webapi;

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, RawQuery, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{any, post},
};
use mock_core::{
    http::json,
    mcp::{self, TOOLS_CALL_PATH, ToolCall},
    service::MockService,
    world::SharedWorld,
};
use serde_json::{Value, json as json_value};

use crate::{model::Ctx, util::s};

pub const NAME: &str = "slack";
pub const PORT: u16 = 8126;

pub struct Slack;

impl MockService for Slack {
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
        Router::new()
            .route(TOOLS_CALL_PATH, post(tools_call))
            .route("/api/{method}", any(web_api))
            .route("/upload/v1/{token}", post(upload))
            .fallback(not_found)
    }
}

async fn not_found() -> Response {
    json(
        StatusCode::NOT_FOUND,
        json_value!({ "ok": false, "error": "unknown_method" }),
    )
}

/// Runs `f` against the Slack store at the current logical time.
fn with_ctx<T>(world: &SharedWorld, f: impl FnOnce(&mut Ctx) -> T) -> T {
    let mut guard = world.lock().expect("world lock");
    let now = guard.clock.now_unix();
    let st = guard.service_mut(NAME);
    let mut ctx = Ctx { st, now };
    f(&mut ctx)
}

/// Replica tool calls (see `servers/snapshots/slack.json` for the tool list).
async fn tools_call(State(world): State<SharedWorld>, Json(call): Json<ToolCall>) -> Json<Value> {
    let args = match call.arguments {
        Value::Null => json_value!({}),
        other => other,
    };
    let result = with_ctx(&world, |ctx| tools::dispatch(ctx, &call.name, &args));
    Json(match result {
        Some(Ok(v)) => mcp::text(v.to_string()),
        Some(Err(e)) => mcp::error(e),
        None => mcp::error(format!("Unknown tool: {}", call.name)),
    })
}

async fn web_api(
    State(world): State<SharedWorld>,
    Path(method): Path<String>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let args = webapi::params(query.as_deref(), content_type, &body);
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .or_else(|| {
            args.get("token")
                .and_then(Value::as_str)
                .map(|t| format!("Bearer {t}"))
        });
    if auth.is_none() && method != "api.test" {
        return json(
            StatusCode::OK,
            json_value!({"ok": false, "error": "not_authed"}),
        );
    }
    let reply = with_ctx(&world, |ctx| webapi::call(ctx, &method, &args));
    json(StatusCode::OK, reply)
}

async fn upload(
    State(world): State<SharedWorld>,
    Path(token): Path<String>,
    body: Bytes,
) -> Response {
    let found = with_ctx(&world, |ctx| {
        let file = ctx
            .all("files")
            .into_iter()
            .find(|(_, f)| s(f, "upload_token") == token && s(f, "upload_state") == "pending");
        let Some((key, mut f)) = file else {
            return false;
        };
        f["upload_state"] = json_value!("uploaded");
        f["size"] = json_value!(body.len());
        match std::str::from_utf8(&body) {
            Ok(text) => f["content"] = json_value!(text),
            Err(_) => f["content_base64"] = json_value!(util::b64(&body)),
        }
        ctx.put("files", &key, f);
        true
    });
    if found {
        json(StatusCode::OK, json_value!({"ok": true}))
    } else {
        (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
    }
}
