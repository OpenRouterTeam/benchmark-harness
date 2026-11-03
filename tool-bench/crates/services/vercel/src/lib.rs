//! Mock Vercel (replica mode). The tools container forwards `tools/call` for the 15 tools of
//! nganiet/mcp-vercel to `/_mcp/tools/call` (see `tools`); the same store also backs the
//! Vercel REST endpoints those tools call (`api.vercel.com` paths), served on the world port.

mod api;
mod model;
mod tools;
mod zod;

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use mock_core::{
    http::json,
    mcp::{TOOLS_CALL_PATH, ToolCall},
    service::MockService,
    world::{SharedWorld, WorldState},
};
use serde_json::Value;

pub use crate::model::Query as Params;
use crate::model::{ApiError, ApiResult, api_error};

pub const NAME: &str = "vercel";
pub const PORT: u16 = 8110;

pub struct Vercel;

impl MockService for Vercel {
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
            .route("/v6/deployments", get(list_deployments))
            .route("/v13/deployments", post(create_deployment))
            .route("/v13/deployments/{id_or_url}", get(get_deployment))
            .route("/v6/deployments/{id}/files", get(deployment_files))
            .route("/v9/projects", get(list_projects))
            .route("/v10/projects", get(list_projects))
            .route("/v11/projects", post(create_project))
            .route(
                "/v9/projects/{project}",
                get(get_project).delete(delete_project),
            )
            .route(
                "/v10/projects/{project}/env",
                get(list_env).post(create_env),
            )
            .route("/v9/projects/{project}/env/{env_id}", delete(delete_env))
            .route("/v10/projects/{project}/env/{env_id}", delete(delete_env))
            .route("/v9/projects/{project}/domains", get(list_domains))
            .route("/v9/projects/{project}/domains/{domain}", get(get_domain))
            .route(
                "/v9/projects/{project}/custom-environments",
                get(list_custom_envs).post(create_custom_env),
            )
            .route("/v2/teams", get(list_teams))
            .route("/v2/teams/{team}", get(get_team))
            .route("/v1/teams", post(create_team))
            .route("/v2/user", get(get_user))
            .fallback(fallback)
    }
}

async fn fallback() -> Response {
    error_response(api_error(
        StatusCode::NOT_FOUND,
        "not_found",
        "The requested API endpoint was not found.",
    ))
}

/// Replica tool calls. Always 200: tool failures are reported inside the result.
async fn tools_call(State(world): State<SharedWorld>, Json(call): Json<ToolCall>) -> Json<Value> {
    let mut world = world.lock().expect("world lock");
    Json(tools::call(&mut world, &call.name, &call.arguments))
}

fn error_response(e: ApiError) -> Response {
    json(e.status, e.body)
}

fn respond(r: ApiResult) -> Response {
    match r {
        Ok((StatusCode::NO_CONTENT, _)) => StatusCode::NO_CONTENT.into_response(),
        Ok((status, body)) => json(status, body),
        Err(e) => error_response(e),
    }
}

/// Vercel answers `403 forbidden` to requests without a bearer token.
fn authorized(headers: &HeaderMap) -> Result<(), ApiError> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.starts_with("Bearer ") && v.len() > 7)
        .map(|_| ())
        .ok_or_else(|| {
            let mut e = model::forbidden();
            e.body["error"]["invalidToken"] = Value::Bool(true);
            e
        })
}

fn parse_body(body: &Bytes) -> Result<Value, ApiError> {
    if body.is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    serde_json::from_slice(body)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "bad_request", "Invalid JSON"))
}

/// Runs `op` against the locked world after the auth check.
fn with_world(
    world: &SharedWorld,
    headers: &HeaderMap,
    op: impl FnOnce(&mut WorldState) -> ApiResult,
) -> Response {
    if let Err(e) = authorized(headers) {
        return error_response(e);
    }
    let mut world = world.lock().expect("world lock");
    respond(op(&mut world))
}

type W = State<SharedWorld>;
type Q = Query<Params>;

async fn list_deployments(State(w): W, h: HeaderMap, Query(q): Q) -> Response {
    with_world(&w, &h, |world| api::list_deployments(world, &q))
}

async fn get_deployment(
    State(w): W,
    h: HeaderMap,
    Path(id): Path<String>,
    Query(q): Q,
) -> Response {
    with_world(&w, &h, |world| api::get_deployment(world, &id, &q))
}

async fn deployment_files(
    State(w): W,
    h: HeaderMap,
    Path(id): Path<String>,
    Query(q): Q,
) -> Response {
    with_world(&w, &h, |world| api::deployment_files(world, &id, &q))
}

async fn create_deployment(State(w): W, h: HeaderMap, Query(q): Q, body: Bytes) -> Response {
    with_world(&w, &h, |world| {
        api::create_deployment(world, &q, &parse_body(&body)?)
    })
}

async fn list_projects(State(w): W, h: HeaderMap, Query(q): Q) -> Response {
    with_world(&w, &h, |world| api::list_projects(world, &q))
}

async fn create_project(State(w): W, h: HeaderMap, Query(q): Q, body: Bytes) -> Response {
    with_world(&w, &h, |world| {
        api::create_project(world, &q, &parse_body(&body)?)
    })
}

async fn get_project(State(w): W, h: HeaderMap, Path(p): Path<String>, Query(q): Q) -> Response {
    with_world(&w, &h, |world| api::get_project(world, &p, &q))
}

async fn delete_project(State(w): W, h: HeaderMap, Path(p): Path<String>, Query(q): Q) -> Response {
    with_world(&w, &h, |world| api::delete_project(world, &p, &q))
}

async fn list_env(State(w): W, h: HeaderMap, Path(p): Path<String>, Query(q): Q) -> Response {
    with_world(&w, &h, |world| api::list_env(world, &p, &q))
}

async fn create_env(
    State(w): W,
    h: HeaderMap,
    Path(p): Path<String>,
    Query(q): Q,
    body: Bytes,
) -> Response {
    with_world(&w, &h, |world| {
        api::create_env(world, &p, &q, &parse_body(&body)?)
    })
}

async fn delete_env(
    State(w): W,
    h: HeaderMap,
    Path((p, env_id)): Path<(String, String)>,
    Query(q): Q,
) -> Response {
    with_world(&w, &h, |world| api::delete_env(world, &p, &env_id, &q))
}

async fn list_domains(State(w): W, h: HeaderMap, Path(p): Path<String>, Query(q): Q) -> Response {
    with_world(&w, &h, |world| api::list_project_domains(world, &p, &q))
}

async fn get_domain(
    State(w): W,
    h: HeaderMap,
    Path((p, domain)): Path<(String, String)>,
    Query(q): Q,
) -> Response {
    with_world(&w, &h, |world| {
        api::get_project_domain(world, &p, &domain, &q)
    })
}

async fn list_custom_envs(
    State(w): W,
    h: HeaderMap,
    Path(p): Path<String>,
    Query(q): Q,
) -> Response {
    with_world(&w, &h, |world| api::list_custom_envs(world, &p, &q))
}

async fn create_custom_env(
    State(w): W,
    h: HeaderMap,
    Path(p): Path<String>,
    Query(q): Q,
    body: Bytes,
) -> Response {
    with_world(&w, &h, |world| {
        api::create_custom_env(world, &p, &q, &parse_body(&body)?)
    })
}

async fn list_teams(State(w): W, h: HeaderMap, Query(q): Q) -> Response {
    with_world(&w, &h, |world| api::list_teams(world, &q))
}

async fn get_team(State(w): W, h: HeaderMap, Path(t): Path<String>) -> Response {
    with_world(&w, &h, |world| api::get_team(world, &t))
}

async fn create_team(State(w): W, h: HeaderMap, body: Bytes) -> Response {
    with_world(&w, &h, |world| api::create_team(world, &parse_body(&body)?))
}

async fn get_user(State(w): W, h: HeaderMap) -> Response {
    with_world(&w, &h, api::get_user)
}
