//! Grafana IRM: the app's plugin settings, the OnCall public API (served under `/oncall`,
//! which the settings advertise as `onCallApiUrl`), and the Incident API
//! (`/api/plugins/grafana-irm-app/resources/api/v1/<Service>.<Method>`).

use axum::{
    http::{Method, StatusCode},
    response::Response,
};
use mock_core::http::json;
use serde_json::{Value, json};

use crate::{Ctx, Req, not_found, ok};

pub(crate) fn hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

pub(crate) fn settings(ctx: &Ctx, req: &Req) -> Response {
    if ctx.get("plugins", "grafana-irm-app").is_none() {
        return not_found("Plugin not found, no installed plugin with that id");
    }
    ok(json!({
        "name": "Grafana IRM", "type": "app", "id": "grafana-irm-app", "enabled": true, "pinned": true, "autoEnabled": false,
        "info": {"author": {"name": "Grafana Labs", "url": "https://grafana.com"}, "description": "Incident response and on-call management", "links": [], "logos": {"small": "public/plugins/grafana-irm-app/img/logo.svg", "large": "public/plugins/grafana-irm-app/img/logo.svg"}, "build": {}, "screenshots": null, "version": "1.18.3", "updated": "2026-09-30", "keywords": ["oncall", "incident"]},
        "dependencies": {"grafanaDependency": ">=11.3.0", "grafanaVersion": "*", "plugins": []},
        "latestVersion": "1.18.3", "hasUpdate": false, "defaultNavUrl": "/a/grafana-irm-app/alert-groups", "category": "", "state": "", "signature": "valid", "signatureType": "grafana", "signatureOrg": "Grafana Labs",
        "module": "public/plugins/grafana-irm-app/module.js", "baseUrl": "public/plugins/grafana-irm-app", "includes": [],
        "jsonData": {"onCallApiUrl": format!("http://{}/oncall", req.host()), "stackId": 5, "orgId": 1, "license": "OpenSource"},
        "secureJsonFields": {"onCallApiToken": true},
    }))
}

// ───────────────────────────── OnCall public API ─────────────────────────────

fn page(req: &Req, host: &str, path: &str, items: Vec<Value>) -> Value {
    let per_page = 50;
    let page = req
        .q("page")
        .and_then(|p| p.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    let total = items.len();
    let pages = total.div_ceil(per_page).max(1);
    let results: Vec<Value> = items
        .into_iter()
        .skip((page - 1) * per_page)
        .take(per_page)
        .collect();
    let link = |p: usize| json!(format!("http://{host}/oncall/api/v1/{path}/?page={p}"));
    json!({
        "count": total,
        "next": if page < pages { link(page + 1) } else { Value::Null },
        "previous": if page > 1 { link(page - 1) } else { Value::Null },
        "results": results,
        "page_size": per_page,
        "current_page_number": page,
        "total_pages": pages,
    })
}

fn oncall_404() -> Response {
    json(StatusCode::NOT_FOUND, json!({"detail": "Not found."}))
}

pub(crate) fn oncall(ctx: &mut Ctx, req: &Req, rest: &[&str]) -> Response {
    let host = req.host();
    let filtered = |coll: &str, field: &str, param: &str| -> Vec<Value> {
        ctx.coll(coll)
            .into_iter()
            .map(|(_, v)| v)
            .filter(|v| req.q(param).is_none_or(|p| v[field].as_str() == Some(p)))
            .collect()
    };
    match (req.method.clone(), rest) {
        (Method::GET, ["schedules"]) => {
            let mut items = filtered("oncall_schedules", "team_id", "team_id");
            if let Some(n) = req.q("name") {
                items.retain(|s| s["name"] == n);
            }
            ok(page(req, &host, "schedules", items))
        }
        (Method::GET, ["schedules", id]) => {
            ctx.get("oncall_schedules", id).map_or_else(oncall_404, ok)
        }
        (Method::GET, ["on_call_shifts"]) => {
            let mut items = filtered("oncall_shifts", "team_id", "team_id");
            if let Some(s) = req.q("schedule_id") {
                let ids: Vec<String> = ctx
                    .get("oncall_schedules", s)
                    .and_then(|x| x["shifts"].as_array().cloned())
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect();
                items.retain(|x| x["id"].as_str().is_some_and(|i| ids.iter().any(|j| j == i)));
            }
            ok(page(req, &host, "on_call_shifts", items))
        }
        (Method::GET, ["on_call_shifts", id]) => {
            ctx.get("oncall_shifts", id).map_or_else(oncall_404, ok)
        }
        (Method::GET, ["users"]) => {
            let mut items: Vec<Value> = ctx
                .coll("oncall_users")
                .into_iter()
                .map(|(_, v)| v)
                .collect();
            if let Some(u) = req.q("username") {
                items.retain(|x| x["username"] == u);
            }
            ok(page(req, &host, "users", items))
        }
        (Method::GET, ["users", id]) => {
            if *id == "current" {
                return ctx
                    .coll("oncall_users")
                    .into_iter()
                    .next()
                    .map_or_else(oncall_404, |(_, u)| ok(u));
            }
            ctx.get("oncall_users", id).map_or_else(oncall_404, ok)
        }
        (Method::GET, ["teams"]) => {
            let mut items: Vec<Value> = ctx
                .coll("oncall_teams")
                .into_iter()
                .map(|(_, v)| v)
                .collect();
            if let Some(n) = req.q("name") {
                items.retain(|x| x["name"] == n);
            }
            ok(page(req, &host, "teams", items))
        }
        (Method::GET, ["teams", id]) => ctx.get("oncall_teams", id).map_or_else(oncall_404, ok),
        (Method::GET, ["alert_groups"]) => {
            let mut items: Vec<Value> = ctx
                .coll("oncall_alert_groups")
                .into_iter()
                .map(|(_, v)| v)
                .collect();
            for (param, field) in [
                ("id", "id"),
                ("route_id", "route_id"),
                ("integration_id", "integration_id"),
                ("team_id", "team_id"),
                ("state", "state"),
            ] {
                if let Some(v) = req.q(param) {
                    items.retain(|x| x[field].as_str() == Some(v));
                }
            }
            if let Some(name) = req.q("name") {
                let n = name.to_lowercase();
                items.retain(|x| {
                    x["title"]
                        .as_str()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&n)
                });
            }
            for label in req.qs("label") {
                if let Some((k, v)) = label.split_once(':') {
                    items.retain(|x| {
                        x["labels"].as_array().is_some_and(|ls| {
                            ls.iter()
                                .any(|l| l["key"]["name"] == k && l["value"]["name"] == v)
                        })
                    });
                }
            }
            if let Some(range) = req.q("started_at")
                && let Some((from, to)) = range.split_once('_')
            {
                items.retain(|x| {
                    x["created_at"]
                        .as_str()
                        .is_some_and(|c| &c[..19] >= from && &c[..19] <= to)
                });
            }
            items.sort_by(|a, b| b["created_at"].as_str().cmp(&a["created_at"].as_str()));
            ok(page(
                req,
                &host,
                "alert_groups",
                items.into_iter().map(|g| public_alert_group(&g)).collect(),
            ))
        }
        (Method::GET, ["alert_groups", id]) => ctx
            .get("oncall_alert_groups", id)
            .map_or_else(oncall_404, |g| ok(public_alert_group(&g))),
        (Method::POST, ["alert_groups", id, action]) => alert_group_action(ctx, id, action),
        _ => oncall_404(),
    }
}

fn public_alert_group(g: &Value) -> Value {
    let mut v = g.clone();
    if let Some(m) = v.as_object_mut() {
        m.shift_remove("team_id");
    }
    v
}

fn alert_group_action(ctx: &mut Ctx, id: &str, action: &str) -> Response {
    let Some(mut g) = ctx.get("oncall_alert_groups", id) else {
        return oncall_404();
    };
    let now = ctx.now_str();
    let user = ctx
        .coll("oncall_users")
        .into_iter()
        .find(|(_, u)| u["role"] == "admin")
        .map(|(id, _)| id)
        .unwrap_or_default();
    let state = g["state"].as_str().unwrap_or("new").to_owned();
    let err = |msg: &str| json(StatusCode::BAD_REQUEST, json!({"detail": msg}));
    match action {
        "acknowledge" => {
            if state == "resolved" {
                return err("Can't acknowledge a resolved alert group. Unresolve it first.");
            }
            if state == "acknowledged" {
                return err("Can't acknowledge an acknowledged alert group.");
            }
            g["state"] = json!("acknowledged");
            g["acknowledged_at"] = json!(now);
            g["acknowledged_by"] = json!(user);
        }
        "unacknowledge" => {
            if state != "acknowledged" {
                return err("Can't unacknowledge an unacknowledged alert group.");
            }
            g["state"] = json!("new");
            g["acknowledged_at"] = Value::Null;
            g["acknowledged_by"] = Value::Null;
        }
        "resolve" => {
            if state == "resolved" {
                return err("Can't resolve a resolved alert group.");
            }
            g["state"] = json!("resolved");
            g["resolved_at"] = json!(now);
            g["resolved_by"] = json!(user);
        }
        "unresolve" => {
            if state != "resolved" {
                return err("Can't unresolve an unresolved alert group.");
            }
            g["state"] = json!(if g["acknowledged_at"].is_string() {
                "acknowledged"
            } else {
                "new"
            });
            g["resolved_at"] = Value::Null;
            g["resolved_by"] = Value::Null;
        }
        "silence" => {
            g["state"] = json!("silenced");
            g["silenced_at"] = json!(now);
        }
        "unsilence" => {
            g["state"] = json!("new");
            g["silenced_at"] = Value::Null;
        }
        _ => return oncall_404(),
    }
    ctx.put("oncall_alert_groups", id, g);
    json(StatusCode::OK, Value::Null)
}

// ───────────────────────────── Incident API ─────────────────────────────

fn incident_error(code: StatusCode, msg: &str) -> Response {
    json(code, json!({"error": msg}))
}

fn preview(i: &Value) -> Value {
    json!({
        "incidentID": i["incidentID"], "severityID": i["severityID"].as_str().unwrap_or_default(), "severityLabel": i["severity"],
        "incidentType": i["incidentType"].as_str().unwrap_or("internal"), "labels": i["labels"].clone().as_array().cloned().unwrap_or_default(), "isDrill": i["isDrill"].as_bool().unwrap_or(false),
        "createdTime": i["createdTime"], "modifiedTime": i["modifiedTime"], "closedTime": i["closedTime"].as_str().unwrap_or_default(),
        "createdByUser": i["createdByUser"], "title": i["title"], "description": i["description"].as_str().unwrap_or_default(), "summary": i["summary"].as_str().unwrap_or_default(),
        "status": i["status"], "slug": i["slug"].as_str().unwrap_or_default(), "incidentStart": i["incidentStart"].as_str().unwrap_or_default(), "incidentEnd": i["incidentEnd"].as_str().unwrap_or_default(),
        "fieldValues": i["fieldValues"].clone().as_array().cloned().unwrap_or_default(),
        "incidentMembershipPreview": {"importantAssignments": [], "totalAssignments": i["incidentMembership"]["totalAssignments"].as_i64().unwrap_or(0), "totalParticipants": i["incidentMembership"]["totalParticipants"].as_i64().unwrap_or(0)},
        "incidentChannels": [], "version": i["version"].as_i64().unwrap_or(1),
    })
}

fn full(i: &Value) -> Value {
    let mut v = i.clone();
    for (k, d) in [
        ("refs", json!([])),
        ("labels", json!([])),
        ("incidentChannels", json!([])),
        (
            "taskList",
            json!({"tasks": [], "todoCount": 0, "doneCount": 0}),
        ),
        (
            "incidentMembership",
            json!({"assignments": [], "totalAssignments": 0, "totalParticipants": 0}),
        ),
    ] {
        if v[k].is_null() {
            v[k] = d;
        }
    }
    if let Some(m) = v.as_object_mut() {
        m.shift_remove("fieldValues");
        m.shift_remove("severityID");
    }
    v
}

fn user_preview(ctx: &Ctx) -> Value {
    let u = ctx.user();
    json!({"userID": format!("grafana-incident:user-{}", u["id"]), "name": u["name"], "photoURL": format!("{}{}", crate::PUBLIC_URL, u["avatarUrl"].as_str().unwrap_or_default())})
}

pub(crate) fn incident(ctx: &mut Ctx, req: &Req, method: &str) -> Response {
    if req.method != Method::POST {
        return incident_error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
    }
    if ctx.get("plugins", "grafana-irm-app").is_none() {
        return not_found("Plugin not found");
    }
    let body = match req.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    let get_incident = |ctx: &Ctx, id: &str| ctx.get("incidents", id);
    match method {
        "IncidentsService.QueryIncidentPreviews" => {
            let q = &body["query"];
            let limit = q["limit"].as_u64().unwrap_or(10).max(1) as usize;
            let qs = q["queryString"].as_str().unwrap_or_default();
            let mut items: Vec<Value> = ctx.coll("incidents").into_iter().map(|(_, i)| i).collect();
            for term in qs.split_whitespace() {
                if let Some(v) = term.strip_prefix("isdrill:") {
                    let want = v == "true";
                    items.retain(|i| i["isDrill"].as_bool().unwrap_or(false) == want);
                } else if let Some(v) = term.strip_prefix("status:") {
                    items.retain(|i| i["status"] == v);
                } else if let Some(v) = term.strip_prefix("severity:") {
                    items.retain(|i| {
                        i["severity"]
                            .as_str()
                            .is_some_and(|s| s.eq_ignore_ascii_case(v))
                    });
                } else if let Some(v) = term.strip_prefix("label:") {
                    items.retain(|i| {
                        i["labels"]
                            .as_array()
                            .is_some_and(|ls| ls.iter().any(|l| l["label"] == v))
                    });
                } else {
                    let t = term.trim_matches('"').to_lowercase();
                    items.retain(|i| {
                        i["title"]
                            .as_str()
                            .unwrap_or_default()
                            .to_lowercase()
                            .contains(&t)
                    });
                }
            }
            let asc = q["orderDirection"] == "ASC";
            items.sort_by(|a, b| a["createdTime"].as_str().cmp(&b["createdTime"].as_str()));
            if !asc {
                items.reverse();
            }
            let has_more = items.len() > limit;
            let previews: Vec<Value> = items
                .iter()
                .take(limit)
                .map(|i| {
                    let mut p = preview(i);
                    if body["includeCustomFieldValues"] != true {
                        p["fieldValues"] = json!([]);
                    }
                    p
                })
                .collect();
            ok(
                json!({"incidentPreviews": previews, "query": q, "cursor": {"nextValue": if has_more { limit.to_string() } else { String::new() }, "hasMore": has_more}}),
            )
        }
        "IncidentsService.GetIncident" => {
            let id = body["incidentID"].as_str().unwrap_or_default();
            match get_incident(ctx, id) {
                Some(i) => ok(json!({"incident": full(&i)})),
                None => incident_error(StatusCode::NOT_FOUND, &format!("incident not found: {id}")),
            }
        }
        "IncidentsService.CreateIncident" => {
            let title = body["title"].as_str().unwrap_or_default().trim().to_owned();
            if title.is_empty() {
                return incident_error(StatusCode::BAD_REQUEST, "title is required");
            }
            let n = ctx
                .coll("incidents")
                .iter()
                .filter_map(|(k, _)| k.parse::<i64>().ok())
                .max()
                .unwrap_or(0)
                + 1;
            let id = n.to_string();
            let now = ctx.now_str();
            let status = body["status"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("active")
                .to_owned();
            let labels: Vec<Value> = body["labels"].as_array().cloned().unwrap_or_default().into_iter().map(|l| json!({"key": "", "label": l["label"], "description": l["description"].as_str().unwrap_or_default(), "colorHex": l["colorHex"].as_str().unwrap_or("#5794F2")})).collect();
            let slug = crate::url_encode(&title.to_lowercase().replace(' ', "-"));
            let inc = json!({
                "incidentID": id, "refs": [], "severity": body["severity"].as_str().unwrap_or("pending"), "labels": labels, "isDrill": body["isDrill"].as_bool().unwrap_or(false),
                "incidentType": "internal", "createdTime": now, "modifiedTime": now, "createdByUser": user_preview(ctx), "closedTime": "", "durationSeconds": 0,
                "status": status, "title": title, "overviewURL": format!("/a/grafana-irm-app/incidents/{id}/{slug}"), "slug": slug,
                "incidentMembership": {"assignments": [], "totalAssignments": 0, "totalParticipants": 1}, "taskList": {"tasks": [], "todoCount": 0, "doneCount": 0},
                "summary": "", "incidentStart": now, "incidentEnd": "", "incidentChannels": [], "version": 1, "fieldValues": [],
            });
            ctx.put("incidents", &id, inc.clone());
            if let Some(url) = body["attachURL"].as_str().filter(|u| !u.is_empty()) {
                add_activity(
                    ctx,
                    &id,
                    "attachment",
                    body["attachCaption"].as_str().unwrap_or(url),
                    Some(url),
                );
            }
            ok(json!({"incident": full(&inc)}))
        }
        "IncidentsService.UpdateStatus"
        | "IncidentsService.UpdateSeverity"
        | "IncidentsService.UpdateTitle"
        | "IncidentsService.UpdateIncidentIsDrill" => {
            let id = body["incidentID"].as_str().unwrap_or_default().to_owned();
            let Some(mut inc) = get_incident(ctx, &id) else {
                return incident_error(StatusCode::NOT_FOUND, &format!("incident not found: {id}"));
            };
            let now = ctx.now_str();
            let (field, value) = match method {
                "IncidentsService.UpdateStatus" => ("status", body["status"].clone()),
                "IncidentsService.UpdateSeverity" => ("severity", body["severity"].clone()),
                "IncidentsService.UpdateTitle" => ("title", body["title"].clone()),
                _ => ("isDrill", body["isDrill"].clone()),
            };
            if value.is_null() || value == "" {
                return incident_error(StatusCode::BAD_REQUEST, &format!("{field} is required"));
            }
            if field == "status" && !matches!(value.as_str(), Some("active" | "resolved")) {
                return incident_error(
                    StatusCode::BAD_REQUEST,
                    "invalid status: must be active or resolved",
                );
            }
            let old = inc[field].clone();
            inc[field] = value.clone();
            inc["modifiedTime"] = json!(now);
            inc["version"] = json!(inc["version"].as_i64().unwrap_or(1) + 1);
            if field == "status" {
                if value == "resolved" {
                    inc["closedTime"] = json!(now);
                    inc["incidentEnd"] = json!(now);
                } else {
                    inc["closedTime"] = json!("");
                    inc["incidentEnd"] = json!("");
                }
            }
            ctx.put("incidents", &id, inc.clone());
            let kind = match field {
                "status" => "statusChanged",
                "severity" => "severityChanged",
                "title" => "titleChanged",
                _ => "updatedIncident",
            };
            add_activity(
                ctx,
                &id,
                kind,
                &format!(
                    "{field} changed from {} to {}",
                    old.as_str().unwrap_or_default(),
                    value.as_str().unwrap_or_default()
                ),
                None,
            );
            ok(json!({"incident": full(&inc)}))
        }
        "ActivityService.AddActivity" => {
            let id = body["incidentID"].as_str().unwrap_or_default().to_owned();
            if get_incident(ctx, &id).is_none() {
                return incident_error(StatusCode::NOT_FOUND, &format!("incident not found: {id}"));
            }
            let text = body["body"].as_str().unwrap_or_default().to_owned();
            if text.trim().is_empty() {
                return incident_error(StatusCode::BAD_REQUEST, "body is required");
            }
            let kind = body["activityKind"]
                .as_str()
                .filter(|k| !k.is_empty())
                .unwrap_or("userNote")
                .to_owned();
            let item = add_activity(ctx, &id, &kind, &text, None);
            ok(json!({"activityItem": item}))
        }
        "ActivityService.QueryActivity" => {
            let id = body["query"]["incidentID"].as_str().unwrap_or_default();
            let items: Vec<Value> = ctx
                .coll("incident_activity")
                .into_iter()
                .map(|(_, a)| a)
                .filter(|a| a["incidentID"] == id)
                .collect();
            ok(
                json!({"activityItems": items, "query": body["query"], "cursor": {"nextValue": "", "hasMore": false}}),
            )
        }
        "FieldsService.GetFields" => ok(
            json!({"fields": ctx.coll("incident_fields").into_iter().map(|(_, f)| f).collect::<Vec<_>>()}),
        ),
        "FieldsService.GetFieldValues" => {
            let id = body["targetID"]
                .as_str()
                .or(body["incidentID"].as_str())
                .unwrap_or_default();
            let Some(inc) = get_incident(ctx, id) else {
                return incident_error(StatusCode::NOT_FOUND, &format!("incident not found: {id}"));
            };
            let fields = ctx.coll("incident_fields");
            let values: Vec<Value> = inc["fieldValues"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|fv| {
                    let def = fields
                        .iter()
                        .find(|(_, f)| f["uuid"] == fv["fieldUUID"])
                        .map(|(_, f)| f.clone())
                        .unwrap_or(Value::Null);
                    json!({"field": def, "value": fv["value"]})
                })
                .collect();
            ok(json!({"fieldValues": values}))
        }
        "FieldsService.RecordFieldValue"
        | "FieldsService.SetFieldValue"
        | "FieldsService.SetFieldValues" => {
            let id = body["targetID"]
                .as_str()
                .or(body["incidentID"].as_str())
                .unwrap_or_default()
                .to_owned();
            if !ctx
                .coll("incident_fields")
                .iter()
                .any(|(_, f)| f["uuid"] == body["fieldUUID"])
                && body["fieldValues"].is_null()
            {
                return incident_error(
                    StatusCode::NOT_FOUND,
                    &format!(
                        "field not found: {}",
                        body["fieldUUID"].as_str().unwrap_or_default()
                    ),
                );
            }
            let Some(mut inc) = get_incident(ctx, &id) else {
                return incident_error(StatusCode::NOT_FOUND, &format!("incident not found: {id}"));
            };
            let incoming: Vec<Value> = if let Some(a) = body["fieldValues"].as_array() {
                a.clone()
            } else {
                vec![json!({"fieldUUID": body["fieldUUID"], "value": body["value"]})]
            };
            let mut current = inc["fieldValues"].as_array().cloned().unwrap_or_default();
            for fv in incoming {
                current.retain(|c| c["fieldUUID"] != fv["fieldUUID"]);
                current.push(json!({"fieldUUID": fv["fieldUUID"], "value": fv["value"]}));
            }
            inc["fieldValues"] = json!(current);
            ctx.put("incidents", &id, inc);
            ok(json!({}))
        }
        _ => incident_error(StatusCode::NOT_FOUND, &format!("unknown method: {method}")),
    }
}

fn add_activity(ctx: &mut Ctx, incident: &str, kind: &str, body: &str, url: Option<&str>) -> Value {
    let n = ctx
        .coll("incident_activity")
        .iter()
        .filter(|(_, a)| a["incidentID"] == incident)
        .count()
        + 1;
    let id = format!("{incident}-{n}");
    let now = ctx.now_str();
    let item = json!({
        "activityItemID": id, "incidentID": incident, "user": user_preview(ctx), "subjectUser": {"userID": "", "name": "", "photoURL": ""},
        "createdTime": now, "eventTime": now, "activityKind": kind, "body": body, "url": url.unwrap_or_default(), "tags": [], "immutable": kind != "userNote",
        "fieldValues": {}, "attachments": [], "relevance": "",
    });
    ctx.put("incident_activity", &id, item.clone());
    item
}
