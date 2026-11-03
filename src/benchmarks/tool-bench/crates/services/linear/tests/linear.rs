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
use mock_linear::Linear;
use serde_json::{Value, json};
use tower::ServiceExt;

const PAY: &str = "5854633b-d1e9-52b6-b3c7-d309bfb881cb";
const WEB: &str = "6da75b4c-0e7c-5ac7-a415-f09a129582b1";
const PAYV2: &str = "e3c09e4d-1537-5f94-8bf9-aa46108fc659";
const CYCLE42: &str = "627fb4e1-47ac-567a-99cd-3e236709e3b0";
const PAY_IN_PROGRESS: &str = "41688e9c-05a3-5fce-be65-9c7049e13037";
const ALEX_KIM: &str = "0bfae6ae-01d0-58d1-a611-7d1973102c45";
const BUG: &str = "75dbca3d-6368-502e-8e56-0c82837ea691";

/// Every tool in the snapshot is dispatched (never "Unknown tool"), given its required args.
#[tokio::test]
async fn every_tool_answers() {
    let tools: serde_json::Map<String, Value> =
        serde_json::from_str(include_str!("../assets/tools.json")).unwrap();
    assert_eq!(tools.len(), 198);
    let app = service_app(world(), &Linear);
    for (name, spec) in &tools {
        let mut args = serde_json::Map::new();
        for req in spec["required"].as_array().unwrap() {
            let k = req.as_str().unwrap();
            let p = &spec["props"][k];
            let v = match p["type"].as_str().unwrap() {
                _ if k.starts_with("confirm") => json!(true),
                _ if p.get("enum").is_some() => p["enum"][0].clone(),
                "string" => json!("WEB-51"),
                "number" | "integer" => json!(1),
                "boolean" => json!(true),
                "array" => match p.get("items_enum") {
                    Some(e) => json!([e[0]]),
                    None => json!(["x"]),
                },
                "object" => json!({"a": 1}),
                _ => json!("x"),
            };
            args.insert(k.to_owned(), v);
        }
        let r = call(&app, name, Value::Object(args)).await;
        let text = r["content"][0]["text"].as_str().unwrap();
        assert!(!text.contains("Unknown tool"), "{name}: {text}");
        assert!(r["isError"].is_boolean(), "{name}");
    }
}

fn world() -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../scenarios/collab/linear/escalate-right-safari-issue");
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

async fn call(app: &Router, name: &str, args: Value) -> Value {
    let req = Request::builder()
        .method("POST")
        .uri("/_mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"name": name, "arguments": args}).to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap()
}

/// Successful call: returns the parsed text and checks structuredContent mirrors it.
async fn ok(app: &Router, name: &str, args: Value) -> Value {
    let r = call(app, name, args).await;
    assert_eq!(r["isError"], false, "{name}: {r}");
    let text = r["content"][0]["text"].as_str().unwrap();
    let v: Value = serde_json::from_str(text).unwrap();
    assert_eq!(
        text,
        serde_json::to_string_pretty(&v).unwrap(),
        "pretty JSON text"
    );
    match &v {
        Value::Array(_) => assert_eq!(r["structuredContent"]["items"], v, "{name}"),
        Value::Object(_) => assert_eq!(r["structuredContent"], v, "{name}"),
        _ => {}
    }
    v
}

async fn err(app: &Router, name: &str, args: Value) -> String {
    let r = call(app, name, args).await;
    assert_eq!(r["isError"], true, "{name} should fail: {r}");
    r["content"][0]["text"].as_str().unwrap().to_owned()
}

fn state(w: &SharedWorld) -> Value {
    w.lock().unwrap().snapshot()["services"]["linear"].clone()
}

#[tokio::test]
async fn argument_validation_matches_the_server() {
    let app = service_app(world(), &Linear);
    assert_eq!(
        err(
            &app,
            "linear_getIssueById",
            json!({"id": "PAY-1", "bogus": 1})
        )
        .await,
        "Error: Unknown argument(s) for linear_getIssueById: bogus"
    );
    assert_eq!(
        err(&app, "linear_getIssueById", json!({})).await,
        "Error: Invalid arguments for getIssueById"
    );
    assert_eq!(
        err(&app, "linear_getInitiativeById", json!({})).await,
        "Error: Invalid input for getInitiativeById"
    );
    assert_eq!(
        err(
            &app,
            "linear_setIssuePriority",
            json!({"issueId": "WEB-51", "priority": 9})
        )
        .await,
        "Error: Invalid arguments for setIssuePriority"
    );
    assert_eq!(
        err(
            &app,
            "linear_getTeamMemberships",
            json!({"teamId": PAY, "limit": 0})
        )
        .await,
        "Error: Invalid arguments for getTeamMemberships"
    );
    assert_eq!(
        err(&app, "linear_nope", json!({})).await,
        "Error: Unknown tool: linear_nope"
    );
}

#[tokio::test]
async fn viewer_org_users_teams_labels() {
    let app = service_app(world(), &Linear);
    let v = ok(&app, "linear_getViewer", json!({})).await;
    assert_eq!(
        v,
        json!({"id": "2dad689e-9ace-5467-9b02-0cc4e445cfa4", "name": "Casey Morgan", "email": "casey.morgan@octo-corp.example", "displayName": "casey", "active": true})
    );
    let v = ok(&app, "linear_getOrganization", json!({})).await;
    assert_eq!(v["urlKey"], "octo");
    assert!(v["subscription"].is_null() && v.get("logoUrl").is_none());
    assert_eq!(
        ok(&app, "linear_getUsers", json!({}))
            .await
            .as_array()
            .unwrap()
            .len(),
        8
    );
    let teams = ok(&app, "linear_getTeams", json!({})).await;
    assert_eq!(
        teams[1],
        json!({"id": PAY, "name": "Payments", "key": "PAY", "description": "Checkout, billing and payment partners"})
    );
    let labels = ok(&app, "linear_getLabels", json!({})).await;
    assert_eq!(
        labels[0],
        json!({"id": BUG, "name": "Bug", "color": "#eb5757", "description": "Something is broken", "team": null})
    );
    let states = ok(&app, "linear_getWorkflowStates", json!({"teamId": PAY})).await;
    assert_eq!(
        states[2],
        json!({"id": PAY_IN_PROGRESS, "name": "In Progress", "type": "started", "position": 2, "color": "#f2c94c", "description": ""})
    );
    assert_eq!(
        ok(&app, "linear_getWorkflowStates", json!({"teamId": "nope"})).await,
        json!([])
    );
    let m = ok(
        &app,
        "linear_getTeamMemberships",
        json!({"teamId": PAY, "limit": 2}),
    )
    .await;
    assert_eq!(m.as_array().unwrap().len(), 2);
    assert_eq!(m[0]["team"]["key"], "PAY");
    assert_eq!(
        err(&app, "linear_getTeamLabels", json!({"teamId": "missing"})).await,
        "Error: Entity not found: Team - Could not find referenced Team."
    );
}

#[tokio::test]
async fn issue_reads_have_the_servers_shapes() {
    let app = service_app(world(), &Linear);
    let i = ok(&app, "linear_getIssueById", json!({"id": "web-51"})).await;
    assert!(
        i.get("identifier").is_none(),
        "getIssueById has no identifier"
    );
    assert_eq!(i["title"], "Checkout button unresponsive on Safari");
    assert_eq!(
        i["url"],
        "https://linear.app/octo/issue/WEB-51/checkout-button-unresponsive-on-safari"
    );
    assert_eq!(i["state"]["name"], "Todo");
    assert!(i["assignee"].is_null() && i.get("estimate").is_none());
    assert_eq!(i["labels"].as_array().unwrap().len(), 2);
    assert_eq!(i["comments"][0]["user"]["name"], "Lena Fischer");
    assert_eq!(
        err(&app, "linear_getIssueById", json!({"id": "WEB-999"})).await,
        "Error: Entity not found: Issue - Could not find referenced Issue."
    );

    let found = ok(&app, "linear_searchIssues", json!({"query": "Safari"})).await;
    let titles: Vec<&str> = found
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles.len(), 3, "{titles:?}");
    // `contains` is case-sensitive, as in Linear's IssueFilter.
    assert_eq!(
        ok(&app, "linear_searchIssues", json!({"query": "safari"}))
            .await
            .as_array()
            .unwrap()
            .len(),
        0
    );
    let found = ok(
        &app,
        "linear_searchIssues",
        json!({"teamId": PAY, "states": ["in progress"]}),
    )
    .await;
    assert_eq!(found.as_array().unwrap().len(), 1);
    let found = ok(
        &app,
        "linear_searchIssues",
        json!({"dates": {"created": {"after": "2026-10-26"}}, "limit": 50}),
    )
    .await;
    assert_eq!(found.as_array().unwrap().len(), 2);
    assert!(
        err(
            &app,
            "linear_searchIssues",
            json!({"dates": {"shipped": {"after": "2026-10-26"}}})
        )
        .await
        .contains("dates has unknown field: shipped")
    );

    let p = ok(
        &app,
        "linear_getProjectIssues",
        json!({"projectId": PAYV2, "includeCompleted": false}),
    )
    .await;
    assert_eq!(p[0]["identifier"], "PAY-122");
    assert!(
        p.as_array()
            .unwrap()
            .iter()
            .all(|x| x["identifier"] != "PAY-104")
    );
    assert!(
        p[0]["assignee"].is_null() && p[0]["estimate"].is_null(),
        "raw GraphQL keeps nulls"
    );
    assert_eq!(
        ok(&app, "linear_getIssues", json!({"limit": 3}))
            .await
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn create_update_and_assign_issues() {
    let w = world();
    let app = service_app(w.clone(), &Linear);
    let created = ok(&app, "linear_createIssue", json!({"title": "Saved carts tab reloads", "teamId": WEB, "priority": 2, "labelIds": [BUG], "assigneeId": ALEX_KIM})).await;
    assert_eq!(
        created.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["id", "title", "url"]
    );
    assert_eq!(
        created["url"],
        "https://linear.app/octo/issue/WEB-58/saved-carts-tab-reloads"
    );
    let s = state(&w);
    let issue = &s["issues"]["WEB-58"];
    assert_eq!(issue["identifier"], "WEB-58");
    assert_eq!(
        issue["stateId"], "0964aa8b-46b3-5ca7-89ef-326f7c8ffadb",
        "team default state (Todo)"
    );
    assert_eq!(issue["createdAt"], "2026-10-29T15:00:07.000Z");
    assert_eq!(
        err(
            &app,
            "linear_createIssue",
            json!({"title": "x", "teamId": "nope"})
        )
        .await,
        "Error: Entity not found: Team - Could not find referenced Team."
    );
    assert!(
        err(
            &app,
            "linear_createIssue",
            json!({"title": "x", "teamId": WEB, "priority": 7})
        )
        .await
        .contains("priority must not be greater than 4")
    );

    let u = ok(&app, "linear_updateIssue", json!({"id": "WEB-58", "stateId": "4f856b72-2c9a-52b0-9765-a08e69992cea", "addedLabelIds": ["b528a42a-7d96-511c-ab34-1b8cb0bf0c8d"], "assigneeId": ""})).await;
    assert!(u["description"].is_null() || u.get("description").is_none());
    let s = state(&w);
    assert_eq!(
        s["issues"]["WEB-58"]["labelIds"].as_array().unwrap().len(),
        2
    );
    assert_eq!(
        s["issues"]["WEB-58"]["assigneeId"], ALEX_KIM,
        "empty strings are ignored"
    );
    assert!(s["issues"]["WEB-58"]["startedAt"].is_string());
    assert!(
        err(
            &app,
            "linear_updateIssue",
            json!({"id": "WEB-58", "stateId": PAY_IN_PROGRESS})
        )
        .await
        .contains("must belong to the issue's team")
    );

    let a = ok(
        &app,
        "linear_assignIssue",
        json!({"issueId": "WEB-51", "assigneeId": ALEX_KIM}),
    )
    .await;
    assert_eq!(
        a["issue"]["assignee"],
        json!({"id": ALEX_KIM, "name": "Alex Kim", "displayName": "alexk"})
    );
    let p = ok(
        &app,
        "linear_setIssuePriority",
        json!({"issueId": "WEB-51", "priority": 1}),
    )
    .await;
    assert_eq!(p["issue"]["priority"], 1);
    let sub = ok(
        &app,
        "linear_subscribeToIssue",
        json!({"issueId": "WEB-51"}),
    )
    .await;
    assert_eq!(
        sub["message"],
        "User Casey Morgan (2dad689e-9ace-5467-9b02-0cc4e445cfa4) is subscribed to issue WEB-51."
    );
    let h = ok(&app, "linear_getIssueHistory", json!({"issueId": "WEB-51"})).await;
    assert_eq!(h["history"][0]["type"], "unknown");
    assert_eq!(h["history"].as_array().unwrap().len(), 2);

    let t = ok(
        &app,
        "linear_transferIssue",
        json!({"issueId": "WEB-55", "teamId": PAY}),
    )
    .await;
    assert_eq!(t["issue"]["identifier"], "PAY-123");
    assert_eq!(
        state(&w)["issues"]["WEB-55"]["stateId"],
        "b1bac644-a518-5be3-9055-013f718463f6",
        "Backlog in the new team"
    );
    let d = ok(&app, "linear_duplicateIssue", json!({"issueId": "WEB-57"})).await;
    assert_eq!(d["duplicatedIssue"]["identifier"], "WEB-59");
    assert_eq!(
        d["duplicatedIssue"]["title"],
        "Upgrade to React Router 7 (Copy)"
    );
    let sub = ok(
        &app,
        "linear_convertIssueToSubtask",
        json!({"issueId": "WEB-59", "parentIssueId": "WEB-57"}),
    )
    .await;
    assert_eq!(sub["issue"]["parent"]["identifier"], "WEB-57");
    let rel = ok(
        &app,
        "linear_createIssueRelation",
        json!({"issueId": "WEB-51", "relatedIssueId": "PAY-97", "type": "duplicate_of"}),
    )
    .await;
    assert_eq!(rel["relation"]["issueIdentifier"], "PAY-97");
    assert_eq!(rel["relation"]["type"], "duplicate");
    let id = rel["relation"]["id"].as_str().unwrap().to_owned();
    ok(&app, "linear_deleteIssueRelation", json!({"id": id})).await;
    let label = ok(
        &app,
        "linear_removeIssueLabel",
        json!({"issueId": "WEB-51", "labelId": BUG}),
    )
    .await;
    assert_eq!(
        label,
        json!({"success": true, "issueId": "d27f2e8f-39a2-58fa-be0d-a2b0f760cbb2", "labelId": BUG})
    );
    let arch = ok(&app, "linear_archiveIssue", json!({"issueId": "WEB-44"})).await;
    assert_eq!(arch["message"], "Issue WEB-44 has been archived");
    assert!(state(&w)["issues"]["WEB-44"]["archivedAt"].is_string());
}

#[tokio::test]
async fn comments() {
    let w = world();
    let app = service_app(w.clone(), &Linear);
    let c = ok(
        &app,
        "linear_createComment",
        json!({"issueId": "WEB-51", "body": "Acme Retail escalated this."}),
    )
    .await;
    assert_eq!(c["issue"]["identifier"], "WEB-51");
    assert_eq!(c["user"]["name"], "Casey Morgan");
    assert!(c["url"].as_str().unwrap().starts_with(
        "https://linear.app/octo/issue/WEB-51/checkout-button-unresponsive-on-safari#comment-"
    ));
    assert!(c["parent"].is_null() && c["projectUpdate"].is_null());
    assert_eq!(
        state(&w)["comments"]["WEB-51#2"]["body"],
        "Acme Retail escalated this."
    );
    let id = c["id"].as_str().unwrap().to_owned();
    let list = ok(&app, "linear_getComments", json!({"issueId": "WEB-51"})).await;
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert_eq!(list[0]["id"], id, "newest first");
    let edited = ok(
        &app,
        "linear_updateComment",
        json!({"id": id, "body": "Edited"}),
    )
    .await;
    assert!(edited["editedAt"].is_string());
    assert_eq!(
        err(&app, "linear_updateComment", json!({"id": id})).await,
        "Error: At least one comment field must be provided"
    );
    assert_eq!(
        err(&app, "linear_getComments", json!({})).await,
        "Error: A comment target ID is required"
    );
    assert_eq!(
        err(&app, "linear_createComment", json!({"body": "x"})).await,
        "Error: A comment target ID or parentId is required"
    );
    let reply = ok(
        &app,
        "linear_createComment",
        json!({"parentId": id, "body": "reply"}),
    )
    .await;
    assert_eq!(reply["parent"]["id"], id);
    let del = ok(&app, "linear_deleteComment", json!({"id": id})).await;
    assert_eq!(del["success"], true);
}

#[tokio::test]
async fn cycles() {
    let w = world();
    let app = service_app(w.clone(), &Linear);
    let active = ok(&app, "linear_getActiveCycle", json!({"teamId": PAY})).await;
    assert_eq!(active["id"], CYCLE42);
    assert_eq!(active["number"], 42);
    assert!(
        active.get("name").is_none(),
        "null cycle names are dropped by the SDK"
    );
    assert_eq!(active["issueCount"], 3);
    let list = ok(&app, "linear_getCycles", json!({"teamId": PAY})).await;
    assert_eq!(list.as_array().unwrap().len(), 3);
    let added = ok(
        &app,
        "linear_addIssueToCycle",
        json!({"issueId": "PAY-121", "cycleId": CYCLE42}),
    )
    .await;
    assert_eq!(
        added["issue"]["cycle"],
        json!({"id": CYCLE42, "number": 42})
    );
    assert_eq!(state(&w)["issues"]["PAY-121"]["cycleId"], CYCLE42);
    assert!(
        err(
            &app,
            "linear_addIssueToCycle",
            json!({"issueId": "WEB-51", "cycleId": CYCLE42})
        )
        .await
        .contains("cycle must belong")
    );
    let issues = ok(
        &app,
        "linear_getCycleIssues",
        json!({"cycleId": CYCLE42, "states": ["Backlog"]}),
    )
    .await;
    assert_eq!(issues[0]["identifier"], "PAY-121");
    let stats = ok(&app, "linear_getCycleStats", json!({"id": CYCLE42})).await;
    assert_eq!(stats["issueCount"], 4);
    let removed = ok(
        &app,
        "linear_removeIssueFromCycle",
        json!({"issueId": "PAY-121", "cycleId": CYCLE42}),
    )
    .await;
    assert_eq!(removed["cycle"]["number"], 42);
    assert!(
        err(
            &app,
            "linear_removeIssueFromCycle",
            json!({"issueId": "PAY-121", "cycleId": CYCLE42})
        )
        .await
        .contains("is not associated with cycle")
    );
    let c = ok(&app, "linear_createCycle", json!({"teamId": WEB, "startsAt": "2026-11-05T06:00:00Z", "endsAt": "2026-11-19T06:00:00Z"})).await;
    assert_eq!(c["number"], 8);
    let id = c["id"].as_str().unwrap().to_owned();
    let done = ok(&app, "linear_completeCycle", json!({"id": id})).await;
    assert!(done["completedAt"].is_string());
    assert_eq!(
        err(&app, "linear_getActiveCycle", json!({"teamId": "nope"})).await,
        "Error: Entity not found: Team - Could not find referenced Team."
    );
}

#[tokio::test]
async fn projects_milestones_and_updates() {
    let w = world();
    let app = service_app(w.clone(), &Linear);
    let ps = ok(&app, "linear_getProjects", json!({})).await;
    assert_eq!(ps.as_array().unwrap().len(), 4);
    assert_eq!(
        ps[1]["url"],
        "https://linear.app/octo/project/payments-v2-8b2e4d6f1a3c"
    );
    assert_eq!(ps[1]["lead"]["name"], "Lena Fischer");
    let created = ok(&app, "linear_createProject", json!({"name": "Fraud rules", "teamIds": [PAY], "state": "planned", "targetDate": "2026-12-01"})).await;
    assert_eq!(created["state"], "planned");
    assert_eq!(created["targetDate"], "2026-12-01");
    assert!(err(&app, "linear_createProject", json!({"name": "x", "teamIds": [PAY], "state": "shipping"})).await.contains("Project status \"shipping\" not found. Use a project status ID or one of: Backlog, Planned"));
    let pid = created["id"].as_str().unwrap().to_owned();
    ok(
        &app,
        "linear_addProjectMember",
        json!({"projectId": pid, "userId": ALEX_KIM}),
    )
    .await;
    let members = ok(&app, "linear_getProjectMembers", json!({"projectId": pid})).await;
    assert_eq!(members[0]["name"], "Alex Kim");
    let upd = ok(
        &app,
        "linear_createProjectUpdate",
        json!({"projectId": PAYV2, "body": "On track for GA", "health": "onTrack"}),
    )
    .await;
    assert_eq!(upd["project"]["name"], "Payments v2");
    let list = ok(
        &app,
        "linear_getProjectUpdates",
        json!({"projectId": PAYV2}),
    )
    .await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    let ms = ok(&app, "linear_getMilestones", json!({"projectId": PAYV2})).await;
    assert_eq!(ms.as_array().unwrap().len(), 2);
    let m = ok(
        &app,
        "linear_createMilestone",
        json!({"name": "Fraud beta", "projectId": pid, "targetDate": "2026-11-20"}),
    )
    .await;
    assert_eq!(m["status"], "unstarted");
    let added = ok(
        &app,
        "linear_addIssueToProject",
        json!({"issueId": "WEB-57", "projectId": pid}),
    )
    .await;
    assert_eq!(added["issue"]["project"]["name"], "Fraud rules");
    let arch = ok(&app, "linear_archiveProject", json!({"projectId": pid})).await;
    assert!(arch["project"]["archivedAt"].is_string());
    assert_eq!(
        err(
            &app,
            "linear_getInitiativeById",
            json!({"initiativeId": "missing"})
        )
        .await,
        "Error: Entity not found: Initiative - Could not find referenced Initiative."
    );
}

#[tokio::test]
async fn documents_initiatives_and_other_families() {
    let w = world();
    let app = service_app(w.clone(), &Linear);
    let docs = ok(&app, "linear_searchDocuments", json!({"term": "pgbouncer"})).await;
    assert_eq!(docs["totalCount"], 1);
    let doc = ok(
        &app,
        "linear_createDocument",
        json!({"title": "Fraud rules RFC", "content": "Draft", "teamId": PAY}),
    )
    .await;
    assert_eq!(doc["team"], json!({"id": PAY, "name": "Payments"}));
    assert_eq!(
        err(&app, "linear_createDocument", json!({"title": " "})).await,
        "Error: Document title is required"
    );
    let inits = ok(&app, "linear_getInitiatives", json!({})).await;
    let iid = inits[0]["id"].as_str().unwrap().to_owned();
    let one = ok(
        &app,
        "linear_getInitiativeById",
        json!({"initiativeId": iid}),
    )
    .await;
    assert_eq!(one["projects"].as_array().unwrap().len(), 2);
    let ip = ok(
        &app,
        "linear_getInitiativeProjects",
        json!({"initiativeId": iid}),
    )
    .await;
    assert_eq!(ip[0]["name"], "Postgres 16 migration");
    assert_eq!(
        err(&app, "linear_getRoadmaps", json!({})).await,
        "Error: Roadmaps are not enabled for organization Octo Corp"
    );
    assert!(
        err(&app, "linear_getCustomFields", json!({}))
            .await
            .contains("does not expose a custom field definitions query")
    );
    let cust = ok(&app, "linear_getCustomers", json!({"name": "acme"})).await;
    assert_eq!(cust[0]["owner"]["name"], "Lena Fischer");
    let need = ok(
        &app,
        "linear_createCustomerNeed",
        json!({"customerId": cust[0]["id"], "issueId": "WEB-51", "body": "Blocking checkouts"}),
    )
    .await;
    assert_eq!(need["issue"]["identifier"], "WEB-51");
    assert_eq!(
        ok(&app, "linear_getUnreadNotificationCount", json!({})).await,
        json!({"count": 1})
    );
    assert_eq!(
        ok(&app, "linear_markAllNotificationsAsRead", json!({})).await,
        json!({"success": true, "count": 1})
    );
    let pipe = ok(
        &app,
        "linear_createReleasePipeline",
        json!({"name": "Web", "teamIds": [WEB]}),
    )
    .await;
    let pipe_id = pipe["id"].as_str().unwrap().to_owned();
    assert!(
        err(
            &app,
            "linear_completeRelease",
            json!({"pipelineId": pipe_id})
        )
        .await
        .contains("no completed release stage exists")
    );
    ok(&app, "linear_createReleaseStage", json!({"pipelineId": pipe_id, "name": "Released", "color": "#00ff00", "position": 1, "type": "completed"})).await;
    let rel = ok(
        &app,
        "linear_completeRelease",
        json!({"pipelineId": pipe_id, "version": "4.12.0"}),
    )
    .await;
    assert_eq!(rel["stage"]["type"], "completed");
    let hook = ok(&app, "linear_createWebhook", json!({"url": "https://hooks.octo-corp.example/linear", "resourceTypes": ["Issue"], "teamId": PAY})).await;
    assert_eq!(hook["team"]["key"], "PAY");
    assert_eq!(
        err(
            &app,
            "linear_createWebhook",
            json!({"url": "http://localhost/x", "resourceTypes": ["Issue"]})
        )
        .await,
        "Error: Invalid arguments for createWebhook"
    );
    let view = ok(
        &app,
        "linear_createSavedView",
        json!({"name": "My bugs", "filterData": {"labels": {"name": {"eq": "Bug"}}}}),
    )
    .await;
    let fav = ok(
        &app,
        "linear_addToFavorites",
        json!({"entityId": view["id"]}),
    )
    .await;
    assert_eq!(fav["favorite"]["customView"]["name"], "My bugs");
    assert_eq!(
        ok(&app, "linear_getFavoriteViews", json!({}))
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let url = ok(
        &app,
        "linear_generateOAuthAuthorizationUrl",
        json!({"clientId": "abc", "redirectUri": "https://x.example/cb", "scopes": ["write"]}),
    )
    .await;
    assert_eq!(url["scopes"], json!(["read", "write"]));
    assert_eq!(
        ok(&app, "linear_getServerStatus", json!({})).await["toolCount"],
        198
    );
    let t = ok(
        &app,
        "linear_createTeam",
        json!({"name": "Data", "key": "data"}),
    )
    .await;
    assert_eq!(t["key"], "DATA");
    assert_eq!(
        ok(&app, "linear_getWorkflowStates", json!({"teamId": t["id"]}))
            .await
            .as_array()
            .unwrap()
            .len(),
        6
    );
}

/// Runs each scenario's oracle calls and checks they succeed.
#[tokio::test]
async fn scenario_solutions_succeed() {
    let w = world();
    let app = service_app(w.clone(), &Linear);
    ok(&app, "linear_searchIssues", json!({"query": "Safari"})).await;
    ok(
        &app,
        "linear_updateIssue",
        json!({"id": "WEB-51", "priority": 1, "assigneeId": ALEX_KIM}),
    )
    .await;
    ok(
        &app,
        "linear_createComment",
        json!({"issueId": "WEB-51", "body": "Escalated: Acme Retail"}),
    )
    .await;
    let s = state(&w);
    assert_eq!(s["issues"]["WEB-51"]["priority"], 1);
    assert!(
        s["comments"]["WEB-51#2"]["body"]
            .as_str()
            .unwrap()
            .contains("Acme")
    );
    ok(&app, "linear_getActiveCycle", json!({"teamId": PAY})).await;
    ok(
        &app,
        "linear_addIssueToCycle",
        json!({"issueId": "PAY-121", "cycleId": CYCLE42}),
    )
    .await;
    ok(
        &app,
        "linear_updateIssue",
        json!({"id": "PAY-121", "stateId": PAY_IN_PROGRESS}),
    )
    .await;
    assert_eq!(state(&w)["issues"]["PAY-121"]["stateId"], PAY_IN_PROGRESS);
}
