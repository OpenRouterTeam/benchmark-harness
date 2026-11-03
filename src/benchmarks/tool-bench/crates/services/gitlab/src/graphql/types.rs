//! GraphQL object materializers and field resolvers.

use mock_core::store::Store;
use serde_json::{Map, Value, json};

use super::{Ctx, R, connection};
use crate::common::*;

// ---------------------------------------------------------------------------
// Ids and enums

/// `(name, type id, enum, issue_type)` for every work item type.
pub(crate) const WORK_ITEM_TYPES: &[(&str, u64, &str, &str)] = &[
    ("Issue", 1, "ISSUE", "issue"),
    ("Incident", 2, "INCIDENT", "incident"),
    ("Test Case", 3, "TEST_CASE", "test_case"),
    ("Requirement", 4, "REQUIREMENT", "requirement"),
    ("Task", 5, "TASK", "task"),
    ("Objective", 6, "OBJECTIVE", "objective"),
    ("Key Result", 7, "KEY_RESULT", "key_result"),
    ("Epic", 8, "EPIC", "epic"),
    ("Ticket", 9, "TICKET", "ticket"),
];

pub(crate) fn type_by_issue_type(t: &str) -> (&'static str, u64, &'static str, &'static str) {
    *WORK_ITEM_TYPES
        .iter()
        .find(|x| x.3 == t)
        .unwrap_or(&WORK_ITEM_TYPES[0])
}

pub(crate) fn work_item_type_obj(t: &str) -> Value {
    let (name, id, _, _) = type_by_issue_type(t);
    json!({"__typename": "WorkItemType", "id": format!("gid://gitlab/WorkItems::Type/{id}"), "name": name, "iconName": format!("issue-type-{}", t.replace('_', "-")), "_issue_type": t})
}

/// Default (system-defined) work item statuses.
pub(crate) const STATUSES: &[(u64, &str, &str, &str, &str)] = &[
    (1, "To do", "TO_DO", "#737278", "status-waiting"),
    (2, "In progress", "IN_PROGRESS", "#1f75cb", "status-running"),
    (3, "Done", "DONE", "#108548", "status-success"),
    (4, "Won't do", "CANCELED", "#DD2B0E", "status-cancelled"),
    (5, "Duplicate", "CANCELED", "#DD2B0E", "status-cancelled"),
];

pub(crate) fn status_obj(s: &(u64, &str, &str, &str, &str)) -> Value {
    json!({
        "__typename": "WorkItemStatus",
        "id": format!("gid://gitlab/WorkItems::Statuses::SystemDefined::Status/{}", s.0),
        "name": s.1,
        "category": s.2.to_lowercase(),
        "color": s.3,
        "iconName": s.4,
        "position": s.0 - 1,
        "description": null,
    })
}

pub(crate) fn gid_num(gid: &str) -> Option<u64> {
    gid.rsplit('/').next()?.parse().ok()
}

pub(crate) fn user_gid(st: &Store, username: &str) -> String {
    format!(
        "gid://gitlab/User/{}",
        user(st, username)
            .and_then(|u| as_u64(&u["id"]))
            .unwrap_or(0)
    )
}

// ---------------------------------------------------------------------------
// Materializers

pub(crate) fn user_obj(ctx: &Ctx, username: &str) -> Value {
    let st = ctx.st;
    let u = user(st, username).cloned().unwrap_or(json!({}));
    json!({
        "__typename": "UserCore",
        "_types": ["User", "UserCore"],
        "id": user_gid(st, username),
        "username": username,
        "name": u.get("name").and_then(Value::as_str).unwrap_or(username),
        "state": u.get("state").and_then(Value::as_str).unwrap_or("active"),
        "avatarUrl": avatar_url(&ctx.base, username),
        "webUrl": format!("{}/{username}", ctx.base),
        "webPath": format!("/{username}"),
        "publicEmail": u.get("public_email").cloned().unwrap_or(Value::Null),
        "bot": u.get("bot").cloned().unwrap_or(json!(false)),
        "location": u.get("location").cloned().unwrap_or(Value::Null),
        "jobTitle": u.get("job_title").cloned().unwrap_or(Value::Null),
        "organization": u.get("organization").cloned().unwrap_or(Value::Null),
        "bio": u.get("bio").cloned().unwrap_or(Value::Null),
        "createdAt": u.get("created_at").cloned().unwrap_or(Value::Null),
        "pronouns": null,
        "status": null,
        "_username": username,
    })
}

fn user_opt_obj(ctx: &Ctx, v: &Value) -> Value {
    match v.as_str() {
        Some(u) if !u.is_empty() => user_obj(ctx, u),
        _ => Value::Null,
    }
}

pub(crate) fn project_obj(ctx: &Ctx, key: &str) -> Value {
    let p = render_project(ctx.st, &ctx.base, key);
    json!({
        "__typename": "Project",
        "_types": ["Project", "Namespace"],
        "_key": key,
        "_kind": "project",
        "id": format!("gid://gitlab/Project/{}", p["id"]),
        "name": p["name"],
        "path": p["path"],
        "fullPath": key,
        "nameWithNamespace": p["name_with_namespace"],
        "description": p["description"],
        "webUrl": p["web_url"],
        "webPath": format!("/{key}"),
        "visibility": p["visibility"],
        "createdAt": p["created_at"],
        "lastActivityAt": p["last_activity_at"],
        "archived": p["archived"],
        "starCount": p["star_count"],
        "forksCount": p["forks_count"],
        "topics": p["topics"],
        "httpUrlToRepo": p["http_url_to_repo"],
        "sshUrlToRepo": p["ssh_url_to_repo"],
        "openIssuesCount": p["open_issues_count"],
        "avatarUrl": null,
        "issuesEnabled": p["issues_enabled"],
        "mergeRequestsEnabled": true,
        "wikiEnabled": p["wiki_enabled"],
        "jobsEnabled": true,
        "squashOption": p["squash_option"],
        "mergeMethod": p["merge_method"],
        "onlyAllowMergeIfPipelineSucceeds": p["only_allow_merge_if_pipeline_succeeds"],
    })
}

pub(crate) fn group_obj(ctx: &Ctx, key: &str) -> Value {
    let g = render_group(ctx.st, &ctx.base, key);
    json!({
        "__typename": "Group",
        "_types": ["Group", "Namespace"],
        "_key": key,
        "_kind": "group",
        "id": format!("gid://gitlab/Group/{}", g["id"]),
        "name": g["name"],
        "path": g["path"],
        "fullPath": key,
        "fullName": g["full_name"],
        "description": g["description"],
        "webUrl": g["web_url"],
        "webPath": format!("/groups/{key}"),
        "visibility": g["visibility"],
        "createdAt": g["created_at"],
        "avatarUrl": null,
        "dependencyProxyImagePrefix": format!("{}/{key}/dependency_proxy/containers", ctx.base.replace("http://", "").replace("https://", "")),
    })
}

/// `namespace(fullPath:)`: the project or group, typed `Namespace`.
pub(crate) fn namespace_obj(ctx: &Ctx, path: &str) -> Value {
    if let Some(k) = project_key(ctx.st, path) {
        let mut o = project_obj(ctx, &k);
        o["__typename"] = json!("Namespace");
        o["_types"] = json!(["Namespace", "Project", "ProjectNamespace"]);
        return o;
    }
    if let Some(k) = group_key(ctx.st, path) {
        let mut o = group_obj(ctx, &k);
        o["__typename"] = json!("Group");
        o["_types"] = json!(["Namespace", "Group"]);
        return o;
    }
    Value::Null
}

fn label_obj(ctx: &Ctx, key: &str) -> Value {
    let l = crate::issues::render_label(ctx.st, key, false);
    let scope = key.rsplit_once('~').map_or("", |(s, _)| s);
    let is_project = get(ctx.st, "projects", scope).is_some();
    json!({
        "__typename": "Label",
        "id": format!("gid://gitlab/{}Label/{}", if is_project { "Project" } else { "Group" }, l["id"]),
        "title": l["name"],
        "color": l["color"],
        "textColor": l["text_color"],
        "description": l["description"],
        "descriptionHtml": l["description_html"],
        "createdAt": null,
        "lockOnMerge": false,
    })
}

fn labels_for(ctx: &Ctx, project: &str, names: &[String]) -> Vec<Value> {
    let labels = crate::issues::project_labels(ctx.st, project, true);
    names
        .iter()
        .filter_map(|n| {
            labels
                .iter()
                .find(|(_, l)| str_of(l, "name") == n)
                .map(|(k, _)| label_obj(ctx, k))
        })
        .collect()
}

fn milestone_obj(ctx: &Ctx, key: &str) -> Value {
    let m = crate::issues::render_milestone(ctx.st, &ctx.base, key);
    json!({
        "__typename": "Milestone",
        "id": format!("gid://gitlab/Milestone/{}", m["id"]),
        "iid": m["iid"].to_string(),
        "title": m["title"],
        "description": m["description"],
        "state": m["state"],
        "dueDate": m["due_date"],
        "startDate": m["start_date"],
        "expired": m["expired"],
        "webUrl": m["web_url"],
        "webPath": m["web_url"].as_str().map(|u| u.replacen(&ctx.base, "", 1)),
        "createdAt": m["created_at"],
        "updatedAt": m["updated_at"],
        "projectMilestone": m.get("project_id").is_some(),
        "groupMilestone": m.get("group_id").is_some(),
    })
}

fn issue_state(doc: &Value) -> &'static str {
    if str_of(doc, "state") == "closed" {
        "CLOSED"
    } else {
        "OPEN"
    }
}

pub(crate) fn work_item_obj(ctx: &Ctx, key: &str) -> Value {
    let Some(doc) = get(ctx.st, "issues", key) else {
        return Value::Null;
    };
    let project = project_of(key);
    let iid = as_u64(&doc["iid"]).unwrap_or_else(|| iid_of(key));
    let issue_type = doc
        .get("issue_type")
        .and_then(Value::as_str)
        .unwrap_or("issue");
    let is_group = get(ctx.st, "groups", project).is_some();
    let web = if is_group {
        format!("{}/groups/{project}/-/work_items/{iid}", ctx.base)
    } else if issue_type == "issue" || issue_type == "incident" {
        format!("{}/{project}/-/issues/{iid}", ctx.base)
    } else {
        format!("{}/{project}/-/work_items/{iid}", ctx.base)
    };
    json!({
        "__typename": "WorkItem",
        "_key": key,
        "id": format!("gid://gitlab/WorkItem/{}", doc["id"]),
        "iid": iid.to_string(),
        "title": str_of(doc, "title"),
        "titleHtml": str_of(doc, "title"),
        "state": issue_state(doc),
        "description": doc.get("description").cloned().unwrap_or(json!("")),
        "webUrl": web,
        "webPath": web.replacen(&ctx.base, "", 1),
        "confidential": doc.get("confidential").cloned().unwrap_or(json!(false)),
        "author": user_opt_obj(ctx, &doc["author"]),
        "createdAt": doc["created_at"],
        "updatedAt": doc.get("updated_at").unwrap_or(&doc["created_at"]),
        "closedAt": doc.get("closed_at").cloned().unwrap_or(Value::Null),
        "workItemType": work_item_type_obj(issue_type),
        "reference": format!("{project}#{iid}"),
        "archived": false,
        "hidden": false,
        "imported": false,
        "lockVersion": 0,
        "movedToWorkItemUrl": null,
        "duplicatedToWorkItemUrl": null,
        "userPermissions": {"__typename": "WorkItemPermissions", "updateWorkItem": true, "deleteWorkItem": true, "adminWorkItem": true, "createNote": true, "readWorkItem": true, "setWorkItemMetadata": true},
    })
}

pub(crate) fn issue_obj(ctx: &Ctx, key: &str) -> Value {
    let Some(doc) = get(ctx.st, "issues", key) else {
        return Value::Null;
    };
    let r = crate::issues::render(ctx.st, &ctx.base, key);
    json!({
        "__typename": "Issue",
        "_key": key,
        "id": format!("gid://gitlab/Issue/{}", doc["id"]),
        "iid": r["iid"].to_string(),
        "title": r["title"],
        "description": r["description"],
        "state": if str_of(doc, "state") == "closed" { "closed" } else { "opened" },
        "webUrl": r["web_url"],
        "webPath": r["web_url"].as_str().map(|u| u.replacen(&ctx.base, "", 1)),
        "author": user_opt_obj(ctx, &doc["author"]),
        "createdAt": r["created_at"],
        "updatedAt": r["updated_at"],
        "closedAt": r["closed_at"],
        "confidential": r["confidential"],
        "dueDate": r["due_date"],
        "weight": r["weight"],
        "severity": doc.get("severity").and_then(Value::as_str).map(str::to_uppercase).unwrap_or_else(|| "UNKNOWN".into()),
        "escalationStatus": doc.get("escalation_status").and_then(Value::as_str).map(str::to_uppercase),
        "type": r["type"],
        "reference": format!("#{}", r["iid"]),
        "userNotesCount": r["user_notes_count"],
        "upvotes": r["upvotes"],
        "downvotes": r["downvotes"],
        "healthStatus": doc.get("health_status").cloned().unwrap_or(Value::Null),
        "discussionLocked": doc.get("discussion_locked").and_then(Value::as_bool).unwrap_or(false),
        "moved": doc.get("moved_to_id").is_some_and(|m| !m.is_null()),
    })
}

pub(crate) fn mr_obj(ctx: &Ctx, key: &str) -> Value {
    let Some(doc) = get(ctx.st, "merge_requests", key) else {
        return Value::Null;
    };
    let r = crate::merge_requests::render(ctx.st, &ctx.base, key);
    let approved_by = value_list(&doc["approved_by"]);
    let required = as_u64(&doc["approvals_required"]).unwrap_or(0);
    json!({
        "__typename": "MergeRequest",
        "_key": key,
        "id": format!("gid://gitlab/MergeRequest/{}", doc["id"]),
        "iid": r["iid"].to_string(),
        "title": r["title"],
        "description": r["description"],
        "state": r["state"],
        "webUrl": r["web_url"],
        "webPath": r["web_url"].as_str().map(|u| u.replacen(&ctx.base, "", 1)),
        "sourceBranch": r["source_branch"],
        "targetBranch": r["target_branch"],
        "author": user_opt_obj(ctx, &doc["author"]),
        "createdAt": r["created_at"],
        "updatedAt": r["updated_at"],
        "mergedAt": r["merged_at"],
        "closedAt": r["closed_at"],
        "draft": r["draft"],
        "conflicts": r["has_conflicts"],
        "mergeable": r["detailed_merge_status"] == "mergeable",
        "detailedMergeStatus": r["detailed_merge_status"].as_str().map(str::to_uppercase),
        "mergeStatusEnum": if r["has_conflicts"] == true { "CANNOT_BE_MERGED" } else { "CAN_BE_MERGED" },
        "approved": approved_by.len() as u64 >= required,
        "approvalsRequired": required,
        "approvalsLeft": required.saturating_sub(approved_by.len() as u64),
        "diffHeadSha": r["sha"],
        "sha": r["sha"],
        "mergeCommitSha": r["merge_commit_sha"],
        "userNotesCount": r["user_notes_count"],
        "upvotes": r["upvotes"],
        "downvotes": r["downvotes"],
        "reference": r["reference"],
        "squash": r["squash"],
        "shouldRemoveSourceBranch": r["should_remove_source_branch"],
        "mergeWhenPipelineSucceeds": r["merge_when_pipeline_succeeds"],
        "discussionLocked": r["discussion_locked"].as_bool().unwrap_or(false),
        "commitCount": crate::merge_requests::mr_commit_shas(ctx.st, key).len(),
        "diffStatsSummary": {"__typename": "DiffStatsSummary", "fileCount": crate::merge_requests::mr_changes(ctx.st, key).len(), "additions": 0, "deletions": 0, "changes": 0},
    })
}

fn note_obj(ctx: &Ctx, n: &Value) -> Value {
    let noteable = str_of(n, "noteable");
    let (nt, nid) = if noteable.contains('!') {
        (
            "MergeRequest",
            get(ctx.st, "merge_requests", noteable).map(|d| d["id"].clone()),
        )
    } else {
        (
            "Issue",
            get(ctx.st, "issues", noteable).map(|d| d["id"].clone()),
        )
    };
    let nid = nid.unwrap_or(json!(0));
    let resolvable = n["resolvable"].as_bool().unwrap_or(false);
    json!({
        "__typename": "Note",
        "_id": n["id"],
        "id": format!("gid://gitlab/Note/{}", n["id"]),
        "body": str_of(n, "body"),
        "bodyHtml": format!("<p dir=\"auto\">{}</p>", str_of(n, "body")),
        "system": n["system"].as_bool().unwrap_or(false),
        "internal": n["internal"].as_bool().unwrap_or(false),
        "confidential": n["internal"].as_bool().unwrap_or(false),
        "createdAt": n["created_at"],
        "updatedAt": n.get("updated_at").unwrap_or(&n["created_at"]),
        "lastEditedAt": if n.get("updated_at").is_some_and(|u| *u != n["created_at"]) { n["updated_at"].clone() } else { Value::Null },
        "author": user_opt_obj(ctx, &n["author"]),
        "resolvable": resolvable,
        "resolved": resolvable && n["resolved"].as_bool().unwrap_or(false),
        "resolvedAt": n.get("resolved_at").cloned().unwrap_or(Value::Null),
        "resolvedBy": user_opt_obj(ctx, &n["resolved_by"]),
        "noteableId": format!("gid://gitlab/{nt}/{nid}"),
        "noteableType": nt,
        "url": format!("{}/{}#note_{}", ctx.base, project_of(noteable), n["id"]),
        "discussion": {"__typename": "Discussion", "id": format!("gid://gitlab/Discussion/{}", str_of(n, "discussion_id"))},
        "position": n.get("position").cloned().unwrap_or(Value::Null),
    })
}

fn discussions_for(ctx: &Ctx, noteable: &str) -> Vec<Value> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: Vec<Vec<Value>> = Vec::new();
    for (_, n) in all(ctx.st, "notes") {
        if str_of(n, "noteable") != noteable {
            continue;
        }
        let did = str_of(n, "discussion_id").to_owned();
        match order.iter().position(|d| *d == did) {
            Some(i) => groups[i].push(n.clone()),
            None => {
                order.push(did);
                groups.push(vec![n.clone()]);
            }
        }
    }
    order
        .into_iter()
        .zip(groups)
        .map(|(id, notes)| {
            let resolvable = notes
                .iter()
                .any(|n| n["resolvable"].as_bool().unwrap_or(false));
            let resolved = resolvable
                && notes
                    .iter()
                    .filter(|n| n["resolvable"] == true)
                    .all(|n| n["resolved"] == true);
            json!({
                "__typename": "Discussion",
                "id": format!("gid://gitlab/Discussion/{id}"),
                "resolvable": resolvable,
                "resolved": resolved,
                "createdAt": notes[0]["created_at"],
                "_notes": notes,
                "_system": notes[0]["system"].as_bool().unwrap_or(false),
            })
        })
        .collect()
}

fn award_obj(ctx: &Ctx, a: &Value) -> Value {
    json!({
        "__typename": "AwardEmoji",
        "name": str_of(a, "name"),
        "description": str_of(a, "name").replace('_', " "),
        "emoji": "",
        "unicode": "",
        "unicodeVersion": "6.0",
        "user": user_opt_obj(ctx, &a["user"]),
    })
}

fn awards_for(ctx: &Ctx, awardable: &str) -> Vec<Value> {
    all(ctx.st, "award_emoji")
        .into_iter()
        .filter(|(_, a)| str_of(a, "awardable") == awardable)
        .map(|(_, a)| award_obj(ctx, a))
        .collect()
}

pub(crate) fn iteration_obj(ctx: &Ctx, it: &Value) -> Value {
    let rest = super::render_iteration_rest(ctx.st, &ctx.base, it, &ctx.today);
    let state = match as_u64(&rest["state"]) {
        Some(3) => "closed",
        Some(2) => "current",
        _ => "upcoming",
    };
    let cadence = as_u64(&it["cadence_id"])
        .and_then(|c| get(ctx.st, "iteration_cadences", &c.to_string()))
        .map(cadence_obj)
        .unwrap_or(Value::Null);
    json!({
        "__typename": "Iteration",
        "id": format!("gid://gitlab/Iteration/{}", it["id"]),
        "iid": rest["iid"].to_string(),
        "sequence": rest["sequence"],
        "title": rest["title"],
        "description": rest["description"],
        "state": state,
        "startDate": rest["start_date"],
        "dueDate": rest["due_date"],
        "createdAt": rest["created_at"],
        "updatedAt": rest["updated_at"],
        "webUrl": rest["web_url"],
        "webPath": rest["web_url"].as_str().map(|u| u.replacen(&ctx.base, "", 1)),
        "iterationCadence": cadence,
    })
}

pub(crate) fn cadence_obj(c: &Value) -> Value {
    json!({
        "__typename": "IterationCadence",
        "id": format!("gid://gitlab/Iterations::Cadence/{}", c["id"]),
        "title": str_of(c, "title"),
        "description": c.get("description").cloned().unwrap_or(Value::Null),
        "automatic": c.get("automatic").cloned().unwrap_or(json!(true)),
        "active": c.get("active").cloned().unwrap_or(json!(true)),
        "startDate": c.get("start_date").cloned().unwrap_or(Value::Null),
        "durationInWeeks": c.get("duration_in_weeks").cloned().unwrap_or(json!(2)),
        "iterationsInAdvance": c.get("iterations_in_advance").cloned().unwrap_or(json!(2)),
        "rollOver": c.get("roll_over").cloned().unwrap_or(json!(false)),
    })
}

pub(crate) fn vulnerability_obj(ctx: &Ctx, v: &Value) -> Value {
    let project = str_of(v, "project");
    let loc = v.get("location").cloned().unwrap_or(json!({}));
    let loc_type = match str_of(v, "report_type") {
        "sast" => "VulnerabilityLocationSast",
        "secret_detection" => "VulnerabilityLocationSecretDetection",
        "dependency_scanning" => "VulnerabilityLocationDependencyScanning",
        "container_scanning" => "VulnerabilityLocationContainerScanning",
        "dast" => "VulnerabilityLocationDast",
        _ => "VulnerabilityLocationGeneric",
    };
    let mut location = json!({
        "__typename": loc_type,
        "file": loc.get("file").cloned().unwrap_or(Value::Null),
        "startLine": loc.get("start_line").map(|l| json!(l.to_string())).unwrap_or(Value::Null),
        "endLine": loc.get("end_line").map(|l| json!(l.to_string())).unwrap_or(Value::Null),
        "image": loc.get("image").cloned().unwrap_or(Value::Null),
        "operatingSystem": loc.get("operating_system").cloned().unwrap_or(Value::Null),
        "path": loc.get("path").cloned().unwrap_or(Value::Null),
        "hostname": loc.get("hostname").cloned().unwrap_or(Value::Null),
        "dependency": loc.get("dependency").map(|d| json!({"__typename": "VulnerableDependency", "version": d.get("version").cloned().unwrap_or(Value::Null), "package": {"__typename": "VulnerablePackage", "name": d.get("package").cloned().unwrap_or(Value::Null)}})).unwrap_or(Value::Null),
    });
    location["_types"] = json!(["VulnerabilityLocation"]);
    json!({
        "__typename": "Vulnerability",
        "_id": v["id"],
        "id": format!("gid://gitlab/Vulnerability/{}", v["id"]),
        "title": str_of(v, "title"),
        "description": v.get("description").cloned().unwrap_or(Value::Null),
        "state": str_of(v, "state").to_uppercase(),
        "severity": str_of(v, "severity").to_uppercase(),
        "reportType": str_of(v, "report_type").to_uppercase(),
        "detectedAt": v["detected_at"],
        "confirmedAt": v.get("confirmed_at").cloned().unwrap_or(Value::Null),
        "resolvedAt": v.get("resolved_at").cloned().unwrap_or(Value::Null),
        "dismissedAt": v.get("dismissed_at").cloned().unwrap_or(Value::Null),
        "dismissalReason": v.get("dismissal_reason").and_then(Value::as_str).map(str::to_uppercase),
        "webUrl": format!("{}/{project}/-/security/vulnerabilities/{}", ctx.base, v["id"]),
        "scanner": v.get("scanner").map(|s| json!({"__typename": "VulnerabilityScanner", "name": s.get("name").cloned().unwrap_or(Value::Null), "externalId": s.get("external_id").cloned().unwrap_or(Value::Null), "vendor": s.get("vendor").cloned().unwrap_or(json!("GitLab"))})).unwrap_or(Value::Null),
        "identifiers": v.get("identifiers").and_then(Value::as_array).cloned().unwrap_or_default().iter().map(|i| json!({"__typename": "VulnerabilityIdentifier", "externalType": i.get("external_type").cloned().unwrap_or(Value::Null), "externalId": i.get("external_id").cloned().unwrap_or(Value::Null), "name": i.get("name").cloned().unwrap_or(Value::Null), "url": i.get("url").cloned().unwrap_or(Value::Null)})).collect::<Vec<_>>(),
        "links": v.get("links").and_then(Value::as_array).cloned().unwrap_or_default().iter().map(|l| json!({"__typename": "VulnerabilityLink", "name": l.get("name").cloned().unwrap_or(Value::Null), "url": str_of(l, "url")})).collect::<Vec<_>>(),
        "location": location,
        "project": if get(ctx.st, "projects", project).is_some() { project_obj(ctx, project) } else { Value::Null },
    })
}

fn catalog_obj(ctx: &Ctx, c: &Value) -> Value {
    let project = str_of(c, "project");
    json!({
        "__typename": "CiCatalogResource",
        "_id": c["id"],
        "id": format!("gid://gitlab/Ci::Catalog::Resource/{}", c["id"]),
        "name": c.get("name").cloned().unwrap_or_else(|| json!(last_segment(project))),
        "description": c.get("description").cloned().unwrap_or(Value::Null),
        "fullPath": project,
        "icon": c.get("icon").cloned().unwrap_or(Value::Null),
        "starCount": c.get("star_count").cloned().unwrap_or(json!(0)),
        "topics": c.get("topics").cloned().unwrap_or(json!([])),
        "verificationLevel": c.get("verification_level").cloned().unwrap_or(json!("UNVERIFIED")),
        "visibilityLevel": c.get("visibility_level").cloned().unwrap_or(json!("public")),
        "webPath": format!("/explore/catalog/{project}"),
        "webUrl": format!("{}/explore/catalog/{project}", ctx.base),
        "latestReleasedAt": c.get("latest_released_at").cloned().unwrap_or(Value::Null),
        "last30DayUsageCount": c.get("usage_30d").cloned().unwrap_or(json!(0)),
    })
}

// ---------------------------------------------------------------------------
// Widgets

const WIDGETS_ALL: &[(&str, &str)] = &[
    ("ASSIGNEES", "WorkItemWidgetAssignees"),
    ("DESCRIPTION", "WorkItemWidgetDescription"),
    ("HIERARCHY", "WorkItemWidgetHierarchy"),
    ("LABELS", "WorkItemWidgetLabels"),
    ("MILESTONE", "WorkItemWidgetMilestone"),
    ("NOTES", "WorkItemWidgetNotes"),
    ("START_AND_DUE_DATE", "WorkItemWidgetStartAndDueDate"),
    ("HEALTH_STATUS", "WorkItemWidgetHealthStatus"),
    ("WEIGHT", "WorkItemWidgetWeight"),
    ("ITERATION", "WorkItemWidgetIteration"),
    ("STATUS", "WorkItemWidgetStatus"),
    ("LINKED_ITEMS", "WorkItemWidgetLinkedItems"),
    ("TIME_TRACKING", "WorkItemWidgetTimeTracking"),
    ("DEVELOPMENT", "WorkItemWidgetDevelopment"),
    ("CUSTOM_FIELDS", "WorkItemWidgetCustomFields"),
    ("AWARD_EMOJI", "WorkItemWidgetAwardEmoji"),
    ("CURRENT_USER_TODOS", "WorkItemWidgetCurrentUserTodos"),
    ("NOTIFICATIONS", "WorkItemWidgetNotifications"),
    ("PARTICIPANTS", "WorkItemWidgetParticipants"),
    ("PROGRESS", "WorkItemWidgetProgress"),
    ("COLOR", "WorkItemWidgetColor"),
];

fn widget_supported(issue_type: &str, w: &str) -> bool {
    match issue_type {
        "incident" => !["WEIGHT", "ITERATION", "HEALTH_STATUS", "PROGRESS", "COLOR"].contains(&w),
        "epic" => ![
            "MILESTONE",
            "ITERATION",
            "DEVELOPMENT",
            "TIME_TRACKING",
            "PROGRESS",
        ]
        .contains(&w),
        "objective" | "key_result" => !["ITERATION", "DEVELOPMENT", "COLOR", "WEIGHT"].contains(&w),
        "requirement" | "test_case" => [
            "DESCRIPTION",
            "NOTES",
            "LABELS",
            "AWARD_EMOJI",
            "CURRENT_USER_TODOS",
            "NOTIFICATIONS",
            "PARTICIPANTS",
            "STATUS",
        ]
        .contains(&w),
        _ => !["PROGRESS", "COLOR"].contains(&w),
    }
}

fn widgets(ctx: &Ctx, key: &str, only: Option<Vec<String>>) -> Vec<Value> {
    let Some(doc) = get(ctx.st, "issues", key) else {
        return Vec::new();
    };
    let issue_type = doc
        .get("issue_type")
        .and_then(Value::as_str)
        .unwrap_or("issue");
    WIDGETS_ALL
        .iter()
        .filter(|(t, _)| widget_supported(issue_type, t))
        .filter(|(t, _)| only.as_ref().is_none_or(|o| o.iter().any(|x| x == t)))
        .map(|(t, n)| widget(ctx, key, doc, t, n))
        .collect()
}

fn widget(ctx: &Ctx, key: &str, doc: &Value, ty: &str, name: &str) -> Value {
    let project = project_of(key);
    let mut w = json!({"__typename": name, "_types": ["WorkItemWidget"], "type": ty, "_key": key});
    match ty {
        "ASSIGNEES" => {
            w["allowsMultipleAssignees"] = json!(true);
            w["canInviteMembers"] = json!(false);
        }
        "DESCRIPTION" => {
            w["description"] = doc.get("description").cloned().unwrap_or(json!(""));
            w["descriptionHtml"] = json!(format!(
                "<p dir=\"auto\">{}</p>",
                str_of(doc, "description")
            ));
            w["edited"] = json!(false);
            w["lastEditedAt"] = Value::Null;
        }
        "HIERARCHY" => {
            let children = children_of(ctx.st, key);
            w["hasChildren"] = json!(!children.is_empty());
            w["hasParent"] = json!(
                doc.get("parent")
                    .and_then(Value::as_str)
                    .is_some_and(|p| get(ctx.st, "issues", p).is_some())
            );
            w["parent"] = doc
                .get("parent")
                .and_then(Value::as_str)
                .map(|p| work_item_obj(ctx, p))
                .unwrap_or(Value::Null);
            w["depthLimitReachedByType"] = json!([]);
            w["rolledUpCountsByType"] = json!([]);
        }
        "LABELS" => {
            w["allowsScopedLabels"] = json!(true);
        }
        "MILESTONE" => {
            w["milestone"] = crate::issues::milestone_key_by_id(ctx.st, &doc["milestone_id"])
                .map(|k| milestone_obj(ctx, &k))
                .unwrap_or(Value::Null);
        }
        "NOTES" => {
            w["discussionLocked"] = json!(
                doc.get("discussion_locked")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            );
        }
        "START_AND_DUE_DATE" => {
            w["startDate"] = doc.get("start_date").cloned().unwrap_or(Value::Null);
            w["dueDate"] = doc.get("due_date").cloned().unwrap_or(Value::Null);
            w["isFixed"] = json!(true);
            w["rollUp"] = json!(false);
        }
        "HEALTH_STATUS" => {
            w["healthStatus"] = doc.get("health_status").cloned().unwrap_or(Value::Null);
            w["rolledUpHealthStatus"] = json!([]);
        }
        "WEIGHT" => {
            let children = children_of(ctx.st, key);
            let weight = doc.get("weight").cloned().unwrap_or(Value::Null);
            let rolled: u64 = children
                .iter()
                .filter_map(|c| get(ctx.st, "issues", c).and_then(|d| as_u64(&d["weight"])))
                .sum();
            let done: u64 = children
                .iter()
                .filter_map(|c| get(ctx.st, "issues", c))
                .filter(|d| str_of(d, "state") == "closed")
                .filter_map(|d| as_u64(&d["weight"]))
                .sum();
            w["weight"] = weight;
            w["rolledUpWeight"] = if children.is_empty() {
                Value::Null
            } else {
                json!(rolled)
            };
            w["rolledUpCompletedWeight"] = if children.is_empty() {
                Value::Null
            } else {
                json!(done)
            };
            w["widgetDefinition"] = json!({"__typename": "WorkItemWidgetDefinitionWeight", "editable": true, "rollUp": true});
        }
        "ITERATION" => {
            w["iteration"] = as_u64(&doc["iteration_id"])
                .and_then(|i| get(ctx.st, "iterations", &i.to_string()))
                .map(|it| iteration_obj(ctx, it))
                .unwrap_or(Value::Null);
        }
        "STATUS" => {
            let name = doc
                .get("status")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    if str_of(doc, "state") == "closed" {
                        "Done".into()
                    } else {
                        "To do".into()
                    }
                });
            w["status"] = STATUSES
                .iter()
                .find(|s| s.1.eq_ignore_ascii_case(&name))
                .map(status_obj)
                .unwrap_or(Value::Null);
        }
        "LINKED_ITEMS" => {
            let links = linked(ctx.st, key);
            w["blocked"] = json!(links.iter().any(|(_, t)| t == "is_blocked_by"));
            w["blockedByCount"] = json!(links.iter().filter(|(_, t)| t == "is_blocked_by").count());
            w["blockingCount"] = json!(links.iter().filter(|(_, t)| t == "blocks").count());
        }
        "TIME_TRACKING" => {
            w["timeEstimate"] = json!(as_u64(&doc["time_estimate"]).unwrap_or(0));
            w["totalTimeSpent"] = json!(as_u64(&doc["total_time_spent"]).unwrap_or(0));
            w["humanReadableAttributes"] = json!({"__typename": "WorkItemWidgetTimeTrackingHumanReadableAttributes", "timeEstimate": null, "totalTimeSpent": null});
        }
        "DEVELOPMENT" => {
            w["willAutoCloseByMergeRequest"] = json!(!closing_mrs(ctx.st, key).is_empty());
        }
        "AWARD_EMOJI" => {
            let awards = awards_for(ctx, key);
            w["upvotes"] = json!(awards.iter().filter(|a| a["name"] == "thumbsup").count());
            w["downvotes"] = json!(awards.iter().filter(|a| a["name"] == "thumbsdown").count());
        }
        "CURRENT_USER_TODOS" => {
            w["currentUserTodos"] =
                json!({"__typename": "TodoConnection", "nodes": [], "edges": [], "count": 0});
        }
        "NOTIFICATIONS" => {
            w["subscribed"] = json!(false);
        }
        "PROGRESS" => {
            w["progress"] = doc.get("progress").cloned().unwrap_or(Value::Null);
            w["updatedAt"] = Value::Null;
            w["currentValue"] = Value::Null;
            w["startValue"] = Value::Null;
            w["endValue"] = Value::Null;
        }
        "COLOR" => {
            w["color"] = doc.get("color").cloned().unwrap_or(json!("#1068bf"));
            w["textColor"] = json!("#FFFFFF");
        }
        _ => {}
    }
    let _ = project;
    w
}

fn children_of(st: &Store, key: &str) -> Vec<String> {
    all(st, "issues")
        .into_iter()
        .filter(|(_, d)| str_of(d, "parent") == key)
        .map(|(k, _)| k.to_owned())
        .collect()
}

/// `(other key, link type from this item's perspective)`.
fn linked(st: &Store, key: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (_, l) in all(st, "issue_links") {
        let ty = str_of(l, "link_type");
        if str_of(l, "source") == key {
            out.push((str_of(l, "target").to_owned(), ty.to_owned()));
        } else if str_of(l, "target") == key {
            let inv = match ty {
                "blocks" => "is_blocked_by",
                "is_blocked_by" => "blocks",
                _ => "relates_to",
            };
            out.push((str_of(l, "source").to_owned(), inv.to_owned()));
        }
    }
    out
}

fn closing_mrs(st: &Store, key: &str) -> Vec<String> {
    let project = project_of(key);
    let iid = iid_of(key);
    all(st, "merge_requests")
        .into_iter()
        .filter(|(k, m)| {
            let d = str_of(m, "description").to_lowercase();
            project_of(k) == project
                && (d.contains(&format!("closes #{iid}")) || d.contains(&format!("fixes #{iid}")))
        })
        .map(|(k, _)| k.to_owned())
        .collect()
}

fn related_mrs(st: &Store, key: &str) -> Vec<String> {
    let project = project_of(key);
    let iid = iid_of(key);
    all(st, "merge_requests")
        .into_iter()
        .filter(|(k, m)| {
            project_of(k) == project && str_of(m, "description").contains(&format!("#{iid}"))
        })
        .map(|(k, _)| k.to_owned())
        .collect()
}

fn mr_brief(ctx: &Ctx, k: &str) -> Value {
    let m = mr_obj(ctx, k);
    json!({"__typename": "MergeRequest", "_key": k, "iid": m["iid"], "title": m["title"], "webUrl": m["webUrl"], "state": m["state"], "sourceBranch": m["sourceBranch"], "id": m["id"], "targetBranch": m["targetBranch"], "draft": m["draft"]})
}

// ---------------------------------------------------------------------------
// Work item queries

fn wi_key_by_gid(st: &Store, gid: &str) -> Option<String> {
    let n = gid_num(gid)?;
    all(st, "issues")
        .into_iter()
        .find(|(_, d)| as_u64(&d["id"]) == Some(n))
        .map(|(k, _)| k.to_owned())
}

fn arg_list(args: &Map<String, Value>, k: &str) -> Option<Vec<String>> {
    args.get(k).filter(|v| !v.is_null()).map(value_list)
}

fn filter_work_items(ctx: &Ctx, scopes: &[String], args: &Map<String, Value>) -> Vec<Value> {
    let st = ctx.st;
    let types = arg_list(args, "types");
    let state = args.get("state").and_then(Value::as_str);
    let search = args
        .get("search")
        .and_then(Value::as_str)
        .map(str::to_lowercase);
    let assignees = arg_list(args, "assigneeUsernames");
    let labels = arg_list(args, "labelName");
    let author = args.get("authorUsername").and_then(Value::as_str);
    let iids = arg_list(args, "iids").or_else(|| {
        args.get("iid")
            .and_then(Value::as_str)
            .map(|i| vec![i.to_owned()])
    });
    let mut items: Vec<(String, Value)> = all(st, "issues")
        .into_iter()
        .filter(|(k, _)| scopes.iter().any(|s| s == project_of(k)))
        .filter(|(_, d)| {
            let it = d
                .get("issue_type")
                .and_then(Value::as_str)
                .unwrap_or("issue");
            types
                .as_ref()
                .is_none_or(|ts| ts.iter().any(|t| type_by_issue_type(it).2 == t.as_str()))
                && state.is_none_or(|s| match s {
                    "opened" => str_of(d, "state") == "opened",
                    "closed" => str_of(d, "state") == "closed",
                    _ => true,
                })
                && search.as_ref().is_none_or(|q| {
                    str_of(d, "title").to_lowercase().contains(q)
                        || str_of(d, "description").to_lowercase().contains(q)
                })
                && assignees
                    .as_ref()
                    .is_none_or(|a| a.iter().all(|u| value_list(&d["assignees"]).contains(u)))
                && labels.as_ref().is_none_or(|l| {
                    l.iter().all(|n| {
                        value_list(&d["labels"])
                            .iter()
                            .any(|x| x.eq_ignore_ascii_case(n))
                    })
                })
                && author.is_none_or(|a| str_of(d, "author") == a)
        })
        .filter(|(k, d)| {
            iids.as_ref().is_none_or(|ids| {
                ids.contains(&as_u64(&d["iid"]).unwrap_or_else(|| iid_of(k)).to_string())
            })
        })
        .map(|(k, d)| (k.to_owned(), d.clone()))
        .collect();
    let sort = args
        .get("sort")
        .and_then(Value::as_str)
        .unwrap_or("CREATED_DESC");
    items.sort_by(|a, b| {
        let (fa, fb) = if sort.starts_with("UPDATED") {
            (str_of(&a.1, "updated_at"), str_of(&b.1, "updated_at"))
        } else {
            (str_of(&a.1, "created_at"), str_of(&b.1, "created_at"))
        };
        let ord = fa.cmp(fb).then(as_u64(&a.1["id"]).cmp(&as_u64(&b.1["id"])));
        if sort.ends_with("ASC") {
            ord
        } else {
            ord.reverse()
        }
    });
    items.iter().map(|(k, _)| work_item_obj(ctx, k)).collect()
}

fn namespace_scopes(ctx: &Ctx, obj: &Value, descendants: bool) -> Vec<String> {
    let key = str_of(obj, "_key").to_owned();
    if obj["_kind"] == "group" {
        let mut out = vec![key.clone()];
        if descendants {
            let prefix = format!("{key}/");
            out.extend(
                all(ctx.st, "projects")
                    .iter()
                    .filter(|(k, _)| k.starts_with(&prefix))
                    .map(|(k, _)| (*k).to_owned()),
            );
            out.extend(
                all(ctx.st, "groups")
                    .iter()
                    .filter(|(k, _)| k.starts_with(&prefix))
                    .map(|(k, _)| (*k).to_owned()),
            );
        }
        out
    } else {
        vec![key]
    }
}

fn work_item_types(ctx: &Ctx, args: &Map<String, Value>, is_group: bool, scope: &str) -> Value {
    let filter = args.get("name").and_then(Value::as_str);
    let nodes: Vec<Value> = WORK_ITEM_TYPES
        .iter()
        .filter(|t| {
            if is_group {
                t.3 == "epic"
            } else {
                t.3 != "epic"
            }
        })
        .filter(|t| filter.is_none_or(|f| t.2 == f))
        .map(|t| {
            let mut o = work_item_type_obj(t.3);
            o["_types"] = json!(["WorkItemType"]);
            o["_scope"] = json!(scope);
            o
        })
        .collect();
    let _ = ctx;
    connection("WorkItemTypeConnection", nodes, args)
}

fn widget_definitions(ctx: &Ctx, issue_type: &str, project: &str) -> Vec<Value> {
    let mut out = Vec::new();
    for (t, n) in WIDGETS_ALL {
        if !widget_supported(issue_type, t) {
            continue;
        }
        let def_name = n.replace("WorkItemWidget", "WorkItemWidgetDefinition");
        let mut d =
            json!({"__typename": def_name, "_types": ["WorkItemWidgetDefinition"], "type": t});
        match *t {
            "STATUS" => {
                d["allowedStatuses"] = Value::Array(STATUSES.iter().map(status_obj).collect());
                d["defaultOpenStatus"] = status_obj(&STATUSES[0]);
                d["defaultClosedStatus"] = status_obj(&STATUSES[2]);
            }
            "HIERARCHY" => {
                let (children, parents): (Vec<&str>, Vec<&str>) = match issue_type {
                    "epic" => (vec!["epic", "issue"], vec!["epic"]),
                    "issue" => (vec!["task"], vec!["epic"]),
                    "objective" => (vec!["objective", "key_result"], vec!["objective"]),
                    "key_result" => (vec![], vec!["objective"]),
                    "task" => (vec![], vec!["issue", "incident"]),
                    "incident" => (vec!["task"], vec![]),
                    _ => (vec![], vec![]),
                };
                let conn = |ts: Vec<&str>| json!({"__typename": "WorkItemTypeConnection", "nodes": ts.iter().map(|t| work_item_type_obj(t)).collect::<Vec<_>>()});
                d["allowedChildTypes"] = conn(children);
                d["allowedParentTypes"] = conn(parents);
            }
            "CUSTOM_FIELDS" => {
                let type_name = type_by_issue_type(issue_type).0;
                let fields: Vec<Value> = custom_fields_for(ctx.st, project, type_name)
                    .iter()
                    .map(|f| json!({"__typename": "WorkItemCustomFieldValue", "customField": custom_field_obj(f)}))
                    .collect();
                d["customFieldValues"] = Value::Array(fields);
            }
            _ => {}
        }
        out.push(d);
    }
    out
}

fn custom_fields_for(st: &Store, project: &str, type_name: &str) -> Vec<Value> {
    let root = project.split('/').next().unwrap_or(project).to_owned();
    all(st, "custom_fields")
        .into_iter()
        .filter(|(_, f)| {
            f.get("namespace")
                .and_then(Value::as_str)
                .is_none_or(|n| n == root || project.starts_with(&format!("{n}/")))
        })
        .filter(|(_, f)| {
            f.get("work_item_types")
                .is_none_or(|t| value_list(t).iter().any(|x| x == type_name))
        })
        .filter(|(_, f)| f.get("active").and_then(Value::as_bool).unwrap_or(true))
        .map(|(_, f)| f.clone())
        .collect()
}

fn custom_field_obj(f: &Value) -> Value {
    json!({
        "__typename": "CustomField",
        "id": format!("gid://gitlab/Issuables::CustomField/{}", f["id"]),
        "name": str_of(f, "name"),
        "fieldType": f.get("field_type").and_then(Value::as_str).unwrap_or("TEXT"),
        "active": true,
        "selectOptions": f.get("select_options").and_then(Value::as_array).cloned().unwrap_or_default().iter().map(|o| json!({"__typename": "CustomFieldSelectOption", "id": format!("gid://gitlab/Issuables::CustomFieldSelectOption/{}", o["id"]), "value": str_of(o, "value")})).collect::<Vec<_>>(),
        "workItemTypes": value_list(f.get("work_item_types").unwrap_or(&json!(["Issue"]))).iter().filter_map(|n| WORK_ITEM_TYPES.iter().find(|t| t.0 == n)).map(|t| work_item_type_obj(t.3)).collect::<Vec<_>>(),
    })
}

fn custom_field_values(ctx: &Ctx, key: &str) -> Vec<Value> {
    let Some(doc) = get(ctx.st, "issues", key) else {
        return Vec::new();
    };
    let type_name = type_by_issue_type(
        doc.get("issue_type")
            .and_then(Value::as_str)
            .unwrap_or("issue"),
    )
    .0;
    let values = doc.get("custom_fields").cloned().unwrap_or(json!({}));
    custom_fields_for(ctx.st, project_of(key), type_name)
        .iter()
        .map(|f| {
            let ft = f.get("field_type").and_then(Value::as_str).unwrap_or("TEXT");
            let v = values.get(f["id"].to_string()).cloned().unwrap_or(Value::Null);
            let mut out = json!({"customField": custom_field_obj(f)});
            match ft {
                "SINGLE_SELECT" | "MULTI_SELECT" => {
                    out["__typename"] = json!("WorkItemSelectFieldValue");
                    let ids = value_list(&v);
                    out["selectedOptions"] = Value::Array(
                        f.get("select_options").and_then(Value::as_array).cloned().unwrap_or_default().iter()
                            .filter(|o| ids.contains(&o["id"].to_string()))
                            .map(|o| json!({"__typename": "CustomFieldSelectOption", "id": format!("gid://gitlab/Issuables::CustomFieldSelectOption/{}", o["id"]), "value": str_of(o, "value")}))
                            .collect(),
                    );
                    if ids.is_empty() {
                        out["selectedOptions"] = Value::Null;
                    }
                }
                "NUMBER" => {
                    out["__typename"] = json!("WorkItemNumberFieldValue");
                    out["value"] = v;
                }
                "DATE" => {
                    out["__typename"] = json!("WorkItemDateFieldValue");
                    out["value"] = v;
                }
                _ => {
                    out["__typename"] = json!("WorkItemTextFieldValue");
                    out["value"] = v;
                }
            }
            out["_types"] = json!(["WorkItemCustomFieldValue"]);
            out
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The resolver

fn missing_field(obj: &Value, field: &str) -> String {
    format!(
        "Field '{field}' doesn't exist on type '{}'",
        obj.get("__typename")
            .and_then(Value::as_str)
            .unwrap_or("Object")
    )
}

/// Resolves `field` on `obj`.
pub(crate) fn resolve(ctx: &Ctx, obj: &Value, field: &str, args: &Map<String, Value>) -> R {
    let ty = obj.get("__typename").and_then(Value::as_str).unwrap_or("");
    let special = match ty {
        "Query" => Some(query_field(ctx, field, args)?),
        "Project" | "Group" | "Namespace" => namespace_field(ctx, obj, field, args)?,
        "WorkItem" => work_item_field(ctx, obj, field, args)?,
        "Issue" | "MergeRequest" => issuable_field(ctx, obj, field, args)?,
        "Discussion" => match field {
            "notes" => Some(connection(
                "NoteConnection",
                obj.get("_notes")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|n| note_obj(ctx, n))
                    .collect(),
                args,
            )),
            _ => None,
        },
        "Note" => match field {
            "awardEmoji" => Some(connection(
                "AwardEmojiConnection",
                awards_for(ctx, &format!("note:{}", obj["_id"])),
                args,
            )),
            _ => None,
        },
        "Awardable" => match field {
            "awardEmoji" => Some(connection(
                "AwardEmojiConnection",
                awards_for(ctx, str_of(obj, "_awardable")),
                args,
            )),
            _ => None,
        },
        "CiCatalogResource" => match field {
            "versions" => Some(catalog_versions(ctx, obj, args)),
            _ => None,
        },
        "CiCatalogResourceVersion" => match field {
            "components" => Some(connection(
                "CiCatalogResourceComponentConnection",
                obj.get("_components")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
                args,
            )),
            _ => None,
        },
        "WorkItemType" => match field {
            "widgetDefinitions" => Some(Value::Array(widget_definitions(
                ctx,
                str_of(obj, "_issue_type"),
                str_of(obj, "_scope"),
            ))),
            "supportedConversionTypes" => {
                let me = str_of(obj, "_issue_type");
                Some(Value::Array(
                    WORK_ITEM_TYPES
                        .iter()
                        .filter(|t| {
                            t.3 != me
                                && ![
                                    "epic",
                                    "objective",
                                    "key_result",
                                    "test_case",
                                    "requirement",
                                ]
                                .contains(&t.3)
                        })
                        .map(|t| work_item_type_obj(t.3))
                        .collect(),
                ))
            }
            _ => None,
        },
        "Vulnerability" => match field {
            "notes" | "discussions" => Some(connection("NoteConnection", vec![], args)),
            _ => None,
        },
        t if t.starts_with("WorkItemWidget") && !t.starts_with("WorkItemWidgetDefinition") => {
            widget_field(ctx, obj, field, args)?
        }
        _ => None,
    };
    if let Some(v) = special {
        return Ok(v);
    }
    if field.starts_with('_') {
        return Err(missing_field(obj, field));
    }
    match obj.get(field) {
        Some(v) => Ok(v.clone()),
        None => Err(missing_field(obj, field)),
    }
}

fn query_field(ctx: &Ctx, field: &str, args: &Map<String, Value>) -> R {
    let st = ctx.st;
    let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    Ok(match field {
        "currentUser" => {
            let u = current_username(st);
            let mut o = user_obj(ctx, &u);
            o["__typename"] = json!("CurrentUser");
            o["_types"] = json!(["User", "UserCore", "CurrentUser"]);
            o["emails"] = json!({"__typename": "EmailConnection", "nodes": []});
            o["publicEmail"] = user(st, &u)
                .and_then(|d| d.get("email").cloned())
                .unwrap_or(Value::Null);
            o
        }
        "user" => {
            let name = if args.contains_key("username") {
                Some(s("username"))
            } else {
                gid_num(&s("id"))
                    .and_then(|n| user_by_id(st, &n.to_string()).map(|(k, _)| k.to_owned()))
            };
            name.filter(|n| user(st, n).is_some())
                .map(|n| user_obj(ctx, &n))
                .unwrap_or(Value::Null)
        }
        "users" => {
            let names = arg_list(args, "usernames");
            let search = args
                .get("search")
                .and_then(Value::as_str)
                .map(str::to_lowercase);
            let nodes = all(st, "users")
                .into_iter()
                .filter(|(k, u)| {
                    names.as_ref().is_none_or(|n| n.iter().any(|x| x == k))
                        && search.as_ref().is_none_or(|q| {
                            k.contains(q.as_str()) || str_of(u, "name").to_lowercase().contains(q)
                        })
                })
                .map(|(k, _)| user_obj(ctx, k))
                .collect();
            connection("UserCoreConnection", nodes, args)
        }
        "project" => project_key(st, &s("fullPath"))
            .map(|k| project_obj(ctx, &k))
            .unwrap_or(Value::Null),
        "projects" => {
            let paths = arg_list(args, "fullPaths");
            let search = args
                .get("search")
                .and_then(Value::as_str)
                .map(str::to_lowercase);
            let ids = arg_list(args, "ids");
            let nodes = all(st, "projects")
                .into_iter()
                .filter(|(k, p)| {
                    paths.as_ref().is_none_or(|ps| ps.iter().any(|x| x == k))
                        && search.as_ref().is_none_or(|q| k.to_lowercase().contains(q))
                        && ids
                            .as_ref()
                            .is_none_or(|is| is.iter().any(|i| gid_num(i) == as_u64(&p["id"])))
                })
                .map(|(k, _)| project_obj(ctx, k))
                .collect();
            connection("ProjectConnection", nodes, args)
        }
        "group" => group_key(st, &s("fullPath"))
            .map(|k| group_obj(ctx, &k))
            .unwrap_or(Value::Null),
        "groups" => {
            let search = args
                .get("search")
                .and_then(Value::as_str)
                .map(str::to_lowercase);
            let nodes = all(st, "groups")
                .into_iter()
                .filter(|(k, _)| search.as_ref().is_none_or(|q| k.to_lowercase().contains(q)))
                .map(|(k, _)| group_obj(ctx, k))
                .collect();
            connection("GroupConnection", nodes, args)
        }
        "namespace" => namespace_obj(ctx, &s("fullPath")),
        "workItem" => wi_key_by_gid(st, &s("id"))
            .map(|k| work_item_obj(ctx, &k))
            .unwrap_or(Value::Null),
        "workItemsByReference" => {
            let refs = arg_list(args, "refs").unwrap_or_default();
            let ctx_path = s("contextNamespacePath");
            let nodes = refs
                .iter()
                .filter_map(|r| {
                    let (p, iid) = r.rsplit_once('#')?;
                    let p = if p.is_empty() { ctx_path.as_str() } else { p };
                    let key = format!("{}#{iid}", project_key(st, p)?);
                    get(st, "issues", &key).map(|_| work_item_obj(ctx, &key))
                })
                .collect();
            connection("WorkItemConnection", nodes, args)
        }
        "issue" => wi_key_by_gid(st, &s("id"))
            .map(|k| issue_obj(ctx, &k))
            .unwrap_or(Value::Null),
        "issues" => {
            let projects: Vec<String> = all(st, "projects")
                .iter()
                .map(|(k, _)| (*k).to_owned())
                .collect();
            let nodes = filter_work_items(ctx, &projects, args)
                .iter()
                .map(|w| issue_obj(ctx, str_of(w, "_key")))
                .collect();
            connection("IssueConnection", nodes, args)
        }
        "mergeRequest" => {
            let n = gid_num(&s("id"));
            all(st, "merge_requests")
                .into_iter()
                .find(|(_, m)| as_u64(&m["id"]) == n)
                .map(|(k, _)| mr_obj(ctx, k))
                .unwrap_or(Value::Null)
        }
        "note" => {
            let n = gid_num(&s("id")).map(|n| n.to_string()).unwrap_or_default();
            get(st, "notes", &n)
                .map(|d| note_obj(ctx, d))
                .unwrap_or(Value::Null)
        }
        "awardable" => {
            let id = s("id");
            let target = if id.contains("/WorkItem/") || id.contains("/Issue/") {
                wi_key_by_gid(st, &id)
            } else if id.contains("Note/") {
                gid_num(&id)
                    .filter(|n| get(st, "notes", &n.to_string()).is_some())
                    .map(|n| format!("note:{n}"))
            } else if id.contains("/MergeRequest/") {
                let n = gid_num(&id);
                all(st, "merge_requests")
                    .into_iter()
                    .find(|(_, m)| as_u64(&m["id"]) == n)
                    .map(|(k, _)| k.to_owned())
            } else {
                None
            };
            target
                .map(|t| json!({"__typename": "Awardable", "_awardable": t, "id": id}))
                .unwrap_or(Value::Null)
        }
        "vulnerability" => {
            let n = gid_num(&s("id")).map(|n| n.to_string()).unwrap_or_default();
            get(st, "vulnerabilities", &n)
                .map(|v| vulnerability_obj(ctx, v))
                .unwrap_or(Value::Null)
        }
        "ciCatalogResources" => {
            let search = args
                .get("search")
                .and_then(Value::as_str)
                .map(str::to_lowercase);
            let topics = arg_list(args, "topics");
            let level = args.get("verificationLevel").and_then(Value::as_str);
            let mut nodes: Vec<Value> = all(st, "ci_catalog_resources")
                .into_iter()
                .filter(|(_, c)| {
                    search.as_ref().is_none_or(|q| {
                        str_of(c, "name").to_lowercase().contains(q)
                            || str_of(c, "description").to_lowercase().contains(q)
                            || str_of(c, "project").contains(q.as_str())
                    }) && topics
                        .as_ref()
                        .is_none_or(|t| t.iter().all(|x| value_list(&c["topics"]).contains(x)))
                        && level.is_none_or(|l| {
                            c.get("verification_level")
                                .and_then(Value::as_str)
                                .unwrap_or("UNVERIFIED")
                                == l
                        })
                })
                .map(|(_, c)| catalog_obj(ctx, c))
                .collect();
            if let Some(sort) = args.get("sort").and_then(Value::as_str) {
                match sort {
                    "NAME_ASC" => nodes.sort_by(|a, b| str_of(a, "name").cmp(str_of(b, "name"))),
                    "NAME_DESC" => nodes.sort_by(|a, b| str_of(b, "name").cmp(str_of(a, "name"))),
                    "STAR_COUNT_DESC" => {
                        nodes.sort_by(|a, b| as_u64(&b["starCount"]).cmp(&as_u64(&a["starCount"])))
                    }
                    "USAGE_COUNT_DESC" => nodes.sort_by(|a, b| {
                        as_u64(&b["last30DayUsageCount"]).cmp(&as_u64(&a["last30DayUsageCount"]))
                    }),
                    _ => {}
                }
            }
            connection("CiCatalogResourceConnection", nodes, args)
        }
        "ciCatalogResource" => {
            let id = gid_num(&s("id"));
            let path = s("fullPath");
            all(st, "ci_catalog_resources")
                .into_iter()
                .find(|(_, c)| {
                    (id.is_some() && as_u64(&c["id"]) == id)
                        || (!path.is_empty() && str_of(c, "project") == path)
                })
                .map(|(_, c)| catalog_obj(ctx, c))
                .unwrap_or(Value::Null)
        }
        "metadata" => {
            let version = instance_field(st, "version").unwrap_or(DEFAULT_VERSION);
            json!({"__typename": "Metadata", "version": version, "revision": instance_field(st, "revision").unwrap_or("6b9e5d1c3a2"), "enterprise": version.ends_with("-ee"), "kas": {"__typename": "Kas", "enabled": false, "externalUrl": null, "version": null}})
        }
        "echo" => json!(format!("nil says: {}", s("text"))),
        _ => return Err(format!("Field '{field}' doesn't exist on type 'Query'")),
    })
}

fn namespace_field(
    ctx: &Ctx,
    obj: &Value,
    field: &str,
    args: &Map<String, Value>,
) -> Result<Option<Value>, String> {
    let st = ctx.st;
    let key = str_of(obj, "_key").to_owned();
    let is_group = obj["_kind"] == "group";
    let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    Ok(Some(match field {
        "workItem" => {
            let k = format!("{key}#{}", s("iid"));
            work_item_obj(ctx, &k)
        }
        "workItems" => {
            let descendants = args
                .get("includeDescendants")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            connection(
                "WorkItemConnection",
                filter_work_items(ctx, &namespace_scopes(ctx, obj, descendants), args),
                args,
            )
        }
        "workItemTypes" => work_item_types(ctx, args, is_group, &key),
        "issue" => issue_obj(ctx, &format!("{key}#{}", s("iid"))),
        "issues" => {
            let scopes = namespace_scopes(ctx, obj, true);
            let mut a = args.clone();
            if let Some(st) = a.get("state").and_then(Value::as_str).map(str::to_owned) {
                a.insert("state".into(), json!(st));
            }
            let nodes = filter_work_items(ctx, &scopes, &a)
                .iter()
                .filter(|w| {
                    ["Issue", "Incident"]
                        .contains(&w["workItemType"]["name"].as_str().unwrap_or(""))
                })
                .map(|w| issue_obj(ctx, str_of(w, "_key")))
                .collect();
            connection("IssueConnection", nodes, args)
        }
        "mergeRequest" => {
            if is_group {
                return Err(missing_field(obj, field));
            }
            mr_obj(ctx, &format!("{key}!{}", s("iid")))
        }
        "mergeRequests" => {
            let scopes = namespace_scopes(ctx, obj, true);
            let state = args
                .get("state")
                .and_then(Value::as_str)
                .filter(|s| *s != "all");
            let iids = arg_list(args, "iids");
            let src = arg_list(args, "sourceBranches");
            let tgt = arg_list(args, "targetBranches");
            let labels = arg_list(args, "labels").or_else(|| arg_list(args, "labelName"));
            let author = args.get("authorUsername").and_then(Value::as_str);
            let reviewer = args.get("reviewerUsername").and_then(Value::as_str);
            let assignee = args.get("assigneeUsername").and_then(Value::as_str);
            let mut keys: Vec<(String, Value)> = all(st, "merge_requests")
                .into_iter()
                .filter(|(k, m)| {
                    scopes.iter().any(|s| s == project_of(k))
                        && state.is_none_or(|s| str_of(m, "state") == s)
                        && iids
                            .as_ref()
                            .is_none_or(|i| i.contains(&iid_of(k).to_string()))
                        && src
                            .as_ref()
                            .is_none_or(|b| b.iter().any(|x| x == str_of(m, "source_branch")))
                        && tgt
                            .as_ref()
                            .is_none_or(|b| b.iter().any(|x| x == str_of(m, "target_branch")))
                        && labels
                            .as_ref()
                            .is_none_or(|l| l.iter().all(|n| value_list(&m["labels"]).contains(n)))
                        && author.is_none_or(|a| str_of(m, "author") == a)
                        && reviewer
                            .is_none_or(|a| value_list(&m["reviewers"]).iter().any(|x| x == a))
                        && assignee
                            .is_none_or(|a| value_list(&m["assignees"]).iter().any(|x| x == a))
                })
                .map(|(k, m)| (k.to_owned(), m.clone()))
                .collect();
            keys.sort_by(|a, b| str_of(&b.1, "created_at").cmp(str_of(&a.1, "created_at")));
            connection(
                "MergeRequestConnection",
                keys.iter().map(|(k, _)| mr_obj(ctx, k)).collect(),
                args,
            )
        }
        "labels" => {
            let title = args.get("title").and_then(Value::as_str);
            let term = args
                .get("searchTerm")
                .and_then(Value::as_str)
                .map(str::to_lowercase);
            let ancestors = args
                .get("includeAncestorGroups")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let labels: Vec<(String, Value)> = if is_group {
                let mut scopes = vec![key.clone()];
                if ancestors {
                    let mut p = parent_path(&key);
                    while let Some(x) = p {
                        scopes.push(x.to_owned());
                        p = parent_path(x);
                    }
                }
                all(st, "labels")
                    .into_iter()
                    .filter(|(k, _)| {
                        k.rsplit_once('~')
                            .is_some_and(|(s, _)| scopes.iter().any(|x| x == s))
                    })
                    .map(|(k, l)| (k.to_owned(), l.clone()))
                    .collect()
            } else {
                crate::issues::project_labels(st, &key, ancestors)
            };
            let nodes = labels
                .iter()
                .filter(|(_, l)| title.is_none_or(|t| str_of(l, "name") == t))
                .filter(|(_, l)| {
                    term.as_ref()
                        .is_none_or(|q| str_of(l, "name").to_lowercase().contains(q))
                })
                .map(|(k, _)| label_obj(ctx, k))
                .collect();
            connection("LabelConnection", nodes, args)
        }
        "label" => {
            let title = s("title");
            crate::issues::project_labels(st, &key, true)
                .iter()
                .find(|(_, l)| str_of(l, "name") == title)
                .map(|(k, _)| label_obj(ctx, k))
                .unwrap_or(Value::Null)
        }
        "milestones" => {
            let mut scopes = vec![key.clone()];
            if args
                .get("includeAncestors")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                let mut p = parent_path(&key);
                while let Some(x) = p {
                    scopes.push(x.to_owned());
                    p = parent_path(x);
                }
            }
            let state = args.get("state").and_then(Value::as_str);
            let title = args
                .get("searchTitle")
                .and_then(Value::as_str)
                .map(str::to_lowercase);
            let nodes = all(st, "milestones")
                .into_iter()
                .filter(|(k, m)| {
                    scopes.iter().any(|s| s == project_of(k))
                        && state.is_none_or(|s| {
                            m.get("state").and_then(Value::as_str).unwrap_or("active") == s
                        })
                        && title
                            .as_ref()
                            .is_none_or(|t| str_of(m, "title").to_lowercase().contains(t))
                })
                .map(|(k, _)| milestone_obj(ctx, k))
                .collect();
            connection("MilestoneConnection", nodes, args)
        }
        "incidentManagementTimelineEvents" => {
            let incident = wi_key_by_gid(st, &s("incidentId"));
            let nodes = all(st, "timeline_events")
                .into_iter()
                .filter(|(_, e)| incident.as_deref() == Some(str_of(e, "incident")))
                .map(|(_, e)| timeline_obj(e))
                .collect();
            connection("TimelineEventTypeConnection", nodes, args)
        }
        "vulnerabilities" => {
            let states = arg_list(args, "state");
            let sev = arg_list(args, "severity");
            let rt = arg_list(args, "reportType");
            let nodes = all(st, "vulnerabilities")
                .into_iter()
                .filter(|(_, v)| {
                    let p = str_of(v, "project");
                    (if is_group {
                        p.starts_with(&format!("{key}/"))
                    } else {
                        p == key
                    }) && states.as_ref().is_none_or(|x| {
                        x.iter().any(|s| s.eq_ignore_ascii_case(str_of(v, "state")))
                    }) && sev.as_ref().is_none_or(|x| {
                        x.iter()
                            .any(|s| s.eq_ignore_ascii_case(str_of(v, "severity")))
                    }) && rt.as_ref().is_none_or(|x| {
                        x.iter()
                            .any(|s| s.eq_ignore_ascii_case(str_of(v, "report_type")))
                    })
                })
                .map(|(_, v)| vulnerability_obj(ctx, v))
                .collect();
            connection("VulnerabilityConnection", nodes, args)
        }
        "iterations" => {
            let groups: Vec<String> = if is_group {
                let mut g = vec![key.clone()];
                if args
                    .get("includeAncestors")
                    .and_then(Value::as_bool)
                    .unwrap_or(true)
                {
                    let mut p = parent_path(&key);
                    while let Some(x) = p {
                        g.push(x.to_owned());
                        p = parent_path(x);
                    }
                }
                g
            } else {
                let mut g = Vec::new();
                let mut p = parent_path(&key);
                while let Some(x) = p {
                    g.push(x.to_owned());
                    p = parent_path(x);
                }
                g
            };
            let state = args.get("state").and_then(Value::as_str);
            let nodes = all(st, "iterations")
                .into_iter()
                .filter(|(_, it)| groups.iter().any(|g| g == str_of(it, "group")))
                .map(|(_, it)| iteration_obj(ctx, it))
                .filter(|it| {
                    state.is_none_or(|s| {
                        s == "all" || it["state"] == s || (s == "opened" && it["state"] != "closed")
                    })
                })
                .collect();
            connection("IterationConnection", nodes, args)
        }
        "iterationCadences" => {
            if !is_group {
                return Err(missing_field(obj, field));
            }
            let mut scopes = vec![key.clone()];
            if args
                .get("includeAncestorGroups")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                let mut p = parent_path(&key);
                while let Some(x) = p {
                    scopes.push(x.to_owned());
                    p = parent_path(x);
                }
            }
            let id = args.get("id").and_then(Value::as_str).and_then(gid_num);
            let title = args
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_lowercase);
            let nodes = all(st, "iteration_cadences")
                .into_iter()
                .filter(|(_, c)| scopes.iter().any(|s| s == str_of(c, "group")))
                .filter(|(_, c)| id.is_none_or(|i| as_u64(&c["id"]) == Some(i)))
                .filter(|(_, c)| {
                    title
                        .as_ref()
                        .is_none_or(|t| str_of(c, "title").to_lowercase().contains(t))
                })
                .map(|(_, c)| cadence_obj(c))
                .filter(|c| {
                    args.get("automatic")
                        .and_then(Value::as_bool)
                        .is_none_or(|a| c["automatic"] == a)
                        && args
                            .get("active")
                            .and_then(Value::as_bool)
                            .is_none_or(|a| c["active"] == a)
                        && args
                            .get("durationInWeeks")
                            .and_then(Value::as_u64)
                            .is_none_or(|d| as_u64(&c["durationInWeeks"]) == Some(d))
                })
                .collect();
            connection("IterationCadenceConnection", nodes, args)
        }
        "dependencyProxySetting" if is_group => {
            let s = get(st, "dependency_proxy", &key)
                .cloned()
                .unwrap_or(json!({}));
            json!({"__typename": "DependencyProxySetting", "enabled": s.get("enabled").cloned().unwrap_or(json!(true)), "identity": s.get("identity").cloned().unwrap_or(Value::Null)})
        }
        "dependencyProxyImageTtlPolicy" if is_group => {
            let s = get(st, "dependency_proxy", &key)
                .cloned()
                .unwrap_or(json!({}));
            json!({"__typename": "DependencyProxyImageTtlGroupPolicy", "enabled": s.get("ttl_enabled").cloned().unwrap_or(json!(false)), "ttl": s.get("ttl").cloned().unwrap_or(json!(90)), "createdAt": null, "updatedAt": null})
        }
        "dependencyProxyBlobCount" if is_group => json!(blobs_of(st, &key).len()),
        "dependencyProxyTotalSize" if is_group => {
            let total: u64 = blobs_of(st, &key)
                .iter()
                .filter_map(|b| as_u64(&b["size_bytes"]))
                .sum();
            json!(human_size(total))
        }
        "dependencyProxyTotalSizeInBytes" if is_group => json!(
            blobs_of(st, &key)
                .iter()
                .filter_map(|b| as_u64(&b["size_bytes"]))
                .sum::<u64>()
        ),
        "dependencyProxyBlobs" if is_group => {
            let nodes = blobs_of(st, &key)
                .iter()
                .map(|b| json!({"__typename": "DependencyProxyBlob", "fileName": str_of(b, "file_name"), "size": human_size(as_u64(&b["size_bytes"]).unwrap_or(0)), "createdAt": b["created_at"], "updatedAt": b.get("updated_at").unwrap_or(&b["created_at"])}))
                .collect();
            connection("DependencyProxyBlobConnection", nodes, args)
        }
        "dependencyProxyManifests" if is_group => {
            connection("DependencyProxyManifestConnection", vec![], args)
        }
        "projects" if is_group => {
            let subgroups = args
                .get("includeSubgroups")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let search = args
                .get("search")
                .and_then(Value::as_str)
                .map(str::to_lowercase);
            let prefix = format!("{key}/");
            let nodes = all(st, "projects")
                .into_iter()
                .filter(|(k, _)| {
                    parent_path(k) == Some(key.as_str()) || (subgroups && k.starts_with(&prefix))
                })
                .filter(|(k, _)| search.as_ref().is_none_or(|q| k.to_lowercase().contains(q)))
                .map(|(k, _)| project_obj(ctx, k))
                .collect();
            connection("ProjectConnection", nodes, args)
        }
        "descendantGroups" | "subgroups" if is_group => {
            let prefix = format!("{key}/");
            let nodes = all(st, "groups")
                .into_iter()
                .filter(|(k, _)| k.starts_with(&prefix))
                .map(|(k, _)| group_obj(ctx, k))
                .collect();
            connection("GroupConnection", nodes, args)
        }
        "parent" if is_group => parent_path(&key)
            .and_then(|p| group_key(st, p))
            .map(|g| group_obj(ctx, &g))
            .unwrap_or(Value::Null),
        "group" if !is_group => parent_path(&key)
            .and_then(|p| group_key(st, p))
            .map(|g| group_obj(ctx, &g))
            .unwrap_or(Value::Null),
        "namespace" if !is_group => {
            let ns = parent_path(&key).unwrap_or("");
            match group_key(st, ns) {
                Some(g) => {
                    let mut o = group_obj(ctx, &g);
                    o["__typename"] = json!("Namespace");
                    o
                }
                None => {
                    json!({"__typename": "Namespace", "fullPath": ns, "path": ns, "name": ns, "id": format!("gid://gitlab/Namespaces::UserNamespace/{}", user(st, ns).and_then(|u| as_u64(&u["id"])).unwrap_or(0) + 100_000)})
                }
            }
        }
        "pipelines" if !is_group => {
            let status = args
                .get("status")
                .and_then(Value::as_str)
                .map(str::to_lowercase);
            let reference = args.get("ref").and_then(Value::as_str);
            let mut ps: Vec<Value> = all(st, "pipelines")
                .into_iter()
                .filter(|(_, p)| str_of(p, "project") == key)
                .filter(|(_, p)| {
                    status.as_ref().is_none_or(|s| str_of(p, "status") == s)
                        && reference.is_none_or(|r| str_of(p, "ref") == r)
                })
                .map(|(_, p)| p.clone())
                .collect();
            ps.sort_by(|a, b| as_u64(&b["id"]).cmp(&as_u64(&a["id"])));
            connection(
                "PipelineConnection",
                ps.iter().map(|p| pipeline_obj(ctx, p)).collect(),
                args,
            )
        }
        "pipeline" if !is_group => {
            let iid = args
                .get("iid")
                .and_then(Value::as_str)
                .and_then(|i| i.parse::<u64>().ok());
            let id = args.get("id").and_then(Value::as_str).and_then(gid_num);
            all(st, "pipelines")
                .into_iter()
                .filter(|(_, p)| str_of(p, "project") == key)
                .find(|(_, p)| {
                    (iid.is_some() && as_u64(&p["iid"]) == iid)
                        || (id.is_some() && as_u64(&p["id"]) == id)
                })
                .map(|(_, p)| pipeline_obj(ctx, p))
                .unwrap_or(Value::Null)
        }
        "repository" if !is_group => {
            let default = crate::repository::default_branch(st, &key);
            json!({"__typename": "Repository", "_key": key, "rootRef": default, "exists": true, "empty": crate::repository::branch_head(st, &key, &default).is_none()})
        }
        "releases" if !is_group => {
            let prefix = format!("{key}@");
            let nodes = all(st, "releases")
                .into_iter()
                .filter(|(k, _)| k.starts_with(&prefix))
                .map(|(_, r)| json!({"__typename": "Release", "tagName": str_of(r, "tag_name"), "name": r.get("name").cloned().unwrap_or(Value::Null), "description": r.get("description").cloned().unwrap_or(Value::Null), "releasedAt": r.get("released_at").cloned().unwrap_or(Value::Null), "createdAt": r["created_at"]}))
                .collect();
            connection("ReleaseConnection", nodes, args)
        }
        _ => return Ok(None),
    }))
}

fn human_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.2} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn blobs_of(st: &Store, group: &str) -> Vec<Value> {
    all(st, "dependency_proxy_blobs")
        .into_iter()
        .filter(|(_, b)| str_of(b, "group") == group)
        .map(|(_, b)| b.clone())
        .collect()
}

pub(crate) fn timeline_obj(e: &Value) -> Value {
    json!({
        "__typename": "TimelineEventType",
        "id": format!("gid://gitlab/IncidentManagement::TimelineEvent/{}", e["id"]),
        "note": str_of(e, "note"),
        "noteHtml": format!("<p>{}</p>", str_of(e, "note")),
        "action": e.get("action").cloned().unwrap_or(json!("comment")),
        "occurredAt": e["occurred_at"],
        "createdAt": e["created_at"],
        "updatedAt": e.get("updated_at").unwrap_or(&e["created_at"]),
        "editable": true,
        "timelineEventTags": {
            "__typename": "TimelineEventTagTypeConnection",
            "nodes": value_list(&e["tags"]).iter().enumerate().map(|(i, t)| json!({"__typename": "TimelineEventTagType", "id": format!("gid://gitlab/IncidentManagement::TimelineEventTag/{}", i + 1), "name": t})).collect::<Vec<_>>(),
        },
    })
}

fn pipeline_obj(ctx: &Ctx, p: &Value) -> Value {
    let jobs: Vec<Value> = all(ctx.st, "jobs")
        .into_iter()
        .filter(|(_, j)| j["pipeline_id"] == p["id"] && !j["retried"].as_bool().unwrap_or(false))
        .map(|(_, j)| json!({"__typename": "CiJob", "id": format!("gid://gitlab/Ci::Build/{}", j["id"]), "name": str_of(j, "name"), "status": str_of(j, "status").to_uppercase(), "stage": {"__typename": "CiStage", "name": str_of(j, "stage")}, "webPath": format!("/{}/-/jobs/{}", str_of(p, "project"), j["id"]), "allowFailure": j.get("allow_failure").cloned().unwrap_or(json!(false)), "duration": j.get("duration").cloned().unwrap_or(Value::Null), "failureMessage": j.get("failure_reason").cloned().unwrap_or(Value::Null)}))
        .collect();
    json!({
        "__typename": "Pipeline",
        "id": format!("gid://gitlab/Ci::Pipeline/{}", p["id"]),
        "iid": p.get("iid").map(|i| i.to_string()).unwrap_or_default(),
        "status": str_of(p, "status").to_uppercase(),
        "ref": str_of(p, "ref"),
        "sha": str_of(p, "sha"),
        "source": p.get("source").cloned().unwrap_or(json!("push")),
        "createdAt": p["created_at"],
        "updatedAt": p.get("updated_at").unwrap_or(&p["created_at"]),
        "startedAt": p.get("started_at").cloned().unwrap_or(Value::Null),
        "finishedAt": p.get("finished_at").cloned().unwrap_or(Value::Null),
        "duration": p.get("duration").cloned().unwrap_or(Value::Null),
        "path": format!("/{}/-/pipelines/{}", str_of(p, "project"), p["id"]),
        "user": user_opt_obj(ctx, &p["user"]),
        "jobs": {"__typename": "CiJobConnection", "nodes": jobs, "count": jobs.len()},
    })
}

fn work_item_field(
    ctx: &Ctx,
    obj: &Value,
    field: &str,
    args: &Map<String, Value>,
) -> Result<Option<Value>, String> {
    let key = str_of(obj, "_key").to_owned();
    Ok(Some(match field {
        "widgets" => {
            let only = arg_list(args, "onlyTypes");
            Value::Array(widgets(ctx, &key, only))
        }
        "widget" => {
            let ty = args.get("type").and_then(Value::as_str).unwrap_or("");
            widgets(ctx, &key, Some(vec![ty.to_owned()]))
                .into_iter()
                .next()
                .unwrap_or(Value::Null)
        }
        "namespace" | "project" => {
            let p = project_of(&key);
            if field == "project" && get(ctx.st, "projects", p).is_none() {
                Value::Null
            } else {
                let mut o = namespace_obj(ctx, p);
                if field == "project" {
                    o["__typename"] = json!("Project");
                }
                o
            }
        }
        "awardEmoji" => connection("AwardEmojiConnection", awards_for(ctx, &key), args),
        "discussions" | "notes" => {
            connection("DiscussionConnection", discussions_for(ctx, &key), args)
        }
        _ => return Ok(None),
    }))
}

fn issuable_field(
    ctx: &Ctx,
    obj: &Value,
    field: &str,
    args: &Map<String, Value>,
) -> Result<Option<Value>, String> {
    let key = str_of(obj, "_key").to_owned();
    let is_mr = obj["__typename"] == "MergeRequest";
    let coll = if is_mr { "merge_requests" } else { "issues" };
    let Some(doc) = get(ctx.st, coll, &key) else {
        return Ok(None);
    };
    let project = project_of(&key);
    Ok(Some(match field {
        "assignees" => connection("UserCoreConnection", value_list(&doc["assignees"]).iter().map(|u| user_obj(ctx, u)).collect(), args),
        "reviewers" if is_mr => connection("UserCoreConnection", value_list(&doc["reviewers"]).iter().map(|u| user_obj(ctx, u)).collect(), args),
        "approvedBy" if is_mr => connection("UserCoreConnection", value_list(&doc["approved_by"]).iter().map(|u| user_obj(ctx, u)).collect(), args),
        "participants" => connection("UserCoreConnection", vec![user_obj(ctx, str_of(doc, "author"))], args),
        "labels" => connection("LabelConnection", labels_for(ctx, project, &value_list(&doc["labels"])), args),
        "milestone" => crate::issues::milestone_key_by_id(ctx.st, &doc["milestone_id"]).map(|k| milestone_obj(ctx, &k)).unwrap_or(Value::Null),
        "notes" => {
            let notes: Vec<Value> = all(ctx.st, "notes").into_iter().filter(|(_, n)| str_of(n, "noteable") == key).map(|(_, n)| note_obj(ctx, n)).collect();
            connection("NoteConnection", notes, args)
        }
        "discussions" => connection("DiscussionConnection", discussions_for(ctx, &key), args),
        "project" => project_obj(ctx, project),
        "headPipeline" if is_mr => crate::ci::latest_mr_pipeline(ctx.st, &key).map(|p| pipeline_obj(ctx, &p)).unwrap_or(Value::Null),
        "pipelines" if is_mr => connection("PipelineConnection", crate::ci::mr_pipelines(ctx.st, &key).iter().map(|p| pipeline_obj(ctx, p)).collect(), args),
        "diffStats" if is_mr => Value::Array(
            crate::merge_requests::mr_changes(ctx.st, &key)
                .iter()
                .map(|d| {
                    let diff = str_of(d, "diff");
                    let add = diff.lines().filter(|l| l.starts_with('+')).count();
                    let del = diff.lines().filter(|l| l.starts_with('-')).count();
                    json!({"__typename": "DiffStats", "path": str_of(d, "new_path"), "additions": add, "deletions": del})
                })
                .collect(),
        ),
        "commits" if is_mr => connection(
            "CommitConnection",
            crate::merge_requests::mr_commit_shas(ctx.st, &key)
                .iter()
                .map(|s| {
                    let c = crate::repository::render_commit(ctx.st, &ctx.base, project, s);
                    json!({"__typename": "Commit", "id": format!("gid://gitlab/CommitPresenter/{s}"), "sha": s, "shortId": c["short_id"], "title": c["title"], "message": c["message"], "authorName": c["author_name"], "authorEmail": c["author_email"], "authoredDate": c["authored_date"], "webUrl": c["web_url"]})
                })
                .collect(),
            args,
        ),
        _ => return Ok(None),
    }))
}

fn widget_field(
    ctx: &Ctx,
    obj: &Value,
    field: &str,
    args: &Map<String, Value>,
) -> Result<Option<Value>, String> {
    let key = str_of(obj, "_key").to_owned();
    let Some(doc) = get(ctx.st, "issues", &key) else {
        return Ok(None);
    };
    let ty = str_of(obj, "type");
    let project = project_of(&key);
    Ok(Some(match (ty, field) {
        ("ASSIGNEES", "assignees") => connection("UserCoreConnection", value_list(&doc["assignees"]).iter().map(|u| user_obj(ctx, u)).collect(), args),
        ("LABELS", "labels") => connection("LabelConnection", labels_for(ctx, project, &value_list(&doc["labels"])), args),
        ("HIERARCHY", "children") => connection("WorkItemConnection", children_of(ctx.st, &key).iter().map(|c| work_item_obj(ctx, c)).collect(), args),
        ("HIERARCHY", "ancestors") => {
            let mut out = Vec::new();
            let mut cur = doc.get("parent").and_then(Value::as_str).map(str::to_owned);
            while let Some(p) = cur {
                if out.len() > 10 {
                    break;
                }
                out.push(work_item_obj(ctx, &p));
                cur = get(ctx.st, "issues", &p).and_then(|d| d.get("parent")?.as_str().map(str::to_owned));
            }
            out.reverse();
            connection("WorkItemConnection", out, args)
        }
        ("NOTES", "discussions") => {
            let mut ds = discussions_for(ctx, &key);
            match args.get("filter").and_then(Value::as_str) {
                Some("ONLY_COMMENTS") => ds.retain(|d| !d["_system"].as_bool().unwrap_or(false)),
                Some("ONLY_HISTORY") => ds.retain(|d| d["_system"].as_bool().unwrap_or(false)),
                _ => {}
            }
            if args.get("sort").and_then(Value::as_str) == Some("CREATED_DESC") {
                ds.reverse();
            }
            connection("DiscussionConnection", ds, args)
        }
        ("LINKED_ITEMS", "linkedItems") => {
            let types = arg_list(args, "filter");
            let nodes = linked(ctx.st, &key)
                .iter()
                .filter(|(other, _)| get(ctx.st, "issues", other).is_some())
                .map(|(other, t)| {
                    let lt = match t.as_str() {
                        "blocks" => "blocks",
                        "is_blocked_by" => "is_blocked_by",
                        _ => "relates_to",
                    };
                    json!({"__typename": "LinkedWorkItemType", "linkType": lt, "linkId": "gid://gitlab/WorkItems::RelatedWorkItemLink/1", "linkCreatedAt": null, "linkUpdatedAt": null, "workItem": work_item_obj(ctx, other)})
                })
                .filter(|n| types.as_ref().is_none_or(|f| f.iter().any(|x| x.eq_ignore_ascii_case(n["linkType"].as_str().unwrap_or("")))))
                .collect();
            connection("LinkedWorkItemTypeConnection", nodes, args)
        }
        ("DEVELOPMENT", "relatedMergeRequests") => connection("MergeRequestConnection", related_mrs(ctx.st, &key).iter().map(|k| mr_brief(ctx, k)).collect(), args),
        ("DEVELOPMENT", "closingMergeRequests") => connection(
            "WorkItemClosingMergeRequestConnection",
            closing_mrs(ctx.st, &key).iter().map(|k| json!({"__typename": "WorkItemClosingMergeRequest", "id": format!("gid://gitlab/MergeRequestsClosingIssues/{}", iid_of(k)), "fromMrDescription": true, "mergeRequest": mr_brief(ctx, k)})).collect(),
            args,
        ),
        ("DEVELOPMENT", "relatedBranches") => {
            let iid = iid_of(&key);
            let prefix = format!("{project}:");
            let nodes = all(ctx.st, "branches")
                .into_iter()
                .filter_map(|(k, _)| k.strip_prefix(&prefix).map(str::to_owned))
                .filter(|b| b.starts_with(&format!("{iid}-")))
                .map(|b| json!({"__typename": "WorkItemRelatedBranch", "name": b, "comparePath": format!("/{project}/-/compare/main...{b}"), "pipelineStatus": null}))
                .collect();
            connection("WorkItemRelatedBranchConnection", nodes, args)
        }
        ("DEVELOPMENT", "featureFlags") => connection("FeatureFlagConnection", vec![], args),
        ("CUSTOM_FIELDS", "customFieldValues") => Value::Array(custom_field_values(ctx, &key)),
        ("AWARD_EMOJI", "awardEmoji") => connection("AwardEmojiConnection", awards_for(ctx, &key), args),
        ("PARTICIPANTS", "participants") => {
            let mut names = vec![str_of(doc, "author").to_owned()];
            for (_, n) in all(ctx.st, "notes") {
                if str_of(n, "noteable") == key && !names.contains(&str_of(n, "author").to_owned()) {
                    names.push(str_of(n, "author").to_owned());
                }
            }
            connection("UserCoreConnection", names.iter().map(|u| user_obj(ctx, u)).collect(), args)
        }
        ("TIME_TRACKING", "timelogs") => connection("WorkItemTimelogConnection", vec![], args),
        _ => return Ok(None),
    }))
}

fn catalog_versions(ctx: &Ctx, obj: &Value, args: &Map<String, Value>) -> Value {
    let id = obj["_id"].to_string();
    let c = get(ctx.st, "ci_catalog_resources", &id)
        .cloned()
        .unwrap_or(json!({}));
    let project = str_of(&c, "project").to_owned();
    let nodes: Vec<Value> = c
        .get("versions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|v| {
            let name = str_of(v, "name");
            let sem: Vec<u64> = name.trim_start_matches('v').split('.').filter_map(|p| p.parse().ok()).collect();
            let components: Vec<Value> = v
                .get("components")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|comp| {
                    json!({
                        "__typename": "CiCatalogResourceComponent",
                        "id": format!("gid://gitlab/Ci::Catalog::Resources::Component/{}", comp["id"]),
                        "name": str_of(comp, "name"),
                        "description": comp.get("description").cloned().unwrap_or(Value::Null),
                        "includePath": comp.get("include_path").cloned().unwrap_or_else(|| json!(format!("$CI_SERVER_FQDN/{project}/{}@{name}", str_of(comp, "name")))),
                        "last30DayUsageCount": comp.get("usage_30d").cloned().unwrap_or(json!(0)),
                        "inputs": comp.get("inputs").and_then(Value::as_array).cloned().unwrap_or_default().iter().map(|i| json!({
                            "__typename": "CiCatalogResourceComponentInput",
                            "name": str_of(i, "name"),
                            "description": i.get("description").cloned().unwrap_or(Value::Null),
                            "type": i.get("type").cloned().unwrap_or(json!("STRING")),
                            "required": i.get("required").cloned().unwrap_or(json!(false)),
                            "default": i.get("default").cloned().unwrap_or(Value::Null),
                            "options": i.get("options").cloned().unwrap_or(Value::Null),
                            "regex": i.get("regex").cloned().unwrap_or(Value::Null),
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            json!({
                "__typename": "CiCatalogResourceVersion",
                "id": format!("gid://gitlab/Ci::Catalog::Resources::Version/{}", v["id"]),
                "name": name,
                "path": format!("/{project}/-/tags/{name}"),
                "createdAt": v["created_at"],
                "releasedAt": v.get("released_at").unwrap_or(&v["created_at"]),
                "readme": v.get("readme").cloned().unwrap_or(json!("")),
                "readmeHtml": v.get("readme").cloned().unwrap_or(json!("")),
                "semver": {"__typename": "CiCatalogResourceVersionSemver", "major": sem.first(), "minor": sem.get(1), "patch": sem.get(2)},
                "_components": components,
            })
        })
        .collect();
    connection("CiCatalogResourceVersionConnection", nodes, args)
}

pub(crate) fn note_obj_pub(ctx: &Ctx, n: &Value) -> Value {
    note_obj(ctx, n)
}

pub(crate) fn key_by_gid(st: &Store, gid: &str) -> Option<String> {
    wi_key_by_gid(st, gid)
}
