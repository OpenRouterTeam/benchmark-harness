use std::path::{Path, PathBuf};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode},
};
use mock_core::{
    dataset::Dataset,
    grading::{GradeInput, grade},
    serve::service_app,
    world::{SharedWorld, WorldState, shared},
};
use mock_zendesk::Zendesk;
use serde_json::{Value, json};
use tower::ServiceExt;

const SCENARIOS: [&str; 4] = [
    "find-invoice-attachment",
    "reply-refund-and-solve",
    "create-ticket-for-caller",
    "escalate-outage-ticket",
];

fn scenario_dir(slug: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../scenarios/business/zendesk")
        .join(slug)
}

fn world(slug: &str) -> SharedWorld {
    shared(WorldState::from_dataset(Dataset::load(&scenario_dir(slug)).unwrap()).unwrap())
}

fn app(world: &SharedWorld) -> Router {
    service_app(world.clone(), &Zendesk)
}

async fn http(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, HeaderMap, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "octo-corp.zendesk.com")
        .header("content-type", "application/json")
        .body(body.map_or(Body::empty(), |b| Body::from(b.to_string())))
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

/// Calls a replica tool and returns the raw `CallToolResult`.
async fn tool_result(app: &Router, name: &str, arguments: Value) -> Value {
    let (status, _, body) = http(
        app,
        "POST",
        "/_mcp/tools/call",
        Some(json!({ "name": name, "arguments": arguments })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    body
}

/// The text of a tool result (the server never sets `isError`).
async fn tool(app: &Router, name: &str, arguments: Value) -> String {
    let result = tool_result(app, name, arguments).await;
    assert!(result.get("isError").is_none(), "{result}");
    result["content"][0]["text"].as_str().unwrap().to_owned()
}

fn state(world: &SharedWorld) -> Value {
    world.lock().unwrap().snapshot()["services"]["zendesk"].clone()
}

// ───────────────────────────── tools ─────────────────────────────

#[tokio::test]
async fn get_ticket_prints_compact_python_json() {
    let w = world("find-invoice-attachment");
    let text = tool(&app(&w), "get_ticket", json!({ "ticket_id": 3108 })).await;
    assert!(
        text.starts_with(r#"{"id": 3108, "subject": "Question about our October bill", "description": "Hi,\n\nOur October bill"#),
        "{text}"
    );
    let v: Value = serde_json::from_str(&text).unwrap();
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "id",
            "subject",
            "description",
            "status",
            "priority",
            "created_at",
            "updated_at",
            "requester_id",
            "assignee_id",
            "organization_id"
        ]
    );
    assert_eq!(v["organization_id"], 3600101, "derived from the requester");
    assert_eq!(v["created_at"], "2026-10-21T09:47:00Z");
}

#[tokio::test]
async fn tool_errors_match_the_server_text() {
    let w = world("find-invoice-attachment");
    let app = app(&w);
    assert_eq!(
        tool(&app, "get_ticket", json!({ "ticket_id": 999 })).await,
        r#"Error: Failed to get ticket 999: {"error": "RecordNotFound", "description": "Not found"}"#
    );
    assert_eq!(
        tool(&app, "get_ticket", json!({})).await,
        "Error: Missing arguments"
    );
    assert_eq!(
        tool(&app, "get_ticket", json!({ "id": 1 })).await,
        "Error: 'ticket_id'"
    );
    assert_eq!(
        tool(&app, "get_ticket_comments", json!({ "ticket_id": 42 })).await,
        r#"Error: Failed to get comments for ticket 42: {"error": "RecordNotFound", "description": "Not found"}"#
    );
    assert_eq!(
        tool(
            &app,
            "create_ticket_comment",
            json!({ "ticket_id": 42, "comment": "hi" })
        )
        .await,
        r#"Error: Failed to post comment on ticket 42: {"error": "RecordNotFound", "description": "Not found"}"#
    );
    assert_eq!(
        tool(&app, "update_ticket", json!({ "ticket_id": "abc" })).await,
        "Error: invalid literal for int() with base 10: 'abc'"
    );
    assert_eq!(
        tool(
            &app,
            "update_ticket",
            json!({ "ticket_id": 3108, "status": "on-hold" })
        )
        .await,
        r#"Error: Failed to update ticket 3108: {"error": "RecordInvalid", "description": "Record validation errors", "details": {"status": [{"description": "Status: is invalid", "error": "InvalidValue"}]}}"#
    );
}

#[tokio::test]
async fn get_tickets_sorts_and_paginates_like_the_server() {
    let w = world("find-invoice-attachment");
    let app = app(&w);
    let text = tool(&app, "get_tickets", json!({})).await;
    assert!(
        text.starts_with("{\n  \"tickets\": [\n    {\n      \"id\": 3114,"),
        "{text}"
    );
    let v: Value = serde_json::from_str(&text).unwrap();
    let ids: Vec<u64> = v["tickets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_u64().unwrap())
        .collect();
    assert_eq!(ids, [3114, 3112, 3110, 3108, 3105, 3101], "created_at desc");
    assert_eq!(v["count"], 6);
    assert_eq!(v["has_more"], false);
    assert_eq!(v["next_page"], Value::Null);

    let v: Value = serde_json::from_str(
        &tool(
            &app,
            "get_tickets",
            json!({ "page": 2, "per_page": 4, "sort_by": "priority", "sort_order": "asc" }),
        )
        .await,
    )
    .unwrap();
    let ids: Vec<u64> = v["tickets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_u64().unwrap())
        .collect();
    // priority asc: none(3114), low(3110), normal(3101, 3105, 3108), high(3112)
    assert_eq!(ids, [3108, 3112]);
    assert_eq!(v["page"], 2);
    assert_eq!(v["previous_page"], 1);
    assert_eq!(v["has_more"], false);

    let v: Value =
        serde_json::from_str(&tool(&app, "get_tickets", json!({ "per_page": 500 })).await).unwrap();
    assert_eq!(v["per_page"], 100, "capped like the server");
}

#[tokio::test]
async fn get_ticket_comments_includes_attachment_metadata() {
    let w = world("find-invoice-attachment");
    let text = tool(
        &app(&w),
        "get_ticket_comments",
        json!({ "ticket_id": 3108 }),
    )
    .await;
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 3);
    assert_eq!(
        v[2]["attachments"][0],
        json!({
            "id": 77000103,
            "file_name": "Lumen-Invoice-INV-20931.pdf",
            "content_url": "https://octo-corp.zendesk.com/attachments/token/Wm2Bq7Jd4Nf9Rt1Kx8Ls3Vh5G/?name=Lumen-Invoice-INV-20931.pdf",
            "content_type": "application/pdf",
            "size": 118734
        })
    );
    assert_eq!(v[0]["author_id"], 9010101);
    assert!(
        v[0]["html_body"]
            .as_str()
            .unwrap()
            .starts_with("<div class=\"zd-comment\" dir=\"auto\"><p>Hi,</p>")
    );
}

#[tokio::test]
async fn get_ticket_attachment_returns_images_and_rejects_other_types() {
    let w = world("find-invoice-attachment");
    let app = app(&w);
    let image = tool_result(
        &app,
        "get_ticket_attachment",
        json!({ "content_url": "https://octo-corp.zendesk.com/attachments/token/q3Lr8sVb2KxP1mWn7TgYh0aZc/?name=export-timeout.png" }),
    )
    .await;
    assert_eq!(image["content"][0]["type"], "image");
    assert_eq!(image["content"][0]["mimeType"], "image/png");
    assert!(
        image["content"][0]["data"]
            .as_str()
            .unwrap()
            .starts_with("iVBORw0KGgo")
    );

    let pdf_url = "https://octo-corp.zendesk.com/attachments/token/Wm2Bq7Jd4Nf9Rt1Kx8Ls3Vh5G/?name=Lumen-Invoice-INV-20931.pdf";
    assert_eq!(
        tool(
            &app,
            "get_ticket_attachment",
            json!({ "content_url": pdf_url })
        )
        .await,
        "Error: Attachment type 'application/pdf' is not allowed. Supported types: ['image/gif', 'image/jpeg', 'image/png', 'image/webp']"
    );
    let missing = "https://octo-corp.zendesk.com/attachments/token/nope/?name=x.png";
    assert_eq!(
        tool(
            &app,
            "get_ticket_attachment",
            json!({ "content_url": missing })
        )
        .await,
        format!("Error: 404 Client Error: Not Found for url: {missing}")
    );
}

#[tokio::test]
async fn create_ticket_stores_a_full_ticket_and_first_comment() {
    let w = world("create-ticket-for-caller");
    let app = app(&w);
    let text = tool(
        &app,
        "create_ticket",
        json!({ "subject": "Scanner sync", "description": "Scanners stopped syncing.", "requester_id": 9010301, "priority": "high", "type": "incident", "tags": ["Scanners", "sync"] }),
    )
    .await;
    assert!(
        text.starts_with(
            "{\n  \"message\": \"Ticket created successfully\",\n  \"ticket\": {\n    \"id\": 5109,"
        ),
        "{text}"
    );
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["ticket"]["status"], "new");
    assert_eq!(v["ticket"]["organization_id"], 3600301);
    assert_eq!(v["ticket"]["tags"], json!(["scanners", "sync"]));
    assert_eq!(
        v["ticket"]["created_at"], "2026-10-30T10:15:07Z",
        "one clock tick"
    );

    let s = state(&w);
    assert_eq!(s["tickets"]["5109"]["requester_id"], 9010301);
    assert_eq!(
        s["tickets"]["5109"]["submitter_id"], 9010001,
        "the token owner"
    );
    assert_eq!(s["comments"]["5109/1"]["body"], "Scanners stopped syncing.");
    assert_eq!(s["comments"]["5109/1"]["id"], 41000510803_u64);
    assert_eq!(s["audits"]["41000510804"]["ticket_id"], 5109);

    assert_eq!(
        tool(
            &app,
            "create_ticket",
            json!({ "subject": "x", "description": "y", "priority": "asap" })
        )
        .await,
        r#"Error: Failed to create ticket: {"error": "RecordInvalid", "description": "Record validation errors", "details": {"priority": [{"description": "Priority: is invalid", "error": "InvalidValue"}]}}"#
    );
}

#[tokio::test]
async fn create_ticket_comment_renders_markdown_and_respects_public() {
    let w = world("escalate-outage-ticket");
    let app = app(&w);
    assert_eq!(
        tool(
            &app,
            "create_ticket_comment",
            json!({ "ticket_id": 6230, "comment": "Escalated: **INC-4471**\nPaging on-call.", "public": false })
        )
        .await,
        "Comment created successfully: Escalated: **INC-4471**\nPaging on-call."
    );
    let s = state(&w);
    let c = &s["comments"]["6230/3"];
    assert_eq!(c["public"], false);
    assert_eq!(
        c["html_body"],
        "<p>Escalated: <strong>INC-4471</strong><br>\nPaging on-call.</p>\n"
    );
    assert_eq!(c["body"], "Escalated: INC-4471\nPaging on-call.");
    assert_eq!(c["author_id"], 9010001);
    assert_eq!(s["tickets"]["6230"]["updated_at"], "2026-10-30T13:20:07Z");

    tool(
        &app,
        "create_ticket_comment",
        json!({ "ticket_id": 6230, "comment": "Update" }),
    )
    .await;
    assert_eq!(
        state(&w)["comments"]["6230/4"]["public"],
        true,
        "defaults to public"
    );
}

#[tokio::test]
async fn update_ticket_changes_fields_and_reports_the_fresh_ticket() {
    let w = world("escalate-outage-ticket");
    let app = app(&w);
    let text = tool(
        &app,
        "update_ticket",
        json!({ "ticket_id": 6230, "priority": "urgent", "tags": ["api", "escalated"], "due_at": null }),
    )
    .await;
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["message"], "Ticket updated successfully");
    assert_eq!(v["ticket"]["priority"], "urgent");
    assert_eq!(v["ticket"]["tags"], json!(["api", "escalated"]));
    let s = state(&w);
    assert_eq!(s["tickets"]["6230"]["priority"], "urgent");
    let audit = s["audits"].as_object().unwrap().values().next().unwrap();
    assert_eq!(audit["events"][0]["field_name"], "priority");
    assert_eq!(audit["events"][0]["previous_value"], "normal");

    // Solving an unassigned ticket assigns it to the acting agent; closed tickets are frozen.
    tool(
        &app,
        "update_ticket",
        json!({ "ticket_id": 6232, "assignee_id": null, "status": "closed" }),
    )
    .await;
    assert_eq!(
        tool(
            &app,
            "update_ticket",
            json!({ "ticket_id": 6232, "priority": "high" })
        )
        .await,
        r#"Error: Failed to update ticket 6232: {"error": "RecordInvalid", "description": "Record validation errors", "details": {"status": [{"description": "Status: closed prevents ticket update", "error": "ClosedPreventsUpdate"}]}}"#
    );
    assert_eq!(
        tool(&app, "update_ticket", json!({ "ticket_id": 1 })).await,
        r#"Error: Failed to update ticket 1: {"error": "RecordNotFound", "description": "Not found"}"#
    );
}

// ───────────────────────────── REST ─────────────────────────────

#[tokio::test]
async fn rest_tickets_show_list_and_paginate() {
    let w = world("reply-refund-and-solve");
    let app = app(&w);
    let (status, _, body) = http(&app, "GET", "/api/v2/tickets/4217.json", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["ticket"]["url"],
        "https://octo-corp.zendesk.com/api/v2/tickets/4217.json"
    );
    assert_eq!(
        body["ticket"]["subject"],
        "Charged twice for October invoice"
    );
    assert_eq!(body["ticket"]["organization_id"], 3600201);

    let (status, _, body) = http(&app, "GET", "/api/v2/tickets/1", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body,
        json!({ "error": "RecordNotFound", "description": "Not found" })
    );

    let (_, _, body) = http(&app, "GET", "/api/v2/tickets.json?per_page=2", None).await;
    assert_eq!(body["count"], 5);
    assert_eq!(body["tickets"][0]["id"], 4201, "id ascending by default");
    assert_eq!(
        body["next_page"],
        "https://octo-corp.zendesk.com/api/v2/tickets.json?page=2&per_page=2"
    );
    assert_eq!(body["previous_page"], Value::Null);

    let (_, _, body) = http(
        &app,
        "GET",
        "/api/v2/tickets?page=3&per_page=2&sort_by=created_at&sort_order=desc",
        None,
    )
    .await;
    assert_eq!(body["tickets"].as_array().unwrap().len(), 1);
    assert_eq!(body["tickets"][0]["id"], 4201);
    assert_eq!(body["next_page"], Value::Null);
    assert_eq!(
        body["previous_page"],
        "https://octo-corp.zendesk.com/api/v2/tickets.json?page=2&per_page=2&sort_by=created_at&sort_order=desc"
    );

    let (status, _, body) = http(&app, "GET", "/api/v2/nope", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "InvalidEndpoint");
}

#[tokio::test]
async fn rest_create_and_update_tickets_with_comments() {
    let w = world("reply-refund-and-solve");
    let app = app(&w);
    let (status, headers, body) = http(
        &app,
        "POST",
        "/api/v2/tickets.json",
        Some(json!({ "ticket": { "subject": "Printer jam", "comment": { "body": "Label printer jams." }, "requester": { "name": "New Person", "email": "new@example.com" } } })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        headers["location"],
        "https://octo-corp.zendesk.com/api/v2/tickets/4221.json"
    );
    assert_eq!(body["ticket"]["id"], 4221);
    assert_eq!(
        body["ticket"]["requester_id"], 9010204,
        "requester created as end-user"
    );
    assert_eq!(body["audit"]["events"][0]["type"], "Comment");

    let (status, _, body) = http(
        &app,
        "POST",
        "/api/v2/tickets",
        Some(json!({ "ticket": { "subject": "x" } })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body["details"]["base"][0]["description"],
        "Description: cannot be blank"
    );

    let (status, _, body) = http(
        &app,
        "PUT",
        "/api/v2/tickets/4219.json",
        Some(json!({ "ticket": { "status": "pending", "comment": { "body": "Internal: waiting on PO", "public": false }, "additional_tags": ["needs_po"] } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ticket"]["status"], "pending");
    assert_eq!(body["ticket"]["tags"], json!(["seats", "needs_po"]));
    let s = state(&w);
    assert_eq!(s["comments"]["4219/2"]["public"], false);
    assert_eq!(s["comments"]["4219/2"]["body"], "Internal: waiting on PO");

    let (status, _, body) = http(
        &app,
        "PUT",
        "/api/v2/tickets/77",
        Some(json!({ "ticket": { "status": "open" } })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "RecordNotFound");
}

#[tokio::test]
async fn rest_comments_and_attachments() {
    let w = world("find-invoice-attachment");
    let app = app(&w);
    let (status, _, body) = http(&app, "GET", "/api/v2/tickets/3108/comments.json", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["count"], 3);
    assert_eq!(body["comments"][2]["type"], "Comment");
    assert_eq!(body["comments"][2]["attachments"][0]["id"], 77000103);
    assert_eq!(body["next_page"], Value::Null);

    let (_, _, body) = http(
        &app,
        "GET",
        "/api/v2/tickets/3108/comments?sort_order=desc&per_page=1",
        None,
    )
    .await;
    assert_eq!(body["comments"][0]["id"], 41000310804_u64);
    assert!(body["next_page"].is_string());

    let (status, _, body) = http(&app, "GET", "/api/v2/tickets/999/comments", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "RecordNotFound");

    let (status, _, body) = http(&app, "GET", "/api/v2/attachments/77000103.json", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["attachment"]["file_name"],
        "Lumen-Invoice-INV-20931.pdf"
    );
    assert_eq!(
        body["attachment"]["malware_scan_result"],
        "malware_not_found"
    );

    let req = Request::get("/attachments/token/q3Lr8sVb2KxP1mWn7TgYh0aZc/?name=export-timeout.png")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["content-type"], "image/png");
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
}

#[tokio::test]
async fn rest_users_and_organizations() {
    let w = world("find-invoice-attachment");
    let app = app(&w);
    let (_, _, body) = http(&app, "GET", "/api/v2/users/me.json", None).await;
    assert_eq!(body["user"]["name"], "Jordan Blake");
    assert_eq!(body["user"]["role"], "admin");

    let (_, _, body) = http(&app, "GET", "/api/v2/users.json?role=end-user", None).await;
    assert_eq!(body["count"], 4);

    let (_, _, body) = http(&app, "GET", "/api/v2/users/search.json?query=priya", None).await;
    let names: Vec<&str> = body["users"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Priya Raman", "Priya Ramesh"]);

    let (_, _, body) = http(&app, "GET", "/api/v2/users/9010101/tickets/requested", None).await;
    let ids: Vec<u64> = body["tickets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_u64().unwrap())
        .collect();
    assert_eq!(ids, [3101, 3108, 3112]);

    let (status, _, body) = http(&app, "GET", "/api/v2/users/1", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "RecordNotFound");

    let (_, _, body) = http(&app, "GET", "/api/v2/organizations/3600101.json", None).await;
    assert_eq!(body["organization"]["name"], "Lumen Analytics");
    let (_, _, body) = http(
        &app,
        "GET",
        "/api/v2/organizations/3600101/tickets.json",
        None,
    )
    .await;
    assert_eq!(body["count"], 4);
    let (_, _, body) = http(&app, "GET", "/api/v2/organizations", None).await;
    assert_eq!(body["organizations"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn rest_help_center() {
    let w = world("escalate-outage-ticket");
    let app = app(&w);
    let (_, _, body) = http(&app, "GET", "/api/v2/help_center/articles.json", None).await;
    assert_eq!(body["count"], 2);
    assert_eq!(body["page_count"], 1);
    assert_eq!(body["sort_by"], "position");
    assert_eq!(
        body["articles"][0]["html_url"],
        "https://octo-corp.zendesk.com/hc/en-us/articles/360030001-escalating-api-outages-to-on-call-engineering"
    );

    let (_, _, body) = http(
        &app,
        "GET",
        "/api/v2/help_center/articles/search.json?query=502%20proxy",
        None,
    )
    .await;
    assert_eq!(body["count"], 1);
    assert_eq!(body["results"][0]["id"], 360030002);
    assert_eq!(body["results"][0]["result_type"], "article");

    let (_, _, body) = http(
        &app,
        "GET",
        "/api/v2/help_center/sections/360010001/articles.json",
        None,
    )
    .await;
    assert_eq!(body["articles"].as_array().unwrap().len(), 2);
    let (_, _, body) = http(
        &app,
        "GET",
        "/api/v2/help_center/categories/360020001/sections",
        None,
    )
    .await;
    assert_eq!(body["sections"][0]["name"], "API status and incidents");
    let (_, _, body) = http(&app, "GET", "/api/v2/help_center/sections", None).await;
    assert_eq!(body["sections"].as_array().unwrap().len(), 1);
    let (_, _, body) = http(&app, "GET", "/api/v2/help_center/categories.json", None).await;
    assert_eq!(body["categories"][0]["name"], "Developers");
    let (status, _, body) = http(&app, "GET", "/api/v2/help_center/articles/1", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "RecordNotFound");

    // A world without Help Center data still answers with empty lists.
    let w = world("reply-refund-and-solve");
    let (_, _, body) = http(
        &service_app(w.clone(), &Zendesk),
        "GET",
        "/api/v2/help_center/articles",
        None,
    )
    .await;
    assert_eq!(body["articles"], json!([]));
    assert_eq!(body["count"], 0);
}

// ───────────────────────────── scenarios ─────────────────────────────

/// Replays each scenario's `[[solution.call]]` through the tool endpoint and grades it with
/// the real grader: the oracle must pass and doing nothing must fail.
#[tokio::test]
async fn scenario_oracles_pass_and_nop_fails() {
    for slug in SCENARIOS {
        let scenario = scenario::Scenario::load(&scenario_dir(slug)).unwrap();

        let nop = world(slug);
        let snapshot = nop.lock().unwrap().snapshot();
        let g = grade(&GradeInput {
            expect: &scenario.expect,
            state: snapshot,
            journal: &[],
            answer: None,
            tools_ips: vec!["unknown".to_owned()],
            context_overflow: false,
        });
        assert_eq!(g.reward, 0.0, "{slug}: nop must fail");

        let w = world(slug);
        let app = app(&w);
        for call in &scenario.solution.calls {
            assert_eq!(call.endpoint, "zendesk");
            let text = tool(&app, &call.tool, call.arguments.clone()).await;
            assert!(
                !text.starts_with("Error:"),
                "{slug}: {} -> {text}",
                call.tool
            );
        }
        let (snapshot, journal) = {
            let world = w.lock().unwrap();
            (world.snapshot(), world.journal.entries().to_vec())
        };
        let g = grade(&GradeInput {
            expect: &scenario.expect,
            state: snapshot,
            journal: &journal,
            answer: scenario.solution.answer.as_deref(),
            tools_ips: vec!["unknown".to_owned()],
            context_overflow: false,
        });
        assert_eq!(g.reward, 1.0, "{slug}: oracle must pass: {g:?}");
    }
}

#[tokio::test]
async fn identical_requests_give_identical_responses() {
    let mut outputs = Vec::new();
    for _ in 0..2 {
        let w = world("reply-refund-and-solve");
        let app = app(&w);
        tool(
            &app,
            "create_ticket_comment",
            json!({ "ticket_id": 4217, "comment": "Refund RF-88213 issued." }),
        )
        .await;
        outputs.push(
            tool(
                &app,
                "update_ticket",
                json!({ "ticket_id": 4217, "status": "solved" }),
            )
            .await,
        );
    }
    assert_eq!(outputs[0], outputs[1]);
    assert!(
        outputs[0].contains("\"updated_at\": \"2026-10-29T16:30:14Z\""),
        "{}",
        outputs[0]
    );
}
