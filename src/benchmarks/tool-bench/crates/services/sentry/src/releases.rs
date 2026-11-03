//! Releases, their deploys and commits.

use axum::{
    extract::{Path, RawQuery, State},
    http::{HeaderMap, Uri},
    response::Response,
};
use mock_core::world::{SharedWorld, WorldState};
use serde_json::{Value, json};

use crate::{
    org::{find_project, org_exists, project_brief},
    util::{Query, collection, docs, not_found, ok, paginate, str_of, value_time},
};

/// `pkg@1.2.3+abc` -> `1.2.3+abc`; long hashes are cut to 12 characters.
pub fn short_version(version: &str) -> String {
    let v = version.split_once('@').map_or(version, |(_, v)| v);
    if v.len() >= 32 && v.chars().all(|c| c.is_ascii_hexdigit()) {
        v[..12].to_owned()
    } else {
        v.to_owned()
    }
}

fn release_projects(r: &Value) -> Vec<String> {
    r["projects"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|p| p.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn events_in(world: &WorldState, version: &str) -> Vec<i64> {
    let mut t: Vec<i64> = docs(world, "events")
        .iter()
        .filter(|e| e["release"].as_str() == Some(version))
        .filter_map(|e| value_time(&e["dateCreated"]))
        .collect();
    t.sort_unstable();
    t
}

fn release_json(world: &WorldState, r: &Value) -> Value {
    let version = r["version"].as_str().unwrap_or("");
    let times = events_in(world, version);
    let fmt = |t: Option<&i64>| t.map_or(Value::Null, |t| json!(mock_core::clock::format_unix(*t)));
    let new_groups = docs(world, "issues")
        .iter()
        .filter(|i| i["firstRelease"].as_str() == Some(version))
        .count();
    let projects: Vec<Value> = release_projects(r)
        .iter()
        .map(|slug| {
            let mut p = project_brief(world, slug);
            let n = docs(world, "issues")
                .iter()
                .filter(|i| {
                    i["firstRelease"].as_str() == Some(version)
                        && i["project"].as_str() == Some(slug)
                })
                .count();
            p["id"] = json!(str_of(&p["id"]).parse::<u64>().unwrap_or(0));
            p["newGroups"] = json!(n);
            p["platforms"] = p["platform"].as_str().map_or(json!([]), |pl| json!([pl]));
            p["hasHealthData"] = json!(false);
            p
        })
        .collect();
    let get = |k: &str, d: Value| r.get(k).cloned().unwrap_or(d);
    json!({
        "id": get("id", json!(0)),
        "version": version,
        "status": get("status", json!("open")),
        "shortVersion": short_version(version),
        "versionInfo": {
            "package": version.split_once('@').map(|(p, _)| p),
            "version": { "raw": short_version(version) },
            "description": short_version(version),
            "buildHash": null,
        },
        "ref": get("ref", Value::Null),
        "url": get("url", Value::Null),
        "dateReleased": get("dateReleased", Value::Null),
        "dateCreated": get("dateCreated", Value::Null),
        "data": {},
        "newGroups": get("newGroups", json!(new_groups)),
        "owner": get("owner", Value::Null),
        "commitCount": get("commitCount", json!(r["commits"].as_array().map_or(0, Vec::len))),
        "lastCommit": get("lastCommit", r["commits"].as_array().and_then(|c| c.first()).cloned().unwrap_or(Value::Null)),
        "deployCount": get("deployCount", json!(r["deploys"].as_array().map_or(0, Vec::len))),
        "lastDeploy": get("lastDeploy", r["deploys"].as_array().and_then(|d| d.last()).cloned().unwrap_or(Value::Null)),
        "authors": get("authors", json!([])),
        "projects": projects,
        "firstEvent": get("firstEvent", fmt(times.first())),
        "lastEvent": get("lastEvent", fmt(times.last())),
        "currentProjectMeta": {},
        "userAgent": null,
    })
}

fn list(world: &WorldState, q: &Query, project: Option<&str>) -> Vec<Value> {
    let ids: Vec<String> = q
        .all("project")
        .into_iter()
        .filter(|p| *p != "-1")
        .map(str::to_owned)
        .collect();
    let needle = q.get("query").map(str::to_lowercase).unwrap_or_default();
    let mut releases: Vec<&Value> = docs(world, "releases")
        .into_iter()
        .filter(|r| {
            let ps = release_projects(r);
            project.is_none_or(|p| ps.iter().any(|x| x == p))
                && (ids.is_empty()
                    || ps.iter().any(|slug| {
                        find_project(world, slug).is_some_and(|p| ids.contains(&str_of(&p["id"])))
                    }))
        })
        .filter(|r| {
            needle.is_empty()
                || r["version"]
                    .as_str()
                    .is_some_and(|v| v.to_lowercase().contains(&needle))
        })
        .collect();
    releases.sort_by(|a, b| {
        value_time(&b["dateCreated"])
            .cmp(&value_time(&a["dateCreated"]))
            .then_with(|| str_of(&b["version"]).cmp(&str_of(&a["version"])))
    });
    releases
        .into_iter()
        .map(|r| release_json(world, r))
        .collect()
}

pub async fn list_org_releases(
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
    let out = list(&world, &q, None);
    paginate(out, &q, &uri, &headers, 25, 100)
}

pub async fn list_project_releases(
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
    let Some(slug) = find_project(&world, &project).and_then(|p| p["slug"].as_str()) else {
        return not_found();
    };
    let out = list(&world, &q, Some(slug));
    paginate(out, &q, &uri, &headers, 25, 100)
}

fn find_release<'a>(
    world: &'a WorldState,
    version: &str,
    project: Option<&str>,
) -> Option<&'a Value> {
    let r = collection(world, "releases")?.get(version)?;
    match project {
        Some(p) => release_projects(r).iter().any(|x| x == p).then_some(r),
        None => Some(r),
    }
}

fn detail_json(world: &WorldState, r: &Value) -> Value {
    let mut out = release_json(world, r);
    out["adoptionStages"] = json!({});
    out["refs"] = r.get("refs").cloned().unwrap_or(json!([]));
    out
}

pub async fn get_org_release(
    State(world): State<SharedWorld>,
    Path((org, version)): Path<(String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return not_found();
    }
    match find_release(&world, &version, None) {
        Some(r) => ok(detail_json(&world, r)),
        None => not_found(),
    }
}

pub async fn get_project_release(
    State(world): State<SharedWorld>,
    Path((org, project, version)): Path<(String, String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return not_found();
    }
    let Some(slug) = find_project(&world, &project).and_then(|p| p["slug"].as_str()) else {
        return not_found();
    };
    match find_release(&world, &version, Some(slug)) {
        Some(r) => {
            let mut out = detail_json(&world, r);
            out["currentProjectMeta"] = json!({
                "nextReleaseVersion": null,
                "prevReleaseVersion": null,
                "firstReleaseVersion": null,
                "lastReleaseVersion": null,
                "sessionsCrashed": 0,
                "sessionsLowerBound": null,
                "sessionsUpperBound": null,
            });
            ok(out)
        }
        None => not_found(),
    }
}

pub async fn list_deploys(
    State(world): State<SharedWorld>,
    Path((org, version)): Path<(String, String)>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return not_found();
    }
    let Some(r) = find_release(&world, &version, None) else {
        return not_found();
    };
    let mut deploys: Vec<Value> = r["deploys"].as_array().cloned().unwrap_or_default();
    deploys.reverse();
    paginate(deploys, &q, &uri, &headers, 100, 100)
}

pub async fn list_org_commits(
    State(world): State<SharedWorld>,
    Path((org, version)): Path<(String, String)>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return not_found();
    }
    let Some(r) = find_release(&world, &version, None) else {
        return not_found();
    };
    let commits = r["commits"].as_array().cloned().unwrap_or_default();
    paginate(commits, &q, &uri, &headers, 100, 100)
}

pub async fn list_project_commits(
    State(world): State<SharedWorld>,
    Path((org, project, version)): Path<(String, String, String)>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return not_found();
    }
    let Some(slug) = find_project(&world, &project).and_then(|p| p["slug"].as_str()) else {
        return not_found();
    };
    let Some(r) = find_release(&world, &version, Some(slug)) else {
        return not_found();
    };
    let commits = r["commits"].as_array().cloned().unwrap_or_default();
    paginate(commits, &q, &uri, &headers, 100, 100)
}
