mod support;

use axum::http::StatusCode;
use serde_json::{Value, json};
use support::{app, call, get, state, world};

const API: &str = "/projects/acme%2Fplatform%2Fapi";

fn v4(path: &str) -> String {
    format!("/api/v4{path}")
}

fn ids(v: &Value, field: &str) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|i| i[field].to_string().trim_matches('"').to_owned())
        .collect()
}

// ---------------------------------------------------------------------------
// Instance, users, projects, groups, namespaces

#[tokio::test]
async fn instance_user_and_unknown_routes() {
    let w = world("basic");
    let a = app(&w);
    let r = get(&a, &v4("/user")).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json["username"], "dana");
    assert_eq!(r.json["email"], "dana@acme.dev");
    let r = get(&a, &v4("/version")).await;
    assert_eq!(r.json["version"], "18.4.1-ee");
    let r = get(&a, &v4("/nope")).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.json, json!({"error": "404 Not Found"}));
    let r = get(&a, &v4("/users?username=raj")).await;
    assert_eq!(ids(&r.json, "username"), ["raj"]);
    let r = get(&a, &v4("/users/9")).await;
    assert_eq!(r.json["username"], "lena");
    let r = get(&a, &v4("/users/999")).await;
    assert_eq!(r.json, json!({"message": "404 User Not Found"}));
}

#[tokio::test]
async fn projects_by_path_or_id_and_404() {
    let w = world("basic");
    let a = app(&w);
    let by_path = get(&a, &v4(API)).await;
    let by_id = get(&a, &v4("/projects/101")).await;
    assert_eq!(by_path.json, by_id.json);
    assert_eq!(by_path.json["path_with_namespace"], "acme/platform/api");
    assert_eq!(by_path.json["namespace"]["full_path"], "acme/platform");
    assert_eq!(by_path.json["name_with_namespace"], "Acme / Platform / api");
    let r = get(&a, &v4("/projects/acme%2Fnope")).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.json, json!({"message": "404 Project Not Found"}));
}

#[tokio::test]
async fn project_lists_paginate_with_gitlab_headers() {
    let w = world("basic");
    let a = app(&w);
    let r = get(&a, &v4("/projects?per_page=2&order_by=id&sort=asc")).await;
    assert_eq!(ids(&r.json, "id"), ["101", "102"]);
    assert_eq!(r.headers["x-total"], "3");
    assert_eq!(r.headers["x-total-pages"], "2");
    assert_eq!(r.headers["x-next-page"], "2");
    assert!(r.headers["link"].to_str().unwrap().contains("rel=\"next\""));
    let r = get(&a, &v4("/projects?per_page=2&page=2&order_by=id&sort=asc")).await;
    assert_eq!(ids(&r.json, "id"), ["140"]);
    assert_eq!(r.headers["x-next-page"], "");
    let r = get(&a, &v4("/projects?search=web")).await;
    assert_eq!(ids(&r.json, "path_with_namespace"), ["acme/platform/web"]);
    let r = get(
        &a,
        &v4("/groups/acme/projects?include_subgroups=true&simple=true"),
    )
    .await;
    assert_eq!(r.json.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn create_update_fork_projects_and_groups() {
    let w = world("basic");
    let a = app(&w);
    let r = call(
        &a,
        "POST",
        &v4("/projects"),
        r#"{"name":"Scratch Pad","initialize_with_readme":true}"#,
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(r.json["path_with_namespace"], "dana/scratch-pad");
    let r = call(&a, "POST", &v4("/projects"), r#"{"name":"Scratch Pad"}"#).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = call(
        &a,
        "PUT",
        &v4(API),
        r#"{"description":"v2","topics":["rust"]}"#,
    )
    .await;
    assert_eq!(r.json["description"], "v2");
    let r = call(&a, "POST", &v4(&format!("{API}/fork")), "").await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(
        r.json["forked_from_project"]["path_with_namespace"],
        "acme/platform/api"
    );
    let r = call(
        &a,
        "POST",
        &v4("/groups"),
        r#"{"name":"Data","path":"data","parent_id":20}"#,
    )
    .await;
    assert_eq!(r.json["full_path"], "acme/data");
    let r = call(&a, "POST", &v4("/groups"), r#"{"name":"Data"}"#).await;
    assert_eq!(r.json, json!({"error": "path is missing"}));
    let s = state(&w);
    assert_eq!(
        s["projects"]["dana/api"]["forked_from"],
        "acme/platform/api"
    );
    assert_eq!(s["projects"]["acme/platform/api"]["description"], "v2");
    assert!(s["files"]["dana/scratch-pad:main:README.md"].is_object());
}

#[tokio::test]
async fn members_namespaces_events_search_todos() {
    let w = world("basic");
    let a = app(&w);
    let r = get(&a, &v4(&format!("{API}/members"))).await;
    assert_eq!(ids(&r.json, "username"), ["raj"]);
    let r = get(&a, &v4(&format!("{API}/members/all"))).await;
    assert_eq!(ids(&r.json, "username"), ["raj", "lena", "dana"]);
    let r = get(&a, &v4("/groups/acme/members")).await;
    assert_eq!(r.json[0]["access_level"], 50);
    let r = get(&a, &v4("/namespaces")).await;
    assert_eq!(ids(&r.json, "full_path"), ["dana", "acme", "acme/platform"]);
    let r = get(&a, &v4("/namespaces/nobody/exists")).await;
    assert_eq!(r.json["exists"], false);
    let r = get(&a, &v4("/namespaces/acme/exists")).await;
    assert_eq!(r.json["exists"], true);
    let r = get(&a, &v4(&format!("{API}/events"))).await;
    assert_eq!(r.json[0]["action_name"], "commented on");
    let r = get(&a, &v4("/search?scope=blobs&search=refill")).await;
    assert!(ids(&r.json, "path").contains(&"src/limiter.rs".to_owned()));
    let r = get(&a, &v4("/search?search=x")).await;
    assert_eq!(r.json, json!({"error": "scope is missing"}));
    let r = get(&a, &v4("/todos")).await;
    assert_eq!(r.json[0]["target_type"], "MergeRequest");
    let r = call(&a, "POST", &v4("/todos/1/mark_as_done"), "").await;
    assert_eq!(r.json["state"], "done");
    assert_eq!(state(&w)["todos"]["1"]["state"], "done");
}

// ---------------------------------------------------------------------------
// Issues, labels, milestones, links

#[tokio::test]
async fn issue_lists_filter_and_render() {
    let w = world("basic");
    let a = app(&w);
    let r = get(&a, &v4(&format!("{API}/issues?state=opened"))).await;
    assert_eq!(ids(&r.json, "iid"), ["3", "4", "1"], "newest first");
    let r = get(&a, &v4(&format!("{API}/issues?labels=bug,backend"))).await;
    assert_eq!(ids(&r.json, "iid"), ["1"]);
    assert_eq!(r.json[0]["milestone"]["title"], "v2.4");
    assert_eq!(r.json[0]["references"]["full"], "acme/platform/api#1");
    let r = get(&a, &v4("/issues?assignee_username[]=dana")).await;
    assert_eq!(ids(&r.json, "iid"), ["1"]);
    let r = get(&a, &v4(&format!("{API}/issues/99"))).await;
    assert_eq!(r.json, json!({"message": "404 Issue Not Found"}));
}

#[tokio::test]
async fn create_and_update_issue_reflect_in_state() {
    let w = world("basic");
    let a = app(&w);
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/issues")),
        r#"{"description":"x"}"#,
    )
    .await;
    assert_eq!(r.json, json!({"error": "title is missing"}));
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/issues")),
        r#"{"title":"T","labels":"bug,security","milestone_id":"401","assignee_ids":[9]}"#,
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(r.json["iid"], 5);
    assert_eq!(
        r.json["created_at"], "2026-03-14T09:00:14.000Z",
        "logical clock: two writes"
    );
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/issues")),
        r#"{"title":"T","milestone_id":"999"}"#,
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = call(
        &a,
        "PUT",
        &v4(&format!("{API}/issues/5")),
        r#"{"state_event":"close","add_labels":"backend"}"#,
    )
    .await;
    assert_eq!(r.json["state"], "closed");
    let s = state(&w);
    let i = &s["issues"]["acme/platform/api#5"];
    assert_eq!(i["labels"], json!(["bug", "security", "backend"]));
    assert_eq!(i["milestone_id"], 401);
    assert_eq!(i["assignees"], json!(["lena"]));
    assert_eq!(i["closed_by"], "dana");
    // `security` is the group label, not a new project label.
    assert!(s["labels"].get("acme/platform/api~security").is_none());
}

#[tokio::test]
async fn issue_links_and_delete_permissions() {
    let w = world("basic");
    let a = app(&w);
    let r = get(&a, &v4(&format!("{API}/issues/1/links"))).await;
    assert_eq!(r.json[0]["link_type"], "relates_to");
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/issues/2/links")),
        r#"{"target_project_id":"101","target_issue_iid":"1","link_type":"blocks"}"#,
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(r.json["source_issue"]["iid"], 2);
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/issues/2/links")),
        r#"{"target_project_id":"101","target_issue_iid":"1"}"#,
    )
    .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let r = get(&a, &v4(&format!("{API}/issues/1/links"))).await;
    assert!(
        r.json
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["link_type"] == "is_blocked_by")
    );
    let r = call(&a, "DELETE", &v4(&format!("{API}/issues/2")), "").await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(state(&w)["issues"].get("acme/platform/api#2").is_none());
}

#[tokio::test]
async fn labels_and_milestones() {
    let w = world("basic");
    let a = app(&w);
    let r = get(&a, &v4(&format!("{API}/labels?with_counts=true"))).await;
    assert_eq!(ids(&r.json, "name"), ["backend", "bug", "security"]);
    assert_eq!(r.json[1]["open_issues_count"], 1);
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/labels")),
        r#"{"name":"perf","color":"nope!"}"#,
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/labels")),
        r##"{"name":"perf","color":"#ff9900"}"##,
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let r = call(
        &a,
        "PUT",
        &v4(&format!("{API}/labels/bug")),
        r#"{"new_name":"defect"}"#,
    )
    .await;
    assert_eq!(r.json["name"], "defect");
    assert_eq!(
        state(&w)["issues"]["acme/platform/api#1"]["labels"],
        json!(["defect", "backend"])
    );
    let r = get(&a, &v4(&format!("{API}/milestones?state=active"))).await;
    assert_eq!(ids(&r.json, "title"), ["v2.4"]);
    let r = get(&a, &v4(&format!("{API}/milestones/401/issues"))).await;
    assert_eq!(ids(&r.json, "iid"), ["1"]);
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/milestones")),
        r#"{"title":"v2.4"}"#,
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = call(
        &a,
        "PUT",
        &v4(&format!("{API}/milestones/401")),
        r#"{"state_event":"close"}"#,
    )
    .await;
    assert_eq!(r.json["state"], "closed");
    let r = get(&a, &v4("/groups/acme/milestones")).await;
    assert_eq!(ids(&r.json, "group_id"), ["20"]);
    let r = get(&a, &v4(&format!("{API}/milestones/450"))).await;
    assert_eq!(r.json, json!({"message": "404 Milestone Not Found"}));
}

// ---------------------------------------------------------------------------
// Merge requests, notes, discussions, emoji, drafts

#[tokio::test]
async fn merge_request_get_list_changes_and_status() {
    let w = world("basic");
    let a = app(&w);
    let r = get(&a, &v4(&format!("{API}/merge_requests/7"))).await;
    assert_eq!(r.json["detailed_merge_status"], "not_approved");
    assert_eq!(r.json["head_pipeline"]["id"], 602);
    assert_eq!(
        r.json["diff_refs"]["head_sha"],
        "9f1c2e7a4b5d6c8e0f1a2b3c4d5e6f708192a3b4"
    );
    let r = get(&a, &v4(&format!("{API}/merge_requests?state=opened"))).await;
    assert_eq!(ids(&r.json, "iid"), ["7"]);
    let r = get(
        &a,
        &v4(&format!("{API}/merge_requests?source_branch=deps%2Fserde")),
    )
    .await;
    assert_eq!(ids(&r.json, "iid"), ["6"]);
    let r = get(&a, &v4(&format!("{API}/merge_requests/7/changes"))).await;
    assert_eq!(r.json["changes"][0]["new_path"], "src/limiter.rs");
    let r = get(&a, &v4(&format!("{API}/merge_requests/7/diffs"))).await;
    assert_eq!(r.json[0]["collapsed"], false);
    let r = get(&a, &v4(&format!("{API}/merge_requests/7/commits"))).await;
    assert_eq!(r.json[0]["title"], "Clamp refill to capacity");
    let r = get(&a, &v4(&format!("{API}/merge_requests/7/versions"))).await;
    let vid = r.json[0]["id"].as_u64().unwrap();
    let r = get(&a, &v4(&format!("{API}/merge_requests/7/versions/{vid}"))).await;
    assert_eq!(r.json["diffs"].as_array().unwrap().len(), 1);
    let r = get(&a, &v4(&format!("{API}/merge_requests/7/pipelines"))).await;
    assert_eq!(ids(&r.json, "id"), ["602"]);
    let r = get(&a, &v4(&format!("{API}/merge_requests/70"))).await;
    assert_eq!(r.json, json!({"message": "404 Merge Request Not Found"}));
}

#[tokio::test]
async fn merge_requires_approval_then_merges_and_closes_issue() {
    let w = world("basic");
    let a = app(&w);
    let r = call(
        &a,
        "PUT",
        &v4(&format!("{API}/merge_requests/7/merge")),
        "{}",
    )
    .await;
    assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED);
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/merge_requests/7/approve")),
        r#"{"sha":"bad"}"#,
    )
    .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/merge_requests/7/approve")),
        "{}",
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(r.json["approved"], true);
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/merge_requests/7/approve")),
        "{}",
    )
    .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = get(&a, &v4(&format!("{API}/merge_requests/7/approval_state"))).await;
    assert_eq!(r.json["rules"][0]["approved"], true);
    let r = call(
        &a,
        "PUT",
        &v4(&format!("{API}/merge_requests/7/merge")),
        r#"{"should_remove_source_branch":true}"#,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json["state"], "merged");
    let s = state(&w);
    assert_eq!(
        s["merge_requests"]["acme/platform/api!7"]["merged_by"],
        "dana"
    );
    assert!(
        s["branches"]
            .get("acme/platform/api:fix/rate-limiter")
            .is_none()
    );
    assert_eq!(s["issues"]["acme/platform/api#1"]["state"], "closed");
    // The source branch's change landed on main.
    let f = get(
        &a,
        &v4(&format!(
            "{API}/repository/files/src%2Flimiter.rs/raw?ref=main"
        )),
    )
    .await;
    assert!(f.text.contains(".min(bucket.capacity)"));
}

#[tokio::test]
async fn create_and_update_merge_requests() {
    let w = world("basic");
    let a = app(&w);
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/merge_requests")),
        r#"{"source_branch":"nope","target_branch":"main","title":"x"}"#,
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/merge_requests")),
        r#"{"source_branch":"fix/rate-limiter","target_branch":"main","title":"dup"}"#,
    )
    .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    call(
        &a,
        "POST",
        &v4(&format!("{API}/repository/branches")),
        r#"{"branch":"feat/x","ref":"main"}"#,
    )
    .await;
    let r = call(&a, "POST", &v4(&format!("{API}/merge_requests")), r#"{"source_branch":"feat/x","target_branch":"main","title":"X","labels":"backend","draft":true}"#).await;
    assert_eq!(r.json["iid"], 8);
    assert_eq!(r.json["title"], "Draft: X");
    let r = call(
        &a,
        "PUT",
        &v4(&format!("{API}/merge_requests/8")),
        r#"{"draft":false,"reviewer_ids":[8]}"#,
    )
    .await;
    assert_eq!(r.json["title"], "X");
    assert_eq!(r.json["reviewers"][0]["username"], "raj");
    let r = call(
        &a,
        "PUT",
        &v4(&format!("{API}/merge_requests/8")),
        r#"{"state_event":"close"}"#,
    )
    .await;
    assert_eq!(r.json["state"], "closed");
    assert_eq!(
        state(&w)["merge_requests"]["acme/platform/api!8"]["state"],
        "closed"
    );
}

#[tokio::test]
async fn notes_discussions_and_resolution() {
    let w = world("basic");
    let a = app(&w);
    let mr = format!("{API}/merge_requests/7");
    let r = get(&a, &v4(&format!("{mr}/discussions"))).await;
    assert_eq!(r.json[0]["notes"].as_array().unwrap().len(), 2);
    assert_eq!(r.json[0]["individual_note"], false);
    let r = call(&a, "POST", &v4(&format!("{mr}/notes")), "{}").await;
    assert_eq!(r.json, json!({"error": "body is missing"}));
    let r = call(
        &a,
        "POST",
        &v4(&format!("{mr}/notes")),
        r#"{"body":"LGTM"}"#,
    )
    .await;
    assert_eq!(r.json["id"], 9004);
    let did = "6a9c1750b37d513a43987b574953fceb50b03ce7";
    let r = call(
        &a,
        "POST",
        &v4(&format!("{mr}/discussions/{did}/notes")),
        r#"{"body":"done"}"#,
    )
    .await;
    assert_eq!(r.json["type"], "DiscussionNote");
    let r = call(
        &a,
        "PUT",
        &v4(&format!("{mr}/discussions/{did}?resolved=true")),
        "",
    )
    .await;
    assert!(
        r.json["notes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["resolved"] == true)
    );
    let r = call(&a, "POST", &v4(&format!("{mr}/discussions")), r#"{"body":"thread","position":{"new_path":"src/limiter.rs","new_line":2,"position_type":"text"}}"#).await;
    assert_eq!(r.json["notes"][0]["type"], "DiffNote");
    let r = get(&a, &v4(&format!("{API}/issues/1/notes?sort=asc"))).await;
    assert_eq!(r.json[0]["body"], "Reproduced on staging.");
    let s = state(&w);
    assert_eq!(s["notes"]["9001"]["resolved"], true);
    assert_eq!(s["notes"]["9005"]["discussion_id"], did);
}

#[tokio::test]
async fn award_emoji_and_draft_notes() {
    let w = world("basic");
    let a = app(&w);
    let mr = format!("{API}/merge_requests/7");
    let r = get(&a, &v4(&format!("{mr}/award_emoji"))).await;
    assert_eq!(ids(&r.json, "name"), ["thumbsup"]);
    let r = call(
        &a,
        "POST",
        &v4(&format!("{mr}/award_emoji")),
        r#"{"name":"rocket"}"#,
    )
    .await;
    assert_eq!(r.json["awardable_type"], "MergeRequest");
    let r = call(
        &a,
        "POST",
        &v4(&format!("{mr}/award_emoji")),
        r#"{"name":"rocket"}"#,
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = call(&a, "DELETE", &v4(&format!("{mr}/award_emoji/1")), "").await;
    assert_eq!(
        r.status,
        StatusCode::FORBIDDEN,
        "cannot delete another user's award"
    );
    let r = call(
        &a,
        "POST",
        &v4(&format!("{mr}/notes/9001/award_emoji")),
        r#"{"name":"eyes"}"#,
    )
    .await;
    assert_eq!(r.json["awardable_type"], "Note");
    let r = call(
        &a,
        "POST",
        &v4(&format!("{mr}/draft_notes")),
        r#"{"note":"draft"}"#,
    )
    .await;
    let draft = r.json["id"].as_u64().unwrap();
    let r = call(
        &a,
        "PUT",
        &v4(&format!("{mr}/draft_notes/{draft}/publish")),
        "",
    )
    .await;
    assert_eq!(r.json["body"], "draft");
    let r = call(
        &a,
        "POST",
        &v4(&format!("{mr}/draft_notes")),
        r#"{"note":"second"}"#,
    )
    .await;
    assert_ne!(
        r.json["id"].as_u64().unwrap(),
        draft,
        "ids are never reused"
    );
    let r = call(
        &a,
        "POST",
        &v4(&format!("{mr}/draft_notes/bulk_publish")),
        "{}",
    )
    .await;
    assert_eq!(r.json[0]["body"], "second");
    assert!(state(&w)["draft_notes"].as_object().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Repository

#[tokio::test]
async fn files_tree_and_refs() {
    let w = world("basic");
    let a = app(&w);
    let r = get(
        &a,
        &v4(&format!(
            "{API}/repository/files/src%2Flimiter.rs?ref=fix%2Frate-limiter"
        )),
    )
    .await;
    assert_eq!(r.json["encoding"], "base64");
    let content = mock_gitlab::common::b64_decode(r.json["content"].as_str().unwrap()).unwrap();
    assert!(String::from_utf8(content).unwrap().contains(".min("));
    // Unchanged files are inherited from the base branch.
    let r = get(
        &a,
        &v4(&format!(
            "{API}/repository/files/README.md?ref=fix%2Frate-limiter"
        )),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let r = get(&a, &v4(&format!("{API}/repository/files/README.md"))).await;
    assert_eq!(r.json, json!({"error": "ref is missing"}));
    let r = get(&a, &v4(&format!("{API}/repository/files/nope.md?ref=main"))).await;
    assert_eq!(r.json, json!({"message": "404 File Not Found"}));
    let r = get(&a, &v4(&format!("{API}/repository/tree"))).await;
    assert_eq!(ids(&r.json, "path"), ["src", ".gitlab-ci.yml", "README.md"]);
    let r = get(
        &a,
        &v4(&format!("{API}/repository/tree?recursive=true&ref=v2.3.0")),
    )
    .await;
    assert!(ids(&r.json, "path").contains(&"src/limiter.rs".to_owned()));
    let r = get(&a, &v4(&format!("{API}/repository/tree?path=nope"))).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn commits_branches_compare_and_writes() {
    let w = world("basic");
    let a = app(&w);
    let r = get(
        &a,
        &v4(&format!(
            "{API}/repository/compare?from=main&to=fix%2Frate-limiter"
        )),
    )
    .await;
    assert_eq!(r.json["commits"].as_array().unwrap().len(), 1);
    assert!(
        r.json["diffs"][0]["diff"]
            .as_str()
            .unwrap()
            .contains("+    bucket.tokens = (bucket.tokens")
    );
    let r = get(&a, &v4(&format!("{API}/repository/branches"))).await;
    assert_eq!(ids(&r.json, "name"), ["fix/rate-limiter", "main"]);
    assert_eq!(r.json[1]["protected"], true);
    let r = call(
        &a,
        "DELETE",
        &v4(&format!("{API}/repository/branches/main")),
        "",
    )
    .await;
    assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED);
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/repository/branches")),
        r#"{"branch":"feat/y","ref":"nope"}"#,
    )
    .await;
    assert_eq!(r.json, json!({"message": "Invalid reference name: nope"}));
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/repository/files/docs%2Fa.md")),
        r#"{"branch":"main","content":"hi","commit_message":"Add a"}"#,
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/repository/files/docs%2Fa.md")),
        r#"{"branch":"main","content":"hi","commit_message":"Add a"}"#,
    )
    .await;
    assert_eq!(
        r.json,
        json!({"message": "A file with this name already exists"})
    );
    let r = call(&a, "POST", &v4(&format!("{API}/repository/commits")), r#"{"branch":"main","commit_message":"Two","actions":[{"action":"update","file_path":"docs/a.md","content":"hello"},{"action":"delete","file_path":"README.md"}]}"#).await;
    assert_eq!(r.json["title"], "Two");
    let r = get(&a, &v4(&format!("{API}/repository/commits?ref_name=main"))).await;
    assert_eq!(
        ids(&r.json, "title"),
        ["Two", "Add a", "Add token bucket limiter"]
    );
    let r = get(
        &a,
        &v4(&format!("{API}/repository/files/README.md?ref=main")),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let s = state(&w);
    assert_eq!(
        s["files"]["acme/platform/api:main:docs/a.md"]["content"],
        "hello"
    );
}

// ---------------------------------------------------------------------------
// CI/CD

#[tokio::test]
async fn pipelines_jobs_and_traces() {
    let w = world("basic");
    let a = app(&w);
    let r = get(&a, &v4(&format!("{API}/pipelines?status=failed"))).await;
    assert_eq!(ids(&r.json, "id"), ["601"]);
    let r = get(&a, &v4(&format!("{API}/pipelines/601/jobs?scope[]=failed"))).await;
    assert_eq!(ids(&r.json, "name"), ["unit-tests"]);
    let r = get(&a, &v4(&format!("{API}/jobs/9102/trace"))).await;
    assert!(r.text.contains("assertion failed: tokens <= capacity"));
    let r = get(
        &a,
        &v4(&format!("{API}/jobs/9102/artifacts/reports/junit.xml")),
    )
    .await;
    assert!(r.text.contains("testsuite"));
    let r = get(&a, &v4(&format!("{API}/jobs/1"))).await;
    assert_eq!(r.json, json!({"message": "404 Job Not Found"}));
    let r = get(&a, &v4(&format!("{API}/pipelines/999"))).await;
    assert_eq!(r.json, json!({"message": "404 Pipeline Not Found"}));
}

#[tokio::test]
async fn create_retry_and_play_finish_deterministically() {
    let w = world("basic");
    let a = app(&w);
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/pipeline")),
        r#"{"ref":"nope"}"#,
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/pipeline")),
        r#"{"ref":"main"}"#,
    )
    .await;
    assert_eq!(r.json["status"], "created");
    let id = r.json["id"].as_u64().unwrap();
    let r = get(&a, &v4(&format!("{API}/pipelines/{id}"))).await;
    assert_eq!(r.json["status"], "running");
    let r = get(&a, &v4(&format!("{API}/pipelines/{id}"))).await;
    assert_eq!(r.json["status"], "success");
    let jobs = get(&a, &v4(&format!("{API}/pipelines/{id}/jobs"))).await;
    assert_eq!(ids(&jobs.json, "name"), ["build", "unit-tests"]);
    // Retrying the failed job creates a new job that passes (retry_result).
    let r = call(&a, "POST", &v4(&format!("{API}/jobs/9102/retry")), "").await;
    let new_job = r.json["id"].as_u64().unwrap();
    assert_eq!(r.json["status"], "pending");
    let r = get(&a, &v4(&format!("{API}/jobs/{new_job}"))).await;
    assert_eq!(r.json["status"], "success");
    let s = state(&w);
    assert_eq!(s["jobs"]["9102"]["retried"], true);
    let r = call(&a, "POST", &v4(&format!("{API}/jobs/9101/play")), "").await;
    assert_eq!(r.json, json!({"error": "Unplayable Job"}));
    let r = call(&a, "POST", &v4(&format!("{API}/jobs/9103/play")), "").await;
    assert_eq!(r.json["status"], "pending");
}

#[tokio::test]
async fn ci_variables_schedules_triggers_and_lint() {
    let w = world("basic");
    let a = app(&w);
    let r = get(&a, &v4(&format!("{API}/variables"))).await;
    assert_eq!(ids(&r.json, "key"), ["DATABASE_URL"]);
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/variables")),
        r#"{"key":"BAD-KEY","value":"x"}"#,
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/variables")),
        r#"{"key":"DATABASE_URL","value":"x"}"#,
    )
    .await;
    assert_eq!(
        r.json["message"]["key"][0],
        "(DATABASE_URL) has already been taken"
    );
    let r = call(
        &a,
        "POST",
        &v4("/groups/acme/variables"),
        r#"{"key":"TOKEN","value":"short","masked":true}"#,
    )
    .await;
    assert_eq!(r.json, json!({"message": {"value": ["is invalid"]}}));
    let r = get(&a, &v4(&format!("{API}/pipeline_schedules/61"))).await;
    assert_eq!(r.json["variables"][0]["key"], "NIGHTLY");
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/pipeline_schedules/61/play")),
        "",
    )
    .await;
    assert_eq!(r.json["message"], "201 Created");
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/trigger/pipeline?token=wrong&ref=main")),
        "",
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/ci/lint")),
        r#"{"content":"t:\n  stage: nope\n  script: x\n"}"#,
    )
    .await;
    assert_eq!(r.json["valid"], false);
    let r = get(&a, &v4(&format!("{API}/ci/lint"))).await;
    assert_eq!(r.json["valid"], true);
}

#[tokio::test]
async fn deployments_environments_runners_statuses() {
    let w = world("basic");
    let a = app(&w);
    let r = get(&a, &v4(&format!("{API}/deployments"))).await;
    assert_eq!(ids(&r.json, "id"), ["81", "82"]);
    let r = get(&a, &v4(&format!("{API}/deployments/81/merge_requests"))).await;
    assert_eq!(ids(&r.json, "iid"), ["6"]);
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/deployments/82/approval")),
        r#"{"status":"approved"}"#,
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(state(&w)["deployments"]["82"]["status"], "running");
    let r = call(&a, "DELETE", &v4(&format!("{API}/environments/71")), "").await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = get(&a, &v4(&format!("{API}/environments/71"))).await;
    assert_eq!(r.json["last_deployment"]["id"], 82);
    let r = get(&a, &v4(&format!("{API}/runners"))).await;
    assert_eq!(ids(&r.json, "id"), ["31", "32"]);
    let r = get(&a, &v4("/runners/32")).await;
    assert_eq!(
        r.json["projects"][0]["path_with_namespace"],
        "acme/platform/api"
    );
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/statuses/c0ffee00?state=failed&name=scan")),
        "",
    )
    .await;
    assert_eq!(r.json["status"], "failed");
    let r = get(
        &a,
        &v4(&format!(
            "{API}/repository/commits/c0ffee0000000000000000000000000000000001/statuses"
        )),
    )
    .await;
    assert!(ids(&r.json, "name").contains(&"scan".to_owned()));
}

// ---------------------------------------------------------------------------
// Releases, tags, snippets, hooks, wikis

#[tokio::test]
async fn releases_tags_and_protection() {
    let w = world("basic");
    let a = app(&w);
    let r = get(&a, &v4(&format!("{API}/releases"))).await;
    assert_eq!(r.json[0]["milestones"][0]["title"], "v2.3");
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/releases")),
        r#"{"tag_name":"v2.4.0"}"#,
    )
    .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/releases")),
        r#"{"tag_name":"v2.4.0","ref":"main","milestones":["v2.4"]}"#,
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let r = get(&a, &v4(&format!("{API}/repository/tags"))).await;
    assert_eq!(ids(&r.json, "name"), ["v2.4.0", "v2.3.0"]);
    let r = call(
        &a,
        "DELETE",
        &v4(&format!("{API}/repository/tags/v2.3.0")),
        "",
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "v* tags are protected");
    let r = get(&a, &v4(&format!("{API}/repository/tags/v2.3.0/signature"))).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let s = state(&w);
    assert_eq!(
        s["tags"]["acme/platform/api:v2.4.0"]["commit"],
        "c0ffee0000000000000000000000000000000001"
    );
    assert_eq!(
        s["releases"]["acme/platform/api@v2.4.0"]["milestones"],
        json!(["v2.4"])
    );
}

#[tokio::test]
async fn snippets_hooks_and_wikis() {
    let w = world("basic");
    let a = app(&w);
    let r = get(&a, &v4("/snippets")).await;
    assert_eq!(ids(&r.json, "id"), ["302"]);
    let r = get(&a, &v4(&format!("{API}/snippets/301/raw"))).await;
    assert!(r.text.contains("k6/http"));
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/hooks")),
        r#"{"url":"ftp://x"}"#,
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = get(
        &a,
        &v4(&format!("{API}/hooks/401/events?status=server_failure")),
    )
    .await;
    assert_eq!(ids(&r.json, "id"), ["4011"]);
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/wikis")),
        r#"{"title":"Deploys","content":"How"}"#,
    )
    .await;
    assert_eq!(r.json["slug"], "Deploys");
    let r = call(
        &a,
        "POST",
        &v4(&format!("{API}/wikis")),
        r#"{"title":"Deploys","content":"again"}"#,
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = get(&a, &v4("/groups/acme/wikis?with_content=1")).await;
    assert_eq!(r.json[0]["content"], "Welcome to Acme engineering.");
    assert_eq!(
        state(&w)["wikis"]["acme/platform/api/-/Deploys"]["content"],
        "How"
    );
}

// ---------------------------------------------------------------------------
// GraphQL

async fn gql(a: &axum::Router, query: &str, vars: Value) -> Value {
    call(
        a,
        "POST",
        "/api/graphql",
        &json!({"query": query, "variables": vars}).to_string(),
    )
    .await
    .json
}

#[tokio::test]
async fn graphql_work_items_and_errors() {
    let w = world("basic");
    let a = app(&w);
    let r = gql(&a, "query($p: ID!, $iid: String!) { namespace(fullPath: $p) { workItem(iid: $iid) { id title state workItemType { name } widgets { __typename ... on WorkItemWidgetLabels { labels { nodes { title } } } ... on WorkItemWidgetHierarchy { children { nodes { iid } } } } } } }", json!({"p": "acme/platform/api", "iid": "1"})).await;
    let wi = &r["data"]["namespace"]["workItem"];
    assert_eq!(wi["id"], "gid://gitlab/WorkItem/5001");
    assert_eq!(wi["state"], "OPEN");
    let widgets = wi["widgets"].as_array().unwrap();
    let labels = widgets
        .iter()
        .find(|w| w["__typename"] == "WorkItemWidgetLabels")
        .unwrap();
    assert_eq!(labels["labels"]["nodes"][0]["title"], "bug");
    let h = widgets
        .iter()
        .find(|w| w["__typename"] == "WorkItemWidgetHierarchy")
        .unwrap();
    assert_eq!(h["children"]["nodes"][0]["iid"], "4");
    let r = gql(
        &a,
        "{ project(fullPath: \"acme/platform/api\") { nope } }",
        json!({}),
    )
    .await;
    assert_eq!(
        r["errors"][0]["message"],
        "Field 'nope' doesn't exist on type 'Project'"
    );
    let r = gql(&a, "{ project(fullPath: \"acme/x\") { name } }", json!({})).await;
    assert_eq!(r["data"]["project"], Value::Null);
    let r = gql(&a, "query { project(", json!({})).await;
    assert!(
        r["errors"][0]["message"]
            .as_str()
            .unwrap()
            .starts_with("Syntax Error")
    );
}

#[tokio::test]
async fn graphql_mutations_update_shared_state() {
    let w = world("basic");
    let a = app(&w);
    let r = gql(&a, "mutation($in: WorkItemCreateInput!) { workItemCreate(input: $in) { workItem { iid title } errors } }", json!({"in": {"namespacePath": "acme/platform/api", "title": "From GraphQL", "workItemTypeId": "gid://gitlab/WorkItems::Type/5", "labelsWidget": {"labelIds": ["gid://gitlab/ProjectLabel/301"]}}})).await;
    assert_eq!(r["data"]["workItemCreate"]["workItem"]["iid"], "5");
    let r = gql(&a, "mutation { workItemUpdate(input: { id: \"gid://gitlab/WorkItem/5001\", stateEvent: CLOSE, milestoneWidget: { milestoneId: null } }) { workItem { state } errors } }", json!({})).await;
    assert_eq!(r["data"]["workItemUpdate"]["workItem"]["state"], "CLOSED");
    let r = gql(&a, "mutation { createNote(input: { noteableId: \"gid://gitlab/WorkItem/5001\", body: \"hi\" }) { note { body } errors } }", json!({})).await;
    assert_eq!(r["data"]["createNote"]["note"]["body"], "hi");
    let r = gql(&a, "mutation { iterationCreate(input: { groupPath: \"acme\", title: \"S13\" }) { iteration { id } errors } }", json!({})).await;
    assert_eq!(
        r["data"]["iterationCreate"]["errors"][0],
        "Start date can't be blank"
    );
    let r = gql(&a, "mutation { nope(input: {}) { errors } }", json!({})).await;
    assert_eq!(
        r["errors"][0]["message"],
        "Field 'nope' doesn't exist on type 'Mutation'"
    );
    let s = state(&w);
    assert_eq!(s["issues"]["acme/platform/api#5"]["issue_type"], "task");
    assert_eq!(s["issues"]["acme/platform/api#5"]["labels"], json!(["bug"]));
    assert_eq!(s["issues"]["acme/platform/api#1"]["state"], "closed");
    assert_eq!(
        s["issues"]["acme/platform/api#1"]["milestone_id"],
        Value::Null
    );
    // REST sees the GraphQL change.
    let r = get(&a, &v4(&format!("{API}/issues/1"))).await;
    assert_eq!(r.json["state"], "closed");
}

#[tokio::test]
async fn iterations_rest_and_dependency_proxy() {
    let w = world("basic");
    let a = app(&w);
    let r = get(&a, &v4("/groups/acme/iterations?state=current")).await;
    assert_eq!(ids(&r.json, "title"), ["Sprint 12"]);
    let r = get(&a, &v4(&format!("{API}/iterations"))).await;
    assert_eq!(r.json[0]["state"], 2);
    let r = call(&a, "DELETE", &v4("/groups/acme/dependency_proxy/cache"), "").await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    assert!(
        state(&w)["dependency_proxy_blobs"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    let r = get(&a, &v4("/orbit/status")).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}
