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

/// Fixture `now`: 2026-10-30T15:00:00Z.
const NOW: i64 = 1_793_372_400;

fn world() -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture");
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

fn app() -> (SharedWorld, Router) {
    let w = world();
    let app = service_app(w.clone(), &Datadog);
    (w, app)
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    body: &str,
    keys: bool,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "api.datadoghq.com")
        .header("content-type", "application/json");
    if keys {
        req = req
            .header("DD-API-KEY", "dd-api-toolbench")
            .header("DD-APPLICATION-KEY", "dd-app-toolbench");
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn call(app: &Router, method: &str, uri: &str, body: &str) -> (StatusCode, Value) {
    send(app, method, uri, body, true).await
}

fn ids(v: &Value) -> Vec<Value> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].clone())
        .collect()
}

fn state(w: &SharedWorld) -> Value {
    w.lock().unwrap().snapshot()["services"]["datadog"].clone()
}

#[tokio::test]
async fn missing_keys_are_forbidden_and_unknown_routes_404() {
    let (_, app) = app();
    let (status, body) = send(&app, "GET", "/api/v1/monitor", "", false).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body, json!({"errors": ["Forbidden"]}));
    let (status, body) = call(&app, "GET", "/api/v1/nope", "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, json!({"errors": ["Not found"]}));
}

#[tokio::test]
async fn incidents_list_paginates_with_offsets_and_includes_users() {
    let (_, app) = app();
    let (status, body) = call(
        &app,
        "GET",
        "/api/v2/incidents?page%5Bsize%5D=2&page%5Boffset%5D=0",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().unwrap().len(), 2);
    assert_eq!(body["data"][0]["attributes"]["public_id"], 412);
    assert_eq!(
        body["meta"]["pagination"],
        json!({"next_offset": 2, "offset": 0, "size": 2})
    );
    let (_, page2) = call(
        &app,
        "GET",
        "/api/v2/incidents?page[size]=2&page[offset]=2&include=users",
        "",
    )
    .await;
    assert_eq!(page2["data"].as_array().unwrap().len(), 1);
    assert_eq!(page2["included"][0]["type"], "users");
    let (status, body) = call(&app, "GET", "/api/v2/incidents?page[size]=500", "").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["errors"][0].as_str().unwrap().contains("page[size]"));
}

#[tokio::test]
async fn incident_get_and_404() {
    let (_, app) = app();
    let (status, body) = call(
        &app,
        "GET",
        "/api/v2/incidents/a1f0c2d4-1b2c-5d6e-8f90-0a1b2c3d4e02",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["data"]["attributes"]["title"],
        "Search latency regression"
    );
    let (status, body) = call(
        &app,
        "GET",
        "/api/v2/incidents/00000000-0000-0000-0000-000000000000",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, json!({"errors": ["Not found"]}));
}

#[tokio::test]
async fn metrics_query_filters_scope_aggregates_and_rolls_up() {
    let (_, app) = app();
    let from = NOW - 3600;
    let q = |query: &str| {
        format!(
            "/api/v1/query?from={from}&to={NOW}&query={}",
            query
                .replace('{', "%7B")
                .replace('}', "%7D")
                .replace(',', "%2C")
                .replace(' ', "%20")
        )
    };
    let (status, body) = call(
        &app,
        "GET",
        &q("max:checkout.api.latency.p95{env:prod}"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["from_date"], from * 1000);
    let series = body["series"].as_array().unwrap();
    assert_eq!(series.len(), 1);
    assert_eq!(series[0]["length"], 13);
    assert_eq!(series[0]["interval"], 300);
    assert_eq!(series[0]["scope"], "env:prod");
    assert_eq!(series[0]["unit"][0]["short_name"], "ms");
    assert_eq!(
        series[0]["pointlist"][6],
        json!([((from + 1800) * 1000) as f64, 1430.0])
    );
    // avg across both prod hosts
    let (_, body) = call(
        &app,
        "GET",
        &q("avg:checkout.api.latency.p95{env:prod}"),
        "",
    )
    .await;
    assert_eq!(body["series"][0]["pointlist"][6][1], 1380.0);
    // group by host and rollup into one bucket per hour
    let (_, body) = call(
        &app,
        "GET",
        &q("max:checkout.api.latency.p95{service:checkout-api} by {host}.rollup(max, 3600)"),
        "",
    )
    .await;
    let series = body["series"].as_array().unwrap();
    assert_eq!(series.len(), 3);
    assert_eq!(series[0]["tag_set"], json!(["host:web-prod-01"]));
    assert_eq!(series[0]["pointlist"][0][1], 1430.0);
    assert_eq!(series[2]["pointlist"][0][1], 2100.0);
    assert_eq!(body["group_by"], json!(["host"]));
    // no data -> empty series; bad query -> 400
    let (status, body) = call(&app, "GET", &q("avg:nothing.here{*}"), "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["series"], json!([]));
    let (status, body) = call(&app, "GET", &q("avg:checkout.api.latency.p95"), "").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["errors"][0]
            .as_str()
            .unwrap()
            .starts_with("Error parsing query")
    );
    let (status, _) = call(&app, "GET", "/api/v1/query?query=avg:x%7B*%7D", "").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

fn logs_body(query: &str, limit: u64, cursor: Option<&str>) -> String {
    let mut page = json!({"limit": limit});
    if let Some(c) = cursor {
        page["cursor"] = json!(c);
    }
    json!({
        "filter": {"query": query, "from": format!("{}", (NOW - 3600) * 1000), "to": format!("{}", NOW * 1000)},
        "page": page,
        "sort": "-timestamp"
    })
    .to_string()
}

#[tokio::test]
async fn logs_search_filters_sorts_and_paginates_with_cursor() {
    let (_, app) = app();
    let (status, body) = call(
        &app,
        "POST",
        "/api/v2/logs/events/search",
        &logs_body("service:checkout-api status:error env:prod", 100, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        ids(&body["data"]),
        [json!("AwAAAZK1aa0002"), json!("AwAAAZK1aa0001")]
    );
    assert_eq!(body["meta"]["status"], "done");
    assert!(body.get("links").is_none());

    let (_, body) = call(
        &app,
        "POST",
        "/api/v2/logs/events/search",
        &logs_body("\"connection reset\" -host:web-staging-01", 100, None),
    )
    .await;
    assert_eq!(body["data"].as_array().unwrap().len(), 2);
    let (_, body) = call(
        &app,
        "POST",
        "/api/v2/logs/events/search",
        &logs_body("@http.status_code:>=500", 100, None),
    )
    .await;
    assert_eq!(body["data"].as_array().unwrap().len(), 3);

    let (_, page1) = call(
        &app,
        "POST",
        "/api/v2/logs/events/search",
        &logs_body("*", 2, None),
    )
    .await;
    assert_eq!(
        ids(&page1["data"]),
        [json!("AwAAAZK1aa0005"), json!("AwAAAZK1aa0004")]
    );
    let cursor = page1["meta"]["page"]["after"].as_str().unwrap().to_owned();
    assert!(
        page1["links"]["next"]
            .as_str()
            .unwrap()
            .starts_with("https://api.datadoghq.com/api/v2/logs/events?page[cursor]=")
    );
    let (_, page2) = call(
        &app,
        "POST",
        "/api/v2/logs/events/search",
        &logs_body("*", 2, Some(&cursor)),
    )
    .await;
    assert_eq!(
        ids(&page2["data"]),
        [json!("AwAAAZK1aa0003"), json!("AwAAAZK1aa0002")]
    );

    // Default window is the last 15 minutes.
    let (_, recent) = call(&app, "POST", "/api/v2/logs/events/search", "{}").await;
    assert_eq!(ids(&recent["data"]), [json!("AwAAAZK1aa0005")]);

    let (status, body) = call(
        &app,
        "POST",
        "/api/v2/logs/events/search",
        &logs_body("service:(a OR", 10, None),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["errors"][0]
            .as_str()
            .unwrap()
            .starts_with("Invalid query")
    );
}

#[tokio::test]
async fn monitors_list_filters_by_name_scope_tags_and_group_states() {
    let (_, app) = app();
    let (status, body) = call(&app, "GET", "/api/v1/monitor", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        ids(&body),
        [json!(5001), json!(5002), json!(5003), json!(5004)]
    );
    assert!(body[0].get("state").is_none());

    let (_, body) = call(&app, "GET", "/api/v1/monitor?name=checkout%20api%20p95", "").await;
    assert_eq!(ids(&body), [json!(5001), json!(5003)]);
    let (_, body) = call(
        &app,
        "GET",
        "/api/v1/monitor?tags=env%3Aprod%2Cservice%3Acheckout-api",
        "",
    )
    .await;
    assert_eq!(ids(&body), [json!(5001), json!(5002)]);
    let (_, body) = call(
        &app,
        "GET",
        "/api/v1/monitor?monitor_tags=team%3Aplatform",
        "",
    )
    .await;
    assert_eq!(ids(&body), [json!(5004)]);
    let (_, body) = call(
        &app,
        "GET",
        "/api/v1/monitor?group_states=warn&with_downtimes=true&page=0&page_size=2",
        "",
    )
    .await;
    assert_eq!(ids(&body), [json!(5001), json!(5002)]);
    assert_eq!(body[1]["state"]["groups"].as_object().unwrap().len(), 1);
    assert_eq!(body[0]["matching_downtimes"][0]["id"], 3100203);
    let (_, body) = call(&app, "GET", "/api/v1/monitor?page=1&page_size=2", "").await;
    assert_eq!(ids(&body), [json!(5003), json!(5004)]);
    let (status, _) = call(&app, "GET", "/api/v1/monitor?group_states=bogus", "").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body) = call(&app, "GET", "/api/v1/monitor/5002", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["overall_state"], "Warn");
    let (status, body) = call(&app, "GET", "/api/v1/monitor/999", "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, json!({"errors": ["Monitor not found"]}));
}

#[tokio::test]
async fn dashboards_list_summaries_and_get() {
    let (_, app) = app();
    let (status, body) = call(
        &app,
        "GET",
        "/api/v1/dashboard?filter%5Bshared%5D=false",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let list = body["dashboards"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["id"], "x7k-9p2-mq4");
    assert!(list[0].get("widgets").is_none());
    let (_, body) = call(&app, "GET", "/api/v1/dashboard?filter[deleted]=true", "").await;
    assert_eq!(body["dashboards"][0]["id"], "old-123-del");
    let (_, body) = call(&app, "GET", "/api/v1/dashboard?count=1&start=1", "").await;
    assert_eq!(body["dashboards"][0]["id"], "b2c-4rt-8zz");

    let (status, body) = call(&app, "GET", "/api/v1/dashboard/x7k-9p2-mq4", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["widgets"][0]["definition"]["type"], "timeseries");
    assert!(body.get("is_shared").is_none());
    let (status, body) = call(&app, "GET", "/api/v1/dashboard/nope-nop-nop", "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body,
        json!({"errors": ["Dashboard with ID nope-nop-nop not found"]})
    );
}

#[tokio::test]
async fn spans_search_filters_and_validates() {
    let (_, app) = app();
    let body = |query: &str, limit: u64| {
        json!({"data": {"attributes": {"filter": {"query": query, "from": "2026-10-30T14:00:00.000Z", "to": "2026-10-30T15:00:00.000Z"}, "sort": "-timestamp", "page": {"limit": limit}}, "type": "search_request"}}).to_string()
    };
    let (status, res) = call(
        &app,
        "POST",
        "/api/v2/spans/events/search",
        &body("* service:checkout-api", 100),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        ids(&res["data"]),
        [json!("AAAAAZK11002"), json!("AAAAAZK11001")]
    );
    let (_, res) = call(
        &app,
        "POST",
        "/api/v2/spans/events/search",
        &body("@status:error operation:express.request", 100),
    )
    .await;
    assert_eq!(ids(&res["data"]), [json!("AAAAAZK11001")]);
    let (_, res) = call(&app, "POST", "/api/v2/spans/events/search", &body("*", 1)).await;
    assert_eq!(res["data"].as_array().unwrap().len(), 1);
    assert!(res["meta"]["page"]["after"].is_string());
    let (status, _) = call(
        &app,
        "POST",
        "/api/v2/spans/events/search",
        r#"{"data":{"type":"aggregate_request"}}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(&app, "POST", "/api/v2/spans/events/search", "{}").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn hosts_list_filters_sorts_and_paginates() {
    let (_, app) = app();
    let (status, body) = call(&app, "GET", "/api/v1/hosts", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total_matching"], 4);
    assert_eq!(body["host_list"][0]["name"], "db-prod-01");
    let (_, body) = call(
        &app,
        "GET",
        "/api/v1/hosts?filter=env%3Aprod&sort_field=cpu&sort_dir=desc&count=2",
        "",
    )
    .await;
    let names: Vec<&str> = body["host_list"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["web-prod-01", "web-prod-02"]);
    assert_eq!(body["total_matching"], 3);
    assert_eq!(body["total_returned"], 2);
    let (_, body) = call(&app, "GET", "/api/v1/hosts?filter=web&start=2", "").await;
    assert_eq!(body["host_list"][0]["name"], "web-staging-01");
    let (_, body) = call(
        &app,
        "GET",
        &format!(
            "/api/v1/hosts?from={}&include_hosts_metadata=false",
            NOW - 3600
        ),
        "",
    )
    .await;
    assert_eq!(body["total_matching"], 3);
    assert!(body["host_list"][0].get("meta").is_none());
    let (status, _) = call(&app, "GET", "/api/v1/hosts?sort_dir=sideways", "").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (_, totals) = call(&app, "GET", "/api/v1/hosts/totals", "").await;
    assert_eq!(totals, json!({"total_active": 3, "total_up": 3}));
    let (_, totals) = call(&app, "GET", "/api/v1/hosts/totals?from=7200", "").await;
    assert_eq!(totals, json!({"total_active": 4, "total_up": 3}));
}

#[tokio::test]
async fn mute_and_unmute_hosts_update_state() {
    let (w, app) = app();
    let end = NOW + 3 * 3600;
    let (status, body) = call(
        &app,
        "POST",
        "/api/v1/host/web-prod-01/mute",
        &json!({"message": "patching CHG-1", "end": end, "override": false}).to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"action": "Muted", "end": end, "hostname": "web-prod-01", "message": "patching CHG-1"})
    );
    let s = state(&w);
    assert_eq!(s["hosts"]["web-prod-01"]["is_muted"], true);
    assert_eq!(s["hosts"]["web-prod-01"]["mute_timeout"], end);
    assert_eq!(s["host_mutes"]["web-prod-01"]["message"], "patching CHG-1");
    let (_, list) = call(&app, "GET", "/api/v1/hosts?filter=web-prod-01", "").await;
    assert_eq!(list["host_list"][0]["is_muted"], true);

    // Already muted without override, past end, unknown host.
    let (status, body) = call(
        &app,
        "POST",
        "/api/v1/host/web-prod-02/mute",
        r#"{"override": false}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["errors"][0]
            .as_str()
            .unwrap()
            .contains("already muted")
    );
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/host/web-prod-02/mute",
        &json!({"override": true, "end": NOW + 60}).to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/host/web-staging-01/mute",
        &json!({"end": NOW - 60}).to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = call(&app, "POST", "/api/v1/host/ghost/mute", "{}").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, json!({"errors": ["Host ghost not found"]}));

    let (status, body) = call(&app, "POST", "/api/v1/host/web-prod-01/unmute", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"action": "Unmuted", "hostname": "web-prod-01"})
    );
    let s = state(&w);
    assert_eq!(s["hosts"]["web-prod-01"]["is_muted"], false);
    assert!(s["host_mutes"].get("web-prod-01").is_none());
    let (status, _) = call(&app, "POST", "/api/v1/host/web-prod-01/unmute", "").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn downtimes_list_create_and_cancel() {
    let (w, app) = app();
    let (status, body) = call(&app, "GET", "/api/v1/downtime", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ids(&body), [json!(3100201), json!(3100202), json!(3100203)]);
    assert_eq!(body[0]["active"], true);
    let (_, current) = call(&app, "GET", "/api/v1/downtime?current_only=true", "").await;
    assert_eq!(ids(&current), [json!(3100201)]);

    let start = NOW + 3600;
    let input = json!({"scope": ["host:web-prod-01"], "start": start, "end": start + 7200, "message": "Kernel patching CHG-77", "monitor_id": 5002, "recurrence": {"type": "weeks", "period": 1, "week_days": ["Sat"]}});
    let (status, created) = call(&app, "POST", "/api/v1/downtime", &input.to_string()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created["id"], 3100204);
    assert_eq!(created["monitor_id"], 5002);
    assert_eq!(created["scope"], json!(["host:web-prod-01"]));
    assert_eq!(created["active"], false);
    assert_eq!(created["recurrence"]["week_days"], json!(["Sat"]));
    assert_eq!(created["monitor_tags"], json!(["*"]));
    assert_eq!(
        state(&w)["downtimes"]["3100204"]["message"],
        "Kernel patching CHG-77"
    );

    let (_, split) = call(
        &app,
        "POST",
        "/api/v1/downtime",
        r#"{"scope": ["host:web-prod-01, env:prod"]}"#,
    )
    .await;
    assert_eq!(split["id"], 3100205);
    assert_eq!(split["scope"], json!(["host:web-prod-01", "env:prod"]));
    assert_eq!(
        split["start"],
        NOW + 14,
        "start defaults to now (two POST ticks in)"
    );

    let (status, got) = call(&app, "GET", "/api/v1/downtime/3100204", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["end"], start + 7200);

    for (bad, needle) in [
        (json!({"start": start}), "scope"),
        (
            json!({"scope": ["env:prod"], "start": start, "end": start - 1}),
            "after",
        ),
        (
            json!({"scope": ["env:prod"], "monitor_id": 42}),
            "Monitor 42",
        ),
    ] {
        let (status, body) = call(&app, "POST", "/api/v1/downtime", &bad.to_string()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
        assert!(
            body["errors"][0].as_str().unwrap().contains(needle),
            "{body}"
        );
    }

    let (status, body) = call(&app, "DELETE", "/api/v1/downtime/3100201", "").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(body, Value::Null);
    let s = state(&w);
    assert_eq!(s["downtimes"]["3100201"]["disabled"], true);
    assert!(s["downtimes"]["3100201"]["canceled"].is_i64());
    let (_, current) = call(&app, "GET", "/api/v1/downtime?current_only=true", "").await;
    assert_eq!(ids(&current), [json!(3100205)]);
    let (status, body) = call(&app, "DELETE", "/api/v1/downtime/999", "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, json!({"errors": ["Downtime not found"]}));
}

#[tokio::test]
async fn rum_applications_and_events() {
    let (_, app) = app();
    let (status, body) = call(&app, "GET", "/api/v2/rum/applications", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"][0]["attributes"]["name"], "shop-web");
    assert_eq!(body["data"][0]["type"], "rum_application");

    let window =
        "filter%5Bfrom%5D=2026-10-30T14%3A00%3A00.000Z&filter%5Bto%5D=2026-10-30T15%3A00%3A00.000Z";
    let (status, body) = call(&app, "GET", &format!("/api/v2/rum/events?filter%5Bquery%5D=%40type%3Aview&{window}&sort=timestamp&page%5Blimit%5D=2000"), "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        ids(&body["data"]),
        [
            json!("AgAAAZK1rr0001"),
            json!("AgAAAZK1rr0004"),
            json!("AgAAAZK1rr0005")
        ]
    );
    // Waterfall-style query with the default (last 15 minutes) window.
    let (_, body) = call(&app, "GET", "/api/v2/rum/events?filter%5Bquery%5D=%40application.name%3Ashop-web%20%40session.id%3As-111&sort=timestamp&page%5Blimit%5D=2000", "").await;
    assert_eq!(body["data"].as_array().unwrap().len(), 3);
    let (_, body) = call(
        &app,
        "GET",
        &format!("/api/v2/rum/events?{window}&page%5Blimit%5D=2&sort=-timestamp"),
        "",
    )
    .await;
    assert_eq!(
        ids(&body["data"]),
        [json!("AgAAAZK1rr0005"), json!("AgAAAZK1rr0004")]
    );
    let cursor = body["meta"]["page"]["after"].as_str().unwrap().to_owned();
    let (_, next) = call(&app, "GET", &format!("/api/v2/rum/events?{window}&page%5Blimit%5D=2&sort=-timestamp&page%5Bcursor%5D={cursor}"), "").await;
    assert_eq!(
        ids(&next["data"]),
        [json!("AgAAAZK1rr0003"), json!("AgAAAZK1rr0002")]
    );
    let (status, _) = call(&app, "GET", "/api/v2/rum/events?sort=newest", "").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
