//! Merge requests: CRUD, lists, changes/diffs/commits/versions, approvals, merge, rebase,
//! conflicts, pipelines, participants and draft notes.
//!
//! MR docs (`merge_requests`, key `<project>!<iid>`) may carry `changes` (GitLab diff objects)
//! and `commits` (shas); otherwise both are derived from the source and target branches.

use axum::{
    Router,
    http::StatusCode,
    response::Response,
    routing::{get as rget, post, put},
};
use mock_core::{
    store::Store,
    world::{SharedWorld, WorldState},
};
use serde_json::{Value, json};

use crate::{
    common::*,
    issues::{ensure_labels, list_filter, milestone_for_project, sort_keys, usernames_for_ids},
};

pub fn routes() -> Router<SharedWorld> {
    let b = "/projects/{id}/merge_requests/{iid}";
    Router::new()
        .route("/merge_requests", rget(wrap(list_global)))
        .route("/groups/{id}/merge_requests", rget(wrap(list_group)))
        .route(
            "/projects/{id}/merge_requests",
            rget(wrap(list_project)).post(wrap(create)),
        )
        .route(b, rget(wrap(get_mr)).put(wrap(update)).delete(wrap(delete)))
        .route(&format!("{b}/changes"), rget(wrap(changes)))
        .route(&format!("{b}/diffs"), rget(wrap(diffs)))
        .route(&format!("{b}/raw_diffs"), rget(wrap(raw_diffs)))
        .route(&format!("{b}/commits"), rget(wrap(commits)))
        .route(&format!("{b}/versions"), rget(wrap(versions)))
        .route(&format!("{b}/versions/{{version_id}}"), rget(wrap(version)))
        .route(&format!("{b}/approvals"), rget(wrap(approvals)))
        .route(&format!("{b}/approval_state"), rget(wrap(approval_state)))
        .route(&format!("{b}/approve"), post(wrap(approve)))
        .route(&format!("{b}/unapprove"), post(wrap(unapprove)))
        .route(&format!("{b}/merge"), put(wrap(merge)))
        .route(
            &format!("{b}/cancel_merge_when_pipeline_succeeds"),
            post(wrap(cancel_auto_merge)),
        )
        .route(&format!("{b}/rebase"), put(wrap(rebase)))
        .route(&format!("{b}/conflicts"), rget(wrap(conflicts)))
        .route(
            &format!("{b}/pipelines"),
            rget(wrap(pipelines)).post(wrap(create_pipeline)),
        )
        .route(&format!("{b}/participants"), rget(wrap(participants)))
        .route(&format!("{b}/reviewers"), rget(wrap(reviewers)))
        .route(&format!("{b}/closes_issues"), rget(wrap(closes_issues)))
        .route(
            &format!("{b}/draft_notes"),
            rget(wrap(list_drafts)).post(wrap(create_draft)),
        )
        .route(
            &format!("{b}/draft_notes/bulk_publish"),
            post(wrap(bulk_publish)),
        )
        .route(
            &format!("{b}/draft_notes/{{draft_id}}"),
            rget(wrap(get_draft))
                .put(wrap(update_draft))
                .delete(wrap(delete_draft)),
        )
        .route(
            &format!("{b}/draft_notes/{{draft_id}}/publish"),
            put(wrap(publish_draft)),
        )
}

// ---------------------------------------------------------------------------
// Rendering

fn is_draft(m: &Value) -> bool {
    let t = str_of(m, "title");
    m.get("draft").and_then(Value::as_bool).unwrap_or(false)
        || t.starts_with("Draft:")
        || t.starts_with("[Draft]")
        || t.starts_with("(Draft)")
}

/// Source/target diff of an MR: the doc's `changes`, else computed from the branches.
pub(crate) fn mr_changes(st: &Store, key: &str) -> Vec<Value> {
    let m = get(st, "merge_requests", key).cloned().unwrap_or(json!({}));
    if let Some(c) = m.get("changes").and_then(Value::as_array) {
        return c.clone();
    }
    let project = source_project(st, key, &m);
    crate::repository::compare_refs(
        st,
        &project,
        str_of(&m, "target_branch"),
        str_of(&m, "source_branch"),
    )
    .map(|(_, d)| d)
    .unwrap_or_default()
}

/// Commit shas of an MR, newest first.
pub(crate) fn mr_commit_shas(st: &Store, key: &str) -> Vec<String> {
    let m = get(st, "merge_requests", key).cloned().unwrap_or(json!({}));
    if let Some(c) = m.get("commits").and_then(Value::as_array) {
        return value_list(&json!(c));
    }
    let project = source_project(st, key, &m);
    crate::repository::compare_refs(
        st,
        &project,
        str_of(&m, "target_branch"),
        str_of(&m, "source_branch"),
    )
    .map(|(c, _)| c.iter().map(|c| str_of(c, "id").to_owned()).collect())
    .unwrap_or_default()
}

fn source_project(st: &Store, key: &str, m: &Value) -> String {
    m.get("source_project")
        .and_then(Value::as_str)
        .filter(|p| get(st, "projects", p).is_some())
        .unwrap_or_else(|| project_of(key))
        .to_owned()
}

fn head_sha(st: &Store, key: &str, m: &Value) -> String {
    if let Some(s) = m.get("sha").and_then(Value::as_str) {
        return s.to_owned();
    }
    let project = source_project(st, key, m);
    crate::repository::branch_head(st, &project, str_of(m, "source_branch"))
        .unwrap_or_else(|| fake_sha(&format!("{key}:head")))
}

fn base_sha(st: &Store, key: &str, m: &Value) -> String {
    if let Some(s) = m.get("base_sha").and_then(Value::as_str) {
        return s.to_owned();
    }
    crate::repository::branch_head(st, project_of(key), str_of(m, "target_branch"))
        .unwrap_or_else(|| fake_sha(&format!("{key}:base")))
}

fn unresolved_threads(st: &Store, key: &str) -> usize {
    all(st, "notes")
        .iter()
        .filter(|(_, n)| {
            str_of(n, "noteable") == key
                && n["resolvable"].as_bool().unwrap_or(false)
                && !n["resolved"].as_bool().unwrap_or(false)
        })
        .count()
}

fn approvals_required(st: &Store, key: &str, m: &Value) -> u64 {
    as_u64(&m["approvals_required"]).unwrap_or_else(|| {
        get(st, "projects", project_of(key))
            .and_then(|p| as_u64(&p["approvals_before_merge"]))
            .unwrap_or(0)
    })
}

/// GitLab's `detailed_merge_status` for an MR doc.
pub(crate) fn detailed_status(st: &Store, key: &str, m: &Value) -> String {
    if str_of(m, "state") != "opened" {
        return "not_open".into();
    }
    if is_draft(m) {
        return "draft_status".into();
    }
    if m.get("has_conflicts")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return "conflict".into();
    }
    if m.get("discussion_locked")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return "discussions_not_resolved".into();
    }
    let project = get(st, "projects", project_of(key))
        .cloned()
        .unwrap_or(json!({}));
    if project["only_allow_merge_if_all_discussions_are_resolved"]
        .as_bool()
        .unwrap_or(false)
        && unresolved_threads(st, key) > 0
    {
        return "discussions_not_resolved".into();
    }
    if (value_list(&m["approved_by"]).len() as u64) < approvals_required(st, key, m) {
        return "not_approved".into();
    }
    let pipeline = crate::ci::latest_mr_pipeline(st, key);
    if project["only_allow_merge_if_pipeline_succeeds"]
        .as_bool()
        .unwrap_or(false)
    {
        match pipeline
            .as_ref()
            .map(|p| str_of(p, "status").to_owned())
            .as_deref()
        {
            Some("success") => {}
            Some("running" | "pending" | "created" | "preparing" | "waiting_for_resource") => {
                return "ci_still_running".into();
            }
            _ => return "ci_must_pass".into(),
        }
    }
    if str_of(&project, "merge_method") == "ff"
        && as_u64(&m["diverged_commits_count"]).unwrap_or(0) > 0
    {
        return "need_rebase".into();
    }
    m.get("detailed_merge_status")
        .and_then(Value::as_str)
        .filter(|s| {
            ![
                "not_approved",
                "mergeable",
                "conflict",
                "draft_status",
                "not_open",
            ]
            .contains(s)
        })
        .unwrap_or("mergeable")
        .to_owned()
}

/// Full merge request representation for the MR keyed `key` (`<project>!<iid>`).
pub(crate) fn render(st: &Store, base: &str, key: &str) -> Value {
    render_with(st, base, key, false)
}

pub(crate) fn render_with(st: &Store, base: &str, key: &str, label_objs: bool) -> Value {
    let m = get(st, "merge_requests", key).cloned().unwrap_or(json!({}));
    let project = project_of(key);
    let pid = project_id(st, project);
    let src_project = source_project(st, key, &m);
    let iid = as_u64(&m["iid"]).unwrap_or_else(|| iid_of(key));
    let assignees = users_list(st, base, &m["assignees"]);
    let reviewers = users_list(st, base, &m["reviewers"]);
    let labels = value_list(&m["labels"]);
    let milestone = crate::issues::milestone_key_by_id(st, &m["milestone_id"])
        .map(|k| crate::issues::render_milestone(st, base, &k))
        .unwrap_or(Value::Null);
    let notes = all(st, "notes")
        .iter()
        .filter(|(_, n)| str_of(n, "noteable") == key && !n["system"].as_bool().unwrap_or(false))
        .count();
    let award = |name: &str| {
        all(st, "award_emoji")
            .iter()
            .filter(|(_, a)| str_of(a, "awardable") == key && str_of(a, "name") == name)
            .count()
    };
    let detailed = detailed_status(st, key, &m);
    let conflicts = m
        .get("has_conflicts")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let merged_by = user_opt(st, base, &m["merged_by"]);
    let head = head_sha(st, key, &m);
    let base_s = base_sha(st, key, &m);
    let pipeline = crate::ci::latest_mr_pipeline(st, key);
    let changes = mr_changes(st, key);
    let short = format!("!{iid}");
    let description = m.get("description").cloned().unwrap_or(json!(""));
    let tasks_total = description.as_str().map_or(0, |d| {
        d.matches("- [ ]").count() + d.matches("- [x]").count()
    });
    let tasks_done = description
        .as_str()
        .map_or(0, |d| d.matches("- [x]").count());
    let title = str_of(&m, "title").to_owned();
    json!({
        "id": as_u64(&m["id"]).unwrap_or(0),
        "iid": iid,
        "project_id": pid,
        "title": title,
        "description": description,
        "state": m.get("state").cloned().unwrap_or(json!("opened")),
        "created_at": m.get("created_at").cloned().unwrap_or(Value::Null),
        "updated_at": m.get("updated_at").or(m.get("created_at")).cloned().unwrap_or(Value::Null),
        "merged_by": merged_by,
        "merge_user": merged_by,
        "merged_at": m.get("merged_at").cloned().unwrap_or(Value::Null),
        "closed_by": user_opt(st, base, &m["closed_by"]),
        "closed_at": m.get("closed_at").cloned().unwrap_or(Value::Null),
        "target_branch": str_of(&m, "target_branch"),
        "source_branch": str_of(&m, "source_branch"),
        "user_notes_count": notes,
        "upvotes": award("thumbsup"),
        "downvotes": award("thumbsdown"),
        "author": user_basic(st, base, str_of(&m, "author")),
        "assignees": assignees,
        "assignee": assignees.get(0).cloned().unwrap_or(Value::Null),
        "reviewers": reviewers,
        "source_project_id": project_id(st, &src_project),
        "target_project_id": pid,
        "labels": if label_objs {
            Value::Array(labels.iter().map(|n| {
                crate::issues::project_labels(st, project, true)
                    .iter()
                    .find(|(_, l)| str_of(l, "name") == n)
                    .map(|(k, _)| crate::issues::render_label(st, k, false))
                    .unwrap_or_else(|| json!({"id": 0, "name": n, "color": "#6699cc", "text_color": "#FFFFFF", "description": null, "description_html": null}))
            }).collect())
        } else {
            json!(labels)
        },
        "draft": is_draft(&m),
        "work_in_progress": is_draft(&m),
        "milestone": milestone,
        "merge_when_pipeline_succeeds": m.get("merge_when_pipeline_succeeds").cloned().unwrap_or(json!(false)),
        "merge_status": if conflicts { "cannot_be_merged" } else { "can_be_merged" },
        "detailed_merge_status": detailed,
        "merge_after": null,
        "sha": head,
        "merge_commit_sha": m.get("merge_commit_sha").cloned().unwrap_or(Value::Null),
        "squash_commit_sha": m.get("squash_commit_sha").cloned().unwrap_or(Value::Null),
        "discussion_locked": m.get("discussion_locked").cloned().unwrap_or(Value::Null),
        "should_remove_source_branch": m.get("should_remove_source_branch").cloned().unwrap_or(Value::Null),
        "force_remove_source_branch": m.get("force_remove_source_branch").cloned().unwrap_or(json!(true)),
        "prepared_at": m.get("created_at").cloned().unwrap_or(Value::Null),
        "allow_collaboration": m.get("allow_collaboration").cloned().unwrap_or(json!(false)),
        "allow_maintainer_to_push": m.get("allow_collaboration").cloned().unwrap_or(json!(false)),
        "reference": short,
        "references": {"short": short, "relative": short, "full": format!("{project}!{iid}")},
        "web_url": format!("{base}/{project}/-/merge_requests/{iid}"),
        "time_stats": {"time_estimate": 0, "total_time_spent": 0, "human_time_estimate": null, "human_total_time_spent": null},
        "squash": m.get("squash").cloned().unwrap_or(json!(false)),
        "squash_on_merge": m.get("squash").cloned().unwrap_or(json!(false)),
        "task_completion_status": {"count": tasks_total, "completed_count": tasks_done},
        "has_conflicts": conflicts,
        "blocking_discussions_resolved": unresolved_threads(st, key) == 0,
        "approvals_before_merge": null,
        "subscribed": false,
        "changes_count": changes.len().to_string(),
        "latest_build_started_at": pipeline.as_ref().map(|p| p.get("started_at").cloned().unwrap_or(Value::Null)).unwrap_or(Value::Null),
        "latest_build_finished_at": pipeline.as_ref().map(|p| p.get("finished_at").cloned().unwrap_or(Value::Null)).unwrap_or(Value::Null),
        "first_deployed_to_production_at": null,
        "pipeline": pipeline.as_ref().map(|p| crate::ci::render_pipeline_basic(st, base, p)).unwrap_or(Value::Null),
        "head_pipeline": pipeline.as_ref().map(|p| crate::ci::render_pipeline(st, base, p)).unwrap_or(Value::Null),
        "diff_refs": {"base_sha": base_s, "head_sha": head, "start_sha": base_s},
        "merge_error": m.get("merge_error").cloned().unwrap_or(Value::Null),
        "first_contribution": false,
        "user": {"can_merge": true},
        "imported": false,
        "imported_from": "none",
    })
}

// ---------------------------------------------------------------------------
// Lists

fn list_in(st: &Store, r: &Req, scope: impl Fn(&str) -> bool) -> Response {
    let mut q = r.clone();
    if q.q("state").is_none() {
        q.query.push(("state".into(), "all".into()));
    }
    let mut keys: Vec<String> = all(st, "merge_requests")
        .into_iter()
        .filter(|(k, d)| scope(project_of(k)) && list_filter(st, &q, k, d, true))
        .map(|(k, _)| k.to_owned())
        .collect();
    sort_keys(st, &q, "merge_requests", &mut keys);
    let details = r.q("with_labels_details") == Some("true");
    let items = keys
        .iter()
        .map(|k| render_with(st, &r.base, k, details))
        .collect();
    paged(r, items)
}

fn list_global(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let mut q = r.clone();
    if q.q("scope").is_none() {
        let explicit = [
            "assignee_id",
            "assignee_username",
            "author_id",
            "author_username",
            "reviewer_id",
            "reviewer_username",
        ]
        .iter()
        .any(|k| r.q(k).is_some());
        q.query.push((
            "scope".into(),
            if explicit { "all" } else { "created_by_me" }.into(),
        ));
    }
    list_in(st, &q, |_| true)
}

fn list_group(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let g = match require_group(st, r) {
        Ok(g) => g,
        Err(e) => return e,
    };
    let prefix = format!("{g}/");
    list_in(st, r, |p| p.starts_with(&prefix))
}

fn list_project(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    list_in(st, r, |p| p == key)
}

pub(crate) fn require_mr(st: &Store, r: &Req) -> Result<String, Response> {
    let project = require_project(st, r)?;
    let key = format!("{project}!{}", r.p("iid"));
    if get(st, "merge_requests", &key).is_some() {
        Ok(key)
    } else {
        Err(not_found("Merge Request"))
    }
}

fn get_mr(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_mr(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let mut out = render(st, &r.base, &key);
    let m = get(st, "merge_requests", &key)
        .cloned()
        .unwrap_or(json!({}));
    if r.q("include_diverged_commits_count") == Some("true") {
        out["diverged_commits_count"] = json!(as_u64(&m["diverged_commits_count"]).unwrap_or(0));
    }
    if r.q("include_rebase_in_progress") == Some("true") {
        out["rebase_in_progress"] = json!(false);
    }
    ok(out)
}

// ---------------------------------------------------------------------------
// Create / update / delete

fn create(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let source_project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(source) = r.s("source_branch") else {
        return missing("source_branch");
    };
    let Some(target) = r.s("target_branch") else {
        return missing("target_branch");
    };
    let Some(title) = r.s("title").filter(|t| !t.trim().is_empty()) else {
        return missing("title");
    };
    let project = match r.s("target_project_id") {
        Some(t) => match project_key(st, &t) {
            Some(p) => p,
            None => return project_not_found(),
        },
        None => source_project.clone(),
    };
    if crate::repository::branch_head(st, &source_project, &source).is_none() {
        return json_status(
            StatusCode::BAD_REQUEST,
            json!({"message": [format!("Source branch \"{source}\" does not exist")]}),
        );
    }
    if crate::repository::branch_head(st, &project, &target).is_none() {
        return json_status(
            StatusCode::BAD_REQUEST,
            json!({"message": [format!("Target branch \"{target}\" does not exist")]}),
        );
    }
    if source == target && project == source_project {
        return json_status(
            StatusCode::BAD_REQUEST,
            json!({"message": ["You can't use same project/branch for source and target"]}),
        );
    }
    if let Some((k, _)) = all(st, "merge_requests").into_iter().find(|(k, m)| {
        project_of(k) == project
            && str_of(m, "state") == "opened"
            && str_of(m, "source_branch") == source
            && str_of(m, "target_branch") == target
    }) {
        return json_status(
            StatusCode::CONFLICT,
            json!({"message": [format!("Another open merge request already exists for this source branch: !{}", iid_of(k))]}),
        );
    }
    let assignees = match r.list("assignee_ids") {
        Some(ids) => usernames_for_ids(st, &ids),
        None => r
            .s("assignee_id")
            .map(|id| usernames_for_ids(st, &[id]))
            .unwrap_or_default(),
    };
    let reviewers = r
        .list("reviewer_ids")
        .map(|ids| usernames_for_ids(st, &ids))
        .unwrap_or_default();
    let milestone = r
        .s("milestone_id")
        .and_then(|m| milestone_for_project(st, &project, &m))
        .map(|m| json!(m))
        .unwrap_or(Value::Null);
    let draft = r.bool("draft").unwrap_or(false);
    let title = if draft && !title.starts_with("Draft:") {
        format!("Draft: {title}")
    } else {
        title
    };
    let head = crate::repository::branch_head(st, &source_project, &source);
    let now = now(w);
    let st = svc_mut(w);
    let labels = ensure_labels(st, &project, &r.list("labels").unwrap_or_default());
    let iid = next_iid(st, "merge_requests", &project, '!');
    let id = next_id(st, "merge_requests", 1);
    let author = current_username(st);
    let key = format!("{project}!{iid}");
    let mut doc = json!({
        "id": id,
        "iid": iid,
        "title": title,
        "description": r.s("description").unwrap_or_default(),
        "state": "opened",
        "source_branch": source,
        "target_branch": target,
        "author": author,
        "assignees": assignees,
        "reviewers": reviewers,
        "labels": labels,
        "milestone_id": milestone,
        "draft": draft,
        "sha": head,
        "created_at": now,
        "updated_at": now,
        "approved_by": [],
        "should_remove_source_branch": r.bool("remove_source_branch"),
        "squash": r.bool("squash").unwrap_or(false),
        "allow_collaboration": r.bool("allow_collaboration").unwrap_or(false),
    });
    if project != source_project {
        doc["source_project"] = json!(source_project);
    }
    let title = str_of(&doc, "title").to_owned();
    st.collection_mut("merge_requests").insert(key.clone(), doc);
    crate::notes::record_event(
        st,
        &project,
        &author,
        "opened",
        "MergeRequest",
        id,
        iid,
        &title,
        &now,
    );
    created(render(st, &r.base, &key))
}

fn update(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_mr(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let project = project_of(&key).to_owned();
    const FIELDS: &[&str] = &[
        "title",
        "description",
        "target_branch",
        "assignee_ids",
        "assignee_id",
        "reviewer_ids",
        "labels",
        "add_labels",
        "remove_labels",
        "state_event",
        "remove_source_branch",
        "squash",
        "draft",
        "milestone_id",
        "discussion_locked",
        "allow_collaboration",
        "allow_maintainer_to_push",
    ];
    if !FIELDS.iter().any(|f| r.has(f)) {
        return bad_request(&format!(
            "{} are missing, at least one parameter must be provided",
            FIELDS.join(", ")
        ));
    }
    if let Some(t) = r.s("target_branch")
        && crate::repository::branch_head(st, &project, &t).is_none()
    {
        return json_status(
            StatusCode::BAD_REQUEST,
            json!({"message": [format!("Target branch \"{t}\" does not exist")]}),
        );
    }
    let milestone = if r.has("milestone_id") {
        match r.s("milestone_id").filter(|m| !m.is_empty() && m != "0") {
            Some(m) => match milestone_for_project(st, &project, &m) {
                Some(id) => Some(json!(id)),
                None => return not_found("Milestone"),
            },
            None => Some(Value::Null),
        }
    } else {
        None
    };
    let assignees = r
        .list("assignee_ids")
        .or_else(|| r.s("assignee_id").map(|a| vec![a]))
        .map(|ids| usernames_for_ids(st, &ids));
    let reviewers = r
        .list("reviewer_ids")
        .map(|ids| usernames_for_ids(st, &ids));
    let state = get(st, "merge_requests", &key)
        .map(|m| str_of(m, "state").to_owned())
        .unwrap_or_default();
    if state == "merged" && r.s("state_event").is_some() {
        return json_status(
            StatusCode::METHOD_NOT_ALLOWED,
            json!({"message": "405 Method Not Allowed"}),
        );
    }
    let now = now(w);
    let st = svc_mut(w);
    let current = current_username(st);
    let labels = r.list("labels").map(|l| ensure_labels(st, &project, &l));
    let add = r
        .list("add_labels")
        .map(|l| ensure_labels(st, &project, &l));
    let remove = r.list("remove_labels");
    let mut system = Vec::new();
    let doc = st
        .collection_mut("merge_requests")
        .get_mut(&key)
        .expect("mr");
    for f in ["title", "description", "target_branch"] {
        if let Some(v) = r.s(f) {
            doc[f] = json!(v);
        }
    }
    if let Some(d) = r.bool("draft") {
        let t = str_of(doc, "title").to_owned();
        let bare = t
            .trim_start_matches("Draft:")
            .trim_start_matches("[Draft]")
            .trim_start_matches("(Draft)")
            .trim()
            .to_owned();
        doc["title"] = json!(if d { format!("Draft: {bare}") } else { bare });
        doc["draft"] = json!(d);
        system.push(
            if d {
                "marked this merge request as **draft**"
            } else {
                "marked this merge request as **ready**"
            }
            .to_owned(),
        );
    }
    for (f, k) in [
        ("remove_source_branch", "should_remove_source_branch"),
        ("squash", "squash"),
        ("discussion_locked", "discussion_locked"),
        ("allow_collaboration", "allow_collaboration"),
    ] {
        if let Some(b) = r.bool(f) {
            doc[k] = json!(b);
        }
    }
    if let Some(a) = assignees {
        system.push(format!(
            "assigned to {}",
            a.iter()
                .map(|u| format!("@{u}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        doc["assignees"] = json!(a);
    }
    if let Some(rv) = reviewers {
        system.push(format!(
            "requested review from {}",
            rv.iter()
                .map(|u| format!("@{u}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        doc["reviewers"] = json!(rv);
    }
    if let Some(m) = milestone {
        doc["milestone_id"] = m;
        system.push("changed milestone".into());
    }
    if let Some(l) = labels {
        doc["labels"] = json!(l);
    }
    if let Some(add) = add {
        let mut cur = value_list(&doc["labels"]);
        for l in add {
            if !cur.contains(&l) {
                system.push(format!("added ~\"{l}\" label"));
                cur.push(l);
            }
        }
        doc["labels"] = json!(cur);
    }
    if let Some(rem) = remove {
        let mut cur = value_list(&doc["labels"]);
        cur.retain(|l| !rem.iter().any(|x| x.eq_ignore_ascii_case(l)));
        doc["labels"] = json!(cur);
    }
    match r.s("state_event").as_deref() {
        Some("close") if state == "opened" => {
            doc["state"] = json!("closed");
            doc["closed_at"] = json!(now);
            doc["closed_by"] = json!(current);
            system.push("closed".into());
        }
        Some("reopen") if state == "closed" => {
            doc["state"] = json!("opened");
            doc["closed_at"] = Value::Null;
            doc["closed_by"] = Value::Null;
            system.push("reopened".into());
        }
        _ => {}
    }
    doc["updated_at"] = json!(now);
    for s in system {
        crate::notes::add_system_note(st, &key, &current, &s, &now);
    }
    ok(render(st, &r.base, &key))
}

fn delete(w: &mut WorldState, r: &Req) -> Response {
    let key = match require_mr(store(w), r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let st = svc_mut(w);
    let current = current_username(st);
    if member_access(st, project_of(&key), &current).unwrap_or(0) < 50 {
        return forbidden();
    }
    remove_doc(st, "merge_requests", &key);
    no_content()
}

// ---------------------------------------------------------------------------
// Changes, diffs, commits, versions

fn diff_for_output(d: &Value, r: &Req) -> Value {
    let mut d = d.clone();
    for (k, v) in [
        ("a_mode", json!("100644")),
        ("b_mode", json!("100644")),
        ("new_file", json!(false)),
        ("renamed_file", json!(false)),
        ("deleted_file", json!(false)),
        ("generated_file", json!(false)),
    ] {
        if d.get(k).is_none() {
            d[k] = v;
        }
    }
    if r.q("unidiff") == Some("true") {
        let diff = str_of(&d, "diff").to_owned();
        let header = format!(
            "--- a/{}\n+++ b/{}\n",
            str_of(&d, "old_path"),
            str_of(&d, "new_path")
        );
        if !diff.starts_with("---") {
            d["diff"] = json!(format!("{header}{diff}"));
        }
    }
    d
}

fn changes(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_mr(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let mut out = render(st, &r.base, &key);
    let diffs: Vec<Value> = mr_changes(st, &key)
        .iter()
        .map(|d| {
            let mut d = diff_for_output(d, r);
            if let Some(o) = d.as_object_mut() {
                o.remove("collapsed");
                o.remove("too_large");
            }
            d
        })
        .collect();
    out["changes"] = Value::Array(diffs);
    out["overflow"] = json!(false);
    ok(out)
}

fn diffs(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_mr(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let items = mr_changes(st, &key)
        .iter()
        .map(|d| {
            let mut d = diff_for_output(d, r);
            d["collapsed"] = json!(false);
            d["too_large"] = json!(false);
            d
        })
        .collect();
    paged(r, items)
}

fn raw_diffs(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_mr(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let mut out = String::new();
    for d in mr_changes(st, &key) {
        out.push_str(&format!(
            "diff --git a/{0} b/{1}\n--- a/{0}\n+++ b/{1}\n{2}",
            str_of(&d, "old_path"),
            str_of(&d, "new_path"),
            str_of(&d, "diff")
        ));
    }
    text(StatusCode::OK, "text/plain; charset=utf-8", out)
}

fn commit_list(st: &Store, base: &str, key: &str) -> Vec<Value> {
    let project = get(st, "merge_requests", key)
        .map(|m| source_project(st, key, m))
        .unwrap_or_else(|| project_of(key).to_owned());
    mr_commit_shas(st, key)
        .iter()
        .map(|sha| crate::repository::render_commit(st, base, &project, sha))
        .collect()
}

fn commits(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_mr(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let items = commit_list(st, &r.base, &key);
    paged(r, items)
}

fn version_json(st: &Store, key: &str) -> Value {
    let m = get(st, "merge_requests", key).cloned().unwrap_or(json!({}));
    let id = as_u64(&m["id"]).unwrap_or(0);
    let head = head_sha(st, key, &m);
    let base = base_sha(st, key, &m);
    json!({
        "id": id * 10 + 1,
        "head_commit_sha": head,
        "base_commit_sha": base,
        "start_commit_sha": base,
        "created_at": m.get("updated_at").or(m.get("created_at")).cloned().unwrap_or(Value::Null),
        "merge_request_id": id,
        "state": "collected",
        "real_size": mr_changes(st, key).len().to_string(),
        "patch_id_sha": fake_sha(&format!("{key}:patch")),
    })
}

fn versions(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_mr(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    paged(r, vec![version_json(st, &key)])
}

fn version(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_mr(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let mut v = version_json(st, &key);
    if !id_is(&v["id"], r.p("version_id")) {
        return not_found("Merge Request Diff");
    }
    v["commits"] = Value::Array(commit_list(st, &r.base, &key));
    v["diffs"] = Value::Array(
        mr_changes(st, &key)
            .iter()
            .map(|d| diff_for_output(d, r))
            .collect(),
    );
    ok(v)
}

// ---------------------------------------------------------------------------
// Approvals

fn approval_user(st: &Store, base: &str, u: &str) -> Value {
    let mut out = user_basic(st, base, u);
    if let Some(o) = out.as_object_mut() {
        o.remove("locked");
    }
    out
}

fn approvals_json(st: &Store, base: &str, key: &str) -> Value {
    let m = get(st, "merge_requests", key).cloned().unwrap_or(json!({}));
    let current = current_username(st);
    let approved_by = value_list(&m["approved_by"]);
    let required = approvals_required(st, key, &m);
    let left = required.saturating_sub(approved_by.len() as u64);
    let author = str_of(&m, "author") == current;
    json!({
        "id": as_u64(&m["id"]).unwrap_or(0),
        "iid": as_u64(&m["iid"]).unwrap_or_else(|| iid_of(key)),
        "project_id": project_id(st, project_of(key)),
        "title": str_of(&m, "title"),
        "description": m.get("description").cloned().unwrap_or(json!("")),
        "state": str_of(&m, "state"),
        "created_at": m["created_at"],
        "updated_at": m.get("updated_at").unwrap_or(&m["created_at"]),
        "merge_status": "can_be_merged",
        "approved": left == 0,
        "approvals_required": required,
        "approvals_left": left,
        "require_password_to_approve": false,
        "approved_by": approved_by.iter().map(|u| json!({"user": approval_user(st, base, u)})).collect::<Vec<_>>(),
        "suggested_approvers": [],
        "approvers": [],
        "approver_groups": [],
        "user_has_approved": approved_by.contains(&current),
        "user_can_approve": !approved_by.contains(&current) && !author && str_of(&m, "state") == "opened",
        "approval_rules_left": if left > 0 { json!([{"id": as_u64(&m["id"]).unwrap_or(0) * 10, "name": "All Members", "rule_type": "any_approver"}]) } else { json!([]) },
        "has_approval_rules": required > 0,
        "merge_request_approvers_available": true,
        "multiple_approval_rules_available": true,
        "invalid_approvers_rules": [],
    })
}

fn approvals(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    match require_mr(st, r) {
        Ok(k) => ok(approvals_json(st, &r.base, &k)),
        Err(e) => e,
    }
}

fn approval_state(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_mr(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let m = get(st, "merge_requests", &key)
        .cloned()
        .unwrap_or(json!({}));
    let required = approvals_required(st, &key, &m);
    let approved_by: Vec<Value> = value_list(&m["approved_by"])
        .iter()
        .map(|u| approval_user(st, &r.base, u))
        .collect();
    let mut rules = Vec::new();
    if let Some(custom) = m.get("approval_rules").and_then(Value::as_array) {
        for (i, rule) in custom.iter().enumerate() {
            let users: Vec<Value> = value_list(&rule["users"])
                .iter()
                .map(|u| approval_user(st, &r.base, u))
                .collect();
            let approvers: Vec<String> = value_list(&rule["users"]);
            let rule_approved: Vec<Value> = value_list(&m["approved_by"])
                .iter()
                .filter(|u| approvers.is_empty() || approvers.contains(u))
                .map(|u| approval_user(st, &r.base, u))
                .collect();
            let req = as_u64(&rule["approvals_required"]).unwrap_or(1);
            rules.push(json!({
                "id": as_u64(&m["id"]).unwrap_or(0) * 10 + i as u64,
                "name": str_of(rule, "name"),
                "rule_type": rule.get("rule_type").cloned().unwrap_or(json!("regular")),
                "eligible_approvers": users,
                "approvals_required": req,
                "users": users,
                "groups": [],
                "contains_hidden_groups": false,
                "approved_by": rule_approved,
                "source_rule": null,
                "approved": rule_approved.len() as u64 >= req,
                "overridden": false,
                "code_owner": false,
                "section": null,
            }));
        }
    } else if required > 0 {
        rules.push(json!({
            "id": as_u64(&m["id"]).unwrap_or(0) * 10,
            "name": "All Members",
            "rule_type": "any_approver",
            "eligible_approvers": [],
            "approvals_required": required,
            "users": [],
            "groups": [],
            "contains_hidden_groups": false,
            "approved_by": approved_by,
            "source_rule": null,
            "approved": approved_by.len() as u64 >= required,
            "overridden": false,
        }));
    }
    ok(json!({"approval_rules_overwritten": false, "rules": rules}))
}

fn approve(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_mr(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let m = get(st, "merge_requests", &key)
        .cloned()
        .unwrap_or(json!({}));
    let current = current_username(st);
    if str_of(&m, "state") != "opened" {
        return error(StatusCode::UNAUTHORIZED, "401 Unauthorized");
    }
    if let Some(sha) = r.s("sha") {
        let head = head_sha(st, &key, &m);
        if sha != head {
            return error(
                StatusCode::CONFLICT,
                &format!("SHA does not match HEAD of source branch: {head}"),
            );
        }
    }
    let mut approved = value_list(&m["approved_by"]);
    if approved.contains(&current) {
        return error(StatusCode::UNAUTHORIZED, "401 Unauthorized");
    }
    approved.push(current.clone());
    let now = now(w);
    let st = svc_mut(w);
    let doc = st
        .collection_mut("merge_requests")
        .get_mut(&key)
        .expect("mr");
    doc["approved_by"] = json!(approved);
    doc["updated_at"] = json!(now);
    crate::notes::add_system_note(st, &key, &current, "approved this merge request", &now);
    let id = as_u64(&m["id"]).unwrap_or(0);
    crate::notes::record_event(
        st,
        project_of(&key),
        &current,
        "approved",
        "MergeRequest",
        id,
        iid_of(&key),
        str_of(&m, "title"),
        &now,
    );
    created(approvals_json(st, &r.base, &key))
}

fn unapprove(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_mr(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let current = current_username(st);
    let mut approved = get(st, "merge_requests", &key)
        .map(|m| value_list(&m["approved_by"]))
        .unwrap_or_default();
    if !approved.contains(&current) {
        return route_not_found();
    }
    approved.retain(|u| *u != current);
    let now = now(w);
    let st = svc_mut(w);
    let doc = st
        .collection_mut("merge_requests")
        .get_mut(&key)
        .expect("mr");
    doc["approved_by"] = json!(approved);
    doc["updated_at"] = json!(now);
    crate::notes::add_system_note(st, &key, &current, "unapproved this merge request", &now);
    created(approvals_json(st, &r.base, &key))
}

// ---------------------------------------------------------------------------
// Merge, rebase, conflicts

fn closing_refs(description: &str) -> Vec<u64> {
    let re = regex::Regex::new(
        r"(?i)\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?|implement(?:s|ed)?)\s*:?\s+#(\d+)",
    )
    .expect("regex");
    re.captures_iter(description)
        .filter_map(|c| c[1].parse().ok())
        .collect()
}

fn merge(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_mr(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let m = get(st, "merge_requests", &key)
        .cloned()
        .unwrap_or(json!({}));
    let project = project_of(&key).to_owned();
    let status = detailed_status(st, &key, &m);
    if str_of(&m, "state") != "opened" {
        return error(StatusCode::METHOD_NOT_ALLOWED, "405 Method Not Allowed");
    }
    if let Some(sha) = r.s("sha") {
        let head = head_sha(st, &key, &m);
        if sha != head {
            return error(
                StatusCode::CONFLICT,
                &format!("SHA does not match HEAD of source branch: {head}"),
            );
        }
    }
    let auto = r.bool("merge_when_pipeline_succeeds").unwrap_or(false)
        || r.bool("auto_merge").unwrap_or(false);
    let pipeline = crate::ci::latest_mr_pipeline(st, &key);
    let running = pipeline.as_ref().is_some_and(|p| {
        [
            "running",
            "pending",
            "created",
            "preparing",
            "waiting_for_resource",
        ]
        .contains(&str_of(p, "status"))
    });
    if auto && (running || status == "ci_still_running") {
        let now = now(w);
        let st = svc_mut(w);
        let current = current_username(st);
        let doc = st
            .collection_mut("merge_requests")
            .get_mut(&key)
            .expect("mr");
        doc["merge_when_pipeline_succeeds"] = json!(true);
        doc["merge_user"] = json!(current);
        doc["updated_at"] = json!(now);
        if let Some(s) = r.bool("squash") {
            doc["squash"] = json!(s);
        }
        if let Some(s) = r.bool("should_remove_source_branch") {
            doc["should_remove_source_branch"] = json!(s);
        }
        crate::notes::add_system_note(
            st,
            &key,
            &current,
            "enabled an automatic merge when the pipeline for this merge request succeeds",
            &now,
        );
        return ok(render(st, &r.base, &key));
    }
    match status.as_str() {
        "mergeable" => {}
        "conflict" | "need_rebase" => {
            return error(StatusCode::NOT_ACCEPTABLE, "Branch cannot be merged");
        }
        "draft_status" => return error(StatusCode::METHOD_NOT_ALLOWED, "405 Method Not Allowed"),
        _ => return error(StatusCode::METHOD_NOT_ALLOWED, "405 Method Not Allowed"),
    }
    let now = now(w);
    let st = svc_mut(w);
    let current = current_username(st);
    let source = str_of(&m, "source_branch").to_owned();
    let target = str_of(&m, "target_branch").to_owned();
    let squash = r.bool("squash").or(m["squash"].as_bool()).unwrap_or(false);
    let message = r.s("merge_commit_message").unwrap_or_else(|| {
        format!(
            "Merge branch '{source}' into '{target}'\n\n{}\n\nSee merge request {project}!{}",
            str_of(&m, "title"),
            iid_of(&key)
        )
    });
    let src_project = source_project(st, &key, &m);
    let merge_sha = crate::repository::merge_branches(
        st,
        &project,
        &src_project,
        &source,
        &target,
        &message,
        &current,
        &now,
    );
    let remove = r
        .bool("should_remove_source_branch")
        .or(m["should_remove_source_branch"].as_bool())
        .unwrap_or(false);
    let doc = st
        .collection_mut("merge_requests")
        .get_mut(&key)
        .expect("mr");
    doc["state"] = json!("merged");
    doc["merged_at"] = json!(now);
    doc["merged_by"] = json!(current);
    doc["merge_commit_sha"] = json!(merge_sha);
    doc["updated_at"] = json!(now);
    doc["merge_when_pipeline_succeeds"] = json!(false);
    if squash {
        doc["squash"] = json!(true);
        doc["squash_commit_sha"] = json!(fake_sha(&format!("{key}:squash")));
    }
    if remove {
        doc["should_remove_source_branch"] = json!(true);
    }
    let description = str_of(doc, "description").to_owned();
    let id = as_u64(&doc["id"]).unwrap_or(0);
    let title = str_of(doc, "title").to_owned();
    if remove && src_project == project {
        crate::repository::remove_branch(st, &project, &source);
    }
    crate::notes::add_system_note(st, &key, &current, "merged", &now);
    crate::notes::record_event(
        st,
        &project,
        &current,
        "accepted",
        "MergeRequest",
        id,
        iid_of(&key),
        &title,
        &now,
    );
    let default_branch = get(st, "projects", &project).map(|p| {
        p.get("default_branch")
            .and_then(Value::as_str)
            .unwrap_or("main")
            .to_owned()
    });
    if default_branch.as_deref() == Some(target.as_str()) {
        for iid in closing_refs(&description) {
            let ik = format!("{project}#{iid}");
            if let Some(i) = st.collection_mut("issues").get_mut(&ik)
                && str_of(i, "state") == "opened"
            {
                i["state"] = json!("closed");
                i["closed_at"] = json!(now);
                i["closed_by"] = json!(current);
                i["updated_at"] = json!(now);
                crate::notes::add_system_note(
                    st,
                    &ik,
                    &current,
                    &format!("closed via merge request !{}", iid_of(&key)),
                    &now,
                );
            }
        }
    }
    ok(render(st, &r.base, &key))
}

fn cancel_auto_merge(w: &mut WorldState, r: &Req) -> Response {
    let key = match require_mr(store(w), r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let st = svc_mut(w);
    let doc = st
        .collection_mut("merge_requests")
        .get_mut(&key)
        .expect("mr");
    if !doc["merge_when_pipeline_succeeds"]
        .as_bool()
        .unwrap_or(false)
    {
        return error(
            StatusCode::NOT_ACCEPTABLE,
            "Can't cancel the automatic merge",
        );
    }
    doc["merge_when_pipeline_succeeds"] = json!(false);
    created(render(st, &r.base, &key))
}

fn rebase(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_mr(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let m = get(st, "merge_requests", &key)
        .cloned()
        .unwrap_or(json!({}));
    if str_of(&m, "state") != "opened" {
        return forbidden();
    }
    let now = now(w);
    let st = svc_mut(w);
    let doc = st
        .collection_mut("merge_requests")
        .get_mut(&key)
        .expect("mr");
    if m["has_conflicts"].as_bool().unwrap_or(false) {
        doc["merge_error"] =
            json!("Rebase failed: Rebase locally, resolve all conflicts, then push the branch.");
    } else {
        doc["diverged_commits_count"] = json!(0);
        doc["merge_error"] = Value::Null;
        doc["sha"] = json!(fake_sha(&format!("{key}:rebase:{now}")));
        doc["updated_at"] = json!(now);
        if doc.get("detailed_merge_status").and_then(Value::as_str) == Some("need_rebase") {
            doc["detailed_merge_status"] = json!("mergeable");
        }
    }
    accepted(json!({"rebase_in_progress": true}))
}

fn conflicts(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_mr(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let m = get(st, "merge_requests", &key)
        .cloned()
        .unwrap_or(json!({}));
    if !m["has_conflicts"].as_bool().unwrap_or(false) {
        return route_not_found();
    }
    let items: Vec<Value> = m
        .get("conflicts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            mr_changes(st, &key)
                .iter()
                .map(|d| {
                    json!({
                        "old_path": d["old_path"],
                        "new_path": d["new_path"],
                        "blob_icon": "file-text-o",
                        "blob_path": format!("/{}/-/blob/{}/{}", project_of(&key), str_of(&m, "source_branch"), str_of(d, "new_path")),
                        "sections": [],
                    })
                })
                .collect()
        });
    ok(Value::Array(items))
}

fn pipelines(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_mr(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let items = crate::ci::mr_pipelines(st, &key)
        .iter()
        .map(|p| crate::ci::render_pipeline_basic(st, &r.base, p))
        .collect();
    paged(r, items)
}

fn create_pipeline(w: &mut WorldState, r: &Req) -> Response {
    let key = match require_mr(store(w), r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    crate::ci::create_mr_pipeline(w, r, &key)
}

fn participants(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_mr(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let m = get(st, "merge_requests", &key)
        .cloned()
        .unwrap_or(json!({}));
    let mut names = vec![str_of(&m, "author").to_owned()];
    for u in value_list(&m["assignees"])
        .into_iter()
        .chain(value_list(&m["reviewers"]))
    {
        if !names.contains(&u) {
            names.push(u);
        }
    }
    for (_, n) in all(st, "notes") {
        if str_of(n, "noteable") == key {
            let a = str_of(n, "author").to_owned();
            if !names.contains(&a) {
                names.push(a);
            }
        }
    }
    let items = names.iter().map(|u| user_basic(st, &r.base, u)).collect();
    paged(r, items)
}

fn reviewers(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_mr(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let m = get(st, "merge_requests", &key)
        .cloned()
        .unwrap_or(json!({}));
    let approved = value_list(&m["approved_by"]);
    let items = value_list(&m["reviewers"])
        .iter()
        .map(|u| {
            json!({
                "user": user_basic(st, &r.base, u),
                "state": if approved.contains(u) { "approved" } else { "unreviewed" },
                "created_at": m["created_at"],
            })
        })
        .collect();
    ok(Value::Array(items))
}

fn closes_issues(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_mr(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let m = get(st, "merge_requests", &key)
        .cloned()
        .unwrap_or(json!({}));
    let project = project_of(&key);
    let items = closing_refs(str_of(&m, "description"))
        .iter()
        .map(|iid| format!("{project}#{iid}"))
        .filter(|k| get(st, "issues", k).is_some())
        .map(|k| crate::issues::render(st, &r.base, &k))
        .collect();
    paged(r, items)
}

// ---------------------------------------------------------------------------
// Draft notes (`draft_notes`, id-keyed: noteable, author, note, position,
// in_reply_to_discussion_id, resolve_discussion)

fn render_draft(st: &Store, d: &Value) -> Value {
    let author = str_of(d, "author");
    let mr = get(st, "merge_requests", str_of(d, "noteable"));
    json!({
        "id": d["id"],
        "author_id": user(st, author).and_then(|u| as_u64(&u["id"])).unwrap_or(0),
        "merge_request_id": mr.and_then(|m| as_u64(&m["id"])).unwrap_or(0),
        "resolve_discussion": d.get("resolve_discussion").cloned().unwrap_or(json!(false)),
        "discussion_id": d.get("in_reply_to_discussion_id").cloned().unwrap_or(Value::Null),
        "note": str_of(d, "note"),
        "commit_id": d.get("commit_id").cloned().unwrap_or(Value::Null),
        "line_code": d.get("line_code").cloned().unwrap_or(Value::Null),
        "position": d.get("position").cloned().unwrap_or(Value::Null),
    })
}

fn drafts_of(st: &Store, key: &str) -> Vec<(String, Value)> {
    let current = current_username(st);
    all(st, "draft_notes")
        .into_iter()
        .filter(|(_, d)| str_of(d, "noteable") == key && str_of(d, "author") == current)
        .map(|(k, d)| (k.to_owned(), d.clone()))
        .collect()
}

fn list_drafts(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_mr(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let items = drafts_of(st, &key)
        .iter()
        .map(|(_, d)| render_draft(st, d))
        .collect();
    paged(r, items)
}

fn find_draft(st: &Store, r: &Req) -> Result<(String, Value), Response> {
    let key = require_mr(st, r)?;
    drafts_of(st, &key)
        .into_iter()
        .find(|(k, _)| k == r.p("draft_id"))
        .ok_or_else(|| not_found("Draft Note"))
}

fn get_draft(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    match find_draft(st, r) {
        Ok((_, d)) => ok(render_draft(st, &d)),
        Err(e) => e,
    }
}

fn create_draft(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_mr(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(note) = r
        .s("note")
        .or_else(|| r.s("body"))
        .filter(|n| !n.trim().is_empty())
    else {
        return missing("note");
    };
    if let Some(did) = r.s("in_reply_to_discussion_id") {
        let exists = all(st, "notes")
            .iter()
            .any(|(_, n)| str_of(n, "noteable") == key && str_of(n, "discussion_id") == did);
        if !exists {
            return not_found("Discussion");
        }
    }
    let st = svc_mut(w);
    let id = next_id(st, "draft_notes", 1);
    let author = current_username(st);
    let doc = json!({
        "id": id,
        "noteable": key,
        "author": author,
        "note": note,
        "position": r.body.get("position").cloned().unwrap_or(Value::Null),
        "in_reply_to_discussion_id": r.s("in_reply_to_discussion_id"),
        "resolve_discussion": r.bool("resolve_discussion").unwrap_or(false),
        "commit_id": r.s("commit_id"),
    });
    st.collection_mut("draft_notes")
        .insert(id.to_string(), doc.clone());
    created(render_draft(st, &doc))
}

fn update_draft(w: &mut WorldState, r: &Req) -> Response {
    let (id, _) = match find_draft(store(w), r) {
        Ok(d) => d,
        Err(e) => return e,
    };
    let st = svc_mut(w);
    let doc = st
        .collection_mut("draft_notes")
        .get_mut(&id)
        .expect("draft");
    if let Some(n) = r.s("note").or_else(|| r.s("body")) {
        doc["note"] = json!(n);
    }
    if let Some(p) = r.body.get("position") {
        doc["position"] = p.clone();
    }
    if let Some(b) = r.bool("resolve_discussion") {
        doc["resolve_discussion"] = json!(b);
    }
    let doc = doc.clone();
    ok(render_draft(st, &doc))
}

fn delete_draft(w: &mut WorldState, r: &Req) -> Response {
    let (id, _) = match find_draft(store(w), r) {
        Ok(d) => d,
        Err(e) => return e,
    };
    remove_doc(svc_mut(w), "draft_notes", &id);
    no_content()
}

/// Turns a draft into a real note (thread reply, diff thread or plain note).
fn publish(st: &mut Store, d: &Value, now: &str) -> Value {
    let key = str_of(d, "noteable").to_owned();
    let id = next_id(st, "notes", 1);
    let reply = d
        .get("in_reply_to_discussion_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let position = d.get("position").filter(|p| p.is_object()).cloned();
    let (discussion_id, ty) = match (&reply, &position) {
        (Some(did), _) => {
            let first_type = all(st, "notes")
                .iter()
                .find(|(_, n)| str_of(n, "discussion_id") == did)
                .map(|(_, n)| n.get("type").cloned().unwrap_or(Value::Null))
                .unwrap_or(Value::Null);
            (
                did.clone(),
                if first_type == "DiffNote" {
                    json!("DiffNote")
                } else {
                    json!("DiscussionNote")
                },
            )
        }
        (None, Some(_)) => (fake_sha(&format!("discussion:{id}")), json!("DiffNote")),
        (None, None) => (fake_sha(&format!("discussion:{id}")), Value::Null),
    };
    let resolvable = !ty.is_null();
    let mut doc = json!({
        "id": id,
        "noteable": key,
        "body": str_of(d, "note"),
        "author": str_of(d, "author"),
        "created_at": now,
        "updated_at": now,
        "system": false,
        "internal": false,
        "discussion_id": discussion_id,
        "type": ty,
        "resolvable": resolvable,
        "resolved": false,
    });
    if let Some(p) = position {
        doc["position"] = p;
    }
    st.collection_mut("notes")
        .insert(id.to_string(), doc.clone());
    if d["resolve_discussion"].as_bool().unwrap_or(false)
        && let Some(did) = reply
    {
        let ids: Vec<String> = all(st, "notes")
            .iter()
            .filter(|(_, n)| str_of(n, "discussion_id") == did)
            .map(|(k, _)| (*k).to_owned())
            .collect();
        let author = str_of(d, "author").to_owned();
        for nid in ids {
            if let Some(n) = st.collection_mut("notes").get_mut(&nid)
                && n["resolvable"].as_bool().unwrap_or(false)
            {
                n["resolved"] = json!(true);
                n["resolved_by"] = json!(author);
                n["resolved_at"] = json!(now);
            }
        }
    }
    doc
}

fn publish_draft(w: &mut WorldState, r: &Req) -> Response {
    let (id, d) = match find_draft(store(w), r) {
        Ok(d) => d,
        Err(e) => return e,
    };
    let now = now(w);
    let st = svc_mut(w);
    remove_doc(st, "draft_notes", &id);
    let note = publish(st, &d, &now);
    ok(crate::notes::render_note(st, &r.base, &note))
}

fn bulk_publish(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_mr(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let drafts = drafts_of(st, &key);
    let now = now(w);
    let st = svc_mut(w);
    let current = current_username(st);
    let mut published = Vec::new();
    for (id, d) in &drafts {
        remove_doc(st, "draft_notes", id);
        published.push(publish(st, d, &now));
    }
    if let Some(note) = r.s("note").filter(|n| !n.is_empty()) {
        let d = json!({"noteable": key, "author": current, "note": note});
        published.push(publish(st, &d, &now));
    }
    if let Some(state) = r.s("reviewer_state")
        && state == "approved"
    {
        let doc = st
            .collection_mut("merge_requests")
            .get_mut(&key)
            .expect("mr");
        let mut approved = value_list(&doc["approved_by"]);
        if !approved.contains(&current) {
            approved.push(current.clone());
        }
        doc["approved_by"] = json!(approved);
    }
    let items: Vec<Value> = published
        .iter()
        .map(|n| crate::notes::render_note(st, &r.base, n))
        .collect();
    ok(Value::Array(items))
}
