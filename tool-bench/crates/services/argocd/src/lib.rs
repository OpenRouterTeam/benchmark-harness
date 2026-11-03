//! Mock Argo CD API server (`/api/v1`), as called by `argocd-mcp` with
//! `ARGOCD_BASE_URL` pointed at the world. See [`model`] for the store layout.

mod apps;
mod error;
mod infra;
mod model;
mod resources;

use axum::{
    Router,
    extract::Request,
    http::header,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use mock_core::{service::MockService, world::SharedWorld};

pub use error::ArgoError;

pub const NAME: &str = "argocd";
pub const PORT: u16 = 8108;
/// The token the catalog gives the MCP server (`ARGOCD_API_TOKEN`).
pub const API_TOKEN: &str = "argocd-toolbench";

pub struct Argocd;

impl MockService for Argocd {
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
            .route("/api/v1/applications", get(apps::list).post(apps::create))
            .route(
                "/api/v1/applications/{name}",
                get(apps::get).put(apps::update).delete(apps::delete),
            )
            .route(
                "/api/v1/applications/{name}/sync",
                axum::routing::post(apps::sync),
            )
            .route(
                "/api/v1/applications/{name}/resource-tree",
                get(resources::tree),
            )
            .route(
                "/api/v1/applications/{name}/managed-resources",
                get(resources::managed),
            )
            .route("/api/v1/applications/{name}/events", get(resources::events))
            .route(
                "/api/v1/applications/{name}/resource",
                get(resources::get_resource),
            )
            .route(
                "/api/v1/applications/{name}/resource/actions",
                get(resources::list_actions).post(resources::run_action),
            )
            .route("/api/v1/applications/{name}/logs", get(resources::logs))
            .route(
                "/api/v1/applications/{name}/pods/{pod}/logs",
                get(resources::pod_logs),
            )
            .route("/api/v1/clusters", get(infra::list_clusters))
            .route("/api/v1/projects", get(infra::list_projects))
            .route("/api/v1/projects/{name}", get(infra::get_project))
            .method_not_allowed_fallback(method_not_allowed)
            .fallback(not_found)
            .layer(middleware::from_fn(authenticate))
    }
}

async fn not_found() -> Response {
    ArgoError::not_found("Not Found").into_response()
}

async fn method_not_allowed() -> Response {
    ArgoError::method_not_allowed().into_response()
}

/// Bearer-token check, as the API server does for every `/api` call.
async fn authenticate(request: Request, next: Next) -> Response {
    let auth = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match auth {
        None => ArgoError::unauthenticated("no session information").into_response(),
        Some(token) if token.trim() != API_TOKEN => ArgoError::unauthenticated(
            "invalid session: token contains an invalid number of segments",
        )
        .into_response(),
        Some(_) => next.run(request).await,
    }
}

/// Query string pairs in order (repeated keys allowed, as grpc-gateway accepts).
#[derive(Debug, Default)]
pub(crate) struct Params(Vec<(String, String)>);

impl Params {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// The value, treating an empty string as absent.
    pub fn non_empty(&self, key: &str) -> Option<&str> {
        self.get(key).filter(|v| !v.is_empty())
    }

    pub fn all(&self, key: &str) -> Vec<&str> {
        self.0
            .iter()
            .filter(|(k, _)| k == key)
            .flat_map(|(_, v)| v.split(','))
            .filter(|v| !v.is_empty())
            .collect()
    }

    pub fn flag(&self, key: &str) -> Option<bool> {
        self.get(key).map(|v| v == "true" || v == "1")
    }
}

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for Params {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let pairs = axum::extract::Query::<Vec<(String, String)>>::from_request_parts(parts, state)
            .await
            .map(|q| q.0)
            .unwrap_or_default();
        Ok(Self(pairs))
    }
}
