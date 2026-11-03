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
use mock_notion::Notion;
use serde_json::{Value, json};
use tower::ServiceExt;

const LAUNCH_DS: &str = "0b7747c0-5ed0-4c77-a8b1-867a05a3c300";
const LAUNCH_DB: &str = "f322d910-ddf9-4ede-8f1e-096fdde05a9d";
const ESC_DS: &str = "f937b556-2452-4808-a340-f4c80ce63d68";
const ESC_DB: &str = "31142074-4482-4c70-9ca0-406aa0edd7ec";
const WEB412: &str = "afa8e804-d2ac-4ee2-80e2-e18ed4c05865";
const PV2GA: &str = "71cdb3b7-f42a-472b-9b30-488942c27a9e";
const RUNBOOK: &str = "7c41530e-e886-4046-9cb4-2f49e6957f99";
const RB_P1: &str = "d18edb31-c7d9-4fa2-9759-783c0b2d0b89";
const ALEX_KIM: &str = "6a5aa3fe-744c-41b8-ab6d-2e0cc2ef706a";
const LENA: &str = "e54f8584-6db0-4f68-92b8-7f3948f40914";

fn world(slug: &str) -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../scenarios/collab/notion")
        .join(slug);
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    version: Option<&str>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", "Bearer ntn_toolbench")
        .header("content-type", "application/json");
    if let Some(v) = version {
        req = req.header("notion-version", v);
    }
    let req = req
        .body(Body::from(body.map(|b| b.to_string()).unwrap_or_default()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn api(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let v = if uri.ends_with("/markdown") {
        "2026-03-11"
    } else {
        "2025-09-03"
    };
    send(app, method, uri, body, Some(v)).await
}

async fn ok(app: &Router, method: &str, uri: &str, body: Option<Value>) -> Value {
    let (status, v) = api(app, method, uri, body).await;
    assert_eq!(status, StatusCode::OK, "{method} {uri}: {v}");
    v
}

fn state(w: &SharedWorld) -> Value {
    w.lock().unwrap().snapshot()["services"]["notion"].clone()
}

#[tokio::test]
async fn auth_version_and_error_shapes() {
    let app = service_app(world("launch-date-owner"), &Notion);
    let (status, v) = send(&app, "GET", "/v1/users", None, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(v["code"], "missing_version");
    let (status, v) = send(&app, "GET", "/v1/users", None, Some("2099-01-01")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(v["code"], "validation_error");
    let req = Request::builder()
        .uri("/v1/users")
        .header("notion-version", "2025-09-03")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let (status, v) = api(
        &app,
        "GET",
        "/v1/pages/00000000-0000-4000-8000-000000000000",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(v["object"], "error");
    assert_eq!(v["code"], "object_not_found");
    assert_eq!(
        v["message"],
        "Could not find page with ID: 00000000-0000-4000-8000-000000000000. Make sure the relevant pages and databases are shared with your integration."
    );
    let (status, v) = api(&app, "GET", "/v1/pages/nope", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        v["message"]
            .as_str()
            .unwrap()
            .contains("path.page_id should be a valid uuid")
    );
    let (status, v) = api(&app, "GET", "/v1/nothing", None).await;
    assert_eq!(
        (status, v["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_request_url"))
    );
}

#[tokio::test]
async fn users_and_search() {
    let app = service_app(world("launch-date-owner"), &Notion);
    let me = ok(&app, "GET", "/v1/users/me", None).await;
    assert_eq!(me["type"], "bot");
    let users = ok(&app, "GET", "/v1/users?page_size=2", None).await;
    assert_eq!(users["object"], "list");
    assert_eq!(users["results"].as_array().unwrap().len(), 2);
    assert_eq!(users["has_more"], true);
    let next = users["next_cursor"].as_str().unwrap().to_owned();
    let users2 = ok(
        &app,
        "GET",
        &format!("/v1/users?page_size=2&start_cursor={next}"),
        None,
    )
    .await;
    assert_ne!(users2["results"][0]["id"], users["results"][0]["id"]);
    let u = ok(
        &app,
        "GET",
        &format!("/v1/users/{}", LENA.replace('-', "")),
        None,
    )
    .await;
    assert_eq!(u["person"]["email"], "lena.fischer@octo-corp.example");

    let r = ok(
        &app,
        "POST",
        "/v1/search",
        Some(json!({"query": "payments v2 ga"})),
    )
    .await;
    let ids: Vec<&str> = r["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [PV2GA], "trashed duplicate excluded");
    assert_eq!(r["type"], "page_or_data_source");
    let r = ok(
        &app,
        "POST",
        "/v1/search",
        Some(
            json!({"query": "launches", "filter": {"property": "object", "value": "data_source"}}),
        ),
    )
    .await;
    assert_eq!(r["results"][0]["object"], "data_source");
    assert_eq!(r["results"][0]["database_parent"]["type"], "page_id");
    let (status, _) = api(
        &app,
        "POST",
        "/v1/search",
        Some(json!({"filter": {"property": "object", "value": "database"}})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn data_sources_databases_and_queries() {
    let app = service_app(world("launch-date-owner"), &Notion);
    let db = ok(&app, "GET", &format!("/v1/databases/{LAUNCH_DB}"), None).await;
    assert_eq!(
        db["data_sources"],
        json!([{"id": LAUNCH_DS, "name": "Product launches"}])
    );
    let ds = ok(&app, "GET", &format!("/v1/data_sources/{LAUNCH_DS}"), None).await;
    assert_eq!(ds["properties"]["Status"]["type"], "status");
    let q = ok(&app, "POST", &format!("/v1/data_sources/{LAUNCH_DS}/query"), Some(json!({
        "filter": {"and": [{"property": "Team", "select": {"equals": "Payments"}}, {"property": "Status", "status": {"does_not_equal": "Done"}}]}
    }))).await;
    let names: Vec<String> = q["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            p["properties"]["Launch"]["title"][0]["plain_text"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(names, ["Payments v2 GA"]);
    assert_eq!(
        q["results"][0]["properties"]["Owner"]["people"][0]["name"],
        "Lena Fischer"
    );
    assert_eq!(
        q["results"][0]["properties"]["Created"]["created_time"],
        "2026-07-05T15:00:00.000Z"
    );
    let q = ok(
        &app,
        "POST",
        &format!("/v1/data_sources/{LAUNCH_DS}/query"),
        Some(json!({
            "filter": {"property": "Launch date", "date": {"on_or_after": "2026-11-01"}},
            "sorts": [{"property": "Launch date", "direction": "ascending"}]
        })),
    )
    .await;
    let names: Vec<&str> = q["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            p["properties"]["Launch"]["title"][0]["plain_text"]
                .as_str()
                .unwrap()
        })
        .collect();
    assert_eq!(names, ["Web 4.12", "Postgres 16 cutover", "Payments v2 GA"]);
    let q = ok(
        &app,
        "POST",
        &format!("/v1/data_sources/{LAUNCH_DS}/query?filter_properties=title"),
        Some(
            json!({"page_size": 2, "filter": {"property": "Launch", "title": {"contains": "web"}}}),
        ),
    )
    .await;
    assert_eq!(q["results"].as_array().unwrap().len(), 2);
    assert_eq!(q["results"][0]["properties"].as_object().unwrap().len(), 1);
    let q = ok(&app, "POST", &format!("/v1/data_sources/{LAUNCH_DS}/query"), Some(json!({"in_trash": true, "filter": {"property": "Launch", "title": {"equals": "Payments v2 GA"}}}))).await;
    assert_eq!(q["results"].as_array().unwrap().len(), 2);
    let (status, v) = api(
        &app,
        "POST",
        &format!("/v1/data_sources/{LAUNCH_DS}/query"),
        Some(json!({"filter": {"property": "Status", "select": {"equals": "Done"}}})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{v}");
    let t = ok(
        &app,
        "GET",
        &format!("/v1/data_sources/{LAUNCH_DS}/templates"),
        None,
    )
    .await;
    assert_eq!(t["templates"][0]["name"], "Launch template");

    let created = ok(&app, "POST", "/v1/data_sources", Some(json!({
        "parent": {"type": "database_id", "database_id": LAUNCH_DB},
        "title": [{"type": "text", "text": {"content": "Launch risks"}}],
        "properties": {"Risk": {"title": {}}, "Level": {"select": {"options": [{"name": "High"}, {"name": "Low"}]}}}
    }))).await;
    let id = created["id"].as_str().unwrap().to_owned();
    let upd = ok(
        &app,
        "PATCH",
        &format!("/v1/data_sources/{id}"),
        Some(json!({"properties": {"Level": {"name": "Impact"}, "Owner": {"people": {}}}})),
    )
    .await;
    assert!(upd["properties"]["Impact"]["select"]["options"].is_array());
    assert_eq!(upd["properties"]["Owner"]["type"], "people");
}

#[tokio::test]
async fn pages_create_update_move_and_properties() {
    let w = world("log-customer-escalation");
    let app = service_app(w.clone(), &Notion);
    let created = ok(&app, "POST", "/v1/pages", Some(json!({
        "parent": {"type": "database_id", "database_id": ESC_DB},
        "properties": {
            "Summary": {"title": [{"text": {"content": "Checkout button unresponsive on Safari"}}]},
            "Customer": {"rich_text": [{"text": {"content": "Acme Retail"}}]},
            "Severity": {"select": {"name": "Sev 2"}},
            "State": {"status": {"name": "Open"}},
            "Owner": {"people": [{"id": ALEX_KIM}]},
            "Reported": {"date": {"start": "2026-10-29"}}
        },
        "children": ["{\"object\":\"block\",\"type\":\"paragraph\",\"paragraph\":{\"rich_text\":[{\"type\":\"text\",\"text\":{\"content\":\"Raised by support.\"}}]}}"]
    }))).await;
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(
        id, "2a312ccd-ad51-42dd-8f17-7f194d6b5db7",
        "first page ID for this scenario seed"
    );
    assert_eq!(
        created["parent"],
        json!({"type": "data_source_id", "data_source_id": ESC_DS, "database_id": ESC_DB})
    );
    assert_eq!(created["properties"]["State"]["status"]["name"], "Open");
    let s = state(&w);
    assert_eq!(
        s["pages"][&id]["properties"]["Owner"]["people"][0]["id"],
        ALEX_KIM
    );
    let kids = ok(&app, "GET", &format!("/v1/blocks/{id}/children"), None).await;
    assert_eq!(
        kids["results"][0]["paragraph"]["rich_text"][0]["plain_text"],
        "Raised by support."
    );

    let (status, v) = api(&app, "POST", "/v1/pages", Some(json!({"parent": {"database_id": ESC_DB}, "properties": {"State": {"status": {"name": "Escalated"}}}}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        v["message"]
            .as_str()
            .unwrap()
            .contains("Invalid status option")
    );
    let (status, v) = api(&app, "POST", "/v1/pages", Some(json!({"parent": {"database_id": ESC_DB}, "properties": {"Priority": {"select": {"name": "P1"}}}}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(v["message"], "Priority is not a property that exists.");
    let (status, _) = api(
        &app,
        "POST",
        "/v1/pages",
        Some(json!({"parent": {"database_id": ESC_DB}, "properties": {"Severity": "Sev 1"}})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let p = ok(&app, "PATCH", &format!("/v1/pages/{id}"), Some(json!({"properties": {"Severity": {"select": {"name": "Sev 4"}}}, "icon": "{\"emoji\":\"🔥\"}"}))).await;
    assert_eq!(p["properties"]["Severity"]["select"]["name"], "Sev 4");
    assert_eq!(p["icon"], json!({"type": "emoji", "emoji": "🔥"}));
    let ds = ok(&app, "GET", &format!("/v1/data_sources/{ESC_DS}"), None).await;
    assert_eq!(
        ds["properties"]["Severity"]["select"]["options"]
            .as_array()
            .unwrap()
            .len(),
        4,
        "new select option created"
    );

    let prop = ok(
        &app,
        "GET",
        &format!("/v1/pages/{id}/properties/title"),
        None,
    )
    .await;
    assert_eq!(prop["object"], "list");
    assert_eq!(
        prop["results"][0]["title"]["plain_text"],
        "Checkout button unresponsive on Safari"
    );
    let prop = ok(
        &app,
        "GET",
        &format!("/v1/pages/{id}/properties/Sv%253Fz"),
        None,
    )
    .await;
    assert_eq!(prop["select"]["name"], "Sev 4");

    let moved = ok(
        &app,
        "POST",
        &format!("/v1/pages/{id}/move"),
        Some(json!({"parent": {"type": "page_id", "page_id": RUNBOOK}})),
    )
    .await;
    assert_eq!(
        moved["parent"],
        json!({"type": "page_id", "page_id": RUNBOOK})
    );
    assert_eq!(
        moved["properties"]["title"]["title"][0]["plain_text"],
        "Checkout button unresponsive on Safari"
    );
    let trashed = ok(
        &app,
        "PATCH",
        &format!("/v1/pages/{id}"),
        Some(json!({"in_trash": true})),
    )
    .await;
    assert_eq!(trashed["in_trash"], true);
}

#[tokio::test]
async fn blocks_and_markdown() {
    let w = world("update-current-runbook");
    let app = service_app(w.clone(), &Notion);
    let kids = ok(
        &app,
        "GET",
        &format!("/v1/blocks/{RUNBOOK}/children?page_size=3"),
        None,
    )
    .await;
    assert_eq!(kids["results"].as_array().unwrap().len(), 3);
    assert_eq!(kids["type"], "block");
    assert_eq!(kids["results"][1]["id"], RB_P1);
    assert!(kids["results"][0].get("_order").is_none());
    let appended = ok(&app, "PATCH", &format!("/v1/blocks/{RUNBOOK}/children"), Some(json!({
        "children": [{"object": "block", "type": "bulleted_list_item", "bulleted_list_item": {"rich_text": [{"type": "text", "text": {"content": "Backup: Sam Ortiz"}}]}}],
        "after": RB_P1
    }))).await;
    let new_id = appended["results"][0]["id"].as_str().unwrap().to_owned();
    let kids = ok(&app, "GET", &format!("/v1/blocks/{RUNBOOK}/children"), None).await;
    assert_eq!(
        kids["results"][2]["id"], new_id,
        "inserted after the paragraph"
    );

    let md = ok(&app, "GET", &format!("/v1/pages/{RUNBOOK}/markdown"), None).await;
    assert_eq!(md["object"], "page_markdown");
    assert!(md["markdown"].as_str().unwrap().starts_with("## Escalation\nEscalation contact: Marcus Webb, +1 555-0100 (Head of Infrastructure)\n- Backup: Sam Ortiz"));
    let (status, _) = send(
        &app,
        "GET",
        &format!("/v1/pages/{RUNBOOK}/markdown"),
        None,
        Some("2025-09-03"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let edited = ok(&app, "PATCH", &format!("/v1/pages/{RUNBOOK}/markdown"), Some(json!({
        "type": "update_content",
        "update_content": {"content_updates": [{"old_str": "Marcus Webb, +1 555-0100", "new_str": "Sam Ortiz, +1 555-0142"}]}
    }))).await;
    assert!(
        edited["markdown"]
            .as_str()
            .unwrap()
            .contains("Escalation contact: Sam Ortiz, +1 555-0142")
    );
    assert_eq!(
        state(&w)["blocks"][RB_P1]["paragraph"]["rich_text"][0]["plain_text"],
        "Escalation contact: Sam Ortiz, +1 555-0142 (Head of Infrastructure)",
        "edited in place"
    );
    let (status, v) = api(&app, "PATCH", &format!("/v1/pages/{RUNBOOK}/markdown"), Some(json!({"type": "update_content", "update_content": {"content_updates": [{"old_str": "not on the page", "new_str": "x"}]}}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{v}");

    let md = ok(&app, "GET", &format!("/v1/pages/{RUNBOOK}/markdown"), None).await;
    let replaced = md["markdown"]
        .as_str()
        .unwrap()
        .replace("10 minutes", "5 minutes");
    ok(
        &app,
        "PATCH",
        &format!("/v1/pages/{RUNBOOK}/markdown"),
        Some(json!({"type": "replace_content", "replace_content": {"new_str": replaced}})),
    )
    .await;
    let s = state(&w);
    assert!(s["blocks"].as_object().unwrap().values().any(|b| {
        b["paragraph"]["rich_text"][0]["plain_text"]
            .as_str()
            .is_some_and(|t| t.contains("5 minutes"))
    }));
    assert!(s["blocks"][RB_P1].is_object(), "same shape keeps block IDs");

    let upd = ok(&app, "PATCH", &format!("/v1/blocks/{RB_P1}"), Some(json!({"type": {"paragraph": {"rich_text": [{"type": "text", "text": {"content": "Escalation contact: Priya"}}]}}}))).await;
    assert_eq!(
        upd["paragraph"]["rich_text"][0]["plain_text"],
        "Escalation contact: Priya"
    );
    let (status, _) = api(
        &app,
        "PATCH",
        &format!("/v1/blocks/{RB_P1}"),
        Some(json!({"to_do": {"checked": true}})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let del = ok(&app, "DELETE", &format!("/v1/blocks/{new_id}"), None).await;
    assert_eq!(del["in_trash"], true);
    let (status, _) = api(
        &app,
        "PATCH",
        &format!("/v1/blocks/{new_id}"),
        Some(json!({"bulleted_list_item": {"rich_text": []}})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let b = ok(&app, "GET", &format!("/v1/blocks/{RUNBOOK}"), None).await;
    assert_eq!(b["type"], "child_page");
    let (status, v) = api(
        &app,
        "PATCH",
        &format!("/v1/blocks/{RUNBOOK}/children"),
        Some(json!({"children": [{"type": "child_page", "child_page": {"title": "x"}}]})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{v}");
}

#[tokio::test]
async fn comments() {
    let app = service_app(world("update-current-runbook"), &Notion);
    let list = ok(
        &app,
        "GET",
        &format!("/v1/comments?block_id={RUNBOOK}"),
        None,
    )
    .await;
    assert_eq!(list["type"], "comment");
    assert_eq!(list["results"].as_array().unwrap().len(), 1);
    let c = ok(&app, "POST", "/v1/comments", Some(json!({"parent": {"page_id": RUNBOOK}, "rich_text": [{"text": {"content": "Updated the contact."}}]}))).await;
    assert_eq!(c["object"], "comment");
    assert_eq!(c["rich_text"][0]["plain_text"], "Updated the contact.");
    let (status, v) = api(&app, "GET", "/v1/comments", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{v}");
    let (status, v) = send(
        &app,
        "POST",
        "/v1/comments",
        Some(json!({"parent": {"page_id": RUNBOOK}, "rich_text": [{"text": {"content": "x"}}]})),
        None,
    )
    .await;
    assert_eq!(
        (status, v["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("missing_version"))
    );
}

#[tokio::test]
async fn scenario_writes() {
    let w = world("delay-web-launch");
    let app = service_app(w.clone(), &Notion);
    ok(&app, "PATCH", &format!("/v1/pages/{WEB412}"), Some(json!({"properties": {"Status": {"status": {"name": "Delayed"}}, "Launch date": {"date": {"start": "2026-11-10"}}}}))).await;
    let s = state(&w);
    assert_eq!(
        s["pages"][WEB412]["properties"]["Status"]["status"]["name"],
        "Delayed"
    );
    assert_eq!(
        s["pages"][WEB412]["properties"]["Launch date"]["date"]["start"],
        "2026-11-10"
    );
}
