//! Mock Sentry (self-hosted style, `/api/0`), as called by `@sentry/mcp-server` with
//! `--host=world:8113 --insecure-http`.
//!
//! Dataset collections (`dataset/sentry/<collection>.json`):
//!
//! | Collection | Key | Notes |
//! |---|---|---|
//! | `organizations` | slug | `{id, slug, name}` |
//! | `users` | id | org members; the **first** one is the token's user (`/auth/`). `teams`: team slugs |
//! | `teams` | slug | `{id, slug, name}` |
//! | `projects` | slug | `{id, slug, name, platform, teams: [slug], environments}` |
//! | `keys` | `<project>/<name>` | client keys (DSNs): `{id, name, project, public, secret, isActive}` |
//! | `releases` | version | `{version, projects: [slug], dateCreated, dateReleased, commits, deploys}` |
//! | `issues` | short ID | API-shaped issue with `project` as a slug and `firstRelease`/`lastRelease` as versions; optional `tags: {key: [{value, count}]}` |
//! | `events` | event ID | compact error events: `{eventID, groupID, project, dateCreated, title, message, level, release, environment, user, tags, exception: {type, value, frames}, breadcrumbs, request, trace}` |
//! | `activities` | `<issue id>/<id>` | issue activity and notes (written by updates) |
//! | `spans`, `logs` | id | flat trace items for the events API (`span.op`, `span.duration`, `trace`, `timestamp`, ...) |
//! | `autofix`, `seer_analyses` | issue id | Seer runs; prepared analyses (`{artifacts}`) used when a run starts |
//! | `monitors`, `uptime_monitors`, `workflows`, `detectors`, `metric_alerts`, `dashboards`, `replays`, `attachments`, `user_reports`, `external_issues`, `repos`, `conversations` | id | API-shaped documents |
//!
//! Times come only from the world clock; Discover aggregates are computed from `events`,
//! `spans` and `logs`.

// Helpers return early with ready-made Sentry error `Response`s.
#![allow(clippy::result_large_err)]

mod discover;
mod extras;
mod issues;
mod org;
mod releases;
mod search;
mod util;

use axum::{
    Router,
    response::Response,
    routing::{get, post},
};
use mock_core::{service::MockService, world::SharedWorld};

pub const NAME: &str = "sentry";
pub const PORT: u16 = 8113;

pub struct Sentry;

impl MockService for Sentry {
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
        Router::new().nest("/api/0", api()).fallback(fallback)
    }
}

async fn fallback() -> Response {
    util::not_found()
}

fn api() -> Router<SharedWorld> {
    const O: &str = "/organizations/{org}";
    const P: &str = "/projects/{org}/{project}";
    let o = |s: &str| format!("{O}{s}");
    let p = |s: &str| format!("{P}{s}");
    Router::new()
        // Identity and organizations
        .route("/auth/", get(org::whoami))
        .route("/users/me/", get(org::whoami))
        .route("/organizations/", get(org::list_orgs))
        .route(&o("/"), get(org::get_org))
        .route(&o("/teams/"), get(org::list_teams).post(org::create_team))
        .route(&o("/projects/"), get(org::list_projects))
        .route(&o("/members/"), get(org::list_members))
        .route(&o("/environments/"), get(org::list_environments))
        .route(&o("/tags/"), get(org::list_tags))
        .route(&o("/repos/"), get(org::list_repos))
        .route(&o("/code-mappings/bulk/"), post(org::bulk_code_mappings))
        .route(
            "/teams/{org}/{team}/projects/",
            get(org::list_team_projects).post(org::create_project),
        )
        // Projects and client keys
        .route(&p("/"), get(org::get_project).put(org::update_project))
        .route(&p("/teams/"), get(org::list_project_teams))
        .route(
            &p("/teams/{team}/"),
            post(org::add_project_team).delete(org::remove_project_team),
        )
        .route(&p("/keys/"), get(org::list_keys).post(org::create_key))
        .route(
            &p("/keys/{key}/"),
            get(org::get_key)
                .put(org::update_key)
                .delete(org::delete_key),
        )
        .route(&p("/environments/"), get(org::list_project_environments))
        // Issues
        .route(&o("/issues/"), get(issues::list_issues))
        .route(
            &o("/issues/{id}/"),
            get(issues::get_issue).put(issues::update_issue),
        )
        .route(&o("/issues/{id}/events/"), get(issues::list_issue_events))
        .route(
            &o("/issues/{id}/events/{event}/"),
            get(issues::get_issue_event),
        )
        .route(&o("/issues/{id}/tags/"), get(issues::list_issue_tags))
        .route(&o("/issues/{id}/tags/{key}/"), get(issues::get_issue_tag))
        .route(
            &o("/issues/{id}/tags/{key}/values/"),
            get(issues::list_issue_tag_values),
        )
        .route(&o("/issues/{id}/activities/"), get(issues::list_activities))
        .route(
            &o("/issues/{id}/notes/"),
            get(issues::list_notes).post(issues::create_note),
        )
        .route(
            &o("/issues/{id}/external-issues/"),
            get(issues::list_external_issues),
        )
        .route(
            &o("/issues/{id}/user-reports/"),
            get(issues::list_user_reports),
        )
        .route(
            &o("/issues/{id}/autofix/"),
            get(issues::get_autofix).post(issues::start_autofix),
        )
        .route(&o("/replay-count/"), get(issues::replay_count))
        .route(&p("/events/{event}/"), get(issues::get_project_event))
        // Events, traces
        .route(&o("/events/"), get(discover::events))
        .route(&o("/events/validate/"), get(discover::validate))
        .route(&o("/events-stats/"), get(discover::events_stats))
        .route(
            &o("/trace-items/attributes/"),
            get(discover::trace_item_attributes),
        )
        .route(&o("/trace-meta/{trace}/"), get(discover::trace_meta))
        .route(&o("/trace/{trace}/"), get(discover::trace))
        // Releases
        .route(&o("/releases/"), get(releases::list_org_releases))
        .route(&o("/releases/{version}/"), get(releases::get_org_release))
        .route(
            &o("/releases/{version}/deploys/"),
            get(releases::list_deploys),
        )
        .route(
            &o("/releases/{version}/commits/"),
            get(releases::list_org_commits),
        )
        .route(&p("/releases/"), get(releases::list_project_releases))
        .route(
            &p("/releases/{version}/"),
            get(releases::get_project_release),
        )
        .route(
            &p("/releases/{version}/commits/"),
            get(releases::list_project_commits),
        )
        // Cron and uptime monitors
        .route(&o("/monitors/"), get(extras::list_monitors))
        .route(&o("/monitors/{slug}/"), get(extras::get_monitor))
        .route(
            &o("/monitors/{slug}/checkins/"),
            get(extras::monitor_checkins),
        )
        .route(&o("/monitors/{slug}/stats/"), get(extras::monitor_stats))
        .route(&p("/monitors/{slug}/"), get(extras::get_project_monitor))
        .route(
            &p("/monitors/{slug}/checkins/"),
            get(extras::project_monitor_checkins),
        )
        .route(
            &p("/monitors/{slug}/stats/"),
            get(extras::project_monitor_stats),
        )
        .route(&o("/uptime/"), get(extras::list_uptime))
        .route(&p("/uptime/"), post(extras::create_uptime))
        .route(
            &p("/uptime/{id}/"),
            get(extras::get_uptime)
                .put(extras::update_uptime)
                .delete(extras::delete_uptime),
        )
        .route(&p("/uptime/{id}/checks/"), get(extras::uptime_checks))
        // Alerts and detectors
        .route(
            &o("/workflows/"),
            get(extras::list_workflows).post(extras::create_workflow),
        )
        .route(
            &o("/workflows/{id}/"),
            get(extras::get_workflow)
                .put(extras::update_workflow)
                .delete(extras::delete_workflow),
        )
        .route(
            &o("/workflows/{id}/project-scope/"),
            get(extras::workflow_project_scope),
        )
        .route(
            &o("/alert-rule-detector/"),
            get(extras::alert_rule_detector),
        )
        .route(&o("/detectors/"), get(extras::list_detectors))
        .route(
            &o("/detectors/{id}/"),
            get(extras::get_detector)
                .put(extras::update_detector)
                .delete(extras::delete_detector),
        )
        .route(
            &o("/projects/{project}/detectors/"),
            post(extras::create_detector),
        )
        .route(&o("/available-actions/"), get(extras::available_actions))
        .route(&o("/data-conditions/"), get(extras::data_conditions))
        .route(&o("/alert-rules/"), get(extras::list_metric_alerts))
        .route(&o("/alert-rules/{id}/"), get(extras::get_metric_alert))
        .route(&o("/combined-rules/"), get(extras::combined_rules))
        // Dashboards, replays, attachments, source links
        .route(&o("/dashboards/"), get(extras::list_dashboards))
        .route(&o("/dashboards/{id}/"), get(extras::get_dashboard))
        .route(&o("/replays/"), get(extras::list_replays))
        .route(&o("/replays/{id}/"), get(extras::get_replay))
        .route(
            &p("/replays/{id}/recording-segments/"),
            get(extras::replay_segments),
        )
        .route(
            &p("/events/{event}/attachments/"),
            get(extras::list_attachments),
        )
        .route(
            &p("/events/{event}/attachments/{id}/"),
            get(extras::get_attachment),
        )
        .route(&p("/stacktrace-link/"), get(extras::stacktrace_link))
        // Profiling, preprod snapshots, AI conversations, onboarding
        .route(&o("/profiling/flamegraph/"), get(extras::flamegraph))
        .route(&o("/profiling/chunks/"), get(extras::profile_chunks))
        .route(&p("/profiling/profiles/{id}/"), get(extras::profile))
        .route(
            &o("/preprodartifacts/snapshots/{id}/"),
            get(extras::org_not_found_2),
        )
        .route(
            &o("/preprodartifacts/snapshots/{id}/images/{image}/"),
            get(extras::org_not_found_3),
        )
        .route(
            &o("/agents/conversations/"),
            get(extras::list_conversations),
        )
        .route(
            &o("/agents/conversations/{id}/"),
            get(extras::get_conversation),
        )
        .route(
            &o("/onboarding/agent/status/"),
            post(extras::org_not_found_1),
        )
}
