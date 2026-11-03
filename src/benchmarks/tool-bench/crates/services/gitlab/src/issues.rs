//! Issues, issue links, labels, milestones (project and group).

use axum::{
    Router,
    http::StatusCode,
    response::Response,
    routing::{get as rget, post},
};
use mock_core::{
    store::Store,
    world::{SharedWorld, WorldState},
};
use serde_json::{Value, json};

use crate::common::*;

pub fn routes() -> Router<SharedWorld> {
    Router::new()
        .route("/issues", rget(wrap(list_global_issues)))
        .route("/groups/{id}/issues", rget(wrap(list_group_issues)))
        .route(
            "/projects/{id}/issues",
            rget(wrap(list_project_issues)).post(wrap(create_issue)),
        )
        .route(
            "/projects/{id}/issues/{iid}",
            rget(wrap(get_issue))
                .put(wrap(update_issue))
                .delete(wrap(delete_issue)),
        )
        .route(
            "/projects/{id}/issues/{iid}/links",
            rget(wrap(list_links)).post(wrap(create_link)),
        )
        .route(
            "/projects/{id}/issues/{iid}/links/{link_id}",
            rget(wrap(get_link)).delete(wrap(delete_link)),
        )
        .route(
            "/projects/{id}/issues/{iid}/related_merge_requests",
            rget(wrap(related_mrs)),
        )
        .route(
            "/projects/{id}/issues/{iid}/closed_by",
            rget(wrap(closed_by_mrs)),
        )
        .route(
            "/projects/{id}/labels",
            rget(wrap(list_project_labels)).post(wrap(create_label)),
        )
        .route(
            "/projects/{id}/labels/{label_id}",
            rget(wrap(get_label))
                .put(wrap(update_label))
                .delete(wrap(delete_label)),
        )
        .route("/groups/{id}/labels", rget(wrap(list_group_labels)))
        .route(
            "/projects/{id}/milestones",
            rget(wrap(list_project_milestones)).post(wrap(create_project_milestone)),
        )
        .route(
            "/projects/{id}/milestones/{milestone_id}",
            rget(wrap(get_project_milestone))
                .put(wrap(edit_project_milestone))
                .delete(wrap(delete_project_milestone)),
        )
        .route(
            "/projects/{id}/milestones/{milestone_id}/issues",
            rget(wrap(project_milestone_issues)),
        )
        .route(
            "/projects/{id}/milestones/{milestone_id}/merge_requests",
            rget(wrap(project_milestone_mrs)),
        )
        .route(
            "/projects/{id}/milestones/{milestone_id}/promote",
            post(wrap(promote_milestone)),
        )
        .route(
            "/projects/{id}/milestones/{milestone_id}/burndown_events",
            rget(wrap(project_burndown)),
        )
        .route(
            "/groups/{id}/milestones",
            rget(wrap(list_group_milestones)).post(wrap(create_group_milestone)),
        )
        .route(
            "/groups/{id}/milestones/{milestone_id}",
            rget(wrap(get_group_milestone))
                .put(wrap(edit_group_milestone))
                .delete(wrap(delete_group_milestone)),
        )
        .route(
            "/groups/{id}/milestones/{milestone_id}/issues",
            rget(wrap(group_milestone_issues)),
        )
        .route(
            "/groups/{id}/milestones/{milestone_id}/merge_requests",
            rget(wrap(group_milestone_mrs)),
        )
        .route(
            "/groups/{id}/milestones/{milestone_id}/burndown_events",
            rget(wrap(group_burndown)),
        )
}

// ---------------------------------------------------------------------------
// Rendering

fn time_stats(doc: &Value) -> Value {
    let est = as_u64(&doc["time_estimate"]).unwrap_or(0);
    let spent = as_u64(&doc["total_time_spent"]).unwrap_or(0);
    let human = |s: u64| -> Value {
        if s == 0 {
            return Value::Null;
        }
        let (h, m) = (s / 3600, (s % 3600) / 60);
        let mut out = String::new();
        if h > 0 {
            out.push_str(&format!("{h}h"));
        }
        if m > 0 {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&format!("{m}m"));
        }
        json!(out)
    };
    json!({
        "time_estimate": est,
        "total_time_spent": spent,
        "human_time_estimate": human(est),
        "human_total_time_spent": human(spent),
    })
}

/// Milestone doc key for a global milestone id.
pub(crate) fn milestone_key_by_id(st: &Store, id: &Value) -> Option<String> {
    let id = as_u64(id)?;
    all(st, "milestones")
        .into_iter()
        .find(|(_, m)| as_u64(&m["id"]) == Some(id))
        .map(|(k, _)| k.to_owned())
}

/// Full milestone JSON (project or group milestone by key `<path>%<iid>`).
pub(crate) fn render_milestone(st: &Store, base: &str, key: &str) -> Value {
    let m = get(st, "milestones", key).cloned().unwrap_or(json!({}));
    let path = project_of(key);
    let is_group = get(st, "groups", path).is_some();
    let iid = as_u64(&m["iid"]).unwrap_or_else(|| iid_of(key));
    let due = m.get("due_date").cloned().unwrap_or(Value::Null);
    let expired = due
        .as_str()
        .is_some_and(|d| d < "2026-01-01" && m.get("expired").is_none())
        || m.get("expired").and_then(Value::as_bool).unwrap_or(false);
    let mut out = json!({
        "id": as_u64(&m["id"]).unwrap_or(0),
        "iid": iid,
    });
    if is_group {
        out["group_id"] = json!(group_id(st, path));
    } else {
        out["project_id"] = json!(project_id(st, path));
    }
    merge_into(
        &mut out,
        json!({
            "title": str_of(&m, "title"),
            "description": m.get("description").cloned().unwrap_or(json!("")),
            "state": m.get("state").cloned().unwrap_or(json!("active")),
            "created_at": m.get("created_at").cloned().unwrap_or(json!("2026-01-01T00:00:00.000Z")),
            "updated_at": m.get("updated_at").or(m.get("created_at")).cloned().unwrap_or(json!("2026-01-01T00:00:00.000Z")),
            "due_date": due,
            "start_date": m.get("start_date").cloned().unwrap_or(Value::Null),
            "expired": expired,
            "web_url": if is_group {
                format!("{base}/groups/{path}/-/milestones/{iid}")
            } else {
                format!("{base}/{path}/-/milestones/{iid}")
            },
        }),
    );
    out
}

/// Labels visible to a project: its own plus its ancestor groups'.
pub(crate) fn project_labels(st: &Store, project: &str, ancestors: bool) -> Vec<(String, Value)> {
    let mut scopes = vec![project.to_owned()];
    if ancestors {
        let mut p = parent_path(project);
        while let Some(path) = p {
            if get(st, "groups", path).is_some() {
                scopes.push(path.to_owned());
            }
            p = parent_path(path);
        }
    }
    let mut out = Vec::new();
    for (k, l) in all(st, "labels") {
        if let Some((scope, _)) = k.rsplit_once('~')
            && scopes.iter().any(|s| s == scope)
        {
            out.push((k.to_owned(), l.clone()));
        }
    }
    out
}

fn text_color(color: &str) -> &'static str {
    let hex = color.trim_start_matches('#');
    let parse =
        |i: usize| u32::from_str_radix(hex.get(i..i + 2).unwrap_or("ff"), 16).unwrap_or(255);
    let lum = (parse(0) * 299 + parse(2) * 587 + parse(4) * 114) / 1000;
    if lum > 150 { "#1F1E24" } else { "#FFFFFF" }
}

pub(crate) fn render_label(st: &Store, key: &str, counts: bool) -> Value {
    let l = get(st, "labels", key).cloned().unwrap_or(json!({}));
    let scope = key.rsplit_once('~').map_or("", |(s, _)| s);
    let name = str_of(&l, "name").to_owned();
    let color = l
        .get("color")
        .and_then(Value::as_str)
        .unwrap_or("#6699cc")
        .to_owned();
    let description = l.get("description").cloned().unwrap_or(Value::Null);
    let mut out = json!({
        "id": as_u64(&l["id"]).unwrap_or(0),
        "name": name,
        "color": color,
        "text_color": text_color(&color),
        "description": description,
        "description_html": description.as_str().map(|d| json!(format!("<p>{d}</p>"))).unwrap_or(Value::Null),
        "subscribed": false,
        "priority": l.get("priority").cloned().unwrap_or(Value::Null),
        "is_project_label": get(st, "projects", scope).is_some(),
        "archived": false,
    });
    if counts {
        let in_scope = |k: &str| {
            let p = project_of(k);
            p == scope || p.starts_with(&format!("{scope}/"))
        };
        let has = |d: &Value| value_list(&d["labels"]).contains(&name);
        let issues = all(st, "issues");
        out["open_issues_count"] = json!(
            issues
                .iter()
                .filter(|(k, i)| in_scope(k) && has(i) && str_of(i, "state") == "opened")
                .count()
        );
        out["closed_issues_count"] = json!(
            issues
                .iter()
                .filter(|(k, i)| in_scope(k) && has(i) && str_of(i, "state") == "closed")
                .count()
        );
        out["open_merge_requests_count"] = json!(
            all(st, "merge_requests")
                .iter()
                .filter(|(k, m)| in_scope(k) && has(m) && str_of(m, "state") == "opened")
                .count()
        );
    }
    out
}

/// Label objects for names on a project document (for `with_labels_details`).
fn label_details(st: &Store, project: &str, names: &[String]) -> Value {
    let labels = project_labels(st, project, true);
    Value::Array(
        names
            .iter()
            .map(|n| {
                labels
                    .iter()
                    .find(|(_, l)| str_of(l, "name") == n)
                    .map(|(k, _)| {
                        let mut l = render_label(st, k, false);
                        if let Some(o) = l.as_object_mut() {
                            o.remove("archived");
                        }
                        l
                    })
                    .unwrap_or_else(|| json!({"id": 0, "name": n, "color": "#6699cc", "text_color": "#FFFFFF", "description": null, "description_html": null}))
            })
            .collect(),
    )
}

/// Full issue JSON for an issue key (`<project>#<iid>`).
pub(crate) fn render(st: &Store, base: &str, key: &str) -> Value {
    render_with(st, base, key, false)
}

pub(crate) fn render_with(st: &Store, base: &str, key: &str, label_objs: bool) -> Value {
    let i = get(st, "issues", key).cloned().unwrap_or(json!({}));
    let project = project_of(key);
    let pid = project_id(st, project);
    let iid = as_u64(&i["iid"]).unwrap_or_else(|| iid_of(key));
    let assignees = users_list(st, base, &i["assignees"]);
    let first = assignees.get(0).cloned().unwrap_or(Value::Null);
    let labels = value_list(&i["labels"]);
    let milestone = milestone_key_by_id(st, &i["milestone_id"])
        .map(|k| render_milestone(st, base, &k))
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
    let mrs = all(st, "merge_requests")
        .iter()
        .filter(|(k, m)| {
            project_of(k) == project && str_of(m, "description").contains(&format!("#{iid}"))
        })
        .count();
    let issue_type = i
        .get("issue_type")
        .and_then(Value::as_str)
        .unwrap_or("issue");
    let ty = match issue_type {
        "incident" => "INCIDENT",
        "task" => "TASK",
        "test_case" => "TEST_CASE",
        _ => "ISSUE",
    };
    let short = format!("#{iid}");
    let url_kind = if issue_type == "incident" {
        "issues/incident"
    } else {
        "issues"
    };
    let description = i.get("description").cloned().unwrap_or(json!(""));
    let tasks_total = description.as_str().map_or(0, |d| {
        d.matches("- [ ]").count() + d.matches("- [x]").count()
    });
    let tasks_done = description
        .as_str()
        .map_or(0, |d| d.matches("- [x]").count());
    json!({
        "id": as_u64(&i["id"]).unwrap_or(0),
        "iid": iid,
        "project_id": pid,
        "title": str_of(&i, "title"),
        "description": description,
        "state": i.get("state").cloned().unwrap_or(json!("opened")),
        "created_at": i.get("created_at").cloned().unwrap_or(Value::Null),
        "updated_at": i.get("updated_at").or(i.get("created_at")).cloned().unwrap_or(Value::Null),
        "closed_at": i.get("closed_at").cloned().unwrap_or(Value::Null),
        "closed_by": user_opt(st, base, &i["closed_by"]),
        "labels": if label_objs { label_details(st, project, &labels) } else { json!(labels) },
        "milestone": milestone,
        "assignees": assignees,
        "author": user_basic(st, base, str_of(&i, "author")),
        "type": ty,
        "assignee": first,
        "user_notes_count": notes,
        "merge_requests_count": mrs,
        "upvotes": award("thumbsup"),
        "downvotes": award("thumbsdown"),
        "due_date": i.get("due_date").cloned().unwrap_or(Value::Null),
        "confidential": i.get("confidential").cloned().unwrap_or(json!(false)),
        "discussion_locked": i.get("discussion_locked").cloned().unwrap_or(Value::Null),
        "issue_type": issue_type,
        "web_url": format!("{base}/{project}/-/{url_kind}/{iid}"),
        "time_stats": time_stats(&i),
        "task_completion_status": {"count": tasks_total, "completed_count": tasks_done},
        "weight": i.get("weight").cloned().unwrap_or(Value::Null),
        "blocking_issues_count": all(st, "issue_links").iter().filter(|(_, l)| str_of(l, "target") == key && str_of(l, "link_type") == "blocks").count(),
        "has_tasks": tasks_total > 0,
        "task_status": format!("{tasks_done} of {tasks_total} checklist items completed"),
        "_links": {
            "self": format!("{base}/api/v4/projects/{pid}/issues/{iid}"),
            "notes": format!("{base}/api/v4/projects/{pid}/issues/{iid}/notes"),
            "award_emoji": format!("{base}/api/v4/projects/{pid}/issues/{iid}/award_emoji"),
            "project": format!("{base}/api/v4/projects/{pid}"),
            "closed_as_duplicate_of": null,
        },
        "references": {"short": short, "relative": short, "full": format!("{project}#{iid}")},
        "severity": i.get("severity").cloned().unwrap_or(json!("UNKNOWN")),
        "subscribed": false,
        "moved_to_id": i.get("moved_to_id").cloned().unwrap_or(Value::Null),
        "imported": false,
        "imported_from": "none",
        "service_desk_reply_to": null,
        "epic_iid": null,
        "epic": null,
        "iteration": null,
        "health_status": i.get("health_status").cloned().unwrap_or(Value::Null),
    })
}

// ---------------------------------------------------------------------------
// Shared list filtering (issues and merge requests)

fn date_ok(v: &str, after: Option<&str>, before: Option<&str>) -> bool {
    after.is_none_or(|a| v >= a) && before.is_none_or(|b| v <= b)
}

/// GitLab's issue/MR list filters over a compact doc keyed `key`.
pub(crate) fn list_filter(st: &Store, r: &Req, key: &str, d: &Value, is_mr: bool) -> bool {
    let current = current_username(st);
    let state = r.q("state").unwrap_or("all");
    if state != "all" && str_of(d, "state") != state {
        return false;
    }
    if let Some(labels) = r.list("labels") {
        let have = value_list(&d["labels"]);
        for l in &labels {
            match l.as_str() {
                "None" | "none" => {
                    if !have.is_empty() {
                        return false;
                    }
                }
                "Any" | "any" => {
                    if have.is_empty() {
                        return false;
                    }
                }
                name => {
                    if !have.iter().any(|h| h.eq_ignore_ascii_case(name)) {
                        return false;
                    }
                }
            }
        }
    }
    if let Some(not_labels) = r.list("not[labels]") {
        let have = value_list(&d["labels"]);
        if not_labels.iter().any(|l| have.contains(l)) {
            return false;
        }
    }
    if let Some(ms) = r.q("milestone") {
        let mkey = milestone_key_by_id(st, &d["milestone_id"]);
        let title = mkey
            .as_ref()
            .and_then(|k| get(st, "milestones", k))
            .map(|m| str_of(m, "title").to_owned());
        let ok = match ms {
            "None" | "none" => title.is_none(),
            "Any" | "any" => title.is_some(),
            t => title.as_deref() == Some(t),
        };
        if !ok {
            return false;
        }
    }
    if let Some(mid) = r.q("milestone_id") {
        let ok = match mid {
            "None" => d["milestone_id"].is_null(),
            "Any" => !d["milestone_id"].is_null(),
            id => as_u64(&d["milestone_id"]).map(|n| n.to_string()).as_deref() == Some(id),
        };
        if !ok {
            return false;
        }
    }
    let author = str_of(d, "author");
    if let Some(a) = r.q("author_username")
        && author != a
    {
        return false;
    }
    if let Some(a) = r.q("author_id")
        && resolve_username(st, a).as_deref() != Some(author)
    {
        return false;
    }
    let assignees = value_list(&d["assignees"]);
    let assignee_names = r.list("assignee_username").filter(|v| !v.is_empty());
    if let Some(names) = assignee_names
        && !names.iter().all(|n| assignees.contains(n))
    {
        return false;
    }
    if let Some(a) = r.q("assignee_id") {
        let ok = match a {
            "None" | "none" => assignees.is_empty(),
            "Any" | "any" => !assignees.is_empty(),
            id => resolve_username(st, id).is_some_and(|u| assignees.contains(&u)),
        };
        if !ok {
            return false;
        }
    }
    if is_mr {
        let reviewers = value_list(&d["reviewers"]);
        if let Some(rv) = r.q("reviewer_username")
            && !reviewers.iter().any(|x| x == rv)
        {
            return false;
        }
        if let Some(rv) = r.q("reviewer_id") {
            let ok = match rv {
                "None" => reviewers.is_empty(),
                "Any" => !reviewers.is_empty(),
                id => resolve_username(st, id).is_some_and(|u| reviewers.contains(&u)),
            };
            if !ok {
                return false;
            }
        }
        if let Some(approvers) = r.list("approved_by_usernames") {
            let approved = value_list(&d["approved_by"]);
            if !approvers.iter().all(|a| approved.contains(a)) {
                return false;
            }
        }
        for f in ["source_branch", "target_branch"] {
            if let Some(v) = r.q(f)
                && str_of(d, f) != v
            {
                return false;
            }
        }
        if let Some(wip) = r.q("wip").or(r.q("draft")) {
            let draft = d.get("draft").and_then(Value::as_bool).unwrap_or(false)
                || str_of(d, "title").starts_with("Draft:");
            if (wip == "yes" || wip == "true") != draft {
                return false;
            }
        }
    } else {
        if let Some(t) = r.q("issue_type")
            && d.get("issue_type")
                .and_then(Value::as_str)
                .unwrap_or("issue")
                != t
        {
            return false;
        }
        if let Some(c) = r.q("confidential")
            && (c == "true")
                != d.get("confidential")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
        {
            return false;
        }
        if let Some(due) = r.q("due_date") {
            let dd = d.get("due_date").and_then(Value::as_str);
            let ok = match due {
                "0" | "none" => dd.is_none(),
                "any" => dd.is_some(),
                "overdue" => dd.is_some_and(|x| x < "2026-01-01"),
                _ => true,
            };
            if !ok {
                return false;
            }
        }
    }
    if let Some(iids) = r.list("iids") {
        let iid = as_u64(&d["iid"]).unwrap_or_else(|| iid_of(key)).to_string();
        if !iids.contains(&iid) {
            return false;
        }
    }
    if let Some(s) = r.q("search").map(str::to_lowercase) {
        let in_title = str_of(d, "title").to_lowercase().contains(&s);
        let in_desc = str_of(d, "description").to_lowercase().contains(&s);
        let ok = match r.q("in") {
            Some("title") => in_title,
            Some("description") => in_desc,
            _ => in_title || in_desc,
        };
        if !ok {
            return false;
        }
    }
    let created = str_of(d, "created_at");
    let updated = d
        .get("updated_at")
        .and_then(Value::as_str)
        .unwrap_or(created);
    if !date_ok(created, r.q("created_after"), r.q("created_before"))
        || !date_ok(updated, r.q("updated_after"), r.q("updated_before"))
    {
        return false;
    }
    match r.q("scope") {
        Some("created_by_me" | "created-by-me") => author == current,
        Some("assigned_to_me" | "assigned-to-me") => assignees.contains(&current),
        Some("reviews_for_me") => value_list(&d["reviewers"]).contains(&current),
        _ => true,
    }
}

/// Sorts list keys by `order_by` (default `created_at`) and `sort` (default desc).
pub(crate) fn sort_keys(st: &Store, r: &Req, collection: &str, keys: &mut [String]) {
    let order_by = r.q("order_by").unwrap_or("created_at");
    let asc = r.q("sort") == Some("asc");
    keys.sort_by(|a, b| {
        let da = get(st, collection, a).cloned().unwrap_or(json!({}));
        let db = get(st, collection, b).cloned().unwrap_or(json!({}));
        let field = match order_by {
            "updated_at" => "updated_at",
            "title" => "title",
            "due_date" => "due_date",
            "merged_at" => "merged_at",
            _ => "created_at",
        };
        let ord = str_of(&da, field)
            .cmp(str_of(&db, field))
            .then(as_u64(&da["id"]).cmp(&as_u64(&db["id"])));
        if asc { ord } else { ord.reverse() }
    });
}

fn list_issues_in(st: &Store, r: &Req, scope: impl Fn(&str) -> bool) -> Response {
    let mut keys: Vec<String> = all(st, "issues")
        .into_iter()
        .filter(|(k, d)| scope(project_of(k)) && list_filter(st, r, k, d, false))
        .map(|(k, _)| k.to_owned())
        .collect();
    sort_keys(st, r, "issues", &mut keys);
    let details = r.q("with_labels_details") == Some("true");
    let items = keys
        .iter()
        .map(|k| render_with(st, &r.base, k, details))
        .collect();
    paged(r, items)
}

fn list_global_issues(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    if r.q("scope").is_none() {
        // GitLab defaults /issues to the caller's own issues.
        let mut q = r.clone();
        q.query.push(("scope".into(), "created_by_me".into()));
        let any_filter = r.q("assignee_username").is_some()
            || r.list("assignee_username").is_some()
            || r.q("assignee_id").is_some()
            || r.q("author_username").is_some()
            || r.q("author_id").is_some();
        if any_filter {
            q.query.pop();
            q.query.push(("scope".into(), "all".into()));
        }
        return list_issues_in(st, &q, |_| true);
    }
    list_issues_in(st, r, |_| true)
}

fn list_group_issues(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let g = match require_group(st, r) {
        Ok(g) => g,
        Err(e) => return e,
    };
    let prefix = format!("{g}/");
    list_issues_in(st, r, |p| p.starts_with(&prefix))
}

fn list_project_issues(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    list_issues_in(st, r, |p| p == key)
}

/// Issue key for `/projects/{id}/issues/{iid}`, or the 404.
pub(crate) fn require_issue(st: &Store, r: &Req) -> Result<String, Response> {
    let project = require_project(st, r)?;
    let key = format!("{project}#{}", r.p("iid"));
    if get(st, "issues", &key).is_some() {
        Ok(key)
    } else {
        Err(not_found("Issue"))
    }
}

fn get_issue(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    match require_issue(st, r) {
        Ok(k) => ok(render(st, &r.base, &k)),
        Err(e) => e,
    }
}

/// Resolves label names for a project, creating missing project labels like GitLab does.
pub(crate) fn ensure_labels(st: &mut Store, project: &str, names: &[String]) -> Vec<String> {
    let existing = project_labels(st, project, true);
    let mut out = Vec::new();
    for n in names {
        if let Some((_, l)) = existing
            .iter()
            .find(|(_, l)| str_of(l, "name").eq_ignore_ascii_case(n))
        {
            out.push(str_of(l, "name").to_owned());
        } else {
            let id = next_id(st, "labels", 1);
            st.collection_mut("labels").insert(
                format!("{project}~{n}"),
                json!({"id": id, "name": n, "color": "#6699cc", "description": null}),
            );
            out.push(n.clone());
        }
    }
    out.dedup();
    out
}

/// Usernames for `assignee_ids` / `reviewer_ids` (unknown ids are dropped, like GitLab).
pub(crate) fn usernames_for_ids(st: &Store, ids: &[String]) -> Vec<String> {
    ids.iter()
        .filter(|id| id.as_str() != "0")
        .filter_map(|id| resolve_username(st, id))
        .collect()
}

/// Validates a milestone id for a project (own or ancestor group milestone).
pub(crate) fn milestone_for_project(st: &Store, project: &str, id: &str) -> Option<u64> {
    let key = milestone_key_by_id(st, &json!(id))?;
    let scope = project_of(&key);
    (scope == project || project.starts_with(&format!("{scope}/")))
        .then(|| as_u64(&json!(id)))
        .flatten()
}

fn create_issue(w: &mut WorldState, r: &Req) -> Response {
    let project = match require_project(store(w), r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(title) = r.s("title").filter(|t| !t.trim().is_empty()) else {
        return missing("title");
    };
    let st = store(w);
    let milestone = match r.s("milestone_id").filter(|m| !m.is_empty() && m != "0") {
        Some(m) => match milestone_for_project(st, &project, &m) {
            Some(id) => json!(id),
            None => return not_found("Milestone"),
        },
        None => Value::Null,
    };
    let assignees = match r.list("assignee_ids") {
        Some(ids) => usernames_for_ids(st, &ids),
        None => r
            .s("assignee_id")
            .map(|id| usernames_for_ids(st, &[id]))
            .unwrap_or_default(),
    };
    if let Some(t) = r.s("issue_type")
        && !["issue", "incident", "test_case", "task"].contains(&t.as_str())
    {
        return bad_request("issue_type does not have a valid value");
    }
    let now = now(w);
    let st = svc_mut(w);
    let labels = ensure_labels(st, &project, &r.list("labels").unwrap_or_default());
    let iid = next_iid(st, "issues", &project, '#');
    let id = next_id(st, "issues", 1);
    let author = current_username(st);
    let key = format!("{project}#{iid}");
    let doc = json!({
        "id": id,
        "iid": iid,
        "title": title,
        "description": r.s("description").unwrap_or_default(),
        "state": "opened",
        "author": author,
        "assignees": assignees,
        "labels": labels,
        "milestone_id": milestone,
        "created_at": r.s("created_at").unwrap_or_else(|| now.clone()),
        "updated_at": now,
        "closed_at": null,
        "confidential": r.bool("confidential").unwrap_or(false),
        "due_date": r.s("due_date"),
        "weight": r.int("weight"),
        "issue_type": r.s("issue_type").unwrap_or_else(|| "issue".into()),
        "discussion_locked": null,
    });
    st.collection_mut("issues").insert(key.clone(), doc);
    crate::notes::record_event(
        st, &project, &author, "opened", "Issue", id, iid, &title, &now,
    );
    created(render(st, &r.base, &key))
}

fn update_issue(w: &mut WorldState, r: &Req) -> Response {
    let key = match require_issue(store(w), r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let project = project_of(&key).to_owned();
    let st = store(w);
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
        .map(|ids| usernames_for_ids(st, &ids));
    if let Some(ev) = r.s("state_event")
        && !["close", "reopen"].contains(&ev.as_str())
    {
        return bad_request("state_event does not have a valid value");
    }
    let now = now(w);
    let st = svc_mut(w);
    let current = current_username(st);
    let labels = r.list("labels").map(|l| ensure_labels(st, &project, &l));
    let add = r
        .list("add_labels")
        .map(|l| ensure_labels(st, &project, &l));
    let remove = r.list("remove_labels");
    let mut system: Vec<String> = Vec::new();
    let doc = st.collection_mut("issues").get_mut(&key).expect("issue");
    for f in ["title", "description", "due_date"] {
        if let Some(v) = r.body.get(f) {
            if f == "title" && v.as_str().is_some_and(|t| t.trim().is_empty()) {
                continue;
            }
            if f == "title" && v != &doc[f] {
                system.push(format!(
                    "changed title from **{}** to **{}**",
                    str_of(doc, "title"),
                    v.as_str().unwrap_or("")
                ));
            }
            if f == "description" && v != &doc[f] {
                system.push("changed the description".into());
            }
            doc[f] = v.clone();
        }
    }
    for f in ["confidential", "discussion_locked"] {
        if let Some(b) = r.bool(f) {
            doc[f] = json!(b);
        }
    }
    if let Some(wt) = r.int("weight") {
        doc["weight"] = json!(wt);
    }
    if let Some(t) = r.s("issue_type") {
        doc["issue_type"] = json!(t);
    }
    if let Some(m) = milestone {
        system.push(if m.is_null() {
            "removed milestone".into()
        } else {
            "changed milestone".into()
        });
        doc["milestone_id"] = m;
    }
    if let Some(a) = assignees {
        if a.is_empty() {
            system.push("unassigned".into());
        } else {
            system.push(format!(
                "assigned to {}",
                a.iter()
                    .map(|u| format!("@{u}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        doc["assignees"] = json!(a);
    }
    if let Some(l) = labels {
        doc["labels"] = json!(l);
        system.push("changed labels".into());
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
        system.push("removed labels".into());
    }
    match r.s("state_event").as_deref() {
        Some("close") if str_of(doc, "state") != "closed" => {
            doc["state"] = json!("closed");
            doc["closed_at"] = json!(now);
            doc["closed_by"] = json!(current);
            system.push("closed".into());
        }
        Some("reopen") if str_of(doc, "state") == "closed" => {
            doc["state"] = json!("opened");
            doc["closed_at"] = Value::Null;
            doc["closed_by"] = Value::Null;
            system.push("reopened".into());
        }
        _ => {}
    }
    doc["updated_at"] = json!(now);
    for body in system {
        crate::notes::add_system_note(st, &key, &current, &body, &now);
    }
    ok(render(st, &r.base, &key))
}

fn delete_issue(w: &mut WorldState, r: &Req) -> Response {
    let key = match require_issue(store(w), r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let st = svc_mut(w);
    let current = current_username(st);
    let project = project_of(&key).to_owned();
    if member_access(st, &project, &current).unwrap_or(0) < 50
        && parent_path(&project) != Some(current.as_str())
    {
        return forbidden();
    }
    remove_doc(st, "issues", &key);
    no_content()
}

// ---------------------------------------------------------------------------
// Issue links

fn inverse(link_type: &str) -> &'static str {
    match link_type {
        "blocks" => "is_blocked_by",
        "is_blocked_by" => "blocks",
        _ => "relates_to",
    }
}

fn link_view(st: &Store, key: &str) -> Vec<(Value, String, String)> {
    // (link doc, other issue key, link type from this issue's perspective)
    let mut out = Vec::new();
    for (_, l) in all(st, "issue_links") {
        let (src, tgt, ty) = (
            str_of(l, "source"),
            str_of(l, "target"),
            str_of(l, "link_type"),
        );
        if src == key {
            out.push((l.clone(), tgt.to_owned(), ty.to_owned()));
        } else if tgt == key {
            out.push((l.clone(), src.to_owned(), inverse(ty).to_owned()));
        }
    }
    out
}

fn list_links(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_issue(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let items: Vec<Value> = link_view(st, &key)
        .into_iter()
        .filter(|(_, other, _)| get(st, "issues", other).is_some())
        .map(|(l, other, ty)| {
            let mut i = render(st, &r.base, &other);
            merge_into(
                &mut i,
                json!({
                    "issue_link_id": l["id"],
                    "link_type": ty,
                    "link_created_at": l["created_at"],
                    "link_updated_at": l.get("updated_at").unwrap_or(&l["created_at"]),
                }),
            );
            i
        })
        .collect();
    ok(Value::Array(items))
}

fn link_json(st: &Store, base: &str, l: &Value) -> Value {
    json!({
        "id": l["id"],
        "source_issue": render(st, base, str_of(l, "source")),
        "target_issue": render(st, base, str_of(l, "target")),
        "link_type": str_of(l, "link_type"),
        "created_at": l["created_at"],
        "updated_at": l.get("updated_at").unwrap_or(&l["created_at"]),
    })
}

fn find_link(st: &Store, key: &str, link_id: &str) -> Option<(String, Value)> {
    all(st, "issue_links")
        .into_iter()
        .find(|(k, l)| *k == link_id && (str_of(l, "source") == key || str_of(l, "target") == key))
        .map(|(k, l)| (k.to_owned(), l.clone()))
}

fn get_link(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_issue(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    match find_link(st, &key, r.p("link_id")) {
        Some((_, l)) => ok(link_json(st, &r.base, &l)),
        None => error(axum::http::StatusCode::NOT_FOUND, "404 Not found"),
    }
}

fn create_link(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_issue(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(tp) = r.s("target_project_id") else {
        return missing("target_project_id");
    };
    let Some(ti) = r.s("target_issue_iid") else {
        return missing("target_issue_iid");
    };
    let Some(tproject) = project_key(st, &tp) else {
        return project_not_found();
    };
    let target = format!("{tproject}#{ti}");
    if get(st, "issues", &target).is_none() {
        return not_found("Issue");
    }
    let link_type = r.s("link_type").unwrap_or_else(|| "relates_to".into());
    if !["relates_to", "blocks", "is_blocked_by"].contains(&link_type.as_str()) {
        return bad_request("link_type does not have a valid value");
    }
    if target == key {
        return json_status(
            StatusCode::BAD_REQUEST,
            json!({"message": "Cannot link an issue to itself"}),
        );
    }
    if link_view(st, &key).iter().any(|(_, o, _)| *o == target) {
        return json_status(
            StatusCode::CONFLICT,
            json!({"message": "Issue(s) already assigned"}),
        );
    }
    let now = now(w);
    let st = svc_mut(w);
    let id = next_id(st, "issue_links", 1);
    let doc = json!({"id": id, "source": key, "target": target, "link_type": link_type, "created_at": now, "updated_at": now});
    st.collection_mut("issue_links")
        .insert(id.to_string(), doc.clone());
    let current = current_username(st);
    crate::notes::add_system_note(
        st,
        &key,
        &current,
        &format!("marked this issue as related to {target}"),
        &now,
    );
    created(link_json(st, &r.base, &doc))
}

fn delete_link(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_issue(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some((lk, l)) = find_link(st, &key, r.p("link_id")) else {
        return error(axum::http::StatusCode::NOT_FOUND, "404 Not found");
    };
    let st = svc_mut(w);
    remove_doc(st, "issue_links", &lk);
    ok(link_json(st, &r.base, &l))
}

fn related_mrs(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_issue(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let project = project_of(&key);
    let iid = iid_of(&key);
    let items = all(st, "merge_requests")
        .into_iter()
        .filter(|(k, m)| {
            project_of(k) == project && str_of(m, "description").contains(&format!("#{iid}"))
        })
        .map(|(k, _)| crate::merge_requests::render(st, &r.base, k))
        .collect();
    paged(r, items)
}

fn closed_by_mrs(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_issue(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let project = project_of(&key);
    let iid = iid_of(&key);
    let items = all(st, "merge_requests")
        .into_iter()
        .filter(|(k, m)| {
            let d = str_of(m, "description").to_lowercase();
            project_of(k) == project
                && (d.contains(&format!("closes #{iid}")) || d.contains(&format!("fixes #{iid}")))
        })
        .map(|(k, _)| crate::merge_requests::render(st, &r.base, k))
        .collect();
    paged(r, items)
}

// ---------------------------------------------------------------------------
// Labels

fn find_label(st: &Store, project: &str, id_or_name: &str) -> Option<String> {
    project_labels(st, project, true)
        .into_iter()
        .find(|(_, l)| {
            as_u64(&l["id"]).map(|n| n.to_string()).as_deref() == Some(id_or_name)
                || str_of(l, "name") == id_or_name
        })
        .map(|(k, _)| k)
}

fn list_project_labels(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let ancestors = r.q("include_ancestor_groups") != Some("false");
    let counts = r.q("with_counts") == Some("true");
    let search = r.q("search").map(str::to_lowercase);
    let mut labels = project_labels(st, &key, ancestors);
    labels.sort_by(|a, b| {
        str_of(&a.1, "name")
            .to_lowercase()
            .cmp(&str_of(&b.1, "name").to_lowercase())
    });
    let items = labels
        .iter()
        .filter(|(_, l)| {
            search
                .as_ref()
                .is_none_or(|s| str_of(l, "name").to_lowercase().contains(s))
        })
        .map(|(k, _)| render_label(st, k, counts))
        .collect();
    paged(r, items)
}

fn list_group_labels(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let g = match require_group(st, r) {
        Ok(g) => g,
        Err(e) => return e,
    };
    let counts = r.q("with_counts") == Some("true");
    let mut scopes = vec![g.clone()];
    if r.q("include_ancestor_groups") != Some("false") {
        let mut p = parent_path(&g);
        while let Some(path) = p {
            scopes.push(path.to_owned());
            p = parent_path(path);
        }
    }
    let items = all(st, "labels")
        .into_iter()
        .filter(|(k, _)| {
            k.rsplit_once('~')
                .is_some_and(|(s, _)| scopes.iter().any(|x| x == s))
        })
        .map(|(k, _)| render_label(st, k, counts))
        .collect();
    paged(r, items)
}

fn get_label(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    match find_label(st, &key, r.p("label_id")) {
        Some(l) => ok(render_label(st, &l, true)),
        None => not_found("Label"),
    }
}

fn valid_color(c: &str) -> bool {
    let hex = c.strip_prefix('#');
    hex.is_some_and(|h| (h.len() == 6 || h.len() == 3) && h.chars().all(|c| c.is_ascii_hexdigit()))
        || c.chars().all(|c| c.is_ascii_alphabetic())
}

fn create_label(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(name) = r.s("name") else {
        return missing("name");
    };
    let Some(color) = r.s("color") else {
        return missing("color");
    };
    if !valid_color(&color) {
        return invalid("color", "must be a valid color code");
    }
    if get(st, "labels", &format!("{key}~{name}")).is_some() {
        return json_status(
            StatusCode::CONFLICT,
            json!({"message": "Label already exists"}),
        );
    }
    let st = svc_mut(w);
    let id = next_id(st, "labels", 1);
    let lk = format!("{key}~{name}");
    st.collection_mut("labels").insert(
        lk.clone(),
        json!({
            "id": id,
            "name": name,
            "color": color,
            "description": r.s("description"),
            "priority": r.int("priority"),
        }),
    );
    created(render_label(st, &lk, true))
}

fn update_label(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(lk) = find_label(st, &key, r.p("label_id")) else {
        return not_found("Label");
    };
    if project_of(&lk) != key {
        return forbidden();
    }
    if !r.has("new_name") && !r.has("color") && !r.has("description") && !r.has("priority") {
        return bad_request(
            "new_name, color, description, priority are missing, at least one parameter must be provided",
        );
    }
    if let Some(c) = r.s("color")
        && !valid_color(&c)
    {
        return invalid("color", "must be a valid color code");
    }
    let st = svc_mut(w);
    let mut doc = remove_doc(st, "labels", &lk).expect("label");
    let old_name = str_of(&doc, "name").to_owned();
    if let Some(c) = r.s("color") {
        doc["color"] = json!(c);
    }
    if r.has("description") {
        doc["description"] = r.arg("description").unwrap_or(Value::Null);
    }
    if r.has("priority") {
        doc["priority"] = r.int("priority").map(|p| json!(p)).unwrap_or(Value::Null);
    }
    let new_name = r.s("new_name").unwrap_or_else(|| old_name.clone());
    doc["name"] = json!(new_name);
    let nk = format!("{key}~{new_name}");
    st.collection_mut("labels").insert(nk.clone(), doc);
    if new_name != old_name {
        for coll in ["issues", "merge_requests"] {
            let keys: Vec<String> = all(st, coll)
                .iter()
                .filter(|(k, _)| project_of(k) == key)
                .map(|(k, _)| (*k).to_owned())
                .collect();
            for k in keys {
                let d = st.collection_mut(coll).get_mut(&k).expect("doc");
                let labels: Vec<String> = value_list(&d["labels"])
                    .into_iter()
                    .map(|l| if l == old_name { new_name.clone() } else { l })
                    .collect();
                d["labels"] = json!(labels);
            }
        }
    }
    ok(render_label(st, &nk, true))
}

fn delete_label(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(lk) = find_label(st, &key, r.p("label_id")) else {
        return not_found("Label");
    };
    if project_of(&lk) != key {
        return forbidden();
    }
    let st = svc_mut(w);
    let doc = remove_doc(st, "labels", &lk).expect("label");
    let name = str_of(&doc, "name").to_owned();
    for coll in ["issues", "merge_requests"] {
        let keys: Vec<String> = all(st, coll)
            .iter()
            .filter(|(k, _)| project_of(k) == key)
            .map(|(k, _)| (*k).to_owned())
            .collect();
        for k in keys {
            let d = st.collection_mut(coll).get_mut(&k).expect("doc");
            let mut labels = value_list(&d["labels"]);
            labels.retain(|l| *l != name);
            d["labels"] = json!(labels);
        }
    }
    no_content()
}

// ---------------------------------------------------------------------------
// Milestones

fn scope_milestones(st: &Store, scope: &str) -> Vec<String> {
    let prefix = format!("{scope}%");
    all(st, "milestones")
        .into_iter()
        .filter(|(k, _)| k.starts_with(&prefix))
        .map(|(k, _)| k.to_owned())
        .collect()
}

fn list_milestones(st: &Store, r: &Req, scopes: &[String]) -> Response {
    let iids = r.list("iids");
    let mut keys: Vec<String> = scopes
        .iter()
        .flat_map(|s| scope_milestones(st, s))
        .filter(|k| {
            let m = get(st, "milestones", k).cloned().unwrap_or(json!({}));
            let iid = as_u64(&m["iid"]).unwrap_or_else(|| iid_of(k)).to_string();
            r.q("state")
                .is_none_or(|s| m.get("state").and_then(Value::as_str).unwrap_or("active") == s)
                && r.q("title").is_none_or(|t| str_of(&m, "title") == t)
                && r.q("search").is_none_or(|s| {
                    let s = s.to_lowercase();
                    str_of(&m, "title").to_lowercase().contains(&s)
                        || str_of(&m, "description").to_lowercase().contains(&s)
                })
                && iids
                    .as_ref()
                    .is_none_or(|ids| ids.is_empty() || ids.contains(&iid))
                && r.q("updated_after")
                    .is_none_or(|a| str_of(&m, "updated_at") >= a)
                && r.q("updated_before")
                    .is_none_or(|b| str_of(&m, "updated_at") <= b)
        })
        .collect();
    keys.sort_by(|a, b| {
        let da = get(st, "milestones", a).cloned().unwrap_or(json!({}));
        let db = get(st, "milestones", b).cloned().unwrap_or(json!({}));
        // GitLab orders milestones by due date, then title.
        let due = |d: &Value| {
            d.get("due_date")
                .and_then(Value::as_str)
                .unwrap_or("9999")
                .to_owned()
        };
        due(&da)
            .cmp(&due(&db))
            .then(str_of(&da, "title").cmp(str_of(&db, "title")))
    });
    let items = keys
        .iter()
        .map(|k| render_milestone(st, &r.base, k))
        .collect();
    paged(r, items)
}

fn ancestor_scopes(st: &Store, path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut p = parent_path(path);
    while let Some(x) = p {
        if get(st, "groups", x).is_some() {
            out.push(x.to_owned());
        }
        p = parent_path(x);
    }
    out
}

fn list_project_milestones(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let mut scopes = vec![key.clone()];
    if r.q("include_ancestors") == Some("true") || r.q("include_parent_milestones") == Some("true")
    {
        scopes.extend(ancestor_scopes(st, &key));
    }
    list_milestones(st, r, &scopes)
}

fn list_group_milestones(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let g = match require_group(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let mut scopes = vec![g.clone()];
    if r.q("include_ancestors") == Some("true") || r.q("include_parent_milestones") == Some("true")
    {
        let mut p = parent_path(&g);
        while let Some(x) = p {
            scopes.push(x.to_owned());
            p = parent_path(x);
        }
    }
    if r.q("include_descendants") == Some("true") {
        let prefix = format!("{g}/");
        for (k, _) in all(st, "groups") {
            if k.starts_with(&prefix) {
                scopes.push(k.to_owned());
            }
        }
    }
    list_milestones(st, r, &scopes)
}

/// Milestone key in `scope` for a global milestone id.
fn scoped_milestone(st: &Store, scope: &str, id: &str) -> Option<String> {
    milestone_key_by_id(st, &json!(id)).filter(|k| project_of(k) == scope)
}

fn require_milestone(st: &Store, r: &Req, group: bool) -> Result<String, Response> {
    let scope = if group {
        require_group(st, r)?
    } else {
        require_project(st, r)?
    };
    scoped_milestone(st, &scope, r.p("milestone_id")).ok_or_else(|| not_found("Milestone"))
}

fn get_project_milestone(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    match require_milestone(st, r, false) {
        Ok(k) => ok(render_milestone(st, &r.base, &k)),
        Err(e) => e,
    }
}

fn get_group_milestone(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    match require_milestone(st, r, true) {
        Ok(k) => ok(render_milestone(st, &r.base, &k)),
        Err(e) => e,
    }
}

fn create_milestone(w: &mut WorldState, r: &Req, group: bool) -> Response {
    let st = store(w);
    let scope = match if group {
        require_group(st, r)
    } else {
        require_project(st, r)
    } {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(title) = r.s("title") else {
        return missing("title");
    };
    if scope_milestones(st, &scope)
        .iter()
        .any(|k| get(st, "milestones", k).is_some_and(|m| str_of(m, "title") == title))
    {
        return invalid(
            "title",
            "already being used for another group or project milestone.",
        );
    }
    let now = now(w);
    let st = svc_mut(w);
    let iid = next_iid(st, "milestones", &scope, '%');
    let id = next_id(st, "milestones", 1);
    let key = format!("{scope}%{iid}");
    st.collection_mut("milestones").insert(
        key.clone(),
        json!({
            "id": id,
            "iid": iid,
            "title": title,
            "description": r.s("description").unwrap_or_default(),
            "state": "active",
            "due_date": r.s("due_date"),
            "start_date": r.s("start_date"),
            "created_at": now,
            "updated_at": now,
        }),
    );
    created(render_milestone(st, &r.base, &key))
}

fn create_project_milestone(w: &mut WorldState, r: &Req) -> Response {
    create_milestone(w, r, false)
}

fn create_group_milestone(w: &mut WorldState, r: &Req) -> Response {
    create_milestone(w, r, true)
}

fn edit_milestone(w: &mut WorldState, r: &Req, group: bool) -> Response {
    let key = match require_milestone(store(w), r, group) {
        Ok(k) => k,
        Err(e) => return e,
    };
    if let Some(ev) = r.s("state_event")
        && !["close", "activate"].contains(&ev.as_str())
    {
        return bad_request("state_event does not have a valid value");
    }
    let now = now(w);
    let st = svc_mut(w);
    let doc = st
        .collection_mut("milestones")
        .get_mut(&key)
        .expect("milestone");
    for f in ["title", "description", "due_date", "start_date"] {
        if let Some(v) = r.body.get(f) {
            doc[f] = v.clone();
        }
    }
    match r.s("state_event").as_deref() {
        Some("close") => doc["state"] = json!("closed"),
        Some("activate") => doc["state"] = json!("active"),
        _ => {}
    }
    doc["updated_at"] = json!(now);
    ok(render_milestone(st, &r.base, &key))
}

fn edit_project_milestone(w: &mut WorldState, r: &Req) -> Response {
    edit_milestone(w, r, false)
}

fn edit_group_milestone(w: &mut WorldState, r: &Req) -> Response {
    edit_milestone(w, r, true)
}

fn delete_milestone(w: &mut WorldState, r: &Req, group: bool) -> Response {
    let key = match require_milestone(store(w), r, group) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let st = svc_mut(w);
    let doc = remove_doc(st, "milestones", &key).expect("milestone");
    for coll in ["issues", "merge_requests"] {
        let keys: Vec<String> = all(st, coll)
            .iter()
            .filter(|(_, d)| as_u64(&d["milestone_id"]) == as_u64(&doc["id"]))
            .map(|(k, _)| (*k).to_owned())
            .collect();
        for k in keys {
            st.collection_mut(coll).get_mut(&k).expect("doc")["milestone_id"] = Value::Null;
        }
    }
    no_content()
}

fn delete_project_milestone(w: &mut WorldState, r: &Req) -> Response {
    delete_milestone(w, r, false)
}

fn delete_group_milestone(w: &mut WorldState, r: &Req) -> Response {
    delete_milestone(w, r, true)
}

fn milestone_items(st: &Store, r: &Req, key: &str, coll: &str) -> Response {
    let id = get(st, "milestones", key).and_then(|m| as_u64(&m["id"]));
    let items = all(st, coll)
        .into_iter()
        .filter(|(_, d)| as_u64(&d["milestone_id"]) == id)
        .map(|(k, _)| {
            if coll == "issues" {
                render(st, &r.base, k)
            } else {
                crate::merge_requests::render(st, &r.base, k)
            }
        })
        .collect();
    paged(r, items)
}

fn project_milestone_issues(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    match require_milestone(st, r, false) {
        Ok(k) => milestone_items(st, r, &k, "issues"),
        Err(e) => e,
    }
}

fn project_milestone_mrs(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    match require_milestone(st, r, false) {
        Ok(k) => milestone_items(st, r, &k, "merge_requests"),
        Err(e) => e,
    }
}

fn group_milestone_issues(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    match require_milestone(st, r, true) {
        Ok(k) => milestone_items(st, r, &k, "issues"),
        Err(e) => e,
    }
}

fn group_milestone_mrs(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    match require_milestone(st, r, true) {
        Ok(k) => milestone_items(st, r, &k, "merge_requests"),
        Err(e) => e,
    }
}

fn promote_milestone(w: &mut WorldState, r: &Req) -> Response {
    let key = match require_milestone(store(w), r, false) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let project = project_of(&key).to_owned();
    let Some(group) = parent_path(&project)
        .filter(|g| get(store(w), "groups", g).is_some())
        .map(str::to_owned)
    else {
        return bad_request("Project does not belong to a group.");
    };
    let now = now(w);
    let st = svc_mut(w);
    let mut doc = remove_doc(st, "milestones", &key).expect("milestone");
    let iid = next_iid(st, "milestones", &group, '%');
    doc["iid"] = json!(iid);
    doc["updated_at"] = json!(now);
    let nk = format!("{group}%{iid}");
    st.collection_mut("milestones").insert(nk.clone(), doc);
    ok(render_milestone(st, &r.base, &nk))
}

fn burndown(st: &Store, r: &Req, key: &str) -> Response {
    let id = get(st, "milestones", key).and_then(|m| as_u64(&m["id"]));
    let mut events = Vec::new();
    for (_, i) in all(st, "issues") {
        if as_u64(&i["milestone_id"]) != id {
            continue;
        }
        let weight = i.get("weight").cloned().unwrap_or(Value::Null);
        events.push(json!({
            "created_at": i["created_at"],
            "weight": weight,
            "action": "created",
        }));
        if let Some(c) = i.get("closed_at").filter(|c| !c.is_null()) {
            events.push(json!({"created_at": c, "weight": weight, "action": "closed"}));
        }
    }
    events.sort_by(|a, b| str_of(a, "created_at").cmp(str_of(b, "created_at")));
    paged(r, events)
}

fn project_burndown(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    match require_milestone(st, r, false) {
        Ok(k) => burndown(st, r, &k),
        Err(e) => e,
    }
}

fn group_burndown(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    match require_milestone(st, r, true) {
        Ok(k) => burndown(st, r, &k),
        Err(e) => e,
    }
}
