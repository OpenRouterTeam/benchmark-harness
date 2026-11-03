use std::path::Path;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use mock_core::{
    dataset::Dataset,
    serve::service_app,
    world::{SharedWorld, WorldState, shared},
};
use mock_google::{Google, tool_names};
use serde_json::{Value, json};
use tower::ServiceExt;

const INVOICE: &str = "b26a427d1034a022";
const SOC_2026: &str = "150dluRbaiTmEP9Spu-wAk0p_lQupubYY";
const SOC_2025: &str = "12klLxpfyaiox7A1n8sigN7b3qTt3DYXh";
const RENEWALS: &str = "1p91PmGrfYlol_BkGN9Op8j9dWzibNwgg";
const OPS_LIST: &str = "MTc0OTk1NDYxNjk2NzA5";
const FIRST_EVENT: &str = "445b671f493e8a06958e4ff0dd";

fn world(slug: &str) -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../scenarios/collab/google")
        .join(slug);
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

async fn call(app: &Router, name: &str, args: Value) -> Value {
    let body = json!({"name": name, "arguments": args}).to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/_mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn text(v: &Value) -> &str {
    v["content"][0]["text"].as_str().unwrap_or("")
}

async fn ok(app: &Router, name: &str, args: Value) -> String {
    let v = call(app, name, args.clone()).await;
    assert_eq!(v["isError"], false, "{name} {args}: {}", text(&v));
    assert_eq!(v["structuredContent"]["result"], v["content"][0]["text"]);
    text(&v).to_owned()
}

async fn err(app: &Router, name: &str, args: Value) -> String {
    let v = call(app, name, args.clone()).await;
    assert_eq!(
        v["isError"],
        true,
        "{name} {args} should fail: {}",
        text(&v)
    );
    text(&v).to_owned()
}

fn state(w: &SharedWorld) -> Value {
    w.lock().unwrap().snapshot()["services"]["google"].clone()
}

#[tokio::test]
async fn every_tool_answers() {
    let w = world("northwind-invoice-lookup");
    let app = service_app(w, &Google);
    let names = tool_names();
    assert_eq!(names.len(), 119);
    for name in names {
        let v = call(&app, name, json!({})).await;
        assert!(v["content"][0]["text"].is_string(), "{name}: {v}");
    }
    let e = err(&app, "get_gmail_message_content", json!({})).await;
    assert!(
        e.starts_with("Error calling tool 'get_gmail_message_content': 1 validation error"),
        "{e}"
    );
    assert!(e.contains("message_id\n  Missing required argument"), "{e}");
}

#[tokio::test]
async fn gmail_search_and_read() {
    let w = world("northwind-invoice-lookup");
    let app = service_app(w, &Google);
    let out = ok(
        &app,
        "search_gmail_messages",
        json!({"query": "from:northwind.example invoice October"}),
    )
    .await;
    assert!(out.contains(INVOICE), "{out}");
    assert!(!out.contains("northwood"), "{out}");
    let out = ok(
        &app,
        "get_gmail_message_content",
        json!({"message_id": INVOICE}),
    )
    .await;
    assert!(
        out.contains("PO-48213") && out.contains("$12,480.00"),
        "{out}"
    );
    let out = ok(&app, "search_gmail_messages", json!({"query": "PO-48213"})).await;
    assert!(out.contains(INVOICE), "{out}");
    let e = err(
        &app,
        "get_gmail_message_content",
        json!({"message_id": "deadbeef"}),
    )
    .await;
    assert!(e.contains("404"), "{e}");
}

#[tokio::test]
async fn schedule_meeting_with_meet() {
    let w = world("schedule-q1-planning");
    let app = service_app(w.clone(), &Google);
    let fb = ok(&app, "query_freebusy", json!({"time_min": "2026-11-03T00:00:00-05:00", "time_max": "2026-11-04T00:00:00-05:00", "calendar_ids": ["primary", "priya.natarajan@octo-corp.example"]})).await;
    assert!(fb.contains("2026-11-03T15:00:00Z"), "{fb}");
    let out = ok(&app, "manage_event", json!({
        "action": "create", "summary": "Q1 planning sync",
        "start_time": "2026-11-03T14:00:00", "end_time": "2026-11-03T14:45:00", "timezone": "America/New_York",
        "attendees": ["priya.natarajan@octo-corp.example", "marcus.webb@octo-corp.example"], "add_google_meet": true
    })).await;
    assert!(
        out.starts_with("Successfully created event 'Q1 planning sync' for ops@octo-corp.example."),
        "{out}"
    );
    let s = state(&w);
    let created: Vec<(&String, &Value)> = s["calendar_events"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(_, e)| e["summary"] == "Q1 planning sync")
        .collect();
    assert_eq!(created.len(), 1);
    let (id, ev) = created[0];
    assert_eq!(id, FIRST_EVENT, "first event ID for this scenario seed");
    assert_eq!(ev["start"]["dateTime"], "2026-11-03T14:00:00-05:00");
    assert_eq!(ev["end"]["dateTime"], "2026-11-03T14:45:00-05:00");
    assert_eq!(
        ev["conferenceData"]["conferenceSolution"]["key"]["type"], "hangoutsMeet",
        "{ev}"
    );
    assert!(
        ev["attendees"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["email"] == "marcus.webb@octo-corp.example")
    );
    let listed = ok(
        &app,
        "get_events",
        json!({"time_min": "2026-11-03T00:00:00-05:00", "time_max": "2026-11-04T00:00:00-05:00"}),
    )
    .await;
    assert!(
        listed.contains("Q1 planning sync") && listed.contains("Ops standup"),
        "{listed}"
    );
}

#[tokio::test]
async fn sheets_read_and_update() {
    let w = world("mark-northwind-renewed");
    let app = service_app(w.clone(), &Google);
    let list = ok(&app, "list_spreadsheets", json!({})).await;
    assert!(
        list.contains("\"Vendor renewals 2026\"") && list.contains(RENEWALS),
        "{list}"
    );
    let info = ok(
        &app,
        "get_spreadsheet_info",
        json!({"spreadsheet_id": RENEWALS}),
    )
    .await;
    assert!(
        info.contains("\"Renewals\" (ID: 0)") && info.contains("\"Archive\""),
        "{info}"
    );
    let read = ok(
        &app,
        "read_sheet_values",
        json!({"spreadsheet_id": RENEWALS, "range_name": "Renewals!A1:E10"}),
    )
    .await;
    assert!(
        read.starts_with("Successfully read 5 rows from range 'Renewals!A1:E10'"),
        "{read}"
    );
    assert!(read.contains("Row  2: ['Northwind Traders', 'Lena Fischer', '2026-11-30', 'Pending renewal', 'Quote received 10/14']"), "{read}");
    assert!(
        read.contains("Row  4: ['Contoso Ltd', 'Marcus Webb', '2027-03-31', 'Active', '']"),
        "{read}"
    );
    let upd = ok(&app, "modify_sheet_values", json!({"spreadsheet_id": RENEWALS, "range_name": "Renewals!C2:D2", "values": [["2027-11-30", "Renewed"]]})).await;
    assert_eq!(
        upd,
        format!(
            "Successfully updated range 'Renewals!C2:D2' in spreadsheet {RENEWALS} for ops@octo-corp.example. Updated: 2 cells, 1 rows, 2 columns."
        )
    );
    let s = state(&w);
    assert_eq!(
        s["drive_files"][RENEWALS]["sheets"][0]["values"][1],
        json!([
            "Northwind Traders",
            "Lena Fischer",
            "2027-11-30",
            "Renewed",
            "Quote received 10/14"
        ])
    );
    // JSON-string values and a whole-sheet range.
    ok(&app, "modify_sheet_values", json!({"spreadsheet_id": RENEWALS, "range_name": "Archive!A3", "values": "[[\"Litware\", \"Sam Ortiz\"]]"})).await;
    let read = ok(
        &app,
        "read_sheet_values",
        json!({"spreadsheet_id": RENEWALS, "range_name": "Archive"}),
    )
    .await;
    assert!(
        read.contains("Row  3: ['Litware', 'Sam Ortiz', '', '', '']"),
        "{read}"
    );
    let e = err(
        &app,
        "read_sheet_values",
        json!({"spreadsheet_id": RENEWALS, "range_name": "Nope!A1"}),
    )
    .await;
    assert!(e.contains("Unable to parse range: Nope!A1"), "{e}");
}

#[tokio::test]
async fn drive_search_and_share() {
    let w = world("share-soc2-with-contoso");
    let app = service_app(w.clone(), &Google);
    let out = ok(
        &app,
        "search_drive_files",
        json!({"query": "name contains 'SOC 2'"}),
    )
    .await;
    assert!(
        out.starts_with(
            "Found 4 files for ops@octo-corp.example matching 'name contains 'SOC 2'':"
        ),
        "{out}"
    );
    assert!(out.contains(SOC_2026) && out.contains(SOC_2025), "{out}");
    let out = ok(
        &app,
        "search_drive_files",
        json!({"query": "SOC 2 2026", "detailed": false}),
    )
    .await;
    assert!(out.contains(SOC_2026), "{out}");
    let out = ok(&app, "manage_drive_access", json!({"file_id": SOC_2026, "action": "grant", "share_with": "dana.whitfield@contoso.example", "role": "reader"})).await;
    assert!(
        out.starts_with("Successfully shared 'SOC 2 Type II Report 2026.pdf'"),
        "{out}"
    );
    assert!(
        out.contains("User: dana.whitfield@contoso.example (reader)"),
        "{out}"
    );
    let s = state(&w);
    let perms = &s["drive_files"][SOC_2026]["permissions"];
    assert_eq!(perms[1]["emailAddress"], "dana.whitfield@contoso.example");
    assert_eq!(perms[1]["role"], "reader");
    let p = ok(
        &app,
        "get_drive_file_permissions",
        json!({"file_id": SOC_2026}),
    )
    .await;
    assert!(p.contains("Number of permissions: 2"), "{p}");
    let folder = ok(&app, "list_drive_items", json!({"folder_id": "root"})).await;
    assert!(
        folder.contains("\"Compliance\"") && folder.contains("\"Vendors\""),
        "{folder}"
    );
}

#[tokio::test]
async fn docs_tasks_contacts_and_comments() {
    let w = world("northwind-invoice-lookup");
    let app = service_app(w.clone(), &Google);
    let created = ok(
        &app,
        "create_doc",
        json!({"title": "Notes", "content": "Hello world"}),
    )
    .await;
    let id = created
        .split("(ID: ")
        .nth(1)
        .unwrap()
        .split(')')
        .next()
        .unwrap()
        .to_owned();
    ok(
        &app,
        "modify_doc_text",
        json!({"document_id": id, "end_of_segment": true, "text": "\nSecond line"}),
    )
    .await;
    let r = ok(
        &app,
        "find_and_replace_doc",
        json!({"document_id": id, "find_text": "world", "replace_text": "there"}),
    )
    .await;
    assert!(
        r.starts_with("Replaced 1 occurrence(s) of 'world' with 'there'"),
        "{r}"
    );
    let doc = ok(&app, "get_doc_content", json!({"document_id": id})).await;
    assert!(
        doc.ends_with(
            "--- CONTENT ---\n\n--- TAB: Tab 1 (ID: t.0) ---\nHello there\nSecond line\n"
        ),
        "{doc:?}"
    );
    let c = ok(
        &app,
        "manage_document_comment",
        json!({"document_id": id, "action": "create", "comment_content": "Looks good"}),
    )
    .await;
    assert!(c.starts_with("Comment created successfully!"), "{c}");
    let l = ok(&app, "list_document_comments", json!({"document_id": id})).await;
    assert!(l.contains("Content: Looks good"), "{l}");

    let lists = ok(&app, "list_task_lists", json!({})).await;
    assert!(
        lists.contains("- Ops (ID: MTc0OTk1NDYxNjk2NzA5)"),
        "{lists}"
    );
    let t = ok(&app, "manage_task", json!({"action": "create", "task_list_id": OPS_LIST, "title": "Pay Northwood", "due": "2026-11-25T00:00:00Z"})).await;
    assert!(t.contains("- Due Date: 2026-11-25T00:00:00.000Z"), "{t}");
    let tasks = ok(
        &app,
        "list_tasks",
        json!({"task_list_id": OPS_LIST, "show_completed": false}),
    )
    .await;
    assert!(
        tasks.contains("Pay Northwood") && !tasks.contains("Renew SSL"),
        "{tasks}"
    );
    let e = err(
        &app,
        "manage_task",
        json!({"action": "update", "task_list_id": OPS_LIST, "task_id": "x", "status": "done"}),
    )
    .await;
    assert!(e.contains("invalid status"), "{e}");

    let found = ok(&app, "search_contacts", json!({"query": "Dana"})).await;
    assert!(
        found.starts_with("Search Results for 'Dana' (2 found):"),
        "{found}"
    );
    assert!(
        found.contains("Email: dana.whitfield@contoso.example (work)"),
        "{found}"
    );
}
