//! Mock Datadog API (`https://api.datadoghq.com`, real-tls mode), as called by
//! `@winor30/mcp-server-datadog` through `@datadog/datadog-api-client`.
//!
//! Collections (dataset `dataset/datadog/<collection>.json`):
//!
//! | Collection | Key | Document |
//! |---|---|---|
//! | `incidents` | incident UUID | v2 `IncidentResponseData` |
//! | `users` | user UUID | v2 `IncidentUserData` (for `include=users`) |
//! | `monitors` | numeric id | v1 `Monitor` |
//! | `dashboards` | dashboard id (`abc-def-ghi`) | v1 `Dashboard` |
//! | `hosts` | host name (`_key`) | v1 `Host` |
//! | `host_mutes` | host name | `{hostname, message, end, muted_at}` written by mute |
//! | `downtimes` | numeric id | v1 `Downtime` |
//! | `metrics` | `_key` | `{metric, tags, unit?, interval, points: [[epoch_s, value]]}` |
//! | `logs` | log id | v2 `Log` |
//! | `spans` | span event id | v2 `Span` |
//! | `rum_applications` | application id | v2 `RUMApplicationList` |
//! | `rum_events` | event id | v2 `RUMEvent` |

// Validation helpers return the ready-made Datadog error response as their `Err`.
#![allow(clippy::result_large_err)]

mod dashboards;
mod downtimes;
mod hosts;
mod incidents;
mod metrics;
mod monitors;
mod search;
mod support;
mod timeparse;

use axum::{
    Router,
    extract::Request,
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
};
use mock_core::{service::MockService, world::SharedWorld};

use support::errors;

pub const NAME: &str = "datadog";
pub const PORT: u16 = 8112;
pub const BASE_URL: &str = "https://api.datadoghq.com";

pub struct Datadog;

impl MockService for Datadog {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port(&self) -> u16 {
        PORT
    }

    fn hosts(&self) -> &'static [&'static str] {
        &["api.datadoghq.com"]
    }

    fn router(&self) -> Router<SharedWorld> {
        Router::new()
            // v2 incidents
            .route("/api/v2/incidents", get(incidents::list))
            .route("/api/v2/incidents/{incident_id}", get(incidents::get))
            // v1 metrics
            .route("/api/v1/query", get(metrics::query))
            // v2 logs / spans / RUM search
            .route("/api/v2/logs/events/search", post(search::logs_search))
            .route("/api/v2/spans/events/search", post(search::spans_search))
            .route("/api/v2/rum/events", get(search::rum_list))
            .route("/api/v2/rum/applications", get(search::rum_applications))
            // v1 monitors
            .route("/api/v1/monitor", get(monitors::list))
            .route("/api/v1/monitor/{monitor_id}", get(monitors::get))
            // v1 dashboards
            .route("/api/v1/dashboard", get(dashboards::list))
            .route("/api/v1/dashboard/{dashboard_id}", get(dashboards::get))
            // v1 hosts
            .route("/api/v1/hosts", get(hosts::list))
            .route("/api/v1/hosts/totals", get(hosts::totals))
            .route("/api/v1/host/{host_name}/mute", post(hosts::mute))
            .route("/api/v1/host/{host_name}/unmute", post(hosts::unmute))
            // v1 downtimes
            .route(
                "/api/v1/downtime",
                get(downtimes::list).post(downtimes::create),
            )
            .route(
                "/api/v1/downtime/{downtime_id}",
                get(downtimes::get).delete(downtimes::cancel),
            )
            .fallback(not_found)
            .layer(middleware::from_fn(require_keys))
    }
}

async fn not_found() -> Response {
    errors::not_found()
}

/// Datadog answers 403 `{"errors":["Forbidden"]}` when either key header is missing.
async fn require_keys(request: Request, next: Next) -> Response {
    if !has_keys(request.headers()) {
        return errors::forbidden();
    }
    next.run(request).await
}

fn has_keys(headers: &axum::http::HeaderMap) -> bool {
    let has = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| !v.trim().is_empty())
    };
    has("dd-api-key") && has("dd-application-key")
}
