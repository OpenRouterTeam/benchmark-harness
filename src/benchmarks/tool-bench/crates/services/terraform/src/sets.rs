//! State versions, variable sets, policy sets and stacks.

use axum::{
    Router,
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::Response,
    routing::get,
};
use mock_core::world::SharedWorld;
use serde_json::{Value, json};

use crate::{
    NAME,
    common::{
        Ctx, Db, attributes, bad_request, body_json, contains_ci, data, data_ids,
        data_with_included, invalid, list_response, new_id, no_content, not_found, opt_str,
        str_field, str_list,
    },
    resources,
};

pub fn routes() -> Router<SharedWorld> {
    Router::new()
        .route("/api/v2/state-versions", get(list_state_versions))
        .route("/api/v2/state-versions/{id}", get(get_state_version))
        .route("/api/v2/state-versions/{id}/outputs", get(list_sv_outputs))
        .route("/api/v2/state-version-outputs/{id}", get(get_sv_output))
        .route(
            "/api/v2/workspaces/{id}/current-state-version",
            get(current_state_version),
        )
        .route(
            "/api/v2/workspaces/{id}/current-state-version-outputs",
            get(current_state_version_outputs),
        )
        .route(
            "/api/v2/organizations/{org}/varsets",
            get(list_varsets).post(create_varset),
        )
        .route(
            "/api/v2/varsets/{id}",
            get(get_varset).patch(update_varset).delete(delete_varset),
        )
        .route(
            "/api/v2/varsets/{id}/relationships/workspaces",
            axum::routing::post(attach_varset_workspaces).delete(detach_varset_workspaces),
        )
        .route(
            "/api/v2/varsets/{id}/relationships/vars",
            get(list_varset_vars).post(create_varset_var),
        )
        .route(
            "/api/v2/varsets/{id}/relationships/vars/{var}",
            get(get_varset_var)
                .patch(update_varset_var)
                .delete(delete_varset_var),
        )
        .route(
            "/api/v2/organizations/{org}/policy-sets",
            get(list_policy_sets),
        )
        .route("/api/v2/policy-sets/{id}", get(get_policy_set))
        .route(
            "/api/v2/policy-sets/{id}/relationships/workspaces",
            axum::routing::post(attach_policy_set_workspaces).delete(detach_policy_set_workspaces),
        )
        .route("/api/v2/organizations/{org}/stacks", get(list_stacks))
        .route("/api/v2/stacks/{id}", get(get_stack))
}

// ---------------------------------------------------------------- state versions

async fn list_state_versions(State(world): State<SharedWorld>, ctx: Ctx) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let (Some(org), Some(ws_name)) = (
        ctx.q.value("filter[organization][name]"),
        ctx.q.value("filter[workspace][name]"),
    ) else {
        return bad_request("filter[organization][name] and filter[workspace][name] are required");
    };
    let Some(ws) = db.workspace_by_name(org, ws_name) else {
        return not_found();
    };
    let ws_id = str_field(ws, "id");
    let mut svs = db.filter("state_versions", |s| str_field(s, "workspace") == ws_id);
    svs.sort_by_key(|s| std::cmp::Reverse(s.get("serial").and_then(Value::as_u64).unwrap_or(0)));
    let items = svs
        .into_iter()
        .map(|s| resources::state_version(&ctx.base, s))
        .collect();
    list_response(&ctx.base, "/api/v2/state-versions", &ctx.q, items, vec![])
}

fn sv_response(db: &Db, sv: &Value, ctx: &Ctx) -> Response {
    let mut included = Vec::new();
    if ctx.q.has_include("outputs") {
        for o in sv
            .get("outputs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            included.push(resources::state_version_output(o));
        }
    }
    if ctx.q.has_include("run")
        && let Some(r) = opt_str(sv, "run").and_then(|r| db.get("runs", r))
    {
        included.push(resources::run(db, r));
    }
    data_with_included(resources::state_version(&ctx.base, sv), included)
}

async fn get_state_version(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    match db.get("state_versions", &id) {
        Some(sv) => sv_response(&db, sv, &ctx),
        None => not_found(),
    }
}

async fn current_state_version(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    match db
        .get("workspaces", &id)
        .and_then(|w| opt_str(w, "current-state-version"))
        .and_then(|s| db.get("state_versions", s))
    {
        Some(sv) => sv_response(&db, sv, &ctx),
        None => not_found(),
    }
}

fn outputs_list(sv: &Value, base: &str, path: &str, ctx: &Ctx) -> Response {
    let items = sv
        .get("outputs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(resources::state_version_output)
        .collect();
    list_response(base, path, &ctx.q, items, vec![])
}

async fn list_sv_outputs(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    match Db::new(&world).get("state_versions", &id) {
        Some(sv) => outputs_list(
            sv,
            &ctx.base,
            &format!("/api/v2/state-versions/{id}/outputs"),
            &ctx,
        ),
        None => not_found(),
    }
}

async fn current_state_version_outputs(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    match db
        .get("workspaces", &id)
        .and_then(|w| opt_str(w, "current-state-version"))
        .and_then(|s| db.get("state_versions", s))
    {
        Some(sv) => outputs_list(
            sv,
            &ctx.base,
            &format!("/api/v2/workspaces/{id}/current-state-version-outputs"),
            &ctx,
        ),
        None => not_found(),
    }
}

async fn get_sv_output(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    for sv in db.all("state_versions") {
        for o in sv
            .get("outputs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if str_field(o, "id") == id {
                return data(StatusCode::OK, resources::state_version_output(o));
            }
        }
    }
    not_found()
}

// ---------------------------------------------------------------- variable sets

async fn list_varsets(
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
    let query = ctx.q.value("q").map(str::to_owned);
    let sets = db.filter("varsets", |v| {
        str_field(v, "organization") == org_name
            && query
                .as_deref()
                .is_none_or(|q| contains_ci(str_field(v, "name"), q))
    });
    let mut included = Vec::new();
    for vs in &sets {
        if ctx.q.has_include("vars") {
            for v in db.filter("varset_vars", |v| {
                str_field(v, "varset") == str_field(vs, "id")
            }) {
                included.push(resources::varset_var(v));
            }
        }
        if ctx.q.has_include("workspaces") {
            for w in str_list(vs, "workspaces") {
                if let Some(ws) = db.get("workspaces", &w) {
                    included.push(resources::workspace(&db, ws));
                }
            }
        }
    }
    let items = sets
        .into_iter()
        .map(|v| resources::varset(&db, v))
        .collect();
    list_response(
        &ctx.base,
        &format!("/api/v2/organizations/{org_name}/varsets"),
        &ctx.q,
        items,
        included,
    )
}

async fn create_varset(
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
    let org_name = match Db::new(world).org(&org) {
        Some(o) => str_field(o, "name").to_owned(),
        None => return not_found(),
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
    let taken = !Db::new(world)
        .filter("varsets", |v| {
            str_field(v, "organization") == org_name && str_field(v, "name") == name
        })
        .is_empty();
    if taken {
        return invalid("Name has already been taken", "name");
    }
    let now = world.clock.now_rfc3339();
    let id = new_id(world, "varsets", "varset", "varset-");
    let vs = json!({
        "id": id,
        "organization": org_name,
        "name": name,
        "description": attrs.get("description").cloned().unwrap_or(json!("")),
        "global": attrs.get("global").cloned().unwrap_or(json!(false)),
        "priority": attrs.get("priority").cloned().unwrap_or(json!(false)),
        "updated-at": now,
        "workspaces": [],
        "projects": [],
    });
    world
        .service_mut(NAME)
        .store
        .collection_mut("varsets")
        .insert(id, vs.clone());
    data(StatusCode::CREATED, resources::varset(&Db::new(world), &vs))
}

async fn get_varset(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    match db.get("varsets", &id) {
        Some(v) => data(StatusCode::OK, resources::varset(&db, v)),
        None => not_found(),
    }
}

async fn update_varset(
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
    let now = world.clock.now_rfc3339();
    let store = &mut world.service_mut(NAME).store;
    let Some(vs) = store.collection_mut("varsets").get_mut(&id) else {
        return not_found();
    };
    for key in ["name", "description", "global", "priority"] {
        if let Some(v) = attrs.get(key) {
            vs[key] = v.clone();
        }
    }
    vs["updated-at"] = json!(now);
    let vs = vs.clone();
    data(StatusCode::OK, resources::varset(&Db::new(world), &vs))
}

async fn delete_varset(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let mut world = world.lock().expect("world lock");
    let store = &mut world.service_mut(NAME).store;
    if store.collection_mut("varsets").remove(&id).is_none() {
        return not_found();
    }
    let vars: Vec<String> = store
        .collection_mut("varset_vars")
        .iter()
        .filter(|(_, v)| str_field(v, "varset") == id)
        .map(|(k, _)| k.to_owned())
        .collect();
    for k in vars {
        store.collection_mut("varset_vars").remove(&k);
    }
    no_content()
}

fn change_links(
    world: &SharedWorld,
    collection: &str,
    id: &str,
    body: &Bytes,
    attach: bool,
    reject_global: bool,
) -> Response {
    let doc = match body_json(body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let ids = data_ids(&doc);
    if ids.is_empty() {
        return invalid("Workspaces can't be blank", "workspaces");
    }
    let mut world = world.lock().expect("world lock");
    {
        let db = Db::new(&world);
        let Some(set) = db.get(collection, id) else {
            return not_found();
        };
        if attach && reject_global && set.get("global").and_then(Value::as_bool) == Some(true) {
            return invalid(
                "Global policy sets cannot be attached to individual workspaces",
                "global",
            );
        }
        let org = str_field(set, "organization");
        for ws in &ids {
            let ok = db
                .get("workspaces", ws)
                .is_some_and(|w| str_field(w, "organization") == org);
            if !ok {
                return not_found();
            }
        }
    }
    let store = &mut world.service_mut(NAME).store;
    let set = store
        .collection_mut(collection)
        .get_mut(id)
        .expect("set exists");
    let mut current = str_list(set, "workspaces");
    if attach {
        for ws in ids {
            if !current.contains(&ws) {
                current.push(ws);
            }
        }
    } else {
        current.retain(|w| !ids.contains(w));
    }
    set["workspaces"] = json!(current);
    no_content()
}

async fn attach_varset_workspaces(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    change_links(&world, "varsets", &id, &body, true, false)
}

async fn detach_varset_workspaces(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    change_links(&world, "varsets", &id, &body, false, false)
}

async fn list_varset_vars(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    if db.get("varsets", &id).is_none() {
        return not_found();
    }
    let items = db
        .filter("varset_vars", |v| str_field(v, "varset") == id)
        .into_iter()
        .map(resources::varset_var)
        .collect();
    list_response(
        &ctx.base,
        &format!("/api/v2/varsets/{id}/relationships/vars"),
        &ctx.q,
        items,
        vec![],
    )
}

async fn create_varset_var(
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
    if Db::new(world).get("varsets", &id).is_none() {
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
    let duplicate = Db::new(world)
        .filter("varset_vars", |v| str_field(v, "varset") == id)
        .iter()
        .any(|v| str_field(v, "key") == key && str_field(v, "category") == category);
    if duplicate {
        return invalid("Key has already been taken", "key");
    }
    let now = world.clock.now_rfc3339();
    let var_id = new_id(world, "varset_vars", "var", "var-");
    let var = json!({
        "id": var_id,
        "varset": id,
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
        .collection_mut("varset_vars")
        .insert(var_id, var.clone());
    data(StatusCode::CREATED, resources::varset_var(&var))
}

async fn get_varset_var(
    State(world): State<SharedWorld>,
    Path((id, var)): Path<(String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    match Db::new(&world)
        .get("varset_vars", &var)
        .filter(|v| str_field(v, "varset") == id)
    {
        Some(v) => data(StatusCode::OK, resources::varset_var(v)),
        None => not_found(),
    }
}

async fn update_varset_var(
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
    if Db::new(&world)
        .get("varset_vars", &var_id)
        .filter(|v| str_field(v, "varset") == id)
        .is_none()
    {
        return not_found();
    }
    let store = &mut world.service_mut(NAME).store;
    let var = store
        .collection_mut("varset_vars")
        .get_mut(&var_id)
        .expect("var");
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
    data(StatusCode::OK, resources::varset_var(&var))
}

async fn delete_varset_var(
    State(world): State<SharedWorld>,
    Path((id, var_id)): Path<(String, String)>,
) -> Response {
    let mut world = world.lock().expect("world lock");
    if Db::new(&world)
        .get("varset_vars", &var_id)
        .filter(|v| str_field(v, "varset") == id)
        .is_none()
    {
        return not_found();
    }
    world
        .service_mut(NAME)
        .store
        .collection_mut("varset_vars")
        .remove(&var_id);
    no_content()
}

// ---------------------------------------------------------------- policy sets

async fn list_policy_sets(
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
    let search = ctx.q.value("search[name]").map(str::to_owned);
    let kind = ctx.q.value("filter[kind]").map(str::to_owned);
    let sets = db.filter("policy_sets", |p| {
        str_field(p, "organization") == org_name
            && search
                .as_deref()
                .is_none_or(|s| contains_ci(str_field(p, "name"), s))
            && kind.as_deref().is_none_or(|k| {
                let actual = opt_str(p, "kind").unwrap_or("sentinel");
                actual == k
            })
    });
    let mut included = Vec::new();
    if ctx.q.has_include("workspaces") {
        for ps in &sets {
            for w in str_list(ps, "workspaces") {
                if let Some(ws) = db.get("workspaces", &w) {
                    included.push(resources::workspace(&db, ws));
                }
            }
        }
    }
    let items = sets.into_iter().map(resources::policy_set).collect();
    list_response(
        &ctx.base,
        &format!("/api/v2/organizations/{org_name}/policy-sets"),
        &ctx.q,
        items,
        included,
    )
}

async fn get_policy_set(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let world = world.lock().expect("world lock");
    match Db::new(&world).get("policy_sets", &id) {
        Some(p) => data(StatusCode::OK, resources::policy_set(p)),
        None => not_found(),
    }
}

async fn attach_policy_set_workspaces(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    change_links(&world, "policy_sets", &id, &body, true, true)
}

async fn detach_policy_set_workspaces(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    change_links(&world, "policy_sets", &id, &body, false, true)
}

// ---------------------------------------------------------------- stacks

async fn list_stacks(
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
    let project = ctx.q.value("filter[project][id]").map(str::to_owned);
    let search = ctx.q.value("search[name]").map(str::to_owned);
    let mut stacks = db.filter("stacks", |s| {
        str_field(s, "organization") == org_name
            && project
                .as_deref()
                .is_none_or(|p| str_field(s, "project") == p)
            && search
                .as_deref()
                .is_none_or(|q| contains_ci(str_field(s, "name"), q))
    });
    match ctx.q.value("sort") {
        Some("name") => stacks.sort_by_key(|s| str_field(s, "name").to_lowercase()),
        Some("-name") => {
            stacks.sort_by_key(|s| std::cmp::Reverse(str_field(s, "name").to_lowercase()))
        }
        _ => {}
    }
    let items = stacks.into_iter().map(resources::stack).collect();
    list_response(
        &ctx.base,
        &format!("/api/v2/organizations/{org_name}/stacks"),
        &ctx.q,
        items,
        vec![],
    )
}

async fn get_stack(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let world = world.lock().expect("world lock");
    match Db::new(&world).get("stacks", &id) {
        Some(s) => data(StatusCode::OK, resources::stack(s)),
        None => not_found(),
    }
}
