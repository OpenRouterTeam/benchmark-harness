//! Incidents: list/get/create/manage, notes, alerts, timeline, responder requests, snooze,
//! merge, status updates, and the Event Intelligence views (outlier, past and related
//! incidents, related change events).

use axum::extract::{Path, State};
use axum::response::Response;
use mock_core::world::SharedWorld;
use serde_json::{Value, json};

use crate::directory::related_service_ids;
use crate::records::{filtered_log_entries, log_entry, render_change_event};
use crate::schedules::oncall_users_at_level;
use crate::util::{
    ApiErr, Ctx, Kind, Req, Res, classic, created, default, derived_hex, derived_id, fmt, ok,
    ref_id, run, stored_ref, time_of,
};

const DAY: i64 = 86_400;

// ---------------------------------------------------------------------------------------
// Lookup and rendering
// ---------------------------------------------------------------------------------------

/// Finds an incident by ID or by incident number.
pub fn find_incident(ctx: &Ctx, id: &str) -> Option<(String, Value)> {
    if let Some(doc) = ctx.get("incidents", id) {
        return Some((id.to_owned(), doc));
    }
    ctx.entries("incidents").into_iter().find(|(_, d)| {
        d["id"] == id
            || id
                .parse::<u64>()
                .is_ok_and(|n| d["incident_number"].as_u64() == Some(n))
    })
}

fn incident_or_404(ctx: &Ctx, id: &str) -> Result<(String, Value), ApiErr> {
    find_incident(ctx, id).ok_or(ApiErr::NotFound)
}

fn alerts_of(ctx: &Ctx, incident: &str) -> Vec<Value> {
    let mut alerts: Vec<Value> = ctx
        .docs("alerts")
        .into_iter()
        .filter(|a| a["incident"]["id"] == incident)
        .collect();
    alerts.sort_by_key(|a| time_of(a, "created_at"));
    alerts
}

pub fn render_incident(ctx: &Ctx, mut doc: Value) -> Value {
    let id = doc["id"].as_str().unwrap_or_default().to_owned();
    ctx.base_fields(&mut doc, Kind::Incident);
    let service = ref_id(&doc["service"]).map(str::to_owned);
    let service_doc = service
        .as_deref()
        .and_then(|s| ctx.lookup(Kind::Service, s));
    let created = doc["created_at"].clone();
    let title = doc["title"].clone();
    default(&mut doc, "description", title);
    default(&mut doc, "status", json!("triggered"));
    default(&mut doc, "urgency", json!("high"));
    default(&mut doc, "incident_key", json!(derived_hex(&id, 32)));
    let updated = [&doc["updated_at"], &doc["last_status_change_at"], &created]
        .into_iter()
        .find(|v| !v.is_null())
        .cloned()
        .unwrap_or(Value::Null);
    doc["updated_at"] = updated;
    default(&mut doc, "last_status_change_at", created.clone());
    default(&mut doc, "resolved_at", Value::Null);
    default(&mut doc, "assignments", json!([]));
    default(&mut doc, "acknowledgements", json!([]));
    default(&mut doc, "assigned_via", json!("escalation_policy"));
    default(&mut doc, "pending_actions", json!([]));
    default(&mut doc, "is_mergeable", json!(true));
    default(&mut doc, "priority", Value::Null);
    default(&mut doc, "incidents_responders", json!([]));
    default(&mut doc, "responder_requests", json!([]));
    default(&mut doc, "subscriber_requests", json!([]));
    default(&mut doc, "alert_grouping", Value::Null);
    default(&mut doc, "basic_alert_grouping", Value::Null);
    default(&mut doc, "conference_bridge", Value::Null);
    if let Some(s) = &service_doc {
        default(
            &mut doc,
            "escalation_policy",
            s["escalation_policy"].clone(),
        );
        default(&mut doc, "teams", s["teams"].clone());
    }
    default(&mut doc, "teams", json!([]));
    default(&mut doc, "escalation_policy", Value::Null);
    if let Some(s) = &service {
        default(
            &mut doc,
            "last_status_change_by",
            stored_ref(Kind::Service, s),
        );
    }
    let alerts = alerts_of(ctx, &id);
    let triggered = alerts.iter().filter(|a| a["status"] != "resolved").count();
    doc["alert_counts"] = json!({
        "all": alerts.len(),
        "triggered": triggered,
        "resolved": alerts.len() - triggered,
    });
    let first_trigger = ctx
        .docs("log_entries")
        .into_iter()
        .filter(|e| e["incident"]["id"] == id.as_str() && e["type"] == "trigger_log_entry")
        .min_by_key(|e| time_of(e, "created_at"));
    if let Some(e) = first_trigger {
        let eid = e["id"].as_str().unwrap_or_default();
        default(
            &mut doc,
            "first_trigger_log_entry",
            json!({
                "id": eid,
                "type": "trigger_log_entry_reference",
                "summary": e["summary"],
                "self": format!("{}/log_entries/{eid}", ctx.api),
                "html_url": format!("{}/incidents/{id}/log_entries/{eid}", ctx.web),
            }),
        );
    }
    ctx.fix_ref(&mut doc["service"], Kind::Service);
    ctx.fix_ref(&mut doc["escalation_policy"], Kind::EscalationPolicy);
    ctx.fix_refs(&mut doc["teams"], Kind::Team);
    ctx.fix_ref(&mut doc["priority"], Kind::Priority);
    if let Some(items) = doc["assignments"].as_array_mut() {
        for a in items {
            default(a, "at", created.clone());
            ctx.fix_ref(&mut a["assignee"], Kind::User);
        }
    }
    if let Some(items) = doc["acknowledgements"].as_array_mut() {
        for a in items {
            let kind = if a["acknowledger"]["type"]
                .as_str()
                .is_some_and(|t| t.starts_with("service"))
            {
                Kind::Service
            } else {
                Kind::User
            };
            ctx.fix_ref(&mut a["acknowledger"], kind);
        }
    }
    let by_kind = if doc["last_status_change_by"]["type"]
        .as_str()
        .is_some_and(|t| t.starts_with("user"))
    {
        Kind::User
    } else {
        Kind::Service
    };
    ctx.fix_ref(&mut doc["last_status_change_by"], by_kind);
    doc
}

fn short_ref(ctx: &Ctx, inc: &Value) -> Value {
    let id = inc["id"].as_str().unwrap_or_default();
    json!({
        "id": id,
        "created_at": inc["created_at"],
        "self": ctx.api_url(Kind::Incident, id),
        "title": inc["title"],
    })
}

// ---------------------------------------------------------------------------------------
// List / get
// ---------------------------------------------------------------------------------------

fn sort_incidents(items: &mut [Value], sort_by: Option<&str>) {
    let keys: Vec<(String, bool)> = sort_by
        .unwrap_or("created_at:asc")
        .split(',')
        .filter(|k| !k.is_empty())
        .map(|k| {
            let (field, dir) = k.split_once(':').unwrap_or((k, "asc"));
            (field.trim().to_owned(), dir.trim() == "desc")
        })
        .collect();
    let key_of = |v: &Value, field: &str| -> (i64, String) {
        match field {
            "incident_number" => (v["incident_number"].as_i64().unwrap_or(0), String::new()),
            "urgency" => (
                i64::from(v["urgency"].as_str().unwrap_or("high") == "high"),
                String::new(),
            ),
            f => (time_of(v, f).unwrap_or(i64::MIN), String::new()),
        }
    };
    items.sort_by(|a, b| {
        for (field, desc) in &keys {
            let o = key_of(a, field).cmp(&key_of(b, field));
            let o = if *desc { o.reverse() } else { o };
            if o.is_ne() {
                return o;
            }
        }
        a["incident_number"]
            .as_i64()
            .cmp(&b["incident_number"].as_i64())
    });
}

pub async fn list_incidents(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let q = &req.query;
        let window = if q.get("date_range") == Some("all") {
            None
        } else {
            let until = q.time("until")?.unwrap_or(ctx.now);
            let since = q.time("since")?.unwrap_or(until - 30 * DAY);
            if since > until {
                return Err(ApiErr::invalid("Since must be before until"));
            }
            if until - since > 183 * DAY {
                return Err(ApiErr::invalid(
                    "The maximum range between since and until is 6 months",
                ));
            }
            Some((since, until))
        };
        let statuses = q.list("statuses");
        let urgencies = q.list("urgencies");
        let services = q.list("service_ids");
        let teams = q.list("team_ids");
        let users = q.list("user_ids");
        let mut priorities = q.list("priority_ids");
        priorities.extend(q.list("priorities"));
        let key = q.get("incident_key");
        let mut items: Vec<Value> = ctx
            .docs("incidents")
            .into_iter()
            .map(|i| render_incident(ctx, i))
            .filter(|i| {
                window
                    .is_none_or(|(s, u)| time_of(i, "created_at").is_some_and(|t| t >= s && t < u))
            })
            .filter(|i| statuses.is_empty() || statuses.iter().any(|s| i["status"] == s.as_str()))
            .filter(|i| {
                urgencies.is_empty() || urgencies.iter().any(|s| i["urgency"] == s.as_str())
            })
            .filter(|i| {
                services.is_empty() || services.iter().any(|s| i["service"]["id"] == s.as_str())
            })
            .filter(|i| {
                teams.is_empty()
                    || i["teams"].as_array().is_some_and(|ts| {
                        ts.iter()
                            .any(|t| teams.iter().any(|x| t["id"] == x.as_str()))
                    })
            })
            .filter(|i| {
                users.is_empty()
                    || i["assignments"].as_array().is_some_and(|a| {
                        a.iter()
                            .any(|a| users.iter().any(|u| a["assignee"]["id"] == u.as_str()))
                    })
            })
            .filter(|i| {
                priorities.is_empty()
                    || priorities.iter().any(|p| i["priority"]["id"] == p.as_str())
            })
            .filter(|i| key.is_none_or(|k| i["incident_key"] == k))
            .collect();
        sort_incidents(&mut items, q.get("sort_by"));
        classic(q, "incidents", items)
    })
}

pub async fn get_incident(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let (_, doc) = incident_or_404(ctx, &id)?;
        ok(json!({ "incident": render_incident(ctx, doc) }))
    })
}

// ---------------------------------------------------------------------------------------
// Mutations
// ---------------------------------------------------------------------------------------

/// A log entry to write once a change is committed.
struct PendingLog {
    kind: &'static str,
    summary: String,
    extra: Value,
}

fn user_name(ctx: &Ctx, id: &str) -> String {
    ctx.lookup(Kind::User, id)
        .and_then(|u| u["name"].as_str().map(str::to_owned))
        .unwrap_or_else(|| id.to_owned())
}

fn assign_to(ctx: &Ctx, doc: &mut Value, users: &[String]) {
    let now = ctx.now_str();
    doc["assignments"] = json!(
        users
            .iter()
            .map(|u| json!({ "at": now, "assignee": stored_ref(Kind::User, u) }))
            .collect::<Vec<_>>()
    );
    if doc["status"] == "acknowledged" {
        doc["status"] = json!("triggered");
        doc["acknowledgements"] = json!([]);
        doc["last_status_change_at"] = json!(now);
    }
}

/// Applies one incident update (bulk element or single PUT) to `doc`.
fn apply_update(
    ctx: &Ctx,
    mut doc: Value,
    input: &Value,
    actor: &str,
) -> Result<(Value, Vec<PendingLog>), Vec<String>> {
    let mut errors = Vec::new();
    let mut logs = Vec::new();
    let now = ctx.now_str();
    let actor_name = user_name(ctx, actor);
    let resolved = doc["status"] == "resolved";
    let changes_assignment = input.get("assignments").is_some_and(|a| !a.is_null())
        || input.get("escalation_level").is_some_and(|a| !a.is_null())
        || input.get("escalation_policy").is_some_and(|a| !a.is_null());
    if resolved && (changes_assignment || input.get("status").is_some_and(|s| !s.is_null())) {
        return Err(vec!["Incident Already Resolved".to_owned()]);
    }

    if let Some(title) = input.get("title").and_then(Value::as_str) {
        if title.trim().is_empty() {
            errors.push("Title can't be blank".to_owned());
        } else {
            doc["title"] = json!(title);
            doc["description"] = json!(title);
            if let Some(o) = doc.as_object_mut() {
                o.shift_remove("summary");
            }
        }
    }
    if let Some(urgency) = input.get("urgency").and_then(Value::as_str) {
        if !["high", "low"].contains(&urgency) {
            errors.push("Urgency must be high or low".to_owned());
        } else if doc["urgency"].as_str().unwrap_or("high") != urgency {
            doc["urgency"] = json!(urgency);
            logs.push(PendingLog {
                kind: "urgency_change_log_entry",
                summary: format!("Urgency changed to {urgency} by {actor_name}"),
                extra: json!({}),
            });
        }
    }
    if let Some(priority) = input.get("priority") {
        match ref_id(priority) {
            Some(p) if ctx.exists("priorities", p) => {
                doc["priority"] = stored_ref(Kind::Priority, p)
            }
            Some(_) => errors.push("Priority not found".to_owned()),
            None if priority.is_null() => doc["priority"] = Value::Null,
            None => errors.push("Priority is invalid".to_owned()),
        }
    }
    if let Some(ep) = input.get("escalation_policy").filter(|v| !v.is_null()) {
        match ref_id(ep).and_then(|id| ctx.lookup(Kind::EscalationPolicy, id)) {
            Some(policy) => {
                let pid = policy["id"].as_str().unwrap_or_default();
                doc["escalation_policy"] = stored_ref(Kind::EscalationPolicy, pid);
                let users = oncall_users_at_level(ctx, &policy, 1, ctx.now);
                assign_to(ctx, &mut doc, &users);
                doc["escalation_level"] = json!(1);
                logs.push(PendingLog {
                    kind: "assign_log_entry",
                    summary: format!("Reassigned to {} by {actor_name}", policy["name"].as_str().unwrap_or(pid)),
                    extra: json!({ "assignees": users.iter().map(|u| stored_ref(Kind::User, u)).collect::<Vec<_>>() }),
                });
            }
            None => errors.push("Escalation policy not found".to_owned()),
        }
    }
    if let Some(assignments) = input.get("assignments").filter(|v| !v.is_null()) {
        let mut users = Vec::new();
        for a in assignments.as_array().into_iter().flatten() {
            match ref_id(&a["assignee"]) {
                Some(u) if ctx.exists("users", u) => {
                    if !users.iter().any(|x: &String| x == u) {
                        users.push(u.to_owned());
                    }
                }
                Some(u) => errors.push(format!("Assignee {u} not found")),
                None => errors.push("Assignee can't be blank".to_owned()),
            }
        }
        if users.is_empty() && errors.is_empty() {
            errors.push("Assignments can't be blank".to_owned());
        }
        if errors.is_empty() {
            assign_to(ctx, &mut doc, &users);
            doc["assigned_via"] = json!("direct_assignment");
            let names: Vec<String> = users.iter().map(|u| user_name(ctx, u)).collect();
            logs.push(PendingLog {
                kind: "assign_log_entry",
                summary: format!("Reassigned to {} by {actor_name}", names.join(", ")),
                extra: json!({ "assignees": users.iter().map(|u| stored_ref(Kind::User, u)).collect::<Vec<_>>() }),
            });
        }
    }
    if let Some(level) = input.get("escalation_level").filter(|v| !v.is_null()) {
        let ep = ref_id(&doc["escalation_policy"])
            .map(str::to_owned)
            .or_else(|| {
                ref_id(&doc["service"])
                    .and_then(|s| ctx.lookup(Kind::Service, s))
                    .and_then(|s| ref_id(&s["escalation_policy"]).map(str::to_owned))
            })
            .and_then(|id| ctx.lookup(Kind::EscalationPolicy, &id));
        let rules = ep
            .as_ref()
            .and_then(|e| e["escalation_rules"].as_array().map(Vec::len))
            .unwrap_or(0);
        match (level.as_u64(), ep) {
            (Some(l), Some(policy)) if l >= 1 && (l as usize) <= rules => {
                let users = oncall_users_at_level(ctx, &policy, l as usize, ctx.now);
                assign_to(ctx, &mut doc, &users);
                doc["status"] = json!("triggered");
                doc["acknowledgements"] = json!([]);
                doc["escalation_level"] = json!(l);
                doc["escalation_policy"] =
                    stored_ref(Kind::EscalationPolicy, policy["id"].as_str().unwrap_or_default());
                let names: Vec<String> = users.iter().map(|u| user_name(ctx, u)).collect();
                logs.push(PendingLog {
                    kind: "escalate_log_entry",
                    summary: format!("Escalated to level {l} by {actor_name}"),
                    extra: json!({
                        "assignees": users.iter().map(|u| stored_ref(Kind::User, u)).collect::<Vec<_>>(),
                        "escalation_level": l,
                        "note": if names.is_empty() { Value::Null } else { json!(format!("Assigned to {}", names.join(", "))) },
                    }),
                });
            }
            _ => errors.push(format!(
                "Escalation level must be between 1 and {rules} for this incident's escalation policy"
            )),
        }
    }
    if let Some(status) = input.get("status").filter(|v| !v.is_null()) {
        match status.as_str() {
            Some("acknowledged") => {
                let already = doc["acknowledgements"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|x| x["acknowledger"]["id"] == actor));
                doc["status"] = json!("acknowledged");
                doc["last_status_change_at"] = json!(now);
                doc["last_status_change_by"] = stored_ref(Kind::User, actor);
                if !already {
                    default(&mut doc, "acknowledgements", json!([]));
                    if let Some(a) = doc["acknowledgements"].as_array_mut() {
                        a.push(json!({ "at": now, "acknowledger": stored_ref(Kind::User, actor) }));
                    }
                }
                logs.push(PendingLog {
                    kind: "acknowledge_log_entry",
                    summary: format!("Acknowledged by {actor_name}"),
                    extra: json!({ "acknowledgement_timeout": 1800 }),
                });
            }
            Some("resolved") => {
                doc["status"] = json!("resolved");
                doc["resolved_at"] = json!(now);
                doc["last_status_change_at"] = json!(now);
                doc["last_status_change_by"] = stored_ref(Kind::User, actor);
                doc["assignments"] = json!([]);
                doc["pending_actions"] = json!([]);
                let mut extra = json!({});
                if let Some(r) = input.get("resolution").and_then(Value::as_str) {
                    extra["note"] = json!(r);
                }
                logs.push(PendingLog {
                    kind: "resolve_log_entry",
                    summary: format!("Resolved by {actor_name}"),
                    extra,
                });
            }
            _ => errors.push("Status must be acknowledged or resolved".to_owned()),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    doc["updated_at"] = json!(now);
    Ok((doc, logs))
}

fn commit(ctx: &mut Ctx, key: &str, doc: Value, logs: Vec<PendingLog>, actor: &str) {
    let id = doc["id"].as_str().unwrap_or_default().to_owned();
    if doc["status"] == "resolved" {
        for (akey, mut alert) in ctx.entries("alerts") {
            if alert["incident"]["id"] == id.as_str() && alert["status"] != "resolved" {
                alert["status"] = json!("resolved");
                alert["resolved_at"] = json!(ctx.now_str());
                ctx.put("alerts", &akey, alert);
            }
        }
    }
    ctx.put("incidents", key, doc.clone());
    for l in logs {
        log_entry(
            ctx,
            &doc,
            l.kind,
            l.summary,
            stored_ref(Kind::User, actor),
            l.extra,
        );
    }
}

pub async fn update_incidents(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let actor = ctx.require_from()?;
        let actor = actor["id"].as_str().unwrap_or_default().to_owned();
        let body = req.json()?;
        let Some(items) = body["incidents"].as_array().filter(|a| !a.is_empty()) else {
            return Err(ApiErr::invalid("Incidents can't be blank"));
        };
        let mut staged = Vec::new();
        let mut errors = Vec::new();
        for item in items {
            let Some(id) = item["id"].as_str() else {
                errors.push("Incident id can't be blank".to_owned());
                continue;
            };
            let Some((key, doc)) = find_incident(ctx, id) else {
                return Err(ApiErr::NotFound);
            };
            match apply_update(ctx, doc, item, &actor) {
                Ok((doc, logs)) => staged.push((key, doc, logs)),
                Err(e) => errors.extend(e.into_iter().map(|m| format!("{id}: {m}"))),
            }
        }
        if !errors.is_empty() {
            return Err(ApiErr::Invalid(errors));
        }
        let mut out = Vec::new();
        for (key, doc, logs) in staged {
            commit(ctx, &key, doc, logs, &actor);
            out.push(key);
        }
        let incidents: Vec<Value> = out
            .iter()
            .filter_map(|k| ctx.get("incidents", k))
            .map(|d| render_incident(ctx, d))
            .collect();
        ok(json!({ "incidents": incidents }))
    })
}

pub async fn update_incident(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let actor = ctx.require_from()?;
        let actor = actor["id"].as_str().unwrap_or_default().to_owned();
        let (key, doc) = incident_or_404(ctx, &id)?;
        let body = req.json()?;
        let input = body.get("incident").unwrap_or(&body);
        let (doc, logs) = apply_update(ctx, doc, input, &actor).map_err(ApiErr::Invalid)?;
        commit(ctx, &key, doc, logs, &actor);
        let doc = ctx.get("incidents", &key).unwrap_or_default();
        ok(json!({ "incident": render_incident(ctx, doc) }))
    })
}

pub async fn create_incident(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let actor = ctx.require_from()?;
        let actor_id = actor["id"].as_str().unwrap_or_default().to_owned();
        let body = req.json()?;
        let input = body.get("incident").unwrap_or(&body);
        let mut errors = Vec::new();
        let title = input["title"].as_str().map(str::trim).unwrap_or_default();
        if title.is_empty() {
            errors.push("Title can't be blank".to_owned());
        }
        let service = ref_id(&input["service"]).and_then(|s| ctx.lookup(Kind::Service, s));
        if service.is_none() {
            errors.push("Service not found".to_owned());
        }
        let urgency = input["urgency"].as_str();
        if urgency.is_some_and(|u| !["high", "low"].contains(&u)) {
            errors.push("Urgency must be high or low".to_owned());
        }
        let priority = ref_id(&input["priority"]).map(str::to_owned);
        if priority
            .as_deref()
            .is_some_and(|p| !ctx.exists("priorities", p))
        {
            errors.push("Priority not found".to_owned());
        }
        let ep_input = ref_id(&input["escalation_policy"]).map(str::to_owned);
        let assignees: Vec<String> = input["assignments"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| ref_id(&a["assignee"]).map(str::to_owned))
            .collect();
        if ep_input.is_some() && !assignees.is_empty() {
            errors.push("Assignments and escalation policy cannot both be specified".to_owned());
        }
        if ep_input
            .as_deref()
            .is_some_and(|e| !ctx.exists("escalation_policies", e))
        {
            errors.push("Escalation policy not found".to_owned());
        }
        for u in &assignees {
            if !ctx.exists("users", u) {
                errors.push(format!("Assignee {u} not found"));
            }
        }
        if let (Some(key), Some(s)) = (input["incident_key"].as_str(), &service) {
            let dup = ctx.docs("incidents").iter().any(|i| {
                i["incident_key"] == key
                    && i["service"]["id"] == s["id"]
                    && i["status"] != "resolved"
            });
            if dup {
                errors.push(
                    "Open incident with matching dedup key already exists on this service"
                        .to_owned(),
                );
            }
        }
        if !errors.is_empty() {
            return Err(ApiErr::Invalid(errors));
        }
        let service = service.unwrap_or_default();
        let sid = service["id"].as_str().unwrap_or_default().to_owned();
        let ep_id = ep_input
            .or_else(|| ref_id(&service["escalation_policy"]).map(str::to_owned))
            .unwrap_or_default();
        let (users, via) = if assignees.is_empty() {
            let users = ctx
                .lookup(Kind::EscalationPolicy, &ep_id)
                .map(|ep| oncall_users_at_level(ctx, &ep, 1, ctx.now))
                .unwrap_or_default();
            (users, "escalation_policy")
        } else {
            (assignees, "direct_assignment")
        };
        let id = ctx.new_incident_id();
        let number = ctx
            .docs("incidents")
            .iter()
            .filter_map(|i| i["incident_number"].as_u64())
            .max()
            .unwrap_or(0)
            + 1;
        let now = ctx.now_str();
        let default_urgency = service["incident_urgency_rule"]["urgency"]
            .as_str()
            .unwrap_or("high")
            .to_owned();
        let mut doc = json!({
            "id": id,
            "incident_number": number,
            "title": title,
            "description": title,
            "created_at": now,
            "updated_at": now,
            "status": "triggered",
            "incident_key": input["incident_key"].as_str().map(str::to_owned).unwrap_or_else(|| derived_hex(&id, 32)),
            "service": stored_ref(Kind::Service, &sid),
            "assignments": users.iter().map(|u| json!({ "at": now, "assignee": stored_ref(Kind::User, u) })).collect::<Vec<_>>(),
            "assigned_via": via,
            "last_status_change_at": now,
            "last_status_change_by": stored_ref(Kind::Service, &sid),
            "resolved_at": null,
            "acknowledgements": [],
            "escalation_policy": stored_ref(Kind::EscalationPolicy, &ep_id),
            "teams": service["teams"].clone(),
            "urgency": urgency.map(str::to_owned).unwrap_or(default_urgency),
            "priority": priority.map(|p| stored_ref(Kind::Priority, &p)),
        });
        if let Some(details) = input["body"]["details"].as_str() {
            doc["body"] = json!({ "type": "incident_body", "details": details });
        }
        if doc["teams"].is_null() {
            doc["teams"] = json!([]);
        }
        ctx.put("incidents", &id, doc.clone());
        let actor_name = user_name(ctx, &actor_id);
        log_entry(
            ctx,
            &doc,
            "trigger_log_entry",
            format!("Triggered through the API by {actor_name}"),
            stored_ref(Kind::User, &actor_id),
            json!({}),
        );
        created(json!({ "incident": render_incident(ctx, doc) }))
    })
}

// ---------------------------------------------------------------------------------------
// Notes, alerts, timeline
// ---------------------------------------------------------------------------------------

fn render_note(ctx: &Ctx, mut note: Value) -> Value {
    ctx.fix_ref(&mut note["user"], Kind::User);
    default(
        &mut note,
        "channel",
        json!({ "summary": "The PagerDuty website or APIs" }),
    );
    note
}

pub async fn list_notes(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let (key, _) = incident_or_404(ctx, &id)?;
        let notes: Vec<Value> = ctx
            .children("notes", &key)
            .into_iter()
            .map(|(_, n)| render_note(ctx, n))
            .collect();
        ok(json!({ "notes": notes }))
    })
}

pub async fn create_note(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let actor = ctx.require_from()?;
        let actor_id = actor["id"].as_str().unwrap_or_default().to_owned();
        let (key, mut incident) = incident_or_404(ctx, &id)?;
        let body = req.json()?;
        let content = body["note"]["content"]
            .as_str()
            .or_else(|| body["content"].as_str())
            .unwrap_or_default();
        if content.trim().is_empty() {
            return Err(ApiErr::invalid("Content can't be blank"));
        }
        let note_id = ctx.new_id("notes");
        let note = json!({
            "id": note_id,
            "user": stored_ref(Kind::User, &actor_id),
            "channel": { "summary": "The PagerDuty website or APIs" },
            "content": content,
            "created_at": ctx.now_str(),
        });
        let note_key = ctx.next_child_key("notes", &key);
        ctx.put("notes", &note_key, note.clone());
        incident["updated_at"] = json!(ctx.now_str());
        ctx.put("incidents", &key, incident.clone());
        log_entry(
            ctx,
            &incident,
            "annotate_log_entry",
            format!("Note added by {}", user_name(ctx, &actor_id)),
            stored_ref(Kind::User, &actor_id),
            json!({ "channel": { "type": "note", "summary": content } }),
        );
        created(json!({ "note": render_note(ctx, note) }))
    })
}

fn render_alert(ctx: &Ctx, mut a: Value) -> Value {
    let id = a["id"].as_str().unwrap_or_default().to_owned();
    default(&mut a, "type", json!("alert"));
    default(&mut a, "summary", json!(format!("Alert {id}")));
    default(&mut a, "self", json!(format!("{}/alerts/{id}", ctx.api)));
    default(
        &mut a,
        "html_url",
        json!(format!("{}/alerts/{id}", ctx.web)),
    );
    default(&mut a, "status", json!("triggered"));
    default(&mut a, "alert_key", json!(derived_hex(&id, 32)));
    default(&mut a, "severity", json!("error"));
    default(&mut a, "suppressed", json!(false));
    default(&mut a, "resolved_at", Value::Null);
    default(
        &mut a,
        "body",
        json!({ "type": "alert_body", "contexts": [], "details": {} }),
    );
    default(&mut a, "integration", Value::Null);
    if let Some(iid) = ref_id(&a["integration"]).map(str::to_owned) {
        a["integration"] = json!({
            "id": iid,
            "type": "inbound_integration_reference",
            "summary": a["integration"]["summary"],
            "self": format!("{}/integrations/{iid}", ctx.api),
            "html_url": Value::Null,
        });
    }
    ctx.fix_ref(&mut a["service"], Kind::Service);
    ctx.fix_ref(&mut a["incident"], Kind::Incident);
    a
}

pub async fn list_alerts(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let (key, incident) = incident_or_404(ctx, &id)?;
        let iid = incident["id"].as_str().unwrap_or(&key).to_owned();
        let statuses = req.query.list("statuses");
        let alert_key = req.query.get("alert_key");
        let alerts: Vec<Value> = alerts_of(ctx, &iid)
            .into_iter()
            .map(|a| render_alert(ctx, a))
            .filter(|a| statuses.is_empty() || statuses.iter().any(|s| a["status"] == s.as_str()))
            .filter(|a| alert_key.is_none_or(|k| a["alert_key"] == k))
            .collect();
        classic(&req.query, "alerts", alerts)
    })
}

pub async fn get_alert(
    State(world): State<SharedWorld>,
    Path((id, alert)): Path<(String, String)>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let (key, incident) = incident_or_404(ctx, &id)?;
        let iid = incident["id"].as_str().unwrap_or(&key).to_owned();
        let doc = alerts_of(ctx, &iid)
            .into_iter()
            .find(|a| a["id"] == alert.as_str())
            .ok_or(ApiErr::NotFound)?;
        ok(json!({ "alert": render_alert(ctx, doc) }))
    })
}

pub async fn incident_log_entries(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let (key, incident) = incident_or_404(ctx, &id)?;
        let iid = incident["id"].as_str().unwrap_or(&key).to_owned();
        let items = filtered_log_entries(ctx, &req.query, Some(&iid))?;
        classic(&req.query, "log_entries", items)
    })
}

// ---------------------------------------------------------------------------------------
// Responders, snooze, merge, status updates
// ---------------------------------------------------------------------------------------

pub async fn create_responder_request(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        ctx.require_from()?;
        let (key, mut incident) = incident_or_404(ctx, &id)?;
        let body = req.json()?;
        let mut errors = Vec::new();
        let requester = body["requester_id"].as_str().unwrap_or_default().to_owned();
        if !ctx.exists("users", &requester) {
            errors.push("Requester not found".to_owned());
        }
        if incident["status"] == "resolved" {
            errors.push("Incident Already Resolved".to_owned());
        }
        let targets: Vec<Value> = body["responder_request_targets"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if targets.is_empty() {
            errors.push("Responder request targets can't be blank".to_owned());
        }
        let now = ctx.now_str();
        let message = body["message"].clone();
        let iid = incident["id"].as_str().unwrap_or(&key).to_owned();
        let mut rendered_targets = Vec::new();
        let mut responders = Vec::new();
        for t in &targets {
            let target = &t["responder_request_target"];
            let tid = target["id"].as_str().unwrap_or_default();
            let (kind, users) = match target["type"].as_str() {
                Some("user_reference" | "user") => (Kind::User, vec![tid.to_owned()]),
                Some("escalation_policy_reference" | "escalation_policy") => {
                    let users = ctx
                        .lookup(Kind::EscalationPolicy, tid)
                        .map(|ep| oncall_users_at_level(ctx, &ep, 1, ctx.now))
                        .unwrap_or_default();
                    (Kind::EscalationPolicy, users)
                }
                _ => {
                    errors.push(format!("Target {tid} has an invalid type"));
                    continue;
                }
            };
            if ctx.lookup(kind, tid).is_none() {
                errors.push(format!("Target {tid} not found"));
                continue;
            }
            let entries: Vec<Value> = users
                .iter()
                .map(|u| {
                    json!({
                        "state": "pending",
                        "user": stored_ref(Kind::User, u),
                        "incident": stored_ref(Kind::Incident, &iid),
                        "updated_at": now,
                        "message": message,
                        "requester": stored_ref(Kind::User, &requester),
                        "requested_at": now,
                    })
                })
                .collect();
            responders.extend(entries.clone());
            rendered_targets.push(json!({
                "responder_request_target": {
                    "type": kind.object_type(),
                    "id": tid,
                    "summary": ctx.lookup(kind, tid).map(|d| d["name"].clone()).unwrap_or(Value::Null),
                    "incidents_responders": entries,
                }
            }));
        }
        if !errors.is_empty() {
            return Err(ApiErr::Invalid(errors));
        }
        let request = json!({
            "incident": stored_ref(Kind::Incident, &iid),
            "requester": stored_ref(Kind::User, &requester),
            "requested_at": now,
            "message": message,
            "responder_request_targets": rendered_targets,
        });
        default(&mut incident, "responder_requests", json!([]));
        default(&mut incident, "incidents_responders", json!([]));
        if let Some(r) = incident["responder_requests"].as_array_mut() {
            r.push(request.clone());
        }
        if let Some(r) = incident["incidents_responders"].as_array_mut() {
            r.extend(responders);
        }
        incident["updated_at"] = json!(now);
        ctx.put("incidents", &key, incident);
        let mut out = request;
        ctx.fix_ref(&mut out["incident"], Kind::Incident);
        ctx.fix_ref(&mut out["requester"], Kind::User);
        ok(json!({ "responder_request": out }))
    })
}

pub async fn snooze(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let actor = ctx.require_from()?;
        let actor_id = actor["id"].as_str().unwrap_or_default().to_owned();
        let (key, mut incident) = incident_or_404(ctx, &id)?;
        let body = req.json()?;
        let Some(duration) = body["duration"].as_i64().filter(|d| *d > 0) else {
            return Err(ApiErr::invalid(
                "Duration must be a positive number of seconds",
            ));
        };
        if incident["status"] != "acknowledged" {
            return Err(ApiErr::invalid(
                "Incident must be acknowledged to be snoozed",
            ));
        }
        incident["pending_actions"] =
            json!([{ "type": "unacknowledge", "at": fmt(ctx.now + duration) }]);
        incident["updated_at"] = json!(ctx.now_str());
        ctx.put("incidents", &key, incident.clone());
        log_entry(
            ctx,
            &incident,
            "snooze_log_entry",
            format!("Snoozed by {}", user_name(ctx, &actor_id)),
            stored_ref(Kind::User, &actor_id),
            json!({ "changed_actions": [{ "type": "unacknowledge", "at": fmt(ctx.now + duration) }] }),
        );
        created(json!({ "incident": render_incident(ctx, incident) }))
    })
}

pub async fn merge(State(world): State<SharedWorld>, Path(id): Path<String>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let actor = ctx.require_from()?;
        let actor_id = actor["id"].as_str().unwrap_or_default().to_owned();
        let (key, target) = incident_or_404(ctx, &id)?;
        let tid = target["id"].as_str().unwrap_or(&key).to_owned();
        if target["status"] == "resolved" {
            return Err(ApiErr::invalid("Incident Already Resolved"));
        }
        let body = req.json()?;
        let sources: Vec<String> = body["source_incidents"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| ref_id(s).map(str::to_owned))
            .collect();
        if sources.is_empty() {
            return Err(ApiErr::invalid("Source incidents can't be blank"));
        }
        let mut staged = Vec::new();
        for s in &sources {
            let (skey, sdoc) = find_incident(ctx, s).ok_or(ApiErr::NotFound)?;
            if sdoc["id"] == tid.as_str() {
                return Err(ApiErr::invalid("An incident cannot be merged into itself"));
            }
            if sdoc["status"] == "resolved" {
                return Err(ApiErr::invalid(format!("{s}: Incident Already Resolved")));
            }
            staged.push((skey, sdoc));
        }
        let now = ctx.now_str();
        for (skey, mut sdoc) in staged {
            let sid = sdoc["id"].as_str().unwrap_or(&skey).to_owned();
            for (akey, mut alert) in ctx.entries("alerts") {
                if alert["incident"]["id"] == sid.as_str() {
                    alert["incident"] = stored_ref(Kind::Incident, &tid);
                    ctx.put("alerts", &akey, alert);
                }
            }
            sdoc["status"] = json!("resolved");
            sdoc["resolved_at"] = json!(now);
            sdoc["last_status_change_at"] = json!(now);
            sdoc["last_status_change_by"] = stored_ref(Kind::User, &actor_id);
            sdoc["assignments"] = json!([]);
            sdoc["updated_at"] = json!(now);
            ctx.put("incidents", &skey, sdoc.clone());
            log_entry(
                ctx,
                &sdoc,
                "resolve_log_entry",
                format!(
                    "Resolved by {} (merged into {tid})",
                    user_name(ctx, &actor_id)
                ),
                stored_ref(Kind::User, &actor_id),
                json!({}),
            );
        }
        let mut target = ctx.get("incidents", &key).unwrap_or(target);
        target["updated_at"] = json!(now);
        ctx.put("incidents", &key, target.clone());
        ok(json!({ "incident": render_incident(ctx, target) }))
    })
}

pub async fn create_status_update(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let actor = ctx.require_from()?;
        let actor_id = actor["id"].as_str().unwrap_or_default().to_owned();
        let (key, _) = incident_or_404(ctx, &id)?;
        let body = req.json()?;
        let Some(message) = body["message"].as_str().filter(|m| !m.trim().is_empty()) else {
            return Err(ApiErr::invalid("Message can't be blank"));
        };
        let uid = ctx.new_id("status_updates");
        let update = json!({
            "id": uid,
            "message": message,
            "subject": body.get("subject").cloned().unwrap_or(Value::Null),
            "html_message": body.get("html_message").cloned().unwrap_or(Value::Null),
            "sender": stored_ref(Kind::User, &actor_id),
            "created_at": ctx.now_str(),
        });
        let ukey = ctx.next_child_key("status_updates", &key);
        ctx.put("status_updates", &ukey, update.clone());
        let mut out = update;
        ctx.fix_ref(&mut out["sender"], Kind::User);
        ok(json!({ "status_update": out }))
    })
}

// ---------------------------------------------------------------------------------------
// Event Intelligence views
// ---------------------------------------------------------------------------------------

fn tokens(text: &str) -> Vec<String> {
    let mut out: Vec<String> = text
        .to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() > 1)
        .map(str::to_owned)
        .collect();
    out.sort();
    out.dedup();
    out
}

fn similarity(a: &str, b: &str) -> f64 {
    let (ta, tb) = (tokens(a), tokens(b));
    if ta.is_empty() || tb.is_empty() {
        return 0.0;
    }
    let common = ta.iter().filter(|t| tb.contains(t)).count();
    let union = ta.len() + tb.len() - common;
    (common as f64 * 10_000.0 / union as f64).round() / 100.0
}

/// The title with digits collapsed, as PagerDuty's template mining shows it.
fn mined_text(title: &str) -> String {
    let mut out = String::new();
    let mut in_number = false;
    for c in title.chars() {
        if c.is_ascii_digit() {
            if !in_number {
                out.push_str("<*>");
            }
            in_number = true;
        } else {
            in_number = false;
            out.push(c);
        }
    }
    out
}

fn same_service(ctx: &Ctx, incident: &Value) -> Vec<Value> {
    ctx.docs("incidents")
        .into_iter()
        .filter(|i| i["service"]["id"] == incident["service"]["id"])
        .collect()
}

pub async fn outlier_incident(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let (_, incident) = incident_or_404(ctx, &id)?;
        let created = time_of(&incident, "created_at").unwrap_or(ctx.now);
        let since = req.query.time("since")?.unwrap_or(created - 180 * DAY);
        let until = created.max(ctx.now);
        let template = mined_text(incident["title"].as_str().unwrap_or_default());
        let window: Vec<Value> = same_service(ctx, &incident)
            .into_iter()
            .filter(|i| time_of(i, "created_at").is_some_and(|t| t >= since && t <= until))
            .collect();
        let count = window
            .iter()
            .filter(|i| mined_text(i["title"].as_str().unwrap_or_default()) == template)
            .count()
            .max(1);
        let frequency = (count as f64 * 100.0 / window.len().max(1) as f64).round() / 100.0;
        let category = if count == 1 {
            "rare"
        } else if frequency >= 0.5 {
            "frequent"
        } else {
            "anomalous"
        };
        let mut reference = short_ref(ctx, &incident);
        reference["occurrence"] = json!({
            "count": count,
            "frequency": frequency,
            "category": category,
            "since": fmt(since),
            "until": fmt(until),
        });
        let service = incident["service"]["id"].as_str().unwrap_or_default();
        ok(json!({
            "outlier_incident": {
                "incident": reference,
                "incident_template": {
                    "id": derived_id(&format!("template/{service}/{template}")),
                    "cluster_id": derived_id(&format!("cluster/{service}")),
                    "mined_text": template,
                }
            }
        }))
    })
}

pub async fn past_incidents(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let (_, incident) = incident_or_404(ctx, &id)?;
        let limit = req.query.number("limit")?.unwrap_or(5).clamp(1, 999);
        let created = time_of(&incident, "created_at").unwrap_or(ctx.now);
        let title = incident["title"].as_str().unwrap_or_default();
        let mut scored: Vec<(f64, i64, Value)> = same_service(ctx, &incident)
            .into_iter()
            .filter(|i| i["id"] != incident["id"])
            .filter_map(|i| {
                let t = time_of(&i, "created_at")?;
                (t < created && t >= created - 183 * DAY).then_some(())?;
                let score = similarity(title, i["title"].as_str().unwrap_or_default());
                (score > 0.0).then_some((score, t, i))
            })
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.cmp(&a.1)));
        let total = scored.len();
        let items: Vec<Value> = scored
            .into_iter()
            .take(limit)
            .map(|(score, _, i)| json!({ "incident": short_ref(ctx, &i), "score": score }))
            .collect();
        ok(json!({
            "past_incidents": items,
            "limit": limit,
            "total": if req.query.flag("total") { json!(total) } else { Value::Null },
        }))
    })
}

pub async fn related_incidents(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let (_, incident) = incident_or_404(ctx, &id)?;
        let service = incident["service"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let related = related_service_ids(ctx, &service);
        let created = time_of(&incident, "created_at").unwrap_or(ctx.now);
        let mut items: Vec<(i64, Value)> = Vec::new();
        for other in ctx.docs("incidents") {
            if other["id"] == incident["id"] || other["status"] == "resolved" {
                continue;
            }
            let osvc = other["service"]["id"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            if osvc == service {
                continue;
            }
            let t = time_of(&other, "created_at").unwrap_or(0);
            let relationship = if related.contains(&osvc) {
                let (dependent, supporting) = if ctx.docs("service_dependencies").iter().any(|d| {
                    d["supporting_service"]["id"] == service.as_str()
                        && d["dependent_service"]["id"] == osvc.as_str()
                }) {
                    (&osvc, &service)
                } else {
                    (&service, &osvc)
                };
                json!({
                    "type": "service_dependency",
                    "metadata": {
                        "dependent_services": [{ "id": dependent, "type": "technical_service_reference" }],
                        "supporting_services": [{ "id": supporting, "type": "technical_service_reference" }],
                    }
                })
            } else if (t - created).abs() <= 3600 {
                json!({
                    "type": "machine_learning_inferred",
                    "metadata": {
                        "grouping_classification": "prior_feedback",
                        "user_feedback": { "positive_feedback_count": 0, "negative_feedback_count": 0 },
                    }
                })
            } else {
                continue;
            };
            items.push((
                t,
                json!({ "incident": short_ref(ctx, &other), "relationships": [relationship] }),
            ));
        }
        items.sort_by_key(|item| std::cmp::Reverse(item.0));
        let items: Vec<Value> = items.into_iter().take(20).map(|(_, v)| v).collect();
        ok(json!({ "related_incidents": items }))
    })
}

pub async fn related_change_events(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| -> Res {
        let (_, incident) = incident_or_404(ctx, &id)?;
        let service = incident["service"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let related = related_service_ids(ctx, &service);
        let created = time_of(&incident, "created_at").unwrap_or(ctx.now);
        let mut items: Vec<(i64, Value)> = ctx
            .docs("change_events")
            .into_iter()
            .filter_map(|e| {
                let t = time_of(&e, "timestamp")?;
                if t > created + 3600 || t < created - DAY {
                    return None;
                }
                let services: Vec<String> = e["services"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|s| ref_id(s).map(str::to_owned))
                    .collect();
                let reason = if services.contains(&service) {
                    "recent"
                } else if services.iter().any(|s| related.contains(s)) {
                    "related_service"
                } else {
                    return None;
                };
                let mut rendered = render_change_event(ctx, e);
                rendered["correlation_reason"] = json!({ "reason": reason });
                Some((t, rendered))
            })
            .collect();
        items.sort_by_key(|item| std::cmp::Reverse(item.0));
        classic(
            &req.query,
            "change_events",
            items.into_iter().map(|(_, v)| v).collect(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similarity_and_templates() {
        assert_eq!(similarity("Disk full on db-1", "Disk full on db-1"), 100.0);
        assert!(similarity("Disk full on db-1", "Disk usage high") > 0.0);
        assert_eq!(similarity("abc", "xyz"), 0.0);
        assert_eq!(
            mined_text("5xx rate 12% on node 3"),
            "<*>xx rate <*>% on node <*>"
        );
    }
}
