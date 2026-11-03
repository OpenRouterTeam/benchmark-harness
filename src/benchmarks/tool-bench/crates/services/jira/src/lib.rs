//! Mock Jira Server/Data Center and Confluence Server as called by `mcp-atlassian` 0.23.1
//! (`JIRA_URL={world}`, `CONFLUENCE_URL={world}/wiki`, personal access tokens).
//!
//! Jira: `/rest/api/2` (issues, search with JQL, comments, transitions, worklogs, watchers,
//! links, remote links, attachments, fields, create metadata, projects, versions,
//! components, statuses, priorities, users), `/rest/agile/1.0` (boards, sprints, backlog),
//! `/rest/servicedeskapi` and `/rest/dev-status`. Confluence: `/wiki/rest/api` (content,
//! children, labels, comments, CQL search, spaces, users, attachments, templates).
//!
//! Collections: `users`, `projects`, `issuetypes`, `statuses`, `priorities`, `resolutions`,
//! `transitions`, `fields`, `issues` (keyed by issue key), `comments`, `worklogs`,
//! `issuelinks`, `issuelinktypes`, `remotelinks`, `attachments`, `versions`, `boards`,
//! `sprints`, `servicedesks`, `queues`, `requesttypes`, and for Confluence `spaces`,
//! `pages`, `page_comments`, `page_attachments`, `templates`.

mod agile;
mod confluence;
mod issues;
mod jql;
mod model;

use axum::{
    Router,
    body::Bytes,
    http::{HeaderMap, Method, StatusCode, Uri, header},
    response::Response,
};
use mock_core::{http::json, service::MockService, world::SharedWorld};
use serde_json::json as json_value;

pub use model::Ctx;
use model::{Out, Req};

pub const NAME: &str = "jira";
pub const PORT: u16 = 8128;

pub struct Jira;

impl MockService for Jira {
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
        Router::new().fallback(handle)
    }
}

async fn handle(
    axum::extract::State(world): axum::extract::State<SharedWorld>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("world:8128")
        .to_owned();
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let path = uri.path().trim_end_matches('/').to_owned();
    let confluence = path.starts_with("/wiki/");
    if !auth.starts_with("Bearer ") && !auth.starts_with("Basic ") {
        return if confluence {
            json(
                StatusCode::UNAUTHORIZED,
                json_value!({"statusCode": 401, "data": {"authorized": false, "valid": true, "errors": [], "successful": false}, "message": "This request requires authentication."}),
            )
        } else {
            json(
                StatusCode::UNAUTHORIZED,
                json_value!({"errorMessages": ["You are not authenticated. Authentication required to perform this operation."], "errors": {}}),
            )
        };
    }
    let mut guard = world.lock().expect("world lock");
    let now = guard.clock.now_unix();
    let st = guard.service_mut(NAME);
    let mut ctx = Ctx {
        st,
        now,
        base: format!("http://{host}"),
    };
    let req = Req::new(method, &path, uri.query(), &body, &content_type);
    let out = if confluence {
        confluence::route(&mut ctx, &req)
    } else {
        route(&mut ctx, &req)
    };
    match out {
        Out::Json(status, v) => json(status, v),
        Out::Empty(status) => Response::builder()
            .status(status)
            .body(axum::body::Body::empty())
            .expect("response"),
        Out::Bytes(ct, bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, ct)
            .body(axum::body::Body::from(bytes))
            .expect("response"),
    }
}

fn route(ctx: &mut Ctx, req: &Req) -> Out {
    let segs: Vec<&str> = req.path.trim_start_matches('/').split('/').collect();
    let m = req.method.as_str();
    match segs.as_slice() {
        ["rest", "api", "2" | "latest" | "3", rest @ ..] => issues::route(ctx, req, m, rest),
        ["rest", "agile", "1.0", rest @ ..] => agile::route(ctx, req, m, rest),
        ["rest", "servicedeskapi", rest @ ..] => agile::servicedesk(ctx, req, m, rest),
        ["rest", "dev-status", "1.0" | "latest", rest @ ..] => agile::dev_status(ctx, req, m, rest),
        ["secure", "attachment", id, ..] => issues::attachment_content(ctx, id),
        _ => model::not_found_page(),
    }
}
