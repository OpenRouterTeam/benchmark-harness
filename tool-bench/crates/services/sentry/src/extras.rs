//! Long-tail resources: cron monitors, uptime monitors, alert workflows, detectors, metric
//! alert rules, dashboards, replays, attachments, profiling, snapshots, AI conversations.
//! Each reads its own dataset collection (API-shaped documents) and is empty otherwise.

use axum::{
    body::Bytes,
    extract::{Path, RawQuery, State},
    http::{HeaderMap, StatusCode, Uri},
    response::Response,
};
use mock_core::world::{SharedWorld, WorldState};
use serde_json::{Value, json};

use crate::{
    NAME,
    org::{find_project, org_exists},
    util::{
        Query, bad_request, collection, created, detail, docs, field_error, next_numeric_id,
        no_content, not_found, ok, paginate, parse_body, str_of,
    },
};

fn doc_project_matches(world: &WorldState, doc: &Value, wanted: &[String]) -> bool {
    if wanted.is_empty() {
        return true;
    }
    let slug = doc["project"]["slug"]
        .as_str()
        .or_else(|| doc["projectSlug"].as_str())
        .or_else(|| doc["project"].as_str());
    let id = doc["projectId"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| doc["project"]["id"].as_str().map(str::to_owned))
        .or_else(|| {
            slug.and_then(|s| find_project(world, s))
                .map(|p| str_of(&p["id"]))
        });
    wanted.iter().any(|w| {
        Some(w.as_str()) == slug
            || id.as_deref() == Some(w.as_str())
            || find_project(world, w).is_some_and(|p| Some(str_of(&p["id"])) == id)
    })
}

fn name_matches(doc: &Value, q: &Query) -> bool {
    let raw = q
        .get("query")
        .or_else(|| q.get("name"))
        .unwrap_or("")
        .trim();
    let needle = raw
        .strip_prefix("name:")
        .unwrap_or(raw)
        .trim_matches('"')
        .replace("\\\"", "\"")
        .to_lowercase();
    if needle.is_empty() {
        return true;
    }
    ["name", "title", "slug"].iter().any(|k| {
        doc[*k].as_str().is_some_and(|s| {
            if needle.contains('*') {
                crate::search::glob(&needle, s)
            } else {
                s.to_lowercase().contains(&needle)
            }
        })
    })
}

fn list_collection(world: &WorldState, name: &str, q: &Query, project: Option<&str>) -> Vec<Value> {
    let mut wanted: Vec<String> = q
        .all("project")
        .into_iter()
        .chain(q.all("projectSlug"))
        .filter(|p| *p != "-1")
        .map(str::to_owned)
        .collect();
    if let Some(p) = project {
        wanted.push(p.to_owned());
    }
    docs(world, name)
        .into_iter()
        .filter(|d| doc_project_matches(world, d, &wanted))
        .filter(|d| name_matches(d, q))
        .cloned()
        .collect()
}

fn find_in<'a>(world: &'a WorldState, name: &str, id: &str) -> Option<(String, &'a Value)> {
    collection(world, name)?
        .iter()
        .find(|(k, d)| *k == id || str_of(&d["id"]) == id || d["slug"].as_str() == Some(id))
        .map(|(k, d)| (k.to_owned(), d))
}

macro_rules! org_guard {
    ($world:expr, $org:expr) => {
        if !org_exists($world, $org) {
            return not_found();
        }
    };
}

// --- Cron monitors ----------------------------------------------------------------------

pub async fn list_monitors(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    paginate(
        list_collection(&world, "monitors", &q, None),
        &q,
        &uri,
        &headers,
        25,
        100,
    )
}

pub async fn get_monitor(
    State(world): State<SharedWorld>,
    Path((org, slug)): Path<(String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    match find_in(&world, "monitors", &slug) {
        Some((_, m)) => ok(m.clone()),
        None => not_found(),
    }
}

pub async fn get_project_monitor(
    State(world): State<SharedWorld>,
    Path((org, project, slug)): Path<(String, String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    match find_in(&world, "monitors", &slug)
        .filter(|(_, m)| doc_project_matches(&world, m, std::slice::from_ref(&project)))
    {
        Some((_, m)) => ok(m.clone()),
        None => not_found(),
    }
}

fn monitor_sub(world: &WorldState, slug: &str, field: &str) -> Option<Vec<Value>> {
    find_in(world, "monitors", slug).map(|(_, m)| {
        let id = str_of(&m["id"]);
        let slug = m["slug"].clone();
        docs(world, field)
            .into_iter()
            .filter(|c| c["monitor"] == slug || str_of(&c["monitor"]) == id)
            .map(|c| {
                let mut c = c.clone();
                if let Some(o) = c.as_object_mut() {
                    o.remove("monitor");
                }
                c
            })
            .collect()
    })
}

pub async fn monitor_checkins(
    State(world): State<SharedWorld>,
    Path((org, slug)): Path<(String, String)>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    match monitor_sub(&world, &slug, "monitor_checkins") {
        Some(c) => paginate(c, &q, &uri, &headers, 25, 100),
        None => not_found(),
    }
}

pub async fn project_monitor_checkins(
    State(world): State<SharedWorld>,
    Path((org, _project, slug)): Path<(String, String, String)>,
    raw: RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    monitor_checkins(State(world), Path((org, slug)), raw, uri, headers).await
}

pub async fn monitor_stats(
    State(world): State<SharedWorld>,
    Path((org, slug)): Path<(String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    match monitor_sub(&world, &slug, "monitor_stats") {
        Some(s) => ok(Value::Array(s)),
        None => not_found(),
    }
}

pub async fn project_monitor_stats(
    State(world): State<SharedWorld>,
    Path((org, _project, slug)): Path<(String, String, String)>,
) -> Response {
    monitor_stats(State(world), Path((org, slug))).await
}

// --- Uptime monitors --------------------------------------------------------------------

pub async fn list_uptime(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    paginate(
        list_collection(&world, "uptime_monitors", &q, None),
        &q,
        &uri,
        &headers,
        25,
        100,
    )
}

pub async fn get_uptime(
    State(world): State<SharedWorld>,
    Path((org, project, id)): Path<(String, String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    match find_in(&world, "uptime_monitors", &id)
        .filter(|(_, m)| doc_project_matches(&world, m, std::slice::from_ref(&project)))
    {
        Some((_, m)) => ok(m.clone()),
        None => not_found(),
    }
}

pub async fn uptime_checks(
    State(world): State<SharedWorld>,
    Path((org, project, id)): Path<(String, String, String)>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    let Some((_, m)) = find_in(&world, "uptime_monitors", &id)
        .filter(|(_, m)| doc_project_matches(&world, m, std::slice::from_ref(&project)))
    else {
        return not_found();
    };
    let mid = str_of(&m["id"]);
    let checks: Vec<Value> = docs(&world, "uptime_checks")
        .into_iter()
        .filter(|c| str_of(&c["monitor"]) == mid)
        .map(|c| {
            let mut c = c.clone();
            if let Some(o) = c.as_object_mut() {
                o.remove("monitor");
            }
            c
        })
        .collect();
    paginate(checks, &q, &uri, &headers, 25, 100)
}

fn uptime_doc(
    world: &WorldState,
    project: &str,
    id: &str,
    input: &Value,
    existing: Option<&Value>,
) -> Value {
    let base = existing.cloned().unwrap_or_else(|| {
        json!({
            "id": id,
            "projectSlug": project,
            "environment": null,
            "name": "",
            "status": "active",
            "uptimeStatus": 1,
            "mode": 1,
            "owner": null,
            "recoveryThreshold": 1,
            "downtimeThreshold": 3,
            "url": "",
            "method": "GET",
            "body": null,
            "headers": [],
            "intervalSeconds": 60,
            "timeoutMs": 5000,
            "traceSampling": false,
        })
    });
    let _ = world;
    let mut doc = base;
    for k in [
        "name",
        "environment",
        "status",
        "owner",
        "recoveryThreshold",
        "downtimeThreshold",
        "url",
        "method",
        "body",
        "headers",
        "intervalSeconds",
        "timeoutMs",
        "traceSampling",
        "assertion",
    ] {
        if let Some(v) = input.get(k) {
            doc[k] = v.clone();
        }
    }
    doc
}

pub async fn create_uptime(
    State(world): State<SharedWorld>,
    Path((org, project)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let input = match parse_body(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    org_guard!(world, &org);
    let Some(slug) =
        find_project(world, &project).and_then(|p| p["slug"].as_str().map(str::to_owned))
    else {
        return not_found();
    };
    if input["url"].as_str().is_none_or(str::is_empty) {
        return field_error("url", "This field is required.");
    }
    if input["name"].as_str().is_none_or(str::is_empty) {
        return field_error("name", "This field is required.");
    }
    let id = next_numeric_id(world, "uptime_monitors", 1).to_string();
    let doc = uptime_doc(world, &slug, &id, &input, None);
    world
        .service_mut(NAME)
        .store
        .collection_mut("uptime_monitors")
        .insert(id, doc.clone());
    created(doc)
}

pub async fn update_uptime(
    State(world): State<SharedWorld>,
    Path((org, project, id)): Path<(String, String, String)>,
    body: Bytes,
) -> Response {
    let input = match parse_body(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    org_guard!(world, &org);
    let Some((key, existing)) = find_in(world, "uptime_monitors", &id)
        .filter(|(_, m)| doc_project_matches(world, m, std::slice::from_ref(&project)))
        .map(|(k, m)| (k, m.clone()))
    else {
        return not_found();
    };
    let slug = existing["projectSlug"]
        .as_str()
        .unwrap_or(&project)
        .to_owned();
    let doc = uptime_doc(world, &slug, &id, &input, Some(&existing));
    world
        .service_mut(NAME)
        .store
        .collection_mut("uptime_monitors")
        .insert(key, doc.clone());
    ok(doc)
}

pub async fn delete_uptime(
    State(world): State<SharedWorld>,
    Path((org, project, id)): Path<(String, String, String)>,
) -> Response {
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    org_guard!(world, &org);
    let Some(key) = find_in(world, "uptime_monitors", &id)
        .filter(|(_, m)| doc_project_matches(world, m, std::slice::from_ref(&project)))
        .map(|(k, _)| k)
    else {
        return not_found();
    };
    world
        .service_mut(NAME)
        .store
        .collection_mut("uptime_monitors")
        .remove(&key);
    no_content()
}

// --- Alert workflows (issue alerts) ---------------------------------------------------------

fn workflow_doc(world: &WorldState, id: &str, input: &Value, existing: Option<&Value>) -> Value {
    let now = world.clock.now_rfc3339();
    let mut doc = existing.cloned().unwrap_or_else(|| {
        json!({
            "id": id,
            "name": "",
            "organizationId": collection(world, "organizations")
                .and_then(|c| c.values().next().map(|o| str_of(&o["id"])))
                .unwrap_or_default(),
            "createdBy": crate::org::auth_user(world).map(|u| str_of(&u["id"])),
            "dateCreated": now,
            "dateUpdated": now,
            "triggers": { "logicType": "any-short", "conditions": [], "actions": [] },
            "actionFilters": [],
            "environment": null,
            "config": { "frequency": 30 },
            "detectorIds": [],
            "enabled": true,
            "lastTriggered": null,
            "owner": null,
        })
    });
    for k in [
        "name",
        "triggers",
        "actionFilters",
        "environment",
        "config",
        "detectorIds",
        "enabled",
        "owner",
    ] {
        if let Some(v) = input.get(k) {
            doc[k] = v.clone();
        }
    }
    doc["dateUpdated"] = json!(now);
    doc
}

pub async fn list_workflows(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    let project = q.get("projectSlug").map(str::to_owned);
    let mut items: Vec<Value> = docs(&world, "workflows")
        .into_iter()
        .filter(|w| name_matches(w, &q))
        .filter(|w| {
            project.as_ref().is_none_or(|p| {
                let pid = find_project(&world, p).map(|p| str_of(&p["id"]));
                workflow_project_ids(&world, w)
                    .iter()
                    .any(|i| Some(i) == pid.as_ref())
            })
        })
        .cloned()
        .collect();
    items.sort_by_key(|w| std::cmp::Reverse(str_of(&w["id"]).parse::<u64>().unwrap_or(0)));
    paginate(items, &q, &uri, &headers, 100, 100)
}

pub async fn get_workflow(
    State(world): State<SharedWorld>,
    Path((org, id)): Path<(String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    match find_in(&world, "workflows", &id) {
        Some((_, w)) => ok(w.clone()),
        None => not_found(),
    }
}

pub async fn workflow_project_scope(
    State(world): State<SharedWorld>,
    Path((org, id)): Path<(String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    match find_in(&world, "workflows", &id) {
        Some((_, w)) => {
            let ids = workflow_project_ids(&world, w);
            ok(json!({ "projectIds": ids, "includesAllProjects": false }))
        }
        None => not_found(),
    }
}

pub async fn create_workflow(
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
    org_guard!(world, &org);
    if input["name"].as_str().is_none_or(str::is_empty) {
        return field_error("name", "This field is required.");
    }
    let id = next_numeric_id(world, "workflows", 1).to_string();
    let doc = workflow_doc(world, &id, &input, None);
    world
        .service_mut(NAME)
        .store
        .collection_mut("workflows")
        .insert(id, doc.clone());
    created(doc)
}

pub async fn update_workflow(
    State(world): State<SharedWorld>,
    Path((org, id)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let input = match parse_body(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    org_guard!(world, &org);
    let Some((key, existing)) = find_in(world, "workflows", &id).map(|(k, w)| (k, w.clone()))
    else {
        return not_found();
    };
    let doc = workflow_doc(world, &id, &input, Some(&existing));
    world
        .service_mut(NAME)
        .store
        .collection_mut("workflows")
        .insert(key, doc.clone());
    ok(doc)
}

fn delete_from(world: &SharedWorld, org: &str, name: &str, id: &str) -> Response {
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    org_guard!(world, org);
    let Some(key) = find_in(world, name, id).map(|(k, _)| k) else {
        return not_found();
    };
    world
        .service_mut(NAME)
        .store
        .collection_mut(name)
        .remove(&key);
    no_content()
}

pub async fn delete_workflow(
    State(world): State<SharedWorld>,
    Path((org, id)): Path<(String, String)>,
) -> Response {
    delete_from(&world, &org, "workflows", &id)
}

// --- Detectors (metric monitors) ---------------------------------------------------------

/// Sentry creates one `issue_stream` detector per project; these stand in for them.
fn issue_stream_detectors(world: &WorldState) -> Vec<Value> {
    docs(world, "projects")
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            let id = (100_001 + i).to_string();
            let workflows: Vec<Value> = docs(world, "workflows")
                .into_iter()
                .filter(|w| {
                    w["detectorIds"]
                        .as_array()
                        .is_some_and(|d| d.iter().any(|x| str_of(x) == id))
                })
                .map(|w| json!(str_of(&w["id"])))
                .collect();
            json!({
                "id": id,
                "projectId": str_of(&p["id"]),
                "name": "Issue Stream",
                "type": "issue_stream",
                "enabled": true,
                "config": {},
                "conditionGroup": null,
                "dataSources": [],
                "workflowIds": workflows,
                "description": null,
                "owner": null,
                "createdBy": null,
                "dateCreated": p.get("dateCreated").cloned().unwrap_or(Value::Null),
                "dateUpdated": p.get("dateCreated").cloned().unwrap_or(Value::Null),
                "alertRuleId": null,
                "ruleId": null,
            })
        })
        .collect()
}

fn all_detectors(world: &WorldState) -> Vec<Value> {
    let mut out = issue_stream_detectors(world);
    out.extend(docs(world, "detectors").into_iter().cloned());
    out
}

/// Project ids an alert workflow covers, from its `projectIds` or its connected detectors.
fn workflow_project_ids(world: &WorldState, w: &Value) -> Vec<String> {
    if let Some(ids) = w["projectIds"].as_array() {
        return ids.iter().map(str_of).collect();
    }
    let detectors = all_detectors(world);
    let mut ids: Vec<String> = w["detectorIds"]
        .as_array()
        .map(|d| {
            d.iter()
                .filter_map(|x| detectors.iter().find(|det| str_of(&det["id"]) == str_of(x)))
                .filter_map(|det| det["projectId"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids.dedup();
    ids
}

pub async fn alert_rule_detector(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    RawQuery(raw): RawQuery,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    let wanted = q
        .get("alert_rule_id")
        .or_else(|| q.get("rule_id"))
        .unwrap_or("");
    match docs(&world, "detectors")
        .into_iter()
        .find(|d| str_of(&d["alertRuleId"]) == wanted || str_of(&d["ruleId"]) == wanted)
    {
        Some(d) => ok(json!({
            "detectorId": str_of(&d["id"]),
            "alertRuleId": d.get("alertRuleId").map(str_of),
            "ruleId": d.get("ruleId").map(str_of),
        })),
        None => not_found(),
    }
}

pub async fn list_detectors(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    let types = q.all("type");
    let wanted: Vec<String> = q
        .all("project")
        .into_iter()
        .filter(|p| *p != "-1")
        .map(str::to_owned)
        .collect();
    let items: Vec<Value> = all_detectors(&world)
        .into_iter()
        .filter(|d| doc_project_matches(&world, d, &wanted))
        .filter(|d| name_matches(d, &q))
        .filter(|d| types.is_empty() || d["type"].as_str().is_some_and(|t| types.contains(&t)))
        .collect();
    paginate(items, &q, &uri, &headers, 25, 100)
}

pub async fn get_detector(
    State(world): State<SharedWorld>,
    Path((org, id)): Path<(String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    match all_detectors(&world)
        .into_iter()
        .find(|d| str_of(&d["id"]) == id)
    {
        Some(d) => ok(d),
        None => not_found(),
    }
}

fn detector_doc(
    world: &WorldState,
    id: &str,
    project_id: &str,
    input: &Value,
    existing: Option<&Value>,
) -> Value {
    let now = world.clock.now_rfc3339();
    let mut doc = existing.cloned().unwrap_or_else(|| {
        json!({
            "id": id,
            "projectId": project_id,
            "name": "",
            "type": "metric_issue",
            "enabled": true,
            "config": {},
            "conditionGroup": null,
            "dataSources": [],
            "workflowIds": [],
            "description": null,
            "owner": null,
            "createdBy": crate::org::auth_user(world).map(|u| str_of(&u["id"])),
            "dateCreated": now,
            "dateUpdated": now,
            "alertRuleId": null,
            "ruleId": null,
        })
    });
    for k in [
        "name",
        "type",
        "enabled",
        "config",
        "conditionGroup",
        "dataSources",
        "workflowIds",
        "description",
        "owner",
    ] {
        if let Some(v) = input.get(k) {
            doc[k] = v.clone();
        }
    }
    if let Some(ds) = input.get("dataSource") {
        doc["dataSources"] = json!([ds]);
    }
    doc["dateUpdated"] = json!(now);
    doc
}

pub async fn create_detector(
    State(world): State<SharedWorld>,
    Path((org, project)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let input = match parse_body(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    org_guard!(world, &org);
    let Some(pid) = find_project(world, &project).map(|p| str_of(&p["id"])) else {
        return not_found();
    };
    if input["name"].as_str().is_none_or(str::is_empty) {
        return field_error("name", "This field is required.");
    }
    let id = next_numeric_id(world, "detectors", 200_001).to_string();
    let doc = detector_doc(world, &id, &pid, &input, None);
    world
        .service_mut(NAME)
        .store
        .collection_mut("detectors")
        .insert(id, doc.clone());
    created(doc)
}

pub async fn update_detector(
    State(world): State<SharedWorld>,
    Path((org, id)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let input = match parse_body(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    org_guard!(world, &org);
    let Some((key, existing)) = find_in(world, "detectors", &id).map(|(k, d)| (k, d.clone()))
    else {
        return not_found();
    };
    let pid = str_of(&existing["projectId"]);
    let doc = detector_doc(world, &id, &pid, &input, Some(&existing));
    world
        .service_mut(NAME)
        .store
        .collection_mut("detectors")
        .insert(key, doc.clone());
    ok(doc)
}

pub async fn delete_detector(
    State(world): State<SharedWorld>,
    Path((org, id)): Path<(String, String)>,
) -> Response {
    delete_from(&world, &org, "detectors", &id)
}

pub async fn available_actions(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    let mut actions: Vec<Value> = docs(&world, "available_actions")
        .into_iter()
        .cloned()
        .collect();
    if actions.is_empty() {
        actions = vec![json!({
            "type": "email",
            "handlerGroup": "notification",
            "configSchema": {
                "type": "object",
                "properties": {
                    "target_type": { "type": "string", "enum": ["user", "team", "issue_owners"] },
                    "target_identifier": { "type": ["string", "null"] }
                }
            },
            "dataSchema": { "type": "object", "properties": { "fallthroughType": { "type": "string" } } }
        })];
    }
    paginate(actions, &q, &uri, &headers, 100, 100)
}

pub async fn data_conditions(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    let group = q.get("group").unwrap_or("");
    let all = [
        (
            "first_seen_event",
            "workflow_trigger",
            json!({ "type": "boolean" }),
        ),
        (
            "regression_event",
            "workflow_trigger",
            json!({ "type": "boolean" }),
        ),
        (
            "reappeared_event",
            "workflow_trigger",
            json!({ "type": "boolean" }),
        ),
        (
            "issue_priority_greater_or_equal",
            "action_filter",
            json!({ "type": "integer", "enum": [25, 50, 75] }),
        ),
        (
            "event_frequency_count",
            "action_filter",
            json!({ "type": "object", "properties": { "value": { "type": "integer" }, "interval": { "type": "string" } } }),
        ),
        (
            "tagged_event",
            "action_filter",
            json!({ "type": "object", "properties": { "key": { "type": "string" }, "match": { "type": "string" }, "value": { "type": "string" } } }),
        ),
    ];
    let out: Vec<Value> = all
        .into_iter()
        .filter(|(_, g, _)| group.is_empty() || *g == group)
        .map(|(t, g, schema)| json!({ "type": t, "handlerGroup": g, "comparisonJsonSchema": schema }))
        .collect();
    paginate(out, &q, &uri, &headers, 100, 100)
}

pub async fn list_metric_alerts(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    paginate(
        list_collection(&world, "metric_alerts", &q, None),
        &q,
        &uri,
        &headers,
        25,
        100,
    )
}

pub async fn get_metric_alert(
    State(world): State<SharedWorld>,
    Path((org, id)): Path<(String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    match find_in(&world, "metric_alerts", &id) {
        Some((_, d)) => ok(d.clone()),
        None => not_found(),
    }
}

pub async fn combined_rules(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    let mut items = list_collection(&world, "metric_alerts", &q, None);
    for i in &mut items {
        i["type"] = json!("alert_rule");
    }
    paginate(items, &q, &uri, &headers, 25, 100)
}

// --- Dashboards -----------------------------------------------------------------------------

pub async fn list_dashboards(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    let mut items: Vec<Value> = docs(&world, "dashboards")
        .into_iter()
        .filter(|d| name_matches(d, &q))
        .map(|d| {
            let widgets = d["widgets"].as_array().cloned().unwrap_or_default();
            json!({
                "id": str_of(&d["id"]),
                "title": d["title"].clone(),
                "dateCreated": d["dateCreated"].clone(),
                "createdBy": d.get("createdBy").cloned().unwrap_or(Value::Null),
                "environment": d.get("environment").cloned().unwrap_or(json!([])),
                "filters": d.get("filters").cloned().unwrap_or(json!({})),
                "lastVisited": d.get("lastVisited").cloned().unwrap_or(Value::Null),
                "widgetDisplay": widgets.iter().map(|w| w["displayType"].clone()).collect::<Vec<_>>(),
                "widgetPreview": widgets.iter().map(|w| json!({ "displayType": w["displayType"].clone(), "layout": w.get("layout").cloned().unwrap_or(Value::Null) })).collect::<Vec<_>>(),
                "permissions": null,
                "isFavorited": d.get("isFavorited").cloned().unwrap_or(json!(false)),
                "projects": d.get("projects").cloned().unwrap_or(json!([])),
                "prebuiltId": null,
            })
        })
        .collect();
    match q.get("sort") {
        Some("title") => items.sort_by_key(|d| str_of(&d["title"])),
        Some("-title") => items.sort_by_key(|d| std::cmp::Reverse(str_of(&d["title"]))),
        Some("-dateCreated") => items.sort_by_key(|d| std::cmp::Reverse(str_of(&d["dateCreated"]))),
        Some("dateCreated") => items.sort_by_key(|d| str_of(&d["dateCreated"])),
        _ => {}
    }
    paginate(items, &q, &uri, &headers, 25, 100)
}

pub async fn get_dashboard(
    State(world): State<SharedWorld>,
    Path((org, id)): Path<(String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    match find_in(&world, "dashboards", &id) {
        Some((_, d)) => ok(d.clone()),
        None => not_found(),
    }
}

// --- Replays --------------------------------------------------------------------------------

fn replay_json(r: &Value) -> Value {
    let mut r = r.clone();
    if let Some(o) = r.as_object_mut() {
        o.remove("issues");
        o.remove("segments");
    }
    r
}

pub async fn list_replays(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    let terms = crate::search::parse(q.get("query").unwrap_or(""));
    let now = world.clock.now_unix();
    let items: Vec<Value> = list_collection(&world, "replays", &Query::default(), None)
        .into_iter()
        .filter(|r| {
            let lookup = |k: &str| r.get(k).cloned();
            crate::search::matches(&terms, now, &lookup, &|_| false, &|_| None)
        })
        .map(|r| replay_json(&r))
        .collect();
    let page = crate::util::page(items, &q, &uri, &headers, 25, 100);
    crate::util::with_link(ok(json!({ "data": page.items })), &page.link)
}

pub async fn get_replay(
    State(world): State<SharedWorld>,
    Path((org, id)): Path<(String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    match find_in(&world, "replays", &id) {
        Some((_, r)) => ok(json!({ "data": replay_json(r) })),
        None => not_found(),
    }
}

pub async fn replay_segments(
    State(world): State<SharedWorld>,
    Path((org, _project, id)): Path<(String, String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    match find_in(&world, "replays", &id) {
        Some((_, r)) => ok(r.get("segments").cloned().unwrap_or(json!([]))),
        None => not_found(),
    }
}

// --- Attachments, source links --------------------------------------------------------------

pub async fn list_attachments(
    State(world): State<SharedWorld>,
    Path((org, project, event)): Path<(String, String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    if find_project(&world, &project).is_none() {
        return not_found();
    }
    let items: Vec<Value> = docs(&world, "attachments")
        .into_iter()
        .filter(|a| a["event"].as_str() == Some(event.as_str()))
        .map(|a| {
            let mut a = a.clone();
            if let Some(o) = a.as_object_mut() {
                o.remove("event");
                o.remove("content");
            }
            a
        })
        .collect();
    ok(Value::Array(items))
}

pub async fn get_attachment(
    State(world): State<SharedWorld>,
    Path((org, project, event, id)): Path<(String, String, String, String)>,
    RawQuery(raw): RawQuery,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    if find_project(&world, &project).is_none() {
        return not_found();
    }
    let Some(a) = docs(&world, "attachments")
        .into_iter()
        .find(|a| a["event"].as_str() == Some(event.as_str()) && str_of(&a["id"]) == id)
    else {
        return not_found();
    };
    if q.get("download").is_some() {
        let content = a["content"].as_str().unwrap_or("").to_owned();
        let mime = a["mimetype"]
            .as_str()
            .unwrap_or("application/octet-stream")
            .to_owned();
        return axum::response::IntoResponse::into_response((
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, mime)],
            content,
        ));
    }
    let mut a = a.clone();
    if let Some(o) = a.as_object_mut() {
        o.remove("event");
        o.remove("content");
    }
    ok(a)
}

pub async fn stacktrace_link(
    State(world): State<SharedWorld>,
    Path((org, project)): Path<(String, String)>,
    RawQuery(raw): RawQuery,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    let Some(p) = find_project(&world, &project) else {
        return not_found();
    };
    let Some(file) = q.non_empty("file") else {
        return bad_request("Filepath is required");
    };
    let slug = p["slug"].as_str().unwrap_or("");
    let mapping = docs(&world, "code_mappings")
        .into_iter()
        .find(|m| m["project"].as_str() == Some(slug));
    match mapping {
        Some(m) => {
            let repo = m["repository"].as_str().unwrap_or("");
            let branch = m["defaultBranch"].as_str().unwrap_or("main");
            let stack_root = m["stackRoot"].as_str().unwrap_or("");
            let source_root = m["sourceRoot"].as_str().unwrap_or("");
            let rel = file
                .strip_prefix(stack_root)
                .unwrap_or(file)
                .trim_start_matches('/');
            let path = if source_root.is_empty() {
                rel.to_owned()
            } else {
                format!("{}/{rel}", source_root.trim_end_matches('/'))
            };
            ok(json!({
                "config": { "repoName": repo, "stackRoot": stack_root, "sourceRoot": source_root, "defaultBranch": branch, "provider": { "key": "github", "name": "GitHub" } },
                "sourceUrl": format!("https://github.com/{repo}/blob/{branch}/{path}"),
                "sourcePath": path,
                "integrations": [],
                "error": null,
            }))
        }
        None => ok(json!({
            "config": null,
            "sourceUrl": null,
            "integrations": [],
            "error": "no_code_mappings_for_project",
        })),
    }
}

// --- Profiling, snapshots, AI conversations, onboarding ---------------------------------

pub async fn flamegraph(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    RawQuery(raw): RawQuery,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    let project = q.get("project").and_then(|p| find_project(&world, p));
    let Some(p) = project else {
        return detail(StatusCode::BAD_REQUEST, "one project_id must be specified.");
    };
    ok(json!({
        "activeProfileIndex": 0,
        "metadata": {},
        "platform": p.get("platform").cloned().unwrap_or(json!("")),
        "profiles": [],
        "projectID": str_of(&p["id"]).parse::<u64>().unwrap_or(0),
        "shared": { "frames": [], "frame_infos": [], "profiles": [] },
        "transactionName": q.get("query").map(|qq| qq.trim_start_matches("transaction:").trim_matches('"').to_owned()),
    }))
}

pub async fn profile(
    State(world): State<SharedWorld>,
    Path((org, _project, _id)): Path<(String, String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    not_found()
}

pub async fn profile_chunks(State(world): State<SharedWorld>, Path(org): Path<String>) -> Response {
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    not_found()
}

pub async fn org_not_found_2(
    State(world): State<SharedWorld>,
    Path((org, _a)): Path<(String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    not_found()
}

pub async fn org_not_found_3(
    State(world): State<SharedWorld>,
    Path((org, _a, _b)): Path<(String, String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    not_found()
}

pub async fn org_not_found_1(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
) -> Response {
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    not_found()
}

pub async fn list_conversations(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    let items: Vec<Value> = docs(&world, "conversations")
        .into_iter()
        .map(|c| {
            let mut c = c.clone();
            if let Some(o) = c.as_object_mut() {
                o.remove("spans");
            }
            c
        })
        .collect();
    paginate(items, &q, &uri, &headers, 25, 100)
}

pub async fn get_conversation(
    State(world): State<SharedWorld>,
    Path((org, id)): Path<(String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    org_guard!(&world, &org);
    match docs(&world, "conversations")
        .into_iter()
        .find(|c| c["conversationId"].as_str() == Some(id.as_str()))
    {
        Some(c) => ok(json!({
            "conversationId": id,
            "title": c.get("title").cloned().unwrap_or(Value::Null),
            "spans": c.get("spans").cloned().unwrap_or(json!([])),
        })),
        None => not_found(),
    }
}
