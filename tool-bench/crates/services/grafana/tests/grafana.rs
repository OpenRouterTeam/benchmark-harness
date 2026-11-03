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
use mock_grafana::Grafana;
use serde_json::{Value, json};
use tower::ServiceExt;

const WORLD: &str = "top-5xx-checkout-pod";

fn world() -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../scenarios/ops/grafana")
        .join(WORLD);
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

struct Resp {
    status: StatusCode,
    body: Value,
    text: String,
    content_type: String,
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    ct: &str,
    body: &str,
    auth: Option<&str>,
) -> Resp {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "world:8114")
        .header("content-type", ct);
    if let Some(a) = auth {
        req = req.header("authorization", a);
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let content_type = resp
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap().to_owned())
        .unwrap_or_default();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    Resp {
        status,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        text: String::from_utf8_lossy(&bytes).into_owned(),
        content_type,
    }
}

const AUTH: Option<&str> = Some("Bearer glsa_toolbench");

async fn get(app: &Router, uri: &str) -> Resp {
    send(app, "GET", uri, "application/json", "", AUTH).await
}

async fn post(app: &Router, uri: &str, body: Value) -> Resp {
    send(
        app,
        "POST",
        uri,
        "application/json",
        &body.to_string(),
        AUTH,
    )
    .await
}

#[tokio::test]
async fn auth_settings_health_and_user() {
    let app = service_app(world(), &Grafana);
    let denied = send(
        &app,
        "GET",
        "/api/user",
        "application/json",
        "",
        Some("Bearer nope"),
    )
    .await;
    assert_eq!(denied.status, StatusCode::UNAUTHORIZED);
    assert_eq!(denied.body["messageId"], "api-key.invalid");
    let settings = get(&app, "/api/frontend/settings").await;
    assert_eq!(settings.body["buildInfo"]["version"], "11.6.4");
    assert_eq!(
        settings.body["datasources"]["Prometheus (prod)"]["uid"],
        "prom-prod"
    );
    assert_eq!(get(&app, "/api/health").await.body["database"], "ok");
    assert_eq!(
        get(&app, "/api/user").await.body["login"],
        "sa-1-mcp-assistant"
    );
    assert_eq!(
        get(&app, "/api/user/orgs").await.body[0]["name"],
        "Octo Corp"
    );
    let k8s = get(&app, "/apis/dashboard.grafana.app").await;
    assert_eq!(k8s.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn search_filters_and_pages() {
    let app = service_app(world(), &Grafana);
    let all = get(&app, "/api/search?query=checkout").await.body;
    let titles: Vec<&str> = all
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        [
            "Checkout Overview (legacy)",
            "Checkout Service",
            "Checkout Service (staging)"
        ]
    );
    assert_eq!(all[1]["folderTitle"], "Production");
    assert_eq!(all[1]["url"], "/d/checkout-prod/checkout-service");
    let folders = get(&app, "/api/search?type=dash-folder").await.body;
    assert_eq!(folders.as_array().unwrap().len(), 4);
    let tagged = get(&app, "/api/search?type=dash-db&tag=prod&tag=slo")
        .await
        .body;
    assert_eq!(tagged.as_array().unwrap().len(), 1);
    let page2 = get(&app, "/api/search?type=dash-db&limit=2&page=2")
        .await
        .body;
    assert_eq!(page2.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn dashboards_versions_and_saves() {
    let w = world();
    let app = service_app(w.clone(), &Grafana);
    let d = get(&app, "/api/dashboards/uid/checkout-prod").await;
    assert_eq!(d.body["dashboard"]["version"], 7);
    assert_eq!(d.body["meta"]["folderUid"], "f-prod");
    assert_eq!(
        get(&app, "/api/dashboards/uid/nope").await.body["message"],
        "Dashboard not found"
    );
    let versions = get(&app, "/api/dashboards/uid/checkout-prod/versions?limit=3")
        .await
        .body;
    assert_eq!(versions["versions"].as_array().unwrap().len(), 3);
    assert_eq!(versions["versions"][0]["version"], 7);
    let v6 = get(&app, "/api/dashboards/uid/checkout-prod/versions/6")
        .await
        .body;
    assert_eq!(v6["data"]["version"], 6);

    let mut dash = d.body["dashboard"].clone();
    dash["title"] = json!("Checkout Service (prod)");
    let saved = post(
        &app,
        "/api/dashboards/db",
        json!({"dashboard": dash.clone(), "message": "rename"}),
    )
    .await;
    assert_eq!(saved.status, StatusCode::OK);
    assert_eq!(saved.body["version"], 8);
    let stale = post(&app, "/api/dashboards/db", json!({"dashboard": dash})).await;
    assert_eq!(stale.status, StatusCode::PRECONDITION_FAILED);
    assert_eq!(stale.body["status"], "version-mismatch");
    let empty = post(
        &app,
        "/api/dashboards/db",
        json!({"dashboard": {"title": " "}}),
    )
    .await;
    assert_eq!(empty.status, StatusCode::BAD_REQUEST);
    let created = post(
        &app,
        "/api/dashboards/db",
        json!({"dashboard": {"title": "Cart"}, "folderUid": "f-prod"}),
    )
    .await;
    assert_eq!(created.body["version"], 1);
    let snap = w.lock().unwrap().snapshot();
    assert_eq!(
        snap["services"]["grafana"]["dashboards"]["checkout-prod"]["dashboard"]["title"],
        "Checkout Service (prod)"
    );
    assert_eq!(
        snap["services"]["grafana"]["dashboard_versions"]["checkout-prod/8"]["message"],
        "rename"
    );
}

#[tokio::test]
async fn folders_and_datasources() {
    let app = service_app(world(), &Grafana);
    let f = post(&app, "/api/folders", json!({"title": "Incident Review"})).await;
    assert_eq!(f.status, StatusCode::OK);
    assert_eq!(f.body["title"], "Incident Review");
    let dup = post(&app, "/api/folders", json!({"title": "Production"})).await;
    assert_eq!(dup.status, StatusCode::CONFLICT);
    assert_eq!(
        get(&app, "/api/folders/nope").await.body["messageId"],
        "folder.notFound"
    );

    let list = get(&app, "/api/datasources").await.body;
    assert_eq!(list.as_array().unwrap().len(), 7);
    assert_eq!(
        get(&app, "/api/datasources/name/Loki%20(prod)").await.body["uid"],
        "loki-prod"
    );
    assert_eq!(
        get(&app, "/api/datasources/uid/nope").await.body["message"],
        "Data source not found"
    );
    assert_eq!(
        get(&app, "/api/datasources/uid/prom-prod/health")
            .await
            .body["status"],
        "OK"
    );
    let created = post(
        &app,
        "/api/datasources",
        json!({"name": "Prometheus (dr)", "type": "prometheus", "url": "http://p:9090"}),
    )
    .await;
    assert_eq!(created.body["message"], "Datasource added");
    let conflict = post(
        &app,
        "/api/datasources",
        json!({"name": "Prometheus (dr)", "type": "prometheus"}),
    )
    .await;
    assert_eq!(conflict.status, StatusCode::CONFLICT);
    let ro = send(
        &app,
        "PUT",
        "/api/datasources/uid/am-prod",
        "application/json",
        "{\"url\":\"x\"}",
        AUTH,
    )
    .await;
    assert_eq!(ro.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn annotations_lifecycle() {
    let w = world();
    let app = service_app(w.clone(), &Grafana);
    let list = get(&app, "/api/annotations?dashboardUID=checkout-prod")
        .await
        .body;
    assert_eq!(list.as_array().unwrap().len(), 1);
    let created = post(&app, "/api/annotations", json!({"dashboardUID": "checkout-prod", "time": 1794492300000_i64, "text": "Deploy checkout v4.19.0", "tags": ["deploy"]})).await;
    assert_eq!(
        created.body,
        json!({"message": "Annotation added", "id": 42})
    );
    let bad = post(
        &app,
        "/api/annotations",
        json!({"dashboardUID": "nope", "text": "x"}),
    )
    .await;
    assert_eq!(bad.status, StatusCode::NOT_FOUND);
    let patched = send(
        &app,
        "PATCH",
        "/api/annotations/42",
        "application/json",
        "{\"text\":\"Deploy v4.19.0 (canary)\"}",
        AUTH,
    )
    .await;
    assert_eq!(patched.body["message"], "Annotation patched");
    let tags = get(&app, "/api/annotations/tags?tag=dep").await.body;
    assert_eq!(
        tags["result"]["tags"][0],
        json!({"tag": "deploy", "count": 3})
    );
    let del = send(
        &app,
        "DELETE",
        "/api/annotations/41",
        "application/json",
        "",
        AUTH,
    )
    .await;
    assert_eq!(del.body["message"], "Annotation deleted");
    let snap = w.lock().unwrap().snapshot();
    let ann = &snap["services"]["grafana"]["annotations"];
    assert_eq!(ann["42"]["text"], "Deploy v4.19.0 (canary)");
    assert_eq!(ann["42"]["dashboardId"], 101);
    assert!(ann.get("41").is_none());
}

#[tokio::test]
async fn prometheus_queries_labels_and_errors() {
    let app = service_app(world(), &Grafana);
    let base = "/api/datasources/uid/prom-prod/resources/api/v1";
    let q = send(
        &app,
        "POST",
        &format!("{base}/query"),
        "application/x-www-form-urlencoded",
        "query=topk(1%2C+sum+by+(pod)+(rate(http_requests_total%7Bstatus%3D~%225..%22%7D%5B5m%5D)))&time=1794497400",
        AUTH,
    )
    .await;
    assert_eq!(q.body["data"]["resultType"], "vector");
    assert_eq!(
        q.body["data"]["result"][0]["metric"]["pod"],
        "checkout-7b9d6c5f4-xk2lp"
    );
    assert_eq!(q.body["data"]["result"][0]["value"][1], "3.5");
    let staging = get(&app, "/api/datasources/proxy/uid/prom-staging/api/v1/query?query=sum(rate(http_requests_total%7Bstatus%3D%22500%22%7D%5B5m%5D))&time=1794497400").await;
    assert_eq!(
        staging.body["data"]["result"][0]["value"][1],
        "4.6000000000000005"
    );
    let range = get(
        &app,
        &format!("{base}/query_range?query=up&start=1794493800&end=1794497400&step=600"),
    )
    .await;
    assert_eq!(
        range.body["data"]["result"][0]["values"]
            .as_array()
            .unwrap()
            .len(),
        7
    );
    let bad = get(&app, &format!("{base}/query?query=sum(")).await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    assert_eq!(bad.body["errorType"], "bad_data");
    let too_many = get(
        &app,
        &format!("{base}/query_range?query=up&start=0&end=1794497400&step=1"),
    )
    .await;
    assert_eq!(too_many.status, StatusCode::BAD_REQUEST);
    let names = get(&app, &format!("{base}/label/__name__/values"))
        .await
        .body;
    assert!(
        names["data"]
            .as_array()
            .unwrap()
            .contains(&json!("http_requests_total"))
    );
    let series = get(
        &app,
        &format!("{base}/series?match%5B%5D=up%7Bjob%3D%22cart%22%7D"),
    )
    .await
    .body;
    assert_eq!(series["data"].as_array().unwrap().len(), 1);
    let meta = get(&app, &format!("{base}/metadata?metric=http_requests_total"))
        .await
        .body;
    assert_eq!(meta["data"]["http_requests_total"][0]["type"], "counter");
    let missing = get(
        &app,
        "/api/datasources/uid/nope/resources/api/v1/query?query=up",
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn loki_logs_metrics_and_stats() {
    let app = service_app(world(), &Grafana);
    let base = "/api/datasources/uid/loki-prod/resources/loki/api/v1";
    let start = 1794493800_i64 * 1_000_000_000;
    let end = 1794497400_i64 * 1_000_000_000;
    let logs = get(&app, &format!("{base}/query_range?query=%7Bapp%3D%22checkout%22%7D+%7C%3D+%22error%22&start={start}&end={end}&limit=2")).await.body;
    assert_eq!(logs["data"]["resultType"], "streams");
    let n: usize = logs["data"]["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["values"].as_array().unwrap().len())
        .sum();
    assert_eq!(n, 2);
    assert_eq!(
        logs["data"]["result"][0]["stream"]["pod"],
        "checkout-7b9d6c5f4-xk2lp"
    );
    let shifted = get(
        &app,
        &format!(
            "{base}/query_range?query=%7Bapp%3D%22checkout%22%7D&start={}&end={}",
            start - 86_400_000_000_000 * 12,
            end - 86_400_000_000_000 * 12
        ),
    )
    .await
    .body;
    assert!(
        !shifted["data"]["result"].as_array().unwrap().is_empty(),
        "wall-clock anchored ranges see the data"
    );
    let instant_log = get(
        &app,
        &format!("{base}/query?query=%7Bapp%3D%22checkout%22%7D"),
    )
    .await;
    assert_eq!(instant_log.status, StatusCode::BAD_REQUEST);
    let metric = get(&app, &format!("{base}/query?query=sum+by+(pod)+(count_over_time(%7Bapp%3D%22checkout%22%7D+%7C+logfmt+%7C+level%3D%22error%22+%5B1h%5D))&time={end}")).await.body;
    assert_eq!(metric["data"]["result"][0]["value"][1], "3");
    let bad = get(
        &app,
        &format!("{base}/query_range?query=%7Bapp%3D~%22.*%22%7D"),
    )
    .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    assert!(
        bad.text
            .contains("queries require at least one regexp or equality matcher")
    );
    let labels = get(&app, &format!("{base}/labels?start={start}&end={end}"))
        .await
        .body;
    assert_eq!(
        labels["data"],
        json!(["app", "container", "namespace", "pod"])
    );
    let values = get(
        &app,
        &format!("{base}/label/app/values?start={start}&end={end}"),
    )
    .await
    .body;
    assert_eq!(values["data"], json!(["checkout", "payments-api"]));
    let stats = get(
        &app,
        &format!(
            "{base}/index/stats?query=%7Bnamespace%3D%22shop-prod%22%7D&start={start}&end={end}"
        ),
    )
    .await
    .body;
    assert_eq!(stats["streams"], 3);
}

#[tokio::test]
async fn tempo_search_trace_and_tags() {
    let app = service_app(world(), &Grafana);
    let base = "/api/datasources/proxy/uid/tempo-prod";
    let found = get(
        &app,
        &format!("{base}/api/search?q=%7B+status+%3D+error+%7D&start=1794490200&end=1794497400"),
    )
    .await
    .body;
    assert_eq!(found["traces"].as_array().unwrap().len(), 1);
    assert_eq!(found["traces"][0]["rootServiceName"], "checkout");
    let slow = get(
        &app,
        &format!("{base}/api/search?q=%7B+duration+%3E+1s+%7D&start=1794490200&end=1794497400"),
    )
    .await
    .body;
    assert_eq!(slow["traces"].as_array().unwrap().len(), 1);
    let id = found["traces"][0]["traceID"].as_str().unwrap().to_owned();
    let llm = send(
        &app,
        "GET",
        &format!("{base}/api/v2/traces/{id}"),
        "application/json",
        "",
        AUTH,
    )
    .await;
    assert_eq!(
        llm.body["trace"]["resourceSpans"].as_array().unwrap().len(),
        2
    );
    let unknown = get(&app, &format!("{base}/api/v2/traces/deadbeef"))
        .await
        .body;
    assert_eq!(unknown, json!({"trace": {}}));
    let tags = get(&app, &format!("{base}/api/v2/search/tags")).await.body;
    assert_eq!(tags["scopes"][0]["tags"], json!(["service.name"]));
    let values = get(
        &app,
        &format!("{base}/api/v2/search/tag/resource.service.name/values"),
    )
    .await
    .body;
    assert_eq!(values["tagValues"].as_array().unwrap().len(), 2);
    let bad = get(&app, &format!("{base}/api/search?q=service%3Dcheckout")).await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn pyroscope_speaks_binary_connect() {
    let app = service_app(world(), &Grafana);
    let r = send(
        &app,
        "POST",
        "/api/datasources/proxy/uid/pyroscope-prod/querier.v1.QuerierService/ProfileTypes",
        "application/proto",
        "",
        AUTH,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.content_type, "application/proto");
    assert!(
        r.text
            .contains("process_cpu:cpu:nanoseconds:cpu:nanoseconds")
    );
}

#[tokio::test]
async fn alerting_rules_silences_and_alerts() {
    let w = world();
    let app = service_app(w.clone(), &Grafana);
    let rules = get(
        &app,
        "/api/prometheus/grafana/api/v1/rules?state=firing&folder_uid=f-prod",
    )
    .await
    .body;
    let groups = rules["data"]["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["rules"].as_array().unwrap().len(), 2);
    assert_eq!(groups[0]["file"], "Production");
    let one = get(&app, "/api/v1/provisioning/alert-rules/ae3pglag")
        .await
        .body;
    assert_eq!(one["ruleGroup"], "databases");
    assert_eq!(
        get(&app, "/api/v1/provisioning/alert-rules/nope")
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    let invalid = post(
        &app,
        "/api/v1/provisioning/alert-rules",
        json!({"title": "x"}),
    )
    .await;
    assert_eq!(invalid.status, StatusCode::BAD_REQUEST);

    let path = "/api/alertmanager/grafana/api/v2/silences";
    let past = post(&app, path, json!({"matchers": [{"name": "alertname", "value": "X"}], "startsAt": "2026-11-01T00:00:00Z", "endsAt": "2026-11-02T00:00:00Z"})).await;
    assert_eq!(past.status, StatusCode::BAD_REQUEST);
    let created = post(&app, path, json!({"matchers": [{"name": "alertname", "value": "PostgresReplicationLag"}, {"name": "instance", "value": "pg-replica-2"}], "startsAt": "2026-11-12T00:00:00Z", "endsAt": "2026-11-13T04:00:00Z", "comment": "CHG-1", "createdBy": "sre"})).await;
    assert_eq!(created.status, StatusCode::ACCEPTED);
    let id = created.body["silenceID"].as_str().unwrap().to_owned();
    let listed = get(&app, path).await.body;
    assert_eq!(listed[0]["status"]["state"], "active");
    let alerts = get(
        &app,
        "/api/alertmanager/grafana/api/v2/alerts?filter=alertname%3D%22PostgresReplicationLag%22",
    )
    .await
    .body;
    assert_eq!(alerts[0]["status"]["state"], "suppressed");
    assert_eq!(alerts[0]["status"]["silencedBy"][0], id.as_str());
    let del = send(
        &app,
        "DELETE",
        &format!("/api/alertmanager/grafana/api/v2/silence/{id}"),
        "application/json",
        "",
        AUTH,
    )
    .await;
    assert_eq!(del.status, StatusCode::OK);
    let snap = w.lock().unwrap().snapshot();
    let s = &snap["services"]["grafana"]["silences"][id.as_str()];
    assert_eq!(
        s["startsAt"], "2026-11-12T15:30:21Z",
        "a start in the past becomes now"
    );
    assert_eq!(
        s["endsAt"], "2026-11-12T15:30:28Z",
        "expiring ends the silence now"
    );
    let policies = get(&app, "/api/v1/provisioning/policies").await.body;
    assert_eq!(policies["receiver"], "grafana-default-email");
}

#[tokio::test]
async fn oncall_public_api() {
    let w = world();
    let app = service_app(w.clone(), &Grafana);
    let settings = get(&app, "/api/plugins/grafana-irm-app/settings")
        .await
        .body;
    assert_eq!(
        settings["jsonData"]["onCallApiUrl"],
        "http://world:8114/oncall"
    );
    let oc = |m: &'static str, p: String| {
        let app = app.clone();
        async move { send(&app, m, &p, "application/json", "", Some("glsa_toolbench")).await }
    };
    let schedules = oc("GET", "/oncall/api/v1/schedules/?team_id=TCK4M2P8".into())
        .await
        .body;
    assert_eq!(schedules["count"], 2);
    assert_eq!(schedules["results"][0]["on_call_now"], json!(["UZ9D6K3L"]));
    let groups = oc("GET", "/oncall/api/v1/alert_groups/?state=new".into())
        .await
        .body;
    assert_eq!(groups["count"], 2);
    let ack = oc(
        "POST",
        "/oncall/api/v1/alert_groups/IHQ4K2M7/acknowledge".into(),
    )
    .await;
    assert_eq!(ack.status, StatusCode::OK);
    let again = oc(
        "POST",
        "/oncall/api/v1/alert_groups/IHQ4K2M7/acknowledge".into(),
    )
    .await;
    assert_eq!(again.status, StatusCode::BAD_REQUEST);
    let missing = oc("GET", "/oncall/api/v1/users/NOPE/".into()).await;
    assert_eq!(missing.body, json!({"detail": "Not found."}));
    let unauth = send(
        &app,
        "GET",
        "/oncall/api/v1/users/",
        "application/json",
        "",
        Some("bad"),
    )
    .await;
    assert_eq!(unauth.status, StatusCode::UNAUTHORIZED);
    let snap = w.lock().unwrap().snapshot();
    assert_eq!(
        snap["services"]["grafana"]["oncall_alert_groups"]["IHQ4K2M7"]["state"],
        "acknowledged"
    );
}

#[tokio::test]
async fn incident_rpc() {
    let w = world();
    let app = service_app(w.clone(), &Grafana);
    let base = "/api/plugins/grafana-irm-app/resources/api/v1";
    let q = post(
        &app,
        &format!("{base}/IncidentsService.QueryIncidentPreviews"),
        json!({"query": {"queryString": "isdrill:false", "limit": 10, "orderDirection": "DESC"}}),
    )
    .await
    .body;
    assert_eq!(q["incidentPreviews"].as_array().unwrap().len(), 1);
    let created = post(
        &app,
        &format!("{base}/IncidentsService.CreateIncident"),
        json!({"title": "Checkout p99 above SLO", "severity": "minor", "roomPrefix": "inc"}),
    )
    .await
    .body;
    assert_eq!(created["incident"]["incidentID"], "215");
    let note = post(
        &app,
        &format!("{base}/ActivityService.AddActivity"),
        json!({"incidentID": "215", "activityKind": "userNote", "body": "investigating"}),
    )
    .await
    .body;
    assert_eq!(note["activityItem"]["activityItemID"], "215-1");
    let resolved = post(
        &app,
        &format!("{base}/IncidentsService.UpdateStatus"),
        json!({"incidentID": "215", "status": "resolved"}),
    )
    .await
    .body;
    assert_eq!(resolved["incident"]["status"], "resolved");
    let missing = post(
        &app,
        &format!("{base}/IncidentsService.GetIncident"),
        json!({"incidentID": "999"}),
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.body["error"], "incident not found: 999");
    let snap = w.lock().unwrap().snapshot();
    assert_eq!(
        snap["services"]["grafana"]["incidents"]["215"]["closedTime"],
        "2026-11-12T15:30:28Z"
    );
}

#[tokio::test]
async fn snapshots_short_urls_plugins_render() {
    let app = service_app(world(), &Grafana);
    let s = post(
        &app,
        "/api/snapshots",
        json!({"dashboard": {"title": "x"}, "key": "k1"}),
    )
    .await
    .body;
    assert_eq!(s["key"], "k1");
    assert_eq!(
        get(&app, "/api/snapshots/k1").await.body["meta"]["isSnapshot"],
        true
    );
    assert_eq!(
        get(&app, "/api/dashboard/snapshots")
            .await
            .body
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let del = send(
        &app,
        "DELETE",
        "/api/snapshots/k1",
        "application/json",
        "",
        AUTH,
    )
    .await;
    assert_eq!(del.status, StatusCode::OK);
    assert_eq!(
        get(&app, "/api/snapshots/k1").await.status,
        StatusCode::NOT_FOUND
    );
    let short = post(&app, "/api/short-urls", json!({"path": "d/checkout-prod"}))
        .await
        .body;
    assert!(short["url"].as_str().unwrap().contains("/goto/"));
    assert_eq!(
        get(&app, "/api/plugins/nope/settings").await.status,
        StatusCode::NOT_FOUND
    );
    let install = post(
        &app,
        "/api/plugins/grafana-polystat-panel/install",
        json!({"version": "2.1.14"}),
    )
    .await;
    assert_eq!(install.status, StatusCode::OK);
    assert_eq!(
        get(&app, "/api/plugins/grafana-polystat-panel/settings")
            .await
            .body["info"]["version"],
        "2.1.14"
    );
    let png = get(&app, "/render/d-solo/checkout-prod/x?panelId=2").await;
    assert_eq!(png.content_type, "image/png");
    let asserts = post(
        &app,
        "/api/plugins/grafana-asserts-app/resources/asserts/api-server/v1/assertions/llm-summary",
        json!({}),
    )
    .await;
    assert_eq!(asserts.body["message"], "Plugin not found");
}

#[tokio::test]
async fn ds_query_prometheus_frames() {
    let app = service_app(world(), &Grafana);
    let r = post(&app, "/api/ds/query", json!({"from": "now-1h", "to": "now", "queries": [{"refId": "A", "datasource": {"uid": "prom-prod", "type": "prometheus"}, "expr": "sum(up)"}]})).await.body;
    let frame = &r["results"]["A"]["frames"][0];
    assert_eq!(frame["schema"]["fields"][1]["type"], "number");
    assert_eq!(frame["data"]["values"][1][0], 4.0);
}
