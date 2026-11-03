//! Identity, organizations, teams, projects, client keys (DSNs), environments, tags, repos.

use axum::{
    body::Bytes,
    extract::{Path, RawQuery, State},
    http::{HeaderMap, StatusCode, Uri},
    response::Response,
};
use mock_core::world::{SharedWorld, WorldState};
use serde_json::{Map, Value, json};

use crate::{
    NAME,
    util::{
        Query, bad_request, base_url, collection, created, detail, docs, field_error, hex_id,
        next_numeric_id, no_content, not_found, ok, paginate, parse_body, slugify, str_of,
    },
};

const ORG_ACCESS: [&str; 15] = [
    "org:read",
    "org:write",
    "org:integrations",
    "member:read",
    "team:read",
    "team:write",
    "team:admin",
    "project:read",
    "project:write",
    "project:admin",
    "project:releases",
    "event:read",
    "event:write",
    "event:admin",
    "alerts:write",
];

// ---------------------------------------------------------------------------------------
// Lookups shared with other modules
// ---------------------------------------------------------------------------------------

pub fn org_exists(world: &WorldState, slug: &str) -> bool {
    collection(world, "organizations").is_some_and(|c| c.contains(slug))
}

pub fn auth_user(world: &WorldState) -> Option<&Value> {
    docs(world, "users").into_iter().next()
}

pub fn find_user<'a>(world: &'a WorldState, needle: &str) -> Option<&'a Value> {
    docs(world, "users").into_iter().find(|u| {
        str_of(&u["id"]) == needle
            || u["email"]
                .as_str()
                .is_some_and(|e| e.eq_ignore_ascii_case(needle))
            || u["username"]
                .as_str()
                .is_some_and(|e| e.eq_ignore_ascii_case(needle))
    })
}

pub fn find_team<'a>(world: &'a WorldState, needle: &str) -> Option<&'a Value> {
    docs(world, "teams")
        .into_iter()
        .find(|t| str_of(&t["id"]) == needle || t["slug"].as_str() == Some(needle))
}

/// A project by slug or numeric id.
pub fn find_project<'a>(world: &'a WorldState, slug_or_id: &str) -> Option<&'a Value> {
    collection(world, "projects")?
        .iter()
        .find(|(k, p)| *k == slug_or_id || str_of(&p["id"]) == slug_or_id)
        .map(|(_, p)| p)
}

pub fn project_key(world: &WorldState, slug_or_id: &str) -> Option<String> {
    collection(world, "projects")?
        .iter()
        .find(|(k, p)| *k == slug_or_id || str_of(&p["id"]) == slug_or_id)
        .map(|(k, _)| k.to_owned())
}

/// `{id, slug, name, platform}` as embedded in issues and releases.
pub fn project_brief(world: &WorldState, slug: &str) -> Value {
    match find_project(world, slug) {
        Some(p) => json!({
            "id": str_of(&p["id"]),
            "name": p["name"].clone(),
            "slug": p["slug"].clone(),
            "platform": p.get("platform").cloned().unwrap_or(Value::Null),
        }),
        None => json!({ "id": "0", "name": slug, "slug": slug, "platform": null }),
    }
}

pub fn user_json(user: &Value) -> Value {
    let id = str_of(&user["id"]);
    let email = user["email"].clone();
    json!({
        "id": id,
        "name": user["name"].clone(),
        "username": user.get("username").cloned().unwrap_or_else(|| email.clone()),
        "email": email.clone(),
        "avatarUrl": null,
        "isActive": true,
        "hasPasswordAuth": true,
        "isManaged": false,
        "dateJoined": user.get("dateJoined").cloned().unwrap_or(json!("2023-01-09T10:00:00Z")),
        "lastLogin": user.get("lastLogin").cloned().unwrap_or(Value::Null),
        "has2fa": true,
        "lastActive": user.get("lastLogin").cloned().unwrap_or(Value::Null),
        "isSuperuser": false,
        "isStaff": false,
        "experiments": {},
        "emails": [{ "id": id, "email": email, "is_verified": true }],
        "avatar": { "avatarType": "letter_avatar", "avatarUuid": null, "avatarUrl": null },
    })
}

/// Assignee object as Sentry renders `assignedTo`.
pub fn user_actor(user: &Value) -> Value {
    json!({
        "type": "user",
        "id": str_of(&user["id"]),
        "name": user["name"].clone(),
        "email": user["email"].clone(),
    })
}

pub fn team_actor(team: &Value) -> Value {
    json!({ "type": "team", "id": str_of(&team["id"]), "name": team["slug"].clone() })
}

fn org_json(org: &Value, base: &str) -> Value {
    let mut out = json!({
        "id": str_of(&org["id"]),
        "slug": org["slug"].clone(),
        "status": { "id": "active", "name": "active" },
        "name": org["name"].clone(),
        "dateCreated": org.get("dateCreated").cloned().unwrap_or(json!("2022-06-01T12:00:00Z")),
        "isEarlyAdopter": false,
        "require2FA": false,
        "avatar": { "avatarType": "letter_avatar", "avatarUuid": null, "avatarUrl": null },
        "links": { "organizationUrl": base, "regionUrl": "" },
        "hasAuthProvider": false,
        "allowMemberInvite": true,
        "allowMemberProjectCreation": true,
        "allowSuperuserAccess": false,
        "features": ["discover-basic", "discover-query", "performance-view", "incidents", "event-attachments"],
    });
    if let (Some(o), Some(extra)) = (out.as_object_mut(), org.as_object()) {
        for (k, v) in extra {
            if !o.contains_key(k) {
                o.insert(k.clone(), v.clone());
            }
        }
    }
    out
}

fn org_detail_json(world: &WorldState, org: &Value, base: &str) -> Value {
    let mut out = org_json(org, base);
    out["access"] = json!(ORG_ACCESS);
    out["role"] = json!("owner");
    out["orgRole"] = json!("owner");
    out["isDefault"] = json!(false);
    out["defaultRole"] = json!("member");
    out["openMembership"] = json!(true);
    out["quota"] =
        json!({ "maxRate": null, "maxRateInterval": 60, "accountLimit": 0, "projectLimit": 100 });
    out["teams"] = Value::Array(
        docs(world, "teams")
            .into_iter()
            .map(|t| team_json(world, t))
            .collect(),
    );
    out["projects"] = Value::Array(
        docs(world, "projects")
            .into_iter()
            .map(|p| project_json(world, p))
            .collect(),
    );
    out
}

fn team_members(world: &WorldState, slug: &str) -> usize {
    docs(world, "users")
        .iter()
        .filter(|u| {
            u["teams"]
                .as_array()
                .is_some_and(|t| t.iter().any(|x| x.as_str() == Some(slug)))
        })
        .count()
}

pub fn team_json(world: &WorldState, team: &Value) -> Value {
    let slug = team["slug"].as_str().unwrap_or("");
    let is_member = auth_user(world).is_some_and(|u| {
        u["teams"]
            .as_array()
            .is_some_and(|t| t.iter().any(|x| x.as_str() == Some(slug)))
    });
    json!({
        "id": str_of(&team["id"]),
        "slug": team["slug"].clone(),
        "name": team["name"].clone(),
        "dateCreated": team.get("dateCreated").cloned().unwrap_or(json!("2023-01-09T10:00:00Z")),
        "isMember": is_member,
        "teamRole": if is_member { json!("admin") } else { Value::Null },
        "flags": { "idp:provisioned": false },
        "access": ["team:read", "team:write", "project:read", "project:write", "event:read", "event:write"],
        "hasAccess": true,
        "isPending": false,
        "memberCount": team_members(world, slug),
        "avatar": { "avatarType": "letter_avatar", "avatarUuid": null },
        "externalTeams": [],
        "projects": [],
    })
}

fn team_brief(world: &WorldState, slug: &str) -> Value {
    match find_team(world, slug) {
        Some(t) => {
            json!({ "id": str_of(&t["id"]), "slug": t["slug"].clone(), "name": t["name"].clone() })
        }
        None => json!({ "id": "0", "slug": slug, "name": slug }),
    }
}

fn project_teams(project: &Value) -> Vec<String> {
    project["teams"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|t| t.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

pub fn project_json(world: &WorldState, p: &Value) -> Value {
    let teams: Vec<Value> = project_teams(p)
        .iter()
        .map(|t| team_brief(world, t))
        .collect();
    let latest_release = docs(world, "releases")
        .into_iter()
        .filter(|r| {
            r["projects"]
                .as_array()
                .is_some_and(|ps| ps.iter().any(|x| x == &p["slug"]))
        })
        .max_by_key(|r| r["dateCreated"].as_str().unwrap_or("").to_owned())
        .map(|r| json!({ "version": r["version"].clone() }))
        .unwrap_or(Value::Null);
    json!({
        "id": str_of(&p["id"]),
        "slug": p["slug"].clone(),
        "name": p["name"].clone(),
        "platform": p.get("platform").cloned().unwrap_or(Value::Null),
        "dateCreated": p.get("dateCreated").cloned().unwrap_or(json!("2023-01-09T10:00:00Z")),
        "isBookmarked": false,
        "isMember": true,
        "features": ["alert-filters", "discard-groups", "minidump", "similarity-indexing"],
        "firstEvent": p.get("firstEvent").cloned().unwrap_or(Value::Null),
        "firstTransactionEvent": p.get("firstTransactionEvent").cloned().unwrap_or(json!(false)),
        "access": ["project:read", "project:write", "project:admin", "project:releases", "event:read", "event:write"],
        "hasAccess": true,
        "hasMinifiedStackTrace": false,
        "hasMonitors": !docs(world, "monitors").is_empty(),
        "hasProfiles": p.get("hasProfiles").cloned().unwrap_or(json!(false)),
        "hasReplays": p.get("hasReplays").cloned().unwrap_or(json!(false)),
        "hasFeedbacks": false,
        "hasNewFeedbacks": false,
        "hasSessions": p.get("hasSessions").cloned().unwrap_or(json!(false)),
        "hasLogs": p.get("hasLogs").cloned().unwrap_or(json!(false)),
        "hasInsightsHttp": false,
        "hasInsightsDb": false,
        "isInternal": false,
        "isPublic": false,
        "avatar": { "avatarType": "letter_avatar", "avatarUuid": null },
        "color": p.get("color").cloned().unwrap_or(json!("#3f70bf")),
        "status": "active",
        "team": teams.first().cloned().unwrap_or(Value::Null),
        "teams": teams,
        "environments": p.get("environments").cloned().unwrap_or(json!([])),
        "platforms": p.get("platform").map(|pl| json!([pl])).unwrap_or(json!([])),
        "latestRelease": latest_release,
        "latestDeploys": null,
    })
}

fn key_json(world: &WorldState, k: &Value, base: &str) -> Value {
    let public = k["public"].as_str().unwrap_or("").to_owned();
    let secret = k["secret"].as_str().unwrap_or("").to_owned();
    let project = find_project(world, k["project"].as_str().unwrap_or(""));
    let project_id = project.map(|p| str_of(&p["id"])).unwrap_or_default();
    let host = base.split("://").nth(1).unwrap_or(base);
    let scheme = base.split("://").next().unwrap_or("http");
    json!({
        "id": public,
        "name": k["name"].clone(),
        "label": k["name"].clone(),
        "public": public,
        "secret": secret,
        "projectId": project_id.parse::<u64>().map(Value::from).unwrap_or(json!(project_id)),
        "isActive": k.get("isActive").cloned().unwrap_or(json!(true)),
        "rateLimit": k.get("rateLimit").cloned().unwrap_or(Value::Null),
        "dsn": {
            "secret": format!("{scheme}://{public}:{secret}@{host}/{project_id}"),
            "public": format!("{scheme}://{public}@{host}/{project_id}"),
            "csp": format!("{base}/api/{project_id}/csp-report/?sentry_key={public}"),
            "security": format!("{base}/api/{project_id}/security/?sentry_key={public}"),
            "minidump": format!("{base}/api/{project_id}/minidump/?sentry_key={public}"),
            "nel": format!("{base}/api/{project_id}/nel/?sentry_key={public}"),
            "unreal": format!("{base}/api/{project_id}/unreal/{public}/"),
            "crons": format!("{base}/api/{project_id}/cron/___MONITOR_SLUG___/{public}/"),
            "cdn": format!("{base}/js-sdk-loader/{public}.min.js"),
        },
        "browserSdkVersion": k.get("browserSdkVersion").cloned().unwrap_or(json!("8.x")),
        "browserSdk": { "choices": [["9.x", "9.x"], ["8.x", "8.x"], ["7.x", "7.x"]] },
        "dateCreated": k.get("dateCreated").cloned().unwrap_or(Value::Null),
        "dynamicSdkLoaderOptions": k.get("dynamicSdkLoaderOptions").cloned().unwrap_or(json!({
            "hasReplay": true, "hasPerformance": true, "hasDebug": false
        })),
    })
}

fn text_filter(doc: &Value, query: Option<&str>) -> bool {
    let Some(q) = query.map(str::trim).filter(|q| !q.is_empty()) else {
        return true;
    };
    q.split_whitespace().all(|word| {
        let (field, needle) = match word.split_once(':') {
            Some((f, n)) => (Some(f), n),
            None => (None, word),
        };
        let needle = needle.to_lowercase();
        let hit = |key: &str| {
            doc[key]
                .as_str()
                .is_some_and(|s| s.to_lowercase().contains(&needle))
        };
        match field {
            Some("slug") => hit("slug"),
            Some("name") => hit("name"),
            Some("id") => str_of(&doc["id"]) == needle,
            _ => hit("slug") || hit("name"),
        }
    })
}

// ---------------------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------------------

pub async fn whoami(State(world): State<SharedWorld>) -> Response {
    let world = world.lock().expect("world lock");
    match auth_user(&world) {
        Some(u) => ok(user_json(u)),
        None => detail(
            StatusCode::UNAUTHORIZED,
            "Authentication credentials were not provided.",
        ),
    }
}

pub async fn list_orgs(
    State(world): State<SharedWorld>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    let base = base_url(&headers);
    let orgs: Vec<Value> = docs(&world, "organizations")
        .into_iter()
        .filter(|o| text_filter(o, q.get("query")))
        .map(|o| org_json(o, &base))
        .collect();
    paginate(orgs, &q, &uri, &headers, 100, 100)
}

pub async fn get_org(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    headers: HeaderMap,
) -> Response {
    let world = world.lock().expect("world lock");
    match collection(&world, "organizations").and_then(|c| c.get(&org)) {
        Some(o) => ok(org_detail_json(&world, o, &base_url(&headers))),
        None => not_found(),
    }
}

pub async fn list_teams(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return not_found();
    }
    let teams: Vec<Value> = docs(&world, "teams")
        .into_iter()
        .filter(|t| text_filter(t, q.get("query")))
        .map(|t| team_json(&world, t))
        .collect();
    paginate(teams, &q, &uri, &headers, 100, 100)
}

pub async fn create_team(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    body: Bytes,
) -> Response {
    let input = match parse_body(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    if !org_exists(world, &org) {
        return not_found();
    }
    let name = input["name"].as_str().map(str::trim).unwrap_or("");
    let slug = input["slug"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| slugify(name));
    if slug.is_empty() {
        return field_error("slug", "This field is required.");
    }
    if find_team(world, &slug).is_some() {
        return detail(
            StatusCode::CONFLICT,
            "A team with this slug already exists.",
        );
    }
    let id = next_numeric_id(world, "teams", 1);
    let now = world.clock.now_rfc3339();
    let team = json!({
        "id": id.to_string(),
        "slug": slug,
        "name": if name.is_empty() { slug.clone() } else { name.to_owned() },
        "dateCreated": now,
    });
    world
        .service_mut(NAME)
        .store
        .collection_mut("teams")
        .insert(slug.clone(), team.clone());
    // The creator becomes a member of the new team.
    let first_user =
        collection(world, "users").and_then(|c| c.iter().next().map(|(k, _)| k.to_owned()));
    if let Some(user_key) = first_user
        && let Some(user) = world
            .service_mut(NAME)
            .store
            .collection_mut("users")
            .get_mut(&user_key)
    {
        match user["teams"].as_array_mut() {
            Some(t) => t.push(json!(slug)),
            None => user["teams"] = json!([slug]),
        }
    }
    created(team_json(world, &team))
}

pub async fn list_projects(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return not_found();
    }
    let projects: Vec<Value> = docs(&world, "projects")
        .into_iter()
        .filter(|p| text_filter(p, q.get("query")))
        .map(|p| project_json(&world, p))
        .collect();
    paginate(projects, &q, &uri, &headers, 100, 100)
}

pub async fn list_team_projects(
    State(world): State<SharedWorld>,
    Path((org, team)): Path<(String, String)>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) || find_team(&world, &team).is_none() {
        return not_found();
    }
    let projects: Vec<Value> = docs(&world, "projects")
        .into_iter()
        .filter(|p| project_teams(p).iter().any(|t| t == &team))
        .map(|p| project_json(&world, p))
        .collect();
    paginate(projects, &q, &uri, &headers, 100, 100)
}

fn project_detail(world: &WorldState, p: &Value, org: &str, base: &str) -> Value {
    let mut out = project_json(world, p);
    if let Some(o) = collection(world, "organizations").and_then(|c| c.get(org)) {
        out["organization"] = org_json(o, base);
    }
    out["options"] = json!({ "sentry:resolve_age": 0, "sentry:scrub_data": true });
    out["resolveAge"] = json!(0);
    out["dataScrubber"] = json!(true);
    out["allowedDomains"] = json!(["*"]);
    out["subjectPrefix"] = json!("");
    out["digestsMinDelay"] = json!(300);
    out["digestsMaxDelay"] = json!(1800);
    out
}

pub async fn get_project(
    State(world): State<SharedWorld>,
    Path((org, project)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return not_found();
    }
    match find_project(&world, &project) {
        Some(p) => ok(project_detail(&world, p, &org, &base_url(&headers))),
        None => not_found(),
    }
}

pub async fn create_project(
    State(world): State<SharedWorld>,
    Path((org, team)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let input = match parse_body(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    if !org_exists(world, &org) {
        return not_found();
    }
    let Some(team_doc) = find_team(world, &team).cloned() else {
        return not_found();
    };
    let Some(name) = input["name"]
        .as_str()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    else {
        return field_error("name", "This field is required.");
    };
    let slug = input["slug"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| slugify(name));
    if slug.is_empty()
        || !slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return field_error(
            "slug",
            "Enter a valid slug consisting of lowercase letters, numbers, underscores or hyphens. It cannot be entirely numeric.",
        );
    }
    if find_project(world, &slug).is_some() {
        return detail(
            StatusCode::CONFLICT,
            "A project with this slug already exists.",
        );
    }
    let id = next_numeric_id(world, "projects", 1);
    let now = world.clock.now_rfc3339();
    let project = json!({
        "id": id.to_string(),
        "slug": slug,
        "name": name,
        "platform": input.get("platform").cloned().filter(|p| !p.is_null()).unwrap_or(Value::Null),
        "teams": [team_doc["slug"].clone()],
        "dateCreated": now,
        "firstEvent": null,
    });
    world
        .service_mut(NAME)
        .store
        .collection_mut("projects")
        .insert(slug.clone(), project.clone());
    // Sentry creates a "Default" client key with every new project.
    insert_key(world, &slug, "Default");
    created(project_json(world, &project))
}

pub async fn update_project(
    State(world): State<SharedWorld>,
    Path((org, project)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let input = match parse_body(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    if !org_exists(world, &org) {
        return not_found();
    }
    let Some(key) = project_key(world, &project) else {
        return not_found();
    };
    let new_slug = input["slug"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(s) = new_slug
        && s != key
        && find_project(world, s).is_some()
    {
        return field_error(
            "slug",
            &format!("Another project ({s}) is already using that slug"),
        );
    }
    let store = &mut world.service_mut(NAME).store;
    let mut doc = store
        .collection_mut("projects")
        .get(&key)
        .cloned()
        .unwrap_or_default();
    for field in ["name", "platform"] {
        if let Some(v) = input.get(field).filter(|v| !v.is_null()) {
            doc[field] = v.clone();
        }
    }
    let final_key = match new_slug {
        Some(s) if s != key => {
            doc["slug"] = json!(s);
            store.collection_mut("projects").remove(&key);
            s.to_owned()
        }
        _ => key,
    };
    store
        .collection_mut("projects")
        .insert(final_key, doc.clone());
    ok(project_detail(world, &doc, &org, &base_url(&headers)))
}

pub async fn list_project_teams(
    State(world): State<SharedWorld>,
    Path((org, project)): Path<(String, String)>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return not_found();
    }
    let Some(p) = find_project(&world, &project) else {
        return not_found();
    };
    let teams: Vec<Value> = project_teams(p)
        .iter()
        .filter_map(|t| find_team(&world, t))
        .map(|t| team_json(&world, t))
        .collect();
    paginate(teams, &q, &uri, &headers, 100, 100)
}

pub async fn add_project_team(
    State(world): State<SharedWorld>,
    Path((org, project, team)): Path<(String, String, String)>,
) -> Response {
    change_project_team(world, &org, &project, &team, true)
}

pub async fn remove_project_team(
    State(world): State<SharedWorld>,
    Path((org, project, team)): Path<(String, String, String)>,
) -> Response {
    change_project_team(world, &org, &project, &team, false)
}

fn change_project_team(
    world: SharedWorld,
    org: &str,
    project: &str,
    team: &str,
    add: bool,
) -> Response {
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    if !org_exists(world, org) {
        return not_found();
    }
    let (Some(key), Some(team_doc)) =
        (project_key(world, project), find_team(world, team).cloned())
    else {
        return not_found();
    };
    let slug = team_doc["slug"].clone();
    let store = &mut world.service_mut(NAME).store;
    let Some(doc) = store.collection_mut("projects").get_mut(&key) else {
        return not_found();
    };
    let mut teams: Vec<Value> = doc["teams"].as_array().cloned().unwrap_or_default();
    if add {
        if !teams.contains(&slug) {
            teams.push(slug);
        }
    } else {
        teams.retain(|t| t != &slug);
    }
    doc["teams"] = Value::Array(teams);
    let doc = doc.clone();
    let body = project_json(world, &doc);
    if add { created(body) } else { ok(body) }
}

fn keys_of(world: &WorldState, project: &str) -> Vec<(String, Value)> {
    collection(world, "keys")
        .map(|c| {
            c.iter()
                .filter(|(_, k)| k["project"].as_str() == Some(project))
                .map(|(key, k)| (key.to_owned(), k.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Inserts a new client key, keyed `<project>/<name>` (suffixed `#2`, `#3`, ... on reuse).
fn insert_key(world: &mut WorldState, project: &str, name: &str) -> Value {
    let public = hex_id(world, "key_public", 32);
    let secret = hex_id(world, "key_secret", 32);
    let now = world.clock.now_rfc3339();
    let doc = json!({
        "id": public,
        "name": name,
        "project": project,
        "public": public,
        "secret": secret,
        "isActive": true,
        "rateLimit": null,
        "dateCreated": now,
    });
    let base = format!("{project}/{name}");
    let mut key = base.clone();
    let mut n = 2;
    while collection(world, "keys").is_some_and(|c| c.contains(&key)) {
        key = format!("{base}#{n}");
        n += 1;
    }
    world
        .service_mut(NAME)
        .store
        .collection_mut("keys")
        .insert(key, doc.clone());
    doc
}

pub async fn list_keys(
    State(world): State<SharedWorld>,
    Path((org, project)): Path<(String, String)>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return not_found();
    }
    let Some(slug) = project_key(&world, &project) else {
        return not_found();
    };
    let base = base_url(&headers);
    let status = q.get("status");
    let keys: Vec<Value> = keys_of(&world, &slug)
        .into_iter()
        .map(|(_, k)| k)
        .filter(|k| match status {
            Some("active") => k["isActive"] != json!(false),
            Some("inactive") => k["isActive"] == json!(false),
            _ => true,
        })
        .map(|k| key_json(&world, &k, &base))
        .collect();
    paginate(keys, &q, &uri, &headers, 100, 100)
}

pub async fn create_key(
    State(world): State<SharedWorld>,
    Path((org, project)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let input = match parse_body(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    if !org_exists(world, &org) {
        return not_found();
    }
    let Some(slug) = project_key(world, &project) else {
        return not_found();
    };
    let name = input["name"]
        .as_str()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("Key #{}", keys_of(world, &slug).len() + 1));
    if let Some(rl) = input.get("rateLimit").filter(|v| !v.is_null())
        && (rl["window"].as_u64().is_none() || rl["count"].as_u64().is_none())
    {
        return field_error("rateLimit", "Both window and count are required.");
    }
    let doc = insert_key(world, &slug, &name);
    created(key_json(world, &doc, &base_url(&headers)))
}

fn find_key(world: &WorldState, project: &str, id: &str) -> Option<String> {
    keys_of(world, project)
        .into_iter()
        .find(|(_, k)| k["id"].as_str() == Some(id) || k["public"].as_str() == Some(id))
        .map(|(key, _)| key)
}

pub async fn get_key(
    State(world): State<SharedWorld>,
    Path((org, project, key_id)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return not_found();
    }
    let Some(slug) = project_key(&world, &project) else {
        return not_found();
    };
    match find_key(&world, &slug, &key_id).and_then(|k| collection(&world, "keys")?.get(&k)) {
        Some(doc) => ok(key_json(&world, doc, &base_url(&headers))),
        None => not_found(),
    }
}

pub async fn update_key(
    State(world): State<SharedWorld>,
    Path((org, project, key_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let input = match parse_body(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    if !org_exists(world, &org) {
        return not_found();
    }
    let Some(slug) = project_key(world, &project) else {
        return not_found();
    };
    let Some(key) = find_key(world, &slug, &key_id) else {
        return not_found();
    };
    if let Some(v) = input.get("isActive")
        && !v.is_boolean()
    {
        return field_error("isActive", "Must be a valid boolean.");
    }
    let store = &mut world.service_mut(NAME).store;
    let Some(doc) = store.collection_mut("keys").get_mut(&key) else {
        return not_found();
    };
    for field in ["name", "isActive", "rateLimit", "browserSdkVersion"] {
        if let Some(v) = input.get(field) {
            doc[field] = v.clone();
        }
    }
    if let Some(opts) = input
        .get("dynamicSdkLoaderOptions")
        .and_then(Value::as_object)
    {
        let mut merged: Map<String, Value> = doc["dynamicSdkLoaderOptions"]
            .as_object()
            .cloned()
            .unwrap_or_else(|| {
                json!({"hasReplay": true, "hasPerformance": true, "hasDebug": false})
                    .as_object()
                    .cloned()
                    .unwrap_or_default()
            });
        for (k, v) in opts {
            merged.insert(k.clone(), v.clone());
        }
        doc["dynamicSdkLoaderOptions"] = Value::Object(merged);
    }
    let doc = doc.clone();
    ok(key_json(world, &doc, &base_url(&headers)))
}

pub async fn delete_key(
    State(world): State<SharedWorld>,
    Path((org, project, key_id)): Path<(String, String, String)>,
) -> Response {
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    if !org_exists(world, &org) {
        return not_found();
    }
    let Some(slug) = project_key(world, &project) else {
        return not_found();
    };
    let Some(key) = find_key(world, &slug, &key_id) else {
        return not_found();
    };
    world
        .service_mut(NAME)
        .store
        .collection_mut("keys")
        .remove(&key);
    no_content()
}

pub async fn list_members(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return not_found();
    }
    let members: Vec<Value> = docs(&world, "users")
        .into_iter()
        .filter(|u| {
            q.get("query").is_none_or(|needle| {
                let n = needle.to_lowercase();
                ["name", "email", "username"]
                    .iter()
                    .any(|f| u[*f].as_str().is_some_and(|s| s.to_lowercase().contains(&n)))
            })
        })
        .map(|u| {
            json!({
                "id": str_of(&u["id"]),
                "email": u["email"].clone(),
                "name": u["name"].clone(),
                "user": user_json(u),
                "orgRole": u.get("role").cloned().unwrap_or(json!("member")),
                "role": u.get("role").cloned().unwrap_or(json!("member")),
                "pending": false,
                "expired": false,
                "flags": { "sso:linked": false, "sso:invalid": false, "member-limit:restricted": false },
                "dateCreated": u.get("dateJoined").cloned().unwrap_or(json!("2023-01-09T10:00:00Z")),
                "inviteStatus": "approved",
                "teams": u.get("teams").cloned().unwrap_or(json!([])),
            })
        })
        .collect();
    paginate(members, &q, &uri, &headers, 100, 100)
}

/// Visible environments, from projects' `environments` and events.
pub fn environment_names(world: &WorldState, project_ids: &[String]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let project_match = |slug: &str| {
        project_ids.is_empty()
            || find_project(world, slug).is_some_and(|p| project_ids.contains(&str_of(&p["id"])))
    };
    for p in docs(world, "projects") {
        if !project_match(p["slug"].as_str().unwrap_or("")) {
            continue;
        }
        for e in p["environments"].as_array().into_iter().flatten() {
            if let Some(e) = e.as_str()
                && !names.iter().any(|n| n == e)
            {
                names.push(e.to_owned());
            }
        }
    }
    for e in docs(world, "events") {
        if !project_match(e["project"].as_str().unwrap_or("")) {
            continue;
        }
        if let Some(env) = e["environment"].as_str()
            && !env.is_empty()
            && !names.iter().any(|n| n == env)
        {
            names.push(env.to_owned());
        }
    }
    names
}

pub async fn list_environments(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    RawQuery(raw): RawQuery,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return not_found();
    }
    let projects: Vec<String> = q.all("project").into_iter().map(str::to_owned).collect();
    let envs: Vec<Value> = environment_names(&world, &projects)
        .into_iter()
        .enumerate()
        .map(|(i, name)| json!({ "id": (i + 1).to_string(), "name": name }))
        .collect();
    ok(Value::Array(envs))
}

pub async fn list_project_environments(
    State(world): State<SharedWorld>,
    Path((org, project)): Path<(String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return not_found();
    }
    let Some(p) = find_project(&world, &project) else {
        return not_found();
    };
    let envs: Vec<Value> = environment_names(&world, &[str_of(&p["id"])])
        .into_iter()
        .enumerate()
        .map(|(i, name)| json!({ "id": (i + 1).to_string(), "name": name, "isHidden": false }))
        .collect();
    ok(Value::Array(envs))
}

const BUILTIN_TAGS: [(&str, &str); 6] = [
    ("environment", "Environment"),
    ("release", "Release"),
    ("level", "Level"),
    ("user", "User"),
    ("transaction", "Transaction"),
    ("url", "URL"),
];

pub async fn list_tags(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    RawQuery(raw): RawQuery,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return not_found();
    }
    let projects: Vec<String> = q.all("project").into_iter().map(str::to_owned).collect();
    if matches!(q.get("dataset"), Some("replays")) {
        return ok(json!([]));
    }
    let mut keys: Vec<(String, std::collections::BTreeSet<String>)> = Vec::new();
    for e in docs(&world, "events") {
        let in_scope = projects.is_empty()
            || find_project(&world, e["project"].as_str().unwrap_or(""))
                .is_some_and(|p| projects.contains(&str_of(&p["id"])));
        if !in_scope {
            continue;
        }
        for tag in crate::issues::event_tags(&world, e) {
            let (Some(k), Some(v)) = (tag["key"].as_str(), tag["value"].as_str()) else {
                continue;
            };
            match keys.iter_mut().find(|(key, _)| key == k) {
                Some((_, vals)) => {
                    vals.insert(v.to_owned());
                }
                None => keys.push((k.to_owned(), [v.to_owned()].into_iter().collect())),
            }
        }
    }
    let mut out: Vec<Value> = Vec::new();
    for (k, name) in BUILTIN_TAGS {
        if !keys.iter().any(|(key, _)| key == k) {
            out.push(json!({ "key": k, "name": name, "totalValues": 0 }));
        }
    }
    for (k, vals) in keys {
        let name = BUILTIN_TAGS
            .iter()
            .find(|(b, _)| *b == k)
            .map_or_else(|| k.clone(), |(_, n)| (*n).to_owned());
        out.push(json!({ "key": k, "name": name, "totalValues": vals.len() }));
    }
    ok(Value::Array(out))
}

pub async fn list_repos(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return not_found();
    }
    let repos: Vec<Value> = docs(&world, "repos")
        .into_iter()
        .filter(|r| {
            q.get("query")
                .is_none_or(|n| r["name"].as_str().is_some_and(|s| s.contains(n)))
        })
        .cloned()
        .collect();
    paginate(repos, &q, &uri, &headers, 100, 100)
}

pub async fn bulk_code_mappings(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    body: Bytes,
) -> Response {
    let input = match parse_body(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    if !org_exists(world, &org) {
        return not_found();
    }
    let Some(project) = input["project"].as_str() else {
        return field_error("project", "This field is required.");
    };
    if find_project(world, project).is_none() {
        return bad_request(&format!("Project not found: {project}"));
    }
    let Some(repo) = input["repository"].as_str() else {
        return field_error("repository", "This field is required.");
    };
    if !docs(world, "repos")
        .iter()
        .any(|r| r["name"].as_str() == Some(repo))
    {
        return bad_request(&format!("Repository not found: {repo}"));
    }
    let mappings: Vec<Value> = input["mappings"].as_array().cloned().unwrap_or_default();
    let mut out = Vec::new();
    for m in &mappings {
        let stack_root = m["stackRoot"].as_str().unwrap_or("").to_owned();
        let source_root = m["sourceRoot"].as_str().unwrap_or("").to_owned();
        let key = format!("{project}:{repo}:{stack_root}");
        let existed = collection(world, "code_mappings").is_some_and(|c| c.contains(&key));
        world
            .service_mut(NAME)
            .store
            .collection_mut("code_mappings")
            .insert(
                key,
                json!({
                    "project": project, "repository": repo,
                    "stackRoot": stack_root, "sourceRoot": source_root,
                    "defaultBranch": input.get("defaultBranch").cloned().unwrap_or(json!("main")),
                }),
            );
        out.push(json!({
            "stackRoot": stack_root,
            "sourceRoot": source_root,
            "status": if existed { "updated" } else { "created" },
        }));
    }
    let created_n = out.iter().filter(|m| m["status"] == "created").count();
    ok(json!({
        "created": created_n,
        "updated": out.len() - created_n,
        "errors": 0,
        "mappings": out,
    }))
}
