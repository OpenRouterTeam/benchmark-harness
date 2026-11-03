//! Issues (groups): search, details, updates, events, tags, activity, notes, Seer autofix.

use std::collections::BTreeMap;

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
    org::{
        auth_user, find_project, find_team, find_user, org_exists, project_brief, team_actor,
        user_actor,
    },
    search::{self, Op, Term},
    util::{
        Query, bad_request, base_url, collection, created, docs, field_error, json_status, now, ok,
        paginate, parse_body, str_of, time_window, value_time,
    },
};

// ---------------------------------------------------------------------------------------
// Lookups
// ---------------------------------------------------------------------------------------

/// Store key (short ID) of an issue addressed by short ID (any case) or numeric id.
pub fn issue_key(world: &WorldState, id: &str) -> Option<String> {
    let c = collection(world, "issues")?;
    c.iter()
        .find(|(k, doc)| {
            k.eq_ignore_ascii_case(id)
                || doc["shortId"]
                    .as_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case(id))
                || str_of(&doc["id"]) == id
        })
        .map(|(k, _)| k.to_owned())
}

pub fn find_issue<'a>(world: &'a WorldState, id: &str) -> Option<&'a Value> {
    let key = issue_key(world, id)?;
    collection(world, "issues")?.get(&key)
}

pub fn issue_by_id_any<'a>(world: &'a WorldState, id: &str) -> Option<&'a Value> {
    docs(world, "issues")
        .into_iter()
        .find(|i| str_of(&i["id"]) == id)
}

/// Events of an issue, oldest first.
pub fn issue_events<'a>(world: &'a WorldState, issue: &Value) -> Vec<&'a Value> {
    let id = str_of(&issue["id"]);
    let mut events: Vec<&Value> = docs(world, "events")
        .into_iter()
        .filter(|e| str_of(&e["groupID"]) == id)
        .collect();
    events.sort_by_key(|e| {
        (
            value_time(&e["dateCreated"]).unwrap_or(0),
            str_of(&e["eventID"]),
        )
    });
    events
}

/// Every issue has at least one event; when the dataset has none, this stands in for it.
fn synthetic_event(issue: &Value) -> Value {
    let id = str_of(&issue["id"]);
    let n: u128 = id.parse().unwrap_or(0);
    json!({
        "eventID": format!("{:032x}", n.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x5e17),
        "groupID": id,
        "project": issue["project"].clone(),
        "dateCreated": issue["lastSeen"].clone(),
        "title": issue["title"].clone(),
        "message": issue["metadata"]["value"].clone(),
        "level": issue.get("level").cloned().unwrap_or(json!("error")),
        "culprit": issue["culprit"].clone(),
        "platform": issue["platform"].clone(),
        "release": issue.get("lastRelease").cloned().unwrap_or(Value::Null),
        "exception": issue["metadata"].get("type").map(|t| json!({
            "type": t,
            "value": issue["metadata"]["value"].clone(),
            "frames": [],
        })).unwrap_or(Value::Null),
    })
}

fn issue_events_or_synthetic(world: &WorldState, issue: &Value) -> Vec<Value> {
    let events: Vec<Value> = issue_events(world, issue).into_iter().cloned().collect();
    if events.is_empty() {
        vec![synthetic_event(issue)]
    } else {
        events
    }
}

/// Tags of an event as Sentry lists them: explicit tags plus the built-in ones.
pub fn event_tags(world: &WorldState, e: &Value) -> Vec<Value> {
    let _ = world;
    let mut tags: Vec<(String, String)> = Vec::new();
    let mut push = |k: &str, v: Option<String>| {
        if let Some(v) = v.filter(|v| !v.is_empty())
            && !tags.iter().any(|(key, _)| key == k)
        {
            tags.push((k.to_owned(), v));
        }
    };
    match &e["tags"] {
        Value::Object(m) => {
            for (k, v) in m {
                push(k, Some(str_of(v)));
            }
        }
        Value::Array(a) => {
            for t in a {
                if let Some(k) = t["key"].as_str() {
                    push(k, t["value"].as_str().map(str::to_owned));
                }
            }
        }
        _ => {}
    }
    push(
        "level",
        e["level"]
            .as_str()
            .map(str::to_owned)
            .or(Some("error".into())),
    );
    push("environment", e["environment"].as_str().map(str::to_owned));
    push("release", e["release"].as_str().map(str::to_owned));
    push("transaction", e["transaction"].as_str().map(str::to_owned));
    push("url", e["request"]["url"].as_str().map(str::to_owned));
    let user = &e["user"];
    let user_tag = user["id"]
        .as_str()
        .map(|i| format!("id:{i}"))
        .or_else(|| user["email"].as_str().map(|m| format!("email:{m}")))
        .or_else(|| user["ip_address"].as_str().map(|ip| format!("ip:{ip}")));
    push("user", user_tag);
    tags.sort_by(|a, b| a.0.cmp(&b.0));
    tags.into_iter()
        .map(|(k, v)| json!({ "key": k, "value": v }))
        .collect()
}

fn tag_value(world: &WorldState, e: &Value, key: &str) -> Option<String> {
    event_tags(world, e)
        .into_iter()
        .find(|t| t["key"] == key)
        .and_then(|t| t["value"].as_str().map(str::to_owned))
}

fn release_json(world: &WorldState, version: &Value) -> Value {
    let Some(v) = version.as_str() else {
        return Value::Null;
    };
    match collection(world, "releases").and_then(|c| c.get(v)) {
        Some(r) => json!({
            "version": v,
            "shortVersion": crate::releases::short_version(v),
            "dateCreated": r["dateCreated"].clone(),
            "dateReleased": r.get("dateReleased").cloned().unwrap_or(Value::Null),
            "ref": r.get("ref").cloned().unwrap_or(Value::Null),
            "url": r.get("url").cloned().unwrap_or(Value::Null),
            "lastCommit": r.get("lastCommit").cloned().unwrap_or(Value::Null),
            "lastDeploy": r.get("lastDeploy").cloned().unwrap_or(Value::Null),
        }),
        None => json!({ "version": v, "shortVersion": crate::releases::short_version(v) }),
    }
}

fn org_slug(world: &WorldState) -> String {
    collection(world, "organizations")
        .and_then(|c| c.iter().next().map(|(k, _)| k.to_owned()))
        .unwrap_or_default()
}

fn note_count(world: &WorldState, issue_id: &str) -> usize {
    docs(world, "activities")
        .iter()
        .filter(|a| str_of(&a["issue"]) == issue_id && a["type"] == "note")
        .count()
}

pub fn issue_json(world: &WorldState, issue: &Value, base: &str) -> Value {
    let id = str_of(&issue["id"]);
    let project = issue["project"].as_str().unwrap_or("");
    let org = org_slug(world);
    let get = |k: &str, d: Value| issue.get(k).cloned().unwrap_or(d);
    let mut metadata = get("metadata", json!({}));
    if metadata.get("title").is_none() {
        metadata["title"] = issue["title"].clone();
    }
    json!({
        "id": id,
        "shareId": null,
        "shortId": issue["shortId"].clone(),
        "title": issue["title"].clone(),
        "culprit": get("culprit", Value::Null),
        "permalink": format!("{base}/organizations/{org}/issues/{id}/"),
        "logger": get("logger", Value::Null),
        "level": get("level", json!("error")),
        "status": get("status", json!("unresolved")),
        "statusDetails": get("statusDetails", json!({})),
        "substatus": get("substatus", json!("ongoing")),
        "isPublic": false,
        "platform": get("platform", find_project(world, project).map_or(Value::Null, |p| p["platform"].clone())),
        "project": project_brief(world, project),
        "type": get("type", json!("error")),
        "metadata": metadata,
        "numComments": note_count(world, &id),
        "assignedTo": get("assignedTo", Value::Null),
        "isBookmarked": get("isBookmarked", json!(false)),
        "isSubscribed": get("isSubscribed", json!(false)),
        "subscriptionDetails": null,
        "hasSeen": get("hasSeen", json!(true)),
        "annotations": [],
        "issueType": get("issueType", json!("error")),
        "issueCategory": get("issueCategory", json!("error")),
        "priority": get("priority", json!("medium")),
        "priorityLockedAt": null,
        "seerFixabilityScore": get("seerFixabilityScore", Value::Null),
        "seerAutofixLastTriggered": null,
        "isUnhandled": get("isUnhandled", json!(true)),
        "count": str_of(&get("count", json!("1"))),
        "userCount": get("userCount", json!(0)),
        "firstSeen": get("firstSeen", Value::Null),
        "lastSeen": get("lastSeen", Value::Null),
        "firstRelease": release_json(world, &get("firstRelease", Value::Null)),
        "lastRelease": release_json(world, &get("lastRelease", Value::Null)),
    })
}

fn issue_detail_json(world: &WorldState, issue: &Value, base: &str) -> Value {
    let mut out = issue_json(world, issue, base);
    out["activity"] = Value::Array(activity_for(world, issue));
    out["openPeriods"] = json!([{
        "start": issue["firstSeen"].clone(),
        "end": if issue["status"] == "resolved" { issue.get("resolvedAt").cloned().unwrap_or(Value::Null) } else { Value::Null },
        "duration": null,
        "isOpen": issue["status"] != "resolved",
        "lastChecked": issue["lastSeen"].clone(),
    }]);
    out["seenBy"] = json!([]);
    out["pluginActions"] = json!([]);
    out["pluginIssues"] = json!([]);
    out["pluginContexts"] = json!([]);
    out["userReportCount"] = json!(
        docs(world, "user_reports")
            .iter()
            .filter(|r| str_of(&r["issue"]) == str_of(&issue["id"]))
            .count()
    );
    out["participants"] = json!([]);
    out["tags"] = Value::Array(tag_summaries(world, issue, 0));
    out
}

// ---------------------------------------------------------------------------------------
// Issue search
// ---------------------------------------------------------------------------------------

fn issue_tag_values(world: &WorldState, issue: &Value, key: &str) -> Vec<String> {
    if let Some(dist) = issue["tags"].get(key).and_then(Value::as_array) {
        return dist
            .iter()
            .filter_map(|t| t["value"].as_str().map(str::to_owned))
            .collect();
    }
    let mut vals: Vec<String> = Vec::new();
    for e in issue_events(world, issue) {
        if let Some(v) = tag_value(world, e, key)
            && !vals.contains(&v)
        {
            vals.push(v);
        }
    }
    vals
}

fn latest_release_for(world: &WorldState, project: &str) -> Option<String> {
    docs(world, "releases")
        .into_iter()
        .filter(|r| {
            r["projects"]
                .as_array()
                .is_some_and(|p| p.iter().any(|x| x.as_str() == Some(project)))
        })
        .max_by_key(|r| {
            (
                value_time(&r["dateReleased"]).or_else(|| value_time(&r["dateCreated"])),
                str_of(&r["version"]),
            )
        })
        .and_then(|r| r["version"].as_str().map(str::to_owned))
}

fn issue_field(world: &WorldState, issue: &Value, key: &str) -> Option<Value> {
    let arr = |v: Vec<String>| Some(Value::Array(v.into_iter().map(Value::from).collect()));
    Some(match key {
        "level" | "status" | "substatus" | "title" | "culprit" | "platform" | "firstSeen"
        | "lastSeen" | "shortId" => issue.get(key).cloned()?,
        "message" => issue["title"].clone(),
        "first_seen" | "age" => issue["firstSeen"].clone(),
        "last_seen" => issue["lastSeen"].clone(),
        "issue" | "issue.short_id" => issue["shortId"].clone(),
        "id" | "issue.id" => json!(str_of(&issue["id"])),
        "project" | "project.slug" | "project.name" => issue["project"].clone(),
        "project.id" => find_project(world, issue["project"].as_str().unwrap_or(""))
            .map(|p| json!(str_of(&p["id"])))?,
        "times_seen" | "timesSeen" | "count" | "event.count" => {
            json!(str_of(&issue["count"]).parse::<f64>().unwrap_or(0.0))
        }
        "user.count" | "userCount" | "users" => issue["userCount"].clone(),
        "firstRelease" | "first_release" | "first-release" => issue["firstRelease"].clone(),
        "lastRelease" | "last_release" => issue["lastRelease"].clone(),
        "release" => {
            let mut vals = issue_tag_values(world, issue, "release");
            for k in ["firstRelease", "lastRelease"] {
                if let Some(v) = issue[k].as_str()
                    && !vals.iter().any(|x| x == v)
                {
                    vals.push(v.to_owned());
                }
            }
            return arr(vals);
        }
        "issue.category" | "issue.type" | "issue.priority" | "priority" => {
            let field = match key {
                "issue.category" => "issueCategory",
                "issue.type" => "issueType",
                _ => "priority",
            };
            issue.get(field).cloned().unwrap_or(match field {
                "priority" => json!("medium"),
                _ => json!("error"),
            })
        }
        "error.type" => issue["metadata"]["type"].clone(),
        "error.value" => issue["metadata"]["value"].clone(),
        "error.unhandled" | "error.handled" => {
            let unhandled = issue
                .get("isUnhandled")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            json!(if key == "error.unhandled" {
                unhandled
            } else {
                !unhandled
            })
        }
        k => {
            let tag = k
                .strip_prefix("tags[")
                .and_then(|r| r.strip_suffix(']'))
                .unwrap_or(k);
            let vals = issue_tag_values(world, issue, tag);
            if vals.is_empty() {
                return None;
            }
            return arr(vals);
        }
    })
}

fn assigned_matches(world: &WorldState, issue: &Value, needle: &str) -> bool {
    let assignee = &issue["assignedTo"];
    let needle = needle.trim();
    match needle {
        "none" | "" => assignee.is_null(),
        "me" | "my_teams" | "[me,my_teams]" => {
            let Some(me) = auth_user(world) else {
                return false;
            };
            let my_teams: Vec<String> = me["teams"]
                .as_array()
                .map(|t| {
                    t.iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let is_me = assignee["type"] == "user" && str_of(&assignee["id"]) == str_of(&me["id"]);
            let my_team = assignee["type"] == "team"
                && assignee["name"]
                    .as_str()
                    .is_some_and(|n| my_teams.iter().any(|t| t == n));
            match needle {
                "me" => is_me,
                "my_teams" => my_team,
                _ => is_me || my_team,
            }
        }
        n if n.starts_with('#') => {
            assignee["type"] == "team"
                && assignee["name"]
                    .as_str()
                    .is_some_and(|t| t.eq_ignore_ascii_case(&n[1..]))
        }
        n => {
            assignee["type"] == "user"
                && find_user(world, n).is_some_and(|u| str_of(&u["id"]) == str_of(&assignee["id"]))
        }
    }
}

fn is_matches(issue: &Value, value: &str, world: &WorldState) -> bool {
    let status = issue["status"].as_str().unwrap_or("unresolved");
    let substatus = issue["substatus"].as_str().unwrap_or("");
    match value {
        "unresolved" => status == "unresolved",
        "resolved" => status == "resolved",
        "ignored" | "archived" | "muted" => status == "ignored",
        "assigned" => !issue["assignedTo"].is_null(),
        "unassigned" => issue["assignedTo"].is_null(),
        "for_review" => {
            status == "unresolved" && matches!(substatus, "new" | "regressed" | "escalating")
        }
        "new" | "regressed" | "escalating" | "ongoing" => substatus == value,
        "bookmarked" => issue["isBookmarked"] == json!(true),
        "linked" => !external_issues(world, issue).is_empty(),
        "unlinked" => external_issues(world, issue).is_empty(),
        _ => false,
    }
}

fn issue_matches(world: &WorldState, issue: &Value, terms: &[Term], now: i64) -> bool {
    let lookup = |k: &str| issue_field(world, issue, k);
    let text = |t: &str| {
        let needle = t.to_lowercase();
        [
            &issue["title"],
            &issue["culprit"],
            &issue["metadata"]["value"],
            &issue["metadata"]["type"],
            &issue["shortId"],
        ]
        .iter()
        .any(|v| {
            v.as_str().is_some_and(|s| {
                if needle.contains('*') {
                    search::glob(&format!("*{needle}*"), s)
                } else {
                    s.to_lowercase().contains(&needle)
                }
            })
        })
    };
    let custom = |term: &Term| -> Option<bool> {
        let Term::Filter {
            negated,
            key,
            op,
            values,
        } = term
        else {
            return None;
        };
        let project = issue["project"].as_str().unwrap_or("");
        let result = match key.as_str() {
            "is" => values.iter().any(|v| is_matches(issue, v, world)),
            "assigned" | "assigned_or_suggested" | "assigned_to" => {
                values.iter().any(|v| assigned_matches(world, issue, v))
            }
            "bookmarks" => false,
            "release" | "firstRelease" | "first_release"
                if values.iter().any(|v| v == "latest") =>
            {
                let latest = latest_release_for(world, project);
                let field = issue_field(world, issue, key).unwrap_or(Value::Null);
                latest.is_some_and(|l| search::compare(&field, Op::Eq, &l, now))
            }
            "firstSeen" | "lastSeen" | "first_seen" | "last_seen" | "age"
                if *op == Op::Eq
                    && values
                        .iter()
                        .all(|v| v.starts_with('-') || v.starts_with('+')) =>
            {
                let field = issue_field(world, issue, key).unwrap_or(Value::Null);
                values.iter().any(|v| search::compare(&field, *op, v, now))
            }
            _ => return None,
        };
        Some(result != *negated)
    };
    search::matches(terms, now, &lookup, &text, &custom)
}

fn sort_issues(issues: &mut [&Value], sort: &str) {
    let num = |i: &Value, k: &str| str_of(&i[k]).parse::<f64>().unwrap_or(0.0);
    let time = |i: &Value, k: &str| value_time(&i[k]).unwrap_or(0);
    let prio = |i: &Value| match i["priority"].as_str().unwrap_or("medium") {
        "high" => 3,
        "medium" => 2,
        _ => 1,
    };
    issues.sort_by(|a, b| {
        let primary = match sort {
            "new" => time(b, "firstSeen").cmp(&time(a, "firstSeen")),
            "freq" => num(b, "count").total_cmp(&num(a, "count")),
            "user" => num(b, "userCount").total_cmp(&num(a, "userCount")),
            "priority" | "recommended" | "trends" => prio(b)
                .cmp(&prio(a))
                .then(num(b, "count").total_cmp(&num(a, "count"))),
            _ => time(b, "lastSeen").cmp(&time(a, "lastSeen")),
        };
        primary.then_with(|| {
            let ia = str_of(&a["id"]).parse::<u64>().unwrap_or(0);
            let ib = str_of(&b["id"]).parse::<u64>().unwrap_or(0);
            ib.cmp(&ia)
        })
    });
}

pub async fn list_issues(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return crate::util::not_found();
    }
    let base = base_url(&headers);
    let now = now(&world);
    let query = q.get("query").unwrap_or("is:unresolved").trim().to_owned();

    // Short-ID and event-ID lookups bypass the search, as in Sentry.
    if !query.is_empty() && !query.contains(char::is_whitespace) {
        if let Some(issue) = find_issue(&world, &query).filter(|i| {
            i["shortId"]
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case(&query))
        }) {
            return paginate(
                vec![issue_json(&world, issue, &base)],
                &q,
                &uri,
                &headers,
                25,
                100,
            );
        }
        if query.len() == 32 && query.chars().all(|c| c.is_ascii_hexdigit()) {
            let found: Vec<Value> = docs(&world, "events")
                .into_iter()
                .filter(|e| {
                    e["eventID"]
                        .as_str()
                        .is_some_and(|id| id.eq_ignore_ascii_case(&query))
                })
                .filter_map(|e| issue_by_id_any(&world, &str_of(&e["groupID"])))
                .map(|i| issue_json(&world, i, &base))
                .collect();
            return paginate(found, &q, &uri, &headers, 25, 100);
        }
    }

    let projects = q.all("project");
    let mut project_ids: Vec<String> = Vec::new();
    for p in projects {
        if p == "-1" {
            continue;
        }
        if !p.chars().all(|c| c.is_ascii_digit()) {
            return bad_request("Invalid project parameter. Values must be numbers.");
        }
        if find_project(&world, p).is_none() {
            return json_status(
                StatusCode::FORBIDDEN,
                json!({ "detail": "You do not have permission to perform this action." }),
            );
        }
        project_ids.push(p.to_owned());
    }
    let environments: Vec<&str> = q.all("environment");
    let (from, to) = match time_window(&q, now, "90d") {
        Ok(w) => w,
        Err(r) => return r,
    };
    let terms = search::parse(&query);
    let mut issues: Vec<&Value> = docs(&world, "issues")
        .into_iter()
        .filter(|i| {
            project_ids.is_empty()
                || find_project(&world, i["project"].as_str().unwrap_or(""))
                    .is_some_and(|p| project_ids.contains(&str_of(&p["id"])))
        })
        .filter(|i| {
            environments.is_empty() || {
                let envs = issue_tag_values(&world, i, "environment");
                environments.iter().any(|e| envs.iter().any(|x| x == e))
            }
        })
        .filter(|i| {
            let last = value_time(&i["lastSeen"]).unwrap_or(0);
            let first = value_time(&i["firstSeen"]).unwrap_or(0);
            last >= from && first <= to
        })
        .filter(|i| issue_matches(&world, i, &terms, now))
        .collect();
    sort_issues(&mut issues, q.get("sort").unwrap_or("date"));
    let out: Vec<Value> = issues
        .into_iter()
        .map(|i| issue_json(&world, i, &base))
        .collect();
    paginate(out, &q, &uri, &headers, 25, 100)
}

pub async fn get_issue(
    State(world): State<SharedWorld>,
    Path((org, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return crate::util::not_found();
    }
    match find_issue(&world, &id) {
        Some(issue) => ok(issue_detail_json(&world, issue, &base_url(&headers))),
        None => crate::util::not_found(),
    }
}

/// Resolves `assignedTo` input (`user:ID`, `team:ID_OR_SLUG`, `me`, username or email).
fn resolve_actor(world: &WorldState, input: &str) -> Result<Value, Response> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(Value::Null);
    }
    let unknown = || field_error("assignedTo", "Unknown actor input");
    if input == "me" {
        return auth_user(world).map(user_actor).ok_or_else(unknown);
    }
    if let Some(id) = input.strip_prefix("user:") {
        return docs(world, "users")
            .into_iter()
            .find(|u| str_of(&u["id"]) == id)
            .map(user_actor)
            .ok_or_else(unknown);
    }
    if let Some(id) = input.strip_prefix("team:") {
        return find_team(world, id).map(team_actor).ok_or_else(unknown);
    }
    if input.contains(':') {
        return Err(field_error(
            "assignedTo",
            "Could not parse actor. Format should be `type:id` where type is `team` or `user`.",
        ));
    }
    find_user(world, input).map(user_actor).ok_or_else(unknown)
}

const STATUSES: [&str; 5] = [
    "resolved",
    "resolvedInNextRelease",
    "unresolved",
    "ignored",
    "reprocessing",
];
const SUBSTATUSES: [&str; 7] = [
    "archived_until_escalating",
    "archived_until_condition_met",
    "archived_forever",
    "escalating",
    "ongoing",
    "regressed",
    "new",
];

fn add_activity(world: &mut WorldState, issue_id: &str, kind: &str, data: Value) -> Value {
    let id = world
        .service_mut(NAME)
        .ids
        .next_int("activity", 5_100_000_001);
    let now = world.clock.now_rfc3339();
    let user = auth_user(world).map(|u| str_of(&u["id"]));
    let doc = json!({
        "id": id.to_string(),
        "issue": issue_id,
        "type": kind,
        "data": data,
        "user": user,
        "dateCreated": now,
    });
    world
        .service_mut(NAME)
        .store
        .collection_mut("activities")
        .insert(format!("{issue_id}/{id}"), doc.clone());
    doc
}

fn activity_json(world: &WorldState, a: &Value) -> Value {
    let user = a["user"]
        .as_str()
        .and_then(|id| {
            docs(world, "users")
                .into_iter()
                .find(|u| str_of(&u["id"]) == id)
        })
        .map(crate::org::user_json)
        .unwrap_or(Value::Null);
    json!({
        "id": str_of(&a["id"]),
        "user": user,
        "sentry_app": null,
        "type": a["type"].clone(),
        "data": a.get("data").cloned().unwrap_or(json!({})),
        "dateCreated": a["dateCreated"].clone(),
    })
}

fn activity_for(world: &WorldState, issue: &Value) -> Vec<Value> {
    let id = str_of(&issue["id"]);
    let mut out: Vec<Value> = docs(world, "activities")
        .into_iter()
        .filter(|a| str_of(&a["issue"]) == id)
        .map(|a| activity_json(world, a))
        .collect();
    out.sort_by(|a, b| {
        value_time(&b["dateCreated"])
            .cmp(&value_time(&a["dateCreated"]))
            .then_with(|| str_of(&b["id"]).cmp(&str_of(&a["id"])))
    });
    out.push(json!({
        "id": "0",
        "user": null,
        "sentry_app": null,
        "type": "first_seen",
        "data": { "priority": issue.get("priority").cloned().unwrap_or(json!("medium")) },
        "dateCreated": issue["firstSeen"].clone(),
    }));
    out
}

pub async fn update_issue(
    State(world): State<SharedWorld>,
    Path((org, id)): Path<(String, String)>,
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
        return crate::util::not_found();
    }
    let Some(key) = issue_key(world, &id) else {
        return crate::util::not_found();
    };
    let Some(mut doc) = collection(world, "issues")
        .and_then(|c| c.get(&key))
        .cloned()
    else {
        return crate::util::not_found();
    };
    let issue_id = str_of(&doc["id"]);
    let now = world.clock.now_rfc3339();
    let now_unix = world.clock.now_unix();
    let mut activities: Vec<(&str, Value)> = Vec::new();

    if let Some(status) = input.get("status").filter(|s| !s.is_null()) {
        let Some(status) = status.as_str().filter(|s| STATUSES.contains(s)) else {
            return field_error(
                "status",
                &format!("\"{}\" is not a valid choice.", str_of(status)),
            );
        };
        if let Some(sub) = input.get("substatus").and_then(Value::as_str)
            && !SUBSTATUSES.contains(&sub)
        {
            return field_error("substatus", &format!("\"{sub}\" is not a valid choice."));
        }
        let actor = auth_user(world).map(|u| {
            json!({ "id": str_of(&u["id"]), "name": u["name"].clone(), "username": u["username"].clone(), "email": u["email"].clone() })
        });
        let details = input.get("statusDetails").cloned().unwrap_or(json!({}));
        match status {
            "resolved" => {
                doc["status"] = json!("resolved");
                doc["substatus"] = Value::Null;
                if let Some(rel) = details.get("inRelease").and_then(Value::as_str) {
                    doc["statusDetails"] = json!({ "inRelease": rel, "actor": actor });
                    activities.push(("set_resolved_in_release", json!({ "version": rel })));
                } else if details.get("inNextRelease") == Some(&json!(true)) {
                    doc["statusDetails"] = json!({ "inNextRelease": true, "actor": actor });
                    activities.push(("set_resolved_in_release", json!({ "inNextRelease": true })));
                } else {
                    doc["statusDetails"] = json!({});
                    activities.push(("set_resolved", json!({})));
                }
                doc["resolvedAt"] = json!(now);
            }
            "resolvedInNextRelease" => {
                doc["status"] = json!("resolved");
                doc["substatus"] = Value::Null;
                doc["statusDetails"] = json!({ "inNextRelease": true, "actor": actor });
                doc["resolvedAt"] = json!(now);
                activities.push(("set_resolved_in_release", json!({ "inNextRelease": true })));
            }
            "unresolved" | "reprocessing" => {
                doc["status"] = json!("unresolved");
                doc["substatus"] = json!("ongoing");
                doc["statusDetails"] = json!({});
                if let Some(o) = doc.as_object_mut() {
                    o.remove("resolvedAt");
                }
                activities.push(("set_unresolved", json!({})));
            }
            _ => {
                let num = |k: &str| {
                    input
                        .get(k)
                        .or_else(|| details.get(k))
                        .and_then(Value::as_u64)
                };
                let mut sd = Map::new();
                let substatus = if let Some(mins) = num("ignoreDuration") {
                    sd.insert("ignoreDuration".into(), json!(mins));
                    sd.insert(
                        "ignoreUntil".into(),
                        json!(mock_core::clock::format_unix(now_unix + mins as i64 * 60)),
                    );
                    "archived_until_condition_met"
                } else if let Some(count) = num("ignoreCount") {
                    sd.insert("ignoreCount".into(), json!(count));
                    sd.insert(
                        "ignoreWindow".into(),
                        num("ignoreWindow").map_or(Value::Null, Value::from),
                    );
                    "archived_until_condition_met"
                } else if let Some(count) = num("ignoreUserCount") {
                    sd.insert("ignoreUserCount".into(), json!(count));
                    sd.insert(
                        "ignoreUserWindow".into(),
                        num("ignoreUserWindow").map_or(Value::Null, Value::from),
                    );
                    "archived_until_condition_met"
                } else {
                    match input.get("substatus").and_then(Value::as_str) {
                        Some("archived_forever") => "archived_forever",
                        _ => {
                            sd.insert("ignoreUntilEscalating".into(), json!(true));
                            "archived_until_escalating"
                        }
                    }
                };
                if let Some(a) = actor {
                    sd.insert("actor".into(), a);
                }
                doc["status"] = json!("ignored");
                doc["substatus"] = json!(substatus);
                doc["statusDetails"] = Value::Object(sd.clone());
                activities.push(("set_ignored", Value::Object(sd)));
            }
        }
    } else if let Some(sub) = input.get("substatus").and_then(Value::as_str) {
        if !SUBSTATUSES.contains(&sub) {
            return field_error("substatus", &format!("\"{sub}\" is not a valid choice."));
        }
        doc["substatus"] = json!(sub);
    }

    if let Some(assignee) = input.get("assignedTo") {
        let actor = match assignee {
            Value::Null => Value::Null,
            Value::String(s) => match resolve_actor(world, s) {
                Ok(a) => a,
                Err(r) => return r,
            },
            _ => return field_error("assignedTo", "Unknown actor input"),
        };
        if actor.is_null() {
            if !doc["assignedTo"].is_null() {
                activities.push(("unassigned", json!({})));
            }
        } else {
            activities.push((
                "assigned",
                json!({
                    "assignee": actor["id"].clone(),
                    "assigneeType": actor["type"].clone(),
                    "assigneeEmail": actor.get("email").cloned().unwrap_or(Value::Null),
                }),
            ));
        }
        doc["assignedTo"] = actor;
    }
    for field in ["hasSeen", "isBookmarked", "isSubscribed"] {
        if let Some(v) = input.get(field).filter(|v| v.is_boolean()) {
            doc[field] = v.clone();
        }
    }
    if let Some(p) = input.get("priority").and_then(Value::as_str) {
        if !["high", "medium", "low"].contains(&p) {
            return field_error("priority", &format!("\"{p}\" is not a valid choice."));
        }
        doc["priority"] = json!(p);
    }
    for (kind, data) in activities {
        add_activity(world, &issue_id, kind, data);
    }
    world
        .service_mut(NAME)
        .store
        .collection_mut("issues")
        .insert(key, doc.clone());
    ok(issue_detail_json(world, &doc, &base_url(&headers)))
}

// ---------------------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------------------

fn frame_json(f: &Value) -> Value {
    let get = |k: &str| f.get(k).cloned().unwrap_or(Value::Null);
    json!({
        "filename": get("filename"),
        "absPath": f.get("absPath").cloned().unwrap_or_else(|| get("filename")),
        "module": get("module"),
        "package": get("package"),
        "platform": get("platform"),
        "instructionAddr": null,
        "symbolAddr": null,
        "function": get("function"),
        "rawFunction": null,
        "symbol": null,
        "context": f.get("context").cloned().unwrap_or(json!([])),
        "lineNo": get("lineNo"),
        "colNo": get("colNo"),
        "inApp": f.get("inApp").cloned().unwrap_or(json!(true)),
        "trust": null,
        "errors": null,
        "lock": null,
        "sourceLink": null,
        "vars": get("vars"),
    })
}

fn entries(e: &Value) -> Vec<Value> {
    if let Some(entries) = e.get("entries").and_then(Value::as_array) {
        return entries.clone();
    }
    let mut out = Vec::new();
    let exc = &e["exception"];
    if exc.is_object() {
        let frames: Vec<Value> = exc["frames"]
            .as_array()
            .map(|f| f.iter().map(frame_json).collect())
            .unwrap_or_default();
        out.push(json!({
            "type": "exception",
            "data": {
                "values": [{
                    "type": exc["type"].clone(),
                    "value": exc["value"].clone(),
                    "mechanism": exc.get("mechanism").cloned().unwrap_or(json!({ "type": "generic", "handled": false })),
                    "threadId": null,
                    "module": exc.get("module").cloned().unwrap_or(Value::Null),
                    "stacktrace": if frames.is_empty() { Value::Null } else { json!({
                        "frames": frames, "framesOmitted": null, "registers": null, "hasSystemFrames": true
                    }) },
                    "rawStacktrace": null,
                }],
                "hasSystemFrames": true,
                "excOmitted": null,
            }
        }));
    } else if let Some(msg) = e["message"].as_str() {
        out.push(json!({ "type": "message", "data": { "formatted": msg, "message": null, "params": [] } }));
    }
    if let Some(threads) = e["threads"].as_array() {
        let values: Vec<Value> = threads
            .iter()
            .map(|t| {
                let frames: Vec<Value> = t["frames"]
                    .as_array()
                    .map(|f| f.iter().map(frame_json).collect())
                    .unwrap_or_default();
                json!({
                    "id": t.get("id").cloned().unwrap_or(json!(0)),
                    "name": t.get("name").cloned().unwrap_or(Value::Null),
                    "current": t.get("current").cloned().unwrap_or(json!(false)),
                    "crashed": t.get("crashed").cloned().unwrap_or(json!(false)),
                    "state": t.get("state").cloned().unwrap_or(Value::Null),
                    "stacktrace": if frames.is_empty() { Value::Null } else { json!({
                        "frames": frames, "framesOmitted": null, "registers": null, "hasSystemFrames": true
                    }) },
                    "rawStacktrace": null,
                })
            })
            .collect();
        out.push(json!({ "type": "threads", "data": { "values": values } }));
    }
    if let Some(crumbs) = e["breadcrumbs"].as_array() {
        let values: Vec<Value> = crumbs
            .iter()
            .map(|b| {
                json!({
                    "timestamp": b.get("timestamp").cloned().unwrap_or(Value::Null),
                    "type": b.get("type").cloned().unwrap_or(json!("default")),
                    "category": b.get("category").cloned().unwrap_or(Value::Null),
                    "level": b.get("level").cloned().unwrap_or(json!("info")),
                    "message": b.get("message").cloned().unwrap_or(Value::Null),
                    "data": b.get("data").cloned().unwrap_or(Value::Null),
                    "event_id": null,
                })
            })
            .collect();
        out.push(json!({ "type": "breadcrumbs", "data": { "values": values } }));
    }
    let req = &e["request"];
    if req.is_object() {
        out.push(json!({
            "type": "request",
            "data": {
                "method": req.get("method").cloned().unwrap_or(json!("GET")),
                "url": req["url"].clone(),
                "query": req.get("query").cloned().unwrap_or(json!([])),
                "fragment": null,
                "data": req.get("data").cloned().unwrap_or(Value::Null),
                "headers": req.get("headers").cloned().unwrap_or(json!([])),
                "cookies": [],
                "env": null,
                "inferredContentType": null,
                "apiTarget": null,
            }
        }));
    }
    out
}

fn event_contexts(e: &Value) -> Value {
    let mut ctx = e["contexts"].as_object().cloned().unwrap_or_default();
    if let Some(trace) = e["trace"].as_str()
        && !ctx.contains_key("trace")
    {
        ctx.insert(
            "trace".into(),
            json!({
                "trace_id": trace,
                "span_id": e.get("spanId").cloned().unwrap_or(json!(&trace[..16.min(trace.len())])),
                "op": e.get("transactionOp").cloned().unwrap_or(json!("http.server")),
                "status": "internal_error",
                "type": "trace",
            }),
        );
    }
    Value::Object(ctx)
}

pub fn event_json(world: &WorldState, e: &Value) -> Value {
    let project = e["project"].as_str().unwrap_or("");
    let project_id = find_project(world, project).map_or(String::new(), |p| str_of(&p["id"]));
    let exc = &e["exception"];
    let mut metadata = json!({});
    if exc.is_object() {
        metadata = json!({ "type": exc["type"].clone(), "value": exc["value"].clone() });
        if let Some(top) = exc["frames"].as_array().and_then(|f| f.last()) {
            metadata["filename"] = top["filename"].clone();
            metadata["function"] = top["function"].clone();
        }
    }
    let release = match e["release"].as_str() {
        Some(v) => {
            let mut r = release_json(world, &json!(v));
            if r.get("lastCommit").is_none() {
                r["lastCommit"] = Value::Null;
            }
            r
        }
        None => Value::Null,
    };
    json!({
        "id": e["eventID"].clone(),
        "groupID": str_of(&e["groupID"]),
        "eventID": e["eventID"].clone(),
        "projectID": project_id,
        "size": e.get("size").cloned().unwrap_or(json!(4096)),
        "entries": entries(e),
        "dist": null,
        "message": e.get("message").cloned().unwrap_or(json!("")),
        "title": e["title"].clone(),
        "location": e.get("location").cloned().unwrap_or_else(|| metadata.get("filename").cloned().unwrap_or(Value::Null)),
        "user": e.get("user").cloned().unwrap_or(Value::Null),
        "contexts": event_contexts(e),
        "sdk": e.get("sdk").cloned().unwrap_or(Value::Null),
        "context": e.get("extra").cloned().unwrap_or(json!({})),
        "packages": {},
        "type": e.get("type").cloned().unwrap_or(json!("error")),
        "metadata": metadata,
        "tags": event_tags(world, e),
        "platform": e.get("platform").cloned().unwrap_or(Value::Null),
        "dateReceived": e["dateCreated"].clone(),
        "errors": [],
        "occurrence": null,
        "_meta": {},
        "crashFile": null,
        "culprit": e.get("culprit").cloned().unwrap_or(Value::Null),
        "dateCreated": e["dateCreated"].clone(),
        "fingerprints": [],
        "groupingConfig": { "id": "newstyle:2023-01-11", "enhancements": "" },
        "release": release,
        "userReport": null,
        "sdkUpdates": [],
        "resolvedWith": [],
        "nextEventID": null,
        "previousEventID": null,
    })
}

/// The flat row the issue-events list returns.
fn event_list_item(world: &WorldState, e: &Value) -> Value {
    let project = e["project"].as_str().unwrap_or("");
    json!({
        "id": e["eventID"].clone(),
        "eventID": e["eventID"].clone(),
        "groupID": str_of(&e["groupID"]),
        "projectID": find_project(world, project).map_or(String::new(), |p| str_of(&p["id"])),
        "title": e["title"].clone(),
        "message": e.get("message").cloned().unwrap_or(json!("")),
        "platform": e.get("platform").cloned().unwrap_or(Value::Null),
        "dateCreated": e["dateCreated"].clone(),
        "event.type": e.get("type").cloned().unwrap_or(json!("error")),
        "culprit": e.get("culprit").cloned().unwrap_or(Value::Null),
        "location": e.get("location").cloned().unwrap_or(Value::Null),
        "crashFile": null,
        "user": e.get("user").cloned().unwrap_or(Value::Null),
        "tags": event_tags(world, e),
    })
}

pub async fn list_issue_events(
    State(world): State<SharedWorld>,
    Path((org, id)): Path<(String, String)>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return crate::util::not_found();
    }
    let Some(issue) = find_issue(&world, &id) else {
        return crate::util::not_found();
    };
    let now = now(&world);
    let (from, to) = if q.non_empty("statsPeriod").is_some() || q.non_empty("start").is_some() {
        match time_window(&q, now, "90d") {
            Ok(w) => w,
            Err(r) => return r,
        }
    } else {
        (i64::MIN, i64::MAX)
    };
    let terms = search::parse(q.get("query").unwrap_or(""));
    let environments = q.all("environment");
    let mut events: Vec<Value> = issue_events_or_synthetic(&world, issue)
        .into_iter()
        .filter(|e| {
            let t = value_time(&e["dateCreated"]).unwrap_or(0);
            t >= from && t <= to
        })
        .filter(|e| {
            environments.is_empty()
                || e["environment"]
                    .as_str()
                    .is_some_and(|env| environments.contains(&env))
        })
        .filter(|e| {
            let row = crate::discover::error_row(&world, e);
            crate::discover::row_matches(&row, &terms, now)
        })
        .collect();
    let asc = q
        .get("sort")
        .is_some_and(|s| s == "timestamp" || s == "date");
    if !asc {
        events.reverse();
    }
    let full = q.get("full").is_some_and(|f| f == "true" || f == "1");
    let out: Vec<Value> = events
        .iter()
        .map(|e| {
            if full {
                event_json(&world, e)
            } else {
                event_list_item(&world, e)
            }
        })
        .collect();
    paginate(out, &q, &uri, &headers, 50, 100)
}

pub async fn get_issue_event(
    State(world): State<SharedWorld>,
    Path((org, id, event_id)): Path<(String, String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return crate::util::not_found();
    }
    let Some(issue) = find_issue(&world, &id) else {
        return crate::util::not_found();
    };
    let events = issue_events_or_synthetic(&world, issue);
    let found = match event_id.as_str() {
        "latest" | "recommended" => events.last().cloned(),
        "oldest" => events.first().cloned(),
        eid => events
            .iter()
            .find(|e| {
                e["eventID"]
                    .as_str()
                    .is_some_and(|x| x.eq_ignore_ascii_case(eid))
            })
            .cloned(),
    };
    match found {
        Some(mut e) => {
            let idx = events
                .iter()
                .position(|x| x["eventID"] == e["eventID"])
                .unwrap_or(0);
            let mut body = event_json(&world, &e);
            body["previousEventID"] = idx
                .checked_sub(1)
                .and_then(|i| events.get(i))
                .map_or(Value::Null, |p| p["eventID"].clone());
            body["nextEventID"] = events
                .get(idx + 1)
                .map_or(Value::Null, |n| n["eventID"].clone());
            e = body;
            ok(e)
        }
        None => crate::util::not_found(),
    }
}

pub async fn get_project_event(
    State(world): State<SharedWorld>,
    Path((org, project, event_id)): Path<(String, String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return crate::util::not_found();
    }
    let Some(p) = find_project(&world, &project) else {
        return crate::util::not_found();
    };
    let found = docs(&world, "events").into_iter().find(|e| {
        e["eventID"]
            .as_str()
            .is_some_and(|x| x.eq_ignore_ascii_case(&event_id))
            && e["project"] == p["slug"]
    });
    match found {
        Some(e) => ok(event_json(&world, e)),
        None => crate::util::not_found(),
    }
}

// ---------------------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------------------

/// `(value, count, firstSeen, lastSeen)` for one tag key of an issue.
fn tag_distribution(
    world: &WorldState,
    issue: &Value,
    key: &str,
) -> Vec<(String, u64, Value, Value)> {
    if let Some(dist) = issue["tags"].get(key).and_then(Value::as_array) {
        let mut out: Vec<(String, u64, Value, Value)> = dist
            .iter()
            .filter_map(|t| {
                Some((
                    t["value"].as_str()?.to_owned(),
                    t["count"].as_u64().unwrap_or(0),
                    t.get("firstSeen").cloned().unwrap_or(Value::Null),
                    t.get("lastSeen").cloned().unwrap_or(Value::Null),
                ))
            })
            .collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        return out;
    }
    let mut acc: BTreeMap<String, (u64, Value, Value)> = BTreeMap::new();
    for e in issue_events(world, issue) {
        if let Some(v) = tag_value(world, e, key) {
            let entry =
                acc.entry(v)
                    .or_insert((0, e["dateCreated"].clone(), e["dateCreated"].clone()));
            entry.0 += 1;
            entry.2 = e["dateCreated"].clone();
        }
    }
    let mut out: Vec<(String, u64, Value, Value)> =
        acc.into_iter().map(|(v, (c, f, l))| (v, c, f, l)).collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

fn tag_keys(world: &WorldState, issue: &Value) -> Vec<String> {
    let mut keys: Vec<String> = issue["tags"]
        .as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    for e in issue_events(world, issue) {
        for t in event_tags(world, e) {
            if let Some(k) = t["key"].as_str()
                && !keys.iter().any(|x| x == k)
            {
                keys.push(k.to_owned());
            }
        }
    }
    keys.sort();
    keys
}

fn tag_name(key: &str) -> String {
    match key {
        "environment" => "Environment".into(),
        "release" => "Release".into(),
        "level" => "Level".into(),
        "user" => "User".into(),
        "url" => "URL".into(),
        "browser" => "Browser".into(),
        "os" => "OS".into(),
        "transaction" => "Transaction".into(),
        k => k.to_owned(),
    }
}

fn tag_summary(world: &WorldState, issue: &Value, key: &str, top: usize) -> Value {
    let dist = tag_distribution(world, issue, key);
    let total: u64 = dist.iter().map(|d| d.1).sum();
    let unique = dist.len();
    let top_values: Vec<Value> = dist
        .into_iter()
        .take(if top == 0 { usize::MAX } else { top })
        .map(|(v, c, f, l)| {
            json!({ "key": key, "name": v, "value": v, "count": c, "firstSeen": f, "lastSeen": l })
        })
        .collect();
    json!({
        "key": key,
        "name": tag_name(key),
        "totalValues": total,
        "uniqueValues": unique,
        "topValues": top_values,
    })
}

fn tag_summaries(world: &WorldState, issue: &Value, top: usize) -> Vec<Value> {
    tag_keys(world, issue)
        .iter()
        .map(|k| {
            let mut s = tag_summary(world, issue, k, if top == 0 { 9 } else { top });
            if top == 0
                && let Some(o) = s.as_object_mut()
            {
                o.remove("topValues");
            }
            s
        })
        .collect()
}

pub async fn list_issue_tags(
    State(world): State<SharedWorld>,
    Path((org, id)): Path<(String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return crate::util::not_found();
    }
    let Some(issue) = find_issue(&world, &id) else {
        return crate::util::not_found();
    };
    ok(Value::Array(
        tag_keys(&world, issue)
            .iter()
            .map(|k| tag_summary(&world, issue, k, 9))
            .collect(),
    ))
}

pub async fn get_issue_tag(
    State(world): State<SharedWorld>,
    Path((org, id, key)): Path<(String, String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return crate::util::not_found();
    }
    let Some(issue) = find_issue(&world, &id) else {
        return crate::util::not_found();
    };
    if tag_distribution(&world, issue, &key).is_empty() {
        return crate::util::not_found();
    }
    ok(tag_summary(&world, issue, &key, 9))
}

pub async fn list_issue_tag_values(
    State(world): State<SharedWorld>,
    Path((org, id, key)): Path<(String, String, String)>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return crate::util::not_found();
    }
    let Some(issue) = find_issue(&world, &id) else {
        return crate::util::not_found();
    };
    let dist = tag_distribution(&world, issue, &key);
    if dist.is_empty() {
        return crate::util::not_found();
    }
    let rows: Vec<Value> = dist
        .into_iter()
        .map(|(v, c, f, l)| json!({ "key": key, "name": v, "value": v, "count": c, "firstSeen": f, "lastSeen": l }))
        .collect();
    paginate(rows, &q, &uri, &headers, 100, 1000)
}

// ---------------------------------------------------------------------------------------
// Activity, notes, links, user reports
// ---------------------------------------------------------------------------------------

pub async fn list_activities(
    State(world): State<SharedWorld>,
    Path((org, id)): Path<(String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return crate::util::not_found();
    }
    match find_issue(&world, &id) {
        Some(issue) => ok(json!({ "activity": activity_for(&world, issue) })),
        None => crate::util::not_found(),
    }
}

pub async fn list_notes(
    State(world): State<SharedWorld>,
    Path((org, id)): Path<(String, String)>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return crate::util::not_found();
    }
    let Some(issue) = find_issue(&world, &id) else {
        return crate::util::not_found();
    };
    let notes: Vec<Value> = activity_for(&world, issue)
        .into_iter()
        .filter(|a| a["type"] == "note")
        .collect();
    paginate(notes, &q, &uri, &headers, 100, 100)
}

pub async fn create_note(
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
    if !org_exists(world, &org) {
        return crate::util::not_found();
    }
    let Some(issue) = find_issue(world, &id).cloned() else {
        return crate::util::not_found();
    };
    let Some(text) = input["text"].as_str().filter(|t| !t.trim().is_empty()) else {
        return field_error("text", "This field is required.");
    };
    let doc = add_activity(
        world,
        &str_of(&issue["id"]),
        "note",
        json!({ "text": text }),
    );
    created(activity_json(world, &doc))
}

fn external_issues<'a>(world: &'a WorldState, issue: &Value) -> Vec<&'a Value> {
    docs(world, "external_issues")
        .into_iter()
        .filter(|x| str_of(&x["issueId"]) == str_of(&issue["id"]))
        .collect()
}

pub async fn list_external_issues(
    State(world): State<SharedWorld>,
    Path((org, id)): Path<(String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return crate::util::not_found();
    }
    match find_issue(&world, &id) {
        Some(issue) => ok(Value::Array(
            external_issues(&world, issue)
                .into_iter()
                .cloned()
                .collect(),
        )),
        None => crate::util::not_found(),
    }
}

pub async fn list_user_reports(
    State(world): State<SharedWorld>,
    Path((org, id)): Path<(String, String)>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return crate::util::not_found();
    }
    let Some(issue) = find_issue(&world, &id) else {
        return crate::util::not_found();
    };
    let reports: Vec<Value> = docs(&world, "user_reports")
        .into_iter()
        .filter(|r| str_of(&r["issue"]) == str_of(&issue["id"]))
        .map(|r| {
            let mut r = r.clone();
            if let Some(o) = r.as_object_mut() {
                o.remove("issue");
            }
            r
        })
        .collect();
    paginate(reports, &q, &uri, &headers, 100, 100)
}

// ---------------------------------------------------------------------------------------
// Seer autofix
// ---------------------------------------------------------------------------------------

pub async fn get_autofix(
    State(world): State<SharedWorld>,
    Path((org, id)): Path<(String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return crate::util::not_found();
    }
    let Some(issue) = find_issue(&world, &id) else {
        return crate::util::not_found();
    };
    let run = collection(&world, "autofix")
        .and_then(|c| c.get(&str_of(&issue["id"])))
        .cloned()
        .unwrap_or(Value::Null);
    ok(json!({ "autofix": run }))
}

/// Root cause derived from the issue's latest event when the dataset has no prepared analysis.
fn derived_analysis(world: &WorldState, issue: &Value) -> Value {
    let events = issue_events_or_synthetic(world, issue);
    let latest = events.last().cloned().unwrap_or(Value::Null);
    let exc = &latest["exception"];
    let frame = exc["frames"]
        .as_array()
        .and_then(|f| f.iter().rev().find(|fr| fr["inApp"] != json!(false)))
        .cloned()
        .unwrap_or(Value::Null);
    let location = match (frame["function"].as_str(), frame["filename"].as_str()) {
        (Some(func), Some(file)) => format!(
            " in `{func}` ({file}{})",
            frame["lineNo"]
                .as_u64()
                .map(|l| format!(":{l}"))
                .unwrap_or_default()
        ),
        (None, Some(file)) => format!(" in {file}"),
        _ => issue["culprit"]
            .as_str()
            .map(|c| format!(" in {c}"))
            .unwrap_or_default(),
    };
    let headline = match (exc["type"].as_str(), exc["value"].as_str()) {
        (Some(t), Some(v)) => format!("{t}: {v}"),
        _ => str_of(&issue["title"]),
    };
    json!([{
        "key": "root_cause",
        "data": {
            "one_line_description": format!("{headline} is raised{location}."),
            "five_whys": [
                format!("The application raises {headline}{location}."),
                "The failing code path does not guard against the unexpected value it receives.".to_owned(),
            ],
            "reproduction_steps": [],
        }
    }])
}

pub async fn start_autofix(
    State(world): State<SharedWorld>,
    Path((org, id)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    if let Err(r) = parse_body(&body) {
        return r;
    }
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    if !org_exists(world, &org) {
        return crate::util::not_found();
    }
    let Some(issue) = find_issue(world, &id).cloned() else {
        return crate::util::not_found();
    };
    let issue_id = str_of(&issue["id"]);
    if let Some(existing) = collection(world, "autofix").and_then(|c| c.get(&issue_id))
        && let Some(run_id) = existing["run_id"].as_u64()
    {
        return ok(json!({ "run_id": run_id }));
    }
    let run_id = world.service_mut(NAME).ids.next_int("autofix_run", 10_001);
    let now = world.clock.now_rfc3339();
    let artifacts = collection(world, "seer_analyses")
        .and_then(|c| {
            c.get(&issue_id)
                .or_else(|| c.get(issue["shortId"].as_str().unwrap_or("")))
        })
        .and_then(|a| a.get("artifacts").cloned())
        .unwrap_or_else(|| derived_analysis(world, &issue));
    let run = json!({
        "run_id": run_id,
        "updated_at": now,
        "status": "completed",
        "blocks": [{ "artifacts": artifacts, "todos": [] }],
        "pending_user_input": null,
        "repo_pr_states": {},
        "coding_agents": {},
    });
    world
        .service_mut(NAME)
        .store
        .collection_mut("autofix")
        .insert(issue_id, run);
    json_status(StatusCode::ACCEPTED, json!({ "run_id": run_id }))
}

pub async fn replay_count(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    RawQuery(raw): RawQuery,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return crate::util::not_found();
    }
    let query = q.get("query").unwrap_or("");
    let ids: Vec<String> = query
        .split_once(':')
        .map(|(_, v)| {
            v.trim_matches(|c| c == '[' || c == ']')
                .split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let return_ids = q.get("returnIds").is_some_and(|v| v == "true" || v == "1");
    let mut out = Map::new();
    for id in ids {
        let replays: Vec<Value> = docs(&world, "replays")
            .into_iter()
            .filter(|r| {
                r["issues"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|x| str_of(x) == id))
            })
            .map(|r| r["id"].clone())
            .collect();
        if replays.is_empty() {
            continue;
        }
        out.insert(
            id,
            if return_ids {
                Value::Array(replays)
            } else {
                json!(replays.len())
            },
        );
    }
    ok(Value::Object(out))
}
