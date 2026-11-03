//! Mock GitLab REST API (`/api/v4`) and GraphQL API (`/api/graphql`), as called by
//! `@zereight/mcp-gitlab` with `GITLAB_API_URL` / `GITLAB_GRAPHQL_URL` pointed at the world.
//! The data model is documented in [`common`].

#![recursion_limit = "512"]
// Handlers return early with ready-made GitLab error responses.
#![allow(clippy::result_large_err)]

use axum::Router;
use mock_core::{service::MockService, world::SharedWorld};

pub mod common;

mod ci;
mod graphql;
mod issues;
mod merge_requests;
mod notes;
mod projects;
mod releases;
mod repository;
mod wikis;

pub const NAME: &str = "gitlab";
pub const PORT: u16 = 8107;

pub struct Gitlab;

impl MockService for Gitlab {
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
        let api = Router::new()
            .merge(projects::routes())
            .merge(issues::routes())
            .merge(merge_requests::routes())
            .merge(notes::routes())
            .merge(repository::routes())
            .merge(wikis::routes())
            .merge(ci::routes())
            .merge(releases::routes());
        Router::new()
            .nest("/api/v4", api)
            .merge(graphql::routes())
            .fallback(fallback)
    }
}

async fn fallback() -> axum::response::Response {
    common::route_not_found()
}
