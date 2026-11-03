//! Issues, issue comments, labels, milestones, assignees, sub-issues, reactions, and
//! issue types / issue field values.

use axum::{
    http::{Method, StatusCode, header},
    response::Response,
};
use mock_core::world::ServiceState;
use serde_json::{Value, json};

use crate::{
    Ctx,
    render::{
        self, account, all, exists_account, get, is_org, issue_comment_json, issue_json,
        issue_type_out, label_doc, label_json, milestone_json, milestone_number, repo_key,
        split_key, sub_issue_keys, user, with_prefix,
    },
    util::{
        arr, as_i64, b, created, error, login_of, names_of, next_id, no_content, node_id,
        not_found, ok, opt_s, paged, s, status_json, str_list, u, unix, unprocessable, validation,
        validation_msg, with_header,
    },
};

pub fn route(ctx: &mut Ctx, segs: &[&str]) -> Option<Response> {
    let m = ctx.req.method.clone();
    let get_ = m == Method::GET;
    if let ["orgs", org, "issue-types"] = segs {
        if !get_ {
            return None;
        }
        if !is_org(ctx.svc, org) {
            return Some(not_found());
        }
        return Some(ok(Value::Array(org_issue_types(ctx.svc, org))));
    }
    let ["repos", o, r, rest @ ..] = segs else {
        return None;
    };
    let first = *rest.first()?;
    if !matches!(
        first,
        "issues" | "labels" | "milestones" | "assignees" | "issue-types"
    ) {
        return None;
    }
    let Some(repo) = repo_key(ctx.svc, o, r) else {
        return Some(not_found());
    };
    Some(match rest {
        ["issues"] if get_ => list_issues(ctx, &repo),
        ["issues"] if m == Method::POST => create_issue(ctx, &repo),
        ["issues", "comments"] if get_ => {
            let comments: Vec<Value> = all(ctx.svc, "issue_comments")
                .iter()
                .filter(|(_, c)| s(c, "repo") == repo)
                .map(|(_, c)| issue_comment_json(ctx.svc, ctx.base(), c))
                .collect();
            paged(ctx.req, comments)
        }
        ["issues", "comments", id] => comment(ctx, &repo, id, &m),
        ["issues", "comments", id, "reactions"] => reactions(
            ctx,
            &repo,
            &format!("issue_comment:{id}"),
            Target::IssueComment(id),
            &m,
        ),
        ["issues", "comments", id, "reactions", rid] if m == Method::DELETE => {
            delete_reaction(ctx, &format!("issue_comment:{id}"), rid)
        }
        ["issues", n] if get_ => get_issue(ctx, &repo, n),
        ["issues", n] if m == Method::PATCH => update_issue(ctx, &repo, n),
        ["issues", n, "comments"] if get_ => list_comments(ctx, &repo, n),
        ["issues", n, "comments"] if m == Method::POST => create_comment(ctx, &repo, n),
        ["issues", n, "labels"] => issue_labels(ctx, &repo, n, None, &m),
        ["issues", n, "labels", name] => issue_labels(ctx, &repo, n, Some(name), &m),
        ["issues", n, "assignees"] => issue_assignees(ctx, &repo, n, &m),
        ["issues", n, "assignees", login] if get_ => {
            if is_assignable(ctx.svc, &repo, login) {
                no_content()
            } else {
                not_found()
            }
        }
        ["issues", n, "reactions"] => reactions(
            ctx,
            &repo,
            &format!("issue:{repo}#{n}"),
            Target::Issue(n),
            &m,
        ),
        ["issues", n, "reactions", rid] if m == Method::DELETE => {
            delete_reaction(ctx, &format!("issue:{repo}#{n}"), rid)
        }
        ["issues", n, "sub_issues"] if get_ => list_sub_issues(ctx, &repo, n),
        ["issues", n, "sub_issues"] if m == Method::POST => add_sub_issue(ctx, &repo, n),
        ["issues", n, "sub_issue"] if m == Method::DELETE => remove_sub_issue(ctx, &repo, n),
        ["issues", n, "sub_issues", "priority"] if m == Method::PATCH => {
            reprioritize(ctx, &repo, n)
        }
        ["issues", n, "parent"] if get_ => parent_issue(ctx, &repo, n),
        ["issues", n, "issue-field-values", field_id] if m == Method::DELETE => {
            delete_field_value(ctx, &repo, n, field_id)
        }
        ["issues", n, "issue-field-values"] if get_ => field_values(ctx, &repo, n),
        ["issues", n, "events"] | ["issues", n, "timeline"] if get_ => {
            if issue_json(ctx.svc, ctx.base(), &format!("{repo}#{n}")).is_null() {
                not_found()
            } else {
                paged(ctx.req, Vec::new())
            }
        }
        ["labels"] if get_ => {
            let labels: Vec<Value> = with_prefix(ctx.svc, "labels", &format!("{repo}:"))
                .iter()
                .map(|(_, l)| label_json(ctx.svc, ctx.base(), &repo, s(l, "name")))
                .collect();
            paged(ctx.req, labels)
        }
        ["labels"] if m == Method::POST => create_label(ctx, &repo),
        ["labels", name] if get_ => match label_doc(ctx.svc, &repo, name) {
            Some((_, l)) => ok(label_json(ctx.svc, ctx.base(), &repo, s(l, "name"))),
            None => not_found(),
        },
        ["labels", name] if m == Method::PATCH => update_label(ctx, &repo, name),
        ["labels", name] if m == Method::DELETE => delete_label(ctx, &repo, name),
        ["milestones"] if get_ => list_milestones(ctx, &repo),
        ["milestones"] if m == Method::POST => create_milestone(ctx, &repo),
        ["milestones", n] if get_ => {
            let v = milestone_json(ctx.svc, ctx.base(), &repo, n.parse().unwrap_or(0));
            if v.is_null() { not_found() } else { ok(v) }
        }
        ["assignees"] if get_ => {
            let users: Vec<Value> = assignable(ctx.svc, &repo)
                .iter()
                .map(|l| user(ctx.svc, ctx.base(), l))
                .collect();
            paged(ctx.req, users)
        }
        ["assignees", login] if get_ => {
            if is_assignable(ctx.svc, &repo, login) {
                no_content()
            } else {
                not_found()
            }
        }
        ["issue-types"] if get_ => {
            let owner = repo.split('/').next().unwrap_or("");
            ok(Value::Array(org_issue_types(ctx.svc, owner)))
        }
        _ => not_found(),
    })
}

pub fn org_issue_types(svc: &ServiceState, org: &str) -> Vec<Value> {
    with_prefix(svc, "issue_types", &format!("{org}:"))
        .iter()
        .filter(|(_, t)| t.get("is_enabled").and_then(Value::as_bool).unwrap_or(true))
        .map(|(_, t)| issue_type_out(org, t))
        .collect()
}

// ---------------------------------------------------------------- helpers

pub fn next_number(svc: &ServiceState, repo: &str) -> u64 {
    let prefix = format!("{repo}#");
    ["issues", "pulls", "discussions"]
        .iter()
        .flat_map(|c| with_prefix(svc, c, &prefix))
        .map(|(_, d)| u(d, "number"))
        .max()
        .unwrap_or(0)
        + 1
}

pub fn next_issue_db_id(svc: &ServiceState) -> u64 {
    next_id(svc, "issues", "id", 1).max(next_id(svc, "pulls", "id", 1))
}

pub fn assignable(svc: &ServiceState, repo: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let owner = repo.split('/').next().unwrap_or("");
    if !is_org(svc, owner) {
        out.push(owner.to_owned());
    }
    if let Some(r) = get(svc, "repos", repo) {
        for c in arr(r, "collaborators") {
            out.push(login_of(c));
        }
    }
    if let Some(o) = account(svc, owner).filter(|_| is_org(svc, owner)) {
        out.extend(names_of(arr(o, "members")));
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|l| !l.is_empty() && seen.insert(l.to_ascii_lowercase()));
    out
}

pub fn is_assignable(svc: &ServiceState, repo: &str, login: &str) -> bool {
    assignable(svc, repo)
        .iter()
        .any(|l| l.eq_ignore_ascii_case(login))
        || (exists_account(svc, login) && !is_org(svc, login))
}

fn canonical_login(svc: &ServiceState, login: &str) -> String {
    account(svc, login)
        .map(|a| s(a, "login").to_owned())
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| login.to_owned())
}

/// The store collection and key holding issue-or-PR `n`.
fn locate(svc: &ServiceState, repo: &str, n: &str) -> Option<(&'static str, String)> {
    let key = format!("{repo}#{n}");
    if get(svc, "issues", &key).is_some() {
        Some(("issues", key))
    } else if get(svc, "pulls", &key).is_some() {
        Some(("pulls", key))
    } else {
        None
    }
}

/// Ensures each label exists (GitHub creates missing labels on issue writes).
fn ensure_labels(ctx: &mut Ctx, repo: &str, names: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for name in names {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        match label_doc(ctx.svc, repo, name) {
            Some((_, l)) => out.push(s(l, "name").to_owned()),
            None => {
                let id = next_id(ctx.svc, "labels", "id", 1);
                ctx.svc.store.collection_mut("labels").insert(
                    format!("{repo}:{name}"),
                    json!({"id": id, "node_id": node_id("LA", &format!("{repo}:{name}")), "name": name, "color": "ededed", "description": null, "default": false}),
                );
                out.push(name.to_owned());
            }
        }
        out.dedup();
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|l| seen.insert(l.to_ascii_lowercase()));
    out
}

fn validate_assignees(ctx: &Ctx, repo: &str, logins: &[String]) -> Result<Vec<String>, Response> {
    let mut out = Vec::new();
    for l in logins {
        if !is_assignable(ctx.svc, repo, l) {
            return Err(status_json(
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({
                    "message": "Validation Failed",
                    "errors": [{"value": l, "resource": "Issue", "field": "assignees", "code": "invalid"}],
                    "documentation_url": crate::util::DOCS,
                    "status": "422",
                }),
            ));
        }
        let c = canonical_login(ctx.svc, l);
        if !out.contains(&c) {
            out.push(c);
        }
    }
    Ok(out)
}

fn validate_type(ctx: &Ctx, repo: &str, t: &str) -> Result<String, Response> {
    let owner = repo.split('/').next().unwrap_or("");
    let lower = t.to_ascii_lowercase();
    with_prefix(ctx.svc, "issue_types", &format!("{owner}:"))
        .iter()
        .find(|(_, d)| s(d, "name").to_ascii_lowercase() == lower)
        .map(|(_, d)| s(d, "name").to_owned())
        .ok_or_else(|| {
            status_json(
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({
                    "message": "Validation Failed",
                    "errors": [{"value": t, "resource": "Issue", "field": "type", "code": "invalid"}],
                    "documentation_url": crate::util::DOCS,
                    "status": "422",
                }),
            )
        })
}

fn validate_milestone(ctx: &Ctx, repo: &str, v: &Value) -> Result<Value, Response> {
    if v.is_null() {
        return Ok(Value::Null);
    }
    let n = as_i64(v).unwrap_or(0);
    if get(ctx.svc, "milestones", &format!("{repo}:{n}")).is_none() {
        return Err(status_json(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({
                "message": "Validation Failed",
                "errors": [{"value": n, "resource": "Issue", "field": "milestone", "code": "invalid"}],
                "documentation_url": crate::util::DOCS,
                "status": "422",
            }),
        ));
    }
    Ok(json!(n))
}

/// Issue field definitions visible to `repo` (org-level fields).
pub fn issue_fields(svc: &ServiceState, owner: &str) -> Vec<Value> {
    with_prefix(svc, "issue_fields", &format!("{owner}:"))
        .into_iter()
        .map(|(_, f)| f.clone())
        .collect()
}

fn normalize_field_values(ctx: &Ctx, repo: &str, input: &Value) -> Result<Vec<Value>, Response> {
    let owner = repo.split('/').next().unwrap_or("");
    let fields = issue_fields(ctx.svc, owner);
    let mut out = Vec::new();
    for fv in input.as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let fid = u(fv, "field_id");
        let Some(field) = fields.iter().find(|f| u(f, "id") == fid) else {
            return Err(validation("IssueFieldValue", "field_id", "invalid"));
        };
        let value = fv.get("value").cloned().unwrap_or(Value::Null);
        let value = match s(field, "data_type").to_ascii_uppercase().as_str() {
            "SINGLE_SELECT" => {
                let name = value.as_str().unwrap_or("").to_owned();
                match arr(field, "options")
                    .iter()
                    .find(|o| s(o, "name").eq_ignore_ascii_case(&name))
                {
                    Some(o) => json!(s(o, "name")),
                    None => return Err(validation("IssueFieldValue", "value", "invalid")),
                }
            }
            "NUMBER" => match value
                .as_f64()
                .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
            {
                Some(n) => json!(n),
                None => return Err(validation("IssueFieldValue", "value", "invalid")),
            },
            "DATE" => {
                let d = value.as_str().unwrap_or("");
                if unix(d).is_none() {
                    return Err(validation("IssueFieldValue", "value", "invalid"));
                }
                json!(d)
            }
            _ => match &value {
                Value::String(_) => value.clone(),
                Value::Null => return Err(validation("IssueFieldValue", "value", "invalid")),
                other => json!(other.to_string()),
            },
        };
        out.push(json!({"field_id": fid, "value": value}));
    }
    Ok(out)
}

// ---------------------------------------------------------------- issues

fn list_issues(ctx: &Ctx, repo: &str) -> Response {
    let state = ctx.req.q("state").unwrap_or("open");
    let labels: Vec<String> = ctx
        .req
        .q("labels")
        .map(|l| {
            l.split(',')
                .map(|x| x.trim().to_ascii_lowercase())
                .filter(|x| !x.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let assignee = ctx.req.q("assignee");
    let creator = ctx.req.q("creator");
    let mentioned = ctx.req.q("mentioned");
    let milestone = ctx.req.q("milestone");
    let since = ctx.req.q("since").and_then(unix);
    let issue_type = ctx.req.q("type");
    let prefix = format!("{repo}#");
    let mut items: Vec<(String, &Value)> = ["issues", "pulls"]
        .iter()
        .flat_map(|c| with_prefix(ctx.svc, c, &prefix))
        .map(|(k, v)| (k.to_owned(), v))
        .filter(|(_, i)| state == "all" || s(i, "state") == state)
        .filter(|(_, i)| {
            let have: Vec<String> = names_of(arr(i, "labels"))
                .iter()
                .map(|l| l.to_ascii_lowercase())
                .collect();
            labels.iter().all(|l| have.contains(l))
        })
        .filter(|(_, i)| {
            let a = names_of(arr(i, "assignees"));
            match assignee {
                None => true,
                Some("none") => a.is_empty(),
                Some("*") => !a.is_empty(),
                Some(l) => a.iter().any(|x| x.eq_ignore_ascii_case(l)),
            }
        })
        .filter(|(_, i)| {
            creator.is_none_or(|c| {
                login_of(i.get("user").unwrap_or(&Value::Null)).eq_ignore_ascii_case(c)
            })
        })
        .filter(|(_, i)| mentioned.is_none_or(|mn| s(i, "body").contains(&format!("@{mn}"))))
        .filter(|(_, i)| match milestone {
            None => true,
            Some("none") => milestone_number(i).is_none(),
            Some("*") => milestone_number(i).is_some(),
            Some(n) => milestone_number(i).map(|x| x.to_string()) == Some(n.to_owned()),
        })
        .filter(|(_, i)| since.is_none_or(|x| unix(s(i, "updated_at")).unwrap_or(0) >= x))
        .filter(|(_, i)| issue_type.is_none_or(|t| s(i, "type").eq_ignore_ascii_case(t)))
        .collect();
    let sort = ctx.req.q("sort").unwrap_or("created");
    let asc = ctx.req.q("direction") == Some("asc");
    items.sort_by_key(|(k, i)| match sort {
        "updated" => unix(s(i, "updated_at")).unwrap_or(0),
        "comments" => {
            let (r, n) = split_key(k);
            render::comment_count(ctx.svc, r, n) as i64
        }
        _ => unix(s(i, "created_at")).unwrap_or(0) * 1000 + u(i, "number") as i64 % 1000,
    });
    if !asc {
        items.reverse();
    }
    let out: Vec<Value> = items
        .iter()
        .map(|(k, _)| issue_json(ctx.svc, ctx.base(), k))
        .collect();
    paged(ctx.req, out)
}

fn get_issue(ctx: &Ctx, repo: &str, n: &str) -> Response {
    let v = issue_json(ctx.svc, ctx.base(), &format!("{repo}#{n}"));
    if v.is_null() { not_found() } else { ok(v) }
}

fn create_issue(ctx: &mut Ctx, repo: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let title = match body.get("title") {
        Some(Value::String(t)) if !t.trim().is_empty() => t.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(_)) => return validation("Issue", "title", "missing_field"),
        _ => {
            return status_json(
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({"message": "Invalid request.\n\n\"title\" wasn't supplied.", "documentation_url": crate::util::DOCS, "status": "422"}),
            );
        }
    };
    let mut assignees = str_list(body.get("assignees").unwrap_or(&Value::Null));
    if let Some(a) = opt_s(&body, "assignee") {
        assignees.push(a);
    }
    let assignees = match validate_assignees(ctx, repo, &assignees) {
        Ok(a) => a,
        Err(e) => return e,
    };
    let milestone =
        match validate_milestone(ctx, repo, body.get("milestone").unwrap_or(&Value::Null)) {
            Ok(m) => m,
            Err(e) => return e,
        };
    let issue_type = match opt_s(&body, "type") {
        Some(t) => match validate_type(ctx, repo, &t) {
            Ok(t) => json!(t),
            Err(e) => return e,
        },
        None => Value::Null,
    };
    let field_values = match body.get("issue_field_values") {
        Some(v) => match normalize_field_values(ctx, repo, v) {
            Ok(v) => v,
            Err(e) => return e,
        },
        None => Vec::new(),
    };
    let label_names = str_list(body.get("labels").unwrap_or(&Value::Null));
    let labels = ensure_labels(ctx, repo, &label_names);
    let number = next_number(ctx.svc, repo);
    let id = next_issue_db_id(ctx.svc);
    let key = format!("{repo}#{number}");
    let node = ctx.svc.ids.next_prefixed("issue_node", "I_kwDO", 16);
    let mut doc = json!({
        "id": id,
        "node_id": node,
        "number": number,
        "title": title,
        "body": body.get("body").cloned().unwrap_or(Value::Null),
        "user": ctx.viewer(),
        "labels": labels,
        "assignees": assignees,
        "milestone": milestone,
        "state": "open",
        "state_reason": null,
        "locked": false,
        "type": issue_type,
        "created_at": ctx.now,
        "updated_at": ctx.now,
        "closed_at": null,
    });
    if !field_values.is_empty() {
        doc["field_values"] = json!(field_values);
    }
    ctx.svc
        .store
        .collection_mut("issues")
        .insert(key.clone(), doc);
    let v = issue_json(ctx.svc, ctx.base(), &key);
    let url = s(&v, "url").to_owned();
    with_header(created(v), header::LOCATION, &url)
}

fn update_issue(ctx: &mut Ctx, repo: &str, n: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some((coll, key)) = locate(ctx.svc, repo, n) else {
        return not_found();
    };
    let mut doc = get(ctx.svc, coll, &key).cloned().unwrap_or_default();
    if let Some(t) = body.get("title") {
        match t.as_str() {
            Some(t) if !t.trim().is_empty() => doc["title"] = json!(t),
            _ => return validation("Issue", "title", "missing_field"),
        }
    }
    if let Some(bd) = body.get("body") {
        doc["body"] = bd.clone();
    }
    if body.get("assignees").is_some() || body.get("assignee").is_some() {
        let mut a = str_list(body.get("assignees").unwrap_or(&Value::Null));
        if let Some(x) = opt_s(&body, "assignee") {
            a.push(x);
        }
        match validate_assignees(ctx, repo, &a) {
            Ok(a) => doc["assignees"] = json!(a),
            Err(e) => return e,
        }
    }
    if let Some(mv) = body.get("milestone") {
        match validate_milestone(ctx, repo, mv) {
            Ok(m) => doc["milestone"] = m,
            Err(e) => return e,
        }
    }
    if let Some(tv) = body.get("type") {
        if tv.is_null() {
            doc["type"] = Value::Null;
        } else if let Some(t) = tv.as_str() {
            match validate_type(ctx, repo, t) {
                Ok(t) => doc["type"] = json!(t),
                Err(e) => return e,
            }
        }
    }
    if let Some(fv) = body.get("issue_field_values") {
        match normalize_field_values(ctx, repo, fv) {
            Ok(v) => doc["field_values"] = json!(v),
            Err(e) => return e,
        }
    }
    if let Some(lv) = body.get("labels") {
        let names = names_of(lv.as_array().map(Vec::as_slice).unwrap_or(&[]));
        doc["labels"] = json!(ensure_labels(ctx, repo, &names));
    }
    if let Some(state) = opt_s(&body, "state") {
        match state.as_str() {
            "closed" if s(&doc, "state") != "closed" => {
                doc["state"] = json!("closed");
                doc["closed_at"] = json!(ctx.now);
                doc["closed_by"] = json!(ctx.viewer());
                doc["state_reason"] =
                    json!(opt_s(&body, "state_reason").unwrap_or_else(|| "completed".into()));
            }
            "open" if s(&doc, "state") != "open" => {
                doc["state"] = json!("open");
                doc["closed_at"] = Value::Null;
                doc["closed_by"] = Value::Null;
                doc["state_reason"] = json!("reopened");
            }
            "open" | "closed" => {
                if let Some(r) = opt_s(&body, "state_reason")
                    && state == "closed"
                {
                    doc["state_reason"] = json!(r);
                }
            }
            _ => return validation("Issue", "state", "invalid"),
        }
    }
    doc["updated_at"] = json!(ctx.now);
    ctx.svc.store.collection_mut(coll).insert(key.clone(), doc);
    ok(issue_json(ctx.svc, ctx.base(), &key))
}

// ---------------------------------------------------------------- comments

fn list_comments(ctx: &Ctx, repo: &str, n: &str) -> Response {
    if locate(ctx.svc, repo, n).is_none() {
        return not_found();
    }
    let number: u64 = n.parse().unwrap_or(0);
    let since = ctx.req.q("since").and_then(unix);
    let mut comments: Vec<&Value> = all(ctx.svc, "issue_comments")
        .into_iter()
        .map(|(_, c)| c)
        .filter(|c| s(c, "repo") == repo && u(c, "issue_number") == number)
        .filter(|c| {
            since.is_none_or(|x| {
                unix(s(c, "updated_at"))
                    .or_else(|| unix(s(c, "created_at")))
                    .unwrap_or(0)
                    >= x
            })
        })
        .collect();
    comments.sort_by_key(|c| (unix(s(c, "created_at")).unwrap_or(0), u(c, "id")));
    let out: Vec<Value> = comments
        .iter()
        .map(|c| issue_comment_json(ctx.svc, ctx.base(), c))
        .collect();
    paged(ctx.req, out)
}

fn create_comment(ctx: &mut Ctx, repo: &str, n: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(text) = opt_s(&body, "body") else {
        return validation("IssueComment", "body", "missing_field");
    };
    let Some((coll, key)) = locate(ctx.svc, repo, n) else {
        return not_found();
    };
    if get(ctx.svc, coll, &key).is_some_and(|d| b(d, "locked")) {
        return error(
            StatusCode::FORBIDDEN,
            "Unable to create comment because issue is locked.",
        );
    }
    let id = next_id(ctx.svc, "issue_comments", "id", 1);
    let doc = json!({
        "id": id,
        "repo": repo,
        "issue_number": n.parse::<u64>().unwrap_or(0),
        "body": text,
        "user": ctx.viewer(),
        "created_at": ctx.now,
        "updated_at": ctx.now,
    });
    ctx.svc
        .store
        .collection_mut("issue_comments")
        .insert(id.to_string(), doc.clone());
    let now = ctx.now.clone();
    if let Some(d) = ctx.svc.store.collection_mut(coll).get_mut(&key) {
        d["updated_at"] = json!(now);
    }
    let v = issue_comment_json(ctx.svc, ctx.base(), &doc);
    let url = s(&v, "url").to_owned();
    with_header(created(v), header::LOCATION, &url)
}

fn comment(ctx: &mut Ctx, repo: &str, id: &str, m: &Method) -> Response {
    let Some(doc) = get(ctx.svc, "issue_comments", id)
        .filter(|c| s(c, "repo") == repo)
        .cloned()
    else {
        return not_found();
    };
    match *m {
        Method::GET => ok(issue_comment_json(ctx.svc, ctx.base(), &doc)),
        Method::PATCH => {
            let body = match ctx.req.json() {
                Ok(b) => b,
                Err(e) => return e,
            };
            let Some(text) = opt_s(&body, "body") else {
                return validation("IssueComment", "body", "missing_field");
            };
            let mut doc = doc;
            doc["body"] = json!(text);
            doc["updated_at"] = json!(ctx.now);
            ctx.svc
                .store
                .collection_mut("issue_comments")
                .insert(id, doc.clone());
            ok(issue_comment_json(ctx.svc, ctx.base(), &doc))
        }
        Method::DELETE => {
            ctx.svc.store.collection_mut("issue_comments").remove(id);
            no_content()
        }
        _ => not_found(),
    }
}

// ---------------------------------------------------------------- issue labels / assignees

fn issue_labels(ctx: &mut Ctx, repo: &str, n: &str, name: Option<&str>, m: &Method) -> Response {
    let Some((coll, key)) = locate(ctx.svc, repo, n) else {
        return not_found();
    };
    let mut current = names_of(arr(
        get(ctx.svc, coll, &key).unwrap_or(&Value::Null),
        "labels",
    ));
    match (m, name) {
        (&Method::GET, None) => {}
        (&Method::POST, None) | (&Method::PUT, None) => {
            let body = match ctx.req.json() {
                Ok(b) => b,
                Err(e) => return e,
            };
            let list = body.get("labels").cloned().unwrap_or(body);
            let names = names_of(list.as_array().map(Vec::as_slice).unwrap_or(&[]));
            let names = ensure_labels(ctx, repo, &names);
            if *m == Method::PUT {
                current = names;
            } else {
                for l in names {
                    if !current.iter().any(|c| c.eq_ignore_ascii_case(&l)) {
                        current.push(l);
                    }
                }
            }
        }
        (&Method::DELETE, None) => current.clear(),
        (&Method::DELETE, Some(label)) => {
            let before = current.len();
            current.retain(|c| !c.eq_ignore_ascii_case(label));
            if before == current.len() {
                return error(StatusCode::NOT_FOUND, "Label does not exist");
            }
        }
        _ => return not_found(),
    }
    if *m != Method::GET {
        let now = ctx.now.clone();
        if let Some(d) = ctx.svc.store.collection_mut(coll).get_mut(&key) {
            d["labels"] = json!(current);
            d["updated_at"] = json!(now);
        }
    }
    let out: Vec<Value> = current
        .iter()
        .map(|l| label_json(ctx.svc, ctx.base(), repo, l))
        .collect();
    if *m == Method::GET {
        paged(ctx.req, out)
    } else {
        ok(Value::Array(out))
    }
}

fn issue_assignees(ctx: &mut Ctx, repo: &str, n: &str, m: &Method) -> Response {
    let Some((coll, key)) = locate(ctx.svc, repo, n) else {
        return not_found();
    };
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let requested = str_list(body.get("assignees").unwrap_or(&Value::Null));
    let mut current = names_of(arr(
        get(ctx.svc, coll, &key).unwrap_or(&Value::Null),
        "assignees",
    ));
    match *m {
        Method::POST => {
            for l in requested {
                if is_assignable(ctx.svc, repo, &l) {
                    let c = canonical_login(ctx.svc, &l);
                    if !current.contains(&c) && current.len() < 10 {
                        current.push(c);
                    }
                }
            }
        }
        Method::DELETE => current.retain(|c| !requested.iter().any(|r| r.eq_ignore_ascii_case(c))),
        _ => return not_found(),
    }
    let now = ctx.now.clone();
    if let Some(d) = ctx.svc.store.collection_mut(coll).get_mut(&key) {
        d["assignees"] = json!(current);
        d["updated_at"] = json!(now);
    }
    let v = issue_json(ctx.svc, ctx.base(), &key);
    if *m == Method::POST {
        created(v)
    } else {
        ok(v)
    }
}

// ---------------------------------------------------------------- reactions

pub enum Target<'a> {
    Issue(&'a str),
    IssueComment(&'a str),
    PullComment(&'a str),
}

const REACTIONS: [&str; 8] = [
    "+1", "-1", "laugh", "confused", "heart", "hooray", "rocket", "eyes",
];

fn reaction_json(ctx: &Ctx, r: &Value) -> Value {
    json!({
        "id": u(r, "id"),
        "node_id": node_id("REA", &u(r, "id").to_string()),
        "user": user(ctx.svc, ctx.base(), s(r, "user")),
        "content": s(r, "content"),
        "created_at": r.get("created_at").cloned().unwrap_or(Value::Null),
    })
}

pub fn reactions(ctx: &mut Ctx, repo: &str, subject: &str, target: Target, m: &Method) -> Response {
    let exists = match target {
        Target::Issue(n) => locate(ctx.svc, repo, n).is_some(),
        Target::IssueComment(id) => {
            get(ctx.svc, "issue_comments", id).is_some_and(|c| s(c, "repo") == repo)
        }
        Target::PullComment(id) => {
            get(ctx.svc, "review_comments", id).is_some_and(|c| s(c, "repo") == repo)
        }
    };
    if !exists {
        return not_found();
    }
    match *m {
        Method::GET => {
            let content = ctx.req.q("content");
            let out: Vec<Value> = all(ctx.svc, "reactions")
                .iter()
                .filter(|(_, r)| s(r, "subject") == subject)
                .filter(|(_, r)| content.is_none_or(|c| s(r, "content") == c))
                .map(|(_, r)| reaction_json(ctx, r))
                .collect();
            paged(ctx.req, out)
        }
        Method::POST => {
            let body = match ctx.req.json() {
                Ok(b) => b,
                Err(e) => return e,
            };
            let content = s(&body, "content").to_owned();
            if !REACTIONS.contains(&content.as_str()) {
                return validation("Reaction", "content", "invalid");
            }
            let viewer = ctx.viewer();
            if let Some((_, r)) = all(ctx.svc, "reactions").into_iter().find(|(_, r)| {
                s(r, "subject") == subject && s(r, "content") == content && s(r, "user") == viewer
            }) {
                return ok(reaction_json(ctx, r));
            }
            let id = next_id(ctx.svc, "reactions", "id", 1);
            let doc = json!({"id": id, "subject": subject, "content": content, "user": viewer, "created_at": ctx.now});
            ctx.svc
                .store
                .collection_mut("reactions")
                .insert(id.to_string(), doc.clone());
            created(reaction_json(ctx, &doc))
        }
        _ => not_found(),
    }
}

pub fn delete_reaction(ctx: &mut Ctx, subject: &str, rid: &str) -> Response {
    match get(ctx.svc, "reactions", rid) {
        Some(r) if s(r, "subject") == subject => {
            ctx.svc.store.collection_mut("reactions").remove(rid);
            no_content()
        }
        _ => not_found(),
    }
}

// ---------------------------------------------------------------- labels

fn create_label(ctx: &mut Ctx, repo: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(name) = opt_s(&body, "name") else {
        return validation("Label", "name", "missing_field");
    };
    if label_doc(ctx.svc, repo, &name).is_some() {
        return validation("Label", "name", "already_exists");
    }
    let color = opt_s(&body, "color").unwrap_or_else(|| "ededed".into());
    let color = color.trim_start_matches('#').to_ascii_lowercase();
    if color.len() != 6 || !color.chars().all(|c| c.is_ascii_hexdigit()) {
        return validation("Label", "color", "invalid");
    }
    let id = next_id(ctx.svc, "labels", "id", 1);
    let key = format!("{repo}:{name}");
    ctx.svc.store.collection_mut("labels").insert(
        key.clone(),
        json!({"id": id, "node_id": node_id("LA", &key), "name": name, "color": color, "description": body.get("description").cloned().unwrap_or(Value::Null), "default": false}),
    );
    created(label_json(ctx.svc, ctx.base(), repo, &name))
}

pub fn rename_label_everywhere(svc: &mut ServiceState, repo: &str, old: &str, new: Option<&str>) {
    let prefix = format!("{repo}#");
    for coll in ["issues", "pulls"] {
        let keys: Vec<String> = with_prefix(svc, coll, &prefix)
            .iter()
            .map(|(k, _)| (*k).to_owned())
            .collect();
        for k in keys {
            if let Some(d) = svc.store.collection_mut(coll).get_mut(&k) {
                let mut names = names_of(arr(d, "labels"));
                let had = names.iter().any(|n| n.eq_ignore_ascii_case(old));
                if !had {
                    continue;
                }
                names.retain(|n| !n.eq_ignore_ascii_case(old));
                if let Some(n) = new {
                    names.push(n.to_owned());
                }
                d["labels"] = json!(names);
            }
        }
    }
}

fn update_label(ctx: &mut Ctx, repo: &str, name: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some((key, doc)) = label_doc(ctx.svc, repo, name).map(|(k, d)| (k.to_owned(), d.clone()))
    else {
        return not_found();
    };
    let mut doc = doc;
    let old_name = s(&doc, "name").to_owned();
    if let Some(c) = opt_s(&body, "color") {
        doc["color"] = json!(c.trim_start_matches('#').to_ascii_lowercase());
    }
    if let Some(d) = body.get("description") {
        doc["description"] = d.clone();
    }
    let new_name = opt_s(&body, "new_name");
    ctx.svc.store.collection_mut("labels").remove(&key);
    let final_name = new_name.clone().unwrap_or_else(|| old_name.clone());
    doc["name"] = json!(final_name);
    ctx.svc
        .store
        .collection_mut("labels")
        .insert(format!("{repo}:{final_name}"), doc);
    if let Some(n) = &new_name {
        rename_label_everywhere(ctx.svc, repo, &old_name, Some(n));
    }
    ok(label_json(ctx.svc, ctx.base(), repo, &final_name))
}

fn delete_label(ctx: &mut Ctx, repo: &str, name: &str) -> Response {
    let Some(key) = label_doc(ctx.svc, repo, name).map(|(k, _)| k.to_owned()) else {
        return not_found();
    };
    ctx.svc.store.collection_mut("labels").remove(&key);
    rename_label_everywhere(ctx.svc, repo, name, None);
    no_content()
}

// ---------------------------------------------------------------- milestones

fn list_milestones(ctx: &Ctx, repo: &str) -> Response {
    let state = ctx.req.q("state").unwrap_or("open");
    let mut ms: Vec<u64> = with_prefix(ctx.svc, "milestones", &format!("{repo}:"))
        .iter()
        .filter(|(_, m)| {
            state == "all" || opt_s(m, "state").unwrap_or_else(|| "open".into()) == state
        })
        .map(|(_, m)| u(m, "number"))
        .collect();
    ms.sort_unstable();
    let out: Vec<Value> = ms
        .iter()
        .map(|n| milestone_json(ctx.svc, ctx.base(), repo, *n))
        .collect();
    paged(ctx.req, out)
}

fn create_milestone(ctx: &mut Ctx, repo: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(title) = opt_s(&body, "title") else {
        return validation("Milestone", "title", "missing_field");
    };
    let number = with_prefix(ctx.svc, "milestones", &format!("{repo}:"))
        .iter()
        .map(|(_, m)| u(m, "number"))
        .max()
        .unwrap_or(0)
        + 1;
    let id = next_id(ctx.svc, "milestones", "id", 1);
    let creator = ctx.viewer();
    ctx.svc.store.collection_mut("milestones").insert(
        format!("{repo}:{number}"),
        json!({"id": id, "number": number, "title": title, "description": body.get("description").cloned().unwrap_or(Value::Null), "state": opt_s(&body, "state").unwrap_or_else(|| "open".into()), "due_on": body.get("due_on").cloned().unwrap_or(Value::Null), "creator": creator, "created_at": ctx.now, "updated_at": ctx.now}),
    );
    created(milestone_json(ctx.svc, ctx.base(), repo, number))
}

// ---------------------------------------------------------------- sub-issues

fn issue_key_by_id(svc: &ServiceState, id: u64) -> Option<String> {
    all(svc, "issues")
        .iter()
        .find(|(_, i)| u(i, "id") == id)
        .map(|(k, _)| (*k).to_owned())
}

pub fn parent_key(svc: &ServiceState, child: &str) -> Option<String> {
    if let Some(p) = get(svc, "issues", child).and_then(|d| opt_s(d, "parent")) {
        return Some(p);
    }
    all(svc, "issues")
        .iter()
        .find(|(_, i)| sub_issue_keys(i).iter().any(|k| k == child))
        .map(|(k, _)| (*k).to_owned())
}

fn list_sub_issues(ctx: &Ctx, repo: &str, n: &str) -> Response {
    let key = format!("{repo}#{n}");
    let Some(parent) = get(ctx.svc, "issues", &key) else {
        return not_found();
    };
    let out: Vec<Value> = sub_issue_keys(parent)
        .iter()
        .map(|k| issue_json(ctx.svc, ctx.base(), k))
        .filter(|v| !v.is_null())
        .collect();
    paged(ctx.req, out)
}

fn add_sub_issue(ctx: &mut Ctx, repo: &str, n: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let key = format!("{repo}#{n}");
    if get(ctx.svc, "issues", &key).is_none() {
        return not_found();
    }
    let sub_id = u(&body, "sub_issue_id");
    let Some(child) = issue_key_by_id(ctx.svc, sub_id) else {
        return validation_msg("SubIssue", "The provided sub-issue does not exist");
    };
    if child == key {
        return validation_msg("SubIssue", "An issue cannot be a sub-issue of itself");
    }
    if let Some(existing) = parent_key(ctx.svc, &child) {
        if existing == key {
            return validation_msg("SubIssue", "Issue may not contain duplicate sub-issues");
        }
        if !b(&body, "replace_parent") {
            return validation_msg("SubIssue", "Issue may not have more than one parent");
        }
        detach(ctx.svc, &existing, &child);
    }
    if let Some(p) = ctx.svc.store.collection_mut("issues").get_mut(&key) {
        let mut subs = names_of(arr(p, "sub_issues"));
        if subs.len() >= 100 {
            return validation_msg("SubIssue", "Parent cannot have more than 100 sub-issues");
        }
        subs.push(child.clone());
        p["sub_issues"] = json!(subs);
    }
    let now = ctx.now.clone();
    if let Some(c) = ctx.svc.store.collection_mut("issues").get_mut(&child) {
        c["parent"] = json!(key);
        c["updated_at"] = json!(now);
    }
    let url = format!("{}/repos/{repo}/issues/{n}/sub_issues", ctx.req.api());
    with_header(
        created(issue_json(ctx.svc, ctx.base(), &child)),
        header::LOCATION,
        &url,
    )
}

fn detach(svc: &mut ServiceState, parent: &str, child: &str) {
    if let Some(p) = svc.store.collection_mut("issues").get_mut(parent) {
        let mut subs = names_of(arr(p, "sub_issues"));
        subs.retain(|k| k != child);
        p["sub_issues"] = json!(subs);
    }
    if let Some(c) = svc.store.collection_mut("issues").get_mut(child) {
        c["parent"] = Value::Null;
    }
}

fn remove_sub_issue(ctx: &mut Ctx, repo: &str, n: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let key = format!("{repo}#{n}");
    if get(ctx.svc, "issues", &key).is_none() {
        return not_found();
    }
    let Some(child) = issue_key_by_id(ctx.svc, u(&body, "sub_issue_id")) else {
        return not_found();
    };
    if parent_key(ctx.svc, &child).as_deref() != Some(key.as_str()) {
        return not_found();
    }
    detach(ctx.svc, &key, &child);
    ok(issue_json(ctx.svc, ctx.base(), &child))
}

fn reprioritize(ctx: &mut Ctx, repo: &str, n: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let key = format!("{repo}#{n}");
    let Some(parent) = get(ctx.svc, "issues", &key).cloned() else {
        return not_found();
    };
    let Some(child) = issue_key_by_id(ctx.svc, u(&body, "sub_issue_id")) else {
        return not_found();
    };
    let mut subs = sub_issue_keys(&parent);
    if !subs.contains(&child) {
        return validation_msg(
            "SubIssue",
            "The provided sub-issue is not a sub-issue of this issue",
        );
    }
    let anchor_id = if u(&body, "after_id") != 0 {
        u(&body, "after_id")
    } else {
        u(&body, "before_id")
    };
    let after = u(&body, "after_id") != 0;
    let Some(anchor) = issue_key_by_id(ctx.svc, anchor_id).filter(|a| subs.contains(a)) else {
        return validation_msg(
            "SubIssue",
            "The anchor sub-issue is not a sub-issue of this issue",
        );
    };
    subs.retain(|k| *k != child);
    let pos = subs.iter().position(|k| *k == anchor).unwrap_or(0);
    subs.insert(if after { pos + 1 } else { pos }, child.clone());
    if let Some(p) = ctx.svc.store.collection_mut("issues").get_mut(&key) {
        p["sub_issues"] = json!(subs);
    }
    ok(issue_json(ctx.svc, ctx.base(), &child))
}

fn parent_issue(ctx: &Ctx, repo: &str, n: &str) -> Response {
    let key = format!("{repo}#{n}");
    if get(ctx.svc, "issues", &key).is_none() {
        return not_found();
    }
    match parent_key(ctx.svc, &key) {
        Some(p) => ok(issue_json(ctx.svc, ctx.base(), &p)),
        None => not_found(),
    }
}

// ---------------------------------------------------------------- issue field values

fn field_values(ctx: &Ctx, repo: &str, n: &str) -> Response {
    let key = format!("{repo}#{n}");
    let Some(i) = get(ctx.svc, "issues", &key) else {
        return not_found();
    };
    let owner = repo.split('/').next().unwrap_or("");
    let fields = issue_fields(ctx.svc, owner);
    let out: Vec<Value> = arr(i, "field_values")
        .iter()
        .map(|fv| {
            let f = fields.iter().find(|f| u(f, "id") == u(fv, "field_id"));
            json!({
                "issue_field_id": u(fv, "field_id"),
                "node_id": node_id("IFV", &format!("{key}:{}", u(fv, "field_id"))),
                "data_type": f.map(|f| s(f, "data_type").to_ascii_lowercase()).unwrap_or_default(),
                "value": fv.get("value").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();
    paged(ctx.req, out)
}

fn delete_field_value(ctx: &mut Ctx, repo: &str, n: &str, field_id: &str) -> Response {
    let key = format!("{repo}#{n}");
    let Some(mut i) = get(ctx.svc, "issues", &key).cloned() else {
        return not_found();
    };
    let mut values: Vec<Value> = arr(&i, "field_values").to_vec();
    let before = values.len();
    values.retain(|fv| u(fv, "field_id").to_string() != field_id);
    if before == values.len() {
        return not_found();
    }
    i["field_values"] = json!(values);
    i["updated_at"] = json!(ctx.now);
    ctx.svc.store.collection_mut("issues").insert(key, i);
    no_content()
}

#[allow(dead_code)]
fn unknown(_: &str) -> Response {
    unprocessable("unknown")
}
