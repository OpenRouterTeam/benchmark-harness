//! Endpoint-family tests against the full fixture (`tests/fixtures/full`) and the
//! scenario datasets. Each family covers a happy path, GitHub's error shapes,
//! pagination, and mutations reflected in `/_bench/state`.

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
use mock_github::GitHub;
use serde_json::{Value, json};
use tower::ServiceExt;

const HOST: &str = "github.octo-corp.com";

fn load(rel: &str) -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

fn full() -> (SharedWorld, Router) {
    let w = load("tests/fixtures/full");
    let app = service_app(w.clone(), &GitHub);
    (w, app)
}

fn scenario(slug: &str) -> (SharedWorld, Router) {
    let w = load(&format!("../../../scenarios/code/github/{slug}"));
    let app = service_app(w.clone(), &GitHub);
    (w, app)
}

struct Resp {
    status: StatusCode,
    headers: HeaderMap,
    text: String,
    json: Value,
}

async fn send(app: &Router, method: &str, uri: &str, body: &str, accept: Option<&str>) -> Resp {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", HOST)
        .header("x-forwarded-proto", "https")
        .header("content-type", "application/json");
    if let Some(a) = accept {
        req = req.header("accept", a);
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = to_bytes(body, usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Resp {
        status: parts.status,
        headers: parts.headers,
        json: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        text,
    }
}

async fn get(app: &Router, uri: &str) -> Resp {
    send(app, "GET", uri, "", None).await
}

async fn gql(app: &Router, query: &str, vars: Value) -> Value {
    let body = json!({"query": query, "variables": vars}).to_string();
    let r = send(app, "POST", "/api/graphql", &body, None).await;
    assert_eq!(r.status, StatusCode::OK);
    r.json
}

fn state(w: &SharedWorld) -> Value {
    w.lock().unwrap().snapshot()["services"]["github"].clone()
}

const PAY: &str = "/api/v3/repos/octo-org/payments-api";

// ---------------------------------------------------------------- startup / users / orgs

#[tokio::test]
async fn root_reports_token_scopes_and_user_profiles() {
    let (_, app) = full();
    let head = send(&app, "HEAD", "/api/v3/", "", None).await;
    assert_eq!(head.status, StatusCode::OK);
    assert!(
        head.headers["x-oauth-scopes"]
            .to_str()
            .unwrap()
            .contains("repo")
    );
    let me = get(&app, "/api/v3/user").await;
    assert_eq!(me.json["login"], "mona");
    assert_eq!(me.json["name"], "Mona Lisa");
    assert_eq!(
        me.json["url"],
        "https://github.octo-corp.com/api/v3/users/mona"
    );
    assert_eq!(me.json["private_gists"], 1);
    let org = get(&app, "/api/v3/users/octo-org").await;
    assert_eq!(org.json["type"], "Organization");
    assert_eq!(
        get(&app, "/api/v3/users/nobody").await.status,
        StatusCode::NOT_FOUND
    );
    let teams = get(&app, "/api/v3/orgs/octo-org/teams/backend/members").await;
    let logins: Vec<&str> = teams
        .json
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["login"].as_str().unwrap())
        .collect();
    assert_eq!(logins, ["priya", "sam", "mona"]);
}

// ---------------------------------------------------------------- repositories & contents

#[tokio::test]
async fn contents_files_directories_and_raw() {
    let (_, app) = full();
    let file = get(
        &app,
        &format!("{PAY}/contents/internal/charge/retry.go?ref=main"),
    )
    .await;
    assert_eq!(file.status, StatusCode::OK);
    assert_eq!(file.json["type"], "file");
    assert_eq!(file.json["encoding"], "base64");
    let content: String = file.json["content"].as_str().unwrap().replace('\n', "");
    let decoded = mock_github::util::b64_decode(&content).unwrap();
    assert!(
        String::from_utf8(decoded.clone())
            .unwrap()
            .contains("maxAttempts = 3")
    );
    assert_eq!(file.json["sha"], mock_github::sha1::blob_sha(&decoded));
    let dir = get(&app, &format!("{PAY}/contents/internal")).await;
    let names: Vec<&str> = dir
        .json
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["charge", "webhook"]);
    assert_eq!(
        get(&app, &format!("{PAY}/contents/nope.txt")).await.status,
        StatusCode::NOT_FOUND
    );
    let raw = get(
        &app,
        "/raw/octo-org/payments-api/refs/heads/feat/webhook-backoff/internal/webhook/backoff.go",
    )
    .await;
    assert_eq!(raw.status, StatusCode::OK);
    assert!(raw.text.contains("maxDelay"));
    assert_eq!(
        get(&app, "/raw/octo-org/payments-api/main/missing.go")
            .await
            .text,
        "404: Not Found"
    );
    let empty = get(&app, "/api/v3/repos/octo-org/empty/git/ref/heads/main").await;
    assert_eq!(empty.status, StatusCode::CONFLICT);
    assert_eq!(empty.json["message"], "Git Repository is empty.");
}

#[tokio::test]
async fn put_and_delete_contents_create_commits() {
    let (w, app) = full();
    let body = json!({"message": "Add runbook", "content": mock_github::util::b64_encode(b"# Runbook\n"), "branch": "main"}).to_string();
    let r = send(
        &app,
        "PUT",
        &format!("{PAY}/contents/docs/runbook.md"),
        &body,
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let sha = r.json["content"]["sha"].as_str().unwrap().to_owned();
    // Updating without sha is a 422, with a stale sha a 409.
    let again = send(
        &app,
        "PUT",
        &format!("{PAY}/contents/docs/runbook.md"),
        &body,
        None,
    )
    .await;
    assert_eq!(again.status, StatusCode::UNPROCESSABLE_ENTITY);
    let stale = json!({"message": "x", "content": "eA==", "sha": "0000000000000000000000000000000000000000"}).to_string();
    assert_eq!(
        send(
            &app,
            "PUT",
            &format!("{PAY}/contents/docs/runbook.md"),
            &stale,
            None
        )
        .await
        .status,
        StatusCode::CONFLICT
    );
    let del = json!({"message": "Remove runbook", "sha": sha, "branch": "main"}).to_string();
    let r = send(
        &app,
        "DELETE",
        &format!("{PAY}/contents/docs/runbook.md"),
        &del,
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let head = state(&w)["branches"]["octo-org/payments-api:main"]["sha"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(head, r.json["commit"]["sha"]);
    let log = get(&app, &format!("{PAY}/commits?per_page=2")).await;
    assert_eq!(log.json[0]["commit"]["message"], "Remove runbook");
    assert_eq!(log.json[1]["commit"]["message"], "Add runbook");
    assert!(
        log.headers["link"]
            .to_str()
            .unwrap()
            .contains("rel=\"next\"")
    );
}

#[tokio::test]
async fn git_data_api_tree_commit_ref_flow() {
    let (w, app) = full();
    let r#ref = get(&app, &format!("{PAY}/git/ref/heads/main")).await;
    let base = r#ref.json["object"]["sha"].as_str().unwrap().to_owned();
    let commit = get(&app, &format!("{PAY}/git/commits/{base}")).await;
    let tree = commit.json["tree"]["sha"].as_str().unwrap().to_owned();
    let new_tree = send(&app, "POST", &format!("{PAY}/git/trees"), &json!({"base_tree": tree, "tree": [{"path": "a.txt", "mode": "100644", "type": "blob", "content": "A\n"}, {"path": "README.md", "mode": "100644", "type": "blob", "sha": null}]}).to_string(), None).await;
    assert_eq!(new_tree.status, StatusCode::CREATED);
    let new_commit = send(
        &app,
        "POST",
        &format!("{PAY}/git/commits"),
        &json!({"message": "Swap readme", "tree": new_tree.json["sha"], "parents": [base]})
            .to_string(),
        None,
    )
    .await;
    assert_eq!(new_commit.status, StatusCode::CREATED);
    let sha = new_commit.json["sha"].as_str().unwrap().to_owned();
    let upd = send(
        &app,
        "PATCH",
        &format!("{PAY}/git/refs/heads/main"),
        &json!({"sha": sha, "force": false}).to_string(),
        None,
    )
    .await;
    assert_eq!(upd.status, StatusCode::OK);
    assert_eq!(
        state(&w)["branches"]["octo-org/payments-api:main"]["sha"],
        sha.as_str()
    );
    let files = get(&app, &format!("{PAY}/commits/{sha}")).await;
    let changed: Vec<&str> = files.json["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["status"].as_str().unwrap())
        .collect();
    assert_eq!(changed, ["removed", "added"]);
    // Non-fast-forward updates are rejected.
    let back = send(
        &app,
        "PATCH",
        &format!("{PAY}/git/refs/heads/main"),
        &json!({"sha": base}).to_string(),
        None,
    )
    .await;
    assert_eq!(back.status, StatusCode::UNPROCESSABLE_ENTITY);
    let created = send(
        &app,
        "POST",
        &format!("{PAY}/git/refs"),
        &json!({"ref": "refs/heads/topic", "sha": sha}).to_string(),
        None,
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let dup = send(
        &app,
        "POST",
        &format!("{PAY}/git/refs"),
        &json!({"ref": "refs/heads/topic", "sha": sha}).to_string(),
        None,
    )
    .await;
    assert_eq!(dup.json["message"], "Reference already exists");
    let rec = get(&app, &format!("{PAY}/git/trees/main?recursive=1")).await;
    assert!(
        rec.json["tree"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["path"] == "internal/charge/retry.go")
    );
}

#[tokio::test]
async fn branches_tags_releases_compare() {
    let (w, app) = full();
    let branches = get(&app, &format!("{PAY}/branches?per_page=2")).await;
    assert_eq!(branches.json.as_array().unwrap().len(), 2);
    assert!(branches.headers.get("link").is_some());
    let tag_ref = get(&app, &format!("{PAY}/git/ref/tags/v1.0.0")).await;
    assert_eq!(tag_ref.json["object"]["type"], "tag");
    let tag = get(
        &app,
        &format!(
            "{PAY}/git/tags/{}",
            tag_ref.json["object"]["sha"].as_str().unwrap()
        ),
    )
    .await;
    assert_eq!(tag.json["message"], "First stable release");
    let latest = get(&app, &format!("{PAY}/releases/latest")).await;
    assert_eq!(latest.json["tag_name"], "v1.1.0");
    assert_eq!(
        get(&app, &format!("{PAY}/releases/tags/v9")).await.status,
        StatusCode::NOT_FOUND
    );
    let cmp = get(&app, &format!("{PAY}/compare/main...feat/webhook-backoff")).await;
    assert_eq!(cmp.json["status"], "ahead");
    assert_eq!(cmp.json["ahead_by"], 2);
    let rel = send(
        &app,
        "POST",
        &format!("{PAY}/releases"),
        &json!({"tag_name": "v1.2.0", "name": "v1.2.0"}).to_string(),
        None,
    )
    .await;
    assert_eq!(rel.status, StatusCode::CREATED);
    assert!(state(&w)["tags"]["octo-org/payments-api:v1.2.0"].is_object());
}

#[tokio::test]
async fn create_fork_and_star_repositories() {
    let (w, app) = full();
    let r = send(
        &app,
        "POST",
        "/api/v3/orgs/octo-org/repos",
        &json!({"name": "svc", "private": true, "auto_init": true}).to_string(),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(r.json["full_name"], "octo-org/svc");
    let dup = send(
        &app,
        "POST",
        "/api/v3/orgs/octo-org/repos",
        &json!({"name": "svc"}).to_string(),
        None,
    )
    .await;
    assert_eq!(dup.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(state(&w)["branches"]["octo-org/svc:main"].is_object());
    let fork = send(&app, "POST", &format!("{PAY}/forks"), "{}", None).await;
    assert_eq!(fork.status, StatusCode::ACCEPTED);
    assert_eq!(fork.json["full_name"], "mona/payments-api");
    assert_eq!(fork.json["parent"]["full_name"], "octo-org/payments-api");
    assert_eq!(
        send(&app, "PUT", "/api/v3/user/starred/octo-org/app", "", None)
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    let starred = send(
        &app,
        "GET",
        "/api/v3/user/starred",
        "",
        Some("application/vnd.github.star+json"),
    )
    .await;
    assert_eq!(starred.json.as_array().unwrap().len(), 2);
    assert!(starred.json[0]["starred_at"].is_string());
}

// ---------------------------------------------------------------- issues

#[tokio::test]
async fn issues_create_update_comments_labels() {
    let (w, app) = full();
    let r = send(&app, "POST", &format!("{PAY}/issues"), &json!({"title": "Backoff cap", "labels": ["bug", "regression"], "assignees": ["jdoe"], "milestone": 1, "type": "Bug"}).to_string(), None).await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(r.json["number"], 45);
    assert_eq!(r.json["type"]["name"], "Bug");
    let st = state(&w);
    assert_eq!(
        st["issues"]["octo-org/payments-api#45"]["labels"],
        json!(["bug", "regression"])
    );
    assert!(st["labels"]["octo-org/payments-api:regression"].is_object());
    let bad = send(
        &app,
        "POST",
        &format!("{PAY}/issues"),
        &json!({"title": "x", "assignees": ["ghost"]}).to_string(),
        None,
    )
    .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(bad.json["errors"][0]["field"], "assignees");
    let closed = send(
        &app,
        "PATCH",
        &format!("{PAY}/issues/45"),
        &json!({"state": "closed", "state_reason": "not_planned"}).to_string(),
        None,
    )
    .await;
    assert_eq!(closed.json["state_reason"], "not_planned");
    assert_eq!(closed.json["closed_by"]["login"], "mona");
    let c = send(
        &app,
        "POST",
        &format!("{PAY}/issues/45/comments"),
        &json!({"body": "Won't fix"}).to_string(),
        None,
    )
    .await;
    assert_eq!(c.status, StatusCode::CREATED);
    let list = get(&app, &format!("{PAY}/issues?state=all&labels=bug")).await;
    let numbers: Vec<u64> = list
        .json
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["number"].as_u64().unwrap())
        .collect();
    assert_eq!(numbers, [45, 39]);
    let pr_as_issue = get(&app, &format!("{PAY}/issues/42")).await;
    assert!(pr_as_issue.json["pull_request"].is_object());
    let labels = send(
        &app,
        "POST",
        &format!("{PAY}/issues/39/labels"),
        &json!({"labels": ["enhancement"]}).to_string(),
        None,
    )
    .await;
    assert_eq!(labels.json.as_array().unwrap().len(), 2);
    assert_eq!(
        send(
            &app,
            "DELETE",
            &format!("{PAY}/issues/39/labels/nope"),
            "",
            None
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, &format!("{PAY}/issues/999")).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn sub_issues_and_reactions() {
    let (w, app) = full();
    let add = send(
        &app,
        "POST",
        &format!("{PAY}/issues/39/sub_issues"),
        &json!({"sub_issue_id": 7044}).to_string(),
        None,
    )
    .await;
    assert_eq!(add.status, StatusCode::CREATED);
    assert_eq!(add.json["number"], 44);
    let again = send(
        &app,
        "POST",
        &format!("{PAY}/issues/39/sub_issues"),
        &json!({"sub_issue_id": 7044}).to_string(),
        None,
    )
    .await;
    assert_eq!(again.status, StatusCode::UNPROCESSABLE_ENTITY);
    let prio = send(
        &app,
        "PATCH",
        &format!("{PAY}/issues/39/sub_issues/priority"),
        &json!({"sub_issue_id": 7044, "before_id": 7043}).to_string(),
        None,
    )
    .await;
    assert_eq!(prio.status, StatusCode::OK);
    assert_eq!(
        state(&w)["issues"]["octo-org/payments-api#39"]["sub_issues"],
        json!(["octo-org/payments-api#44", "octo-org/payments-api#43"])
    );
    let parent = get(&app, &format!("{PAY}/issues/44/parent")).await;
    assert_eq!(parent.json["number"], 39);
    let rm = send(
        &app,
        "DELETE",
        &format!("{PAY}/issues/39/sub_issue"),
        &json!({"sub_issue_id": 7043}).to_string(),
        None,
    )
    .await;
    assert_eq!(rm.status, StatusCode::OK);
    let re = send(
        &app,
        "POST",
        &format!("{PAY}/issues/39/reactions"),
        &json!({"content": "rocket"}).to_string(),
        None,
    )
    .await;
    assert_eq!(re.status, StatusCode::CREATED);
    let same = send(
        &app,
        "POST",
        &format!("{PAY}/issues/39/reactions"),
        &json!({"content": "rocket"}).to_string(),
        None,
    )
    .await;
    assert_eq!(same.status, StatusCode::OK);
    let bad = send(
        &app,
        "POST",
        &format!("{PAY}/issues/39/reactions"),
        &json!({"content": "tada"}).to_string(),
        None,
    )
    .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        get(&app, &format!("{PAY}/issues/39")).await.json["reactions"]["rocket"],
        1
    );
}

// ---------------------------------------------------------------- pull requests

#[tokio::test]
async fn pulls_read_files_diff_and_checks() {
    let (_, app) = full();
    let pr = get(&app, &format!("{PAY}/pulls/42")).await;
    assert_eq!(pr.json["commits"], 2);
    assert_eq!(pr.json["changed_files"], 2);
    assert_eq!(pr.json["head"]["ref"], "feat/webhook-backoff");
    let diff = send(
        &app,
        "GET",
        &format!("{PAY}/pulls/42"),
        "",
        Some("application/vnd.github.v3.diff"),
    )
    .await;
    assert!(
        diff.text
            .starts_with("diff --git a/internal/webhook/backoff.go")
    );
    let files = get(&app, &format!("{PAY}/pulls/42/files?per_page=1")).await;
    assert_eq!(files.json.as_array().unwrap().len(), 1);
    assert!(files.headers["link"].to_str().unwrap().contains("page=2"));
    let head = pr.json["head"]["sha"].as_str().unwrap();
    let checks = get(&app, &format!("{PAY}/commits/{head}/check-runs")).await;
    assert_eq!(checks.json["total_count"], 2);
    let status = get(&app, &format!("{PAY}/commits/{head}/status")).await;
    assert_eq!(status.json["state"], "failure");
    let open = get(
        &app,
        &format!("{PAY}/pulls?state=open&head=octo-org:feat/webhook-backoff"),
    )
    .await;
    assert_eq!(open.json.as_array().unwrap().len(), 1);
    assert_eq!(
        get(&app, &format!("{PAY}/pulls/77")).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn pulls_create_review_and_merge() {
    let (w, app) = scenario("approve-merge-dependency-bump");
    let web = "/api/v3/repos/octo-org/web";
    let blocked = send(
        &app,
        "PUT",
        &format!("{web}/pulls/57/merge"),
        &json!({"merge_method": "squash"}).to_string(),
        None,
    )
    .await;
    assert_eq!(blocked.status, StatusCode::METHOD_NOT_ALLOWED);
    let review = send(
        &app,
        "POST",
        &format!("{web}/pulls/57/reviews"),
        &json!({"event": "APPROVE", "body": "LGTM"}).to_string(),
        None,
    )
    .await;
    assert_eq!(review.status, StatusCode::OK);
    assert_eq!(review.json["state"], "APPROVED");
    let merge_commit = send(
        &app,
        "PUT",
        &format!("{web}/pulls/57/merge"),
        &json!({"merge_method": "merge"}).to_string(),
        None,
    )
    .await;
    assert_eq!(merge_commit.status, StatusCode::METHOD_NOT_ALLOWED);
    let merged = send(
        &app,
        "PUT",
        &format!("{web}/pulls/57/merge"),
        &json!({"merge_method": "squash"}).to_string(),
        None,
    )
    .await;
    assert_eq!(merged.status, StatusCode::OK);
    assert_eq!(merged.json["merged"], true);
    let st = state(&w);
    assert_eq!(st["pulls"]["octo-org/web#57"]["merged"], true);
    assert_eq!(st["pulls"]["octo-org/web#57"]["merge_method"], "squash");
    let main = st["branches"]["octo-org/web:main"]["sha"].as_str().unwrap();
    let pkg = get(&app, &format!("{web}/contents/package.json?ref={main}")).await;
    let text = String::from_utf8(
        mock_github::util::b64_decode(&pkg.json["content"].as_str().unwrap().replace('\n', ""))
            .unwrap(),
    )
    .unwrap();
    assert!(text.contains("\"lodash\": \"4.17.21\""));
    let twice = send(&app, "PUT", &format!("{web}/pulls/57/merge"), "{}", None).await;
    assert_eq!(twice.status, StatusCode::METHOD_NOT_ALLOWED);
    let exists = send(
        &app,
        "POST",
        &format!("{web}/pulls"),
        &json!({"title": "x", "head": "renovate/axios-1.x", "base": "main"}).to_string(),
        None,
    )
    .await;
    assert_eq!(exists.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        exists.json["errors"][0]["message"],
        "A pull request already exists for octo-org:renovate/axios-1.x."
    );
    let same = send(
        &app,
        "POST",
        &format!("{web}/pulls"),
        &json!({"title": "x", "head": "main", "base": "main"}).to_string(),
        None,
    )
    .await;
    assert_eq!(
        same.json["errors"][0]["message"],
        "No commits between main and main"
    );
    let ok = send(&app, "POST", "/api/v3/repos/octo-org/web/git/refs", &json!({"ref": "refs/heads/topic", "sha": st["commits"].as_object().unwrap().values().find(|c| c["message"] == "feat: dark mode toggle").unwrap()["sha"]}).to_string(), None).await;
    assert_eq!(ok.status, StatusCode::CREATED);
    let created = send(
        &app,
        "POST",
        &format!("{web}/pulls"),
        &json!({"title": "Topic", "head": "topic", "base": "main", "draft": true}).to_string(),
        None,
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    assert_eq!(created.json["number"], 59);
    assert_eq!(created.json["draft"], true);
    assert_eq!(state(&w)["pulls"]["octo-org/web#59"]["head"], "topic");
}

#[tokio::test]
async fn review_comments_replies_and_reviewers() {
    let (w, app) = full();
    let reply = send(
        &app,
        "POST",
        &format!("{PAY}/pulls/42/comments"),
        &json!({"body": "Agreed", "in_reply_to": 902}).to_string(),
        None,
    )
    .await;
    assert_eq!(reply.status, StatusCode::CREATED);
    assert_eq!(reply.json["in_reply_to_id"], 901);
    let req = send(
        &app,
        "POST",
        &format!("{PAY}/pulls/42/requested_reviewers"),
        &json!({"reviewers": ["sam"], "team_reviewers": ["backend"]}).to_string(),
        None,
    )
    .await;
    assert_eq!(req.status, StatusCode::CREATED);
    assert_eq!(
        state(&w)["pulls"]["octo-org/payments-api#42"]["requested_reviewers"],
        json!(["mona", "sam"])
    );
    let author = send(
        &app,
        "POST",
        &format!("{PAY}/pulls/42/requested_reviewers"),
        &json!({"reviewers": ["jdoe"]}).to_string(),
        None,
    )
    .await;
    assert_eq!(author.status, StatusCode::UNPROCESSABLE_ENTITY);
    let outsider = send(
        &app,
        "POST",
        &format!("{PAY}/pulls/42/requested_reviewers"),
        &json!({"reviewers": ["copilot-pull-request-reviewer[bot]"]}).to_string(),
        None,
    )
    .await;
    assert!(
        outsider.json["message"]
            .as_str()
            .unwrap()
            .starts_with("Reviews may only be requested from collaborators")
    );
    let upd = send(
        &app,
        "PUT",
        &format!("{PAY}/pulls/41/update-branch"),
        "{}",
        None,
    )
    .await;
    assert_eq!(upd.status, StatusCode::UNPROCESSABLE_ENTITY);
}

// ---------------------------------------------------------------- actions

#[tokio::test]
async fn actions_runs_jobs_logs_and_triggers() {
    let (w, app) = full();
    let runs = get(
        &app,
        &format!("{PAY}/actions/runs?branch=feat/webhook-backoff"),
    )
    .await;
    assert_eq!(runs.json["total_count"], 1);
    assert_eq!(
        runs.json["workflow_runs"][0]["pull_requests"][0]["number"],
        42
    );
    let jobs = get(&app, &format!("{PAY}/actions/runs/9005/jobs")).await;
    assert_eq!(jobs.json["total_count"], 2);
    let logs = get(&app, &format!("{PAY}/actions/jobs/50114/logs")).await;
    assert_eq!(logs.status, StatusCode::FOUND);
    let loc = logs.headers["location"].to_str().unwrap();
    assert!(
        loc.starts_with("https://github.octo-corp.com/_downloads/jobs/octo-org/payments-api/50114")
    );
    let path = loc.trim_start_matches("https://github.octo-corp.com");
    let text = get(&app, path).await;
    assert!(text.text.contains("TestRetryBackoffCapsAtMaxDelay"));
    let dispatch = send(
        &app,
        "POST",
        &format!("{PAY}/actions/workflows/ci.yml/dispatches"),
        &json!({"ref": "main"}).to_string(),
        None,
    )
    .await;
    assert_eq!(dispatch.status, StatusCode::NO_CONTENT);
    let no_trigger = send(
        &app,
        "POST",
        &format!("{PAY}/actions/workflows/302/dispatches"),
        &json!({"ref": "main"}).to_string(),
        None,
    )
    .await;
    assert_eq!(no_trigger.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{PAY}/actions/runs/9005/cancel"),
            "",
            None
        )
        .await
        .status,
        StatusCode::CONFLICT
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{PAY}/actions/runs/9006/cancel"),
            "",
            None
        )
        .await
        .status,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{PAY}/actions/runs/9005/rerun-failed-jobs"),
            "",
            None
        )
        .await
        .status,
        StatusCode::CREATED
    );
    let st = state(&w);
    assert_eq!(st["workflow_runs"]["9006"]["conclusion"], "cancelled");
    assert_eq!(st["workflow_runs"]["9005"]["run_attempt"], 2);
    assert_eq!(st["workflow_runs"]["9007"]["event"], "workflow_dispatch");
    assert_eq!(
        get(&app, &format!("{PAY}/actions/runs/1")).await.status,
        StatusCode::NOT_FOUND
    );
}

// ---------------------------------------------------------------- search

#[tokio::test]
async fn search_qualifiers() {
    let (_, app) = scenario("triage-duplicate-charge");
    let r = get(
        &app,
        "/api/v3/search/issues?q=org%3Aocto-org+charged+twice+is%3Aopen+is%3Aissue",
    )
    .await;
    assert_eq!(r.json["total_count"], 1);
    assert_eq!(r.json["items"][0]["number"], 12);
    let closed = get(&app, "/api/v3/search/issues?q=duplicate+charge+is%3Aclosed").await;
    assert_eq!(closed.json["items"][0]["number"], 7);
    let labeled = get(
        &app,
        "/api/v3/search/issues?q=label%3Acustomer-reported+-label%3Abug",
    )
    .await;
    assert_eq!(labeled.json["total_count"], 0);
    let paged = get(
        &app,
        "/api/v3/search/issues?q=is%3Aissue+org%3Aocto-org&per_page=2",
    )
    .await;
    assert_eq!(paged.json["items"].as_array().unwrap().len(), 2);
    assert!(
        paged.headers["link"]
            .to_str()
            .unwrap()
            .contains("rel=\"last\"")
    );
    let code = get(
        &app,
        "/api/v3/search/code?q=maxAttempts+repo%3Aocto-org%2Fpayments-api",
    )
    .await;
    assert_eq!(code.json["items"][0]["path"], "internal/charge/retry.go");
    let repos = get(&app, "/api/v3/search/repositories?q=topic%3Abilling").await;
    assert_eq!(
        repos.json["items"][0]["full_name"],
        "octo-org/billing-worker"
    );
    let users = get(&app, "/api/v3/search/users?q=sam+type%3Auser").await;
    assert_eq!(users.json["items"][0]["login"], "sam");
    let commits = get(&app, "/api/v3/search/commits?q=retry+author%3Asam").await;
    assert_eq!(
        commits.json["items"][0]["commit"]["message"],
        "Retry card authorisation on gateway timeout"
    );
    assert_eq!(
        get(&app, "/api/v3/search/issues").await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

// ---------------------------------------------------------------- activity

#[tokio::test]
async fn notifications_and_gists() {
    let (w, app) = full();
    let unread = get(&app, "/api/v3/notifications").await;
    assert_eq!(unread.json.as_array().unwrap().len(), 2);
    let all = get(&app, "/api/v3/notifications?all=true").await;
    assert_eq!(all.json.as_array().unwrap().len(), 3);
    assert_eq!(
        send(
            &app,
            "PATCH",
            "/api/v3/notifications/threads/1001",
            "",
            None
        )
        .await
        .status,
        StatusCode::RESET_CONTENT
    );
    assert_eq!(
        send(
            &app,
            "DELETE",
            "/api/v3/notifications/threads/1003",
            "",
            None
        )
        .await
        .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(get(&app, "/api/v3/notifications").await.json, json!([]));
    assert_eq!(
        get(&app, "/api/v3/notifications/threads/404").await.status,
        StatusCode::NOT_FOUND
    );
    let gist = send(
        &app,
        "POST",
        "/api/v3/gists",
        &json!({"files": {"a.txt": {"content": "hi"}}, "public": false}).to_string(),
        None,
    )
    .await;
    assert_eq!(gist.status, StatusCode::CREATED);
    let id = gist.json["id"].as_str().unwrap().to_owned();
    let upd = send(
        &app,
        "PATCH",
        &format!("/api/v3/gists/{id}"),
        &json!({"files": {"a.txt": {"content": "bye"}}}).to_string(),
        None,
    )
    .await;
    assert_eq!(upd.json["files"]["a.txt"]["content"], "bye");
    assert_eq!(
        state(&w)["gists"][id.as_str()]["files"]["a.txt"]["content"],
        "bye"
    );
    let empty = send(
        &app,
        "POST",
        "/api/v3/gists",
        &json!({"files": {}}).to_string(),
        None,
    )
    .await;
    assert_eq!(empty.status, StatusCode::UNPROCESSABLE_ENTITY);
    let jdoe = get(&app, "/api/v3/users/jdoe/gists").await;
    assert_eq!(jdoe.json.as_array().unwrap().len(), 1);
}

// ---------------------------------------------------------------- security / governance / projects

#[tokio::test]
async fn security_alerts_and_advisories() {
    let (_, app) = full();
    let cs = get(
        &app,
        &format!("{PAY}/code-scanning/alerts?state=open&severity=high"),
    )
    .await;
    assert_eq!(cs.json.as_array().unwrap().len(), 1);
    assert_eq!(
        get(&app, &format!("{PAY}/code-scanning/alerts/99"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    let dep = get(
        &app,
        &format!("{PAY}/dependabot/alerts?state=open&ecosystem=go"),
    )
    .await;
    assert_eq!(
        dep.json[0]["security_advisory"]["ghsa_id"],
        "GHSA-w32m-9786-jp63"
    );
    let secrets = get(&app, &format!("{PAY}/secret-scanning/alerts")).await;
    assert_eq!(secrets.json[0]["secret_type"], "stripe_api_key");
    let adv = get(&app, "/api/v3/advisories?ecosystem=npm").await;
    assert_eq!(adv.json[0]["ghsa_id"], "GHSA-35jh-r3h4-6jhm");
    let repo_adv = get(&app, "/api/v3/orgs/octo-org/security-advisories").await;
    assert_eq!(repo_adv.json[0]["state"], "draft");
    let quality = get(&app, &format!("{PAY}/code-quality/findings/7")).await;
    assert_eq!(quality.json["rule"]["id"], "go/unused-function");
}

#[tokio::test]
async fn custom_properties_and_rulesets() {
    let (w, app) = full();
    let vals = get(&app, &format!("{PAY}/properties/values")).await;
    assert_eq!(
        vals.json,
        json!([{"property_name": "tier", "value": "critical"}])
    );
    let bad = send(
        &app,
        "PATCH",
        &format!("{PAY}/properties/values"),
        &json!({"properties": [{"property_name": "tier", "value": "gold"}]}).to_string(),
        None,
    )
    .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    let ok = send(
        &app,
        "PATCH",
        &format!("{PAY}/properties/values"),
        &json!({"properties": [{"property_name": "tier", "value": "standard"}]}).to_string(),
        None,
    )
    .await;
    assert_eq!(ok.status, StatusCode::NO_CONTENT);
    assert_eq!(
        state(&w)["repos"]["octo-org/payments-api"]["custom_properties"]["tier"],
        "standard"
    );
    let rules = get(&app, &format!("{PAY}/rules/branches/main")).await;
    assert_eq!(rules.json.as_array().unwrap().len(), 2);
    assert_eq!(
        get(&app, &format!("{PAY}/rules/branches/feature"))
            .await
            .json,
        json!([])
    );
    let sets = get(&app, &format!("{PAY}/rulesets")).await;
    assert_eq!(sets.json.as_array().unwrap().len(), 2);
    let created = send(
        &app,
        "POST",
        &format!("{PAY}/rulesets"),
        &json!({"name": "Tags", "target": "tag", "enforcement": "active", "rules": []}).to_string(),
        None,
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let dup = send(
        &app,
        "POST",
        &format!("{PAY}/rulesets"),
        &json!({"name": "Tags", "enforcement": "active"}).to_string(),
        None,
    )
    .await;
    assert_eq!(dup.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        get(&app, "/api/v3/enterprises/acme/rulesets").await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn projects_rest_items_and_cursor_pagination() {
    let (w, app) = full();
    let list = get(&app, "/api/v3/orgs/octo-org/projectsV2").await;
    assert_eq!(list.json[0]["title"], "Payments roadmap");
    assert_eq!(
        get(&app, "/api/v3/users/octo-org/projectsV2").await.status,
        StatusCode::NOT_FOUND
    );
    let fields = get(&app, "/api/v3/orgs/octo-org/projectsV2/7/fields?per_page=2").await;
    assert_eq!(fields.json.as_array().unwrap().len(), 2);
    let link = fields.headers["link"].to_str().unwrap().to_owned();
    let after = link
        .split("after=")
        .nth(1)
        .unwrap()
        .split('>')
        .next()
        .unwrap();
    let next = get(
        &app,
        &format!("/api/v3/orgs/octo-org/projectsV2/7/fields?per_page=2&after={after}"),
    )
    .await;
    assert_eq!(next.json[0]["name"], "Estimate");
    let items = get(
        &app,
        "/api/v3/orgs/octo-org/projectsV2/7/items?fields=1002&q=status%3A%22In+Progress%22",
    )
    .await;
    assert_eq!(items.json.as_array().unwrap().len(), 1);
    assert_eq!(
        items.json[0]["fields"][1]["value"]["name"]["raw"],
        "In Progress"
    );
    let upd = send(
        &app,
        "PATCH",
        "/api/v3/orgs/octo-org/projectsV2/7/items/501",
        &json!({"fields": [{"id": 1002, "value": "98236657"}]}).to_string(),
        None,
    )
    .await;
    assert_eq!(upd.status, StatusCode::OK);
    assert_eq!(
        state(&w)["project_items"]["octo-org/7:501"]["field_values"]["1002"],
        "98236657"
    );
    let bad = send(
        &app,
        "PATCH",
        "/api/v3/orgs/octo-org/projectsV2/7/items/501",
        &json!({"fields": [{"id": 1002, "value": "nope"}]}).to_string(),
        None,
    )
    .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        send(
            &app,
            "DELETE",
            "/api/v3/orgs/octo-org/projectsV2/7/items/502",
            "",
            None
        )
        .await
        .status,
        StatusCode::NO_CONTENT
    );
}

// ---------------------------------------------------------------- GraphQL

#[tokio::test]
async fn graphql_issues_labels_and_errors() {
    let (_, app) = full();
    let q = "query($owner:String!$repo:String!$first:Int!$after:String$states:[IssueState!]!$orderBy:IssueOrderField!$direction:OrderDirection!){repository(owner: $owner, name: $repo){issues(first: $first, after: $after, states: $states, orderBy: {field: $orderBy, direction: $direction}){nodes{number,title,state,author{login},labels(first: 100){nodes{name}},issueFieldValues(first: 25){nodes{__typename,... on IssueFieldSingleSelectValue{field{... on IssueFieldSingleSelect{name}},value}}}},pageInfo{hasNextPage,endCursor},totalCount},isPrivate}}";
    let vars = json!({"owner": "octo-org", "repo": "payments-api", "first": 1, "after": null, "states": ["OPEN"], "orderBy": "CREATED_AT", "direction": "DESC"});
    let r = gql(&app, q, vars.clone()).await;
    let issues = &r["data"]["repository"]["issues"];
    assert_eq!(issues["totalCount"], 2);
    assert_eq!(issues["nodes"][0]["number"], 43);
    assert_eq!(issues["pageInfo"]["hasNextPage"], true);
    let mut next = vars;
    next["after"] = issues["pageInfo"]["endCursor"].clone();
    let r2 = gql(&app, q, next).await;
    let node = &r2["data"]["repository"]["issues"]["nodes"][0];
    assert_eq!(node["number"], 39);
    assert_eq!(node["issueFieldValues"]["nodes"][0]["value"], "P1");
    assert_eq!(
        node["issueFieldValues"]["nodes"][0]["field"]["name"],
        "Priority"
    );
    let missing = gql(
        &app,
        "{repository(owner: \"octo-org\", name: \"nope\"){id}}",
        json!({}),
    )
    .await;
    assert_eq!(missing["data"]["repository"], Value::Null);
    assert_eq!(missing["errors"][0]["type"], "NOT_FOUND");
    assert_eq!(missing["errors"][0]["path"], json!(["repository"]));
    let parse = gql(&app, "{ viewer { login }", json!({})).await;
    assert!(
        parse["errors"][0]["message"]
            .as_str()
            .unwrap()
            .starts_with("Parse error")
    );
}

#[tokio::test]
async fn graphql_review_flow_and_threads() {
    let (w, app) = full();
    let pr = gql(&app, "query($owner:String!$repo:String!$prNum:Int!){repository(owner: $owner, name: $repo){pullRequest(number: $prNum){id}}}", json!({"owner": "octo-org", "repo": "payments-api", "prNum": 42})).await;
    let pr_id = pr["data"]["repository"]["pullRequest"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let add = gql(&app, "mutation($input:AddPullRequestReviewInput!){addPullRequestReview(input: $input){pullRequestReview{id}}}", json!({"input": {"pullRequestId": pr_id}})).await;
    assert!(add["errors"].is_null(), "{add}");
    let viewer = gql(&app, "{viewer{id}}", json!({})).await["data"]["viewer"]["id"].clone();
    let pending = gql(&app, "query($after:String$name:String!$owner:String!$prNum:Int!$states:[PullRequestReviewState!]){repository(owner: $owner, name: $name){pullRequest(number: $prNum){reviews(first: 100, after: $after, states: $states){nodes{id,author{... on Bot{botId: id},... on User{userId: id}}},pageInfo{hasNextPage,endCursor}}}}}", json!({"owner": "octo-org", "name": "payments-api", "prNum": 42, "after": null, "states": ["PENDING"]})).await;
    let node = &pending["data"]["repository"]["pullRequest"]["reviews"]["nodes"][0];
    assert_eq!(node["author"]["userId"], viewer);
    assert!(node["author"].get("botId").is_none());
    let review_id = node["id"].as_str().unwrap().to_owned();
    let thread = gql(&app, "mutation($input:AddPullRequestReviewThreadInput!){addPullRequestReviewThread(input: $input){thread{id}}}", json!({"input": {"pullRequestReviewId": review_id, "path": "internal/webhook/backoff.go", "body": "nit", "subjectType": "LINE", "line": 10, "side": "RIGHT"}})).await;
    assert!(thread["data"]["addPullRequestReviewThread"]["thread"]["id"].is_string());
    let off = gql(&app, "mutation($input:AddPullRequestReviewThreadInput!){addPullRequestReviewThread(input: $input){thread{id}}}", json!({"input": {"pullRequestReviewId": review_id, "path": "internal/webhook/backoff.go", "body": "nit", "subjectType": "LINE", "line": 400}})).await;
    assert_eq!(
        off["data"]["addPullRequestReviewThread"]["thread"],
        Value::Null
    );
    let submit = gql(&app, "mutation($input:SubmitPullRequestReviewInput!){submitPullRequestReview(input: $input){pullRequestReview{id}}}", json!({"input": {"pullRequestReviewId": review_id, "event": "APPROVE"}})).await;
    assert!(submit["errors"].is_null(), "{submit}");
    let threads = gql(&app, "{repository(owner: \"octo-org\", name: \"payments-api\"){pullRequest(number: 42){reviewThreads(first: 10){totalCount,nodes{id,isResolved,comments(first: 100){totalCount,nodes{body,author{login}}}}}}}}", json!({})).await;
    let nodes = &threads["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"];
    assert_eq!(nodes[0]["comments"]["totalCount"], 2);
    let tid = nodes[0]["id"].as_str().unwrap().to_owned();
    let resolved = gql(&app, "mutation($input:ResolveReviewThreadInput!){resolveReviewThread(input: $input){thread{id,isResolved}}}", json!({"input": {"threadId": tid}})).await;
    assert_eq!(
        resolved["data"]["resolveReviewThread"]["thread"]["isResolved"],
        true
    );
    let st = state(&w);
    assert_eq!(st["review_threads"][tid.as_str()]["is_resolved"], true);
    assert!(
        st["reviews"]
            .as_object()
            .unwrap()
            .values()
            .any(|r| r["state"] == "APPROVED" && r["user"] == "mona")
    );
}

#[tokio::test]
async fn graphql_issue_mutations_and_nodes() {
    let (w, app) = full();
    let ids = gql(&app, "query($owner:String!$repo:String!$issueNumber:Int!$duplicateOf:Int!){repository(owner: $owner, name: $repo){issue(number: $issueNumber){id},duplicateIssue: issue(number: $duplicateOf){id}}}", json!({"owner": "octo-org", "repo": "payments-api", "issueNumber": 43, "duplicateOf": 39})).await;
    let issue = ids["data"]["repository"]["issue"]["id"].clone();
    let dup = ids["data"]["repository"]["duplicateIssue"]["id"].clone();
    let closed = gql(
        &app,
        "mutation($input:CloseIssueInput!){closeIssue(input: $input){issue{id,number,url,state}}}",
        json!({"input": {"issueId": issue, "stateReason": "DUPLICATE", "duplicateIssueId": dup}}),
    )
    .await;
    assert_eq!(closed["data"]["closeIssue"]["issue"]["state"], "CLOSED");
    assert_eq!(
        state(&w)["issues"]["octo-org/payments-api#43"]["state_reason"],
        "duplicate"
    );
    let nodes = gql(&app, "query($ids:[ID!]!){nodes(ids: $ids){... on Issue{id,parent{number},subIssuesSummary{total,completed,percentCompleted},closedByPullRequestsReferences(first: 5, includeClosedPrs: true, orderByState: true){totalCount,nodes{number}}}}}", json!({"ids": [dup]})).await;
    let n = &nodes["data"]["nodes"][0];
    assert_eq!(
        n["subIssuesSummary"],
        json!({"total": 1, "completed": 1, "percentCompleted": 100})
    );
    assert_eq!(
        n["closedByPullRequestsReferences"]["nodes"][0]["number"],
        42
    );
    let repo_id = gql(
        &app,
        "{repository(owner: \"octo-org\", name: \"payments-api\"){id}}",
        json!({}),
    )
    .await["data"]["repository"]["id"]
        .clone();
    let label = gql(
        &app,
        "mutation($input:CreateLabelInput!){createLabel(input: $input){label{name,id}}}",
        json!({"input": {"repositoryId": repo_id, "name": "sev-1", "color": "b60205"}}),
    )
    .await;
    assert_eq!(label["data"]["createLabel"]["label"]["name"], "sev-1");
    let dup_label = gql(
        &app,
        "mutation($input:CreateLabelInput!){createLabel(input: $input){label{name}}}",
        json!({"input": {"repositoryId": repo_id, "name": "sev-1", "color": "b60205"}}),
    )
    .await;
    assert_eq!(dup_label["errors"][0]["type"], "UNPROCESSABLE");
    let created = gql(
        &app,
        "mutation($input:CreateIssueInput!){createIssue(input: $input){issue{fullDatabaseId,url}}}",
        json!({"input": {"repositoryId": repo_id, "title": "Child", "parentIssueId": dup}}),
    )
    .await;
    assert_eq!(
        created["data"]["createIssue"]["issue"]["url"],
        "https://github.octo-corp.com/octo-org/payments-api/issues/45"
    );
    assert_eq!(
        state(&w)["issues"]["octo-org/payments-api#45"]["parent"],
        "octo-org/payments-api#39"
    );
}

#[tokio::test]
async fn graphql_discussions_and_projects() {
    let (w, app) = full();
    let d = gql(&app, "query($owner:String!$repo:String!$first:Int!$after:String$categoryId:ID!){repository(owner: $owner, name: $repo){discussions(first: $first, after: $after, categoryId: $categoryId){nodes{number,title,category{name},author{login}},totalCount}}}", json!({"owner": "octo-org", "repo": "app", "first": 10, "after": null, "categoryId": "DIC_kwDOQA01"})).await;
    assert_eq!(d["data"]["repository"]["discussions"]["totalCount"], 1);
    let disc = gql(
        &app,
        "{repository(owner: \"octo-org\", name: \"app\"){discussion(number: 5){id}}}",
        json!({}),
    )
    .await["data"]["repository"]["discussion"]["id"]
        .clone();
    let add = gql(&app, "mutation($input:AddDiscussionCommentInput!){addDiscussionComment(input: $input){comment{id,url}}}", json!({"input": {"discussionId": disc, "body": "See docs", "replyToId": "DC_kwDOQA52"}})).await;
    let cid = add["data"]["addDiscussionComment"]["comment"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        state(&w)["discussion_comments"][cid.as_str()]["reply_to"],
        "DC_kwDOQA51"
    );
    let ans = gql(&app, "mutation($input:MarkDiscussionCommentAsAnswerInput!){markDiscussionCommentAsAnswer(input: $input){discussion{id,url}}}", json!({"input": {"id": "DC_kwDOQA51"}})).await;
    assert!(ans["errors"].is_null());
    assert_eq!(
        state(&w)["discussions"]["octo-org/app#5"]["answer_comment"],
        "DC_kwDOQA51"
    );
    let fields = gql(&app, "query($owner:String!$projectNumber:Int!$first:Int!$after:String){organization(login: $owner){projectV2(number: $projectNumber){fields(first: $first, after: $after){nodes{... on ProjectV2Field{id,databaseId,name,dataType},... on ProjectV2SingleSelectField{id,databaseId,name,dataType,options{id,name}},... on ProjectV2IterationField{id,databaseId,name,dataType}},pageInfo{hasNextPage,endCursor}}}}}", json!({"owner": "octo-org", "projectNumber": 7, "first": 50, "after": null})).await;
    let nodes = fields["data"]["organization"]["projectV2"]["fields"]["nodes"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(nodes.len(), 4);
    let status = nodes.iter().find(|n| n["name"] == "Status").unwrap();
    assert_eq!(status["options"][2]["name"], "Done");
    let project = gql(
        &app,
        "{organization(login: \"octo-org\"){projectV2(number: 7){id}}}",
        json!({}),
    )
    .await["data"]["organization"]["projectV2"]["id"]
        .clone();
    let item = gql(&app, "{repository(owner: \"octo-org\", name: \"payments-api\"){issue(number: 39){projectItems(first: 50, includeArchived: true){nodes{id,fullDatabaseId,project{id}}}}}}", json!({})).await;
    let item_node = item["data"]["repository"]["issue"]["projectItems"]["nodes"][0].clone();
    assert_eq!(item_node["fullDatabaseId"], "501");
    let upd = gql(&app, "mutation($input:UpdateProjectV2ItemFieldValueInput!$input1:UpdateProjectV2ItemFieldValueInput!){item0: updateProjectV2ItemFieldValue(input: $input){projectV2Item{id,fullDatabaseId}} item1: updateProjectV2ItemFieldValue(input: $input1){projectV2Item{id}}}", json!({
        "input": {"projectId": project, "itemId": item_node["id"], "fieldId": status["id"], "value": {"singleSelectOptionId": "98236657"}},
        "input1": {"projectId": project, "itemId": item_node["id"], "fieldId": status["id"], "value": {"singleSelectOptionId": "missing"}},
    })).await;
    assert_eq!(
        upd["data"]["item0"]["projectV2Item"]["fullDatabaseId"],
        "501"
    );
    assert_eq!(upd["data"]["item1"], Value::Null);
    assert_eq!(upd["errors"][0]["path"], json!(["item1"]));
    assert_eq!(
        state(&w)["project_items"]["octo-org/7:501"]["field_values"]["1002"],
        "98236657"
    );
    let teams = gql(&app, "query($login:String!){user(login: $login){organizations(first: 100){nodes{login,teams(first: 100, userLogins: [$login]){nodes{name,slug,description}}}}}}", json!({"login": "sam"})).await;
    assert_eq!(
        teams["data"]["user"]["organizations"]["nodes"][0]["teams"]["nodes"][0]["slug"],
        "backend"
    );
}

// ---------------------------------------------------------------- scenario datasets

#[tokio::test]
async fn scenario_datasets_load_and_answer() {
    let (_, ci) = scenario("ci-failure-root-cause");
    let failed = get(&ci, &format!("{PAY}/actions/runs/9005/jobs?filter=latest")).await;
    let failing: Vec<&str> = failed.json["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|j| j["conclusion"] == "failure")
        .map(|j| j["name"].as_str().unwrap())
        .collect();
    assert_eq!(failing, ["unit-tests"]);
    let (w, triage) = scenario("triage-duplicate-charge");
    let log = get(
        &triage,
        &format!("{PAY}/commits?path=internal/charge/retry.go"),
    )
    .await;
    assert_eq!(log.json[0]["author"]["login"], "sam");
    let upd = send(
        &triage,
        "PATCH",
        &format!("{PAY}/issues/12"),
        &json!({"labels": ["bug", "customer-reported", "sev-1"], "assignees": ["sam"]}).to_string(),
        None,
    )
    .await;
    assert_eq!(upd.status, StatusCode::OK);
    assert_eq!(
        state(&w)["issues"]["octo-org/payments-api#12"]["assignees"],
        json!(["sam"])
    );
    let (w, login) = scenario("file-login-bug");
    let created = send(
        &login,
        "POST",
        "/api/v3/repos/octo-org/app/issues",
        &json!({"title": "Login page returns 500", "body": "Since deploy 412"}).to_string(),
        None,
    )
    .await;
    assert_eq!(created.json["number"], 4);
    assert_eq!(
        state(&w)["issues"]["octo-org/app#4"]["title"],
        "Login page returns 500"
    );
}
