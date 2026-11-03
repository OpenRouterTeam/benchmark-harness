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
use mock_jira::Jira;
use serde_json::{Value, json};
use tower::ServiceExt;

fn world(slug: &str) -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../scenarios/collab/jira")
        .join(slug);
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    auth: bool,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("host", "world:8128");
    if auth {
        req = req.header("authorization", "Bearer jira-toolbench");
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

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    send(app, method, uri, body, true).await
}

fn state(w: &SharedWorld) -> Value {
    w.lock().unwrap().snapshot()["services"]["jira"].clone()
}

#[tokio::test]
async fn auth_myself_and_users() {
    let w = world("blocked-sprint-item");
    let app = service_app(w, &Jira);
    let (st, v) = send(&app, "GET", "/rest/api/2/myself", None, false).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert!(
        v["errorMessages"][0]
            .as_str()
            .unwrap()
            .contains("not authenticated")
    );
    let (st, me) = call(&app, "GET", "/rest/api/2/myself", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(me["name"], "jordan.lee");
    let (_, found) = call(
        &app,
        "GET",
        "/rest/api/2/user/search?username=Alex%20Kim",
        None,
    )
    .await;
    let names: Vec<&str> = found
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["alex.kim", "alex.kimball"]);
    let (st, _) = call(&app, "GET", "/rest/api/2/user?username=nobody", None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn search_jql_and_links() {
    let w = world("blocked-sprint-item");
    let app = service_app(w, &Jira);
    let (st, v) = call(&app, "GET", "/rest/api/2/search?jql=project%20%3D%20PAY%20AND%20sprint%20in%20openSprints()%20AND%20issueLinkType%20%3D%20%22is%20blocked%20by%22&fields=summary,issuelinks", None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["total"], 1);
    let issue = &v["issues"][0];
    assert_eq!(issue["key"], "PAY-119");
    assert_eq!(
        issue["fields"]["issuelinks"][0]["inwardIssue"]["key"],
        "PAY-117"
    );
    assert!(issue["fields"].get("status").is_none());
    let (_, v) = call(&app, "GET", "/rest/api/2/search?jql=assignee%20%3D%20currentUser()%20ORDER%20BY%20priority%20DESC&maxResults=1", None).await;
    assert_eq!(v["issues"][0]["key"], "PAY-123");
    let (_, v) = call(
        &app,
        "GET",
        "/rest/api/2/search?jql=project%20in%20(PAY,WEB)%20AND%20labels%20%3D%20safari",
        None,
    )
    .await;
    assert_eq!(v["issues"][0]["key"], "WEB-51");
    let (_, v) = call(
        &app,
        "GET",
        "/rest/api/2/search?jql=project%3DPAY&startAt=2&maxResults=3",
        None,
    )
    .await;
    assert_eq!(
        (v["total"].as_i64(), v["issues"].as_array().unwrap().len()),
        (Some(12), 3)
    );
    let (st, v) = call(
        &app,
        "GET",
        "/rest/api/2/search?jql=project%20%3D%20NOPE",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(
        v["errorMessages"][0],
        "The value 'NOPE' does not exist for the field 'project'."
    );
    let (st, v) = call(&app, "GET", "/rest/api/2/search?jql=foo%20%3D%20bar", None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(
        v["errorMessages"][0]
            .as_str()
            .unwrap()
            .contains("Field 'foo' does not exist")
    );
    let (st, v) = call(&app, "GET", "/rest/api/2/issue/PAY-999", None).await;
    assert_eq!(
        (st, v["errorMessages"][0].as_str()),
        (StatusCode::NOT_FOUND, Some("Issue Does Not Exist"))
    );
}

#[tokio::test]
async fn transition_comment_and_history() {
    let w = world("ship-refund-retries");
    let app = service_app(w.clone(), &Jira);
    let (_, t) = call(
        &app,
        "GET",
        "/rest/api/2/issue/PAY-121/transitions?expand=transitions.fields",
        None,
    )
    .await;
    let ids: Vec<&str> = t["transitions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["31", "61"]);
    let (st, v) = call(
        &app,
        "POST",
        "/rest/api/2/issue/PAY-121/transitions",
        Some(json!({"transition": {"id": "11"}})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    let (st, _) = call(
        &app,
        "POST",
        "/rest/api/2/issue/PAY-121/transitions",
        Some(json!({"transition": {"id": "31"}, "fields": {"resolution": {"name": "Done"}}})),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, c) = call(
        &app,
        "POST",
        "/rest/api/2/issue/PAY-121/comment",
        Some(json!({"body": "Shipped in 2026.11."})),
    )
    .await;
    assert_eq!((st, c["id"].as_str()), (StatusCode::CREATED, Some("10044")));
    let (st, _) = call(
        &app,
        "POST",
        "/rest/api/2/issue/PAY-121/comment",
        Some(json!({"body": " "})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let s = state(&w);
    assert_eq!(s["issues"]["PAY-121"]["fields"]["status"], "Done");
    assert_eq!(s["issues"]["PAY-121"]["fields"]["resolution"], "Done");
    assert_eq!(s["comments"]["10044"]["issue"], "PAY-121");
    let (_, i) = call(
        &app,
        "GET",
        "/rest/api/2/issue/PAY-121?expand=changelog&fields=status,resolution",
        None,
    )
    .await;
    assert_eq!(i["fields"]["status"]["statusCategory"]["key"], "done");
    assert_eq!(
        i["changelog"]["histories"][0]["items"][0]["toString"],
        "Done"
    );
    let (st, _) = call(
        &app,
        "POST",
        "/rest/api/2/issue/PAY-121/worklog",
        Some(json!({"timeSpent": "1h 30m", "comment": "review"})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let (_, wl) = call(&app, "GET", "/rest/api/2/issue/PAY-121/worklog", None).await;
    assert_eq!(wl["total"], 3);
    assert_eq!(wl["worklogs"][2]["timeSpent"], "1h 30m");
}

#[tokio::test]
async fn create_edit_assign_and_meta() {
    let w = world("file-apple-pay-bug");
    let app = service_app(w.clone(), &Jira);
    let (st, v) = call(&app, "POST", "/rest/api/2/issue", Some(json!({"fields": {"project": {"key": "WEB"}, "summary": "Apple Pay button missing", "issuetype": {"name": "Bug"}, "priority": {"name": "High"}, "labels": ["ios"], "components": [{"name": "Checkout"}], "assignee": {"name": "alex.kim"}}}))).await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    assert_eq!(v["key"], "WEB-56");
    let (st, v) = call(&app, "POST", "/rest/api/2/issue", Some(json!({"fields": {"project": {"key": "WEB"}, "summary": "x", "issuetype": {"name": "Nope"}, "priority": {"name": "Urgent"}}}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(v["errors"]["issuetype"], "valid issue type is required");
    assert_eq!(
        v["errors"]["priority"],
        "Priority name 'Urgent' is not valid"
    );
    let (st, _) = call(&app, "PUT", "/rest/api/2/issue/WEB-56", Some(json!({"fields": {"summary": "Apple Pay button missing on iOS 18"}, "update": {"labels": [{"add": "apple-pay"}]}}))).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = call(
        &app,
        "PUT",
        "/rest/api/2/issue/WEB-56/assignee",
        Some(json!({"name": "alex.kimball"})),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let s = state(&w);
    let f = &s["issues"]["WEB-56"]["fields"];
    assert_eq!(f["labels"], json!(["ios", "apple-pay"]));
    assert_eq!(f["assignee"], "alex.kimball");
    assert_eq!(f["components"], json!(["Checkout"]));
    let (_, meta) = call(
        &app,
        "GET",
        "/rest/api/2/issue/createmeta/WEB/issuetypes",
        None,
    )
    .await;
    assert_eq!(meta["total"], 5);
    let (_, fields) = call(
        &app,
        "GET",
        "/rest/api/2/issue/createmeta/WEB/issuetypes/10004",
        None,
    )
    .await;
    assert!(
        fields["values"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["fieldId"] == "customfield_10100")
    );
    let (st, _) = call(&app, "GET", "/rest/api/2/project/NOPE", None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (_, p) = call(&app, "GET", "/rest/api/2/project/PAY", None).await;
    assert_eq!(p["lead"]["name"], "priya.natarajan");
    let (st, _) = call(&app, "POST", "/rest/api/2/issueLink", Some(json!({"type": {"name": "Relates"}, "inwardIssue": {"key": "WEB-56"}, "outwardIssue": {"key": "WEB-53"}}))).await;
    assert_eq!(st, StatusCode::CREATED);
    let (st, _) = call(
        &app,
        "POST",
        "/rest/api/2/version",
        Some(json!({"project": "WEB", "name": "4.2"})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = call(&app, "DELETE", "/rest/api/2/issue/WEB-56", None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(state(&w)["issues"].get("WEB-56").is_none());
}

#[tokio::test]
async fn agile_and_service_desk() {
    let w = world("blocked-sprint-item");
    let app = service_app(w.clone(), &Jira);
    let (_, b) = call(&app, "GET", "/rest/agile/1.0/board", None).await;
    assert_eq!(b["total"], 2);
    let (_, sp) = call(
        &app,
        "GET",
        "/rest/agile/1.0/board/1/sprint?state=active",
        None,
    )
    .await;
    assert_eq!(sp["values"][0]["name"], "PAY Sprint 23");
    let (st, _) = call(&app, "GET", "/rest/agile/1.0/board/2/sprint", None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, s) = call(
        &app,
        "POST",
        "/rest/agile/1.0/sprint",
        Some(json!({"name": "PAY Sprint 25", "originBoardId": 1})),
    )
    .await;
    assert_eq!((st, s["id"].as_i64()), (StatusCode::CREATED, Some(9)));
    let (st, _) = call(
        &app,
        "POST",
        "/rest/agile/1.0/sprint/9/issue",
        Some(json!({"issues": ["PAY-116"]})),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert_eq!(
        state(&w)["issues"]["PAY-116"]["fields"]["customfield_10104"],
        json!([9])
    );
    let (st, _) = call(
        &app,
        "POST",
        "/rest/agile/1.0/sprint/9",
        Some(json!({"state": "closed"})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (_, si) = call(&app, "GET", "/rest/agile/1.0/sprint/7/issue", None).await;
    assert_eq!(si["total"], 5);
    let (_, q) = call(
        &app,
        "GET",
        "/rest/servicedeskapi/servicedesk/1/queue?includeCount=true",
        None,
    )
    .await;
    assert_eq!(q["values"][0]["issueCount"], 1);
    let (st, r) = call(&app, "POST", "/rest/servicedeskapi/request", Some(json!({"serviceDeskId": "1", "requestTypeId": "10", "requestFieldValues": {"summary": "VPN access"}}))).await;
    assert_eq!(
        (st, r["issueKey"].as_str()),
        (StatusCode::CREATED, Some("OPS-32"))
    );
}

#[tokio::test]
async fn confluence_content() {
    let w = world("assign-go-no-go-review");
    let app = service_app(w.clone(), &Jira);
    let (_, me) = call(&app, "GET", "/wiki/rest/api/user/current", None).await;
    assert_eq!(me["username"], "jordan.lee");
    let (_, r) = call(
        &app,
        "GET",
        "/wiki/rest/api/search?cql=siteSearch%20~%20%22Payments%20v2%20launch%20plan%22",
        None,
    )
    .await;
    assert_eq!(r["totalSize"], 2);
    let (_, r) = call(
        &app,
        "GET",
        "/wiki/rest/api/search?cql=space%20%3D%20ENG%20AND%20label%20%3D%20runbook",
        None,
    )
    .await;
    assert_eq!(r["results"][0]["content"]["id"], "100003");
    let (_, p) = call(
        &app,
        "GET",
        "/wiki/rest/api/content/100002?expand=body.storage,version,ancestors",
        None,
    )
    .await;
    assert!(
        p["body"]["storage"]["value"]
            .as_str()
            .unwrap()
            .contains("Lena Fischer")
    );
    assert_eq!(p["ancestors"][0]["id"], "100001");
    let (st, _) = call(&app, "PUT", "/wiki/rest/api/content/100002", Some(json!({"type": "page", "title": "Payments v2 launch plan", "version": {"number": 9}, "body": {"storage": {"value": "<p>x</p>", "representation": "storage"}}}))).await;
    assert_eq!(st, StatusCode::CONFLICT);
    let (st, _) = call(&app, "PUT", "/wiki/rest/api/content/100002", Some(json!({"type": "page", "title": "Payments v2 launch plan", "version": {"number": 5}, "body": {"storage": {"value": "<p>x</p>", "representation": "storage"}}}))).await;
    assert_eq!(st, StatusCode::OK);
    let (_, old) = call(
        &app,
        "GET",
        "/wiki/rest/api/content/100002?version=4&expand=body.storage",
        None,
    )
    .await;
    assert!(
        old["body"]["storage"]["value"]
            .as_str()
            .unwrap()
            .contains("Lena")
    );
    let (st, c) = call(&app, "POST", "/wiki/rest/api/content", Some(json!({"type": "page", "title": "GA retro", "space": {"key": "ENG"}, "body": {"storage": {"value": "<p>retro</p>", "representation": "storage"}}}))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(c["ancestors"][0]["id"], "100001");
    let (st, _) = call(
        &app,
        "POST",
        "/wiki/rest/api/content",
        Some(json!({"type": "page", "title": "GA retro", "space": {"key": "ENG"}})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (_, kids) = call(
        &app,
        "GET",
        "/wiki/rest/api/content/100001/child/page",
        None,
    )
    .await;
    assert_eq!(kids["size"], 3);
    let (st, _) = call(
        &app,
        "POST",
        "/wiki/rest/api/content/100002/label",
        Some(json!([{"prefix": "global", "name": "ga"}])),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        state(&w)["pages"]["100002"]["labels"],
        json!(["payments", "launch", "ga"])
    );
    let (st, v) = call(&app, "GET", "/wiki/rest/api/content/999", None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(v["statusCode"], 404);
}
