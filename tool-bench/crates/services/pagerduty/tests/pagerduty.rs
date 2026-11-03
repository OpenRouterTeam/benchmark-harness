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
use mock_pagerduty::Pagerduty;
use serde_json::{Value, json};
use tower::ServiceExt;

const ALEX: &str = "alex.kim@tidewater.io";

fn world() -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tidewater");
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

fn app() -> (SharedWorld, Router) {
    let w = world();
    let app = service_app(w.clone(), &Pagerduty);
    (w, app)
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    body: Value,
    from: Option<&str>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "world:8111")
        .header("authorization", "Token token=u+toolbench")
        .header("content-type", "application/json");
    if let Some(f) = from {
        req = req.header("from", f);
    }
    let body = if body.is_null() {
        Body::empty()
    } else {
        Body::from(body.to_string())
    };
    let resp = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
    send(app, "GET", uri, Value::Null, None).await
}

fn ids(list: &Value) -> Vec<&str> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_str().unwrap())
        .collect()
}

fn state(w: &SharedWorld) -> Value {
    w.lock().unwrap().snapshot()["services"]["pagerduty"].clone()
}

fn not_found() -> Value {
    json!({ "error": { "message": "Not Found", "code": 2100 } })
}

// ---------------------------------------------------------------------------------------
// Users and teams
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn users_me_is_the_account_owner_with_full_fields() {
    let (_, app) = app();
    let (status, body) = get(&app, "/users/me").await;
    assert_eq!(status, StatusCode::OK);
    let me = &body["user"];
    assert_eq!(me["id"], "PA1EXKM");
    assert_eq!(me["email"], ALEX);
    assert_eq!(me["type"], "user");
    assert_eq!(me["self"], "http://world:8111/users/PA1EXKM");
    assert_eq!(
        me["html_url"],
        "https://tidewater.pagerduty.com/users/PA1EXKM"
    );
    assert_eq!(me["teams"][0]["summary"], "Platform");
    assert_eq!(me["teams"][0]["type"], "team_reference");
}

#[tokio::test]
async fn users_me_works_without_any_dataset() {
    let dir = std::env::temp_dir().join("pd-empty-fixture");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("scenario.toml"),
        "[scenario]\nid = \"t\"\nnow = \"2026-10-30T14:00:00Z\"\nseed = \"t\"\n",
    )
    .unwrap();
    let w = shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap());
    let app = service_app(w, &Pagerduty);
    let (status, body) = get(&app, "/users/me").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["user"]["email"].is_string());
    assert!(body["user"]["teams"].is_array());
    let (_, incidents) = get(&app, "/incidents").await;
    assert_eq!(incidents["incidents"], json!([]));
    assert_eq!(incidents["more"], false);
}

#[tokio::test]
async fn list_users_filters_and_paginates_classically() {
    let (_, app) = app();
    let (_, body) = get(&app, "/users?query=jordan").await;
    assert_eq!(ids(&body["users"]), ["PJ0RALV", "PJ0RLEE"]);
    let (_, body) = get(&app, "/users?team_ids%5B%5D=PTDBREL").await;
    assert_eq!(ids(&body["users"]), ["PDNGYN2", "PS4MOKF"]);

    let (_, page1) = get(&app, "/users?limit=4&offset=0&total=true").await;
    assert_eq!(page1["users"].as_array().unwrap().len(), 4);
    assert_eq!(page1["limit"], 4);
    assert_eq!(page1["offset"], 0);
    assert_eq!(page1["more"], true);
    assert_eq!(page1["total"], 11);
    let (_, page3) = get(&app, "/users?limit=4&offset=8").await;
    assert_eq!(page3["users"].as_array().unwrap().len(), 3);
    assert_eq!(page3["more"], false);
    assert_eq!(page3["total"], Value::Null);

    let (status, body) = get(&app, "/users?limit=100&offset=9950").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], 2001);
}

#[tokio::test]
async fn create_user_validates_and_persists() {
    let (w, app) = app();
    let (status, body) = send(
        &app,
        "POST",
        "/users",
        json!({ "user": { "name": "Nora Fields", "email": "nora@tidewater.io", "role": "user", "time_zone": "Europe/Dublin" } }),
        Some(ALEX),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = body["user"]["id"].as_str().unwrap().to_owned();
    assert_eq!(id.len(), 7);
    assert!(id.starts_with('P'));
    assert_eq!(state(&w)["users"][&id]["email"], "nora@tidewater.io");

    let (status, body) = send(
        &app,
        "POST",
        "/users",
        json!({ "user": { "name": "Dup", "email": ALEX } }),
        Some(ALEX),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body,
        json!({ "error": { "message": "Invalid Input Provided", "code": 2001, "errors": ["Email has already been taken"] } })
    );
}

#[tokio::test]
async fn teams_crud_and_membership() {
    let (w, app) = app();
    let (_, body) = get(&app, "/teams/PTPLATF/members").await;
    let members = body["members"].as_array().unwrap();
    assert_eq!(members.len(), 3);
    assert_eq!(members[0]["user"]["summary"], "Alex Kim");
    assert_eq!(members[0]["role"], "manager");
    assert_eq!(members[1]["role"], "responder");

    let (status, body) = send(
        &app,
        "POST",
        "/teams",
        json!({ "team": { "name": "Security" } }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let team = body["team"]["id"].as_str().unwrap().to_owned();
    let (status, _) = send(
        &app,
        "POST",
        "/teams",
        json!({ "team": { "name": "security" } }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = send(
        &app,
        "PUT",
        &format!("/teams/{team}/users/PJ0RLEE"),
        json!({ "role": "responder" }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let s = state(&w);
    assert!(
        s["users"]["PJ0RLEE"]["teams"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == team.as_str())
    );
    let (_, body) = get(&app, &format!("/teams/{team}/members")).await;
    assert_eq!(body["members"][0]["role"], "responder");

    let (status, _) = send(
        &app,
        "DELETE",
        &format!("/teams/{team}/users/PJ0RLEE"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = send(
        &app,
        "DELETE",
        &format!("/teams/{team}/users/PJ0RLEE"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!((status, body), (StatusCode::NOT_FOUND, not_found()));

    let (status, _) = send(&app, "DELETE", "/teams/PTPLATF", Value::Null, None).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "team with policies cannot be deleted"
    );
    let (status, _) = send(&app, "DELETE", &format!("/teams/{team}"), Value::Null, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(state(&w)["teams"].get(&team).is_none());
}

// ---------------------------------------------------------------------------------------
// Services, escalation policies, dependencies
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn services_list_get_status_and_writes() {
    let (w, app) = app();
    let (_, body) = get(&app, "/services?query=checkout").await;
    assert_eq!(ids(&body["services"]), ["PSVCHKA", "PSVCHKS", "PSVCHKW"]);
    let (_, body) = get(&app, "/services/PSVCHKA").await;
    let svc = &body["service"];
    assert_eq!(svc["status"], "critical", "open high-urgency incident");
    assert_eq!(svc["escalation_policy"]["summary"], "Commerce");
    assert_eq!(svc["last_incident_timestamp"], "2026-10-30T12:40:00Z");
    let (_, body) = get(&app, "/services/PSVK8SC").await;
    assert_eq!(body["service"]["status"], "active");

    let (status, body) = send(
        &app,
        "POST",
        "/services",
        json!({ "service": { "id": null, "name": "search-api", "description": "Search", "escalation_policy": { "id": "PEPCOMM" }, "teams": null, "status": null, "type": "service" } }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = body["service"]["id"].as_str().unwrap().to_owned();
    assert_eq!(body["service"]["status"], "active");
    let (status, body) = send(
        &app,
        "POST",
        "/services",
        json!({ "service": { "name": "search-api", "escalation_policy": { "id": "PNOPE00" } } }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["error"]["errors"],
        json!(["Name has already been taken", "Escalation policy not found"])
    );

    let (status, _) = send(
        &app,
        "PUT",
        &format!("/services/{id}"),
        json!({ "service": { "description": "Product search", "status": "disabled", "escalation_policy": { "id": "PEPPLAT" } } }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let s = state(&w);
    assert_eq!(s["services"][&id]["status"], "disabled");
    assert_eq!(s["services"][&id]["escalation_policy"]["id"], "PEPPLAT");
    let (status, body) = get(&app, "/services/PNOPE00").await;
    assert_eq!((status, body), (StatusCode::NOT_FOUND, not_found()));
}

#[tokio::test]
async fn escalation_policies_include_services_and_resolve_users() {
    let (_, app) = app();
    let (_, body) = get(&app, "/escalation_policies?query=payments").await;
    assert_eq!(ids(&body["escalation_policies"]), ["PEPPAY1", "PEPPAYL"]);
    let (_, body) = get(&app, "/escalation_policies?user_ids%5B%5D=PDNGYN2").await;
    assert_eq!(ids(&body["escalation_policies"]), ["PEPDBRE"]);
    let (_, body) = get(&app, "/escalation_policies/PEPPAY1").await;
    let ep = &body["escalation_policy"];
    assert_eq!(ep["services"][0]["summary"], "payments-api");
    assert_eq!(
        ep["escalation_rules"][0]["targets"][0]["summary"],
        "Payments Primary"
    );
    assert_eq!(
        ep["escalation_rules"][1]["targets"][0]["type"],
        "user_reference"
    );
}

#[tokio::test]
async fn dependencies_priorities_and_business_services() {
    let (_, app) = app();
    let (_, body) = get(&app, "/service_dependencies/technical_services/PSVCHKA").await;
    let rels = body["relationships"].as_array().unwrap();
    assert_eq!(rels.len(), 3);
    assert_eq!(
        rels[0]["dependent_service"]["type"],
        "business_service_reference"
    );
    let (_, body) = get(&app, "/service_dependencies/business_services/PBSCHKO").await;
    assert_eq!(body["relationships"].as_array().unwrap().len(), 1);
    let (status, _) = get(&app, "/service_dependencies/business_services/PNOPE00").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, body) = get(&app, "/priorities").await;
    assert_eq!(body["priorities"][0]["summary"], "P1");
    assert_eq!(body["priorities"][0]["type"], "priority");
    let (_, body) = get(&app, "/business_services").await;
    assert_eq!(ids(&body["business_services"]), ["PBSCHKO", "PBSPAYM"]);
}

// ---------------------------------------------------------------------------------------
// Schedules and on-call
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn oncalls_follow_rotations_and_overrides() {
    let (_, app) = app();
    let (_, body) = get(&app, "/oncalls?escalation_policy_ids%5B%5D=PEPPAY1").await;
    let oncalls = body["oncalls"].as_array().unwrap();
    assert_eq!(oncalls.len(), 2);
    // Riya is scheduled this week but Priya's override covers today.
    assert_eq!(oncalls[0]["user"]["summary"], "Priya Raman");
    assert_eq!(oncalls[0]["escalation_level"], 1);
    assert_eq!(oncalls[0]["start"], "2026-10-30T06:00:00Z");
    assert_eq!(oncalls[0]["end"], "2026-10-31T06:00:00Z");
    assert_eq!(oncalls[0]["schedule"]["summary"], "Payments Primary");
    assert_eq!(oncalls[1]["user"]["summary"], "Marcus Chen");
    assert_eq!(oncalls[1]["start"], Value::Null);

    let (_, body) = get(&app, "/oncalls?escalation_policy_ids%5B%5D=PEPDBRE").await;
    assert_eq!(body["oncalls"][0]["user"]["id"], "PDNGYN2");
    assert_eq!(body["oncalls"][0]["start"], "2026-10-30T00:00:00Z");
    assert_eq!(body["oncalls"][0]["end"], "2026-10-31T00:00:00Z");

    // A window returns every shift in it; earliest keeps one per user and level.
    let uri =
        "/oncalls?schedule_ids%5B%5D=PSPAYPR&since=2026-10-30T00:00:00Z&until=2026-11-03T00:00:00Z";
    let (_, body) = get(&app, uri).await;
    let users: Vec<&str> = body["oncalls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["user"]["id"].as_str().unwrap())
        .collect();
    assert_eq!(users, ["PR1YPTL", "PPR1YAR", "PR1YPTL", "PPR1YAR"]);
    let (_, body) = get(&app, &format!("{uri}&earliest=true")).await;
    assert_eq!(body["oncalls"].as_array().unwrap().len(), 2);

    let (status, _) = get(
        &app,
        "/oncalls?since=2026-01-01T00:00:00Z&until=2026-10-30T00:00:00Z",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn schedules_v2_render_layers_and_redirect_v3() {
    let (_, app) = app();
    let (_, body) = get(&app, "/schedules?query=platform").await;
    assert_eq!(ids(&body["schedules"]), ["PSDPLPR", "PSPLTPR", "PSPLTSC"]);
    assert!(body["schedules"][0].get("schedule_layers").is_none());

    let (_, body) = get(
        &app,
        "/schedules/PSPLTPR?since=2026-10-26T16:00:00Z&until=2026-11-09T16:00:00Z",
    )
    .await;
    let s = &body["schedule"];
    assert_eq!(
        s["schedule_layers"][0]["users"][1]["user"]["summary"],
        "Alex Kim"
    );
    let entries = s["final_schedule"]["rendered_schedule_entries"]
        .as_array()
        .unwrap();
    assert_eq!(entries[0]["user"]["id"], "PA1EXKM");
    assert_eq!(entries[1]["user"]["id"], "PJ0RLEE");
    assert_eq!(s["final_schedule"]["rendered_coverage_percentage"], 100.0);
    assert_eq!(s["escalation_policies"][0]["id"], "PEPPLAT");

    let (status, body) = get(&app, "/schedules/PSV3SEC").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("shift-based")
    );
    let (status, body) = get(&app, "/v3/schedules/PSPLTPR").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("layer-based")
    );

    let (_, body) = get(&app, "/schedules/PSPAYPR/users").await;
    assert_eq!(ids(&body["users"]), ["PR1YPTL", "PPR1YAR", "PJ0RALV"]);
}

#[tokio::test]
async fn overrides_are_stored_per_schedule_and_change_oncall() {
    let (w, app) = app();
    let (status, body) = send(
        &app,
        "POST",
        "/schedules/PSPLTPR/overrides",
        json!({ "overrides": [
            { "start": "2026-10-31T09:00:00+00:00", "end": "2026-10-31T17:00:00+00:00", "user": { "id": "PJ0RLEE", "type": "user_reference" } },
            { "start": "2026-10-31T09:00:00Z", "end": "2026-10-31T08:00:00Z", "user": { "id": "PJ0RLEE", "type": "user_reference" } }
        ] }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body[0]["status"], 201);
    assert_eq!(body[0]["override"]["user"]["summary"], "Jordan Lee");
    assert_eq!(body[1]["status"], 400);
    let o = &state(&w)["overrides"]["PSPLTPR/1"];
    assert_eq!(o["start"], "2026-10-31T09:00:00Z");
    assert_eq!(o["end"], "2026-10-31T17:00:00Z");
    assert_eq!(o["user"]["id"], "PJ0RLEE");

    let (_, body) = get(
        &app,
        "/oncalls?schedule_ids%5B%5D=PSPLTPR&since=2026-10-31T12:00:00Z&until=2026-10-31T12:00:00Z",
    )
    .await;
    assert_eq!(body["oncalls"][0]["user"]["id"], "PJ0RLEE");
    assert_eq!(body["oncalls"][0]["start"], "2026-10-31T09:00:00Z");

    let (_, body) = get(
        &app,
        "/schedules/PSPLTPR/overrides?since=2026-10-31T00:00:00Z&until=2026-11-01T00:00:00Z",
    )
    .await;
    let oid = body["overrides"][0]["id"].as_str().unwrap().to_owned();
    let (status, _) = send(
        &app,
        "DELETE",
        &format!("/schedules/PSPLTPR/overrides/{oid}"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(state(&w)["overrides"].get("PSPLTPR/1").is_none());
    let (status, _) = send(
        &app,
        "POST",
        "/schedules/PNOPE00/overrides",
        json!({ "overrides": [] }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn v3_schedules_full_lifecycle() {
    let (w, app) = app();
    let (_, body) = get(&app, "/v3/schedules").await;
    assert_eq!(ids(&body["schedules"]), ["PSV3SEC"]);
    assert!(
        body["schedules"][0].get("name").is_none(),
        "v3 list omits names"
    );

    let (status, body) = send(
        &app,
        "POST",
        "/v3/schedules",
        json!({ "schedule": { "name": "x", "time_zone": "" } }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["error"]["errors"],
        json!(["Time zone cannot be empty."])
    );

    let (status, body) = send(
        &app,
        "POST",
        "/v3/schedules",
        json!({ "schedule": { "name": "Search On-Call", "time_zone": "Etc/UTC" } }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let sid = body["schedule"]["id"].as_str().unwrap().to_owned();
    let (status, body) = send(
        &app,
        "POST",
        &format!("/v3/schedules/{sid}/rotations"),
        json!({}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let rid = body["rotation"]["id"].as_str().unwrap().to_owned();
    let event = json!({ "event": {
        "name": "Weekly", "effective_since": "2026-10-26T09:00:00Z",
        "start_time": { "date_time": "2026-10-26T09:00:00Z", "time_zone": "Etc/UTC" },
        "end_time": { "date_time": "2026-11-02T09:00:00Z", "time_zone": "Etc/UTC" },
        "recurrence": ["RRULE:FREQ=WEEKLY"],
        "assignment_strategy": { "type": "rotating_member_assignment_strategy", "shifts_per_member": 1,
            "members": [{ "type": "user_member", "user_id": "PLWEBR5" }, { "type": "user_member", "user_id": "PTBRWN8" }] }
    } });
    let (status, body) = send(
        &app,
        "POST",
        &format!("/v3/schedules/{sid}/rotations/{rid}/events"),
        event,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let eid = body["event"]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        state(&w)["schedules_v3"][&sid]["rotations"][0]["events"][0]["id"],
        eid.as_str()
    );

    let (status, body) = send(
        &app,
        "POST",
        &format!("/v3/schedules/{sid}/overrides"),
        json!({ "overrides": [{ "type": "override_shift", "rotation_id": rid, "start_time": "2026-10-30T12:00:00Z", "end_time": "2026-10-30T18:00:00Z",
            "overridden_member": { "type": "user_member", "user_id": "PLWEBR5" }, "overriding_member": { "type": "user_member", "user_id": "PA1EXKM" } }] }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let oid = body["overrides"][0]["id"].as_str().unwrap().to_owned();
    let (status, body) = send(
        &app,
        "POST",
        &format!("/v3/schedules/{sid}/custom_shifts"),
        json!({ "custom_shifts": [{ "type": "custom_shift", "start_time": "2026-10-31T00:00:00Z", "end_time": "2026-10-31T06:00:00Z",
            "assignments": [{ "type": "shift_assignment", "member": { "type": "user_member", "user_id": "PM3IWNG" } }] }] }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let cid = body["custom_shifts"][0]["id"].as_str().unwrap().to_owned();

    let (_, body) = get(
        &app,
        &format!(
            "/v3/schedules/{sid}/overrides?since=2026-10-30T00:00:00Z&until=2026-10-31T00:00:00Z"
        ),
    )
    .await;
    assert_eq!(ids(&body["overrides"]), [oid.as_str()]);
    let (status, _) = get(&app, &format!("/v3/schedules/{sid}/custom_shifts")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "since/until required");
    let (_, body) = get(&app, &format!("/v3/schedules/{sid}/custom_shifts/{cid}")).await;
    assert_eq!(
        body["custom_shift"]["assignments"][0]["member"]["user_id"],
        "PM3IWNG"
    );

    let (status, _) = send(
        &app,
        "DELETE",
        &format!("/v3/schedules/{sid}/overrides/{oid}"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(
        &app,
        "DELETE",
        &format!("/v3/schedules/{sid}/rotations/{rid}/events/{eid}"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(
        &app,
        "DELETE",
        &format!("/v3/schedules/{sid}"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let s = state(&w);
    assert!(s["schedules_v3"].get(&sid).is_none());
    assert!(s["v3_custom_shifts"].get(format!("{sid}/1")).is_none());
}

#[tokio::test]
async fn v3_rotation_events_render_into_oncalls_when_targeted() {
    let (w, app) = app();
    // Point the Platform policy's second level at the v3 security schedule.
    {
        let mut guard = w.lock().unwrap();
        let svc = guard.service_mut("pagerduty");
        let ep = svc
            .store
            .collection_mut("escalation_policies")
            .get_mut("PEPPLAT")
            .unwrap();
        ep["escalation_rules"][1]["targets"] =
            json!([{ "id": "PSV3SEC", "type": "schedule_v3_reference" }]);
    }
    let (_, body) = get(&app, "/oncalls?escalation_policy_ids%5B%5D=PEPPLAT").await;
    let oncalls = body["oncalls"].as_array().unwrap();
    assert_eq!(oncalls[1]["escalation_level"], 2);
    // Weekday 08:00-20:00 shifts, five per member from Oct 5: Mei, Sam, Mei, then Sam for
    // the week of Oct 26.
    assert_eq!(oncalls[1]["user"]["id"], "PS4MOKF");
    assert_eq!(oncalls[1]["start"], "2026-10-30T08:00:00Z");
    assert_eq!(oncalls[1]["end"], "2026-10-30T20:00:00Z");
    assert_eq!(oncalls[1]["schedule"]["type"], "schedule_v3_reference");
}

// ---------------------------------------------------------------------------------------
// Incidents
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn incidents_list_default_window_filters_and_lookup_by_number() {
    let (_, app) = app();
    let (_, body) = get(&app, "/incidents?limit=100").await;
    let all = ids(&body["incidents"]);
    assert_eq!(
        all.len(),
        12,
        "all fixture incidents are within the last 30 days"
    );
    let (_, body) = get(
        &app,
        "/incidents?statuses%5B%5D=triggered&urgencies%5B%5D=high&service_ids%5B%5D=PSVCHKA",
    )
    .await;
    assert_eq!(ids(&body["incidents"]), ["Q7CHKAP15PCT1D"]);
    let (_, body) = get(
        &app,
        "/incidents?user_ids%5B%5D=PA1EXKM&sort_by=incident_number:desc",
    )
    .await;
    assert_eq!(
        ids(&body["incidents"]),
        ["Q1INVDBLAG7QWE", "Q3K8ORDBLAG2RX", "Q0AP1GW5XX9LMZ"]
    );
    let (_, body) = get(
        &app,
        "/incidents?since=2026-10-30T12:00:00Z&until=2026-10-30T13:00:00Z",
    )
    .await;
    assert_eq!(
        ids(&body["incidents"]),
        ["Q7CHKAP15PCT1D", "Q9CHKWEB5XX3VK"]
    );
    let (_, body) = get(&app, "/incidents?priority_ids%5B%5D=PPRIO01").await;
    assert_eq!(ids(&body["incidents"]), ["Q7CHKAP15PCT1D"]);

    let (_, body) = get(&app, "/incidents/4811").await;
    let inc = &body["incident"];
    assert_eq!(inc["id"], "Q8CHKSTG5PCT2F");
    assert_eq!(inc["summary"], "[#4811] Checkout error rate above 5%");
    assert_eq!(inc["service"]["summary"], "checkout-api-staging");
    assert_eq!(
        inc["alert_counts"],
        json!({ "all": 1, "triggered": 1, "resolved": 0 })
    );
    assert_eq!(
        inc["first_trigger_log_entry"]["type"],
        "trigger_log_entry_reference"
    );
    let (status, body) = get(&app, "/incidents/QNOPE").await;
    assert_eq!((status, body), (StatusCode::NOT_FOUND, not_found()));
}

#[tokio::test]
async fn manage_incidents_requires_from_and_records_timeline() {
    let (w, app) = app();
    let ack = json!({ "incidents": [{ "id": "Q7CHKAP15PCT1D", "type": "incident_reference", "status": "acknowledged" }] });
    let (status, body) = send(&app, "PUT", "/incidents", ack.clone(), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]["errors"][0]
            .as_str()
            .unwrap()
            .contains("From")
    );

    let (status, body) = send(
        &app,
        "PUT",
        "/incidents",
        ack,
        Some("lena.weber@tidewater.io"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["incidents"][0]["status"], "acknowledged");
    assert_eq!(
        body["incidents"][0]["acknowledgements"][0]["acknowledger"]["summary"],
        "Lena Weber"
    );

    let resolve = json!({ "incidents": [{ "id": "Q7CHKAP15PCT1D", "type": "incident_reference", "status": "resolved" }] });
    let (status, _) = send(
        &app,
        "PUT",
        "/incidents",
        resolve.clone(),
        Some("lena.weber@tidewater.io"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let s = state(&w);
    let inc = &s["incidents"]["Q7CHKAP15PCT1D"];
    assert_eq!(inc["status"], "resolved");
    // Third mutating request (the rejected one ticks the clock too).
    assert_eq!(inc["resolved_at"], "2026-10-30T14:00:21Z");
    assert_eq!(inc["assignments"], json!([]));
    assert_eq!(s["alerts"]["PALR001"]["status"], "resolved");
    let (_, body) = get(&app, "/incidents/Q7CHKAP15PCT1D/log_entries").await;
    let types: Vec<&str> = body["log_entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        types,
        [
            "resolve_log_entry",
            "acknowledge_log_entry",
            "trigger_log_entry"
        ]
    );

    let (status, body) = send(
        &app,
        "PUT",
        "/incidents",
        resolve,
        Some("lena.weber@tidewater.io"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["error"]["errors"],
        json!(["Q7CHKAP15PCT1D: Incident Already Resolved"])
    );
    let (status, _) = send(
        &app,
        "PUT",
        "/incidents",
        json!({ "incidents": [{ "id": "QNOPE", "status": "acknowledged" }] }),
        Some(ALEX),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn reassign_and_escalate_use_oncall() {
    let (w, app) = app();
    let (status, _) = send(
        &app,
        "PUT",
        "/incidents",
        json!({ "incidents": [{ "id": "Q0AP1GW5XX9LMZ", "type": "incident_reference", "assignments": [{ "at": "2026-01-01T00:00:00", "assignee": { "id": "PDNGYN2", "type": "user_reference" } }] }] }),
        Some(ALEX),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let inc = &state(&w)["incidents"]["Q0AP1GW5XX9LMZ"];
    assert_eq!(inc["assignments"][0]["assignee"]["id"], "PDNGYN2");
    assert_eq!(inc["assignments"][0]["at"], "2026-10-30T14:00:07Z");
    assert_eq!(
        inc["status"], "triggered",
        "reassigning an acknowledged incident re-triggers it"
    );

    let (status, body) = send(
        &app,
        "PUT",
        "/incidents",
        json!({ "incidents": [{ "id": "Q4PAYDECL1NE5H", "type": "incident_reference", "escalation_level": 2 }] }),
        Some(ALEX),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["incidents"][0]["assignments"][0]["assignee"]["summary"],
        "Marcus Chen"
    );
    let (status, _) = send(
        &app,
        "PUT",
        "/incidents",
        json!({ "incidents": [{ "id": "Q4PAYDECL1NE5H", "escalation_level": 3 }] }),
        Some(ALEX),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_incident_assigns_oncall_and_numbers_from_dataset() {
    let (w, app) = app();
    let (status, body) = send(
        &app,
        "POST",
        "/incidents",
        json!({ "incident": { "type": "incident", "title": "Payments API 502s", "service": { "id": "PSVPAYA", "type": "service_reference" }, "urgency": "high", "body": { "type": "incident_body", "details": "502s from the LB" } } }),
        Some(ALEX),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let inc = &body["incident"];
    let id = inc["id"].as_str().unwrap().to_owned();
    assert_eq!(id.len(), 14);
    assert_eq!(inc["incident_number"], 4815);
    assert_eq!(inc["assignments"][0]["assignee"]["id"], "PPR1YAR");
    assert_eq!(inc["escalation_policy"]["id"], "PEPPAY1");
    assert_eq!(
        state(&w)["incidents"][&id]["body"]["details"],
        "502s from the LB"
    );

    let (status, body) = send(
        &app,
        "POST",
        "/incidents",
        json!({ "incident": { "title": "", "service": { "id": "PNOPE00" } } }),
        Some(ALEX),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["error"]["errors"],
        json!(["Title can't be blank", "Service not found"])
    );
}

#[tokio::test]
async fn notes_alerts_and_responders() {
    let (w, app) = app();
    let (_, body) = get(&app, "/incidents/Q9CHKWEB5XX3VK/notes").await;
    assert_eq!(body["notes"][0]["user"]["summary"], "Lena Weber");
    let (status, body) = send(
        &app,
        "POST",
        "/incidents/Q9CHKWEB5XX3VK/notes",
        json!({ "note": { "content": "Ingress restarted" } }),
        Some(ALEX),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["note"]["user"]["id"], "PA1EXKM");
    assert_eq!(
        state(&w)["notes"]["Q9CHKWEB5XX3VK/2"]["content"],
        "Ingress restarted"
    );
    let (status, _) = send(
        &app,
        "POST",
        "/incidents/Q9CHKWEB5XX3VK/notes",
        json!({ "note": { "content": " " } }),
        Some(ALEX),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (_, body) = get(&app, "/incidents/Q7CHKAP15PCT1D/alerts?limit=1").await;
    assert_eq!(ids(&body["alerts"]), ["PALR001"]);
    assert_eq!(body["more"], true);
    let (_, body) = get(&app, "/incidents/Q7CHKAP15PCT1D/alerts/PALR002").await;
    assert_eq!(
        body["alert"]["incident"]["summary"],
        "[#4810] Checkout error rate above 5%"
    );
    let (status, _) = get(&app, "/incidents/Q7CHKAP15PCT1D/alerts/PALR004").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body) = send(
        &app,
        "POST",
        "/incidents/Q7CHKAP15PCT1D/responder_requests",
        json!({ "requester_id": "PA1EXKM", "message": "help", "responder_request_targets": [{ "responder_request_target": { "id": "PEPDBRE", "type": "escalation_policy_reference" } }] }),
        Some(ALEX),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let target =
        &body["responder_request"]["responder_request_targets"][0]["responder_request_target"];
    assert_eq!(target["incidents_responders"][0]["user"]["id"], "PDNGYN2");
    let inc = &state(&w)["incidents"]["Q7CHKAP15PCT1D"];
    assert_eq!(inc["incidents_responders"][0]["state"], "pending");
}

#[tokio::test]
async fn snooze_merge_and_status_updates() {
    let (w, app) = app();
    let (status, _) = send(
        &app,
        "POST",
        "/incidents/Q7CHKAP15PCT1D/snooze",
        json!({ "duration": 3600 }),
        Some(ALEX),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "only acknowledged incidents snooze"
    );
    let (status, body) = send(
        &app,
        "POST",
        "/incidents/Q9CHKWEB5XX3VK/snooze",
        json!({ "duration": 3600 }),
        Some(ALEX),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        body["incident"]["pending_actions"][0]["type"],
        "unacknowledge"
    );

    let (status, body) = send(
        &app,
        "PUT",
        "/incidents/Q7CHKAP15PCT1D/merge",
        json!({ "source_incidents": [{ "id": "Q9CHKWEB5XX3VK", "type": "incident_reference" }] }),
        Some(ALEX),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["incident"]["id"], "Q7CHKAP15PCT1D");
    let s = state(&w);
    assert_eq!(s["incidents"]["Q9CHKWEB5XX3VK"]["status"], "resolved");

    let (status, body) = send(
        &app,
        "POST",
        "/incidents/Q7CHKAP15PCT1D/status_updates",
        json!({ "message": "Rolling back 2026.10.29.3" }),
        Some(ALEX),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status_update"]["sender"]["summary"], "Alex Kim");
    assert_eq!(
        state(&w)["status_updates"]["Q7CHKAP15PCT1D/1"]["message"],
        "Rolling back 2026.10.29.3"
    );
}

#[tokio::test]
async fn event_intelligence_views() {
    let (_, app) = app();
    let (_, body) = get(&app, "/incidents/Q7CHKAP15PCT1D/outlier_incident").await;
    let o = &body["outlier_incident"];
    assert_eq!(o["incident"]["occurrence"]["count"], 2);
    assert_eq!(
        o["incident_template"]["mined_text"],
        "Checkout error rate above <*>%"
    );

    let (_, body) = get(
        &app,
        "/incidents/Q7CHKAP15PCT1D/past_incidents?limit=1&total=true",
    )
    .await;
    assert_eq!(
        body["past_incidents"][0]["incident"]["id"],
        "QA6CHKAPIOLD0D"
    );
    assert_eq!(body["past_incidents"][0]["score"], 100.0);
    assert_eq!(body["total"], 2);
    assert_eq!(body["limit"], 1);

    let (_, body) = get(&app, "/incidents/Q7CHKAP15PCT1D/related_incidents").await;
    let related = body["related_incidents"].as_array().unwrap();
    assert_eq!(related[0]["incident"]["id"], "Q4PAYDECL1NE5H");
    assert_eq!(related[0]["relationships"][0]["type"], "service_dependency");

    let (_, body) = get(&app, "/incidents/Q7CHKAP15PCT1D/related_change_events").await;
    // Same-service deploy, then the config change on orders-db (which checkout-api depends on).
    assert_eq!(ids(&body["change_events"]), ["PCE0001", "PCE0003"]);
    assert_eq!(
        body["change_events"][0]["correlation_reason"]["reason"],
        "recent"
    );
    assert_eq!(
        body["change_events"][1]["correlation_reason"]["reason"],
        "related_service"
    );
}

#[tokio::test]
async fn log_entries_and_change_events() {
    let (_, app) = app();
    let (_, body) = get(
        &app,
        "/log_entries?since=2026-10-30T12:00:00Z&until=2026-10-30T13:00:00Z",
    )
    .await;
    let entries = body["log_entries"].as_array().unwrap();
    assert_eq!(entries[0]["created_at"], "2026-10-30T12:47:00Z");
    assert_eq!(entries[0]["type"], "acknowledge_log_entry");
    assert_eq!(entries[0]["agent"]["summary"], "Lena Weber");
    let (_, body) = get(&app, "/log_entries/PLE0001").await;
    assert_eq!(body["log_entry"]["incident"]["id"], "Q0AP1GW5XX9LMZ");
    let (status, _) = get(&app, "/log_entries/PNOPE00").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (_, body) = get(&app, "/change_events?team_ids%5B%5D=PTCOMRC").await;
    assert_eq!(ids(&body["change_events"]), ["PCE0002", "PCE0001"]);
    let (_, body) = get(&app, "/services/PSVORDB/change_events").await;
    assert_eq!(ids(&body["change_events"]), ["PCE0003"]);
    let (_, body) = get(&app, "/change_events/PCE0001").await;
    assert_eq!(
        body["change_event"]["services"][0]["summary"],
        "checkout-api"
    );
}

// ---------------------------------------------------------------------------------------
// Configuration resources
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn event_orchestration_router_update_assigns_rule_ids() {
    let (w, app) = app();
    let (_, body) = get(&app, "/event_orchestrations").await;
    assert_eq!(ids(&body["orchestrations"]), ["PEO0001"]);
    assert_eq!(body["orchestrations"][0]["routes"], 2);
    let (_, body) = get(&app, "/event_orchestrations/PEO0001/router").await;
    let path = &body["orchestration_path"];
    assert_eq!(path["type"], "router");
    assert_eq!(path["parent"]["id"], "PEO0001");
    assert_eq!(path["sets"][0]["rules"].as_array().unwrap().len(), 2);

    let mut sets = path["sets"].clone();
    sets[0]["rules"].as_array_mut().unwrap().push(json!({
        "id": "temp_id_will_be_replaced_by_api", "label": "Orders DB",
        "conditions": [{ "expression": "event.summary matches part 'orders-db'" }],
        "actions": { "route_to": "PSVORDB" }
    }));
    let (status, body) = send(
        &app,
        "PUT",
        "/event_orchestrations/PEO0001/router",
        json!({ "orchestration_path": { "type": "router", "sets": sets, "catch_all": path["catch_all"] } }),
        Some(ALEX),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rules = body["orchestration_path"]["sets"][0]["rules"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(rules[0]["id"], "8f2a1c3d", "existing rule IDs are kept");
    assert_ne!(rules[2]["id"], "temp_id_will_be_replaced_by_api");
    assert_eq!(body["orchestration_path"]["updated_by"]["id"], "PA1EXKM");
    assert_eq!(
        state(&w)["orchestration_routers"]["PEO0001"]["sets"][0]["rules"][2]["actions"]["route_to"],
        "PSVORDB"
    );
    let (_, body) = get(&app, "/event_orchestrations/PEO0001").await;
    assert_eq!(body["orchestration"]["routes"], 3);

    let (status, _) = send(
        &app,
        "PUT",
        "/event_orchestrations/PEO0001/router",
        json!({ "orchestration_path": { "sets": [{ "id": "start", "rules": [{ "conditions": [], "actions": { "route_to": "PNOPE00" } }] }], "catch_all": { "actions": { "route_to": "unrouted" } } } }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (_, body) = get(&app, "/event_orchestrations/PEO0001/global").await;
    assert_eq!(body["orchestration_path"]["type"], "global");
    let (_, body) = get(&app, "/event_orchestrations/services/PSVCHKA").await;
    assert_eq!(
        body["orchestration_path"]["parent"]["type"],
        "service_reference"
    );
    assert_eq!(
        body["orchestration_path"]["sets"][0]["rules"][0]["actions"]["suppress"],
        true
    );
    let (status, _) = get(&app, "/event_orchestrations/services/PNOPE00").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn incident_workflows_list_get_and_start() {
    let (w, app) = app();
    let (_, body) = get(&app, "/incident_workflows?query=major").await;
    assert_eq!(ids(&body["incident_workflows"]), ["PWF0001"]);
    assert!(body["incident_workflows"][0].get("steps").is_none());
    let (_, body) = get(&app, "/incident_workflows?include%5B%5D=steps").await;
    assert_eq!(body["incident_workflows"][0]["steps"][0]["type"], "step");
    let (status, body) = send(
        &app,
        "POST",
        "/incident_workflows/PWF0001/instances",
        json!({ "incident_workflow_instance": { "incident": { "id": "Q7CHKAP15PCT1D", "type": "incident_reference" } } }),
        Some(ALEX),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let iid = body["incident_workflow_instance"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        state(&w)["incident_workflow_instances"][&iid]["incident_workflow"]["id"],
        "PWF0001"
    );
    let (status, _) = send(
        &app,
        "POST",
        "/incident_workflows/PWF0001/instances",
        json!({ "incident_workflow_instance": { "incident": { "id": "QNOPE" } } }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = get(&app, "/incident_workflows/PNOPE00").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn alert_grouping_settings_cursor_pagination_and_validation() {
    let (w, app) = app();
    let (_, page1) = get(&app, "/alert_grouping_settings?limit=1").await;
    assert_eq!(ids(&page1["alert_grouping_settings"]), ["PAGS001"]);
    assert_eq!(page1["after"], "PAGS001");
    assert_eq!(page1["before"], Value::Null);
    let (_, page2) = get(&app, "/alert_grouping_settings?limit=1&after=PAGS001").await;
    assert_eq!(ids(&page2["alert_grouping_settings"]), ["PAGS002"]);
    assert_eq!(page2["after"], Value::Null);
    assert_eq!(page2["before"], "PAGS002");
    let (_, body) = get(&app, "/alert_grouping_settings?service_ids%5B%5D=PSVCHKA").await;
    assert_eq!(ids(&body["alert_grouping_settings"]), ["PAGS002"]);

    let (status, body) = send(
        &app,
        "POST",
        "/alert_grouping_settings",
        json!({ "alert_grouping_setting": { "type": "time", "config": { "timeout": 30 }, "services": [{ "id": "PSVORDB" }] } }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["errors"].as_array().unwrap().len(), 2);
    let (status, body) = send(
        &app,
        "POST",
        "/alert_grouping_settings",
        json!({ "alert_grouping_setting": { "name": "payments", "type": "time", "config": { "timeout": 900 }, "services": [{ "id": "PSVPAYA" }] } }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = body["alert_grouping_setting"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        state(&w)["alert_grouping_settings"][&id]["config"]["timeout"],
        900
    );
    let (status, _) = send(
        &app,
        "DELETE",
        &format!("/alert_grouping_settings/{id}"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = get(&app, &format!("/alert_grouping_settings/{id}")).await;
    assert_eq!((status, body), (StatusCode::NOT_FOUND, not_found()));
}

#[tokio::test]
async fn webhooks_return_secret_only_on_create() {
    let (w, app) = app();
    let (status, body) = send(
        &app,
        "POST",
        "/webhook_subscriptions",
        json!({ "webhook_subscription": { "type": "webhook_subscription", "delivery_method": { "type": "http_delivery_method", "url": "https://hooks.example.com/pd" },
            "events": ["incident.triggered"], "filter": { "type": "service_reference", "id": "PSVPAYA" } } }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let sub = &body["webhook_subscription"];
    let id = sub["id"].as_str().unwrap().to_owned();
    assert_eq!(sub["delivery_method"]["secret"].as_str().unwrap().len(), 64);
    assert_eq!(
        state(&w)["webhook_subscriptions"][&id]["delivery_method"]["secret"],
        Value::Null
    );
    let (_, body) = get(&app, &format!("/webhook_subscriptions/{id}")).await;
    assert_eq!(
        body["webhook_subscription"]["delivery_method"]["secret"],
        Value::Null
    );

    let (status, body) = send(
        &app,
        "POST",
        "/webhook_subscriptions",
        json!({ "webhook_subscription": { "delivery_method": { "url": "http://insecure" }, "events": ["incident.exploded"], "filter": { "type": "team_reference", "id": "PNOPE00" } } }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["errors"].as_array().unwrap().len(), 3);

    let (_, _) = send(
        &app,
        "PUT",
        "/webhook_subscriptions/PWHK001",
        json!({ "webhook_subscription": { "active": false } }),
        None,
    )
    .await;
    assert_eq!(
        state(&w)["webhook_subscriptions"]["PWHK001"]["active"],
        false
    );
    let (_, body) = get(&app, "/webhook_subscriptions?filter_type=service_reference").await;
    assert_eq!(
        ids(&body["webhook_subscriptions"]),
        ["PWHK002", id.as_str()]
    );
    let (status, _) = send(
        &app,
        "DELETE",
        "/webhook_subscriptions/PWHK002",
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn extension_schemas_default_to_builtin_vendors() {
    let (_, app) = app();
    let (_, body) = get(&app, "/extension_schemas").await;
    assert_eq!(
        ids(&body["extension_schemas"]),
        ["PJFWPEP", "PAKEDK0", "PD8SURB"]
    );
    let (_, body) = get(&app, "/extension_schemas/PJFWPEP").await;
    assert_eq!(body["extension_schema"]["key"], "generic_v2_webhook");
    let (status, _) = get(&app, "/extension_schemas/PNOPE00").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn status_pages_posts_and_updates() {
    let (w, app) = app();
    let (_, body) = get(&app, "/status_pages").await;
    assert_eq!(ids(&body["status_pages"]), ["PSTPG01"]);
    let (_, body) = get(&app, "/status_pages/PSTPG01/statuses?post_type=maintenance").await;
    assert_eq!(ids(&body["statuses"]), ["PSTS005", "PSTS006", "PSTS007"]);
    let (_, body) = get(&app, "/status_pages/PSTPG01/severities").await;
    assert_eq!(body["severities"][0]["status_page"]["id"], "PSTPG01");
    let (_, body) = get(&app, "/status_pages/PSTPG01/impacts").await;
    assert_eq!(body["impacts"].as_array().unwrap().len(), 5);
    let (status, _) = get(&app, "/status_pages/PNOPE00/impacts").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body) = send(
        &app,
        "POST",
        "/status_pages/PSTPG01/posts/PSPPST1/post_updates",
        json!({ "post_update": { "message": "Fix deployed, monitoring.", "status": { "id": "PSTS003" }, "severity": { "id": "PSEV001" }, "impacted_services": [], "notify_subscribers": false, "post": { "id": "PSPPST1" } } }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let uid = body["post_update"]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        state(&w)["status_page_post_updates"][&uid]["message"],
        "Fix deployed, monitoring."
    );
    let (_, body) = get(&app, "/status_pages/PSTPG01/posts/PSPPST1").await;
    assert_eq!(body["post"]["updates"].as_array().unwrap().len(), 2);
    let (_, body) = get(&app, "/status_pages/PSTPG01/posts/PSPPST1/post_updates").await;
    assert_eq!(body["post_updates"][1]["id"], uid.as_str());

    let (status, body) = send(
        &app,
        "POST",
        "/status_pages/PSTPG01/posts",
        json!({ "post": { "title": "DB maintenance", "post_type": "maintenance", "starts_at": "2026-11-07T02:00:00Z", "ends_at": "2026-11-07T04:00:00Z",
            "status_page": { "id": "PSTPG01" }, "updates": [{ "message": "Planned", "status": { "id": "PSTS001" }, "severity": { "id": "PSEV004" } }] } }),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "incident status on a maintenance post"
    );
    assert_eq!(
        body["error"]["errors"],
        json!(["status does not apply to maintenance posts"])
    );
    let (status, body) = send(
        &app,
        "POST",
        "/status_pages/PSTPG01/posts",
        json!({ "post": { "title": "DB maintenance", "post_type": "maintenance", "starts_at": "2026-11-07T02:00:00Z", "ends_at": "2026-11-07T04:00:00Z",
            "status_page": { "id": "PSTPG01" }, "updates": [{ "message": "Planned", "status": { "id": "PSTS005" }, "severity": { "id": "PSEV004" } }] } }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["post"]["updates"].as_array().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------------------
// Analytics
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn analytics_metrics_are_derived_from_incidents() {
    let (_, app) = app();
    let (status, body) = send(
        &app,
        "POST",
        "/analytics/metrics/incidents/all",
        json!({ "filters": { "created_at_start": "2026-10-01T00:00:00Z", "created_at_end": "2026-10-31T00:00:00Z", "service_ids": ["PSVCHKA"] } }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let m = &body["data"][0];
    assert_eq!(m["total_incident_count"], 3);
    assert_eq!(m["mean_seconds_to_first_ack"], 210);
    assert_eq!(m["mean_seconds_to_resolve"], 3360);
    assert_eq!(m["p50_seconds_to_resolve"], 2520);

    let (_, body) = send(
        &app,
        "POST",
        "/analytics/metrics/incidents/services",
        json!({ "filters": { "created_at_start": "2026-10-30T00:00:00Z", "created_at_end": "2026-10-31T00:00:00Z", "team_ids": ["PTCOMRC"] }, "order_by": "total_incident_count", "order": "desc" }),
        None,
    )
    .await;
    let rows = body["data"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["service_name"], "checkout-api");
    assert_eq!(rows[0]["team_name"], "Commerce");

    let (_, body) = send(
        &app,
        "POST",
        "/analytics/metrics/incidents/teams",
        json!({ "filters": { "created_at_start": "2026-10-01T00:00:00Z", "created_at_end": "2026-10-31T00:00:00Z" }, "aggregate_unit": "week" }),
        None,
    )
    .await;
    assert_eq!(body["data"][0]["range_start"], "2026-10-12T00:00:00Z");

    let (_, body) = send(
        &app,
        "POST",
        "/analytics/metrics/responders/all",
        json!({ "filters": { "date_range_start": "2026-10-01T00:00:00Z", "date_range_end": "2026-10-31T00:00:00Z", "responder_ids": ["PMCHEN7"] } }),
        None,
    )
    .await;
    let marcus = &body["data"][0];
    assert_eq!(
        marcus["total_seconds_on_call"],
        30 * 86_400,
        "permanent level-2 target"
    );
    assert_eq!(marcus["total_seconds_on_call_level_1"], 0);

    let (status, body) = send(
        &app,
        "POST",
        "/analytics/metrics/responders/teams",
        json!({ "filters": {} }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], 2001);
}

#[tokio::test]
async fn analytics_raw_incidents_cursor() {
    let (_, app) = app();
    let filters = json!({ "created_at_start": "2026-10-30T00:00:00Z", "created_at_end": "2026-10-31T00:00:00Z" });
    let (_, page1) = send(
        &app,
        "POST",
        "/analytics/raw/incidents",
        json!({ "filters": filters, "limit": 3 }),
        None,
    )
    .await;
    assert_eq!(page1["more"], true);
    assert_eq!(page1["first"], "Q4PAYDECL1NE5H");
    let last = page1["last"].as_str().unwrap().to_owned();
    let (_, page2) = send(
        &app,
        "POST",
        "/analytics/raw/incidents",
        json!({ "filters": filters, "limit": 3, "starting_after": last }),
        None,
    )
    .await;
    assert_eq!(page2["data"].as_array().unwrap().len(), 3);
    let (_, one) = get(&app, "/analytics/raw/incidents/Q9CHKWEB5XX3VK").await;
    assert_eq!(one["seconds_to_first_ack"], 120);
    assert_eq!(one["service_name"], "checkout-web");
}

#[tokio::test]
async fn unknown_routes_and_methods_are_pagerduty_404s() {
    let (_, app) = app();
    for (method, uri) in [
        ("GET", "/nope"),
        ("DELETE", "/incidents"),
        ("GET", "/teams/PNOPE00"),
        ("GET", "/escalation_policies/PNOPE00"),
        ("GET", "/schedules/PNOPE00"),
    ] {
        let (status, body) = send(&app, method, uri, Value::Null, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
        assert_eq!(body, not_found(), "{method} {uri}");
    }
    let (status, body) = send(&app, "POST", "/teams", json!("not an object"), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["message"], "Invalid Input Provided");
}
