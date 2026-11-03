//! Mock Shopify Admin API for `octo-corp.myshopify.com` (real-tls mode), as called by
//! `shopify-mcp`: the Admin GraphQL endpoint (`POST /admin/api/<version>/graphql.json`)
//! with a small GraphQL engine over the scenario's store, and the client-credentials
//! token exchange. See `model` for the dataset document shapes.

#![recursion_limit = "256"]

mod b64;
mod exec;
pub mod graphql;
mod model;
mod mutations;
mod schema;
mod search;

use axum::{
    Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::Response,
    routing::post,
};
use mock_core::{
    http::{GraphqlRequest, json},
    service::MockService,
    world::SharedWorld,
};
use serde_json::json as json_value;

pub const NAME: &str = "shopify";
pub const PORT: u16 = 8125;
pub const DOMAIN: &str = model::DOMAIN;
/// The Admin API token the catalog passes to the server (`--accessToken`).
pub const ACCESS_TOKEN: &str = "shpat_toolbench";

pub struct Shopify;

impl MockService for Shopify {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port(&self) -> u16 {
        PORT
    }

    fn hosts(&self) -> &'static [&'static str] {
        &["octo-corp.myshopify.com"]
    }

    fn router(&self) -> Router<SharedWorld> {
        Router::new()
            .route("/admin/api/{version}/graphql.json", post(graphql))
            .route("/admin/oauth/access_token", post(access_token))
            .fallback(not_found)
    }
}

async fn graphql(
    State(world): State<SharedWorld>,
    Path(version): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let token = headers
        .get("x-shopify-access-token")
        .and_then(|v| v.to_str().ok());
    if token != Some(ACCESS_TOKEN) {
        return json(
            StatusCode::UNAUTHORIZED,
            json_value!({ "errors": "[API] Invalid API key or access token (unrecognized login or wrong password)" }),
        );
    }
    let Ok(request) = serde_json::from_slice::<GraphqlRequest>(&body) else {
        return json(
            StatusCode::BAD_REQUEST,
            json_value!({ "errors": { "query": "Required parameter missing or invalid" } }),
        );
    };
    let body = match graphql::parse(&request.query) {
        Err(e) => json_value!({ "errors": [{
            "message": e.message,
            "locations": [{ "line": e.pos.line, "column": e.pos.column }],
        }] }),
        Ok(doc) => {
            let mut world = world.lock().expect("world lock");
            let now = world.clock.now_rfc3339();
            let store = world
                .service(NAME)
                .map(|s| s.store.clone())
                .unwrap_or_default();
            let outcome = exec::run(
                &doc,
                request.operation_name.as_deref(),
                &request.variables,
                store,
                &now,
            );
            if let Some(store) = outcome.commit {
                world.service_mut(NAME).store = store;
            }
            outcome.body
        }
    };
    let mut response = json(StatusCode::OK, body);
    if let Ok(v) = HeaderValue::from_str(&version) {
        response.headers_mut().insert("x-shopify-api-version", v);
    }
    response
}

async fn access_token() -> Response {
    json(
        StatusCode::OK,
        json_value!({
            "access_token": ACCESS_TOKEN,
            "scope": "read_products,write_products,read_orders,write_orders,read_customers,write_customers",
            "expires_in": 86399
        }),
    )
}

async fn not_found() -> Response {
    json(
        StatusCode::NOT_FOUND,
        json_value!({ "errors": "Not Found" }),
    )
}
