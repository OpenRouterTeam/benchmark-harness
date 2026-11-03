//! GraphQL object views of store documents. Each object carries `__typename` plus
//! internal `_`-prefixed keys used by resolvers (GraphQL names never start with `_`).

use mock_core::world::ServiceState;
use serde_json::{Value, json};

use crate::{
    projects,
    pulls::review_node_id,
    render::{
        self, account, account_id, account_node_id, account_type, all, get, issue_node_id,
        label_doc, with_prefix,
    },
    util::{b, login_of, node_id, opt_s, s, u},
};

pub fn upper(v: &str) -> String {
    v.to_ascii_uppercase()
}

pub fn actor(svc: &ServiceState, base: &str, login: &str) -> Value {
    if login.is_empty() {
        return Value::Null;
    }
    let doc = account(svc, login).cloned().unwrap_or_default();
    let login = opt_s(&doc, "login").unwrap_or_else(|| login.to_owned());
    let kind = account_type(svc, &login);
    let id = account_id(svc, &login);
    json!({
        "__typename": kind,
        "id": account_node_id(svc, &login),
        "databaseId": id,
        "login": login,
        "name": doc.get("name").cloned().unwrap_or(Value::Null),
        "email": opt_s(&doc, "email").unwrap_or_default(),
        "company": doc.get("company").cloned().unwrap_or(Value::Null),
        "location": doc.get("location").cloned().unwrap_or(Value::Null),
        "bio": doc.get("bio").cloned().unwrap_or(Value::Null),
        "description": doc.get("description").cloned().unwrap_or(Value::Null),
        "url": format!("{base}/{login}"),
        "resourcePath": format!("/{login}"),
        "avatarUrl": format!("{base}/avatars/u/{id}?"),
        "createdAt": doc.get("created_at").cloned().unwrap_or(json!("2019-01-01T00:00:00Z")),
        "isSiteAdmin": b(&doc, "site_admin"),
        "_login": login,
    })
}

pub fn repo(svc: &ServiceState, base: &str, key: &str) -> Value {
    let Some(r) = get(svc, "repos", key) else {
        return Value::Null;
    };
    let (owner, name) = key.split_once('/').unwrap_or((key, key));
    let default = opt_s(r, "default_branch").unwrap_or_else(|| "main".into());
    let private = b(r, "private");
    json!({
        "__typename": "Repository",
        "id": opt_s(r, "node_id").unwrap_or_else(|| node_id("R", key)),
        "databaseId": u(r, "id"),
        "name": name,
        "nameWithOwner": key,
        "description": r.get("description").cloned().unwrap_or(Value::Null),
        "isPrivate": private,
        "isArchived": b(r, "archived"),
        "isFork": b(r, "fork"),
        "visibility": if private { "PRIVATE" } else { "PUBLIC" },
        "url": format!("{base}/{key}"),
        "createdAt": r.get("created_at").cloned().unwrap_or(Value::Null),
        "updatedAt": r.get("updated_at").cloned().unwrap_or(Value::Null),
        "pushedAt": r.get("pushed_at").cloned().unwrap_or(Value::Null),
        "stargazerCount": u(r, "stargazers_count"),
        "forkCount": u(r, "forks_count"),
        "hasIssuesEnabled": r.get("has_issues").cloned().unwrap_or(json!(true)),
        "hasDiscussionsEnabled": b(r, "has_discussions"),
        "defaultBranchRef": {"__typename": "Ref", "name": default, "prefix": "refs/heads/", "target": {"__typename": "Commit", "oid": crate::git::branch_sha(svc, key, &default)}},
        "_key": key,
        "_owner": owner,
    })
}

fn issue_state_reason(i: &Value) -> Value {
    match opt_s(i, "state_reason") {
        Some(r) => json!(upper(&r)),
        None => Value::Null,
    }
}

pub fn issue(svc: &ServiceState, base: &str, key: &str) -> Value {
    let Some(i) = get(svc, "issues", key) else {
        return Value::Null;
    };
    let (repo, number) = render::split_key(key);
    json!({
        "__typename": "Issue",
        "id": issue_node_id(key, i, false),
        "databaseId": u(i, "id"),
        "fullDatabaseId": u(i, "id").to_string(),
        "number": number,
        "title": s(i, "title"),
        "body": s(i, "body"),
        "bodyText": s(i, "body"),
        "state": upper(&opt_s(i, "state").unwrap_or_else(|| "open".into())),
        "stateReason": issue_state_reason(i),
        "closed": s(i, "state") == "closed",
        "locked": b(i, "locked"),
        "url": format!("{base}/{repo}/issues/{number}"),
        "resourcePath": format!("/{repo}/issues/{number}"),
        "createdAt": i.get("created_at").cloned().unwrap_or(Value::Null),
        "updatedAt": i.get("updated_at").cloned().unwrap_or(Value::Null),
        "closedAt": i.get("closed_at").cloned().unwrap_or(Value::Null),
        "_key": key,
        "_repo": repo,
    })
}

pub fn pull(svc: &ServiceState, base: &str, key: &str) -> Value {
    let Some(p) = get(svc, "pulls", key) else {
        return Value::Null;
    };
    let (repo, number) = render::split_key(key);
    let merged = b(p, "merged") || p.get("merged_at").is_some_and(|m| !m.is_null());
    let state = if merged {
        "MERGED".to_owned()
    } else {
        upper(&opt_s(p, "state").unwrap_or_else(|| "open".into()))
    };
    json!({
        "__typename": "PullRequest",
        "id": issue_node_id(key, p, true),
        "databaseId": u(p, "id"),
        "fullDatabaseId": u(p, "id").to_string(),
        "number": number,
        "title": s(p, "title"),
        "body": s(p, "body"),
        "state": state,
        "isDraft": b(p, "draft"),
        "merged": merged,
        "mergedAt": p.get("merged_at").cloned().unwrap_or(Value::Null),
        "closed": state != "OPEN",
        "url": format!("{base}/{repo}/pull/{number}"),
        "resourcePath": format!("/{repo}/pull/{number}"),
        "headRefName": render::pr_head_ref(p),
        "baseRefName": render::pr_base_ref(p),
        "headRefOid": render::pr_head_sha(svc, repo, p),
        "baseRefOid": render::pr_base_sha(svc, repo, p),
        "createdAt": p.get("created_at").cloned().unwrap_or(Value::Null),
        "updatedAt": p.get("updated_at").cloned().unwrap_or(Value::Null),
        "closedAt": p.get("closed_at").cloned().unwrap_or(Value::Null),
        "_key": key,
        "_repo": repo,
    })
}

pub fn issue_or_pull(svc: &ServiceState, base: &str, key: &str) -> Value {
    let v = issue(svc, base, key);
    if v.is_null() { pull(svc, base, key) } else { v }
}

pub fn label(svc: &ServiceState, base: &str, repo: &str, name: &str) -> Value {
    let Some((_, d)) = label_doc(svc, repo, name) else {
        return Value::Null;
    };
    let rest = render::label_json(svc, base, repo, s(d, "name"));
    json!({
        "__typename": "Label",
        "id": rest["node_id"],
        "name": rest["name"],
        "color": rest["color"],
        "description": rest["description"],
        "isDefault": rest["default"],
        "url": format!("{base}/{repo}/labels/{}", crate::util::percent_encode_path(s(d, "name"))),
        "createdAt": d.get("created_at").cloned().unwrap_or(json!("2025-01-01T00:00:00Z")),
        "_repo": repo,
        "_name": s(d, "name"),
    })
}

pub fn milestone(svc: &ServiceState, base: &str, repo: &str, number: u64) -> Value {
    let Some(m) = get(svc, "milestones", &format!("{repo}:{number}")) else {
        return Value::Null;
    };
    json!({
        "__typename": "Milestone",
        "id": opt_s(m, "node_id").unwrap_or_else(|| node_id("MI", &format!("{repo}:{number}"))),
        "number": number,
        "title": s(m, "title"),
        "description": m.get("description").cloned().unwrap_or(Value::Null),
        "state": upper(&opt_s(m, "state").unwrap_or_else(|| "open".into())),
        "dueOn": m.get("due_on").cloned().unwrap_or(Value::Null),
        "url": format!("{base}/{repo}/milestone/{number}"),
        "_repo": repo,
    })
}

pub fn team(svc: &ServiceState, base: &str, org: &str, slug: &str) -> Value {
    let Some(t) = get(svc, "teams", &format!("{org}/{slug}")) else {
        return Value::Null;
    };
    json!({
        "__typename": "Team",
        "id": opt_s(t, "node_id").unwrap_or_else(|| node_id("T", &format!("{org}/{slug}"))),
        "databaseId": u(t, "id"),
        "name": opt_s(t, "name").unwrap_or_else(|| slug.to_owned()),
        "slug": slug,
        "description": opt_s(t, "description").unwrap_or_default(),
        "privacy": upper(&opt_s(t, "privacy").unwrap_or_else(|| "visible".into())),
        "url": format!("{base}/orgs/{org}/teams/{slug}"),
        "_key": format!("{org}/{slug}"),
    })
}

// ---------------------------------------------------------------- issue fields

pub fn issue_field_node_id(owner: &str, f: &Value) -> String {
    opt_s(f, "node_id").unwrap_or_else(|| node_id("IF", &format!("{owner}:{}", u(f, "id"))))
}

pub fn issue_field_option_id(owner: &str, f: &Value, o: &Value) -> String {
    opt_s(o, "node_id").unwrap_or_else(|| {
        node_id(
            "IFSO",
            &format!(
                "{owner}:{}:{}",
                u(f, "id"),
                opt_s(o, "id").unwrap_or_else(|| s(o, "name").to_owned())
            ),
        )
    })
}

pub fn issue_field_typename(f: &Value) -> &'static str {
    match s(f, "data_type").to_ascii_uppercase().as_str() {
        "NUMBER" => "IssueFieldNumber",
        "DATE" => "IssueFieldDate",
        "SINGLE_SELECT" => "IssueFieldSingleSelect",
        _ => "IssueFieldText",
    }
}

pub fn issue_field(owner: &str, f: &Value) -> Value {
    let options: Vec<Value> = crate::util::arr(f, "options")
        .iter()
        .enumerate()
        .map(|(i, o)| {
            json!({
                "__typename": "IssueFieldSingleSelectOption",
                "id": issue_field_option_id(owner, f, o),
                "fullDatabaseId": opt_s(o, "id").unwrap_or_else(|| (i + 1).to_string()),
                "name": s(o, "name"),
                "description": opt_s(o, "description").unwrap_or_default(),
                "color": opt_s(o, "color").unwrap_or_else(|| "GRAY".into()),
                "priority": o.get("priority").cloned().unwrap_or(json!(i + 1)),
            })
        })
        .collect();
    let mut v = json!({
        "__typename": issue_field_typename(f),
        "id": issue_field_node_id(owner, f),
        "fullDatabaseId": u(f, "id").to_string(),
        "databaseId": u(f, "id"),
        "name": s(f, "name"),
        "description": opt_s(f, "description").unwrap_or_default(),
        "dataType": s(f, "data_type").to_ascii_uppercase(),
        "visibility": upper(&opt_s(f, "visibility").unwrap_or_else(|| "org_only".into())),
        "_owner": owner,
    });
    if !options.is_empty() || issue_field_typename(f) == "IssueFieldSingleSelect" {
        v["options"] = json!(options);
    }
    v
}

pub fn issue_field_value(owner: &str, f: &Value, value: &Value) -> Value {
    let field = issue_field(owner, f);
    match issue_field_typename(f) {
        "IssueFieldNumber" => {
            json!({"__typename": "IssueFieldNumberValue", "field": field, "value": value.as_f64().or_else(|| value.as_str().and_then(|s| s.parse().ok()))})
        }
        "IssueFieldDate" => {
            json!({"__typename": "IssueFieldDateValue", "field": field, "value": value})
        }
        "IssueFieldSingleSelect" => {
            let name = value.as_str().unwrap_or("");
            let opt = crate::util::arr(f, "options")
                .iter()
                .find(|o| s(o, "name").eq_ignore_ascii_case(name))
                .cloned()
                .unwrap_or_default();
            json!({
                "__typename": "IssueFieldSingleSelectValue",
                "field": field,
                "value": name,
                "name": name,
                "optionId": issue_field_option_id(owner, f, &opt),
                "color": opt_s(&opt, "color").unwrap_or_else(|| "GRAY".into()),
            })
        }
        _ => json!({"__typename": "IssueFieldTextValue", "field": field, "value": value}),
    }
}

// ---------------------------------------------------------------- discussions

pub fn discussion(svc: &ServiceState, base: &str, key: &str) -> Value {
    let Some(d) = get(svc, "discussions", key) else {
        return Value::Null;
    };
    let (repo, number) = render::split_key(key);
    json!({
        "__typename": "Discussion",
        "id": opt_s(d, "node_id").unwrap_or_else(|| node_id("D", key)),
        "databaseId": u(d, "id"),
        "number": number,
        "title": s(d, "title"),
        "body": s(d, "body"),
        "bodyText": s(d, "body"),
        "createdAt": d.get("created_at").cloned().unwrap_or(Value::Null),
        "updatedAt": d.get("updated_at").cloned().unwrap_or(Value::Null),
        "closed": b(d, "closed"),
        "closedAt": d.get("closed_at").cloned().unwrap_or(Value::Null),
        "isAnswered": d.get("answer_comment").is_some_and(|a| !a.is_null()),
        "answerChosenAt": d.get("answer_chosen_at").cloned().unwrap_or(Value::Null),
        "locked": b(d, "locked"),
        "url": format!("{base}/{repo}/discussions/{number}"),
        "upvoteCount": u(d, "upvote_count"),
        "_key": key,
        "_repo": repo,
    })
}

pub fn category_doc<'a>(
    svc: &'a ServiceState,
    repo: &str,
    reference: &str,
) -> Option<(&'a str, &'a Value)> {
    with_prefix(svc, "discussion_categories", &format!("{repo}:"))
        .into_iter()
        .find(|(k, c)| {
            category_node_id(k, c) == reference
                || s(c, "slug").eq_ignore_ascii_case(reference)
                || s(c, "name").eq_ignore_ascii_case(reference)
        })
}

pub fn category_node_id(key: &str, c: &Value) -> String {
    opt_s(c, "node_id").unwrap_or_else(|| node_id("DIC", key))
}

pub fn category(key: &str, c: &Value) -> Value {
    json!({
        "__typename": "DiscussionCategory",
        "id": category_node_id(key, c),
        "name": s(c, "name"),
        "slug": opt_s(c, "slug").unwrap_or_else(|| s(c, "name").to_ascii_lowercase().replace(' ', "-")),
        "emoji": opt_s(c, "emoji").unwrap_or_else(|| ":speech_balloon:".into()),
        "description": opt_s(c, "description").unwrap_or_default(),
        "isAnswerable": b(c, "is_answerable"),
        "createdAt": c.get("created_at").cloned().unwrap_or(json!("2025-01-01T00:00:00Z")),
    })
}

pub fn discussion_comment(svc: &ServiceState, base: &str, id: &str) -> Value {
    let Some(c) = get(svc, "discussion_comments", id) else {
        return Value::Null;
    };
    let dkey = s(c, "discussion");
    let (repo, number) = render::split_key(dkey);
    let answer = get(svc, "discussions", dkey)
        .and_then(|d| opt_s(d, "answer_comment"))
        .is_some_and(|a| a == id);
    json!({
        "__typename": "DiscussionComment",
        "id": id,
        "databaseId": u(c, "id"),
        "body": s(c, "body"),
        "bodyText": s(c, "body"),
        "isAnswer": answer,
        "isMinimized": b(c, "minimized"),
        "minimizedReason": c.get("minimized_reason").cloned().unwrap_or(Value::Null),
        "createdAt": c.get("created_at").cloned().unwrap_or(Value::Null),
        "updatedAt": c.get("updated_at").cloned().unwrap_or(Value::Null),
        "url": format!("{base}/{repo}/discussions/{number}#discussioncomment-{}", u(c, "id")),
        "upvoteCount": u(c, "upvote_count"),
        "_key": id,
        "_discussion": dkey,
    })
}

// ---------------------------------------------------------------- reviews

pub fn review(svc: &ServiceState, base: &str, r: &Value) -> Value {
    let repo = s(r, "repo");
    let number = u(r, "pull_number");
    let _ = svc;
    json!({
        "__typename": "PullRequestReview",
        "id": review_node_id(r),
        "databaseId": u(r, "id"),
        "fullDatabaseId": u(r, "id").to_string(),
        "body": s(r, "body"),
        "state": opt_s(r, "state").unwrap_or_else(|| "COMMENTED".into()),
        "submittedAt": r.get("submitted_at").cloned().unwrap_or(Value::Null),
        "createdAt": r.get("submitted_at").cloned().unwrap_or(Value::Null),
        "url": format!("{base}/{repo}/pull/{number}#pullrequestreview-{}", u(r, "id")),
        "isMinimized": b(r, "minimized"),
        "_id": u(r, "id"),
        "_pull": format!("{repo}#{number}"),
    })
}

pub fn thread_node_id(root: &Value) -> String {
    opt_s(root, "thread_id").unwrap_or_else(|| node_id("PRRT", &u(root, "id").to_string()))
}

pub fn thread(svc: &ServiceState, root: &Value) -> Value {
    let tid = thread_node_id(root);
    let state = get(svc, "review_threads", &tid)
        .cloned()
        .unwrap_or_default();
    let resolved = b(&state, "is_resolved") || b(root, "is_resolved");
    let outdated = b(root, "outdated");
    json!({
        "__typename": "PullRequestReviewThread",
        "id": tid,
        "isResolved": resolved,
        "isOutdated": outdated,
        "isCollapsed": resolved || outdated,
        "path": s(root, "path"),
        "line": root.get("line").cloned().unwrap_or(Value::Null),
        "originalLine": root.get("original_line").cloned().or_else(|| root.get("line").cloned()).unwrap_or(Value::Null),
        "startLine": root.get("start_line").cloned().unwrap_or(Value::Null),
        "diffSide": opt_s(root, "side").unwrap_or_else(|| "RIGHT".into()),
        "subjectType": upper(&opt_s(root, "subject_type").unwrap_or_else(|| "line".into())),
        "_root": u(root, "id"),
        "_pull": format!("{}#{}", s(root, "repo"), u(root, "pull_number")),
    })
}

pub fn review_comment(base: &str, c: &Value) -> Value {
    let repo = s(c, "repo");
    let number = u(c, "pull_number");
    let id = u(c, "id");
    json!({
        "__typename": "PullRequestReviewComment",
        "id": opt_s(c, "node_id").unwrap_or_else(|| node_id("PRRC", &id.to_string())),
        "databaseId": id,
        "fullDatabaseId": id.to_string(),
        "body": s(c, "body"),
        "path": s(c, "path"),
        "line": c.get("line").cloned().unwrap_or(Value::Null),
        "originalLine": c.get("original_line").cloned().or_else(|| c.get("line").cloned()).unwrap_or(Value::Null),
        "startLine": c.get("start_line").cloned().unwrap_or(Value::Null),
        "originalStartLine": c.get("original_start_line").cloned().or_else(|| c.get("start_line").cloned()).unwrap_or(Value::Null),
        "diffHunk": s(c, "diff_hunk"),
        "createdAt": c.get("created_at").cloned().unwrap_or(Value::Null),
        "updatedAt": c.get("updated_at").cloned().or_else(|| c.get("created_at").cloned()).unwrap_or(Value::Null),
        "url": format!("{base}/{repo}/pull/{number}#discussion_r{id}"),
        "isMinimized": b(c, "minimized"),
        "_id": id,
        "_user": login_of(c.get("user").unwrap_or(&Value::Null)),
    })
}

/// Root comments (threads) of a pull request.
pub fn thread_roots(svc: &ServiceState, pull_key: &str, viewer: &str) -> Vec<Value> {
    let (repo, number) = render::split_key(pull_key);
    let mut roots: Vec<Value> = all(svc, "review_comments")
        .into_iter()
        .map(|(_, c)| c.clone())
        .filter(|c| s(c, "repo") == repo && u(c, "pull_number") == number)
        .filter(|c| c.get("in_reply_to_id").is_none_or(Value::is_null))
        .filter(|c| {
            !crate::pulls::is_pending(svc, c)
                || login_of(c.get("user").unwrap_or(&Value::Null)) == viewer
        })
        .collect();
    roots.sort_by_key(|c| u(c, "id"));
    roots
}

pub fn thread_comments(svc: &ServiceState, root_id: u64) -> Vec<Value> {
    let mut v: Vec<Value> = all(svc, "review_comments")
        .into_iter()
        .map(|(_, c)| c.clone())
        .filter(|c| {
            u(c, "id") == root_id
                || c.get("in_reply_to_id").and_then(Value::as_u64) == Some(root_id)
        })
        .collect();
    v.sort_by_key(|c| u(c, "id"));
    v
}

// ---------------------------------------------------------------- projects

pub fn project(svc: &ServiceState, base: &str, pkey: &str) -> Value {
    let Some(p) = get(svc, "projects", pkey) else {
        return Value::Null;
    };
    let (owner, number) = pkey.split_once('/').unwrap_or((pkey, "0"));
    let org = render::is_org(svc, owner);
    json!({
        "__typename": "ProjectV2",
        "id": projects::project_node_id(svc, pkey),
        "databaseId": u(p, "id"),
        "fullDatabaseId": u(p, "id").to_string(),
        "number": number.parse::<u64>().unwrap_or(0),
        "title": s(p, "title"),
        "shortDescription": p.get("short_description").cloned().unwrap_or(Value::Null),
        "readme": p.get("readme").cloned().unwrap_or(Value::Null),
        "public": b(p, "public"),
        "closed": b(p, "closed"),
        "closedAt": p.get("closed_at").cloned().unwrap_or(Value::Null),
        "createdAt": p.get("created_at").cloned().unwrap_or(Value::Null),
        "updatedAt": p.get("updated_at").cloned().unwrap_or(Value::Null),
        "url": if org { format!("{base}/orgs/{owner}/projects/{number}") } else { format!("{base}/users/{owner}/projects/{number}") },
        "_key": pkey,
    })
}

pub fn project_field(pkey: &str, f: &Value) -> Value {
    let dt = projects::data_type(f);
    let typename = match dt.as_str() {
        "SINGLE_SELECT" => "ProjectV2SingleSelectField",
        "ITERATION" => "ProjectV2IterationField",
        _ => "ProjectV2Field",
    };
    let mut v = json!({
        "__typename": typename,
        "id": projects::field_node_id(pkey, f),
        "databaseId": u(f, "id"),
        "name": s(f, "name"),
        "dataType": dt,
        "isIssueField": false,
        "issueField": null,
        "createdAt": f.get("created_at").cloned().unwrap_or(json!("2025-01-01T00:00:00Z")),
        "updatedAt": f.get("updated_at").cloned().unwrap_or(json!("2025-01-01T00:00:00Z")),
        "_project": pkey,
    });
    if dt == "SINGLE_SELECT" {
        v["options"] = json!(
            crate::util::arr(f, "options")
                .iter()
                .map(|o| json!({
                    "__typename": "ProjectV2SingleSelectFieldOption",
                    "id": s(o, "id"),
                    "name": s(o, "name"),
                    "nameHTML": s(o, "name"),
                    "color": opt_s(o, "color").unwrap_or_else(|| "GRAY".into()),
                    "description": opt_s(o, "description").unwrap_or_default(),
                }))
                .collect::<Vec<_>>()
        );
    }
    if dt == "ITERATION" {
        let c = f.get("configuration").cloned().unwrap_or(json!({}));
        let its: Vec<Value> = crate::util::arr(&c, "iterations")
            .iter()
            .map(|it| json!({"__typename": "ProjectV2IterationFieldIteration", "id": s(it, "id"), "title": s(it, "title"), "titleHTML": s(it, "title"), "startDate": s(it, "start_date"), "duration": u(it, "duration")}))
            .collect();
        v["configuration"] = json!({
            "__typename": "ProjectV2IterationFieldConfiguration",
            "duration": c.get("duration").cloned().unwrap_or(json!(14)),
            "startDay": c.get("start_day").cloned().unwrap_or(json!(1)),
            "iterations": its,
            "completedIterations": [],
        });
    }
    v
}

pub fn project_item(svc: &ServiceState, key: &str) -> Value {
    let Some(i) = get(svc, "project_items", key) else {
        return Value::Null;
    };
    let pkey = key.rsplit_once(':').map(|(p, _)| p).unwrap_or("");
    let content = s(i, "content");
    let kind = if content.is_empty() {
        "DRAFT_ISSUE"
    } else if get(svc, "pulls", content).is_some() {
        "PULL_REQUEST"
    } else {
        "ISSUE"
    };
    json!({
        "__typename": "ProjectV2Item",
        "id": projects::item_node_id(key, i),
        "databaseId": u(i, "id"),
        "fullDatabaseId": u(i, "id").to_string(),
        "type": kind,
        "isArchived": i.get("archived_at").is_some_and(|a| !a.is_null()),
        "createdAt": i.get("created_at").cloned().unwrap_or(Value::Null),
        "updatedAt": i.get("updated_at").cloned().unwrap_or(Value::Null),
        "_key": key,
        "_project": pkey,
    })
}

pub fn status_update(svc: &ServiceState, id: &str) -> Value {
    let Some(su) = get(svc, "project_status_updates", id) else {
        return Value::Null;
    };
    json!({
        "__typename": "ProjectV2StatusUpdate",
        "id": id,
        "databaseId": u(su, "database_id"),
        "body": su.get("body").cloned().unwrap_or(Value::Null),
        "bodyHTML": su.get("body").cloned().unwrap_or(Value::Null),
        "status": su.get("status").cloned().unwrap_or(Value::Null),
        "startDate": su.get("start_date").cloned().unwrap_or(Value::Null),
        "targetDate": su.get("target_date").cloned().unwrap_or(Value::Null),
        "createdAt": su.get("created_at").cloned().unwrap_or(Value::Null),
        "updatedAt": su.get("updated_at").cloned().or_else(|| su.get("created_at").cloned()).unwrap_or(Value::Null),
        "_key": id,
        "_project": s(su, "project"),
        "_creator": login_of(su.get("creator").unwrap_or(&Value::Null)),
    })
}

pub fn view(svc: &ServiceState, id: &str) -> Value {
    let Some(v) = get(svc, "project_views", id) else {
        return Value::Null;
    };
    let layout = opt_s(v, "layout").unwrap_or_else(|| "table".into());
    let layout = if layout.ends_with("_LAYOUT") {
        layout
    } else {
        format!("{}_LAYOUT", layout.to_ascii_uppercase())
    };
    json!({
        "__typename": "ProjectV2View",
        "id": id,
        "databaseId": u(v, "database_id"),
        "number": u(v, "number"),
        "name": s(v, "name"),
        "layout": layout,
        "filter": v.get("filter").cloned().unwrap_or(Value::Null),
        "createdAt": v.get("created_at").cloned().unwrap_or(Value::Null),
        "updatedAt": v.get("updated_at").cloned().unwrap_or(Value::Null),
        "_key": id,
        "_project": s(v, "project"),
    })
}

/// Resolves any node ID to its GraphQL object.
pub fn find_node(svc: &ServiceState, base: &str, id: &str) -> Value {
    for (k, i) in all(svc, "issues") {
        if issue_node_id(k, i, false) == id {
            return issue(svc, base, k);
        }
    }
    for (k, p) in all(svc, "pulls") {
        if issue_node_id(k, p, true) == id {
            return pull(svc, base, k);
        }
    }
    for (k, _) in all(svc, "repos") {
        let r = repo(svc, base, k);
        if r["id"] == id {
            return r;
        }
    }
    let mut logins: Vec<String> = Vec::new();
    for coll in ["users", "orgs", "viewer"] {
        for (k, d) in all(svc, coll) {
            logins.push(opt_s(d, "login").unwrap_or_else(|| k.to_owned()));
        }
    }
    for l in logins {
        if account_node_id(svc, &l) == id {
            return actor(svc, base, &l);
        }
    }
    for (k, l) in all(svc, "labels") {
        let repo_key = k.split_once(':').map(|(r, _)| r).unwrap_or("");
        let obj = label(svc, base, repo_key, s(l, "name"));
        if obj["id"] == id {
            return obj;
        }
    }
    for (k, m) in all(svc, "milestones") {
        let (repo_key, n) = k.rsplit_once(':').unwrap_or(("", "0"));
        let obj = milestone(svc, base, repo_key, n.parse().unwrap_or(0));
        if obj["id"] == id {
            let _ = m;
            return obj;
        }
    }
    for (k, _) in all(svc, "discussions") {
        let d = discussion(svc, base, k);
        if d["id"] == id {
            return d;
        }
    }
    if get(svc, "discussion_comments", id).is_some() {
        return discussion_comment(svc, base, id);
    }
    for (k, c) in all(svc, "discussion_categories") {
        if category_node_id(k, c) == id {
            return category(k, c);
        }
    }
    for (_, r) in all(svc, "reviews") {
        if review_node_id(r) == id {
            return review(svc, base, r);
        }
    }
    for (_, c) in all(svc, "review_comments") {
        if c.get("in_reply_to_id").is_none_or(Value::is_null) && thread_node_id(c) == id {
            return thread(svc, c);
        }
        let rc = review_comment(base, c);
        if rc["id"] == id {
            return rc;
        }
    }
    for (_, c) in all(svc, "issue_comments") {
        let cid = opt_s(c, "node_id").unwrap_or_else(|| node_id("IC", &u(c, "id").to_string()));
        if cid == id {
            return json!({
                "__typename": "IssueComment",
                "id": cid,
                "databaseId": u(c, "id"),
                "body": s(c, "body"),
                "isMinimized": b(c, "minimized"),
                "_id": u(c, "id"),
            });
        }
    }
    for (k, _) in all(svc, "projects") {
        if projects::project_node_id(svc, k) == id {
            return project(svc, base, k);
        }
    }
    for (k, f) in all(svc, "project_fields") {
        let pkey = k.rsplit_once(':').map(|(p, _)| p).unwrap_or("");
        if projects::field_node_id(pkey, f) == id {
            return project_field(pkey, f);
        }
    }
    for (k, i) in all(svc, "project_items") {
        if projects::item_node_id(k, i) == id {
            return project_item(svc, k);
        }
    }
    if get(svc, "project_status_updates", id).is_some() {
        return status_update(svc, id);
    }
    if get(svc, "project_views", id).is_some() {
        return view(svc, id);
    }
    for (k, f) in all(svc, "issue_fields") {
        let owner = k.split(':').next().unwrap_or("");
        if issue_field_node_id(owner, f) == id {
            return issue_field(owner, f);
        }
    }
    for (k, t) in all(svc, "teams") {
        let (org, slug) = k.split_once('/').unwrap_or(("", ""));
        let obj = team(svc, base, org, slug);
        if obj["id"] == id {
            let _ = t;
            return obj;
        }
    }
    Value::Null
}

/// Store key behind a node ID for the given typename, e.g. `Issue` → `owner/repo#N`.
pub fn key_of(obj: &Value) -> Option<String> {
    obj.get("_key").and_then(Value::as_str).map(str::to_owned)
}
