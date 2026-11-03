//! Mock HubSpot API, as called by `@hubspot/mcp-server` 0.4.0 with `BASE_URL_OVERRIDE`
//! pointed at the world.
//!
//! Endpoints (everything the server's 21 tools call, plus close neighbours):
//! - `POST /oauth/v2/private-apps/get/access-token-info`, `GET /account-info/v3/details`
//! - `GET /crm/v3/owners`, `GET /crm/v3/owners/{id}` (`idProperty=userId`)
//! - `GET /crm/v3/objects/{type}`, `GET /crm/v3/objects/{type}/{id}`,
//!   `POST /crm/v3/objects/{type}/search`, `POST /crm/v3/objects/{type}/batch/{read,create,update}`
//! - `GET|POST /crm/v3/properties/{type}`, `GET|PATCH /crm/v3/properties/{type}/{name}`
//! - `GET /crm-object-schemas/v3/schemas[/{type}]`
//! - `GET /crm/v4/objects/{type}/{id}/associations/{toType}`,
//!   `GET /crm/v4/associations/{from}/{to}/labels`, `POST .../batch/create`
//! - `POST /engagements/v1/engagements`, `GET|PATCH /engagements/v1/engagements/{id}`
//! - `GET /automation/v4/flows`, `GET /automation/v4/flows/{id}`
//!
//! State collections: one per object type (`contacts`, `companies`, `deals`, `tickets`,
//! `notes`, `tasks`, custom object names, ...) keyed by id; `associations` keyed
//! `<typeA>/<idA>/<typeB>/<idB>` (types in name order); `properties` keyed
//! `<type>/<name>`; `owners`, `schemas`, `flows`, `association_labels`; `account`
//! (`details`, `token`).

mod assoc;
mod engagements;
mod misc;
mod objects;
mod props;
mod types;
mod util;

use axum::{
    Router,
    extract::Request,
    http::{StatusCode, header},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
};
use mock_core::{http::json, service::MockService, world::SharedWorld};
use serde_json::json as json_value;

pub const NAME: &str = "hubspot";
pub const PORT: u16 = 8123;

pub struct Hubspot;

impl MockService for Hubspot {
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
            .route(
                "/oauth/v2/private-apps/get/access-token-info",
                post(misc::access_token_info),
            )
            .route("/account-info/v3/details", get(misc::details))
            .route("/crm/v3/owners", get(misc::list_owners))
            .route("/crm/v3/owners/", get(misc::list_owners))
            .route("/crm/v3/owners/{id}", get(misc::get_owner))
            .route("/crm/v3/objects/{object_type}", get(objects::list))
            .route(
                "/crm/v3/objects/{object_type}/search",
                post(objects::search),
            )
            .route(
                "/crm/v3/objects/{object_type}/batch/read",
                post(objects::batch_read),
            )
            .route(
                "/crm/v3/objects/{object_type}/batch/create",
                post(objects::batch_create),
            )
            .route(
                "/crm/v3/objects/{object_type}/batch/update",
                post(objects::batch_update),
            )
            .route(
                "/crm/v3/objects/{object_type}/{object_id}",
                get(objects::get_one),
            )
            .route(
                "/crm/v3/properties/{object_type}",
                get(props::list).post(props::create),
            )
            .route(
                "/crm/v3/properties/{object_type}/{property_name}",
                get(props::get_one).patch(props::update),
            )
            .route("/crm-object-schemas/v3/schemas", get(misc::list_schemas))
            .route(
                "/crm-object-schemas/v3/schemas/{object_type}",
                get(misc::get_schema),
            )
            .route(
                "/crm/v4/objects/{object_type}/{object_id}/associations/{to_object_type}",
                get(assoc::list_v4),
            )
            .route(
                "/crm/v4/associations/{from_object_type}/{to_object_type}/labels",
                get(assoc::labels),
            )
            .route(
                "/crm/v4/associations/{from_object_type}/{to_object_type}/batch/create",
                post(assoc::batch_create),
            )
            .route("/engagements/v1/engagements", post(engagements::create))
            .route(
                "/engagements/v1/engagements/{engagement_id}",
                get(engagements::get_one).patch(engagements::update),
            )
            .route("/automation/v4/flows", get(misc::list_flows))
            .route("/automation/v4/flows/{flow_id}", get(misc::get_flow))
            .fallback(not_found)
            .layer(middleware::from_fn(require_auth))
    }
}

/// Every HubSpot API call needs a bearer token (the MCP server always sends one).
async fn require_auth(request: Request, next: Next) -> Response {
    let authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| !t.trim().is_empty());
    if authorized {
        return next.run(request).await;
    }
    json(
        StatusCode::UNAUTHORIZED,
        json_value!({
            "status": "error",
            "message": "Authentication credentials not found. This API supports OAuth 2.0 authentication and you can find more details at https://developers.hubspot.com/docs/methods/auth/oauth-overview",
            "correlationId": "00000000-0000-8000-8000-000000000000",
            "category": "INVALID_AUTHENTICATION"
        }),
    )
}

async fn not_found() -> Response {
    json(
        StatusCode::NOT_FOUND,
        json_value!({
            "status": "error",
            "message": "Resource not found",
            "correlationId": "00000000-0000-8000-8000-000000000000",
            "category": "OBJECT_NOT_FOUND"
        }),
    )
}
