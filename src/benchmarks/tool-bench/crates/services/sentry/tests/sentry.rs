use std::path::Path;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode},
};
use mock_core::{
    dataset::Dataset,
    serve::service_app,
    world::{SharedWorld, WorldState, shared},
};
use mock_sentry::Sentry;
use serde_json::{Value, json};
use tower::ServiceExt;

const ORG: &str = "/api/0/organizations/lumen-labs";

fn world() -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../scenarios/ops/sentry/top-unresolved-checkout");
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

fn app() -> (SharedWorld, Router) {
    let w = world();
    (w.clone(), service_app(w, &Sentry))
}

async fn call(app: &Router, method: &str, uri: &str, body: &str) -> (StatusCode, HeaderMap, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "world:8113")
        .header("content-type", "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = to_bytes(body, usize::MAX).await.unwrap();
    (
        parts.status,
        parts.headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn get(app: &Router, uri: &str) -> (StatusCode, HeaderMap, Value) {
    call(app, "GET", uri, "").await
}

fn field(v: &Value, key: &str) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|i| i[key].as_str().unwrap_or_default().to_owned())
        .collect()
}

fn not_found() -> Value {
    json!({ "detail": "The requested resource does not exist" })
}

fn state(w: &SharedWorld) -> Value {
    w.lock().unwrap().snapshot()["services"]["sentry"].clone()
}

#[tokio::test]
async fn identity_and_organizations() {
    let (_, app) = app();
    let (status, _, me) = get(&app, "/api/0/auth/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["id"], "2841306");
    assert_eq!(me["email"], "priya@lumenlabs.dev");

    let (status, headers, orgs) = get(&app, "/api/0/organizations/?per_page=26").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(field(&orgs, "slug"), ["lumen-labs"]);
    // Self-hosted: region URL is empty so the client stays on the configured host.
    assert_eq!(
        orgs[0]["links"],
        json!({ "organizationUrl": "http://world:8113", "regionUrl": "" })
    );
    let link = headers["link"].to_str().unwrap();
    assert!(
        link.contains(r#"rel="next"; results="false"; cursor="0:26:0""#),
        "{link}"
    );

    let (status, _, org) = get(&app, &format!("{ORG}/")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(org["teams"].as_array().unwrap().len(), 5);
    let (status, _, body) = get(&app, "/api/0/organizations/nope/").await;
    assert_eq!((status, body), (StatusCode::NOT_FOUND, not_found()));
}

#[tokio::test]
async fn projects_and_teams() {
    let (_, app) = app();
    let (_, _, projects) = get(&app, &format!("{ORG}/projects/?query=pay")).await;
    assert_eq!(field(&projects, "slug"), ["payments-worker"]);
    assert_eq!(projects[0]["teams"][1]["slug"], "payments-infra");

    for id in ["checkout-api", "4508812003"] {
        let (status, _, p) = get(&app, &format!("/api/0/projects/lumen-labs/{id}/")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(p["slug"], "checkout-api");
        assert_eq!(p["organization"]["slug"], "lumen-labs");
    }
    let (status, _, body) = get(&app, "/api/0/projects/lumen-labs/nope/").await;
    assert_eq!((status, body), (StatusCode::NOT_FOUND, not_found()));

    let (_, _, teams) = get(&app, &format!("{ORG}/teams/?per_page=26")).await;
    assert_eq!(teams.as_array().unwrap().len(), 5);
    let (_, _, ptm) = get(&app, "/api/0/projects/lumen-labs/payments-worker/teams/").await;
    assert_eq!(field(&ptm, "slug"), ["payments", "payments-infra"]);
}

#[tokio::test]
async fn create_team_project_and_keys_write_state() {
    let (w, app) = app();
    let (status, _, team) = call(
        &app,
        "POST",
        &format!("{ORG}/teams/"),
        r#"{"name":"Notifications"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(team["slug"], "notifications");
    assert_eq!(team["id"], "4507231");
    let (status, _, body) = call(
        &app,
        "POST",
        &format!("{ORG}/teams/"),
        r#"{"name":"Notifications"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["detail"], "A team with this slug already exists.");

    let (status, _, project) = call(
        &app,
        "POST",
        "/api/0/teams/lumen-labs/payments/projects/",
        r#"{"name":"notification-service","platform":"node"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(project["id"], "4508812006");
    assert_eq!(project["team"]["slug"], "payments");
    let (status, _, body) = call(
        &app,
        "POST",
        "/api/0/teams/lumen-labs/payments/projects/",
        r#"{"name":"notification-service"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["detail"], "A project with this slug already exists.");
    let (status, _, body) = call(
        &app,
        "POST",
        "/api/0/teams/lumen-labs/payments/projects/",
        r#"{}"#,
    )
    .await;
    assert_eq!(
        (status, body),
        (
            StatusCode::BAD_REQUEST,
            json!({ "name": ["This field is required."] })
        )
    );
    let (status, _, _) = call(
        &app,
        "POST",
        "/api/0/teams/lumen-labs/nope/projects/",
        r#"{"name":"x"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A Default key comes with the project; more can be created, updated and deleted.
    let (_, _, keys) = get(
        &app,
        "/api/0/projects/lumen-labs/notification-service/keys/",
    )
    .await;
    assert_eq!(field(&keys, "name"), ["Default"]);
    let public = keys[0]["public"].as_str().unwrap().to_owned();
    assert_eq!(public.len(), 32);
    assert_eq!(
        keys[0]["dsn"]["public"],
        format!("http://{public}@world:8113/4508812006")
    );
    let (status, _, edge) = call(
        &app,
        "POST",
        "/api/0/projects/lumen-labs/notification-service/keys/",
        r#"{"name":"edge"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let edge_id = edge["id"].as_str().unwrap().to_owned();
    let (status, _, upd) = call(
        &app,
        "PUT",
        &format!("/api/0/projects/lumen-labs/notification-service/keys/{edge_id}/"),
        r#"{"isActive":false,"rateLimit":{"window":60,"count":100}}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(upd["isActive"], false);
    assert_eq!(upd["rateLimit"], json!({ "window": 60, "count": 100 }));

    let s = state(&w);
    assert_eq!(
        s["projects"]["notification-service"]["teams"],
        json!(["payments"])
    );
    assert_eq!(s["keys"]["notification-service/Default"]["isActive"], true);
    assert_eq!(s["keys"]["notification-service/edge"]["isActive"], false);
    assert_eq!(s["teams"]["notifications"]["name"], "Notifications");

    let (status, _, _) = call(
        &app,
        "DELETE",
        &format!("/api/0/projects/lumen-labs/notification-service/keys/{edge_id}/"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(state(&w)["keys"].get("notification-service/edge").is_none());

    let (status, _, p) = call(
        &app,
        "PUT",
        "/api/0/projects/lumen-labs/notification-service/",
        r#"{"platform":"node-express"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(p["platform"], "node-express");
    let (status, _, p) = call(
        &app,
        "POST",
        "/api/0/projects/lumen-labs/notification-service/teams/notifications/",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(p["teams"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn issue_search_filters_sorts_and_paginates() {
    let (_, app) = app();
    // Default query is is:unresolved; ignored and resolved issues are excluded.
    let (_, _, issues) = get(
        &app,
        &format!("{ORG}/issues/?project=4508812003&sort=freq&statsPeriod=7d"),
    )
    .await;
    assert_eq!(
        field(&issues, "shortId"),
        ["CHECKOUT-API-3F", "CHECKOUT-API-3C", "CHECKOUT-API-2Z"]
    );
    assert_eq!(issues[0]["project"]["slug"], "checkout-api");
    assert_eq!(issues[0]["count"], "1842");

    let (_, _, all) = get(
        &app,
        &format!("{ORG}/issues/?query=&project=4508812003&sort=freq"),
    )
    .await;
    assert_eq!(field(&all, "shortId")[0], "CHECKOUT-API-2R");

    let q = |s: &str| {
        s.bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b == b'-' || b == b'.' {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect::<String>()
    };
    let (_, _, hits) = get(
        &app,
        &format!(
            "{ORG}/issues/?query={}",
            q("is:unresolved firstRelease:storefront@5.3.0 TypeError")
        ),
    )
    .await;
    assert_eq!(field(&hits, "shortId"), ["STOREFRONT-WEB-8D"]);
    let (_, _, hits) = get(
        &app,
        &format!(
            "{ORG}/issues/?query={}",
            q("is:unresolved \"reading 'map'\" !project:admin-dashboard")
        ),
    )
    .await;
    assert_eq!(
        field(&hits, "shortId"),
        ["STOREFRONT-WEB-8D", "STOREFRONT-WEB-7Q"]
    );
    let (_, _, hits) = get(
        &app,
        &format!("{ORG}/issues/?query={}", q("assigned:#payments-infra")),
    )
    .await;
    assert_eq!(field(&hits, "shortId"), ["PAYMENTS-WORKER-1K"]);
    let (_, _, hits) = get(
        &app,
        &format!(
            "{ORG}/issues/?query={}",
            q("is:unresolved times_seen:>2000 level:error")
        ),
    )
    .await;
    assert_eq!(
        field(&hits, "shortId"),
        [
            "STOREFRONT-WEB-8D",
            "STOREFRONT-WEB-7Q",
            "PAYMENTS-WORKER-1K"
        ]
    );
    let (_, _, hits) = get(
        &app,
        &format!("{ORG}/issues/?query={}", q("critical bugs from last week")),
    )
    .await;
    assert_eq!(hits, json!([]));

    // Short-ID lookup bypasses the other filters (CHECKOUT-API-31 is resolved).
    let (_, _, hits) = get(&app, &format!("{ORG}/issues/?query=checkout-api-31")).await;
    assert_eq!(field(&hits, "shortId"), ["CHECKOUT-API-31"]);

    let (_, headers, page1) = get(&app, &format!("{ORG}/issues/?limit=2&sort=new")).await;
    assert_eq!(page1.as_array().unwrap().len(), 2);
    let link = headers["link"].to_str().unwrap().to_owned();
    assert!(
        link.contains(r#"rel="next"; results="true"; cursor="0:2:0""#),
        "{link}"
    );
    assert!(
        link.contains(r#"rel="previous"; results="false""#),
        "{link}"
    );
    let (_, _, page2) = get(
        &app,
        &format!("{ORG}/issues/?limit=2&sort=new&cursor=0:2:0"),
    )
    .await;
    assert_ne!(page1[0]["id"], page2[0]["id"]);

    let (status, _, body) = get(&app, &format!("{ORG}/issues/?project=checkout-api")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["detail"],
        "Invalid project parameter. Values must be numbers."
    );
}

#[tokio::test]
async fn issue_details_events_and_tags() {
    let (_, app) = app();
    for id in ["CHECKOUT-API-3F", "6184420311", "checkout-api-3f"] {
        let (status, _, issue) = get(&app, &format!("{ORG}/issues/{id}/")).await;
        assert_eq!(status, StatusCode::OK, "{id}");
        assert_eq!(issue["shortId"], "CHECKOUT-API-3F");
        assert_eq!(issue["firstRelease"]["version"], "checkout-api@2026.10.14");
    }
    let (status, _, body) = get(&app, &format!("{ORG}/issues/NOPE-1/")).await;
    assert_eq!((status, body), (StatusCode::NOT_FOUND, not_found()));

    let (_, _, latest) = get(
        &app,
        &format!("{ORG}/issues/6184420311/events/latest/?llmFormat=json"),
    )
    .await;
    assert_eq!(latest["dateCreated"], "2026-10-20T14:48:00Z");
    assert_eq!(latest["type"], "error");
    assert_eq!(latest["entries"][0]["type"], "exception");
    assert_eq!(
        latest["entries"][0]["data"]["values"][0]["type"],
        "OperationalError"
    );
    assert_eq!(
        latest["contexts"]["trace"]["trace_id"],
        "a3f9c1e27b4d4f0e9c8b7a6d5e4f3c2b"
    );
    assert!(latest["nextEventID"].is_null());
    let (_, _, oldest) = get(&app, &format!("{ORG}/issues/6184420311/events/oldest/")).await;
    assert_eq!(oldest["release"]["version"], "checkout-api@2026.10.14");
    let eid = oldest["id"].as_str().unwrap();
    let (status, _, same) = get(&app, &format!("{ORG}/issues/6184420311/events/{eid}/")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(same["id"], oldest["id"]);
    let (status, _, _) = get(
        &app,
        &format!("{ORG}/issues/6184420311/events/ffffffffffffffffffffffffffffffff/"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (_, _, events) = get(
        &app,
        &format!(
            "{ORG}/issues/STOREFRONT-WEB-7Q/events/?query=release%3Astorefront%405.2.0&per_page=50"
        ),
    )
    .await;
    assert_eq!(events.as_array().unwrap().len(), 2);
    assert_eq!(events[0]["dateCreated"], "2026-10-08T04:00:00Z");

    let (status, _, tag) = get(
        &app,
        &format!("{ORG}/issues/STOREFRONT-WEB-7Q/tags/release/"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tag["totalValues"], 2600);
    assert_eq!(
        field(&tag["topValues"], "value"),
        ["storefront@5.2.0", "storefront@5.3.0", "storefront@5.3.1"]
    );
    // Without a stored distribution, tag values come from the issue's events.
    let (_, _, tag) = get(&app, &format!("{ORG}/issues/CHECKOUT-API-3C/tags/release/")).await;
    assert_eq!(tag["totalValues"], 4);
    let (status, _, body) = get(&app, &format!("{ORG}/issues/CHECKOUT-API-3C/tags/nope/")).await;
    assert_eq!((status, body), (StatusCode::NOT_FOUND, not_found()));
}

#[tokio::test]
async fn update_issue_writes_status_assignee_and_activity() {
    let (w, app) = app();
    let path = format!("{ORG}/issues/STOREFRONT-WEB-8D/");
    let (status, _, issue) = call(
        &app,
        "PUT",
        &path,
        r#"{"status":"resolvedInNextRelease","assignedTo":"team:frontend-web"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(issue["status"], "resolved");
    assert_eq!(issue["statusDetails"]["inNextRelease"], true);
    assert_eq!(
        issue["assignedTo"],
        json!({ "type": "team", "id": "4507214", "name": "frontend-web" })
    );

    let (_, _, issue) = call(&app, "PUT", &format!("{ORG}/issues/6184420297/"), r#"{"status":"ignored","substatus":"archived_until_condition_met","ignoreCount":100,"ignoreWindow":60}"#).await;
    assert_eq!(issue["status"], "ignored");
    assert_eq!(issue["statusDetails"]["ignoreCount"], 100);
    let (_, _, issue) = call(
        &app,
        "PUT",
        &format!("{ORG}/issues/CHECKOUT-API-2Z/"),
        r#"{"assignedTo":"user:2841311"}"#,
    )
    .await;
    assert_eq!(issue["assignedTo"]["email"], "marcus@lumenlabs.dev");
    let (_, _, issue) = call(
        &app,
        "PUT",
        &format!("{ORG}/issues/CHECKOUT-API-2Z/"),
        r#"{"assignedTo":""}"#,
    )
    .await;
    assert!(issue["assignedTo"].is_null());

    let (status, _, body) = call(&app, "PUT", &path, r#"{"status":"fixed"}"#).await;
    assert_eq!(
        (status, body),
        (
            StatusCode::BAD_REQUEST,
            json!({ "status": ["\"fixed\" is not a valid choice."] })
        )
    );
    let (status, _, body) = call(&app, "PUT", &path, r#"{"assignedTo":"user:1"}"#).await;
    assert_eq!(
        (status, body),
        (
            StatusCode::BAD_REQUEST,
            json!({ "assignedTo": ["Unknown actor input"] })
        )
    );
    let (status, _, _) = call(
        &app,
        "PUT",
        &format!("{ORG}/issues/NOPE-9/"),
        r#"{"status":"resolved"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let s = state(&w);
    assert_eq!(s["issues"]["STOREFRONT-WEB-8D"]["status"], "resolved");
    assert_eq!(
        s["issues"]["STOREFRONT-WEB-8D"]["assignedTo"]["name"],
        "frontend-web"
    );
    assert_eq!(s["issues"]["STOREFRONT-WEB-7Q"]["status"], "unresolved");

    let (_, _, acts) = get(&app, &format!("{ORG}/issues/STOREFRONT-WEB-8D/activities/")).await;
    let types: Vec<&str> = acts["activity"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["type"].as_str().unwrap())
        .collect();
    assert_eq!(types, ["assigned", "set_resolved_in_release", "first_seen"]);

    let (status, _, note) = call(
        &app,
        "POST",
        &format!("{ORG}/issues/STOREFRONT-WEB-8D/notes/"),
        r#"{"text":"Fix merged in #4182"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(note["data"]["text"], "Fix merged in #4182");
    assert_eq!(note["user"]["id"], "2841306");
    let (_, _, notes) = get(&app, &format!("{ORG}/issues/STOREFRONT-WEB-8D/notes/")).await;
    assert_eq!(notes.as_array().unwrap().len(), 1);
    let (_, _, issue) = get(&app, &path).await;
    assert_eq!(issue["numComments"], 1);
    let (status, _, _) = call(
        &app,
        "POST",
        &format!("{ORG}/issues/STOREFRONT-WEB-8D/notes/"),
        r#"{}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn discover_aggregates_and_validation() {
    let (_, app) = app();
    let (status, _, body) = get(
        &app,
        &format!("{ORG}/events/?dataset=errors&project=4508812003&statsPeriod=7d&field=issue&field=title&field=count()&sort=-count()&per_page=10"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        field(&body["data"], "issue")[..2],
        ["CHECKOUT-API-31", "CHECKOUT-API-3F"]
    );
    assert_eq!(body["data"][1]["count()"], 6);
    assert_eq!(body["meta"]["fields"]["count()"], "integer");

    let (_, _, body) = get(&app, &format!("{ORG}/events/?dataset=errors&query=issue%3ASTOREFRONT-WEB-8D&field=release&field=count()&sort=release&statsPeriod=14d")).await;
    assert_eq!(
        body["data"],
        json!([
            { "release": "storefront@5.3.0", "count()": 2 },
            { "release": "storefront@5.3.1", "count()": 2 }
        ])
    );

    let (_, _, body) = get(&app, &format!("{ORG}/events/?dataset=spans&query=span.op%3Adb.connection&field=span.op&field=span.duration&sort=-span.duration")).await;
    assert_eq!(
        body["data"],
        json!([{ "span.op": "db.connection", "span.duration": 5003.1 }])
    );
    let (_, _, body) = get(&app, &format!("{ORG}/events/?dataset=logs&query=severity%3Aerror&field=message&field=timestamp&sort=-timestamp")).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 1);

    let (status, _, body) = get(
        &app,
        &format!("{ORG}/events/?dataset=errors&field=issue&sort=-count()"),
    )
    .await;
    assert_eq!(
        (status, body["detail"].clone()),
        (
            StatusCode::BAD_REQUEST,
            json!("orderby must also be in the selected columns or groupby")
        )
    );
    let (status, _, body) = get(
        &app,
        &format!("{ORG}/events/?dataset=errors&field=bogus(x)"),
    )
    .await;
    assert_eq!(
        (status, body["detail"].clone()),
        (
            StatusCode::BAD_REQUEST,
            json!("bogus is not a valid function")
        )
    );
    let (status, _, _) = get(&app, &format!("{ORG}/events/?dataset=nope&field=id")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _, v) = get(
        &app,
        &format!(
            "{ORG}/events/validate/?dataset=errors&field=issue&field=count()&orderby=-count()"
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["valid"], true);
    let (status, _, v) = get(
        &app,
        &format!("{ORG}/events/validate/?dataset=errors&field=issue&orderby=-count()"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(v["valid"], false);

    let (_, _, stats) = get(&app, &format!("{ORG}/events-stats/?dataset=errors&query=issue%3ACHECKOUT-API-3F&yAxis=count()&statsPeriod=24h&interval=1h")).await;
    let total: u64 = stats["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b[1][0]["count"].as_u64().unwrap())
        .sum();
    assert_eq!(total, 2);
    assert_eq!(stats["data"].as_array().unwrap().len(), 25);
}

#[tokio::test]
async fn traces_releases_and_seer() {
    let (w, app) = app();
    let (_, _, meta) = get(
        &app,
        &format!("{ORG}/trace-meta/a3f9c1e27b4d4f0e9c8b7a6d5e4f3c2b/?statsPeriod=14d"),
    )
    .await;
    assert_eq!(
        (
            meta["span_count"].clone(),
            meta["errors"].clone(),
            meta["logs"].clone()
        ),
        (json!(3), json!(1), json!(2))
    );
    let (_, _, trace) = get(
        &app,
        &format!(
            "{ORG}/trace/a3f9c1e27b4d4f0e9c8b7a6d5e4f3c2b/?limit=1000&project=-1&statsPeriod=14d"
        ),
    )
    .await;
    assert_eq!(trace.as_array().unwrap().len(), 1);
    assert_eq!(trace[0]["children"].as_array().unwrap().len(), 2);
    assert_eq!(trace[0]["errors"][0]["issue_id"], 6184420311_u64);
    let (_, _, empty) = get(
        &app,
        &format!("{ORG}/trace/00000000000000000000000000000000/"),
    )
    .await;
    assert_eq!(empty, json!([]));

    let (_, _, releases) = get(&app, &format!("{ORG}/releases/?per_page=3")).await;
    assert_eq!(
        field(&releases, "version"),
        [
            "payments-worker@1.15.0",
            "checkout-api@2026.10.19",
            "storefront@5.3.1"
        ]
    );
    let (_, _, releases) = get(
        &app,
        "/api/0/projects/lumen-labs/storefront-web/releases/?query=5.3",
    )
    .await;
    assert_eq!(
        field(&releases, "version"),
        ["storefront@5.3.1", "storefront@5.3.0"]
    );
    let (status, _, r) = get(&app, &format!("{ORG}/releases/storefront%405.3.0/")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(r["newGroups"], 2);
    assert_eq!(r["shortVersion"], "5.3.0");
    let (status, _, body) = get(&app, &format!("{ORG}/releases/storefront%409.9.9/")).await;
    assert_eq!((status, body), (StatusCode::NOT_FOUND, not_found()));
    let (status, _, _) = get(
        &app,
        "/api/0/projects/lumen-labs/checkout-api/releases/storefront%405.3.0/",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, _, deploys) = get(&app, &format!("{ORG}/releases/storefront%405.3.0/deploys/")).await;
    assert_eq!(deploys[0]["environment"], "production");
    let (_, _, commits) = get(&app, &format!("{ORG}/releases/storefront%405.3.0/commits/")).await;
    assert_eq!(commits.as_array().unwrap().len(), 2);

    let (_, _, none) = get(
        &app,
        &format!("{ORG}/issues/CHECKOUT-API-3F/autofix/?llmFormat=markdown"),
    )
    .await;
    assert_eq!(none, json!({ "autofix": null }));
    let (status, _, run) = call(
        &app,
        "POST",
        &format!("{ORG}/issues/CHECKOUT-API-3F/autofix/"),
        r#"{"instruction":""}"#,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (_, _, done) = get(&app, &format!("{ORG}/issues/6184420311/autofix/")).await;
    assert_eq!(done["autofix"]["run_id"], run["run_id"]);
    assert_eq!(done["autofix"]["status"], "completed");
    assert!(
        done["autofix"]["blocks"][0]["artifacts"][0]["data"]["one_line_description"]
            .as_str()
            .unwrap()
            .contains("create_order")
    );
    assert_eq!(state(&w)["autofix"]["6184420311"]["status"], "completed");
}

#[tokio::test]
async fn long_tail_resources() {
    let (w, app) = app();
    for path in [
        "monitors/",
        "uptime/",
        "dashboards/",
        "alert-rules/",
        "agents/conversations/",
        "repos/",
    ] {
        let (status, _, body) = get(&app, &format!("{ORG}/{path}")).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert!(body.is_array(), "{path}");
    }
    let (_, _, repos) = get(&app, &format!("{ORG}/repos/?per_page=100")).await;
    assert_eq!(
        field(&repos, "name"),
        ["lumen-labs/storefront", "lumen-labs/checkout"]
    );
    let (_, _, replays) = get(&app, &format!("{ORG}/replays/")).await;
    assert_eq!(replays, json!({ "data": [] }));
    let (status, _, _) = get(&app, &format!("{ORG}/monitors/nightly/")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Every project has an issue_stream detector that alerts can attach to.
    let (_, _, detectors) = get(
        &app,
        &format!("{ORG}/detectors/?project=4508812003&type=issue_stream"),
    )
    .await;
    assert_eq!(field(&detectors, "projectId"), ["4508812003"]);
    let detector = detectors[0]["id"].as_str().unwrap().to_owned();
    let (status, _, rule) = call(
        &app,
        "POST",
        &format!("{ORG}/workflows/"),
        &format!(
            r#"{{"name":"New checkout errors","detectorIds":["{detector}"],"actionFilters":[]}}"#
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = rule["id"].as_str().unwrap().to_owned();
    let (_, _, scope) = get(&app, &format!("{ORG}/workflows/{id}/project-scope/")).await;
    assert_eq!(
        scope,
        json!({ "projectIds": ["4508812003"], "includesAllProjects": false })
    );
    let (_, _, rules) = get(
        &app,
        &format!("{ORG}/workflows/?sortBy=-id&query=name%3A%22*checkout*%22"),
    )
    .await;
    assert_eq!(field(&rules, "name"), ["New checkout errors"]);
    assert_eq!(state(&w)["workflows"][&id]["name"], "New checkout errors");
    let (status, _, _) = call(&app, "DELETE", &format!("{ORG}/workflows/{id}/"), "").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = get(&app, &format!("{ORG}/workflows/{id}/")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _, up) = call(&app, "POST", "/api/0/projects/lumen-labs/checkout-api/uptime/", r#"{"name":"Checkout health","url":"https://checkout.lumenlabs.dev/healthz","intervalSeconds":60}"#).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(up["projectSlug"], "checkout-api");
    let (status, _, _) = call(
        &app,
        "POST",
        "/api/0/projects/lumen-labs/checkout-api/uptime/",
        r#"{"name":"x"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (_, _, link) = get(&app, "/api/0/projects/lumen-labs/checkout-api/stacktrace-link/?file=checkout%2Fviews%2Forders.py").await;
    assert_eq!(link["error"], "no_code_mappings_for_project");
    let (_, _, envs) = get(&app, &format!("{ORG}/environments/")).await;
    assert_eq!(
        field(&envs, "name"),
        ["production", "staging", "testflight"]
    );
    let (_, _, tags) = get(&app, &format!("{ORG}/tags/?dataset=events")).await;
    assert!(field(&tags, "key").contains(&"browser".to_owned()));

    let (status, _, body) = get(&app, "/api/0/unknown/thing/").await;
    assert_eq!((status, body), (StatusCode::NOT_FOUND, not_found()));
}
