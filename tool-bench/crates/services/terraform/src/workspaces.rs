//! Workspaces: CRUD, locking, safe delete, readme, tags and tag bindings, and workspace vars.

use axum::{
    Router,
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::Response,
    routing::{get, post},
};
use mock_core::world::{SharedWorld, WorldState};
use serde_json::{Map, Value, json};

use crate::{
    NAME,
    common::{
        Ctx, Db, attributes, body_json, conflict, contains_ci, data, data_with_included, dedup,
        invalid, list_response, new_id, no_content, not_found, opt_str, relationship_id, str_field,
        str_list,
    },
    resources,
};

pub fn routes() -> Router<SharedWorld> {
    Router::new()
        .route(
            "/api/v2/organizations/{org}/workspaces",
            get(list_workspaces).post(create_workspace),
        )
        .route(
            "/api/v2/organizations/{org}/workspaces/{name}",
            get(get_workspace_by_name)
                .patch(update_workspace_by_name)
                .delete(delete_workspace_by_name),
        )
        .route(
            "/api/v2/organizations/{org}/workspaces/{name}/actions/safe-delete",
            post(safe_delete_by_name),
        )
        .route(
            "/api/v2/workspaces/{id}",
            get(get_workspace)
                .patch(update_workspace)
                .delete(delete_workspace),
        )
        .route(
            "/api/v2/workspaces/{id}/actions/{action}",
            post(workspace_action),
        )
        .route(
            "/api/v2/workspaces/{id}/tag-bindings",
            get(list_tag_bindings).patch(add_tag_bindings),
        )
        .route(
            "/api/v2/workspaces/{id}/effective-tag-bindings",
            get(list_effective_tag_bindings),
        )
        .route(
            "/api/v2/workspaces/{id}/relationships/tags",
            get(list_tags).post(add_tags).delete(remove_tags),
        )
        .route(
            "/api/v2/workspaces/{id}/vars",
            get(list_vars).post(create_var),
        )
        .route("/api/v2/workspaces/{id}/all-vars", get(list_all_vars))
        .route(
            "/api/v2/workspaces/{id}/vars/{var}",
            get(get_var).patch(update_var).delete(delete_var),
        )
}

fn wildcard_match(pattern: &str, name: &str) -> bool {
    let pattern = pattern.to_lowercase();
    let name = name.to_lowercase();
    if !pattern.contains('*') {
        return name.contains(&pattern);
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut rest = name.as_str();
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            if !rest.starts_with(part) {
                return false;
            }
            rest = &rest[part.len()..];
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else if let Some(pos) = rest.find(part) {
            rest = &rest[pos + part.len()..];
        } else {
            return false;
        }
    }
    true
}

fn tag_set(ws: &Value) -> Vec<String> {
    let mut tags = str_list(ws, "tag-names");
    for b in ws
        .get("tag-bindings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let key = str_field(b, "key");
        let value = str_field(b, "value");
        if value.is_empty() {
            tags.push(key.to_owned());
        } else {
            tags.push(format!("{key}:{value}"));
        }
    }
    tags
}

async fn list_workspaces(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let Some(o) = db.org(&org) else {
        return not_found();
    };
    let org_name = str_field(o, "name");
    let q = &ctx.q;
    let search = q.value("search[name]").map(str::to_lowercase);
    let wildcard = q.value("search[wildcard-name]").map(str::to_owned);
    let tags = q.list("search[tags]");
    let exclude = q.list("search[exclude-tags]");
    let project = q.value("filter[project][id]").map(str::to_owned);
    let run_status = q.list("filter[current-run][status]");
    let mut tagged = Vec::new();
    for i in 0.. {
        let Some(key) = q.get(&format!("filter[tagged][{i}][key]")) else {
            break;
        };
        let value = q.get(&format!("filter[tagged][{i}][value]")).unwrap_or("");
        tagged.push((key.to_owned(), value.to_owned()));
    }
    let mut workspaces: Vec<&Value> = db.filter("workspaces", |w| {
        if str_field(w, "organization") != org_name {
            return false;
        }
        let name = str_field(w, "name");
        let ws_tags = tag_set(w);
        search
            .as_deref()
            .is_none_or(|s| name.to_lowercase().contains(s))
            && wildcard.as_deref().is_none_or(|p| wildcard_match(p, name))
            && tags.iter().all(|t| ws_tags.iter().any(|x| x == t))
            && !exclude.iter().any(|t| ws_tags.iter().any(|x| x == t))
            && project
                .as_deref()
                .is_none_or(|p| str_field(w, "project") == p)
            && (run_status.is_empty()
                || opt_str(w, "current-run")
                    .and_then(|r| db.get("runs", r))
                    .is_some_and(|r| run_status.iter().any(|s| s == str_field(r, "status"))))
            && tagged.iter().all(|(k, v)| {
                w.get("tag-bindings")
                    .and_then(Value::as_array)
                    .is_some_and(|b| {
                        b.iter().any(|b| {
                            str_field(b, "key") == k && (v.is_empty() || str_field(b, "value") == v)
                        })
                    })
            })
    });
    match q.value("sort") {
        Some("-name") => {
            workspaces.sort_by_key(|w| std::cmp::Reverse(str_field(w, "name").to_lowercase()))
        }
        _ => workspaces.sort_by_key(|w| str_field(w, "name").to_lowercase()),
    }
    let mut included = Vec::new();
    for w in &workspaces {
        if q.has_include("project")
            && let Some(p) = opt_str(w, "project").and_then(|p| db.get("projects", p))
        {
            included.push(resources::project(p));
        }
        if q.has_include("current_run")
            && let Some(r) = opt_str(w, "current-run").and_then(|r| db.get("runs", r))
        {
            included.push(resources::run(&db, r));
        }
    }
    let items = workspaces
        .into_iter()
        .map(|w| resources::workspace(&db, w))
        .collect();
    list_response(
        &ctx.base,
        &format!("/api/v2/organizations/{org_name}/workspaces"),
        q,
        items,
        included,
    )
}

fn workspace_response(world: &WorldState, ws: &Value, ctx: &Ctx, status: StatusCode) -> Response {
    let db = Db::new(world);
    let mut included = Vec::new();
    if ctx.q.has_include("readme")
        && let Some(r) = resources::readme(ws)
    {
        included.push(r);
    }
    if ctx.q.has_include("project")
        && let Some(p) = opt_str(ws, "project").and_then(|p| db.get("projects", p))
    {
        included.push(resources::project(p));
    }
    if ctx.q.has_include("organization")
        && let Some(o) = db.org(str_field(ws, "organization"))
    {
        included.push(resources::organization(&db, o));
    }
    if ctx.q.has_include("outputs")
        && let Some(sv) =
            opt_str(ws, "current-state-version").and_then(|s| db.get("state_versions", s))
    {
        for o in sv
            .get("outputs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            included.push(resources::workspace_output(o));
        }
    }
    let resource = resources::workspace(&db, ws);
    if status == StatusCode::OK {
        data_with_included(resource, dedup(included))
    } else {
        data(status, resource)
    }
}

async fn get_workspace_by_name(
    State(world): State<SharedWorld>,
    Path((org, name)): Path<(String, String)>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    match db.workspace_by_name(&org, &name) {
        Some(ws) => workspace_response(&world, ws, &ctx, StatusCode::OK),
        None => not_found(),
    }
}

async fn get_workspace(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    match db.get("workspaces", &id) {
        Some(ws) => workspace_response(&world, ws, &ctx, StatusCode::OK),
        None => not_found(),
    }
}

fn valid_workspace_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 90
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn vcs_repo(input: &Value) -> Value {
    let identifier = str_field(input, "identifier");
    json!({
        "branch": input.get("branch").cloned().unwrap_or(json!("")),
        "ingress-submodules": input.get("ingress-submodules").cloned().unwrap_or(json!(false)),
        "identifier": identifier,
        "display-identifier": identifier,
        "oauth-token-id": input.get("oauth-token-id").cloned().unwrap_or(Value::Null),
        "webhook-url": format!("https://app.terraform.io/webhooks/vcs/{}", crate::common::hex_digest(identifier)),
        "repository-http-url": format!("https://github.com/{identifier}"),
        "service-provider": "github",
        "tags-regex": null,
        "tags": false,
    })
}

const SETTABLE: &[&str] = &[
    "description",
    "auto-apply",
    "auto-apply-run-trigger",
    "allow-destroy-plan",
    "assessments-enabled",
    "execution-mode",
    "file-triggers-enabled",
    "global-remote-state",
    "project-remote-state",
    "queue-all-runs",
    "speculative-enabled",
    "structured-run-output-enabled",
    "terraform-version",
    "trigger-prefixes",
    "trigger-patterns",
    "working-directory",
    "source-name",
    "source-url",
    "auto-destroy-at",
    "auto-destroy-activity-duration",
];

fn check_execution_mode(attrs: &Map<String, Value>) -> Option<Response> {
    let mode = attrs.get("execution-mode").and_then(Value::as_str)?;
    (!["remote", "local", "agent"].contains(&mode)).then(|| {
        invalid(
            "Execution mode is not included in the list",
            "execution-mode",
        )
    })
}

async fn create_workspace(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    body: Bytes,
) -> Response {
    let doc = match body_json(&body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let attrs = attributes(&doc);
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    let (org_name, default_project) = {
        let db = Db::new(world);
        let Some(o) = db.org(&org) else {
            return not_found();
        };
        let name = str_field(o, "name").to_owned();
        let default_project = opt_str(o, "default-project")
            .map(str::to_owned)
            .or_else(|| {
                db.all("projects")
                    .into_iter()
                    .find(|p| str_field(p, "organization") == name)
                    .map(|p| str_field(p, "id").to_owned())
            });
        (name, default_project)
    };
    let name = attrs
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    if name.is_empty() {
        return invalid("Name can't be blank", "name");
    }
    if !valid_workspace_name(&name) {
        return invalid(
            "Name is invalid. It can only include letters, numbers, -, and _.",
            "name",
        );
    }
    if Db::new(world).workspace_by_name(&org_name, &name).is_some() {
        return invalid("Name has already been taken", "name");
    }
    if let Some(resp) = check_execution_mode(&attrs) {
        return resp;
    }
    let project = match relationship_id(&doc, "project") {
        Some(p) => {
            let ok = Db::new(world)
                .get("projects", &p)
                .is_some_and(|proj| str_field(proj, "organization") == org_name);
            if !ok {
                return not_found();
            }
            Some(p)
        }
        None => default_project,
    };
    let now = world.clock.now_rfc3339();
    let id = new_id(world, "workspaces", "workspace", "ws-");
    let mut ws = json!({
        "id": id,
        "name": name,
        "organization": org_name,
        "project": project,
        "created-at": now,
        "updated-at": now,
        "locked": false,
        "auto-apply": false,
        "execution-mode": "remote",
        "terraform-version": "1.9.5",
        "working-directory": "",
        "description": "",
        "resource-count": 0,
        "source": if attrs.contains_key("source-name") { "tfe-api" } else { "tfe-ui" },
        "tag-names": [],
        "tag-bindings": [],
    });
    for key in SETTABLE {
        if let Some(v) = attrs.get(*key) {
            ws[*key] = v.clone();
        }
    }
    if let Some(repo) = attrs.get("vcs-repo").filter(|v| v.is_object()) {
        ws["vcs-repo"] = vcs_repo(repo);
    }
    let tag_names: Vec<String> = doc
        .pointer("/data/relationships/tags/data")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(|t| t.pointer("/attributes/name").and_then(Value::as_str))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    ws["tag-names"] = json!(tag_names);
    let bindings: Vec<Value> = doc
        .pointer("/data/relationships/tag-bindings/data")
        .and_then(Value::as_array)
        .map(|b| {
            b.iter()
                .filter_map(|b| b.get("attributes"))
                .map(|a| json!({ "key": str_field(a, "key"), "value": str_field(a, "value") }))
                .collect()
        })
        .unwrap_or_default();
    ws["tag-bindings"] = json!(bindings);
    world
        .service_mut(NAME)
        .store
        .collection_mut("workspaces")
        .insert(id.clone(), ws.clone());
    data(
        StatusCode::CREATED,
        resources::workspace(&Db::new(world), &ws),
    )
}

fn update_workspace_doc(world: &mut WorldState, id: &str, doc: &Value) -> Response {
    let attrs = attributes(doc);
    if let Some(resp) = check_execution_mode(&attrs) {
        return resp;
    }
    let (org, current_name) = match Db::new(world).get("workspaces", id) {
        Some(ws) => (
            str_field(ws, "organization").to_owned(),
            str_field(ws, "name").to_owned(),
        ),
        None => return not_found(),
    };
    if let Some(new_name) = attrs.get("name").and_then(Value::as_str) {
        if !valid_workspace_name(new_name) {
            return invalid(
                "Name is invalid. It can only include letters, numbers, -, and _.",
                "name",
            );
        }
        if !new_name.eq_ignore_ascii_case(&current_name)
            && Db::new(world).workspace_by_name(&org, new_name).is_some()
        {
            return invalid("Name has already been taken", "name");
        }
    }
    let project = relationship_id(doc, "project");
    if let Some(p) = &project
        && !Db::new(world)
            .get("projects", p)
            .is_some_and(|proj| str_field(proj, "organization") == org)
    {
        return not_found();
    }
    let now = world.clock.now_rfc3339();
    let store = &mut world.service_mut(NAME).store;
    let ws = store
        .collection_mut("workspaces")
        .get_mut(id)
        .expect("workspace exists");
    for key in SETTABLE.iter().chain(&["name"]) {
        if let Some(v) = attrs.get(*key) {
            ws[*key] = v.clone();
        }
    }
    if let Some(repo) = attrs.get("vcs-repo") {
        ws["vcs-repo"] = if repo.is_object() {
            vcs_repo(repo)
        } else {
            Value::Null
        };
    }
    if let Some(p) = project {
        ws["project"] = json!(p);
    }
    ws["updated-at"] = json!(now);
    let ws = ws.clone();
    data(StatusCode::OK, resources::workspace(&Db::new(world), &ws))
}

async fn update_workspace_by_name(
    State(world): State<SharedWorld>,
    Path((org, name)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let doc = match body_json(&body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let mut world = world.lock().expect("world lock");
    let Some(id) = Db::new(&world)
        .workspace_by_name(&org, &name)
        .map(|w| str_field(w, "id").to_owned())
    else {
        return not_found();
    };
    update_workspace_doc(&mut world, &id, &doc)
}

async fn update_workspace(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let doc = match body_json(&body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let mut world = world.lock().expect("world lock");
    update_workspace_doc(&mut world, &id, &doc)
}

/// Removes a workspace and everything scoped to it (vars, team access, varset/policy-set links).
fn remove_workspace(world: &mut WorldState, id: &str) {
    let store = &mut world.service_mut(NAME).store;
    store.collection_mut("workspaces").remove(id);
    for collection in ["vars", "team_workspaces"] {
        let keys: Vec<String> = store
            .collection_mut(collection)
            .iter()
            .filter(|(_, d)| str_field(d, "workspace") == id)
            .map(|(k, _)| k.to_owned())
            .collect();
        for k in keys {
            store.collection_mut(collection).remove(&k);
        }
    }
    for collection in ["varsets", "policy_sets"] {
        let keys: Vec<String> = store
            .collection_mut(collection)
            .iter()
            .map(|(k, _)| k.to_owned())
            .collect();
        for k in keys {
            if let Some(doc) = store.collection_mut(collection).get_mut(&k)
                && let Some(list) = doc.get_mut("workspaces").and_then(Value::as_array_mut)
            {
                list.retain(|w| w != id);
            }
        }
    }
}

async fn delete_workspace_by_name(
    State(world): State<SharedWorld>,
    Path((org, name)): Path<(String, String)>,
) -> Response {
    let mut world = world.lock().expect("world lock");
    let Some(id) = Db::new(&world)
        .workspace_by_name(&org, &name)
        .map(|w| str_field(w, "id").to_owned())
    else {
        return not_found();
    };
    remove_workspace(&mut world, &id);
    no_content()
}

async fn delete_workspace(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let mut world = world.lock().expect("world lock");
    if Db::new(&world).get("workspaces", &id).is_none() {
        return not_found();
    }
    remove_workspace(&mut world, &id);
    no_content()
}

fn safe_delete(world: &mut WorldState, id: &str) -> Response {
    let Some(ws) = Db::new(world).get("workspaces", id).cloned() else {
        return not_found();
    };
    if ws.get("locked").and_then(Value::as_bool) == Some(true) {
        return conflict(
            "conflict",
            "Workspace is currently locked and cannot be deleted. Unlock the workspace and try again.",
        );
    }
    let resources = ws
        .get("resource-count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if resources > 0 {
        return conflict(
            "conflict",
            &format!(
                "Workspace is not safe to delete. It still manages {resources} resources. Destroy the resources or use force delete."
            ),
        );
    }
    remove_workspace(world, id);
    no_content()
}

async fn safe_delete_by_name(
    State(world): State<SharedWorld>,
    Path((org, name)): Path<(String, String)>,
) -> Response {
    let mut world = world.lock().expect("world lock");
    let Some(id) = Db::new(&world)
        .workspace_by_name(&org, &name)
        .map(|w| str_field(w, "id").to_owned())
    else {
        return not_found();
    };
    safe_delete(&mut world, &id)
}

async fn workspace_action(
    State(world): State<SharedWorld>,
    Path((id, action)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let doc = match body_json(&body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    if action == "safe-delete" {
        return safe_delete(world, &id);
    }
    let Some(ws) = Db::new(world).get("workspaces", &id).cloned() else {
        return not_found();
    };
    let locked = ws.get("locked").and_then(Value::as_bool) == Some(true);
    let current_user = Db::new(world)
        .current_user()
        .map(|u| str_field(u, "id").to_owned());
    let locked_by = ws.get("locked-by").cloned().unwrap_or(Value::Null);
    let now = world.clock.now_rfc3339();
    let new_state = match action.as_str() {
        "lock" => {
            if locked {
                return conflict(
                    "conflict",
                    "Unable to lock workspace. The workspace is already locked.",
                );
            }
            let reason = attributes(&doc)
                .get("reason")
                .cloned()
                .or_else(|| doc.get("reason").cloned())
                .unwrap_or(json!(""));
            Some((true, json!({ "type": "users", "id": current_user }), reason))
        }
        "unlock" => {
            if !locked {
                return conflict(
                    "conflict",
                    "Unable to unlock workspace. The workspace is not locked.",
                );
            }
            let kind = str_field(&locked_by, "type");
            let by = str_field(&locked_by, "id");
            if kind == "runs" {
                return conflict(
                    "conflict",
                    &format!("Unable to unlock workspace. The workspace is locked by Run {by}."),
                );
            }
            if kind == "teams" {
                return conflict(
                    "conflict",
                    &format!("Unable to unlock workspace. The workspace is locked by Team {by}."),
                );
            }
            if kind == "users" && Some(by) != current_user.as_deref() {
                let username = Db::new(world)
                    .get("users", by)
                    .map(|u| str_field(u, "username").to_owned())
                    .unwrap_or_else(|| by.to_owned());
                return conflict(
                    "conflict",
                    &format!(
                        "Unable to unlock workspace. The workspace is locked by User {username}."
                    ),
                );
            }
            Some((false, Value::Null, json!("")))
        }
        "force-unlock" => {
            if !locked {
                return conflict(
                    "conflict",
                    "Unable to unlock workspace. The workspace is not locked.",
                );
            }
            Some((false, Value::Null, json!("")))
        }
        _ => None,
    };
    let Some((lock, by, reason)) = new_state else {
        return not_found();
    };
    let store = &mut world.service_mut(NAME).store;
    let ws = store
        .collection_mut("workspaces")
        .get_mut(&id)
        .expect("workspace exists");
    ws["locked"] = json!(lock);
    ws["locked-by"] = by;
    ws["locked-reason"] = reason;
    ws["updated-at"] = json!(now);
    let ws = ws.clone();
    data(StatusCode::OK, resources::workspace(&Db::new(world), &ws))
}

// ---------------------------------------------------------------- tags

fn binding_resource(ws_id: &str, index: usize, b: &Value) -> Value {
    json!({
        "id": format!("tb-{}-{index}", ws_id.trim_start_matches("ws-")),
        "type": "tag-bindings",
        "attributes": { "key": str_field(b, "key"), "value": str_field(b, "value") }
    })
}

fn bindings_of(ws: &Value) -> Vec<Value> {
    let id = str_field(ws, "id");
    ws.get("tag-bindings")
        .and_then(Value::as_array)
        .map(|b| {
            b.iter()
                .enumerate()
                .map(|(i, b)| binding_resource(id, i, b))
                .collect()
        })
        .unwrap_or_default()
}

async fn list_tag_bindings(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    let Some(ws) = Db::new(&world).get("workspaces", &id) else {
        return not_found();
    };
    list_response(
        &ctx.base,
        &format!("/api/v2/workspaces/{id}/tag-bindings"),
        &ctx.q,
        bindings_of(ws),
        vec![],
    )
}

async fn list_effective_tag_bindings(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let Some(ws) = db.get("workspaces", &id) else {
        return not_found();
    };
    let mut items: Vec<Value> = bindings_of(ws)
        .into_iter()
        .map(|mut b| {
            b["type"] = json!("effective-tag-bindings");
            b
        })
        .collect();
    if let Some(project) = opt_str(ws, "project").and_then(|p| db.get("projects", p)) {
        for (i, b) in project
            .get("tag-bindings")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            items.push(json!({
                "id": format!("etb-{}-{i}", str_field(project, "id").trim_start_matches("prj-")),
                "type": "effective-tag-bindings",
                "attributes": { "key": str_field(b, "key"), "value": str_field(b, "value") },
                "links": { "inherited-from": format!("/api/v2/projects/{}", str_field(project, "id")) }
            }));
        }
    }
    list_response(
        &ctx.base,
        &format!("/api/v2/workspaces/{id}/effective-tag-bindings"),
        &ctx.q,
        items,
        vec![],
    )
}

async fn add_tag_bindings(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    ctx: Ctx,
    body: Bytes,
) -> Response {
    let doc = match body_json(&body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let incoming: Vec<(String, String)> = doc
        .get("data")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|i| i.get("attributes"))
                .map(|a| {
                    (
                        str_field(a, "key").to_owned(),
                        str_field(a, "value").to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    if incoming.iter().any(|(k, _)| k.is_empty()) {
        return invalid("Key can't be blank", "key");
    }
    let mut world = world.lock().expect("world lock");
    let now = world.clock.now_rfc3339();
    let store = &mut world.service_mut(NAME).store;
    let Some(ws) = store.collection_mut("workspaces").get_mut(&id) else {
        return not_found();
    };
    if !ws.get("tag-bindings").is_some_and(Value::is_array) {
        ws["tag-bindings"] = json!([]);
    }
    let bindings = ws["tag-bindings"].as_array_mut().expect("array");
    for (key, value) in incoming {
        if let Some(existing) = bindings.iter_mut().find(|b| str_field(b, "key") == key) {
            existing["value"] = json!(value);
        } else {
            bindings.push(json!({ "key": key, "value": value }));
        }
    }
    ws["updated-at"] = json!(now);
    let items = bindings_of(ws);
    list_response(
        &ctx.base,
        &format!("/api/v2/workspaces/{id}/tag-bindings"),
        &ctx.q,
        items,
        vec![],
    )
}

fn tag_id(name: &str) -> String {
    format!("tag-{}", &crate::common::hex_digest(name)[..16])
}

async fn list_tags(State(world): State<SharedWorld>, Path(id): Path<String>, ctx: Ctx) -> Response {
    let world = world.lock().expect("world lock");
    let Some(ws) = Db::new(&world).get("workspaces", &id) else {
        return not_found();
    };
    let filter = ctx.q.value("name").map(str::to_owned);
    let items = str_list(ws, "tag-names")
        .into_iter()
        .filter(|t| filter.as_deref().is_none_or(|f| contains_ci(t, f)))
        .map(|t| json!({ "id": tag_id(&t), "type": "tags", "attributes": { "name": t } }))
        .collect();
    list_response(
        &ctx.base,
        &format!("/api/v2/workspaces/{id}/relationships/tags"),
        &ctx.q,
        items,
        vec![],
    )
}

fn tag_names_from_body(doc: &Value) -> Vec<String> {
    doc.get("data")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|i| {
                    i.pointer("/attributes/name")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn add_tags(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let doc = match body_json(&body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let names = tag_names_from_body(&doc);
    let mut world = world.lock().expect("world lock");
    let store = &mut world.service_mut(NAME).store;
    let Some(ws) = store.collection_mut("workspaces").get_mut(&id) else {
        return not_found();
    };
    let mut tags = str_list(ws, "tag-names");
    for n in names {
        if !tags.contains(&n) {
            tags.push(n);
        }
    }
    ws["tag-names"] = json!(tags);
    no_content()
}

async fn remove_tags(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let doc = match body_json(&body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let names = tag_names_from_body(&doc);
    let ids = crate::common::data_ids(&doc);
    let mut world = world.lock().expect("world lock");
    let store = &mut world.service_mut(NAME).store;
    let Some(ws) = store.collection_mut("workspaces").get_mut(&id) else {
        return not_found();
    };
    let tags: Vec<String> = str_list(ws, "tag-names")
        .into_iter()
        .filter(|t| !names.contains(t) && !ids.contains(&tag_id(t)))
        .collect();
    ws["tag-names"] = json!(tags);
    no_content()
}

// ---------------------------------------------------------------- variables

fn workspace_vars<'a>(db: &Db<'a>, ws: &str) -> Vec<&'a Value> {
    db.filter("vars", |v| str_field(v, "workspace") == ws)
}

async fn list_vars(State(world): State<SharedWorld>, Path(id): Path<String>, ctx: Ctx) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    if db.get("workspaces", &id).is_none() {
        return not_found();
    }
    let items = workspace_vars(&db, &id)
        .into_iter()
        .map(resources::variable)
        .collect();
    list_response(
        &ctx.base,
        &format!("/api/v2/workspaces/{id}/vars"),
        &ctx.q,
        items,
        vec![],
    )
}

/// Workspace vars plus vars inherited from variable sets (workspace values win).
async fn list_all_vars(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let Some(ws) = db.get("workspaces", &id) else {
        return not_found();
    };
    let mut items: Vec<Value> = workspace_vars(&db, &id)
        .into_iter()
        .map(resources::variable)
        .collect();
    let project = str_field(ws, "project");
    let org = str_field(ws, "organization");
    for vs in db.filter("varsets", |vs| {
        str_field(vs, "organization") == org
            && (vs.get("global").and_then(Value::as_bool) == Some(true)
                || str_list(vs, "workspaces").iter().any(|w| w == &id)
                || str_list(vs, "projects").iter().any(|p| p == project))
    }) {
        for v in db.filter("varset_vars", |v| {
            str_field(v, "varset") == str_field(vs, "id")
        }) {
            let shadowed = items.iter().any(|i| {
                i["attributes"]["key"] == v["key"] && i["attributes"]["category"] == v["category"]
            });
            if !shadowed {
                items.push(resources::varset_var(v));
            }
        }
    }
    list_response(
        &ctx.base,
        &format!("/api/v2/workspaces/{id}/all-vars"),
        &ctx.q,
        items,
        vec![],
    )
}

async fn create_var(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let doc = match body_json(&body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let attrs = attributes(&doc);
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    if Db::new(world).get("workspaces", &id).is_none() {
        return not_found();
    }
    let key = attrs
        .get("key")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let category = attrs
        .get("category")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if key.is_empty() {
        return invalid("Key can't be blank", "key");
    }
    if !["terraform", "env"].contains(&category.as_str()) {
        return invalid("Category is not included in the list", "category");
    }
    let duplicate = workspace_vars(&Db::new(world), &id)
        .iter()
        .any(|v| str_field(v, "key") == key && str_field(v, "category") == category);
    if duplicate {
        return invalid("Key has already been taken", "key");
    }
    let now = world.clock.now_rfc3339();
    let var_id = new_id(world, "vars", "var", "var-");
    let var = json!({
        "id": var_id,
        "workspace": id,
        "key": key,
        "value": attrs.get("value").cloned().unwrap_or(json!("")),
        "description": attrs.get("description").cloned().unwrap_or(Value::Null),
        "category": category,
        "hcl": attrs.get("hcl").cloned().unwrap_or(json!(false)),
        "sensitive": attrs.get("sensitive").cloned().unwrap_or(json!(false)),
        "created-at": now,
    });
    world
        .service_mut(NAME)
        .store
        .collection_mut("vars")
        .insert(var_id, var.clone());
    data(StatusCode::CREATED, resources::variable(&var))
}

async fn get_var(
    State(world): State<SharedWorld>,
    Path((id, var)): Path<(String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    match Db::new(&world)
        .get("vars", &var)
        .filter(|v| str_field(v, "workspace") == id)
    {
        Some(v) => data(StatusCode::OK, resources::variable(v)),
        None => not_found(),
    }
}

async fn update_var(
    State(world): State<SharedWorld>,
    Path((id, var_id)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let doc = match body_json(&body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let attrs = attributes(&doc);
    let mut world = world.lock().expect("world lock");
    let Some(existing) = Db::new(&world)
        .get("vars", &var_id)
        .filter(|v| str_field(v, "workspace") == id)
        .cloned()
    else {
        return not_found();
    };
    if let Some(key) = attrs.get("key").and_then(Value::as_str) {
        if key.is_empty() {
            return invalid("Key can't be blank", "key");
        }
        let category = attrs
            .get("category")
            .and_then(Value::as_str)
            .unwrap_or(str_field(&existing, "category"));
        let clash = workspace_vars(&Db::new(&world), &id).iter().any(|v| {
            str_field(v, "id") != var_id
                && str_field(v, "key") == key
                && str_field(v, "category") == category
        });
        if clash {
            return invalid("Key has already been taken", "key");
        }
    }
    if existing.get("sensitive").and_then(Value::as_bool) == Some(true)
        && attrs.get("sensitive").and_then(Value::as_bool) == Some(false)
    {
        return invalid(
            "Sensitive variables cannot be made non-sensitive",
            "sensitive",
        );
    }
    let store = &mut world.service_mut(NAME).store;
    let var = store.collection_mut("vars").get_mut(&var_id).expect("var");
    for key in [
        "key",
        "value",
        "description",
        "category",
        "hcl",
        "sensitive",
    ] {
        if let Some(v) = attrs.get(key) {
            var[key] = v.clone();
        }
    }
    let var = var.clone();
    data(StatusCode::OK, resources::variable(&var))
}

async fn delete_var(
    State(world): State<SharedWorld>,
    Path((id, var_id)): Path<(String, String)>,
) -> Response {
    let mut world = world.lock().expect("world lock");
    if Db::new(&world)
        .get("vars", &var_id)
        .filter(|v| str_field(v, "workspace") == id)
        .is_none()
    {
        return not_found();
    }
    world
        .service_mut(NAME)
        .store
        .collection_mut("vars")
        .remove(&var_id);
    no_content()
}
