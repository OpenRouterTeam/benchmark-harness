//! Mock Cloudflare for the `cloudflare` replica endpoint: the Code Mode tools of
//! cloudflare/mcp@68c7ec0 (`docs`, `search`, `execute`, `whoami`) on `/_mcp/tools/call`,
//! backed by a mock Cloudflare REST API (`/client/v4`) that is also served over HTTP.
//!
//! `search` and `execute` run the agent's JavaScript in an embedded QuickJS engine
//! (rquickjs). `search` sees `spec`, a pinned copy of the operations this mock implements,
//! taken verbatim from the server's pre-resolved OpenAPI spec (`assets/spec.json`, written by
//! `assets/trim-spec.py`). `execute` gets `cloudflare.request()` and `accountId`; each
//! request it makes is answered in-process and journaled as `/client/v4/...`, so graders can
//! match API calls the same way as for HTTP services.
//!
//! Dataset collections (`dataset/cloudflare/<collection>.json`):
//!
//! | collection | key | notes |
//! |---|---|---|
//! | `user` | `user` | the token's user (`id`, `email`, names) |
//! | `accounts` | id | the session's accounts; one account makes `accountId` implicit |
//! | `members` | id | account members, with `account_id` |
//! | `zones` | id | `name`, `status`, `account: {id, name}`, optional `plan`, `name_servers` |
//! | `dns_records` | id | with `zone_id`; new ids hash (zone, type, name, content) |
//! | `workers_scripts` | `<account_id>/<name>` | `content`, `bindings`, `secrets`, dates |
//! | `workers_routes` | id | `zone_id`, `pattern`, `script`; new ids hash (zone, pattern) |
//! | `kv_namespaces` | id | `account_id`, `title` |
//! | `kv_values` | `<namespace_id>/<key>` | `value` text, `value_json` when JSON, `metadata`, `expiration` |
//! | `r2_buckets` | `<account_id>/<name>` | `creation_date`, `location`, `storage_class`, `object_count` |
//! | `d1_databases` | uuid | `account_id`, `name`, `created_at` |
//! | `d1_tables` | `<uuid>/<table>` | `sql` (CREATE TABLE), `rows` |
//! | `cache_purges` | `<zone_id>#<n>` | written by purge_cache |

pub mod accounts;
pub mod api;
pub mod d1;
pub mod js;
pub mod kv;
pub mod multipart;
pub mod r2;
pub mod tools;
pub mod workers;
pub mod zones;

use std::net::SocketAddr;

use axum::{
    Router,
    body::{Bytes, to_bytes},
    extract::{ConnectInfo, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{any, post},
};
use mock_core::{
    mcp::{TOOLS_CALL_PATH, ToolCall},
    service::MockService,
    world::SharedWorld,
};

use crate::api::{ApiRequest, ApiResponse, Body};

pub const NAME: &str = "cloudflare";
pub const PORT: u16 = 8117;

pub struct Cloudflare;

impl MockService for Cloudflare {
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
            .route("/client/v4", any(rest))
            .route("/client/v4/{*rest}", any(rest))
            .fallback(not_found)
    }
}

fn to_response(r: ApiResponse) -> Response {
    (
        StatusCode::from_u16(r.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        [(header::CONTENT_TYPE, r.content_type)],
        r.body,
    )
        .into_response()
}

async fn not_found() -> Response {
    to_response(api::fail(404, &[(7000, "No route for that URI")]))
}

/// The REST API over HTTP (the journal middleware has already ticked the clock).
async fn rest(State(world): State<SharedWorld>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let bytes: Bytes = to_bytes(body, 64 * 1024 * 1024).await.unwrap_or_default();
    let path = parts
        .uri
        .path()
        .strip_prefix("/client/v4")
        .unwrap_or("")
        .to_owned();
    let path = if path.is_empty() {
        "/".to_owned()
    } else {
        path
    };
    let mut req = ApiRequest::new(parts.method.as_str(), &path, Vec::new());
    req.query = api::parse_query(parts.uri.query().unwrap_or(""));
    req.content_type = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    req.body = match req.content_type.as_deref().and_then(multipart::boundary) {
        Some(boundary) if req.media_type() == "multipart/form-data" => {
            Body::Form(multipart::parse(&bytes, &boundary))
        }
        _ if bytes.is_empty() => Body::None,
        _ => Body::Bytes(bytes.to_vec()),
    };
    let mut w = world.lock().expect("world lock");
    to_response(api::handle(&mut w, &req))
}

/// Replica tool calls (see `servers/snapshots/cloudflare.json` for the tool list).
async fn tools_call(State(world): State<SharedWorld>, request: Request) -> Response {
    let source = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip().to_string())
        .unwrap_or_else(|| "unknown".to_owned());
    let bytes = to_bytes(request.into_body(), 64 * 1024 * 1024)
        .await
        .unwrap_or_default();
    let Ok(call) = serde_json::from_slice::<ToolCall>(&bytes) else {
        return (StatusCode::BAD_REQUEST, "invalid tool call").into_response();
    };
    let result = tokio::task::spawn_blocking(move || tools::call(&world, &call, &source))
        .await
        .unwrap_or_else(|e| mock_core::mcp::error(format!("Error: {e}")));
    mock_core::http::json(StatusCode::OK, result)
}
