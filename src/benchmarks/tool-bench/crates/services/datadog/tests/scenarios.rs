//! The `ops/datadog/*` scenario datasets answer the way their expectations assume.

use std::path::Path;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mock_core::{
    dataset::Dataset,
    serve::service_app,
    world::{SharedWorld, WorldState, shared},
};
use mock_datadog::Datadog;
use serde_json::{Value, json};
use tower::ServiceExt;

fn scenario(slug: &str) -> (SharedWorld, Router) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../scenarios/ops/datadog")
        .join(slug);
    let w = shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap());
    let app = service_app(w.clone(), &Datadog);
    (w, app)
}

async fn call(app: &Router, method: &str, uri: &str, body: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("DD-API-KEY", "k")
        .header("DD-APPLICATION-KEY", "a")
        .body(Body::from(body.to_owned()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn peak(series: &Value) -> (f64, i64) {
    series["pointlist"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p[1].as_f64().unwrap(), p[0].as_f64().unwrap() as i64 / 1000))
        .fold((f64::MIN, 0), |a, b| if b.0 > a.0 { b } else { a })
}

#[tokio::test]
async fn checkout_p95_peak_is_1843_at_1435_only_for_prod_in_the_window() {
    let (_, app) = scenario("checkout-p95-peak");
    let window = "from=1793282400&to=1793286000";
    let (_, prod) = call(
        &app,
        "GET",
        &format!("/api/v1/query?{window}&query=max:checkout.api.latency.p95%7Benv:prod%7D"),
        "",
    )
    .await;
    // 14:35 UTC on 2026-10-29.
    assert_eq!(peak(&prod["series"][0]), (1843.0, 1_793_284_500));
    let (_, avg) = call(&app, "GET", &format!("/api/v1/query?{window}&query=avg:checkout.api.latency.p95%7Benv:prod,service:checkout-api%7D"), "").await;
    assert_eq!(
        peak(&avg["series"][0]).0,
        1843.0,
        "single series: the space aggregator doesn't matter"
    );
    let (_, both) = call(
        &app,
        "GET",
        &format!("/api/v1/query?{window}&query=max:checkout.api.latency.p95%7B*%7D"),
        "",
    )
    .await;
    assert_eq!(
        peak(&both["series"][0]).0,
        2210.0,
        "without env the staging spike wins"
    );
    let (_, wide) = call(&app, "GET", "/api/v1/query?from=1793278800&to=1793289600&query=max:checkout.api.latency.p95%7Benv:prod%7D", "").await;
    assert_eq!(
        peak(&wide["series"][0]).0,
        2950.0,
        "outside the window prod was higher"
    );
}

#[tokio::test]
async fn replica_downtime_gets_the_next_id() {
    let (w, app) = scenario("schedule-replica-downtime");
    let (_, monitors) = call(&app, "GET", "/api/v1/monitor?name=replication%20lag", "").await;
    let names: Vec<&str> = monitors
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "Postgres replication lag",
            "Postgres replication lag (staging)",
            "MySQL replication lag"
        ]
    );
    let body = json!({"scope": ["host:pg-prod-replica-2"], "monitor_id": 7301, "start": 1793412000, "end": 1793419200, "message": "CHG-5012"});
    let (status, created) = call(&app, "POST", "/api/v1/downtime", &body.to_string()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created["id"], 2318857);
    let state = w.lock().unwrap().snapshot();
    assert_eq!(
        state["services"]["datadog"]["downtimes"]["2318857"]["monitor_id"],
        7301
    );
}

#[tokio::test]
async fn called_off_maintenance_has_one_live_prod_chg_4410_downtime() {
    let (_, app) = scenario("cancel-called-off-maintenance");
    let (_, list) = call(&app, "GET", "/api/v1/downtime", "").await;
    let live: Vec<i64> = list
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| {
            d["message"].as_str().unwrap().contains("CHG-4410")
                && d["disabled"] == false
                && d["scope"].as_array().unwrap().contains(&json!("env:prod"))
        })
        .map(|d| d["id"].as_i64().unwrap())
        .collect();
    assert_eq!(live, [2207731]);
    let (_, current) = call(&app, "GET", "/api/v1/downtime?current_only=true", "").await;
    assert_eq!(
        current
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["id"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [2207760]
    );
    let (status, _) = call(&app, "DELETE", "/api/v1/downtime/2207731", "").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn error_logs_point_at_checkout_api_prod_1b() {
    let (w, app) = scenario("mute-host-from-error-logs");
    let body = json!({"filter": {"query": "env:prod \"connection reset by peer\"", "from": "1793365200000", "to": "1793370600000"}, "page": {"limit": 100}, "sort": "-timestamp"});
    let (_, logs) = call(
        &app,
        "POST",
        "/api/v2/logs/events/search",
        &body.to_string(),
    )
    .await;
    let mut hosts: Vec<&str> = logs["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["attributes"]["host"].as_str().unwrap())
        .collect();
    hosts.sort_unstable();
    hosts.dedup();
    assert_eq!(hosts, ["checkout-api-prod-1b", "payments-api-prod-1a"]);
    assert!(
        logs["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|l| l["attributes"]["service"] == "checkout-api")
            .all(|l| l["attributes"]["host"] == "checkout-api-prod-1b")
    );

    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/host/checkout-api-prod-1b/mute",
        r#"{"end": 1793383200, "message": "INC-2291", "override": false}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/host/payments-api-prod-1a/mute",
        r#"{"override": false}"#,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "already muted by someone else"
    );
    let state = w.lock().unwrap().snapshot();
    assert_eq!(
        state["services"]["datadog"]["hosts"]["checkout-api-prod-1b"]["mute_timeout"],
        1793383200
    );
}
