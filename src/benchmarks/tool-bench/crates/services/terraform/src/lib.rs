//! Mock HCP Terraform (JSON:API under `/api/v2`, private registry under `/api/registry`) and the
//! public Terraform Registry (`registry.terraform.io`, reached through the world's TLS proxy),
//! as called by `terraform-mcp-server` 1.4.1 through go-tfe and its own registry client.
//!
//! Store collections (documents keep HCP attribute names; relation fields hold IDs):
//! `organizations` (by name), `users`, `organization_memberships`, `projects`, `teams`,
//! `team_workspaces`, `team_projects`, `workspaces`, `vars`, `varsets`, `varset_vars`,
//! `policy_sets`, `policy_checks`, `runs`, `plans`, `applies`, `comments`, `state_versions`,
//! `plan_exports`, `stacks`, `registry_modules`, `no_code_modules`, `registry_providers`, and the
//! optional public registry overrides `public_providers`, `public_provider_docs`,
//! `public_modules`, `public_policies`.

#![recursion_limit = "256"]

mod common;
mod org;
mod private_registry;
mod public_registry;
mod resources;
mod runs;
mod sets;
mod tarball;
mod workspaces;

use axum::{
    Router,
    extract::Request,
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use mock_core::{service::MockService, world::SharedWorld};

pub const NAME: &str = "terraform";
pub const PORT: u16 = 8109;

pub struct Terraform;

impl MockService for Terraform {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port(&self) -> u16 {
        PORT
    }

    fn hosts(&self) -> &'static [&'static str] {
        &["registry.terraform.io"]
    }

    fn router(&self) -> Router<SharedWorld> {
        let api = Router::new()
            .merge(org::routes())
            .merge(workspaces::routes())
            .merge(runs::routes())
            .merge(sets::routes())
            .merge(private_registry::routes())
            .layer(middleware::from_fn(require_token));
        Router::new()
            .route("/api/v2/ping", get(ping))
            .merge(api)
            .merge(runs::archivist_routes())
            .merge(public_registry::routes())
            .fallback(fallback)
    }
}

/// HCP Terraform rejects API calls without a bearer token.
async fn require_token(request: Request, next: Next) -> Response {
    let authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| !t.trim().is_empty());
    if !authorized {
        return common::unauthorized();
    }
    next.run(request).await
}

async fn ping() -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    let headers = response.headers_mut();
    headers.insert("tfp-api-version", HeaderValue::from_static("2.6"));
    headers.insert("tfp-appname", HeaderValue::from_static("HCP Terraform"));
    headers.insert("x-ratelimit-limit", HeaderValue::from_static("30"));
    response
}

async fn fallback(request: Request) -> Response {
    let path = request.uri().path();
    if path.starts_with("/v1/") {
        return mock_core::http::json(
            StatusCode::NOT_FOUND,
            serde_json::json!({ "errors": ["Not Found"] }),
        );
    }
    common::not_found()
}
