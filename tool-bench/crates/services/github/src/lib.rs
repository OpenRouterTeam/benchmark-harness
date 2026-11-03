#![recursion_limit = "512"]
// Handlers return `Result<_, Response>` so validation errors short-circuit with GitHub's body.
#![allow(clippy::result_large_err)]
//! Mock GitHub Enterprise Server (`https://github.octo-corp.com`) as called by
//! `github-mcp-server` v2.0.3 with `GITHUB_HOST` pointed at it: REST under `/api/v3`,
//! GraphQL at `POST /api/graphql`, raw files under `/raw/`, and job-log downloads.
//!
//! Requests are dispatched by path segments in [`route`]; each endpoint family lives in
//! its own module. State is the scenario dataset (`dataset/github/*.json`), see the
//! module docs of [`git`] and [`render`] for the document shapes.

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use mock_core::{
    service::MockService,
    world::{ServiceState, SharedWorld},
};
use serde_json::json;

mod actions;
mod activity;
pub mod git;
mod graphql;
mod issues;
mod projects;
mod pulls;
pub mod render;
mod repos;
mod search;
mod security;
pub mod sha1;
pub mod util;

use util::Req;

pub const NAME: &str = "github";
pub const PORT: u16 = 8101;

/// Scopes reported for the token (`X-OAuth-Scopes`); the server filters tools by them.
pub const TOKEN_SCOPES: &str = "admin:enterprise, admin:org, admin:repo_hook, delete_repo, gist, notifications, project, repo, security_events, user, workflow, write:discussion, write:packages";

pub struct GitHub;

impl MockService for GitHub {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port(&self) -> u16 {
        PORT
    }

    fn hosts(&self) -> &'static [&'static str] {
        &["github.octo-corp.com", "api.github.com"]
    }

    fn router(&self) -> Router<SharedWorld> {
        Router::new().fallback(handle)
    }
}

/// Per-request handler context.
pub struct Ctx<'a> {
    pub svc: &'a mut ServiceState,
    pub now: String,
    pub req: &'a Req,
}

impl Ctx<'_> {
    pub fn base(&self) -> &str {
        &self.req.base
    }

    pub fn viewer(&self) -> String {
        render::viewer_login(self.svc)
    }
}

async fn handle(
    State(world): State<SharedWorld>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let req = Req::new(method, &uri, headers, body);
    let mut guard = world.lock().expect("world lock");
    let now = guard.clock.now_rfc3339();
    let svc = guard.service_mut(NAME);
    let mut ctx = Ctx {
        svc,
        now,
        req: &req,
    };
    let mut resp = route(&mut ctx);
    let h = resp.headers_mut();
    h.insert("x-oauth-scopes", HeaderValue::from_static(TOKEN_SCOPES));
    h.insert(
        "x-github-enterprise-version",
        HeaderValue::from_static("3.17.0"),
    );
    h.insert("x-github-media-type", HeaderValue::from_static("github.v3"));
    resp
}

fn route(ctx: &mut Ctx) -> Response {
    let segs: Vec<String> = ctx.req.segs.clone();
    let segs: Vec<&str> = segs.iter().map(String::as_str).collect();
    let method = ctx.req.method.clone();
    match segs.as_slice() {
        ["api", "graphql"] if method == Method::POST => graphql::handle(ctx),
        ["api", "v3"] | [] => api_root(ctx),
        ["api", "v3", tail @ ..] => rest(ctx, tail),
        ["api", "uploads", ..] => util::not_found(),
        ["raw", rest @ ..] => repos::raw(ctx, rest),
        ["_downloads", rest @ ..] => actions::download(ctx, rest),
        ["_ping"] => (StatusCode::OK, "PONG").into_response(),
        tail if ctx.req.base.contains("api.github.com") => rest(ctx, tail),
        _ => util::not_found(),
    }
}

fn rest(ctx: &mut Ctx, segs: &[&str]) -> Response {
    let routed = activity::route(ctx, segs)
        .or_else(|| repos::route(ctx, segs))
        .or_else(|| issues::route(ctx, segs))
        .or_else(|| pulls::route(ctx, segs))
        .or_else(|| actions::route(ctx, segs))
        .or_else(|| search::route(ctx, segs))
        .or_else(|| security::route(ctx, segs))
        .or_else(|| projects::route(ctx, segs));
    routed.unwrap_or_else(util::not_found)
}

fn api_root(ctx: &mut Ctx) -> Response {
    if ctx.req.method == Method::HEAD {
        return StatusCode::OK.into_response();
    }
    let api = ctx.req.api();
    util::ok(json!({
        "current_user_url": format!("{api}/user"),
        "authorizations_url": format!("{api}/authorizations"),
        "code_search_url": format!("{api}/search/code?q={{query}}{{&page,per_page,sort,order}}"),
        "commit_search_url": format!("{api}/search/commits?q={{query}}{{&page,per_page,sort,order}}"),
        "emails_url": format!("{api}/user/emails"),
        "events_url": format!("{api}/events"),
        "feeds_url": format!("{api}/feeds"),
        "gists_url": format!("{api}/gists{{/gist_id}}"),
        "issue_search_url": format!("{api}/search/issues?q={{query}}{{&page,per_page,sort,order}}"),
        "issues_url": format!("{api}/issues"),
        "notifications_url": format!("{api}/notifications"),
        "organization_url": format!("{api}/orgs/{{org}}"),
        "repository_url": format!("{api}/repos/{{owner}}/{{repo}}"),
        "repository_search_url": format!("{api}/search/repositories?q={{query}}{{&page,per_page,sort,order}}"),
        "starred_url": format!("{api}/user/starred{{/owner}}{{/repo}}"),
        "user_url": format!("{api}/users/{{user}}"),
        "user_search_url": format!("{api}/search/users?q={{query}}{{&page,per_page,sort,order}}"),
    }))
}
