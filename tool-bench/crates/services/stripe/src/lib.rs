//! Mock Stripe (replica mode).
//!
//! Two faces over one resource engine:
//! - `POST /_mcp/tools/call`: the ten tools of the remote Stripe MCP (`mcp.stripe.com`,
//!   `servers/snapshots/stripe.json`), addressed by OpenAPI operation id.
//! - `/v1/...`: the matching Stripe REST API (form-encoded bodies, `Authorization: Bearer`).
//!
//! The operation catalogue and object shapes are pinned assets derived from Stripe's OpenAPI
//! spec (API 2026-08-26.dahlia) via Kiln-AI/stripe_world (MIT), whose probes of the live
//! server also fix the tool output and error wording reproduced here.
//!
//! State: one collection per resource (`customers`, `charges`, `refunds`, `subscriptions`, …)
//! keyed by Stripe id, except `discounts`, keyed `<subscription>:<coupon>`. The sandbox
//! account comes from `account.json` (first document).

pub mod api;
pub mod billing;
pub mod catalog;
pub mod commerce;
pub mod engine;
pub mod error;
pub mod params;
pub mod payments;
pub mod tools;

use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header},
    response::Response,
    routing::post,
};
use mock_core::{
    http::json,
    mcp::{TOOLS_CALL_PATH, ToolCall},
    service::MockService,
    world::SharedWorld,
};
use serde_json::Value;

use crate::{engine::Ctx, error::StripeError, params::Params};

pub const NAME: &str = "stripe";
pub const PORT: u16 = 8121;

pub struct Stripe;

impl MockService for Stripe {
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
            .fallback(rest)
    }
}

async fn tools_call(State(world): State<SharedWorld>, Json(call): Json<ToolCall>) -> Json<Value> {
    let mut world = world.lock().expect("world lock");
    let mut ctx = Ctx::new(&mut world);
    Json(tools::call(&mut ctx, &call.name, &call.arguments))
}

fn stripe_response(status: u16, body: Value, request_id: &str) -> Response {
    let mut response = json(
        StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST),
        body,
    );
    let headers = response.headers_mut();
    headers.insert(
        "stripe-version",
        HeaderValue::from_static(catalog::API_VERSION),
    );
    if let Ok(v) = HeaderValue::from_str(request_id) {
        headers.insert("request-id", v);
    }
    response
}

/// The REST face: `/v1/...` with Stripe's form encoding.
async fn rest(
    State(world): State<SharedWorld>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut world = world.lock().expect("world lock");
    let mut ctx = Ctx::new(&mut world);
    let request_id = ctx.raw_id("rest_request", "req_", 14);
    let authorized = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("Bearer ") || v.starts_with("Basic "));
    if !authorized {
        let err = StripeError::invalid(
            "You did not provide an API key. You need to provide your API key in the Authorization header, using Bearer auth (e.g. 'Authorization: Bearer YOUR_SECRET_KEY'). See https://stripe.com/docs/api#authentication for details, or we can help at https://support.stripe.com/.",
        )
        .status(401);
        return stripe_response(err.status, err.body(), &request_id);
    }
    let mut map = params::decode_form(uri.query().unwrap_or_default());
    let text = String::from_utf8_lossy(&body);
    let is_json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    let body_map = if is_json {
        serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default()
    } else {
        params::decode_form(&text)
    };
    map.extend(body_map);
    let params = Params::new(map);
    let path = uri.path();
    let result = match api::match_path(method.as_str(), path) {
        Some((route, args)) => {
            ctx.request_id = Some(request_id.clone());
            api::dispatch(&mut ctx, route, &args, &params)
        }
        None => Err(StripeError::unrecognized_url(method.as_str(), path)),
    };
    match result {
        Ok(value) => stripe_response(200, value, &request_id),
        Err(err) => stripe_response(err.status, err.body(), &request_id),
    }
}
