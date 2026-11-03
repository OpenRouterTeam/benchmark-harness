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
use mock_slack::Slack;
use serde_json::{Value, json};
use tower::ServiceExt;

fn world(slug: &str) -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../scenarios/collab/slack")
        .join(slug);
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

async fn raw(app: &Router, method: &str, uri: &str, ct: &str, body: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", ct)
        .header("authorization", "Bearer xoxp-toolbench")
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

/// Calls a tool; returns (is_error, parsed JSON text or the raw text as a string).
async fn tool(app: &Router, name: &str, args: Value) -> (bool, Value) {
    let (status, body) = raw(
        app,
        "POST",
        "/_mcp/tools/call",
        "application/json",
        &json!({"name": name, "arguments": args}).to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = body["content"][0]["text"].as_str().unwrap().to_owned();
    let is_error = body["isError"].as_bool().unwrap_or(false);
    let parsed = serde_json::from_str(&text).unwrap_or(Value::String(text));
    (is_error, parsed)
}

async fn ok(app: &Router, name: &str, args: Value) -> Value {
    let (is_error, v) = tool(app, name, args).await;
    assert!(!is_error, "{name} failed: {v}");
    v
}

async fn fails(app: &Router, name: &str, args: Value) -> String {
    let (is_error, v) = tool(app, name, args).await;
    assert!(is_error, "{name} should fail: {v}");
    v.as_str().unwrap().to_owned()
}

fn state(w: &SharedWorld) -> Value {
    w.lock().unwrap().snapshot()["services"]["slack"].clone()
}

#[tokio::test]
async fn search_public_formats_results_like_the_real_server() {
    let app = service_app(world("observability-decision"), &Slack);
    let v = ok(
        &app,
        "slack_search_public",
        json!({"keywords": ["\"going with\"", "Grafana"], "natural_language_query": "which vendor"}),
    )
    .await;
    let r = v["results"].as_str().unwrap();
    assert!(r.starts_with("# Search Results for: \n\n## Messages (1 results)\n### Result 1 of 1\nChannel: #infra (ID: C03INFRA9QX)\nFrom: Dana Whitfield <dana.whitfield@octo-corp.example> (ID: U01DWHIT5FA) \nTime: 2026-10-27 15:10:00 CDT\nMessage_ts: "), "{r}");
    assert!(r.contains("Thread_ts: 1791990131.120442"));
    assert!(r.contains("Permalink: [link](https://octo-corp.slack.com/archives/C03INFRA9QX/p"));
    assert!(r.contains("starts 2026-11-15"));
    assert_eq!(
        v["pagination_info"],
        "End of results - No more pages available.\\n"
    );

    // Private channels are invisible to the public search, visible to members otherwise.
    let v = ok(
        &app,
        "slack_search_public",
        json!({"keywords": ["Datadog", "leaning"]}),
    )
    .await;
    assert!(v["results"].as_str().unwrap().contains("No results found."));
    let v = ok(
        &app,
        "slack_search_public_and_private",
        json!({"keywords": ["Datadog", "leaning"]}),
    )
    .await;
    assert!(
        v["results"]
            .as_str()
            .unwrap()
            .contains("Channel: #eng-leads (ID: G07LEADS3PV)")
    );
    // ...but never for channels the caller is not in.
    let v = ok(
        &app,
        "slack_search_public_and_private",
        json!({"keywords": ["offsite"]}),
    )
    .await;
    assert!(v["results"].as_str().unwrap().contains("No results found."));
}

#[tokio::test]
async fn search_filters_sorting_and_pagination() {
    let app = service_app(world("observability-decision"), &Slack);
    let v = ok(&app, "slack_search_public", json!({"filters": "in:#infra from:<@U05SORTIZ6M>", "sort": "timestamp", "include_context": false})).await;
    let r = v["results"].as_str().unwrap();
    assert!(r.contains("## Messages (3 results)"), "{r}");
    assert!(
        r.find("PagerDuty").unwrap() < r.find("Great - I'll start").unwrap()
            || r.contains("Result 1 of 3\nChannel: #infra")
    );
    let v = ok(
        &app,
        "slack_search_public",
        json!({"filters": "in:#infra after:2026-10-26"}),
    )
    .await;
    assert!(
        v["results"]
            .as_str()
            .unwrap()
            .contains("## Messages (2 results)")
    );
    let v = ok(
        &app,
        "slack_search_public",
        json!({"filters": "in:<#C03INFRA9QX>", "limit": 2}),
    )
    .await;
    assert_eq!(
        v["pagination_info"],
        "For the next page of results use cursor `Q1VSUkVOVF9QQUdFOjI=`\n"
    );
    let v2 = ok(
        &app,
        "slack_search_public",
        json!({"filters": "in:<#C03INFRA9QX>", "limit": 2, "cursor": "Q1VSUkVOVF9QQUdFOjI="}),
    )
    .await;
    assert_ne!(v["results"], v2["results"]);
    let v = ok(
        &app,
        "slack_search_public",
        json!({"keywords": ["scorecard"], "content_types": "files"}),
    )
    .await;
    assert!(v["results"].as_str().unwrap().contains("## Files (1 results)\n### Result 1 of 1\nFile: Observability vendor scorecard (ID: F0OBSCANV01)"));
    let e = fails(&app, "slack_search_public", json!({})).await;
    assert!(e.contains("invalid_arguments"));
}

#[tokio::test]
async fn channel_user_and_emoji_search() {
    let app = service_app(world("release-date-dm-designer"), &Slack);
    let v = ok(
        &app,
        "slack_search_channels",
        json!({"keywords": ["release"], "natural_language_query": "release"}),
    )
    .await;
    let r = v["results"].as_str().unwrap();
    assert!(r.contains("## Channels (1 results)\n### Result 1 of 1\nName: #release\nCreator: Alex Rivera (<@U04ARIVERA9)\n"), "{r}");
    assert!(r.contains("Is Archived: false\nChannel Type: public_channel\n"));
    let v = ok(
        &app,
        "slack_search_channels",
        json!({"keywords": ["release"], "include_archived": true}),
    )
    .await;
    assert!(
        v["results"]
            .as_str()
            .unwrap()
            .contains("## Channels (2 results)")
    );

    let v = ok(
        &app,
        "slack_search_users",
        json!({"keywords": ["Alex"], "natural_language_query": "alex"}),
    )
    .await;
    let r = v["results"].as_str().unwrap();
    assert!(r.contains("## Users (2 results)"));
    assert!(r.contains("Name: Alex Kim\nUser ID: U06AKIM3D2Q\n"));
    let v = ok(
        &app,
        "slack_search_users",
        json!({"keywords": ["Alex", "design"]}),
    )
    .await;
    assert!(
        v["results"]
            .as_str()
            .unwrap()
            .contains("## Users (1 results)")
    );
    let v = ok(
        &app,
        "slack_search_users",
        json!({"keywords": ["nobody-here"]}),
    )
    .await;
    assert_eq!(
        v["results"],
        "# Search Results for: \n\nNo results found.\n"
    );

    let v = ok(&app, "slack_search_emojis", json!({"query": "ship,parrot"})).await;
    let r = v["result"].as_str().unwrap();
    assert!(r.contains(":shipit: (alias for :squirrel:)") && r.contains(":partyparrot:"));
}

#[tokio::test]
async fn profiles() {
    let app = service_app(world("release-date-dm-designer"), &Slack);
    let v = ok(&app, "slack_read_user_profile", json!({})).await;
    assert_eq!(
        v["result"],
        "User ID: U0TOOLBNCH1\nUsername: casey.morgan\nDisplay Name: casey\nReal Name: Casey Morgan\nPronouns: \nTitle: Engineering Operations Lead\nEmail: casey.morgan@octo-corp.example\nOrganization Name: Octo Corp\nPhone: \nStatus: :coffee: Heads down until noon \nTimezone: America/Chicago\nAdmin: No\nOwner: No\nBot: No\nRestricted: No\n"
    );
    let v = ok(
        &app,
        "slack_read_user_profile",
        json!({"user_id": "U06AKIM3D2Q", "response_format": "concise"}),
    )
    .await;
    assert!(
        v["result"]
            .as_str()
            .unwrap()
            .contains("Real Name: Alex Kim")
    );
    assert_eq!(
        fails(&app, "slack_read_user_profile", json!({"user_id": "U000"})).await,
        "Error: user_not_found"
    );
}

#[tokio::test]
async fn read_channel_and_thread() {
    let app = service_app(world("cutover-thread-summary"), &Slack);
    let v = ok(
        &app,
        "slack_read_channel",
        json!({"channel_id": "C05DBMIG2RT", "limit": 2}),
    )
    .await;
    let m = v["messages"].as_str().unwrap();
    assert!(m.starts_with("Channel: #db-migration (C05DBMIG2RT)\n\n=== Message from priya <priya.natarajan@octo-corp.example> (U03PRIYAN4T) at 2026-10-26 11:00:00 CDT === \nMessage TS: "), "{m}");
    assert!(m.contains("Thread: 4 replies (latest: 2026-10-22 10:41:00 CDT)"));
    let info = v["pagination_info"].as_str().unwrap();
    let cursor = info.split('`').nth(1).unwrap();
    let v = ok(
        &app,
        "slack_read_channel",
        json!({"channel_id": "C05DBMIG2RT", "limit": 2, "cursor": cursor}),
    )
    .await;
    assert!(
        v["messages"]
            .as_str()
            .unwrap()
            .contains("Staging cutover done")
    );
    assert_eq!(
        v["pagination_info"],
        "There are no more messages available.\n"
    );

    let v = ok(
        &app,
        "slack_read_thread",
        json!({"channel_id": "C05DBMIG2RT", "message_ts": "1792600205.318801"}),
    )
    .await;
    let m = v["messages"].as_str().unwrap();
    assert!(m.starts_with("=== THREAD PARENT MESSAGE ===\nFrom: sam <sam.ortiz@octo-corp.example> (U05SORTIZ6M)\nTime: 2026-10-21 11:30:05 CDT\nMessage TS: 1792600205.318801\n"));
    assert!(m.contains("=== THREAD REPLIES (4 total) ===\n\n--- Reply 1 of 4 ---\nFrom: lena"));
    assert!(m.contains("Reactions: +1 (3)"));
    assert_eq!(
        v["pagination_info"],
        "There are no more messages in this thread.\n"
    );

    assert_eq!(
        fails(
            &app,
            "slack_read_channel",
            json!({"channel_id": "G08EXECOFF1"})
        )
        .await,
        "Error: channel_not_found"
    );
    assert_eq!(
        fails(
            &app,
            "slack_read_thread",
            json!({"channel_id": "C05DBMIG2RT", "message_ts": "1.000001"})
        )
        .await,
        "Error: thread_not_found"
    );
    // DM history by user ID.
    let app = service_app(world("release-date-dm-designer"), &Slack);
    let v = ok(
        &app,
        "slack_read_channel",
        json!({"channel_id": "U06AKIM3D2Q"}),
    )
    .await;
    assert!(
        v["messages"]
            .as_str()
            .unwrap()
            .starts_with("Channel: DM with alexk (D09KIMDM4X1)")
    );
}

#[tokio::test]
async fn sending_messages_threads_and_dms() {
    let w = world("cutover-thread-summary");
    let app = service_app(w.clone(), &Slack);
    let v = ok(&app, "slack_send_message", json!({"channel_id": "C05DBMIG2RT", "thread_ts": "1792600205.318801", "message": "Final: 2026-11-07 22:00"})).await;
    let r = v["result"].as_str().unwrap();
    assert!(
        r.starts_with(
            "Message sent successfully to #db-migration (ID: C05DBMIG2RT).\nMessage_ts: 1793286007."
        ),
        "{r}"
    );
    assert!(
        v["message_link"]
            .as_str()
            .unwrap()
            .contains("?thread_ts=1792600205.318801&cid=C05DBMIG2RT")
    );
    let s = state(&w);
    assert_eq!(
        s["posted"]["C05DBMIG2RT#1"]["thread_ts"],
        "1792600205.318801"
    );
    assert_eq!(s["posted"]["C05DBMIG2RT#1"]["user"], "U0TOOLBNCH1");
    let ts = s["posted"]["C05DBMIG2RT#1"]["ts"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(s["messages"][format!("C05DBMIG2RT/{ts}")].is_object());
    let v = ok(
        &app,
        "slack_read_thread",
        json!({"channel_id": "C05DBMIG2RT", "message_ts": "1792600205.318801"}),
    )
    .await;
    assert!(v["messages"].as_str().unwrap().contains("(5 total)"));

    assert_eq!(
        fails(
            &app,
            "slack_send_message",
            json!({"channel_id": "C04INCID7NS", "message": "x", "thread_ts": "9.9"})
        )
        .await,
        "Error: thread_not_found"
    );
    assert_eq!(
        fails(
            &app,
            "slack_send_message",
            json!({"channel_id": "C07DESIGN8Y", "message": "x"})
        )
        .await,
        "Error: not_in_channel"
    );
    assert_eq!(
        fails(
            &app,
            "slack_send_message",
            json!({"channel_id": "G08EXECOFF1", "message": "x"})
        )
        .await,
        "Error: channel_not_found"
    );
    assert_eq!(
        fails(
            &app,
            "slack_send_message",
            json!({"channel_id": "C02RELARCH5", "message": "x"})
        )
        .await,
        "Error: is_archived"
    );
    assert!(
        fails(
            &app,
            "slack_send_message",
            json!({"channel_id": "C05DBMIG2RT"})
        )
        .await
        .contains("invalid_arguments")
    );

    // A user ID opens a DM.
    ok(
        &app,
        "slack_send_message",
        json!({"channel_id": "U07LFISCH2R", "message": "hi"}),
    )
    .await;
    let s = state(&w);
    let dm = s["channels"]
        .as_object()
        .unwrap()
        .values()
        .find(|c| c["user"] == "U07LFISCH2R")
        .unwrap();
    assert_eq!(dm["is_im"], true);
    assert_eq!(
        s["posted"][format!("{}#1", dm["id"].as_str().unwrap())]["text"],
        "hi"
    );
}

#[tokio::test]
async fn drafts_and_scheduled_messages() {
    let w = world("cutover-thread-summary");
    let app = service_app(w.clone(), &Slack);
    let v = ok(
        &app,
        "slack_send_message_draft",
        json!({"channel_id": "C05DBMIG2RT", "message": "draft"}),
    )
    .await;
    assert_eq!(
        v["channel_link"],
        "https://app.slack.com/client/T0OCTOCORP/C05DBMIG2RT"
    );
    assert_eq!(
        fails(
            &app,
            "slack_send_message_draft",
            json!({"channel_id": "C05DBMIG2RT", "message": "again"})
        )
        .await,
        "Error: attached_draft_exists"
    );
    let draft = v["draft_id"].as_str().unwrap().to_owned();
    ok(
        &app,
        "slack_send_message",
        json!({"channel_id": "C05DBMIG2RT", "message": "sent", "draft_id": draft}),
    )
    .await;
    assert!(state(&w)["drafts"].as_object().unwrap().is_empty());

    assert_eq!(
        fails(
            &app,
            "slack_schedule_message",
            json!({"channel_id": "C05DBMIG2RT", "message": "x", "post_at": 1_793_286_000})
        )
        .await,
        "Error: time_in_past"
    );
    assert_eq!(
        fails(
            &app,
            "slack_schedule_message",
            json!({"channel_id": "C05DBMIG2RT", "message": "x", "post_at": 1_893_286_000})
        )
        .await,
        "Error: time_too_far"
    );
    let v = ok(&app, "slack_schedule_message", json!({"channel_id": "C05DBMIG2RT", "message": "Cutover starts in 1h", "post_at": 1_793_581_200})).await;
    assert!(
        v["result"]
            .as_str()
            .unwrap()
            .contains("Post_at: 1793581200 (2026-11-01 19:00:00 CST)"),
        "{v}"
    );
    let s = state(&w);
    let sched = s["scheduled_messages"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap();
    assert_eq!(sched["text"], "Cutover starts in 1h");
}

#[tokio::test]
async fn conversations_members_and_channel_lists() {
    let w = world("release-date-dm-designer");
    let app = service_app(w.clone(), &Slack);
    assert_eq!(
        fails(
            &app,
            "slack_create_conversation",
            json!({"channel_name": "Release"})
        )
        .await,
        "Error: name_taken"
    );
    let v = ok(
        &app,
        "slack_create_conversation",
        json!({"channel_name": "Web 4.12 Launch", "user_ids": ["U06AKIM3D2Q"]}),
    )
    .await;
    let id = v["channel_id"].as_str().unwrap().to_owned();
    let s = state(&w);
    assert_eq!(s["channels"][&id]["name"], "web-4-12-launch");
    assert_eq!(
        s["channels"][&id]["members"],
        json!(["U0TOOLBNCH1", "U06AKIM3D2Q"])
    );
    let v = ok(
        &app,
        "slack_create_conversation",
        json!({"user_ids": ["U06AKIM3D2Q"]}),
    )
    .await;
    assert_eq!(v["channel_id"], "D09KIMDM4X1");
    assert_eq!(
        fails(
            &app,
            "slack_create_conversation",
            json!({"user_ids": ["U08DEPLOYB0"]})
        )
        .await,
        "Error: cannot_dm_bot"
    );
    assert!(
        fails(
            &app,
            "slack_create_conversation",
            json!({"user_ids": ["UNOPE"]})
        )
        .await
        .starts_with("Error: user_not_found")
    );

    let v = ok(
        &app,
        "slack_list_channel_members",
        json!({"channel_id": "C06RELEASE1", "response_format": "concise"}),
    )
    .await;
    let m = v["members"].as_str().unwrap();
    assert!(m.contains("@alex.kim (alexk) - U06AKIM3D2Q") && !m.contains("deploybot"));
    let v = ok(
        &app,
        "slack_list_channel_members",
        json!({"channel_id": "C06RELEASE1", "response_format": "ids_only", "limit": 2}),
    )
    .await;
    assert!(v["pagination_info"].as_str().unwrap().contains("cursor"));

    let v = ok(
        &app,
        "slack_list_user_channels",
        json!({"types": "public_channel,private_channel,im", "exclude_archived": true}),
    )
    .await;
    let c = v["channels"].as_str().unwrap();
    assert!(c.contains("- #eng-leads (ID: G07LEADS3PV) | Type: private_channel"));
    assert!(c.contains("- DM with alexk (ID: D09KIMDM4X1) | Type: im"));
    assert!(!c.contains("releases-archive") && !c.contains("exec-offsite"));
    let v = ok(
        &app,
        "slack_list_user_channels",
        json!({"name_prefix": "REL", "format": "names_only"}),
    )
    .await;
    assert_eq!(
        v["channels"],
        "Found 2 channels\n\nrelease\nreleases-archive\n"
    );
}

#[tokio::test]
async fn reactions() {
    let w = world("release-date-dm-designer");
    let app = service_app(w.clone(), &Slack);
    let ts = "1793110360.774310";
    ok(
        &app,
        "slack_add_reaction",
        json!({"channel_id": "C06RELEASE1", "message_ts": ts, "emoji": ":eyes:"}),
    )
    .await;
    ok(
        &app,
        "slack_add_reaction",
        json!({"channel_id": "C06RELEASE1", "message_ts": ts, "emoji": "eyes"}),
    )
    .await;
    ok(
        &app,
        "slack_add_reaction",
        json!({"channel_id": "C06RELEASE1", "message_ts": ts, "emoji": "shipit"}),
    )
    .await;
    let v = ok(
        &app,
        "slack_get_reactions",
        json!({"channel_id": "C06RELEASE1", "message_ts": ts}),
    )
    .await;
    let r = v["result"].as_str().unwrap();
    assert!(
        r.contains("- :eyes: (3) - priya (U03PRIYAN4T), lena (U07LFISCH2R), casey (U0TOOLBNCH1)"),
        "{r}"
    );
    assert!(r.contains("- :shipit: (1)"));
    assert_eq!(
        fails(
            &app,
            "slack_add_reaction",
            json!({"channel_id": "C06RELEASE1", "message_ts": ts, "emoji": "not-an-emoji"})
        )
        .await,
        "Error: invalid_name"
    );
    assert_eq!(
        fails(
            &app,
            "slack_get_reactions",
            json!({"channel_id": "C06RELEASE1", "message_ts": "1.2"})
        )
        .await,
        "Error: message_not_found"
    );
    let v = ok(
        &app,
        "slack_get_reactions",
        json!({"channel_id": "C06RELEASE1", "message_ts": "1792501200.002222"}),
    )
    .await;
    assert!(v["result"].as_str().unwrap().starts_with("No reactions"));
    let s = state(&w);
    assert_eq!(
        s["messages"][format!("C06RELEASE1/{ts}")]["reactions"][0]["count"],
        3
    );
}

#[tokio::test]
async fn canvases() {
    let w = world("observability-decision");
    let app = service_app(w.clone(), &Slack);
    let v = ok(
        &app,
        "slack_read_canvas",
        json!({"canvas_id": "F0OBSCANV01"}),
    )
    .await;
    assert!(
        v["markdown_content"]
            .as_str()
            .unwrap()
            .starts_with("# Observability vendor scorecard\n\n## Criteria\n\n- Metrics")
    );
    let mapping = v["section_id_mapping"].as_object().unwrap();
    assert_eq!(mapping.len(), 6);
    let scores = mapping
        .iter()
        .find(|(_, v)| *v == "## Scores")
        .unwrap()
        .0
        .clone();
    ok(&app, "slack_update_canvas", json!({"canvas_id": "F0OBSCANV01", "sections": [{"edit_type": "append", "section_id": scores, "content": "Updated after final quotes."}]})).await;
    let v = ok(
        &app,
        "slack_read_canvas",
        json!({"canvas_id": "F0OBSCANV01"}),
    )
    .await;
    assert!(
        v["markdown_content"]
            .as_str()
            .unwrap()
            .contains("## Scores\n\nUpdated after final quotes.\n\n| Vendor")
    );
    assert!(fails(&app, "slack_update_canvas", json!({"canvas_id": "F0OBSCANV01", "sections": [{"edit_type": "replace", "section_id": "temp:C:nope", "content": "x"}]})).await.starts_with("Error: invalid_section_id"));

    let v = ok(
        &app,
        "slack_create_canvas",
        json!({"title": "Grafana migration plan", "content": "# Phases\n1. Metrics\n2. Logs"}),
    )
    .await;
    let id = v["canvas_id"].as_str().unwrap().to_owned();
    assert!(
        v["canvas_url"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/docs/T0OCTOCORP/{id}"))
    );
    ok(
        &app,
        "slack_update_canvas",
        json!({"canvas_id": id, "action": "append", "content": "## Owners\nSam"}),
    )
    .await;
    let v = ok(&app, "slack_read_file", json!({"file_id": id})).await;
    assert!(v["content"].as_str().unwrap().contains(
        "# Grafana migration plan\n\n# Phases\n\n1. Metrics\n2. Logs\n\n## Owners\n\nSam"
    ));
    assert!(state(&w)["canvases"][&id]["sections"].is_array());
    assert_eq!(
        fails(&app, "slack_read_canvas", json!({"canvas_id": "F0NOPE"})).await,
        "Error: canvas_not_found"
    );
    assert!(
        fails(&app, "slack_create_canvas", json!({"title": "x"}))
            .await
            .contains("content")
    );
}

#[tokio::test]
async fn files_read_and_upload_flow() {
    let w = world("cutover-thread-summary");
    let app = service_app(w.clone(), &Slack);
    let v = ok(&app, "slack_read_file", json!({"file_id": "F0MIGRUNBK1"})).await;
    assert_eq!(v["mimeType"], "text/markdown");
    assert!(
        v["content"]
            .as_str()
            .unwrap()
            .contains("Rollback deadline 00:30 CT")
    );
    assert_eq!(
        fails(&app, "slack_read_file", json!({"file_id": "F0NOPE"})).await,
        "Error: file_not_found"
    );

    let v = ok(
        &app,
        "slack_get_file_upload_url",
        json!({"filename": "notes.txt", "content_length": 5}),
    )
    .await;
    let id = v["file_id"].as_str().unwrap().to_owned();
    let url = v["upload_url"].as_str().unwrap().to_owned();
    assert!(url.starts_with("https://files.slack.com/upload/v1/"));
    assert!(
        fails(&app, "slack_complete_file_upload", json!({"file_id": id}))
            .await
            .contains("never uploaded")
    );
    let token = url.rsplit('/').next().unwrap();
    let (status, _) = raw(
        &app,
        "POST",
        &format!("/upload/v1/{token}"),
        "text/plain",
        "hello",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v = ok(
        &app,
        "slack_complete_file_upload",
        json!({"file_id": id, "channel_id": "C05DBMIG2RT", "initial_comment": "notes"}),
    )
    .await;
    assert_eq!(v["title"], "notes.txt");
    let v = ok(
        &app,
        "slack_read_channel",
        json!({"channel_id": "C05DBMIG2RT", "limit": 1}),
    )
    .await;
    assert!(
        v["messages"]
            .as_str()
            .unwrap()
            .contains(&format!("Files: notes.txt (ID: {id}, text/plain, 5 B)"))
    );
    assert!(
        fails(
            &app,
            "slack_get_file_upload_url",
            json!({"filename": "a", "content_length": 0})
        )
        .await
        .contains("content_length")
    );
}

#[tokio::test]
async fn lists() {
    let w = world("launch-checklist-item");
    let app = service_app(w.clone(), &Slack);
    let v = ok(
        &app,
        "slack_read_list",
        json!({"list_title": "payments v2", "schema_only": true}),
    )
    .await;
    let r = v["result"].as_str().unwrap();
    assert!(r.contains("- Status (type: select, key: Col0STAT0003) options: Not started, In progress, Blocked, Done"), "{r}");
    let v = ok(
        &app,
        "slack_read_list",
        json!({"list_id": "F0PAYV2LST1", "limit": 2}),
    )
    .await;
    let r = v["result"].as_str().unwrap();
    assert!(r.contains("| Record ID | Task | Owner | Status | Due date |\n|---|---|---|---|---|\n| Rec0PV2A0001 | Load test checkout at 3x peak | sam (U05SORTIZ6M) | Done | 2026-10-23 |"), "{r}");
    let cursor = r.split("next_cursor: ").nth(1).unwrap().trim();
    let v = ok(
        &app,
        "slack_read_list",
        json!({"list_id": "F0PAYV2LST1", "cursor": cursor, "format": "csv"}),
    )
    .await;
    assert!(v["result"].as_str().unwrap().contains("Record ID,Task,Owner,Status,Due date\nRec0PV2A0003,Support macros for new refund flow,lena (U07LFISCH2R),Not started,2026-11-09\n"));

    let v = ok(&app, "slack_add_list_record", json!({"list_id": "F0PAYV2LST1", "columns": {"task": "Rotate webhook signing secrets", "Owner": "priya.natarajan@octo-corp.example", "STATUS": "In progress", "Col0DUED0004": "2026-11-05"}})).await;
    assert!(v["result"].as_str().unwrap().contains("Record ID: Rec0"));
    let s = state(&w);
    assert_eq!(
        s["lists"]["F0PAYV2LST1"]["records"][3]["fields"],
        json!({"Col0TASK0001": "Rotate webhook signing secrets", "Col0OWNR0002": ["U03PRIYAN4T"], "Col0STAT0003": ["in_progress"], "Col0DUED0004": "2026-11-05"})
    );
    assert_eq!(
        fails(
            &app,
            "slack_add_list_record",
            json!({"list_id": "F0PAYV2LST1", "columns": {"Nope": "x"}})
        )
        .await,
        "Error: column_names_not_found: Nope"
    );
    assert!(
        fails(
            &app,
            "slack_add_list_record",
            json!({"list_id": "F0PAYV2LST1", "columns": {"Task": "x", "Status": "Shipped"}})
        )
        .await
        .starts_with("Error: data_not_matching_schema: Status:")
    );
    assert_eq!(
        fails(
            &app,
            "slack_add_list_record",
            json!({"list_id": "F0NOPE", "columns": {"Task": "x"}})
        )
        .await,
        "Error: list_not_found"
    );
    assert_eq!(
        fails(
            &app,
            "slack_add_list_record",
            json!({"list_id": "bad", "columns": {"Task": "x"}})
        )
        .await,
        "Error: list_id_invalid"
    );

    ok(&app, "slack_update_list_record", json!({"list_id": "F0PAYV2LST1", "record_id": "Rec0PV2A0002", "updated_columns": {"Status": "Done"}})).await;
    assert_eq!(
        state(&w)["lists"]["F0PAYV2LST1"]["records"][1]["fields"]["Col0STAT0003"],
        json!(["done"])
    );
    assert_eq!(fails(&app, "slack_update_list_record", json!({"list_id": "F0PAYV2LST1", "record_id": "Rec0NOPE", "updated_columns": {"Status": "Done"}})).await, "Error: invalid_row_id");

    let v = ok(&app, "slack_update_list", json!({"list_id": "F0PAYV2LST1", "name": "Payments v2 GA checklist", "columns_to_add": [{"name": "Notes", "type": "rich_text"}], "columns_to_delete": [{"key": "Col0NOPE"}]})).await;
    let r = v["result"].as_str().unwrap();
    assert!(
        r.contains("- Added column \"Notes\"")
            && r.contains("- Failed to delete column Col0NOPE: column_not_found")
    );
    let v = ok(&app, "slack_create_list", json!({"name": "Bug bash", "columns": [{"name": "Severity", "type": "select", "options": ["High", "Low"]}]})).await;
    let r = v["result"].as_str().unwrap();
    assert!(
        r.contains("## List created: Bug bash")
            && r.contains("- Task (type: text")
            && r.contains("options: High, Low")
    );
    assert!(
        fails(
            &app,
            "slack_create_list",
            json!({"name": "x", "columns": [{"name": "S", "type": "select"}]})
        )
        .await
        .contains("invalid_columns")
    );
}

#[tokio::test]
async fn web_api_methods() {
    let w = world("cutover-thread-summary");
    let app = service_app(w.clone(), &Slack);
    let (_, v) = raw(
        &app,
        "POST",
        "/api/auth.test",
        "application/x-www-form-urlencoded",
        "",
    )
    .await;
    assert_eq!(v["user_id"], "U0TOOLBNCH1");
    assert_eq!(v["url"], "https://octo-corp.slack.com/");
    let (_, v) = raw(
        &app,
        "GET",
        "/api/conversations.history?channel=C05DBMIG2RT&limit=1",
        "",
        "",
    )
    .await;
    assert_eq!(v["ok"], true);
    assert_eq!(v["has_more"], true);
    assert_eq!(v["messages"][0]["user"], "U03PRIYAN4T");
    let (_, v) = raw(
        &app,
        "GET",
        "/api/conversations.replies?channel=C05DBMIG2RT&ts=1792600205.318801",
        "",
        "",
    )
    .await;
    assert_eq!(v["messages"][0]["reply_count"], 4);
    let (_, v) = raw(
        &app,
        "POST",
        "/api/chat.postMessage",
        "application/json",
        r#"{"channel":"C05DBMIG2RT","text":"via api"}"#,
    )
    .await;
    assert_eq!(v["ok"], true);
    assert_eq!(v["message"]["text"], "via api");
    let (_, v) = raw(
        &app,
        "POST",
        "/api/chat.postMessage",
        "application/x-www-form-urlencoded",
        "channel=C0NOPE&text=hi+there",
    )
    .await;
    assert_eq!(v, json!({"ok": false, "error": "channel_not_found"}));
    let (_, v) = raw(
        &app,
        "GET",
        "/api/users.lookupByEmail?email=sam.ortiz%40octo-corp.example",
        "",
        "",
    )
    .await;
    assert_eq!(v["user"]["id"], "U05SORTIZ6M");
    let (_, v) = raw(
        &app,
        "GET",
        "/api/search.messages?query=cutover%20in:%23db-migration",
        "",
        "",
    )
    .await;
    assert!(v["messages"]["total"].as_u64().unwrap() >= 2);
    let (_, v) = raw(&app, "GET", "/api/nope.method", "", "").await;
    assert_eq!(v["error"], "unknown_method");
    let (status, body) = raw(
        &app,
        "POST",
        "/_mcp/tools/call",
        "application/json",
        r#"{"name":"slack_nope","arguments":{}}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["isError"], true);
}
