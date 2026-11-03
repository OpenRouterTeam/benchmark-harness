//! Projects (v2) REST API: `/orgs/{org}/projectsV2` and `/users/{user}/projectsV2`.
//!
//! * `projects`        key `owner/N`: `{id, node_id, number, title, short_description, public,
//!   closed, owner, creator, created_at, updated_at, readme}`
//! * `project_fields`  key `owner/N:fieldId`: `{id, node_id, name, data_type, options, configuration}`
//!   (`data_type` upper-case GraphQL style: TEXT, NUMBER, DATE, SINGLE_SELECT, ITERATION, TITLE, ...)
//! * `project_items`   key `owner/N:itemId`: `{id, node_id, content_type, content: "owner/repo#N",
//!   title (drafts), field_values: {fieldId: value}, archived_at, created_at, updated_at, creator}`
//! * `project_status_updates` / `project_views`: GraphQL-only, see `graphql`.

use axum::{
    http::{Method, StatusCode},
    response::Response,
};
use mock_core::world::ServiceState;
use serde_json::{Map, Value, json};

use crate::{
    Ctx,
    render::{self, account, all, get, is_org, issue_json, pr_json, user, user_ref, with_prefix},
    util::{
        error, login_of, names_of, next_id, no_content, node_id, not_found, ok, opt_s,
        percent_encode_query, s, u, unix, validation, with_header,
    },
};

pub fn route(ctx: &mut Ctx, segs: &[&str]) -> Option<Response> {
    let m = ctx.req.method.clone();
    let (kind, owner, rest) = match segs {
        ["orgs", o, "projectsV2", rest @ ..] => ("org", *o, rest),
        ["users", o, "projectsV2", rest @ ..] => ("user", *o, rest),
        _ => return None,
    };
    let owner_ok = match kind {
        "org" => is_org(ctx.svc, owner),
        _ => account(ctx.svc, owner).is_some() && !is_org(ctx.svc, owner),
    };
    if !owner_ok {
        return Some(not_found());
    }
    let owner = account(ctx.svc, owner)
        .map(|a| s(a, "login").to_owned())
        .unwrap_or_else(|| owner.to_owned());
    let get_ = m == Method::GET;
    Some(match rest {
        [] if get_ => list_projects(ctx, &owner),
        [n, rest @ ..] => {
            let pkey = format!("{owner}/{n}");
            if get(ctx.svc, "projects", &pkey).is_none() {
                return Some(not_found());
            }
            match rest {
                [] if get_ => ok(project_json(ctx.svc, ctx.base(), &pkey)),
                ["fields"] if get_ => {
                    let fields: Vec<Value> = fields_of(ctx.svc, &pkey)
                        .iter()
                        .map(|f| field_json(ctx, &pkey, f))
                        .collect();
                    cursor_list(ctx, fields)
                }
                ["fields", id] if get_ => {
                    match get(ctx.svc, "project_fields", &format!("{pkey}:{id}")) {
                        Some(f) => ok(field_json(ctx, &pkey, f)),
                        None => not_found(),
                    }
                }
                ["items"] if get_ => list_items(ctx, &pkey),
                ["items"] if m == Method::POST => add_item(ctx, &pkey),
                ["items", id] if get_ => {
                    match get(ctx.svc, "project_items", &format!("{pkey}:{id}")) {
                        Some(i) => {
                            let fields = requested_fields(ctx);
                            ok(item_json(ctx.svc, ctx.base(), &pkey, i, fields.as_deref()))
                        }
                        None => not_found(),
                    }
                }
                ["items", id] if m == Method::PATCH => update_item(ctx, &pkey, id),
                ["items", id] if m == Method::DELETE => {
                    let key = format!("{pkey}:{id}");
                    if ctx
                        .svc
                        .store
                        .collection_mut("project_items")
                        .remove(&key)
                        .is_some()
                    {
                        no_content()
                    } else {
                        not_found()
                    }
                }
                _ => not_found(),
            }
        }
        _ => not_found(),
    })
}

// ---------------------------------------------------------------- lookups shared with GraphQL

pub fn project_key(svc: &ServiceState, owner: &str, number: u64) -> Option<String> {
    let want = format!("{owner}/{number}").to_ascii_lowercase();
    svc.store.collection("projects").and_then(|c| {
        c.iter()
            .find(|(k, _)| k.to_ascii_lowercase() == want)
            .map(|(k, _)| k.to_owned())
    })
}

pub fn project_node_id(svc: &ServiceState, pkey: &str) -> String {
    get(svc, "projects", pkey)
        .and_then(|p| opt_s(p, "node_id"))
        .unwrap_or_else(|| node_id("PVT", pkey))
}

pub fn fields_of(svc: &ServiceState, pkey: &str) -> Vec<Value> {
    let mut v: Vec<Value> = with_prefix(svc, "project_fields", &format!("{pkey}:"))
        .into_iter()
        .map(|(_, f)| f.clone())
        .collect();
    v.sort_by_key(|f| u(f, "id"));
    v
}

pub fn items_of(svc: &ServiceState, pkey: &str) -> Vec<(String, Value)> {
    let mut v: Vec<(String, Value)> = with_prefix(svc, "project_items", &format!("{pkey}:"))
        .into_iter()
        .map(|(k, i)| (k.to_owned(), i.clone()))
        .collect();
    v.sort_by_key(|(_, i)| u(i, "id"));
    v
}

pub fn field_node_id(pkey: &str, f: &Value) -> String {
    opt_s(f, "node_id").unwrap_or_else(|| {
        let prefix = match data_type(f).as_str() {
            "SINGLE_SELECT" => "PVTSSF",
            "ITERATION" => "PVTIF",
            _ => "PVTF",
        };
        node_id(prefix, &format!("{pkey}:{}", u(f, "id")))
    })
}

pub fn item_node_id(key: &str, i: &Value) -> String {
    opt_s(i, "node_id").unwrap_or_else(|| node_id("PVTI", key))
}

pub fn data_type(f: &Value) -> String {
    s(f, "data_type").to_ascii_uppercase()
}

/// Display value of an item's field (option/iteration names resolved).
pub fn field_value(svc: &ServiceState, i: &Value, f: &Value) -> Value {
    let fid = u(f, "id").to_string();
    let content_key = s(i, "content");
    let content_doc = if content_key.is_empty() {
        None
    } else {
        get(svc, "issues", content_key).or_else(|| get(svc, "pulls", content_key))
    };
    match data_type(f).as_str() {
        "TITLE" => json!(
            content_doc.map_or_else(|| s(i, "title").to_owned(), |d| s(d, "title").to_owned())
        ),
        "ASSIGNEES" => json!(
            content_doc
                .map(|d| names_of(crate::util::arr(d, "assignees")))
                .unwrap_or_default()
        ),
        "LABELS" => json!(
            content_doc
                .map(|d| names_of(crate::util::arr(d, "labels")))
                .unwrap_or_default()
        ),
        "REPOSITORY" => json!(content_key.rsplit_once('#').map(|(r, _)| r).unwrap_or("")),
        _ => i
            .get("field_values")
            .and_then(|fv| fv.get(&fid))
            .cloned()
            .unwrap_or(Value::Null),
    }
}

pub fn option_by_id<'a>(f: &'a Value, id: &str) -> Option<&'a Value> {
    crate::util::arr(f, "options")
        .iter()
        .find(|o| s(o, "id") == id || s(o, "name").eq_ignore_ascii_case(id))
}

pub fn iteration_by_id<'a>(f: &'a Value, id: &str) -> Option<&'a Value> {
    f.get("configuration")
        .and_then(|c| c.get("iterations"))
        .and_then(Value::as_array)
        .and_then(|its| {
            its.iter()
                .find(|it| s(it, "id") == id || s(it, "title").eq_ignore_ascii_case(id))
        })
}

// ---------------------------------------------------------------- JSON

pub fn project_json(svc: &ServiceState, base: &str, pkey: &str) -> Value {
    let p = get(svc, "projects", pkey).cloned().unwrap_or_default();
    let (owner, number) = pkey.split_once('/').unwrap_or((pkey, "0"));
    let org = is_org(svc, owner);
    let html = if org {
        format!("{base}/orgs/{owner}/projects/{number}")
    } else {
        format!("{base}/users/{owner}/projects/{number}")
    };
    let mut owner_json = user(svc, base, owner);
    if org {
        owner_json["type"] = json!("Organization");
    }
    let latest_status = all(svc, "project_status_updates")
        .into_iter()
        .filter(|(_, su)| s(su, "project") == pkey)
        .max_by_key(|(_, su)| unix(s(su, "created_at")).unwrap_or(0))
        .map(|(k, su)| {
            json!({
                "id": u(su, "database_id"),
                "node_id": k,
                "creator": user_ref(svc, base, su.get("creator").unwrap_or(&Value::Null)),
                "created_at": su.get("created_at").cloned().unwrap_or(Value::Null),
                "updated_at": su.get("updated_at").cloned().or_else(|| su.get("created_at").cloned()).unwrap_or(Value::Null),
                "status": su.get("status").cloned().unwrap_or(Value::Null),
                "start_date": su.get("start_date").cloned().unwrap_or(Value::Null),
                "target_date": su.get("target_date").cloned().unwrap_or(Value::Null),
                "body": su.get("body").cloned().unwrap_or(Value::Null),
            })
        })
        .unwrap_or(Value::Null);
    json!({
        "id": u(&p, "id"),
        "node_id": project_node_id(svc, pkey),
        "owner": owner_json,
        "creator": user_ref(svc, base, p.get("creator").unwrap_or(&json!(owner))),
        "title": s(&p, "title"),
        "description": p.get("description").cloned().unwrap_or(Value::Null),
        "public": p.get("public").cloned().unwrap_or(json!(false)),
        "closed_at": p.get("closed_at").cloned().unwrap_or(Value::Null),
        "created_at": p.get("created_at").cloned().unwrap_or(Value::Null),
        "updated_at": p.get("updated_at").cloned().unwrap_or(Value::Null),
        "number": number.parse::<u64>().unwrap_or(0),
        "short_description": p.get("short_description").cloned().unwrap_or(Value::Null),
        "deleted_at": null,
        "deleted_by": null,
        "state": if p.get("closed").and_then(Value::as_bool).unwrap_or(false) { "closed" } else { "open" },
        "latest_status_update": latest_status,
        "is_template": false,
        "url": format!("{base}/api/v3/{}/{owner}/projectsV2/{number}", if org { "orgs" } else { "users" }),
        "html_url": html,
    })
}

fn text_content(v: &str) -> Value {
    json!({"raw": v, "html": v})
}

pub fn field_json(ctx: &Ctx, pkey: &str, f: &Value) -> Value {
    let (owner, number) = pkey.split_once('/').unwrap_or((pkey, "0"));
    let org = is_org(ctx.svc, owner);
    let mut v = json!({
        "id": u(f, "id"),
        "node_id": field_node_id(pkey, f),
        "name": s(f, "name"),
        "data_type": data_type(f).to_ascii_lowercase(),
        "project_url": format!("{}/{}/{owner}/projectsV2/{number}", ctx.req.api(), if org { "orgs" } else { "users" }),
        "created_at": f.get("created_at").cloned().unwrap_or(json!("2025-01-01T00:00:00Z")),
        "updated_at": f.get("updated_at").cloned().unwrap_or(json!("2025-01-01T00:00:00Z")),
    });
    if data_type(f) == "SINGLE_SELECT" {
        v["options"] = Value::Array(
            crate::util::arr(f, "options")
                .iter()
                .map(|o| {
                    json!({
                        "id": s(o, "id"),
                        "name": text_content(s(o, "name")),
                        "color": opt_s(o, "color").unwrap_or_else(|| "GRAY".into()),
                        "description": text_content(s(o, "description")),
                    })
                })
                .collect(),
        );
    }
    if data_type(f) == "ITERATION" {
        let c = f.get("configuration").cloned().unwrap_or(json!({}));
        v["configuration"] = json!({
            "start_day": c.get("start_day").cloned().unwrap_or(json!(1)),
            "duration": c.get("duration").cloned().unwrap_or(json!(14)),
            "iterations": crate::util::arr(&c, "iterations").iter().map(|it| json!({
                "id": s(it, "id"),
                "title": text_content(s(it, "title")),
                "start_date": s(it, "start_date"),
                "duration": u(it, "duration"),
            })).collect::<Vec<_>>(),
        });
    }
    v
}

fn rest_value(svc: &ServiceState, base: &str, i: &Value, f: &Value) -> Value {
    let raw = field_value(svc, i, f);
    match data_type(f).as_str() {
        "SINGLE_SELECT" => raw
            .as_str()
            .and_then(|id| option_by_id(f, id))
            .map(|o| {
                json!({
                    "id": s(o, "id"),
                    "name": text_content(s(o, "name")),
                    "color": opt_s(o, "color").unwrap_or_else(|| "GRAY".into()),
                    "description": text_content(s(o, "description")),
                })
            })
            .unwrap_or(Value::Null),
        "ITERATION" => raw
            .as_str()
            .and_then(|id| iteration_by_id(f, id))
            .map(|it| {
                json!({
                    "id": s(it, "id"),
                    "title": text_content(s(it, "title")),
                    "start_date": s(it, "start_date"),
                    "duration": u(it, "duration"),
                })
            })
            .unwrap_or(Value::Null),
        "TITLE" => {
            json!({"raw": raw.as_str().unwrap_or(""), "html": raw.as_str().unwrap_or(""), "content": {"title": raw}})
        }
        "ASSIGNEES" => Value::Array(
            raw.as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(|l| user(svc, base, l))
                        .collect()
                })
                .unwrap_or_default(),
        ),
        _ => raw,
    }
}

pub fn item_json(
    svc: &ServiceState,
    base: &str,
    pkey: &str,
    i: &Value,
    fields: Option<&[u64]>,
) -> Value {
    let (owner, number) = pkey.split_once('/').unwrap_or((pkey, "0"));
    let org = is_org(svc, owner);
    let id = u(i, "id");
    let key = format!("{pkey}:{id}");
    let project_url = format!(
        "{base}/api/v3/{}/{owner}/projectsV2/{number}",
        if org { "orgs" } else { "users" }
    );
    let content_key = s(i, "content");
    let content_type = opt_s(i, "content_type").unwrap_or_else(|| {
        if content_key.is_empty() {
            "DraftIssue".into()
        } else if get(svc, "pulls", content_key).is_some() {
            "PullRequest".into()
        } else {
            "Issue".into()
        }
    });
    let content = match content_type.as_str() {
        "Issue" => issue_json(svc, base, content_key),
        "PullRequest" => pr_json(svc, base, content_key),
        _ => json!({
            "id": id,
            "node_id": node_id("DI", &key),
            "title": s(i, "title"),
            "body": i.get("body").cloned().unwrap_or(Value::Null),
            "user": user_ref(svc, base, i.get("creator").unwrap_or(&Value::Null)),
            "created_at": i.get("created_at").cloned().unwrap_or(Value::Null),
            "updated_at": i.get("updated_at").cloned().unwrap_or(Value::Null),
        }),
    };
    let all_fields = fields_of(svc, pkey);
    let field_values: Vec<Value> = all_fields
        .iter()
        .filter(|f| match fields {
            Some(ids) => ids.contains(&u(f, "id")) || data_type(f) == "TITLE",
            None => data_type(f) == "TITLE",
        })
        .map(|f| {
            json!({
                "id": u(f, "id"),
                "name": s(f, "name"),
                "data_type": data_type(f).to_ascii_lowercase(),
                "value": rest_value(svc, base, i, f),
            })
        })
        .collect();
    json!({
        "id": id,
        "node_id": item_node_id(&key, i),
        "project_url": project_url,
        "content_type": content_type,
        "content": content,
        "creator": user_ref(svc, base, i.get("creator").unwrap_or(&json!(owner))),
        "created_at": i.get("created_at").cloned().unwrap_or(Value::Null),
        "updated_at": i.get("updated_at").cloned().unwrap_or(Value::Null),
        "archived_at": i.get("archived_at").cloned().unwrap_or(Value::Null),
        "item_url": format!("{project_url}/items/{id}"),
        "fields": field_values,
    })
}

// ---------------------------------------------------------------- handlers

/// Cursor-paginated list (`after`/`before`, `per_page`) with a `Link` header.
fn cursor_list(ctx: &Ctx, items: Vec<Value>) -> Response {
    let per_page = ctx
        .req
        .q("per_page")
        .and_then(|p| p.parse::<usize>().ok())
        .unwrap_or(30)
        .clamp(1, 100);
    let cursor = |i: usize| crate::util::b64_encode(format!("cursor:{i}").as_bytes());
    let decode = |c: &str| {
        crate::util::b64_decode(c)
            .and_then(|b| String::from_utf8(b).ok())
            .and_then(|s| {
                s.strip_prefix("cursor:")
                    .and_then(|n| n.parse::<usize>().ok())
            })
    };
    let (start, end) = if let Some(before) = ctx.req.q("before").and_then(decode) {
        (before.saturating_sub(per_page), before)
    } else {
        let start = ctx.req.q("after").and_then(decode).map_or(0, |a| a + 1);
        (start, (start + per_page).min(items.len()))
    };
    let start = start.min(items.len());
    let end = end.min(items.len()).max(start);
    let page: Vec<Value> = items[start..end].to_vec();
    let resp = ok(Value::Array(page));
    let others: Vec<String> = ctx
        .req
        .query
        .iter()
        .filter(|(k, _)| !matches!(k.as_str(), "after" | "before" | "per_page"))
        .map(|(k, v)| format!("{k}={}", percent_encode_query(v)))
        .collect();
    let mut url = format!("{}{}?", ctx.base(), ctx.req.path);
    for o in &others {
        url.push_str(o);
        url.push('&');
    }
    let mut links = Vec::new();
    if end < items.len() && end > 0 {
        links.push(format!(
            "<{url}per_page={per_page}&after={}>; rel=\"next\"",
            cursor(end - 1)
        ));
    }
    if start > 0 {
        links.push(format!(
            "<{url}per_page={per_page}&before={}>; rel=\"prev\"",
            cursor(start)
        ));
    }
    if links.is_empty() {
        resp
    } else {
        with_header(resp, axum::http::header::LINK, &links.join(", "))
    }
}

fn list_projects(ctx: &Ctx, owner: &str) -> Response {
    let q = ctx.req.q("q").map(str::to_ascii_lowercase);
    let mut ps: Vec<(&str, &Value)> = with_prefix(ctx.svc, "projects", &format!("{owner}/"))
        .into_iter()
        .filter(|(_, p)| {
            q.as_ref().is_none_or(|q| {
                q.split_whitespace().all(|t| match t {
                    "is:open" => !p.get("closed").and_then(Value::as_bool).unwrap_or(false),
                    "is:closed" => p.get("closed").and_then(Value::as_bool).unwrap_or(false),
                    t => {
                        s(p, "title").to_ascii_lowercase().contains(t)
                            || s(p, "short_description").to_ascii_lowercase().contains(t)
                    }
                })
            })
        })
        .collect();
    ps.sort_by_key(|(_, p)| std::cmp::Reverse(u(p, "number")));
    let out: Vec<Value> = ps
        .iter()
        .map(|(k, _)| project_json(ctx.svc, ctx.base(), k))
        .collect();
    cursor_list(ctx, out)
}

fn requested_fields(ctx: &Ctx) -> Option<Vec<u64>> {
    let ids: Vec<u64> = ctx
        .req
        .q_all("fields")
        .iter()
        .filter_map(|f| f.parse().ok())
        .collect();
    (!ids.is_empty()).then_some(ids)
}

fn item_matches_query(svc: &ServiceState, pkey: &str, i: &Value, q: &str) -> bool {
    let fields = fields_of(svc, pkey);
    let content_key = s(i, "content");
    let doc = get(svc, "issues", content_key).or_else(|| get(svc, "pulls", content_key));
    let parsed = crate::search::Query::parse(q);
    for (k, v, neg) in &parsed.quals {
        let hit = match (k.as_str(), v.as_str()) {
            ("is", v) => match v {
                "issue" => get(svc, "issues", content_key).is_some(),
                "pr" => get(svc, "pulls", content_key).is_some(),
                "draft" => content_key.is_empty(),
                "open" => doc.is_some_and(|d| s(d, "state") == "open"),
                "closed" => doc.is_some_and(|d| s(d, "state") == "closed"),
                "archived" => i.get("archived_at").is_some_and(|a| !a.is_null()),
                _ => true,
            },
            ("assignee", v) => doc.is_some_and(|d| {
                names_of(crate::util::arr(d, "assignees"))
                    .iter()
                    .any(|a| a.eq_ignore_ascii_case(v.trim_start_matches('@')))
            }),
            ("label", v) => doc.is_some_and(|d| {
                names_of(crate::util::arr(d, "labels"))
                    .iter()
                    .any(|a| a.eq_ignore_ascii_case(v))
            }),
            ("repo", v) => content_key
                .to_ascii_lowercase()
                .starts_with(&format!("{}#", v.to_ascii_lowercase())),
            ("no", v) => fields
                .iter()
                .find(|f| s(f, "name").eq_ignore_ascii_case(v))
                .is_some_and(|f| field_value(svc, i, f).is_null()),
            (k, want) => fields
                .iter()
                .find(|f| s(f, "name").eq_ignore_ascii_case(k))
                .is_some_and(|f| {
                    let raw = field_value(svc, i, f);
                    let name = match data_type(f).as_str() {
                        "SINGLE_SELECT" => raw
                            .as_str()
                            .and_then(|id| option_by_id(f, id))
                            .map(|o| s(o, "name").to_owned()),
                        "ITERATION" => raw
                            .as_str()
                            .and_then(|id| iteration_by_id(f, id))
                            .map(|it| s(it, "title").to_owned()),
                        _ => raw
                            .as_str()
                            .map(str::to_owned)
                            .or_else(|| raw.as_f64().map(|n| n.to_string())),
                    };
                    name.is_some_and(|n| want.split(',').any(|w| n.eq_ignore_ascii_case(w)))
                }),
        };
        if hit == *neg {
            return false;
        }
    }
    let title = doc.map_or_else(|| s(i, "title").to_owned(), |d| s(d, "title").to_owned());
    if !parsed.text_matches(&title.to_ascii_lowercase()) {
        return false;
    }
    true
}

fn list_items(ctx: &Ctx, pkey: &str) -> Response {
    let fields = requested_fields(ctx);
    let q = ctx.req.q("q").unwrap_or("").to_owned();
    let items: Vec<Value> = items_of(ctx.svc, pkey)
        .iter()
        .filter(|(_, i)| {
            let archived = i.get("archived_at").is_some_and(|a| !a.is_null());
            (!archived || q.contains("is:archived")) && item_matches_query(ctx.svc, pkey, i, &q)
        })
        .map(|(_, i)| item_json(ctx.svc, ctx.base(), pkey, i, fields.as_deref()))
        .collect();
    cursor_list(ctx, items)
}

pub fn insert_item(
    svc: &mut ServiceState,
    pkey: &str,
    content_key: &str,
    viewer: &str,
    now: &str,
) -> (String, Value) {
    if let Some((k, i)) = items_of(svc, pkey)
        .into_iter()
        .find(|(_, i)| s(i, "content") == content_key)
    {
        return (k, i);
    }
    let id = next_id(svc, "project_items", "id", 1);
    let key = format!("{pkey}:{id}");
    let content_type = if get(svc, "pulls", content_key).is_some() {
        "PullRequest"
    } else {
        "Issue"
    };
    let doc = json!({
        "id": id,
        "node_id": node_id("PVTI", &key),
        "content_type": content_type,
        "content": content_key,
        "field_values": {},
        "creator": viewer,
        "created_at": now,
        "updated_at": now,
        "archived_at": null,
    });
    svc.store
        .collection_mut("project_items")
        .insert(key.clone(), doc.clone());
    (key, doc)
}

fn add_item(ctx: &mut Ctx, pkey: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let kind = s(&body, "type");
    let id = u(&body, "id");
    let coll = match kind {
        "Issue" => "issues",
        "PullRequest" => "pulls",
        _ => return validation("ProjectItem", "type", "invalid"),
    };
    let Some(content_key) = all(ctx.svc, coll)
        .into_iter()
        .find(|(_, d)| u(d, "id") == id)
        .map(|(k, _)| k.to_owned())
    else {
        return validation("ProjectItem", "id", "invalid");
    };
    let viewer = ctx.viewer();
    let now = ctx.now.clone();
    let (_, doc) = insert_item(ctx.svc, pkey, &content_key, &viewer, &now);
    crate::util::created(item_json(ctx.svc, ctx.base(), pkey, &doc, None))
}

/// Validates and normalizes a field value for storage. Shared with GraphQL.
pub fn normalize_value(f: &Value, value: &Value) -> Result<Value, String> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    match data_type(f).as_str() {
        "TEXT" => Ok(json!(
            value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string())
        )),
        "NUMBER" => value
            .as_f64()
            .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
            .map(|n| json!(n))
            .ok_or_else(|| format!("value for {} must be a number", s(f, "name"))),
        "DATE" => {
            let d = value.as_str().unwrap_or("");
            let d = d.get(..10).unwrap_or(d);
            if unix(d).is_some() {
                Ok(json!(d))
            } else {
                Err(format!(
                    "value for {} must be a date (YYYY-MM-DD)",
                    s(f, "name")
                ))
            }
        }
        "SINGLE_SELECT" => {
            let id = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            option_by_id(f, &id)
                .map(|o| json!(s(o, "id")))
                .ok_or_else(|| format!("option {id} does not exist for field {}", s(f, "name")))
        }
        "ITERATION" => {
            let id = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            iteration_by_id(f, &id)
                .map(|it| json!(s(it, "id")))
                .ok_or_else(|| format!("iteration {id} does not exist for field {}", s(f, "name")))
        }
        other => Err(format!(
            "fields of type {} cannot be updated",
            other.to_ascii_lowercase()
        )),
    }
}

fn update_item(ctx: &mut Ctx, pkey: &str, id: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let key = format!("{pkey}:{id}");
    let Some(mut item) = get(ctx.svc, "project_items", &key).cloned() else {
        return not_found();
    };
    if let Some(archived) = body.get("archived").and_then(Value::as_bool) {
        item["archived_at"] = if archived {
            json!(ctx.now)
        } else {
            Value::Null
        };
    }
    if let Some(fields) = body.get("fields").and_then(Value::as_array) {
        let defs = fields_of(ctx.svc, pkey);
        if !item.get("field_values").is_some_and(Value::is_object) {
            item["field_values"] = json!({});
        }
        for upd in fields {
            let fid = u(upd, "id");
            let Some(f) = defs.iter().find(|d| u(d, "id") == fid) else {
                return error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    &format!("Field {fid} does not exist on this project"),
                );
            };
            match normalize_value(f, upd.get("value").unwrap_or(&Value::Null)) {
                Ok(Value::Null) => {
                    item["field_values"]
                        .as_object_mut()
                        .map(|o| o.remove(&fid.to_string()));
                }
                Ok(v) => item["field_values"][fid.to_string()] = v,
                Err(msg) => return error(StatusCode::UNPROCESSABLE_ENTITY, &msg),
            }
        }
    }
    item["updated_at"] = json!(ctx.now);
    ctx.svc
        .store
        .collection_mut("project_items")
        .insert(key, item.clone());
    let ids: Vec<u64> = body
        .get("fields")
        .and_then(Value::as_array)
        .map(|fs| fs.iter().map(|f| u(f, "id")).collect())
        .unwrap_or_default();
    ok(item_json(ctx.svc, ctx.base(), pkey, &item, Some(&ids)))
}

#[allow(dead_code)]
fn unused(v: &Value) -> (String, Map<String, Value>) {
    (login_of(v), Map::new())
}

#[allow(dead_code)]
fn unused2(svc: &ServiceState) -> Vec<String> {
    render::all(svc, "projects")
        .iter()
        .map(|(k, _)| (*k).to_owned())
        .collect()
}
