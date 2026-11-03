//! Mock PagerDuty REST API v2 (plus the v3 shift-based schedules API), as called by
//! `pagerduty-mcp` with `PAGERDUTY_API_HOST` pointed at the world.
//!
//! Dataset collections (`dataset/pagerduty/<collection>.json`, keyed by PagerDuty ID unless
//! noted): `account` (`{"_key": "account", "subdomain", "user_id"}`: the API token owner
//! served at `/users/me`; defaults to the first user), `users`, `teams`, `team_members`
//! (`<team>/<user>` -> `{"role"}`), `services`, `escalation_policies`, `schedules` (v2),
//! `overrides` (`<schedule>/<n>`), `schedules_v3`, `v3_custom_shifts` and `v3_overrides`
//! (`<schedule>/<n>`), `incidents`, `notes` (`<incident>/<n>`), `alerts`, `log_entries`,
//! `status_updates` (`<incident>/<n>`), `change_events`, `priorities`, `business_services`,
//! `service_dependencies`, `event_orchestrations`, `orchestration_routers`,
//! `orchestration_globals`, `service_orchestrations` (by service ID), `incident_workflows`,
//! `incident_workflow_instances`, `status_pages`, `status_page_severities`,
//! `status_page_impacts`, `status_page_statuses`, `status_page_services`,
//! `status_page_posts`, `status_page_post_updates`, `webhook_subscriptions`,
//! `extension_schemas`, `alert_grouping_settings`.
//!
//! Stored documents may be minimal; responses fill in PagerDuty's server-generated fields
//! (`type`, `summary`, `self`, `html_url`, full references, defaults).

mod analytics;
mod config;
mod directory;
mod incidents;
mod records;
mod schedules;
mod schedules_v3;
mod status_pages;
mod util;

use axum::{
    Router,
    response::Response,
    routing::{get, post, put},
};
use mock_core::{service::MockService, world::SharedWorld};

pub const NAME: &str = "pagerduty";
pub const PORT: u16 = 8111;

pub struct Pagerduty;

impl MockService for Pagerduty {
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
        use analytics as an;
        use config as cf;
        use directory as d;
        use incidents as i;
        use records as r;
        use schedules as s;
        use schedules_v3 as v3;
        use status_pages as sp;

        Router::new()
            // Users and teams
            .route("/users/me", get(d::get_me))
            .route("/users", get(d::list_users).post(d::create_user))
            .route("/users/{id}", get(d::get_user))
            .route("/teams", get(d::list_teams).post(d::create_team))
            .route(
                "/teams/{id}",
                get(d::get_team).put(d::update_team).delete(d::delete_team),
            )
            .route("/teams/{id}/members", get(d::list_team_members))
            .route(
                "/teams/{id}/users/{user_id}",
                put(d::add_team_member).delete(d::remove_team_member),
            )
            // Services, escalation policies, priorities, business services
            .route("/services", get(d::list_services).post(d::create_service))
            .route(
                "/services/{id}",
                get(d::get_service)
                    .put(d::update_service)
                    .delete(d::delete_service),
            )
            .route(
                "/services/{id}/change_events",
                get(r::list_service_change_events),
            )
            .route(
                "/service_dependencies/technical_services/{id}",
                get(d::technical_service_dependencies),
            )
            .route(
                "/service_dependencies/business_services/{id}",
                get(d::business_service_dependencies),
            )
            .route("/business_services", get(d::list_business_services))
            .route("/business_services/{id}", get(d::get_business_service))
            .route("/escalation_policies", get(d::list_escalation_policies))
            .route("/escalation_policies/{id}", get(d::get_escalation_policy))
            .route("/priorities", get(d::list_priorities))
            // Schedules and on-calls
            .route("/schedules", get(s::list_schedules))
            .route("/schedules/{id}", get(s::get_schedule))
            .route("/schedules/{id}/users", get(s::list_schedule_users))
            .route(
                "/schedules/{id}/overrides",
                get(s::list_overrides).post(s::create_overrides),
            )
            .route(
                "/schedules/{id}/overrides/{override_id}",
                axum::routing::delete(s::delete_override),
            )
            .route("/oncalls", get(s::list_oncalls))
            .route("/v3/schedules", get(v3::list).post(v3::create))
            .route(
                "/v3/schedules/{id}",
                get(v3::get).put(v3::update).delete(v3::delete),
            )
            .route(
                "/v3/schedules/{id}/rotations",
                get(v3::list_rotations).post(v3::create_rotation),
            )
            .route(
                "/v3/schedules/{id}/rotations/{rotation_id}",
                get(v3::get_rotation).delete(v3::delete_rotation),
            )
            .route(
                "/v3/schedules/{id}/rotations/{rotation_id}/events",
                get(v3::list_events).post(v3::create_event),
            )
            .route(
                "/v3/schedules/{id}/rotations/{rotation_id}/events/{event_id}",
                get(v3::get_event)
                    .put(v3::update_event)
                    .delete(v3::delete_event),
            )
            .route(
                "/v3/schedules/{id}/custom_shifts",
                get(v3::list_custom_shifts).post(v3::create_custom_shifts),
            )
            .route(
                "/v3/schedules/{id}/custom_shifts/{custom_shift_id}",
                get(v3::get_custom_shift)
                    .put(v3::update_custom_shift)
                    .delete(v3::delete_custom_shift),
            )
            .route(
                "/v3/schedules/{id}/overrides",
                get(v3::list_overrides).post(v3::create_overrides),
            )
            .route(
                "/v3/schedules/{id}/overrides/{override_id}",
                get(v3::get_override)
                    .put(v3::update_override)
                    .delete(v3::delete_override),
            )
            // Incidents
            .route(
                "/incidents",
                get(i::list_incidents)
                    .post(i::create_incident)
                    .put(i::update_incidents),
            )
            .route(
                "/incidents/{id}",
                get(i::get_incident).put(i::update_incident),
            )
            .route(
                "/incidents/{id}/notes",
                get(i::list_notes).post(i::create_note),
            )
            .route("/incidents/{id}/alerts", get(i::list_alerts))
            .route("/incidents/{id}/alerts/{alert_id}", get(i::get_alert))
            .route("/incidents/{id}/log_entries", get(i::incident_log_entries))
            .route(
                "/incidents/{id}/responder_requests",
                post(i::create_responder_request),
            )
            .route("/incidents/{id}/snooze", post(i::snooze))
            .route("/incidents/{id}/merge", put(i::merge))
            .route(
                "/incidents/{id}/status_updates",
                post(i::create_status_update),
            )
            .route("/incidents/{id}/outlier_incident", get(i::outlier_incident))
            .route("/incidents/{id}/past_incidents", get(i::past_incidents))
            .route(
                "/incidents/{id}/related_incidents",
                get(i::related_incidents),
            )
            .route(
                "/incidents/{id}/related_change_events",
                get(i::related_change_events),
            )
            .route("/log_entries", get(r::list_log_entries))
            .route("/log_entries/{id}", get(r::get_log_entry))
            .route("/change_events", get(r::list_change_events))
            .route("/change_events/{id}", get(r::get_change_event))
            // Event orchestrations and workflows
            .route("/event_orchestrations", get(cf::list_orchestrations))
            .route("/event_orchestrations/{id}", get(cf::get_orchestration))
            .route(
                "/event_orchestrations/{id}/router",
                get(cf::get_router).put(cf::update_router),
            )
            .route("/event_orchestrations/{id}/global", get(cf::get_global))
            .route(
                "/event_orchestrations/services/{service_id}",
                get(cf::get_service_orchestration),
            )
            .route(
                "/event_orchestrations/services/{service_id}/active",
                get(cf::get_service_orchestration_active),
            )
            .route("/incident_workflows", get(cf::list_workflows))
            .route("/incident_workflows/{id}", get(cf::get_workflow))
            .route(
                "/incident_workflows/{id}/instances",
                post(cf::start_workflow),
            )
            .route(
                "/alert_grouping_settings",
                get(cf::list_groupings).post(cf::create_grouping),
            )
            .route(
                "/alert_grouping_settings/{id}",
                get(cf::get_grouping)
                    .put(cf::update_grouping)
                    .delete(cf::delete_grouping),
            )
            .route(
                "/webhook_subscriptions",
                get(cf::list_webhooks).post(cf::create_webhook),
            )
            .route(
                "/webhook_subscriptions/{id}",
                get(cf::get_webhook)
                    .put(cf::update_webhook)
                    .delete(cf::delete_webhook),
            )
            .route("/extension_schemas", get(cf::list_extension_schemas))
            .route("/extension_schemas/{id}", get(cf::get_extension_schema))
            // Status pages
            .route("/status_pages", get(sp::list_pages))
            .route("/status_pages/{id}/severities", get(sp::list_severities))
            .route("/status_pages/{id}/impacts", get(sp::list_impacts))
            .route("/status_pages/{id}/statuses", get(sp::list_statuses))
            .route("/status_pages/{id}/services", get(sp::list_page_services))
            .route(
                "/status_pages/{id}/posts",
                get(sp::list_posts).post(sp::create_post),
            )
            .route("/status_pages/{id}/posts/{post_id}", get(sp::get_post))
            .route(
                "/status_pages/{id}/posts/{post_id}/post_updates",
                get(sp::list_post_updates).post(sp::create_post_update),
            )
            // Analytics
            .route("/analytics/metrics/incidents/all", post(an::incidents_all))
            .route(
                "/analytics/metrics/incidents/services",
                post(an::incidents_by_service),
            )
            .route(
                "/analytics/metrics/incidents/teams",
                post(an::incidents_by_team),
            )
            .route(
                "/analytics/metrics/responders/all",
                post(an::responders_all),
            )
            .route(
                "/analytics/metrics/responders/teams",
                post(an::responders_by_team),
            )
            .route("/analytics/raw/incidents", post(an::raw_incidents))
            .route("/analytics/raw/incidents/{id}", get(an::raw_incident_by_id))
            .fallback(not_found)
            .method_not_allowed_fallback(not_found)
    }
}

async fn not_found() -> Response {
    util::not_found()
}
