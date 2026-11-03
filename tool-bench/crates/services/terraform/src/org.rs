//! Account, organizations, projects, teams, team membership and team access.

use axum::{
    Router,
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::Response,
    routing::{get, post},
};
use mock_core::world::SharedWorld;
use serde_json::{Value, json};

use crate::{
    NAME,
    common::{
        Ctx, Db, attributes, body_json, conflict, contains_ci, data, data_ids, invalid,
        list_response, new_id, no_content, not_found, opt_str, relationship_id, str_field,
        str_list,
    },
    resources,
};

pub fn routes() -> Router<SharedWorld> {
    Router::new()
        .route("/api/v2/account/details", get(account_details))
        .route("/api/v2/organizations", get(list_orgs))
        .route("/api/v2/organizations/{org}", get(get_org))
        .route(
            "/api/v2/organizations/{org}/projects",
            get(list_projects).post(create_project),
        )
        .route(
            "/api/v2/projects/{id}",
            get(get_project)
                .patch(update_project)
                .delete(delete_project),
        )
        .route(
            "/api/v2/organizations/{org}/teams",
            get(list_teams).post(create_team),
        )
        .route(
            "/api/v2/teams/{id}",
            get(get_team).patch(update_team).delete(delete_team),
        )
        .route(
            "/api/v2/teams/{id}/relationships/users",
            post(add_team_users).delete(remove_team_users),
        )
        .route(
            "/api/v2/teams/{id}/relationships/organization-memberships",
            post(add_team_memberships),
        )
        .route(
            "/api/v2/organizations/{org}/organization-memberships",
            get(list_memberships),
        )
        .route(
            "/api/v2/team-workspaces",
            get(list_team_access).post(add_team_access),
        )
        .route(
            "/api/v2/team-workspaces/{id}",
            get(get_team_access)
                .patch(update_team_access)
                .delete(delete_team_access),
        )
        .route(
            "/api/v2/team-projects",
            get(list_team_projects).post(add_team_project),
        )
        .route(
            "/api/v2/team-projects/{id}",
            get(get_team_project)
                .patch(update_team_project)
                .delete(delete_team_project),
        )
}

async fn account_details(State(world): State<SharedWorld>) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    match db.current_user() {
        Some(user) => data(StatusCode::OK, resources::user(user)),
        None => not_found(),
    }
}

async fn list_orgs(State(world): State<SharedWorld>, ctx: Ctx) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let q = ctx.q.value("q").map(str::to_owned);
    let items = db
        .all("organizations")
        .into_iter()
        .filter(|o| {
            q.as_deref()
                .is_none_or(|q| contains_ci(str_field(o, "name"), q))
        })
        .map(|o| resources::organization(&db, o))
        .collect();
    list_response(&ctx.base, "/api/v2/organizations", &ctx.q, items, vec![])
}

async fn get_org(State(world): State<SharedWorld>, Path(org): Path<String>) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    match db.org(&org) {
        Some(o) => data(StatusCode::OK, resources::organization(&db, o)),
        None => not_found(),
    }
}

// ---------------------------------------------------------------- projects

async fn list_projects(
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
    let names = ctx.q.list("filter[names]");
    let query = ctx.q.value("q").map(str::to_owned);
    let mut projects: Vec<&Value> = db.filter("projects", |p| {
        str_field(p, "organization") == org_name
            && (names.is_empty() || names.iter().any(|n| n == str_field(p, "name")))
            && query
                .as_deref()
                .is_none_or(|q| contains_ci(str_field(p, "name"), q))
    });
    projects.sort_by_key(|p| str_field(p, "name").to_lowercase());
    let items = projects
        .into_iter()
        .map(|p| resources::project_with_counts(&db, p))
        .collect();
    list_response(
        &ctx.base,
        &format!("/api/v2/organizations/{org_name}/projects"),
        &ctx.q,
        items,
        vec![],
    )
}

fn valid_project_name(name: &str) -> bool {
    let len = name.chars().count();
    (3..=40).contains(&len)
        && !name.starts_with(' ')
        && !name.ends_with(' ')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_')
}

async fn create_project(
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
    let org_name = {
        let db = Db::new(world);
        let Some(o) = db.org(&org) else {
            return not_found();
        };
        str_field(o, "name").to_owned()
    };
    let name = attrs
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if name.is_empty() {
        return invalid("Name can't be blank", "name");
    }
    if !valid_project_name(name) {
        return invalid(
            "Name must be 3-40 characters and may only contain letters, numbers, spaces, hyphens, and underscores",
            "name",
        );
    }
    let taken = !Db::new(world)
        .filter("projects", |p| {
            str_field(p, "organization") == org_name
                && str_field(p, "name").eq_ignore_ascii_case(name)
        })
        .is_empty();
    if taken {
        return invalid("Name has already been taken", "name");
    }
    if let Some(mode) = attrs.get("default-execution-mode").and_then(Value::as_str)
        && !["remote", "local", "agent"].contains(&mode)
    {
        return invalid(
            "Default execution mode is not included in the list",
            "default-execution-mode",
        );
    }
    let now = world.clock.now_rfc3339();
    let id = new_id(world, "projects", "project", "prj-");
    let project = json!({
        "id": id,
        "organization": org_name,
        "name": name,
        "description": attrs.get("description").cloned().unwrap_or(json!("")),
        "default-execution-mode": attrs.get("default-execution-mode").cloned().unwrap_or(json!("remote")),
        "created-at": now,
        "auto-destroy-activity-duration": attrs.get("auto-destroy-activity-duration").cloned().unwrap_or(Value::Null),
    });
    world
        .service_mut(NAME)
        .store
        .collection_mut("projects")
        .insert(id, project.clone());
    data(StatusCode::CREATED, resources::project(&project))
}

async fn get_project(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    match db.get("projects", &id) {
        Some(p) => data(StatusCode::OK, resources::project_with_counts(&db, p)),
        None => not_found(),
    }
}

async fn update_project(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let doc = match body_json(&body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let attrs = attributes(&doc);
    let mut world = world.lock().expect("world lock");
    let store = &mut world.service_mut(NAME).store;
    let Some(project) = store.collection_mut("projects").get_mut(&id) else {
        return not_found();
    };
    for key in [
        "name",
        "description",
        "default-execution-mode",
        "auto-destroy-activity-duration",
    ] {
        if let Some(v) = attrs.get(key) {
            project[key] = v.clone();
        }
    }
    let project = project.clone();
    data(StatusCode::OK, resources::project(&project))
}

async fn delete_project(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let mut world = world.lock().expect("world lock");
    {
        let db = Db::new(&world);
        if db.get("projects", &id).is_none() {
            return not_found();
        }
        let in_use = !db
            .filter("workspaces", |w| str_field(w, "project") == id)
            .is_empty()
            || !db
                .filter("stacks", |s| str_field(s, "project") == id)
                .is_empty();
        if in_use {
            return conflict(
                "conflict",
                "Project cannot be deleted while it contains workspaces or stacks",
            );
        }
        let is_default = db
            .all("organizations")
            .iter()
            .any(|o| opt_str(o, "default-project") == Some(id.as_str()));
        if is_default {
            return conflict("conflict", "The default project cannot be deleted");
        }
    }
    let store = &mut world.service_mut(NAME).store;
    store.collection_mut("projects").remove(&id);
    let grants: Vec<String> = store
        .collection_mut("team_projects")
        .iter()
        .filter(|(_, t)| str_field(t, "project") == id)
        .map(|(k, _)| k.to_owned())
        .collect();
    for k in grants {
        store.collection_mut("team_projects").remove(&k);
    }
    no_content()
}

// ---------------------------------------------------------------- teams

async fn list_teams(
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
    let names = ctx.q.list("filter[names]");
    let query = ctx.q.value("q").map(str::to_owned);
    let teams: Vec<&Value> = db.filter("teams", |t| {
        str_field(t, "organization") == org_name
            && (names.is_empty() || names.iter().any(|n| n == str_field(t, "name")))
            && query
                .as_deref()
                .is_none_or(|q| contains_ci(str_field(t, "name"), q))
    });
    let mut included = Vec::new();
    if ctx.q.has_include("users") {
        for t in &teams {
            for u in str_list(t, "users") {
                if let Some(user) = db.get("users", &u) {
                    included.push(resources::user(user));
                }
            }
        }
    }
    let items = teams.into_iter().map(resources::team).collect();
    list_response(
        &ctx.base,
        &format!("/api/v2/organizations/{org_name}/teams"),
        &ctx.q,
        items,
        included,
    )
}

async fn create_team(
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
        .trim();
    if name.is_empty() {
        return invalid("Name can't be blank", "name");
    }
    let taken = !Db::new(world)
        .filter("teams", |t| {
            str_field(t, "organization") == org_name
                && str_field(t, "name").eq_ignore_ascii_case(name)
        })
        .is_empty();
    if taken {
        return invalid("Name has already been taken", "name");
    }
    let visibility = attrs
        .get("visibility")
        .and_then(Value::as_str)
        .unwrap_or("secret");
    if !["secret", "organization"].contains(&visibility) {
        return invalid("Visibility is not included in the list", "visibility");
    }
    let id = new_id(world, "teams", "team", "team-");
    let mut team = json!({
        "id": id,
        "organization": org_name,
        "name": name,
        "visibility": visibility,
        "users": [],
        "organization-memberships": [],
    });
    if let Some(access) = attrs.get("organization-access") {
        team["organization-access"] =
            resources::team(&json!({}))["attributes"]["organization-access"].clone();
        if let Some(obj) = access.as_object() {
            for (k, v) in obj {
                team["organization-access"][k] = v.clone();
            }
        }
    }
    if let Some(sso) = attrs.get("sso-team-id") {
        team["sso-team-id"] = sso.clone();
    }
    world
        .service_mut(NAME)
        .store
        .collection_mut("teams")
        .insert(id, team.clone());
    data(StatusCode::CREATED, resources::team(&team))
}

async fn get_team(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let world = world.lock().expect("world lock");
    match Db::new(&world).get("teams", &id) {
        Some(t) => data(StatusCode::OK, resources::team(t)),
        None => not_found(),
    }
}

async fn update_team(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let doc = match body_json(&body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let attrs = attributes(&doc);
    let mut world = world.lock().expect("world lock");
    let store = &mut world.service_mut(NAME).store;
    let Some(team) = store.collection_mut("teams").get_mut(&id) else {
        return not_found();
    };
    for key in [
        "name",
        "visibility",
        "sso-team-id",
        "allow-member-token-management",
    ] {
        if let Some(v) = attrs.get(key) {
            team[key] = v.clone();
        }
    }
    let team = team.clone();
    data(StatusCode::OK, resources::team(&team))
}

async fn delete_team(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let mut world = world.lock().expect("world lock");
    let Some(team) = Db::new(&world).get("teams", &id).cloned() else {
        return not_found();
    };
    if str_field(&team, "name") == "owners" {
        return invalid("The owners team cannot be deleted", "name");
    }
    let store = &mut world.service_mut(NAME).store;
    store.collection_mut("teams").remove(&id);
    for collection in ["team_workspaces", "team_projects"] {
        let keys: Vec<String> = store
            .collection_mut(collection)
            .iter()
            .filter(|(_, g)| str_field(g, "team") == id)
            .map(|(k, _)| k.to_owned())
            .collect();
        for k in keys {
            store.collection_mut(collection).remove(&k);
        }
    }
    no_content()
}

fn push_unique(list: &mut Value, item: &str) {
    if !list.is_array() {
        *list = json!([]);
    }
    let arr = list.as_array_mut().expect("array");
    if !arr.iter().any(|v| v == item) {
        arr.push(json!(item));
    }
}

async fn add_team_users(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let doc = match body_json(&body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let usernames = data_ids(&doc);
    let mut world = world.lock().expect("world lock");
    let (org, resolved) = {
        let db = Db::new(&world);
        let Some(team) = db.get("teams", &id) else {
            return not_found();
        };
        let org = str_field(team, "organization").to_owned();
        let mut resolved = Vec::new();
        for username in &usernames {
            let Some(user) = db
                .all("users")
                .into_iter()
                .find(|u| str_field(u, "username").eq_ignore_ascii_case(username))
            else {
                return not_found();
            };
            let uid = str_field(user, "id").to_owned();
            let Some(membership) = db.all("organization_memberships").into_iter().find(|m| {
                str_field(m, "organization") == org
                    && str_field(m, "user") == uid
                    && str_field(m, "status") == "active"
            }) else {
                return not_found();
            };
            resolved.push((uid, str_field(membership, "id").to_owned()));
        }
        (org, resolved)
    };
    let _ = org;
    let store = &mut world.service_mut(NAME).store;
    let team = store
        .collection_mut("teams")
        .get_mut(&id)
        .expect("team exists");
    for (uid, mid) in resolved {
        push_unique(&mut team["users"], &uid);
        push_unique(&mut team["organization-memberships"], &mid);
    }
    no_content()
}

async fn remove_team_users(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let doc = match body_json(&body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let usernames = data_ids(&doc);
    let mut world = world.lock().expect("world lock");
    let user_ids: Vec<String> = {
        let db = Db::new(&world);
        if db.get("teams", &id).is_none() {
            return not_found();
        }
        db.all("users")
            .into_iter()
            .filter(|u| usernames.iter().any(|n| n == str_field(u, "username")))
            .map(|u| str_field(u, "id").to_owned())
            .collect()
    };
    let store = &mut world.service_mut(NAME).store;
    let team = store.collection_mut("teams").get_mut(&id).expect("team");
    if let Some(users) = team.get_mut("users").and_then(Value::as_array_mut) {
        users.retain(|u| !user_ids.iter().any(|id| u == id.as_str()));
    }
    no_content()
}

async fn add_team_memberships(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let doc = match body_json(&body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let ids = data_ids(&doc);
    let mut world = world.lock().expect("world lock");
    let resolved: Vec<(String, Option<String>)> = {
        let db = Db::new(&world);
        let Some(team) = db.get("teams", &id) else {
            return not_found();
        };
        let org = str_field(team, "organization");
        let mut out = Vec::new();
        for mid in &ids {
            let Some(m) = db.get("organization_memberships", mid) else {
                return not_found();
            };
            if str_field(m, "organization") != org {
                return not_found();
            }
            let user = (str_field(m, "status") == "active")
                .then(|| opt_str(m, "user").map(str::to_owned))
                .flatten();
            out.push((mid.clone(), user));
        }
        out
    };
    let store = &mut world.service_mut(NAME).store;
    let team = store.collection_mut("teams").get_mut(&id).expect("team");
    for (mid, user) in resolved {
        push_unique(&mut team["organization-memberships"], &mid);
        if let Some(uid) = user {
            push_unique(&mut team["users"], &uid);
        }
    }
    no_content()
}

async fn list_memberships(
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
    let mut included = Vec::new();
    let items = db
        .filter("organization_memberships", |m| {
            str_field(m, "organization") == org_name
        })
        .into_iter()
        .map(|m| {
            if let Some(u) = opt_str(m, "user").and_then(|u| db.get("users", u)) {
                included.push(resources::user(u));
            }
            let mid = str_field(m, "id");
            let teams: Vec<&str> = db
                .filter("teams", |t| str_list(t, "organization-memberships").iter().any(|x| x == mid))
                .iter()
                .map(|t| str_field(t, "id"))
                .collect();
            json!({
                "id": mid,
                "type": "organization-memberships",
                "attributes": {
                    "status": str_field(m, "status"),
                    "email": str_field(m, "email"),
                    "created-at": m.get("created-at").cloned().unwrap_or(json!("2024-01-01T00:00:00Z")),
                },
                "relationships": {
                    "organization": { "data": { "id": org_name, "type": "organizations" } },
                    "user": resources_rel_user(opt_str(m, "user")),
                    "teams": { "data": teams.iter().map(|t| json!({ "id": t, "type": "teams" })).collect::<Vec<_>>() },
                }
            })
        })
        .collect();
    list_response(
        &ctx.base,
        &format!("/api/v2/organizations/{org_name}/organization-memberships"),
        &ctx.q,
        items,
        if ctx.q.has_include("user") {
            included
        } else {
            vec![]
        },
    )
}

fn resources_rel_user(id: Option<&str>) -> Value {
    crate::common::rel("users", id)
}

// ---------------------------------------------------------------- team access

async fn list_team_access(State(world): State<SharedWorld>, ctx: Ctx) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let Some(ws) = ctx.q.value("filter[workspace][id]").map(str::to_owned) else {
        return crate::common::bad_request("filter[workspace][id] is required");
    };
    if db.get("workspaces", &ws).is_none() {
        return not_found();
    }
    let items = db
        .filter("team_workspaces", |t| str_field(t, "workspace") == ws)
        .into_iter()
        .map(resources::team_access)
        .collect();
    list_response(&ctx.base, "/api/v2/team-workspaces", &ctx.q, items, vec![])
}

async fn add_team_access(State(world): State<SharedWorld>, body: Bytes) -> Response {
    let doc = match body_json(&body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let attrs = attributes(&doc);
    let team_id = relationship_id(&doc, "team").unwrap_or_default();
    let ws_id = relationship_id(&doc, "workspace").unwrap_or_default();
    let access = attrs
        .get("access")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    {
        let db = Db::new(world);
        if db.get("teams", &team_id).is_none() || db.get("workspaces", &ws_id).is_none() {
            return not_found();
        }
        if !["read", "plan", "write", "admin", "custom"].contains(&access.as_str()) {
            return invalid("Access is not included in the list", "access");
        }
        let exists = db
            .all("team_workspaces")
            .iter()
            .any(|t| str_field(t, "team") == team_id && str_field(t, "workspace") == ws_id);
        if exists {
            return invalid("Team has already been taken", "team");
        }
    }
    let id = new_id(world, "team_workspaces", "team_workspace", "tws-");
    let mut grant = resources::access_permissions(&access);
    grant["id"] = json!(id);
    grant["team"] = json!(team_id);
    grant["workspace"] = json!(ws_id);
    if access == "custom" {
        for key in [
            "runs",
            "variables",
            "state-versions",
            "sentinel-mocks",
            "workspace-locking",
            "run-tasks",
        ] {
            if let Some(v) = attrs.get(key) {
                grant[key] = v.clone();
            }
        }
    }
    world
        .service_mut(NAME)
        .store
        .collection_mut("team_workspaces")
        .insert(id, grant.clone());
    data(StatusCode::OK, resources::team_access(&grant))
}

async fn get_team_access(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let world = world.lock().expect("world lock");
    match Db::new(&world).get("team_workspaces", &id) {
        Some(t) => data(StatusCode::OK, resources::team_access(t)),
        None => not_found(),
    }
}

async fn update_team_access(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let doc = match body_json(&body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let attrs = attributes(&doc);
    let mut world = world.lock().expect("world lock");
    let store = &mut world.service_mut(NAME).store;
    let Some(grant) = store.collection_mut("team_workspaces").get_mut(&id) else {
        return not_found();
    };
    if let Some(access) = attrs.get("access").and_then(Value::as_str) {
        let perms = resources::access_permissions(access);
        for (k, v) in perms.as_object().expect("object") {
            grant[k] = v.clone();
        }
    }
    let grant = grant.clone();
    data(StatusCode::OK, resources::team_access(&grant))
}

async fn delete_team_access(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let mut world = world.lock().expect("world lock");
    match world
        .service_mut(NAME)
        .store
        .collection_mut("team_workspaces")
        .remove(&id)
    {
        Some(_) => no_content(),
        None => not_found(),
    }
}

async fn list_team_projects(State(world): State<SharedWorld>, ctx: Ctx) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let Some(project) = ctx.q.value("filter[project][id]").map(str::to_owned) else {
        return crate::common::bad_request("filter[project][id] is required");
    };
    if db.get("projects", &project).is_none() {
        return not_found();
    }
    let items = db
        .filter("team_projects", |t| str_field(t, "project") == project)
        .into_iter()
        .map(resources::team_project)
        .collect();
    list_response(&ctx.base, "/api/v2/team-projects", &ctx.q, items, vec![])
}

async fn add_team_project(State(world): State<SharedWorld>, body: Bytes) -> Response {
    let doc = match body_json(&body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let attrs = attributes(&doc);
    let team_id = relationship_id(&doc, "team").unwrap_or_default();
    let project_id = relationship_id(&doc, "project").unwrap_or_default();
    let access = attrs
        .get("access")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    {
        let db = Db::new(world);
        if db.get("teams", &team_id).is_none() || db.get("projects", &project_id).is_none() {
            return not_found();
        }
        if !["read", "write", "maintain", "admin", "custom"].contains(&access.as_str()) {
            return invalid("Access is not included in the list", "access");
        }
        let exists = db
            .all("team_projects")
            .iter()
            .any(|t| str_field(t, "team") == team_id && str_field(t, "project") == project_id);
        if exists {
            return invalid("Team has already been taken", "team");
        }
    }
    let id = new_id(world, "team_projects", "team_project", "tprj-");
    let grant = json!({ "id": id, "team": team_id, "project": project_id, "access": access });
    world
        .service_mut(NAME)
        .store
        .collection_mut("team_projects")
        .insert(id, grant.clone());
    data(StatusCode::CREATED, resources::team_project(&grant))
}

async fn get_team_project(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let world = world.lock().expect("world lock");
    match Db::new(&world).get("team_projects", &id) {
        Some(t) => data(StatusCode::OK, resources::team_project(t)),
        None => not_found(),
    }
}

async fn update_team_project(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let doc = match body_json(&body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let attrs = attributes(&doc);
    let mut world = world.lock().expect("world lock");
    let store = &mut world.service_mut(NAME).store;
    let Some(grant) = store.collection_mut("team_projects").get_mut(&id) else {
        return not_found();
    };
    if let Some(access) = attrs.get("access") {
        grant["access"] = access.clone();
    }
    let grant = grant.clone();
    data(StatusCode::OK, resources::team_project(&grant))
}

async fn delete_team_project(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let mut world = world.lock().expect("world lock");
    match world
        .service_mut(NAME)
        .store
        .collection_mut("team_projects")
        .remove(&id)
    {
        Some(_) => no_content(),
        None => not_found(),
    }
}
