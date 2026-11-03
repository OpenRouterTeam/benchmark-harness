//! Instance, users, groups, namespaces, projects, members, events, search, todos.

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
use serde_json::{Map, Value, json};

use crate::common::*;

pub fn routes() -> Router<SharedWorld> {
    Router::new()
        .route("/version", rget(wrap(version)))
        .route("/metadata", rget(wrap(version)))
        .route("/user", rget(wrap(current_user)))
        .route("/job", rget(wrap(job_token)))
        .route("/users", rget(wrap(list_users)))
        .route("/users/{user_id}", rget(wrap(get_user)))
        .route(
            "/projects",
            rget(wrap(list_projects)).post(wrap(create_project)),
        )
        .route(
            "/projects/{id}",
            rget(wrap(get_project))
                .put(wrap(update_project))
                .delete(wrap(delete_project)),
        )
        .route("/projects/{id}/fork", post(wrap(fork_project)))
        .route("/projects/{id}/members", rget(wrap(project_members)))
        .route(
            "/projects/{id}/members/all",
            rget(wrap(project_members_all)),
        )
        .route("/projects/{id}/events", rget(wrap(project_events)))
        .route("/projects/{id}/search", rget(wrap(project_search)))
        .route("/groups", rget(wrap(list_groups)).post(wrap(create_group)))
        .route("/groups/{id}", rget(wrap(get_group)))
        .route("/groups/{id}/projects", rget(wrap(group_projects)))
        .route("/groups/{id}/subgroups", rget(wrap(group_subgroups)))
        .route("/groups/{id}/members", rget(wrap(group_members)))
        .route("/groups/{id}/members/all", rget(wrap(group_members_all)))
        .route("/groups/{id}/search", rget(wrap(group_search)))
        .route("/namespaces", rget(wrap(list_namespaces)))
        .route("/namespaces/{id}", rget(wrap(get_namespace)))
        .route("/namespaces/{id}/exists", rget(wrap(namespace_exists)))
        .route("/events", rget(wrap(user_events)))
        .route("/users/{user_id}/events", rget(wrap(user_events_by_id)))
        .route("/search", rget(wrap(global_search)))
        .route("/todos", rget(wrap(list_todos)))
        .route("/todos/mark_as_done", post(wrap(mark_all_todos_done)))
        .route("/todos/{todo_id}/mark_as_done", post(wrap(mark_todo_done)))
}

// ---------------------------------------------------------------------------
// Instance and users

fn version(w: &mut WorldState, _r: &Req) -> Response {
    let st = store(w);
    let version = instance_field(st, "version").unwrap_or(DEFAULT_VERSION);
    let revision = instance_field(st, "revision").unwrap_or("6b9e5d1c3a2");
    ok(json!({
        "version": version,
        "revision": revision,
        "kas": {"enabled": false, "externalUrl": null, "externalK8sProxyUrl": null, "version": null},
        "enterprise": version.ends_with("-ee"),
    }))
}

fn current_user(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let username = current_username(st);
    if user(st, &username).is_none() {
        return error(StatusCode::UNAUTHORIZED, "401 Unauthorized");
    }
    ok(user_self(st, &r.base, &username))
}

/// `GET /user` shape: the full user plus private fields.
pub(crate) fn user_self(st: &Store, base: &str, username: &str) -> Value {
    let mut out = user_full(st, base, username);
    let u = user(st, username).cloned().unwrap_or(json!({}));
    let created = out["created_at"].clone();
    merge_into(
        &mut out,
        json!({
            "last_sign_in_at": created,
            "confirmed_at": created,
            "last_activity_on": "2026-03-13",
            "email": u.get("email").cloned().unwrap_or(json!(format!("{username}@example.com"))),
            "theme_id": 1,
            "color_scheme_id": 1,
            "projects_limit": 100000,
            "current_sign_in_at": created,
            "identities": [],
            "can_create_group": true,
            "can_create_project": true,
            "two_factor_enabled": true,
            "external": false,
            "private_profile": false,
            "commit_email": u.get("email").cloned().unwrap_or(json!(format!("{username}@example.com"))),
            "is_admin": u.get("is_admin").cloned().unwrap_or(json!(false)),
            "note": null,
            "namespace_id": as_u64(&u["id"]).unwrap_or(0) + 100_000,
            "created_by": null,
            "email_reset_offered_at": null,
            "using_license_seat": true,
        }),
    );
    out
}

fn job_token(_w: &mut WorldState, _r: &Req) -> Response {
    error(StatusCode::UNAUTHORIZED, "401 Unauthorized")
}

fn list_users(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let username = r.q("username");
    let search = r.q("search").map(str::to_lowercase);
    let items: Vec<Value> = all(st, "users")
        .into_iter()
        .filter(|(k, u)| {
            username.is_none_or(|n| k.eq_ignore_ascii_case(n))
                && search.as_ref().is_none_or(|s| {
                    k.to_lowercase().contains(s)
                        || str_of(u, "name").to_lowercase().contains(s)
                        || str_of(u, "email").to_lowercase() == *s
                })
                && (r.q("active") != Some("true")
                    || u.get("state").and_then(Value::as_str).unwrap_or("active") == "active")
                && (r.q("blocked") != Some("true") || str_of(u, "state") == "blocked")
        })
        .map(|(k, _)| user_basic(st, &r.base, k))
        .collect();
    paged(r, items)
}

fn get_user(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    match resolve_username(st, r.p("user_id")) {
        Some(u) => ok(user_full(st, &r.base, &u)),
        None => not_found("User"),
    }
}

// ---------------------------------------------------------------------------
// Projects

fn project_matches(st: &Store, r: &Req, key: &str, p: &Value) -> bool {
    let current = current_username(st);
    if let Some(s) = r.q("search").map(str::to_lowercase) {
        let in_ns = r.q("search_namespaces") == Some("true");
        let name = p
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_else(|| last_segment(key))
            .to_lowercase();
        let desc = str_of(p, "description").to_lowercase();
        let hay_path = if in_ns {
            key.to_lowercase()
        } else {
            last_segment(key).to_lowercase()
        };
        if !(name.contains(&s) || hay_path.contains(&s) || desc.contains(&s)) {
            return false;
        }
    }
    if let Some(v) = r.q("visibility")
        && p.get("visibility")
            .and_then(Value::as_str)
            .unwrap_or("private")
            != v
    {
        return false;
    }
    if let Some(a) = r.q("archived") {
        let archived = p.get("archived").and_then(Value::as_bool).unwrap_or(false);
        if (a == "true") != archived {
            return false;
        }
    }
    if r.q("owned") == Some("true") {
        let owner_ns = parent_path(key) == Some(current.as_str());
        if !owner_ns && member_access(st, key, &current).unwrap_or(0) < 50 {
            return false;
        }
    }
    if r.q("membership") == Some("true") && member_access(st, key, &current).is_none() {
        return false;
    }
    if let Some(min) = r.q("min_access_level").and_then(|m| m.parse::<u64>().ok())
        && member_access(st, key, &current).unwrap_or(0) < min
    {
        return false;
    }
    if r.q("starred") == Some("true") && !p.get("starred").and_then(Value::as_bool).unwrap_or(false)
    {
        return false;
    }
    if let Some(topic) = r.q("topic") {
        let topics = value_list(p.get("topics").unwrap_or(&Value::Null));
        if !topic.split(',').all(|t| topics.iter().any(|x| x == t)) {
            return false;
        }
    }
    true
}

fn sort_projects(st: &Store, r: &Req, keys: &mut [String]) {
    let order_by = r.q("order_by").unwrap_or("created_at");
    let asc = r.q("sort") == Some("asc");
    keys.sort_by(|a, b| {
        let pa = get(st, "projects", a).cloned().unwrap_or(json!({}));
        let pb = get(st, "projects", b).cloned().unwrap_or(json!({}));
        let ord = match order_by {
            "id" => as_u64(&pa["id"]).cmp(&as_u64(&pb["id"])),
            "name" => a.to_lowercase().cmp(&b.to_lowercase()),
            "path" => last_segment(a).cmp(last_segment(b)),
            "last_activity_at" | "updated_at" => str_of(&pa, "last_activity_at")
                .cmp(str_of(&pb, "last_activity_at"))
                .then(as_u64(&pa["id"]).cmp(&as_u64(&pb["id"]))),
            "star_count" => as_u64(&pa["star_count"]).cmp(&as_u64(&pb["star_count"])),
            _ => str_of(&pa, "created_at")
                .cmp(str_of(&pb, "created_at"))
                .then(as_u64(&pa["id"]).cmp(&as_u64(&pb["id"]))),
        };
        if asc { ord } else { ord.reverse() }
    });
}

fn render_project_for(st: &Store, r: &Req, key: &str) -> Value {
    if r.q("simple") == Some("true") {
        render_project_simple(st, &r.base, key)
    } else {
        let mut p = render_project(st, &r.base, key);
        if r.q("statistics") == Some("true") {
            p["statistics"] = project_statistics(st, key);
        }
        p
    }
}

fn project_statistics(st: &Store, key: &str) -> Value {
    let prefix = format!("{key}@");
    let commits = all(st, "commits")
        .iter()
        .filter(|(k, _)| k.starts_with(&prefix))
        .count();
    json!({
        "commit_count": commits,
        "storage_size": 1_843_200,
        "repository_size": 1_536_000,
        "wiki_size": 0,
        "lfs_objects_size": 0,
        "job_artifacts_size": 307_200,
        "pipeline_artifacts_size": 0,
        "packages_size": 0,
        "snippets_size": 0,
        "uploads_size": 0,
        "container_registry_size": 0,
    })
}

fn list_projects(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let mut keys: Vec<String> = all(st, "projects")
        .into_iter()
        .filter(|(k, p)| project_matches(st, r, k, p))
        .map(|(k, _)| k.to_owned())
        .collect();
    sort_projects(st, r, &mut keys);
    let items = keys.iter().map(|k| render_project_for(st, r, k)).collect();
    paged(r, items)
}

fn get_project(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    match require_project(st, r) {
        Ok(key) => {
            let mut p = render_project(st, &r.base, &key);
            if r.q("statistics") == Some("true") {
                p["statistics"] = project_statistics(st, &key);
            }
            if r.q("license") == Some("true") {
                p["license_url"] = Value::Null;
                p["license"] = Value::Null;
            }
            ok(p)
        }
        Err(e) => e,
    }
}

const PROJECT_UPDATABLE: &[&str] = &[
    "name",
    "description",
    "default_branch",
    "visibility",
    "topics",
    "archived",
    "merge_method",
    "squash_option",
    "only_allow_merge_if_pipeline_succeeds",
    "only_allow_merge_if_all_discussions_are_resolved",
    "issues_enabled",
    "wiki_enabled",
    "ci_config_path",
    "approvals_before_merge",
];

fn update_project(w: &mut WorldState, r: &Req) -> Response {
    let key = match require_project(store(w), r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    if let Some(v) = r.s("visibility")
        && !["private", "internal", "public"].contains(&v.as_str())
    {
        return bad_request("visibility does not have a valid value");
    }
    let now = now(w);
    let st = svc_mut(w);
    let doc = st
        .collection_mut("projects")
        .get_mut(&key)
        .expect("project");
    for field in PROJECT_UPDATABLE {
        if let Some(v) = r.body.get(*field).filter(|v| !v.is_null()) {
            let v = if *field == "topics" {
                Value::Array(value_list(v).into_iter().map(Value::String).collect())
            } else {
                v.clone()
            };
            doc[*field] = v;
        }
    }
    if let Some(tags) = r.body.get("tag_list") {
        doc["topics"] = Value::Array(value_list(tags).into_iter().map(Value::String).collect());
    }
    doc["last_activity_at"] = json!(now);
    ok(render_project(st, &r.base, &key))
}

fn delete_project(w: &mut WorldState, r: &Req) -> Response {
    let key = match require_project(store(w), r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let st = svc_mut(w);
    if let Some(p) = st.collection_mut("projects").get_mut(&key) {
        p["marked_for_deletion_on"] = json!("2026-04-14");
    }
    accepted(json!({ "message": "202 Accepted" }))
}

fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-' {
            out.push(c.to_ascii_lowercase());
        } else if c.is_whitespace() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out
}

/// Inserts a new project doc and its default branch; returns the key.
fn insert_project(
    w: &mut WorldState,
    ns_path: &str,
    path: &str,
    fields: Map<String, Value>,
    readme: bool,
) -> String {
    let now = now(w);
    let st = svc_mut(w);
    let id = next_id(st, "projects", 1);
    let key = format!("{ns_path}/{path}");
    let mut doc = json!({
        "id": id,
        "name": path,
        "path_with_namespace": key,
        "description": "",
        "default_branch": "main",
        "visibility": "private",
        "created_at": now,
        "last_activity_at": now,
        "topics": [],
        "archived": false,
        "creator_id": user(st, &current_username(st)).and_then(|u| as_u64(&u["id"])).unwrap_or(1),
    });
    for (k, v) in fields {
        doc[k] = v;
    }
    let branch = str_of(&doc, "default_branch").to_owned();
    st.collection_mut("projects").insert(key.clone(), doc);
    if readme {
        let username = current_username(st);
        let (name, email) = user(st, &username)
            .map(|u| (str_of(u, "name").to_owned(), str_of(u, "email").to_owned()))
            .unwrap_or_default();
        let sha = fake_sha(&format!("{key}:initial"));
        st.collection_mut("commits").insert(
            format!("{key}@{sha}"),
            json!({
                "id": sha, "title": "Initial commit", "message": "Initial commit",
                "author_name": name, "author_email": email, "authored_date": now,
                "committer_name": name, "committer_email": email, "committed_date": now,
                "parent_ids": [], "branch": branch,
            }),
        );
        st.collection_mut("branches").insert(
            format!("{key}:{branch}"),
            json!({ "name": branch, "commit": sha, "base": null }),
        );
        st.collection_mut("files").insert(
            format!("{key}:{branch}:README.md"),
            json!({ "content": format!("# {path}\n") }),
        );
    }
    key
}

fn create_project(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let name = r.s("name");
    let path = r.s("path").or_else(|| name.as_deref().map(slugify));
    let (Some(path), name) = (path, name) else {
        return bad_request("name, path are missing, at least one parameter must be provided");
    };
    let ns_path = match r.s("namespace_id") {
        Some(ns) => match group_key(st, &ns) {
            Some(g) => g,
            None => match resolve_username(st, &ns).or_else(|| {
                ns.parse::<u64>()
                    .ok()
                    .and_then(|n| user_by_id(st, &(n.saturating_sub(100_000)).to_string()))
                    .map(|(k, _)| k.to_owned())
            }) {
                Some(u) => u,
                None => return not_found("Namespace"),
            },
        },
        None => current_username(st),
    };
    let key = format!("{ns_path}/{path}");
    if get(st, "projects", &key).is_some() {
        return json_status(
            StatusCode::BAD_REQUEST,
            json!({"message": {
                "project_namespace.name": ["has already been taken"],
                "name": ["has already been taken"],
                "path": ["has already been taken"],
            }}),
        );
    }
    let mut fields = Map::new();
    fields.insert("name".into(), json!(name.unwrap_or_else(|| path.clone())));
    for f in ["description", "visibility", "default_branch", "topics"] {
        if let Some(v) = r.body.get(f).filter(|v| !v.is_null()) {
            fields.insert(f.into(), v.clone());
        }
    }
    let readme = r.bool("initialize_with_readme").unwrap_or(false);
    let key = insert_project(w, &ns_path, &path, fields, readme);
    let st = store(w);
    let mut p = render_project(st, &r.base, &key);
    p["empty_repo"] = json!(!readme);
    created(p)
}

fn fork_project(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let src = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let ns_path = match r
        .s("namespace_path")
        .or_else(|| r.s("namespace_id"))
        .or_else(|| r.s("namespace"))
    {
        Some(ns) => match group_key(st, &ns).or_else(|| resolve_username(st, &ns)) {
            Some(n) => n,
            None => return not_found("Target Namespace"),
        },
        None => current_username(st),
    };
    let path = r.s("path").unwrap_or_else(|| last_segment(&src).to_owned());
    let key = format!("{ns_path}/{path}");
    if get(st, "projects", &key).is_some() {
        return json_status(
            StatusCode::CONFLICT,
            json!({"message": ["Project namespace name has already been taken", "Name has already been taken", "Path has already been taken"]}),
        );
    }
    let srcdoc = get(st, "projects", &src).cloned().unwrap_or(json!({}));
    let mut fields = Map::new();
    for f in ["description", "default_branch", "topics"] {
        if let Some(v) = srcdoc.get(f) {
            fields.insert(f.into(), v.clone());
        }
    }
    fields.insert(
        "name".into(),
        json!(r.s("name").unwrap_or_else(|| path.clone())),
    );
    fields.insert("forked_from".into(), json!(src));
    if let Some(v) = r.s("visibility") {
        fields.insert("visibility".into(), json!(v));
    } else if let Some(v) = srcdoc.get("visibility") {
        fields.insert("visibility".into(), v.clone());
    }
    let key = insert_project(w, &ns_path, &path, fields, false);
    // The fork shares the source's branches (as overlays on the source's files).
    let st = svc_mut(w);
    let branches: Vec<(String, Value)> = all(st, "branches")
        .into_iter()
        .filter(|(k, _)| k.starts_with(&format!("{src}:")))
        .map(|(k, v)| (k[src.len() + 1..].to_owned(), v.clone()))
        .collect();
    let files: Vec<(String, Value)> = all(st, "files")
        .into_iter()
        .filter(|(k, _)| k.starts_with(&format!("{src}:")))
        .map(|(k, v)| (k[src.len() + 1..].to_owned(), v.clone()))
        .collect();
    let commits: Vec<(String, Value)> = all(st, "commits")
        .into_iter()
        .filter(|(k, _)| k.starts_with(&format!("{src}@")))
        .map(|(k, v)| (k[src.len() + 1..].to_owned(), v.clone()))
        .collect();
    for (rest, v) in branches {
        st.collection_mut("branches")
            .insert(format!("{key}:{rest}"), v);
    }
    for (rest, v) in files {
        st.collection_mut("files")
            .insert(format!("{key}:{rest}"), v);
    }
    for (rest, v) in commits {
        st.collection_mut("commits")
            .insert(format!("{key}@{rest}"), v);
    }
    if let Some(p) = st.collection_mut("projects").get_mut(&src) {
        p["forks_count"] = json!(as_u64(&p["forks_count"]).unwrap_or(0) + 1);
    }
    created(render_project(st, &r.base, &key))
}

// ---------------------------------------------------------------------------
// Members

fn member_json(st: &Store, base: &str, username: &str, level: u64, m: &Value) -> Value {
    let mut out = user_basic(st, base, username);
    merge_into(
        &mut out,
        json!({
            "access_level": level,
            "access_level_description": access_level_description(level),
            "created_at": m.get("created_at").cloned().unwrap_or(json!("2024-01-15T10:00:00.000Z")),
            "created_by": null,
            "expires_at": m.get("expires_at").cloned().unwrap_or(Value::Null),
            "group_saml_identity": null,
            "membership_state": "active",
            "member_role": null,
        }),
    );
    out
}

fn members_of(doc: Option<&Value>) -> Vec<Value> {
    doc.and_then(|d| d.get("members")?.as_array().cloned())
        .unwrap_or_default()
}

fn filter_members(r: &Req, st: &Store, items: Vec<Value>) -> Vec<Value> {
    let query = r.q("query").map(str::to_lowercase);
    let ids = r.list("user_ids").unwrap_or_default();
    let skip = r.list("skip_users").unwrap_or_default();
    let _ = st;
    items
        .into_iter()
        .filter(|m| {
            let id = as_u64(&m["id"]).unwrap_or(0).to_string();
            query.as_ref().is_none_or(|q| {
                str_of(m, "username").to_lowercase().contains(q)
                    || str_of(m, "name").to_lowercase().contains(q)
            }) && (ids.is_empty() || ids.contains(&id))
                && !skip.contains(&id)
        })
        .collect()
}

fn project_members(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let mut items: Vec<Value> = members_of(get(st, "projects", &key))
        .iter()
        .map(|m| {
            member_json(
                st,
                &r.base,
                str_of(m, "username"),
                as_u64(&m["access_level"]).unwrap_or(30),
                m,
            )
        })
        .collect();
    // A project in a user namespace has its owner as a member.
    if get(st, "groups", parent_path(&key).unwrap_or("")).is_none()
        && let Some(owner) = parent_path(&key)
        && !items.iter().any(|m| str_of(m, "username") == owner)
        && user(st, owner).is_some()
    {
        items.insert(0, member_json(st, &r.base, owner, 50, &json!({})));
    }
    let items = filter_members(r, st, items);
    paged(r, items)
}

fn project_members_all(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let mut seen: Vec<String> = Vec::new();
    let mut items = Vec::new();
    let mut push = |m: &Value, items: &mut Vec<Value>| {
        let u = str_of(m, "username").to_owned();
        if !seen.contains(&u) {
            items.push(member_json(
                st,
                &r.base,
                &u,
                as_u64(&m["access_level"]).unwrap_or(30),
                m,
            ));
            seen.push(u);
        }
    };
    for m in members_of(get(st, "projects", &key)) {
        push(&m, &mut items);
    }
    let mut path = parent_path(&key);
    while let Some(p) = path {
        for m in members_of(get(st, "groups", p)) {
            push(&m, &mut items);
        }
        path = parent_path(p);
    }
    if get(st, "groups", parent_path(&key).unwrap_or("")).is_none()
        && let Some(owner) = parent_path(&key).filter(|o| user(st, o).is_some())
    {
        push(&json!({"username": owner, "access_level": 50}), &mut items);
    }
    let items = filter_members(r, st, items);
    paged(r, items)
}

fn group_members(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_group(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let items = members_of(get(st, "groups", &key))
        .iter()
        .map(|m| {
            member_json(
                st,
                &r.base,
                str_of(m, "username"),
                as_u64(&m["access_level"]).unwrap_or(30),
                m,
            )
        })
        .collect();
    let items = filter_members(r, st, items);
    paged(r, items)
}

fn group_members_all(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_group(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let mut seen: Vec<String> = Vec::new();
    let mut items = Vec::new();
    let mut path = Some(key.as_str());
    while let Some(p) = path {
        for m in members_of(get(st, "groups", p)) {
            let u = str_of(&m, "username").to_owned();
            if !seen.contains(&u) {
                items.push(member_json(
                    st,
                    &r.base,
                    &u,
                    as_u64(&m["access_level"]).unwrap_or(30),
                    &m,
                ));
                seen.push(u);
            }
        }
        path = parent_path(p);
    }
    let items = filter_members(r, st, items);
    paged(r, items)
}

// ---------------------------------------------------------------------------
// Groups and namespaces

fn list_groups(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let search = r.q("search").map(str::to_lowercase);
    let current = current_username(st);
    let items: Vec<Value> = all(st, "groups")
        .into_iter()
        .filter(|(k, g)| {
            search.as_ref().is_none_or(|s| {
                k.to_lowercase().contains(s) || str_of(g, "name").to_lowercase().contains(s)
            }) && (r.q("owned") != Some("true")
                || members_of(Some(g)).iter().any(|m| {
                    str_of(m, "username") == current && as_u64(&m["access_level"]) == Some(50)
                }))
                && (r.q("top_level_only") != Some("true") || !k.contains('/'))
        })
        .map(|(k, _)| render_group(st, &r.base, k))
        .collect();
    paged(r, items)
}

fn create_group(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let Some(name) = r.s("name") else {
        return missing("name");
    };
    let Some(path) = r.s("path") else {
        return missing("path");
    };
    let parent = match r.s("parent_id") {
        Some(pid) => match group_key(st, &pid) {
            Some(p) => Some(p),
            None => return not_found("Parent Group"),
        },
        None => None,
    };
    let key = match &parent {
        Some(p) => format!("{p}/{path}"),
        None => path.clone(),
    };
    if get(st, "groups", &key).is_some() || (parent.is_none() && user(st, &key).is_some()) {
        return json_status(
            StatusCode::BAD_REQUEST,
            json!({"message": "Failed to save group {:path=>[\"has already been taken\"]}"}),
        );
    }
    let now = now(w);
    let st = svc_mut(w);
    let id = next_id(st, "groups", 1);
    let current = current_username(st);
    st.collection_mut("groups").insert(
        key.clone(),
        json!({
            "id": id,
            "name": name,
            "full_path": key,
            "description": r.s("description").unwrap_or_default(),
            "visibility": r.s("visibility").unwrap_or_else(|| "private".into()),
            "created_at": now,
            "members": [{"username": current, "access_level": 50}],
        }),
    );
    created(render_group(st, &r.base, &key))
}

fn get_group(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    match require_group(st, r) {
        Ok(key) => {
            let mut g = render_group(st, &r.base, &key);
            if r.q("with_projects") != Some("false") {
                let projects: Vec<Value> = all(st, "projects")
                    .into_iter()
                    .filter(|(k, _)| parent_path(k) == Some(key.as_str()))
                    .map(|(k, _)| render_project_simple(st, &r.base, k))
                    .collect();
                g["projects"] = Value::Array(projects);
                g["shared_projects"] = json!([]);
            }
            ok(g)
        }
        Err(e) => e,
    }
}

fn group_projects(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_group(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let subgroups = r.q("include_subgroups") == Some("true");
    let prefix = format!("{key}/");
    let mut keys: Vec<String> = all(st, "projects")
        .into_iter()
        .filter(|(k, p)| {
            (parent_path(k) == Some(key.as_str()) || (subgroups && k.starts_with(&prefix)))
                && project_matches(st, r, k, p)
        })
        .map(|(k, _)| k.to_owned())
        .collect();
    sort_projects(st, r, &mut keys);
    let items = keys.iter().map(|k| render_project_for(st, r, k)).collect();
    paged(r, items)
}

fn group_subgroups(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_group(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let items = all(st, "groups")
        .into_iter()
        .filter(|(k, _)| parent_path(k) == Some(key.as_str()))
        .map(|(k, _)| render_group(st, &r.base, k))
        .collect();
    paged(r, items)
}

fn namespace_full(st: &Store, base: &str, path: &str) -> Value {
    let mut ns = namespace(st, base, path);
    let projects = all(st, "projects")
        .iter()
        .filter(|(k, _)| parent_path(k) == Some(path))
        .count();
    let members = if get(st, "groups", path).is_some() {
        members_of(get(st, "groups", path)).len().max(1)
    } else {
        1
    };
    merge_into(
        &mut ns,
        json!({
            "members_count_with_descendants": members,
            "billable_members_count": members,
            "root_repository_size": 0,
            "projects_count": projects,
            "plan": "ultimate",
            "end_date": null,
            "trial_ends_on": null,
            "trial": false,
        }),
    );
    ns
}

fn list_namespaces(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let current = current_username(st);
    let search = r.q("search").map(str::to_lowercase);
    let mut paths: Vec<String> = vec![current.clone()];
    for (k, g) in all(st, "groups") {
        let member = {
            let mut p = Some(k);
            let mut found = false;
            while let Some(path) = p {
                if members_of(get(st, "groups", path))
                    .iter()
                    .any(|m| str_of(m, "username") == current)
                {
                    found = true;
                    break;
                }
                p = parent_path(path);
            }
            found
        };
        let owned = members_of(Some(g))
            .iter()
            .any(|m| str_of(m, "username") == current && as_u64(&m["access_level"]) == Some(50));
        if member && (r.q("owned") != Some("true") || owned) {
            paths.push(k.to_owned());
        }
    }
    let items = paths
        .iter()
        .filter(|p| {
            search.as_ref().is_none_or(|s| {
                p.to_lowercase().contains(s)
                    || str_of(&namespace(st, &r.base, p), "name")
                        .to_lowercase()
                        .contains(s)
            })
        })
        .map(|p| namespace_full(st, &r.base, p))
        .collect();
    paged(r, items)
}

fn namespace_key(st: &Store, id: &str) -> Option<String> {
    if let Some(g) = group_key(st, id) {
        return Some(g);
    }
    if user(st, id).is_some() {
        return Some(id.to_owned());
    }
    let n: u64 = id.parse().ok()?;
    user_by_id(st, &n.checked_sub(100_000)?.to_string()).map(|(k, _)| k.to_owned())
}

fn get_namespace(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    match namespace_key(st, r.p("id")) {
        Some(k) => ok(namespace_full(st, &r.base, &k)),
        None => not_found("Namespace"),
    }
}

fn namespace_exists(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let path = r.p("id");
    let full = match r.q("parent_id").and_then(|p| group_key(st, p)) {
        Some(parent) => format!("{parent}/{path}"),
        None => path.to_owned(),
    };
    let exists =
        get(st, "groups", &full).is_some() || (!full.contains('/') && user(st, &full).is_some());
    let suggests = if exists {
        vec![json!(format!("{path}1"))]
    } else {
        vec![]
    };
    ok(json!({ "exists": exists, "suggests": suggests }))
}

// ---------------------------------------------------------------------------
// Events

fn render_event(st: &Store, base: &str, e: &Value) -> Value {
    let author = str_of(e, "author");
    let project = str_of(e, "project");
    let a = user_basic(st, base, author);
    let mut out = json!({
        "id": e["id"],
        "project_id": project_id(st, project),
        "action_name": e.get("action_name").cloned().unwrap_or(json!("opened")),
        "target_id": e.get("target_id").cloned().unwrap_or(Value::Null),
        "target_iid": e.get("target_iid").cloned().unwrap_or(Value::Null),
        "target_type": e.get("target_type").cloned().unwrap_or(Value::Null),
        "author_id": a["id"],
        "target_title": e.get("target_title").cloned().unwrap_or(Value::Null),
        "created_at": e.get("created_at").cloned().unwrap_or(Value::Null),
        "author": a,
        "imported": false,
        "imported_from": "none",
        "author_username": author,
    });
    for extra in ["push_data", "note"] {
        if let Some(v) = e.get(extra) {
            out[extra] = v.clone();
        }
    }
    out
}

fn filter_events(st: &Store, r: &Req, pred: impl Fn(&Value) -> bool) -> Vec<Value> {
    let mut items: Vec<Value> = all(st, "events")
        .into_iter()
        .map(|(_, e)| e)
        .filter(|e| pred(e))
        .filter(|e| {
            r.q("action")
                .is_none_or(|a| str_of(e, "action_name").starts_with(a))
        })
        .filter(|e| {
            r.q("target_type")
                .is_none_or(|t| str_of(e, "target_type").eq_ignore_ascii_case(t))
        })
        .filter(|e| {
            r.q("before").is_none_or(|b| {
                &str_of(e, "created_at")[..10.min(str_of(e, "created_at").len())] < b
            })
        })
        .filter(|e| {
            r.q("after").is_none_or(|a| {
                &str_of(e, "created_at")[..10.min(str_of(e, "created_at").len())] > a
            })
        })
        .cloned()
        .collect();
    items.sort_by(|a, b| {
        let ka = (str_of(a, "created_at").to_owned(), as_u64(&a["id"]));
        let kb = (str_of(b, "created_at").to_owned(), as_u64(&b["id"]));
        if r.q("sort") == Some("asc") {
            ka.cmp(&kb)
        } else {
            kb.cmp(&ka)
        }
    });
    items.iter().map(|e| render_event(st, &r.base, e)).collect()
}

fn user_events(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let current = current_username(st);
    let items = filter_events(st, r, |e| str_of(e, "author") == current);
    paged(r, items)
}

fn user_events_by_id(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let Some(u) = resolve_username(st, r.p("user_id")) else {
        return not_found("User");
    };
    let items = filter_events(st, r, |e| str_of(e, "author") == u);
    paged(r, items)
}

fn project_events(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let items = filter_events(st, r, |e| str_of(e, "project") == key);
    paged(r, items)
}

// ---------------------------------------------------------------------------
// Search

fn search_in(st: &Store, r: &Req, projects: &[String], group: Option<&str>) -> Response {
    let Some(scope) = r.q("scope") else {
        return missing("scope");
    };
    let Some(term) = r.q("search") else {
        return missing("search");
    };
    let needle = term.to_lowercase();
    let in_scope = |key: &str| projects.iter().any(|p| p == crate::common::project_of(key));
    let items: Vec<Value> = match scope {
        "projects" => projects
            .iter()
            .filter(|k| {
                let p = get(st, "projects", k).cloned().unwrap_or(json!({}));
                k.to_lowercase().contains(&needle)
                    || str_of(&p, "description").to_lowercase().contains(&needle)
            })
            .map(|k| render_project_simple(st, &r.base, k))
            .collect(),
        "issues" => all(st, "issues")
            .into_iter()
            .filter(|(k, i)| {
                in_scope(k)
                    && (str_of(i, "title").to_lowercase().contains(&needle)
                        || str_of(i, "description").to_lowercase().contains(&needle))
                    && r.q("state")
                        .is_none_or(|s| s == "all" || str_of(i, "state") == s)
            })
            .map(|(k, _)| crate::issues::render(st, &r.base, k))
            .collect(),
        "merge_requests" => all(st, "merge_requests")
            .into_iter()
            .filter(|(k, m)| {
                in_scope(k)
                    && (str_of(m, "title").to_lowercase().contains(&needle)
                        || str_of(m, "description").to_lowercase().contains(&needle))
                    && r.q("state")
                        .is_none_or(|s| s == "all" || str_of(m, "state") == s)
            })
            .map(|(k, _)| crate::merge_requests::render(st, &r.base, k))
            .collect(),
        "milestones" => all(st, "milestones")
            .into_iter()
            .filter(|(k, m)| in_scope(k) && str_of(m, "title").to_lowercase().contains(&needle))
            .map(|(k, _)| crate::issues::render_milestone(st, &r.base, k))
            .collect(),
        "users" => all(st, "users")
            .into_iter()
            .filter(|(k, u)| {
                k.to_lowercase().contains(&needle)
                    || str_of(u, "name").to_lowercase().contains(&needle)
            })
            .map(|(k, _)| user_basic(st, &r.base, k))
            .collect(),
        "blobs" => crate::repository::search_blobs(st, r, projects, term),
        "commits" => crate::repository::search_commits(st, r, projects, term),
        "wiki_blobs" => crate::wikis::search_wikis(st, projects, group, term),
        "notes" => all(st, "notes")
            .into_iter()
            .filter(|(_, n)| {
                in_scope(str_of(n, "noteable"))
                    && str_of(n, "body").to_lowercase().contains(&needle)
            })
            .map(|(_, n)| crate::notes::render_note(st, &r.base, n))
            .collect(),
        "snippet_titles" => Vec::new(),
        _ => return bad_request("scope does not have a valid value"),
    };
    paged(r, items)
}

fn global_search(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let projects: Vec<String> = all(st, "projects")
        .iter()
        .map(|(k, _)| (*k).to_owned())
        .collect();
    search_in(st, r, &projects, None)
}

fn project_search(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    search_in(st, r, &[key], None)
}

fn group_search(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_group(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let prefix = format!("{key}/");
    let projects: Vec<String> = all(st, "projects")
        .iter()
        .filter(|(k, _)| k.starts_with(&prefix))
        .map(|(k, _)| (*k).to_owned())
        .collect();
    search_in(st, r, &projects, Some(&key))
}

// ---------------------------------------------------------------------------
// Todos

fn render_todo(st: &Store, base: &str, t: &Value) -> Value {
    let target_key = str_of(t, "target");
    let project = crate::common::project_of(target_key);
    let target_type =
        t.get("target_type")
            .and_then(Value::as_str)
            .unwrap_or(if target_key.contains('!') {
                "MergeRequest"
            } else {
                "Issue"
            });
    let target = if target_key.contains('!') {
        crate::merge_requests::render(st, base, target_key)
    } else if target_key.contains('#') {
        crate::issues::render(st, base, target_key)
    } else {
        Value::Null
    };
    let iid = iid_of(target_key);
    let url_kind = if target_key.contains('!') {
        "merge_requests"
    } else {
        "issues"
    };
    json!({
        "id": t["id"],
        "project": if get(st, "projects", project).is_some() { render_project_simple(st, base, project) } else { Value::Null },
        "group": null,
        "author": user_basic(st, base, str_of(t, "author")),
        "action_name": t.get("action_name").cloned().unwrap_or(json!("assigned")),
        "target_type": target_type,
        "target": target,
        "target_url": format!("{base}/{project}/-/{url_kind}/{iid}"),
        "body": t.get("body").cloned().unwrap_or_else(|| target.get("title").cloned().unwrap_or(json!(""))),
        "state": t.get("state").cloned().unwrap_or(json!("pending")),
        "created_at": t.get("created_at").cloned().unwrap_or(Value::Null),
        "updated_at": t.get("updated_at").cloned().or_else(|| t.get("created_at").cloned()).unwrap_or(Value::Null),
    })
}

fn list_todos(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let current = current_username(st);
    let state = r.q("state").unwrap_or("pending");
    let mut items: Vec<Value> = all(st, "todos")
        .into_iter()
        .map(|(_, t)| t)
        .filter(|t| t.get("user").and_then(Value::as_str).unwrap_or(&current) == current)
        .filter(|t| t.get("state").and_then(Value::as_str).unwrap_or("pending") == state)
        .filter(|t| r.q("action").is_none_or(|a| str_of(t, "action_name") == a))
        .filter(|t| {
            r.q("type")
                .is_none_or(|ty| render_todo(st, &r.base, t)["target_type"] == ty)
        })
        .filter(|t| {
            r.q("project_id").is_none_or(|p| {
                project_key(st, p).as_deref()
                    == Some(crate::common::project_of(str_of(t, "target")))
            })
        })
        .filter(|t| {
            r.q("author_id")
                .is_none_or(|a| resolve_username(st, a).as_deref() == Some(str_of(t, "author")))
        })
        .map(|t| render_todo(st, &r.base, t))
        .collect();
    sort_desc_by_created(&mut items);
    paged(r, items)
}

fn mark_todo_done(w: &mut WorldState, r: &Req) -> Response {
    let id = r.p("todo_id").to_owned();
    let now = now(w);
    let st = svc_mut(w);
    let Some(t) = st.collection_mut("todos").get_mut(&id) else {
        return not_found("Todo");
    };
    t["state"] = json!("done");
    t["updated_at"] = json!(now);
    let t = t.clone();
    created(render_todo(st, &r.base, &t))
}

fn mark_all_todos_done(w: &mut WorldState, _r: &Req) -> Response {
    let now = now(w);
    let st = svc_mut(w);
    let current = current_username(st);
    let keys: Vec<String> = all(st, "todos")
        .iter()
        .filter(|(_, t)| t.get("user").and_then(Value::as_str).unwrap_or(&current) == current)
        .map(|(k, _)| (*k).to_owned())
        .collect();
    for k in keys {
        if let Some(t) = st.collection_mut("todos").get_mut(&k)
            && t.get("state").and_then(Value::as_str).unwrap_or("pending") == "pending"
        {
            t["state"] = json!("done");
            t["updated_at"] = json!(now);
        }
    }
    no_content()
}
