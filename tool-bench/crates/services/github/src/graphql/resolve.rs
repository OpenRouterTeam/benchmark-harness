//! Field resolvers for the subset of GitHub's GraphQL schema the MCP server queries.

use mock_core::world::ServiceState;
use serde_json::{Map, Value, json};

use super::{
    exec::{FieldError, FieldResult, Resolver},
    objects as o,
};
use crate::{
    issues::{assignable, issue_fields, parent_key},
    projects,
    render::{self, account, all, get, is_org, repo_key, split_key, sub_issue_keys, with_prefix},
    util::{arr, b, b64_decode, b64_encode, login_of, names_of, opt_s, s, u, unix},
};

pub struct Gh<'a> {
    pub svc: &'a mut ServiceState,
    pub base: String,
    pub now: String,
    pub viewer: String,
}

fn arg<'a>(args: &'a Map<String, Value>, k: &str) -> &'a Value {
    args.get(k).unwrap_or(&Value::Null)
}

fn arg_s(args: &Map<String, Value>, k: &str) -> String {
    match arg(args, k) {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn arg_u(args: &Map<String, Value>, k: &str) -> Option<u64> {
    let v = arg(args, k);
    v.as_u64()
        .or_else(|| v.as_f64().map(|f| f as u64))
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

fn internal<'a>(parent: &'a Value, k: &str) -> &'a str {
    parent.get(k).and_then(Value::as_str).unwrap_or("")
}

pub fn cursor(i: usize) -> String {
    b64_encode(format!("cursor:{i}").as_bytes())
}

fn decode_cursor(c: &str) -> Option<usize> {
    let raw = b64_decode(c)?;
    let text = String::from_utf8(raw).ok()?;
    text.strip_prefix("cursor:")?.parse().ok()
}

/// Relay connection over `nodes` (already ordered), honouring first/after/last/before.
pub fn connection(nodes: Vec<Value>, args: &Map<String, Value>) -> FieldResult {
    let total = nodes.len();
    let mut start = 0usize;
    let mut end = total;
    if let Some(after) = arg(args, "after").as_str() {
        match decode_cursor(after) {
            Some(i) => start = i.min(total),
            None => {
                return Err(FieldError::invalid(format!(
                    "`{after}` does not appear to be a valid cursor."
                )));
            }
        }
    }
    if let Some(before) = arg(args, "before").as_str() {
        match decode_cursor(before) {
            Some(i) => end = i.saturating_sub(1).min(total),
            None => {
                return Err(FieldError::invalid(format!(
                    "`{before}` does not appear to be a valid cursor."
                )));
            }
        }
    }
    let end = end.max(start);
    let (mut lo, mut hi) = (start, end);
    if let Some(first) = arg_u(args, "first") {
        if first > 100 {
            return Err(FieldError::invalid(format!(
                "Requesting {first} records on the connection exceeds the `first` limit of 100 records."
            )));
        }
        hi = (lo + first as usize).min(hi);
    }
    if let Some(last) = arg_u(args, "last") {
        lo = hi.saturating_sub(last as usize).max(lo);
    }
    let page: Vec<Value> = nodes[lo..hi].to_vec();
    let edges: Vec<Value> = page
        .iter()
        .enumerate()
        .map(|(i, n)| json!({"cursor": cursor(lo + i + 1), "node": n}))
        .collect();
    Ok(json!({
        "__typename": "Connection",
        "nodes": page,
        "edges": edges,
        "totalCount": total,
        "pageInfo": {
            "__typename": "PageInfo",
            "hasNextPage": hi < total,
            "hasPreviousPage": lo > 0,
            "startCursor": if hi > lo { json!(cursor(lo + 1)) } else { Value::Null },
            "endCursor": if hi > lo { json!(cursor(hi)) } else { Value::Null },
        },
    }))
}

fn order<T>(
    items: &mut [T],
    args: &Map<String, Value>,
    key: impl Fn(&T, &str) -> i64,
    default_field: &str,
    default_desc: bool,
) {
    let ob = arg(args, "orderBy");
    let field = ob
        .get("field")
        .and_then(Value::as_str)
        .unwrap_or(default_field)
        .to_owned();
    let desc = ob
        .get("direction")
        .and_then(Value::as_str)
        .map_or(default_desc, |d| d == "DESC");
    items.sort_by_key(|i| key(i, &field));
    if desc {
        items.reverse();
    }
}

impl Gh<'_> {
    fn svc(&self) -> &ServiceState {
        self.svc
    }

    fn repo_or_err(&self, owner: &str, name: &str) -> FieldResult {
        match repo_key(self.svc(), owner, name) {
            Some(k) => Ok(o::repo(self.svc(), &self.base, &k)),
            None => Err(FieldError::not_found(format!(
                "Could not resolve to a Repository with the name '{owner}/{name}'."
            ))),
        }
    }

    fn account_obj(&self, login: &str, want_org: Option<bool>) -> FieldResult {
        let exists = account(self.svc(), login).is_some();
        let org = is_org(self.svc(), login);
        match (exists, want_org) {
            (true, Some(true)) if org => Ok(o::actor(self.svc(), &self.base, login)),
            (true, Some(false)) if !org => Ok(o::actor(self.svc(), &self.base, login)),
            (true, None) => Ok(o::actor(self.svc(), &self.base, login)),
            (_, Some(true)) => Err(FieldError::not_found(format!(
                "Could not resolve to an Organization with the login of '{login}'."
            ))),
            _ => Err(FieldError::not_found(format!(
                "Could not resolve to a User with the login of '{login}'."
            ))),
        }
    }

    fn actor_of(&self, v: &Value) -> Value {
        o::actor(self.svc(), &self.base, &login_of(v))
    }

    // ------------------------------------------------------------ Repository

    fn repository_field(
        &mut self,
        parent: &Value,
        field: &str,
        args: &Map<String, Value>,
    ) -> FieldResult {
        let repo = internal(parent, "_key").to_owned();
        let svc = self.svc();
        Ok(match field {
            "owner" => o::actor(svc, &self.base, internal(parent, "_owner")),
            "issue" => {
                let n = arg_u(args, "number").unwrap_or(0);
                let key = format!("{repo}#{n}");
                let v = o::issue(svc, &self.base, &key);
                if v.is_null() {
                    return Err(FieldError::not_found(format!(
                        "Could not resolve to an Issue with the number of {n}."
                    )));
                }
                v
            }
            "pullRequest" => {
                let n = arg_u(args, "number").unwrap_or(0);
                let v = o::pull(svc, &self.base, &format!("{repo}#{n}"));
                if v.is_null() {
                    return Err(FieldError::not_found(format!(
                        "Could not resolve to a PullRequest with the number of {n}."
                    )));
                }
                v
            }
            "issueOrPullRequest" => {
                let n = arg_u(args, "number").unwrap_or(0);
                let v = o::issue_or_pull(svc, &self.base, &format!("{repo}#{n}"));
                if v.is_null() {
                    return Err(FieldError::not_found(format!(
                        "Could not resolve to an issue or pull request with the number of {n}."
                    )));
                }
                v
            }
            "issues" => return self.issues_connection(&repo, args),
            "pullRequests" => {
                let states: Vec<String> = arg(args, "states")
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                let mut items: Vec<Value> = with_prefix(svc, "pulls", &format!("{repo}#"))
                    .iter()
                    .map(|(k, _)| o::pull(svc, &self.base, k))
                    .filter(|p| {
                        states.is_empty() || states.iter().any(|st| p["state"] == st.as_str())
                    })
                    .collect();
                order(
                    &mut items,
                    args,
                    |p, f| match f {
                        "UPDATED_AT" => unix(p["updatedAt"].as_str().unwrap_or("")).unwrap_or(0),
                        _ => unix(p["createdAt"].as_str().unwrap_or("")).unwrap_or(0),
                    },
                    "CREATED_AT",
                    false,
                );
                return connection(items, args);
            }
            "labels" => {
                let mut labels: Vec<Value> = with_prefix(svc, "labels", &format!("{repo}:"))
                    .iter()
                    .map(|(_, l)| o::label(svc, &self.base, &repo, s(l, "name")))
                    .collect();
                let issue_count = |name: &str| -> i64 {
                    ["issues", "pulls"]
                        .iter()
                        .flat_map(|c| with_prefix(svc, c, &format!("{repo}#")))
                        .filter(|(_, d)| {
                            names_of(arr(d, "labels"))
                                .iter()
                                .any(|n| n.eq_ignore_ascii_case(name))
                        })
                        .count() as i64
                };
                let ob = arg(args, "orderBy");
                let field = ob
                    .get("field")
                    .and_then(Value::as_str)
                    .unwrap_or("CREATED_AT")
                    .to_owned();
                let desc = ob.get("direction").and_then(Value::as_str) == Some("DESC");
                match field.as_str() {
                    "NAME" => labels
                        .sort_by_key(|l| l["name"].as_str().unwrap_or("").to_ascii_lowercase()),
                    "ISSUE_COUNT" => labels.sort_by_key(|l| {
                        (
                            std::cmp::Reverse(issue_count(l["name"].as_str().unwrap_or(""))),
                            l["name"].as_str().unwrap_or("").to_ascii_lowercase(),
                        )
                    }),
                    _ => {}
                }
                if desc && field != "ISSUE_COUNT" {
                    labels.reverse();
                }
                if let Some(q) = arg(args, "query").as_str() {
                    let q = q.to_ascii_lowercase();
                    labels.retain(|l| {
                        l["name"]
                            .as_str()
                            .unwrap_or("")
                            .to_ascii_lowercase()
                            .contains(&q)
                    });
                }
                return connection(labels, args);
            }
            "label" => o::label(svc, &self.base, &repo, &arg_s(args, "name")),
            "milestone" => o::milestone(svc, &self.base, &repo, arg_u(args, "number").unwrap_or(0)),
            "milestones" => {
                let mut ms: Vec<(u64, Value)> = with_prefix(svc, "milestones", &format!("{repo}:"))
                    .iter()
                    .map(|(_, m)| {
                        (
                            u(m, "number"),
                            o::milestone(svc, &self.base, &repo, u(m, "number")),
                        )
                    })
                    .collect();
                ms.sort_by_key(|(n, _)| *n);
                return connection(ms.into_iter().map(|(_, m)| m).collect(), args);
            }
            "discussions" => {
                let cat = arg_s(args, "categoryId");
                let mut items: Vec<Value> = with_prefix(svc, "discussions", &format!("{repo}#"))
                    .iter()
                    .filter(|(_, d)| {
                        cat.is_empty()
                            || o::category_doc(svc, &repo, s(d, "category"))
                                .is_some_and(|(k, c)| o::category_node_id(k, c) == cat)
                    })
                    .map(|(k, _)| o::discussion(svc, &self.base, k))
                    .collect();
                order(
                    &mut items,
                    args,
                    |d, f| match f {
                        "CREATED_AT" => unix(d["createdAt"].as_str().unwrap_or("")).unwrap_or(0),
                        _ => unix(d["updatedAt"].as_str().unwrap_or("")).unwrap_or(0),
                    },
                    "UPDATED_AT",
                    true,
                );
                if !cat.is_empty()
                    && !with_prefix(svc, "discussion_categories", &format!("{repo}:"))
                        .iter()
                        .any(|(k, c)| o::category_node_id(k, c) == cat)
                {
                    return Err(FieldError::not_found(format!(
                        "Could not resolve to a node with the global id of '{cat}'"
                    )));
                }
                return connection(items, args);
            }
            "discussion" => {
                let n = arg_u(args, "number").unwrap_or(0);
                let v = o::discussion(svc, &self.base, &format!("{repo}#{n}"));
                if v.is_null() {
                    return Err(FieldError::not_found(format!(
                        "Could not resolve to a Discussion with the number of {n}."
                    )));
                }
                v
            }
            "discussionCategories" => {
                let cats: Vec<Value> =
                    with_prefix(svc, "discussion_categories", &format!("{repo}:"))
                        .iter()
                        .map(|(k, c)| o::category(k, c))
                        .collect();
                return connection(cats, args);
            }
            "issueFields" => {
                let owner = repo.split('/').next().unwrap_or("");
                let fields: Vec<Value> = issue_fields(svc, owner)
                    .iter()
                    .map(|f| o::issue_field(owner, f))
                    .collect();
                return connection(fields, args);
            }
            "issueTypes" => {
                let owner = repo.split('/').next().unwrap_or("");
                let types: Vec<Value> = crate::issues::org_issue_types(svc, owner)
                    .iter()
                    .map(|t| json!({"__typename": "IssueType", "id": t["node_id"], "name": t["name"], "description": t["description"], "color": t["color"], "isEnabled": true}))
                    .collect();
                return connection(types, args);
            }
            "suggestedActors" | "assignableUsers" => {
                let users: Vec<Value> = assignable(svc, &repo)
                    .iter()
                    .map(|l| o::actor(svc, &self.base, l))
                    .collect();
                return connection(users, args);
            }
            "collaborators" => {
                let users: Vec<Value> = get(svc, "repos", &repo)
                    .map(|r| {
                        arr(r, "collaborators")
                            .iter()
                            .map(|c| o::actor(svc, &self.base, &login_of(c)))
                            .collect()
                    })
                    .unwrap_or_default();
                return connection(users, args);
            }
            "projectV2" => {
                let n = arg_u(args, "number").unwrap_or(0);
                let owner = repo.split('/').next().unwrap_or("");
                match projects::project_key(svc, owner, n) {
                    Some(k) => o::project(svc, &self.base, &k),
                    None => Value::Null,
                }
            }
            "object" => {
                let expr = arg_s(args, "expression");
                match crate::git::resolve_ref(svc, &repo, &expr) {
                    Some(sha) => json!({"__typename": "Commit", "oid": sha, "_repo": repo}),
                    None => Value::Null,
                }
            }
            "ref" => {
                let q = arg_s(args, "qualifiedName");
                let branch = q.strip_prefix("refs/heads/").unwrap_or(&q);
                match crate::git::branch_sha(svc, &repo, branch) {
                    Some(sha) => {
                        json!({"__typename": "Ref", "name": branch, "prefix": "refs/heads/", "target": {"__typename": "Commit", "oid": sha}})
                    }
                    None => Value::Null,
                }
            }
            _ => parent.get(field).cloned().unwrap_or(Value::Null),
        })
    }

    fn issues_connection(&self, repo: &str, args: &Map<String, Value>) -> FieldResult {
        let svc = self.svc();
        let states: Vec<String> = arg(args, "states")
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_ascii_lowercase)
                    .collect()
            })
            .unwrap_or_default();
        let labels: Vec<String> = arg(args, "labels")
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_ascii_lowercase)
                    .collect()
            })
            .unwrap_or_default();
        let filter = arg(args, "filterBy").clone();
        let since = filter.get("since").and_then(Value::as_str).and_then(unix);
        let assignee = filter
            .get("assignee")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let created_by = filter
            .get("createdBy")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let field_filters: Vec<Value> = filter
            .get("issueFieldValues")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let owner = repo.split('/').next().unwrap_or("");
        let defs = issue_fields(svc, owner);
        let mut keys: Vec<(String, Value)> = with_prefix(svc, "issues", &format!("{repo}#"))
            .into_iter()
            .map(|(k, i)| (k.to_owned(), i.clone()))
            .filter(|(_, i)| {
                states.is_empty()
                    || states.contains(&opt_s(i, "state").unwrap_or_else(|| "open".into()))
            })
            .filter(|(_, i)| {
                // `labels` matches issues having any of the labels.
                labels.is_empty()
                    || names_of(arr(i, "labels"))
                        .iter()
                        .any(|l| labels.contains(&l.to_ascii_lowercase()))
            })
            .filter(|(_, i)| since.is_none_or(|x| unix(s(i, "updated_at")).unwrap_or(0) >= x))
            .filter(|(_, i)| {
                assignee.as_ref().is_none_or(|a| {
                    a == "*" && !arr(i, "assignees").is_empty()
                        || names_of(arr(i, "assignees"))
                            .iter()
                            .any(|x| x.eq_ignore_ascii_case(a))
                })
            })
            .filter(|(_, i)| {
                created_by.as_ref().is_none_or(|c| {
                    login_of(i.get("user").unwrap_or(&Value::Null)).eq_ignore_ascii_case(c)
                })
            })
            .filter(|(_, i)| {
                field_filters.iter().all(|ff| {
                    let name = ff.get("fieldName").and_then(Value::as_str).unwrap_or("");
                    let Some(def) = defs
                        .iter()
                        .find(|d| s(d, "name").eq_ignore_ascii_case(name))
                    else {
                        return false;
                    };
                    let current = arr(i, "field_values")
                        .iter()
                        .find(|fv| u(fv, "field_id") == u(def, "id"))
                        .and_then(|fv| fv.get("value").cloned());
                    let Some(current) = current else {
                        return false;
                    };
                    for k in ["textValue", "dateValue", "singleSelectOptionValue"] {
                        if let Some(want) = ff.get(k).and_then(Value::as_str) {
                            return current
                                .as_str()
                                .is_some_and(|c| c.eq_ignore_ascii_case(want));
                        }
                    }
                    if let Some(want) = ff.get("numberValue").and_then(Value::as_f64) {
                        return current.as_f64().is_some_and(|c| (c - want).abs() < 1e-9);
                    }
                    true
                })
            })
            .collect();
        order(
            &mut keys,
            args,
            |(k, i), f| match f {
                "UPDATED_AT" => unix(s(i, "updated_at")).unwrap_or(0),
                "COMMENTS" => {
                    let (r, n) = split_key(k);
                    render::comment_count(svc, r, n) as i64
                }
                _ => unix(s(i, "created_at")).unwrap_or(0) * 1000 + (u(i, "number") % 1000) as i64,
            },
            "CREATED_AT",
            false,
        );
        let nodes: Vec<Value> = keys
            .iter()
            .map(|(k, _)| o::issue(svc, &self.base, k))
            .collect();
        connection(nodes, args)
    }

    // ------------------------------------------------------------ Issue / PullRequest

    fn issue_like_field(
        &mut self,
        parent: &Value,
        field: &str,
        args: &Map<String, Value>,
    ) -> FieldResult {
        let key = internal(parent, "_key").to_owned();
        let repo = internal(parent, "_repo").to_owned();
        let svc = self.svc();
        let is_pr = parent["__typename"] == "PullRequest";
        let doc = get(svc, if is_pr { "pulls" } else { "issues" }, &key)
            .cloned()
            .unwrap_or_default();
        let (_, number) = split_key(&key);
        Ok(match field {
            "author" => self.actor_of(doc.get("user").unwrap_or(&Value::Null)),
            "editor" => Value::Null,
            "repository" => o::repo(svc, &self.base, &repo),
            "labels" => {
                let labels: Vec<Value> = names_of(arr(&doc, "labels"))
                    .iter()
                    .map(|n| {
                        let l = o::label(svc, &self.base, &repo, n);
                        if l.is_null() {
                            let r = render::label_json(svc, &self.base, &repo, n);
                            json!({"__typename": "Label", "id": r["node_id"], "name": r["name"], "color": r["color"], "description": r["description"]})
                        } else {
                            l
                        }
                    })
                    .collect();
                return connection(labels, args);
            }
            "assignees" => {
                let users: Vec<Value> = names_of(arr(&doc, "assignees"))
                    .iter()
                    .map(|l| o::actor(svc, &self.base, l))
                    .collect();
                return connection(users, args);
            }
            "comments" => {
                let mut cs: Vec<Value> = all(svc, "issue_comments")
                    .into_iter()
                    .map(|(_, c)| c.clone())
                    .filter(|c| s(c, "repo") == repo && u(c, "issue_number") == number)
                    .collect();
                cs.sort_by_key(|c| u(c, "id"));
                let nodes: Vec<Value> = cs
                    .iter()
                    .map(|c| {
                        let id = u(c, "id");
                        json!({
                            "__typename": "IssueComment",
                            "id": opt_s(c, "node_id").unwrap_or_else(|| crate::util::node_id("IC", &id.to_string())),
                            "databaseId": id,
                            "body": s(c, "body"),
                            "createdAt": c.get("created_at").cloned().unwrap_or(Value::Null),
                            "author": o::actor(svc, &self.base, &login_of(c.get("user").unwrap_or(&Value::Null))),
                            "url": format!("{}/{repo}/issues/{number}#issuecomment-{id}", self.base),
                            "isMinimized": b(c, "minimized"),
                        })
                    })
                    .collect();
                return connection(nodes, args);
            }
            "milestone" => render::milestone_number(&doc)
                .map(|m| o::milestone(svc, &self.base, &repo, m))
                .unwrap_or(Value::Null),
            "issueType" => match opt_s(&doc, "type") {
                Some(t) => {
                    let owner = repo.split('/').next().unwrap_or("");
                    let it = render::issue_type_json(svc, owner, &t);
                    json!({"__typename": "IssueType", "id": it["node_id"], "name": it["name"], "description": it["description"], "color": it["color"]})
                }
                None => Value::Null,
            },
            "issueFieldValues" => {
                let owner = repo.split('/').next().unwrap_or("");
                let defs = issue_fields(svc, owner);
                let vals: Vec<Value> = arr(&doc, "field_values")
                    .iter()
                    .filter_map(|fv| {
                        let def = defs.iter().find(|d| u(d, "id") == u(fv, "field_id"))?;
                        Some(o::issue_field_value(
                            owner,
                            def,
                            fv.get("value").unwrap_or(&Value::Null),
                        ))
                    })
                    .collect();
                return connection(vals, args);
            }
            "parent" => match parent_key(svc, &key) {
                Some(p) => o::issue(svc, &self.base, &p),
                None => Value::Null,
            },
            "subIssues" => {
                let subs: Vec<Value> = sub_issue_keys(&doc)
                    .iter()
                    .map(|k| o::issue(svc, &self.base, k))
                    .filter(|v| !v.is_null())
                    .collect();
                return connection(subs, args);
            }
            "subIssuesSummary" => {
                let subs = sub_issue_keys(&doc);
                let done = subs
                    .iter()
                    .filter(|k| get(svc, "issues", k).is_some_and(|d| s(d, "state") == "closed"))
                    .count();
                json!({
                    "total": subs.len(),
                    "completed": done,
                    "percentCompleted": (done * 100).checked_div(subs.len()).unwrap_or(0),
                })
            }
            "closedByPullRequestsReferences" => {
                let include_closed = arg(args, "includeClosedPrs").as_bool().unwrap_or(false);
                let mut prs: Vec<Value> = with_prefix(svc, "pulls", &format!("{repo}#"))
                    .iter()
                    .filter(|(_, p)| {
                        crate::pulls::closing_refs(&format!("{}\n{}", s(p, "title"), s(p, "body")))
                            .contains(&number)
                    })
                    .filter(|(_, p)| include_closed || s(p, "state") == "open")
                    .map(|(k, _)| o::pull(svc, &self.base, k))
                    .collect();
                if arg(args, "orderByState").as_bool().unwrap_or(false) {
                    prs.sort_by_key(|p| match p["state"].as_str() {
                        Some("OPEN") => 0,
                        Some("MERGED") => 1,
                        _ => 2,
                    });
                }
                return connection(prs, args);
            }
            "timelineItems" => {
                let pat = format!("#{number}");
                let events: Vec<Value> = with_prefix(svc, "pulls", &format!("{repo}#"))
                    .iter()
                    .filter(|(k, p)| *k != key && (s(p, "body").contains(&pat) || s(p, "title").contains(&pat)))
                    .map(|(k, p)| json!({
                        "__typename": "CrossReferencedEvent",
                        "createdAt": p.get("created_at").cloned().unwrap_or(Value::Null),
                        "source": o::pull(svc, &self.base, k),
                        "willCloseTarget": crate::pulls::closing_refs(s(p, "body")).contains(&number),
                    }))
                    .collect();
                return connection(events, args);
            }
            "projectItems" => {
                let include_archived = arg(args, "includeArchived").as_bool().unwrap_or(true);
                let items: Vec<Value> = all(svc, "project_items")
                    .into_iter()
                    .filter(|(_, i)| s(i, "content") == key)
                    .filter(|(_, i)| {
                        include_archived || i.get("archived_at").is_none_or(Value::is_null)
                    })
                    .map(|(k, _)| o::project_item(svc, k))
                    .collect();
                return connection(items, args);
            }
            "reviewThreads" if is_pr => {
                let threads: Vec<Value> = o::thread_roots(svc, &key, &self.viewer)
                    .iter()
                    .map(|r| o::thread(svc, r))
                    .collect();
                return connection(threads, args);
            }
            "reviews" if is_pr => {
                let states: Vec<String> = arg(args, "states")
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                let viewer = self.viewer.clone();
                let reviews: Vec<Value> = crate::pulls::reviews_of(svc, &key)
                    .iter()
                    .filter(|r| states.is_empty() || states.iter().any(|st| st == s(r, "state")))
                    .filter(|r| {
                        s(r, "state") != "PENDING"
                            || login_of(r.get("user").unwrap_or(&Value::Null)) == viewer
                    })
                    .map(|r| o::review(svc, &self.base, r))
                    .collect();
                return connection(reviews, args);
            }
            "reviewRequests" if is_pr => {
                let reqs: Vec<Value> = names_of(arr(&doc, "requested_reviewers"))
                    .iter()
                    .map(|l| json!({"__typename": "ReviewRequest", "requestedReviewer": o::actor(svc, &self.base, l)}))
                    .collect();
                return connection(reqs, args);
            }
            "mergedBy" if is_pr => doc
                .get("merged_by")
                .map(|m| self.actor_of(m))
                .unwrap_or(Value::Null),
            _ => parent.get(field).cloned().unwrap_or(Value::Null),
        })
    }
}

impl Resolver for Gh<'_> {
    fn type_matches(&self, typename: &str, cond: &str) -> bool {
        if typename == cond {
            return true;
        }
        match cond {
            "Node" | "UniformResourceLocatable" => true,
            "Actor" | "Assignee" | "RequestedReviewer" => matches!(
                typename,
                "User" | "Bot" | "Organization" | "Mannequin" | "EnterpriseUserAccount" | "Team"
            ),
            "RepositoryOwner" | "ProjectV2Owner" => matches!(typename, "User" | "Organization"),
            "IssueOrPullRequest" | "Assignable" | "Labelable" | "Closable" | "Comment"
            | "Reactable" => {
                matches!(
                    typename,
                    "Issue"
                        | "PullRequest"
                        | "Discussion"
                        | "DiscussionComment"
                        | "IssueComment"
                        | "PullRequestReviewComment"
                        | "PullRequestReview"
                )
            }
            "IssueFieldCommon" | "IssueFields" => {
                typename.starts_with("IssueField") && !typename.ends_with("Value")
            }
            "IssueFieldValueCommon" | "IssueFieldValue" => {
                typename.starts_with("IssueField") && typename.ends_with("Value")
            }
            "ProjectV2FieldCommon" | "ProjectV2FieldConfiguration" => {
                typename.starts_with("ProjectV2") && typename.ends_with("Field")
            }
            "ProjectV2ItemContent" => matches!(typename, "Issue" | "PullRequest" | "DraftIssue"),
            "Minimizable" | "Deletable" | "Updatable" => true,
            _ => false,
        }
    }

    fn resolve(&mut self, parent: &Value, field: &str, args: &Map<String, Value>) -> FieldResult {
        let typename = parent["__typename"].as_str().unwrap_or("").to_owned();
        match typename.as_str() {
            "Query" => self.query(field, args),
            "Mutation" => self.mutation(field, args),
            "Repository" => self.repository_field(parent, field, args),
            "Issue" | "PullRequest" => self.issue_like_field(parent, field, args),
            "User" | "Organization" | "Bot" | "Mannequin" | "EnterpriseUserAccount" => {
                self.account_field(parent, field, args)
            }
            "Team" => self.team_field(parent, field, args),
            "Discussion" => self.discussion_field(parent, field, args),
            "DiscussionComment" => self.discussion_comment_field(parent, field, args),
            "PullRequestReviewThread" => self.thread_field(parent, field, args),
            "PullRequestReview" => {
                let svc = self.svc();
                let id = parent["_id"].as_u64().unwrap_or(0).to_string();
                let r = get(svc, "reviews", &id).cloned().unwrap_or_default();
                Ok(match field {
                    "author" => self.actor_of(r.get("user").unwrap_or(&Value::Null)),
                    "pullRequest" => o::pull(svc, &self.base, internal(parent, "_pull")),
                    "comments" => {
                        let rid = u(&r, "id");
                        let cs: Vec<Value> = all(svc, "review_comments")
                            .into_iter()
                            .filter(|(_, c)| u(c, "review_id") == rid)
                            .map(|(_, c)| o::review_comment(&self.base, c))
                            .collect();
                        return connection(cs, args);
                    }
                    _ => parent.get(field).cloned().unwrap_or(Value::Null),
                })
            }
            "PullRequestReviewComment" => {
                let svc = self.svc();
                Ok(match field {
                    "author" => o::actor(svc, &self.base, internal(parent, "_user")),
                    _ => parent.get(field).cloned().unwrap_or(Value::Null),
                })
            }
            "ProjectV2" => self.project_field(parent, field, args),
            "ProjectV2Item" => self.project_item_field(parent, field, args),
            "ProjectV2View" => self.view_field(parent, field, args),
            "ProjectV2ViewConfiguration" => {
                let svc = self.svc();
                let vid = internal(parent, "_view").to_owned();
                let v = get(svc, "project_views", &vid).cloned().unwrap_or_default();
                let pkey = s(&v, "project").to_owned();
                Ok(match field {
                    "visibleFields" => {
                        let ids: Vec<u64> = arr(&v, "visible_fields")
                            .iter()
                            .filter_map(Value::as_u64)
                            .collect();
                        let defs = projects::fields_of(svc, &pkey);
                        let nodes: Vec<Value> = ids
                            .iter()
                            .filter_map(|id| defs.iter().find(|f| u(f, "id") == *id))
                            .map(|f| o::project_field(&pkey, f))
                            .collect();
                        return connection(nodes, args);
                    }
                    _ => parent.get(field).cloned().unwrap_or(Value::Null),
                })
            }
            "ProjectV2StatusUpdate" => {
                let svc = self.svc();
                Ok(match field {
                    "creator" => o::actor(svc, &self.base, internal(parent, "_creator")),
                    "project" => o::project(svc, &self.base, internal(parent, "_project")),
                    _ => parent.get(field).cloned().unwrap_or(Value::Null),
                })
            }
            "ProjectV2Field" | "ProjectV2SingleSelectField" | "ProjectV2IterationField" => {
                let svc = self.svc();
                Ok(match field {
                    "project" => o::project(svc, &self.base, internal(parent, "_project")),
                    _ => parent.get(field).cloned().unwrap_or(Value::Null),
                })
            }
            "Commit" => Ok(match field {
                "blame" => json!({"ranges": []}),
                _ => parent.get(field).cloned().unwrap_or(Value::Null),
            }),
            _ => Ok(parent.get(field).cloned().unwrap_or(Value::Null)),
        }
    }
}

impl Gh<'_> {
    fn query(&mut self, field: &str, args: &Map<String, Value>) -> FieldResult {
        let svc = self.svc();
        match field {
            "viewer" => Ok(o::actor(svc, &self.base, &self.viewer)),
            "user" => self.account_obj(&arg_s(args, "login"), Some(false)),
            "organization" => self.account_obj(&arg_s(args, "login"), Some(true)),
            "repositoryOwner" => {
                let login = arg_s(args, "login");
                if account(svc, &login).is_some() {
                    Ok(o::actor(svc, &self.base, &login))
                } else {
                    Ok(Value::Null)
                }
            }
            "repository" => self.repo_or_err(&arg_s(args, "owner"), &arg_s(args, "name")),
            "node" => {
                let id = arg_s(args, "id");
                let v = o::find_node(svc, &self.base, &id);
                if v.is_null() {
                    return Err(FieldError::not_found(format!(
                        "Could not resolve to a node with the global id of '{id}'"
                    )));
                }
                Ok(v)
            }
            "nodes" => {
                let ids: Vec<String> = arg(args, "ids")
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                let mut out = Vec::new();
                for id in &ids {
                    let v = o::find_node(svc, &self.base, id);
                    if v.is_null() {
                        return Err(FieldError::not_found(format!(
                            "Could not resolve to a node with the global id of '{id}'"
                        )));
                    }
                    out.push(v);
                }
                Ok(Value::Array(out))
            }
            "rateLimit" => Ok(
                json!({"__typename": "RateLimit", "limit": 5000, "cost": 1, "remaining": 4999, "used": 1, "resetAt": self.now, "nodeCount": 0}),
            ),
            "meta" => Ok(
                json!({"__typename": "GitHubMetadata", "isPasswordAuthenticationVerifiable": true}),
            ),
            _ => Err(FieldError::invalid(format!(
                "Field '{field}' doesn't exist on type 'Query'"
            ))),
        }
    }

    fn account_field(
        &mut self,
        parent: &Value,
        field: &str,
        args: &Map<String, Value>,
    ) -> FieldResult {
        let login = internal(parent, "_login").to_owned();
        let svc = self.svc();
        Ok(match field {
            "organizations" => {
                let orgs: Vec<Value> = all(svc, "orgs")
                    .iter()
                    .filter(|(_, org)| {
                        names_of(arr(org, "members"))
                            .iter()
                            .any(|m| m.eq_ignore_ascii_case(&login))
                    })
                    .map(|(k, org)| {
                        o::actor(
                            svc,
                            &self.base,
                            &opt_s(org, "login").unwrap_or_else(|| (*k).to_owned()),
                        )
                    })
                    .collect();
                return connection(orgs, args);
            }
            "teams" => {
                let filter: Vec<String> = arg(args, "userLogins")
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                let teams: Vec<Value> = with_prefix(svc, "teams", &format!("{login}/"))
                    .iter()
                    .filter(|(_, t)| {
                        filter.is_empty()
                            || names_of(arr(t, "members"))
                                .iter()
                                .any(|m| filter.iter().any(|f| f.eq_ignore_ascii_case(m)))
                    })
                    .map(|(_, t)| o::team(svc, &self.base, &login, s(t, "slug")))
                    .collect();
                return connection(teams, args);
            }
            "team" => o::team(svc, &self.base, &login, &arg_s(args, "slug")),
            "membersWithRole" => {
                let members: Vec<Value> = account(svc, &login)
                    .map(|a| {
                        names_of(arr(a, "members"))
                            .iter()
                            .map(|m| o::actor(svc, &self.base, m))
                            .collect()
                    })
                    .unwrap_or_default();
                return connection(members, args);
            }
            "projectV2" => {
                let n = arg_u(args, "number").unwrap_or(0);
                match projects::project_key(svc, &login, n) {
                    Some(k) => o::project(svc, &self.base, &k),
                    None => {
                        return Err(FieldError::not_found(format!(
                            "Could not resolve to a ProjectV2 with the number {n}."
                        )));
                    }
                }
            }
            "projectsV2" => {
                let mut ps: Vec<(u64, Value)> = with_prefix(svc, "projects", &format!("{login}/"))
                    .iter()
                    .map(|(k, p)| (u(p, "number"), o::project(svc, &self.base, k)))
                    .collect();
                ps.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
                return connection(ps.into_iter().map(|(_, p)| p).collect(), args);
            }
            "issueFields" => {
                let fields: Vec<Value> = issue_fields(svc, &login)
                    .iter()
                    .map(|f| o::issue_field(&login, f))
                    .collect();
                return connection(fields, args);
            }
            "repository" => {
                let name = arg_s(args, "name");
                return self.repo_or_err(&login, &name);
            }
            "repositories" => {
                let repos: Vec<Value> = all(svc, "repos")
                    .iter()
                    .filter(|(_, r)| render::repo_owner(r).eq_ignore_ascii_case(&login))
                    .map(|(k, _)| o::repo(svc, &self.base, k))
                    .collect();
                return connection(repos, args);
            }
            "status" => Value::Null,
            _ => parent.get(field).cloned().unwrap_or(Value::Null),
        })
    }

    fn team_field(
        &mut self,
        parent: &Value,
        field: &str,
        args: &Map<String, Value>,
    ) -> FieldResult {
        let key = internal(parent, "_key").to_owned();
        let svc = self.svc();
        let t = get(svc, "teams", &key).cloned().unwrap_or_default();
        Ok(match field {
            "members" => {
                let members: Vec<Value> = names_of(arr(&t, "members"))
                    .iter()
                    .map(|m| o::actor(svc, &self.base, m))
                    .collect();
                return connection(members, args);
            }
            "organization" => o::actor(svc, &self.base, key.split('/').next().unwrap_or("")),
            _ => parent.get(field).cloned().unwrap_or(Value::Null),
        })
    }

    fn discussion_field(
        &mut self,
        parent: &Value,
        field: &str,
        args: &Map<String, Value>,
    ) -> FieldResult {
        let key = internal(parent, "_key").to_owned();
        let repo = internal(parent, "_repo").to_owned();
        let svc = self.svc();
        let d = get(svc, "discussions", &key).cloned().unwrap_or_default();
        Ok(match field {
            "author" => self.actor_of(d.get("author").unwrap_or(&Value::Null)),
            "category" => match o::category_doc(svc, &repo, s(&d, "category")) {
                Some((k, c)) => o::category(k, c),
                None => {
                    json!({"__typename": "DiscussionCategory", "id": crate::util::node_id("DIC", &format!("{repo}:{}", s(&d, "category"))), "name": s(&d, "category"), "slug": s(&d, "category"), "isAnswerable": false})
                }
            },
            "repository" => o::repo(svc, &self.base, &repo),
            "answer" => match opt_s(&d, "answer_comment") {
                Some(id) => o::discussion_comment(svc, &self.base, &id),
                None => Value::Null,
            },
            "answerChosenBy" => d
                .get("answer_chosen_by")
                .map(|a| self.actor_of(a))
                .unwrap_or(Value::Null),
            "labels" => {
                let labels: Vec<Value> = names_of(arr(&d, "labels"))
                    .iter()
                    .map(|n| o::label(svc, &self.base, &repo, n))
                    .filter(|l| !l.is_null())
                    .collect();
                return connection(labels, args);
            }
            "comments" => {
                let mut cs: Vec<(String, Value)> = all(svc, "discussion_comments")
                    .into_iter()
                    .filter(|(_, c)| {
                        s(c, "discussion") == key && c.get("reply_to").is_none_or(Value::is_null)
                    })
                    .map(|(k, c)| (k.to_owned(), c.clone()))
                    .collect();
                cs.sort_by_key(|(_, c)| (unix(s(c, "created_at")).unwrap_or(0), u(c, "id")));
                let nodes: Vec<Value> = cs
                    .iter()
                    .map(|(k, _)| o::discussion_comment(svc, &self.base, k))
                    .collect();
                return connection(nodes, args);
            }
            _ => parent.get(field).cloned().unwrap_or(Value::Null),
        })
    }

    fn discussion_comment_field(
        &mut self,
        parent: &Value,
        field: &str,
        args: &Map<String, Value>,
    ) -> FieldResult {
        let id = internal(parent, "_key").to_owned();
        let svc = self.svc();
        let c = get(svc, "discussion_comments", &id)
            .cloned()
            .unwrap_or_default();
        Ok(match field {
            "author" => self.actor_of(c.get("author").unwrap_or(&Value::Null)),
            "discussion" => o::discussion(svc, &self.base, internal(parent, "_discussion")),
            "replyTo" => match opt_s(&c, "reply_to") {
                Some(r) => o::discussion_comment(svc, &self.base, &r),
                None => Value::Null,
            },
            "replies" => {
                let mut rs: Vec<(String, Value)> = all(svc, "discussion_comments")
                    .into_iter()
                    .filter(|(_, r)| s(r, "reply_to") == id)
                    .map(|(k, r)| (k.to_owned(), r.clone()))
                    .collect();
                rs.sort_by_key(|(_, r)| (unix(s(r, "created_at")).unwrap_or(0), u(r, "id")));
                let nodes: Vec<Value> = rs
                    .iter()
                    .map(|(k, _)| o::discussion_comment(svc, &self.base, k))
                    .collect();
                return connection(nodes, args);
            }
            _ => parent.get(field).cloned().unwrap_or(Value::Null),
        })
    }

    fn thread_field(
        &mut self,
        parent: &Value,
        field: &str,
        args: &Map<String, Value>,
    ) -> FieldResult {
        let root = parent["_root"].as_u64().unwrap_or(0);
        let svc = self.svc();
        Ok(match field {
            "comments" => {
                let cs: Vec<Value> = o::thread_comments(svc, root)
                    .iter()
                    .map(|c| o::review_comment(&self.base, c))
                    .collect();
                return connection(cs, args);
            }
            "pullRequest" => o::pull(svc, &self.base, internal(parent, "_pull")),
            "resolvedBy" => {
                let tid = parent["id"].as_str().unwrap_or("");
                get(svc, "review_threads", tid)
                    .and_then(|t| t.get("resolved_by"))
                    .map(|l| self.actor_of(l))
                    .unwrap_or(Value::Null)
            }
            "viewerCanResolve" | "viewerCanUnresolve" | "viewerCanReply" => json!(true),
            _ => parent.get(field).cloned().unwrap_or(Value::Null),
        })
    }

    fn project_field(
        &mut self,
        parent: &Value,
        field: &str,
        args: &Map<String, Value>,
    ) -> FieldResult {
        let pkey = internal(parent, "_key").to_owned();
        let svc = self.svc();
        let p = get(svc, "projects", &pkey).cloned().unwrap_or_default();
        Ok(match field {
            "owner" => o::actor(svc, &self.base, pkey.split('/').next().unwrap_or("")),
            "creator" => self.actor_of(p.get("creator").unwrap_or(&Value::Null)),
            "fields" => {
                let fields: Vec<Value> = projects::fields_of(svc, &pkey)
                    .iter()
                    .map(|f| o::project_field(&pkey, f))
                    .collect();
                return connection(fields, args);
            }
            "field" => {
                let name = arg_s(args, "name");
                projects::fields_of(svc, &pkey)
                    .iter()
                    .find(|f| s(f, "name").eq_ignore_ascii_case(&name))
                    .map(|f| o::project_field(&pkey, f))
                    .unwrap_or(Value::Null)
            }
            "items" => {
                let items: Vec<Value> = projects::items_of(svc, &pkey)
                    .iter()
                    .map(|(k, _)| o::project_item(svc, k))
                    .collect();
                return connection(items, args);
            }
            "views" => {
                let mut views: Vec<(u64, String)> = all(svc, "project_views")
                    .into_iter()
                    .filter(|(_, v)| s(v, "project") == pkey)
                    .map(|(k, v)| (u(v, "number"), k.to_owned()))
                    .collect();
                views.sort();
                let nodes: Vec<Value> = views.iter().map(|(_, k)| o::view(svc, k)).collect();
                return connection(nodes, args);
            }
            "statusUpdates" => {
                let mut ups: Vec<(i64, String)> = all(svc, "project_status_updates")
                    .into_iter()
                    .filter(|(_, su)| s(su, "project") == pkey)
                    .map(|(k, su)| (unix(s(su, "created_at")).unwrap_or(0), k.to_owned()))
                    .collect();
                ups.sort();
                let asc = arg(args, "orderBy")
                    .get("direction")
                    .and_then(Value::as_str)
                    == Some("ASC");
                if !asc {
                    ups.reverse();
                }
                let nodes: Vec<Value> = ups.iter().map(|(_, k)| o::status_update(svc, k)).collect();
                return connection(nodes, args);
            }
            _ => parent.get(field).cloned().unwrap_or(Value::Null),
        })
    }

    fn project_item_field(
        &mut self,
        parent: &Value,
        field: &str,
        args: &Map<String, Value>,
    ) -> FieldResult {
        let key = internal(parent, "_key").to_owned();
        let pkey = internal(parent, "_project").to_owned();
        let svc = self.svc();
        let i = get(svc, "project_items", &key).cloned().unwrap_or_default();
        Ok(match field {
            "project" => o::project(svc, &self.base, &pkey),
            "content" => {
                let c = s(&i, "content");
                if c.is_empty() {
                    json!({"__typename": "DraftIssue", "id": crate::util::node_id("DI", &key), "title": s(&i, "title"), "body": s(&i, "body")})
                } else {
                    o::issue_or_pull(svc, &self.base, c)
                }
            }
            "creator" => self.actor_of(i.get("creator").unwrap_or(&Value::Null)),
            "fieldValues" => {
                let vals: Vec<Value> = projects::fields_of(svc, &pkey)
                    .iter()
                    .filter_map(|f| {
                        let raw = projects::field_value(svc, &i, f);
                        if raw.is_null() {
                            return None;
                        }
                        let field_obj = o::project_field(&pkey, f);
                        Some(match projects::data_type(f).as_str() {
                            "SINGLE_SELECT" => {
                                let opt = raw.as_str().and_then(|id| projects::option_by_id(f, id)).cloned().unwrap_or_default();
                                json!({"__typename": "ProjectV2ItemFieldSingleSelectValue", "field": field_obj, "name": s(&opt, "name"), "optionId": s(&opt, "id")})
                            }
                            "ITERATION" => {
                                let it = raw.as_str().and_then(|id| projects::iteration_by_id(f, id)).cloned().unwrap_or_default();
                                json!({"__typename": "ProjectV2ItemFieldIterationValue", "field": field_obj, "title": s(&it, "title"), "iterationId": s(&it, "id"), "startDate": s(&it, "start_date"), "duration": u(&it, "duration")})
                            }
                            "NUMBER" => json!({"__typename": "ProjectV2ItemFieldNumberValue", "field": field_obj, "number": raw}),
                            "DATE" => json!({"__typename": "ProjectV2ItemFieldDateValue", "field": field_obj, "date": raw}),
                            _ => json!({"__typename": "ProjectV2ItemFieldTextValue", "field": field_obj, "text": raw}),
                        })
                    })
                    .collect();
                return connection(vals, args);
            }
            _ => parent.get(field).cloned().unwrap_or(Value::Null),
        })
    }

    fn view_field(
        &mut self,
        parent: &Value,
        field: &str,
        args: &Map<String, Value>,
    ) -> FieldResult {
        let vid = internal(parent, "_key").to_owned();
        let pkey = internal(parent, "_project").to_owned();
        let svc = self.svc();
        Ok(match field {
            "project" => o::project(svc, &self.base, &pkey),
            "configuration" => json!({"__typename": "ProjectV2ViewConfiguration", "_view": vid}),
            "visibleFields" | "fields" => {
                let v = get(svc, "project_views", &vid).cloned().unwrap_or_default();
                let ids: Vec<u64> = arr(&v, "visible_fields")
                    .iter()
                    .filter_map(Value::as_u64)
                    .collect();
                let defs = projects::fields_of(svc, &pkey);
                let nodes: Vec<Value> = ids
                    .iter()
                    .filter_map(|id| defs.iter().find(|f| u(f, "id") == *id))
                    .map(|f| o::project_field(&pkey, f))
                    .collect();
                return connection(nodes, args);
            }
            _ => parent.get(field).cloned().unwrap_or(Value::Null),
        })
    }
}
