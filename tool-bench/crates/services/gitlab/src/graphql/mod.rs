//! GitLab GraphQL API (`POST /api/graphql`) over the same collections as the REST API, plus
//! the REST endpoints of features GitLab models mostly in GraphQL (iterations, dependency
//! proxy purge, Orbit).
//!
//! The executor parses the document, evaluates variables and directives, and resolves fields
//! lazily: objects are JSON values carrying `__typename` (and `_types` for interfaces); fields
//! that take arguments or are expensive are computed by resolvers in [`types`], everything
//! else is read from the object. Unknown fields fail like GitLab's validator.

mod mutations;
mod parser;
mod types;

use axum::{
    Router,
    response::Response,
    routing::{delete, get as rget, post},
};
use mock_core::{
    store::Store,
    world::{SharedWorld, WorldState},
};
use serde_json::{Map, Value, json};

use crate::common::*;
use parser::{Document, Selection, args_map, included};

pub fn routes() -> Router<SharedWorld> {
    Router::new()
        .route("/api/graphql", post(wrap(graphql)).get(wrap(graphql_get)))
        .route(
            "/api/v4/groups/{id}/iterations",
            rget(wrap(group_iterations)),
        )
        .route(
            "/api/v4/projects/{id}/iterations",
            rget(wrap(project_iterations)),
        )
        .route(
            "/api/v4/groups/{id}/dependency_proxy/cache",
            delete(wrap(purge_dependency_proxy)),
        )
        .route("/api/v4/orbit/{*rest}", rget(wrap(orbit)).post(wrap(orbit)))
}

/// Evaluation context for queries (read-only).
pub(crate) struct Ctx<'a> {
    pub st: &'a Store,
    pub base: String,
    pub vars: Map<String, Value>,
    pub doc: &'a Document,
    pub today: String,
}

pub(crate) type R = Result<Value, String>;

fn graphql_get(w: &mut WorldState, r: &Req) -> Response {
    let query = r.q("query").unwrap_or("").to_owned();
    let vars = r
        .q("variables")
        .and_then(|v| serde_json::from_str::<Value>(v).ok())
        .unwrap_or(json!({}));
    ok(execute(w, &r.base, &query, &vars, r.q("operationName")))
}

fn graphql(w: &mut WorldState, r: &Req) -> Response {
    if let Some(batch) = r.body.as_array() {
        let out: Vec<Value> = batch
            .iter()
            .map(|b| {
                execute(
                    w,
                    &r.base,
                    str_of(b, "query"),
                    b.get("variables").unwrap_or(&Value::Null),
                    b.get("operationName").and_then(Value::as_str),
                )
            })
            .collect();
        return ok(Value::Array(out));
    }
    let Some(query) = r.body.get("query").and_then(Value::as_str) else {
        return ok(json!({"errors": [{"message": "No query string was present"}]}));
    };
    let vars = r.body.get("variables").cloned().unwrap_or(Value::Null);
    let vars = match vars {
        Value::String(s) => serde_json::from_str(&s).unwrap_or(json!({})),
        v => v,
    };
    ok(execute(
        w,
        &r.base,
        query,
        &vars,
        r.body.get("operationName").and_then(Value::as_str),
    ))
}

fn err(message: &str) -> Value {
    json!({"errors": [{"message": message}]})
}

/// Executes a GraphQL request and returns the response body.
pub(crate) fn execute(
    w: &mut WorldState,
    base: &str,
    query: &str,
    vars: &Value,
    op_name: Option<&str>,
) -> Value {
    let doc = match parser::parse(query) {
        Ok(d) => d,
        Err(e) => return err(&e),
    };
    let op = match op_name.filter(|n| !n.is_empty()) {
        Some(n) => match doc.operations.iter().find(|o| o.name.as_deref() == Some(n)) {
            Some(o) => o.clone(),
            None => return err(&format!("No operation named \"{n}\"")),
        },
        None => {
            if doc.operations.len() > 1 {
                return err("An operation name is required");
            }
            doc.operations[0].clone()
        }
    };
    let mut vars: Map<String, Value> = vars.as_object().cloned().unwrap_or_default();
    for (name, default) in &op.defaults {
        if !vars.contains_key(name)
            && let Some(d) = default
        {
            vars.insert(name.clone(), parser::eval(d, &Map::new()));
        }
    }
    let today = w.clock.now_rfc3339()[..10].to_owned();
    let result = if op.kind == "mutation" {
        let mut data = Map::new();
        let mut res = Ok(());
        for sel in flatten_root(&doc, &op.selection, &vars) {
            let args = args_map(&sel.args, &vars);
            let payload = match mutations::run(w, base, &sel.name, &args) {
                Ok(p) => p,
                Err(e) => {
                    res = Err(e);
                    break;
                }
            };
            let ctx = Ctx {
                st: store(w),
                base: base.to_owned(),
                vars: vars.clone(),
                doc: &doc,
                today: today.clone(),
            };
            match complete(&ctx, payload, &sel.selection) {
                Ok(v) => {
                    data.insert(sel.key().to_owned(), v);
                }
                Err(e) => {
                    res = Err(e);
                    break;
                }
            }
        }
        res.map(|_| Value::Object(data))
    } else if op.kind == "query" {
        let ctx = Ctx {
            st: store(w),
            base: base.to_owned(),
            vars: vars.clone(),
            doc: &doc,
            today,
        };
        let root = json!({"__typename": "Query"});
        let mut out = Map::new();
        exec_selection(&ctx, &root, &op.selection, &mut out).map(|_| Value::Object(out))
    } else {
        Err("Subscriptions are not supported over HTTP".into())
    };
    match result {
        Ok(data) => json!({"data": data}),
        Err(e) => err(&e),
    }
}

/// Root-level fields of a mutation (fragments flattened).
fn flatten_root(
    doc: &Document,
    sel: &[Selection],
    vars: &Map<String, Value>,
) -> Vec<parser::Field> {
    let mut out = Vec::new();
    for s in sel {
        match s {
            Selection::Field(f) if included(&f.directives, vars) => out.push(f.clone()),
            Selection::Inline {
                directives,
                selection,
                ..
            } if included(directives, vars) => {
                out.extend(flatten_root(doc, selection, vars));
            }
            Selection::Spread { name, directives } if included(directives, vars) => {
                if let Some(fr) = doc.fragments.get(name) {
                    out.extend(flatten_root(doc, &fr.selection, vars));
                }
            }
            _ => {}
        }
    }
    out
}

fn type_matches(obj: &Value, cond: &str) -> bool {
    obj.get("__typename").and_then(Value::as_str) == Some(cond)
        || obj
            .get("_types")
            .and_then(Value::as_array)
            .is_some_and(|ts| ts.iter().any(|t| t == cond))
}

pub(crate) fn exec_selection(
    ctx: &Ctx,
    obj: &Value,
    sel: &[Selection],
    out: &mut Map<String, Value>,
) -> Result<(), String> {
    for s in sel {
        match s {
            Selection::Field(f) => {
                if !included(&f.directives, &ctx.vars) {
                    continue;
                }
                if f.name == "__typename" {
                    out.insert(
                        f.key().to_owned(),
                        obj.get("__typename").cloned().unwrap_or(Value::Null),
                    );
                    continue;
                }
                let args = args_map(&f.args, &ctx.vars);
                let value = types::resolve(ctx, obj, &f.name, &args)?;
                let completed = complete(ctx, value, &f.selection)?;
                match out.get_mut(f.key()) {
                    Some(Value::Object(existing)) if completed.is_object() => {
                        if let Value::Object(more) = completed {
                            for (k, v) in more {
                                existing.insert(k, v);
                            }
                        }
                    }
                    _ => {
                        out.insert(f.key().to_owned(), completed);
                    }
                }
            }
            Selection::Inline {
                type_cond,
                directives,
                selection,
            } => {
                if included(directives, &ctx.vars)
                    && type_cond.as_deref().is_none_or(|c| type_matches(obj, c))
                {
                    exec_selection(ctx, obj, selection, out)?;
                }
            }
            Selection::Spread { name, directives } => {
                if !included(directives, &ctx.vars) {
                    continue;
                }
                let Some(fr) = ctx.doc.fragments.get(name) else {
                    return Err(format!("Fragment {name} was used, but not defined"));
                };
                if type_matches(obj, &fr.type_cond)
                    || fr.type_cond == "Query" && obj["__typename"] == "Query"
                {
                    let selection = fr.selection.clone();
                    exec_selection(ctx, obj, &selection, out)?;
                }
            }
        }
    }
    Ok(())
}

/// Applies a sub-selection to a resolved value.
pub(crate) fn complete(ctx: &Ctx, v: Value, sel: &[Selection]) -> R {
    if sel.is_empty() {
        return Ok(public(v));
    }
    match v {
        Value::Array(items) => items
            .into_iter()
            .map(|i| complete(ctx, i, sel))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(_) => {
            let mut out = Map::new();
            exec_selection(ctx, &v, sel, &mut out)?;
            Ok(Value::Object(out))
        }
        other => Ok(other),
    }
}

/// Strips internal (`_`-prefixed) keys from a leaf JSON value.
fn public(v: Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.into_iter()
                .filter(|(k, _)| !k.starts_with('_'))
                .map(|(k, v)| (k, public(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.into_iter().map(public).collect()),
        other => other,
    }
}

/// Relay-style connection over already-materialized nodes (`first`/`after`/`last`/`before`).
pub(crate) fn connection(type_name: &str, nodes: Vec<Value>, args: &Map<String, Value>) -> Value {
    let total = nodes.len();
    let cursor = |i: usize| b64_encode(format!("{{\"offset\":{i}}}").as_bytes());
    let decode = |c: &Value| -> Option<usize> {
        let raw = String::from_utf8(b64_decode(c.as_str()?)?).ok()?;
        raw.trim_matches(|ch| ch == '{' || ch == '}')
            .split(':')
            .nth(1)?
            .trim()
            .parse()
            .ok()
    };
    let mut start = args
        .get("after")
        .and_then(decode)
        .map(|i| i + 1)
        .unwrap_or(0);
    let mut end = args
        .get("before")
        .and_then(decode)
        .unwrap_or(total)
        .min(total);
    if let Some(first) = args.get("first").and_then(Value::as_u64) {
        end = end.min(start + first as usize);
    }
    if let Some(last) = args.get("last").and_then(Value::as_u64) {
        start = start.max(end.saturating_sub(last as usize));
    }
    let start = start.min(end);
    let slice: Vec<Value> = nodes[start..end].to_vec();
    let edges: Vec<Value> = slice
        .iter()
        .enumerate()
        .map(|(i, n)| json!({"__typename": format!("{}Edge", type_name.trim_end_matches("Connection")), "node": n, "cursor": cursor(start + i)}))
        .collect();
    json!({
        "__typename": type_name,
        "nodes": slice,
        "edges": edges,
        "count": total,
        "pageInfo": {
            "__typename": "PageInfo",
            "hasNextPage": end < total,
            "hasPreviousPage": start > 0,
            "startCursor": if end > start { json!(cursor(start)) } else { Value::Null },
            "endCursor": if end > start { json!(cursor(end - 1)) } else { Value::Null },
        },
    })
}

// ---------------------------------------------------------------------------
// REST: iterations

fn iteration_state(it: &Value, today: &str) -> u64 {
    let start = str_of(it, "start_date");
    let due = str_of(it, "due_date");
    if !due.is_empty() && due < today {
        3
    } else if !start.is_empty() && start <= today {
        2
    } else {
        1
    }
}

pub(crate) fn render_iteration_rest(st: &Store, base: &str, it: &Value, today: &str) -> Value {
    let group = str_of(it, "group");
    json!({
        "id": it["id"],
        "iid": it.get("iid").cloned().unwrap_or(it["id"].clone()),
        "sequence": it.get("sequence").cloned().unwrap_or(it.get("iid").cloned().unwrap_or(json!(1))),
        "group_id": group_id(st, group),
        "title": it.get("title").cloned().unwrap_or(Value::Null),
        "description": it.get("description").cloned().unwrap_or(Value::Null),
        "state": iteration_state(it, today),
        "created_at": it["created_at"],
        "updated_at": it.get("updated_at").unwrap_or(&it["created_at"]),
        "start_date": it.get("start_date").cloned().unwrap_or(Value::Null),
        "due_date": it.get("due_date").cloned().unwrap_or(Value::Null),
        "web_url": format!("{base}/groups/{group}/-/iterations/{}", it["id"]),
    })
}

fn list_iterations(w: &mut WorldState, r: &Req, groups: Vec<String>) -> Response {
    let st = store(w);
    let today = today(w);
    let state = r.q("state").unwrap_or("all");
    let search = r.q("search").map(str::to_lowercase);
    let items = all(st, "iterations")
        .into_iter()
        .filter(|(_, it)| groups.iter().any(|g| g == str_of(it, "group")))
        .filter(|(_, it)| {
            let s = iteration_state(it, &today);
            match state {
                "opened" => s < 3,
                "upcoming" => s == 1,
                "current" => s == 2,
                "closed" => s == 3,
                _ => true,
            }
        })
        .filter(|(_, it)| {
            search.as_ref().is_none_or(|q| {
                let cadence = as_u64(&it["cadence_id"])
                    .and_then(|c| get(st, "iteration_cadences", &c.to_string()))
                    .map(|c| str_of(c, "title").to_lowercase())
                    .unwrap_or_default();
                let fields = r.q("in").unwrap_or("title");
                (fields.contains("title") && str_of(it, "title").to_lowercase().contains(q))
                    || (fields.contains("cadence_title") && cadence.contains(q))
            })
        })
        .filter(|(_, it)| {
            r.q("updated_after")
                .is_none_or(|a| str_of(it, "updated_at") >= a)
        })
        .filter(|(_, it)| {
            r.q("updated_before")
                .is_none_or(|b| str_of(it, "updated_at") <= b)
        })
        .map(|(_, it)| render_iteration_rest(st, &r.base, it, &today))
        .collect();
    paged(r, items)
}

fn ancestors(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut p = parent_path(path);
    while let Some(x) = p {
        out.push(x.to_owned());
        p = parent_path(x);
    }
    out
}

fn group_iterations(w: &mut WorldState, r: &Req) -> Response {
    let g = match require_group(store(w), r) {
        Ok(g) => g,
        Err(e) => return e,
    };
    let mut groups = vec![g.clone()];
    if r.q("include_ancestors") != Some("false") {
        groups.extend(ancestors(&g));
    }
    if r.q("include_descendants") == Some("true") {
        let prefix = format!("{g}/");
        groups.extend(
            all(store(w), "groups")
                .iter()
                .filter(|(k, _)| k.starts_with(&prefix))
                .map(|(k, _)| (*k).to_owned()),
        );
    }
    list_iterations(w, r, groups)
}

fn project_iterations(w: &mut WorldState, r: &Req) -> Response {
    let p = match require_project(store(w), r) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let groups = ancestors(&p);
    list_iterations(w, r, groups)
}

fn purge_dependency_proxy(w: &mut WorldState, r: &Req) -> Response {
    let g = match require_group(store(w), r) {
        Ok(g) => g,
        Err(e) => return e,
    };
    let st = svc_mut(w);
    let ids: Vec<String> = all(st, "dependency_proxy_blobs")
        .iter()
        .filter(|(_, b)| str_of(b, "group") == g)
        .map(|(k, _)| (*k).to_owned())
        .collect();
    for id in ids {
        remove_doc(st, "dependency_proxy_blobs", &id);
    }
    accepted(json!({"message": "202 Accepted"}))
}

fn orbit(_w: &mut WorldState, _r: &Req) -> Response {
    // Orbit (the knowledge graph API) is not available on this instance.
    route_not_found()
}
