//! Account configuration: event orchestrations (router, global and service paths),
//! incident workflows, alert grouping settings, webhook subscriptions and extension schemas.

use axum::extract::{Path, State};
use axum::response::Response;
use mock_core::world::SharedWorld;
use serde_json::{Value, json};

use crate::directory::sort_by_name;
use crate::incidents::find_incident;
use crate::util::{
    ApiErr, Ctx, Kind, Req, classic, created, default, derived_hex, matches_query, merge_fields,
    no_content, ok, ref_id, run, stored_ref,
};

// ---------------------------------------------------------------------------------------
// Event orchestrations
// ---------------------------------------------------------------------------------------

fn orchestration_url(ctx: &Ctx, id: &str) -> String {
    format!("{}/event_orchestrations/{id}", ctx.api)
}

fn router_doc(ctx: &Ctx, id: &str) -> Value {
    ctx.get("orchestration_routers", id)
        .unwrap_or_else(|| json!({}))
}

fn route_targets(router: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for set in router["sets"].as_array().into_iter().flatten() {
        for rule in set["rules"].as_array().into_iter().flatten() {
            if let Some(s) = rule["actions"]["route_to"].as_str()
                && !out.iter().any(|x| x == s)
            {
                out.push(s.to_owned());
            }
        }
    }
    out
}

fn fix_user(ctx: &Ctx, doc: &mut Value, field: &str) {
    if doc[field].is_object() {
        ctx.fix_ref(&mut doc[field], Kind::User);
    } else {
        doc[field] = Value::Null;
    }
}

fn render_orchestration(ctx: &Ctx, mut doc: Value) -> Value {
    let id = doc["id"].as_str().unwrap_or_default().to_owned();
    default(&mut doc, "self", json!(orchestration_url(ctx, &id)));
    default(&mut doc, "description", Value::Null);
    default(&mut doc, "team", Value::Null);
    ctx.fix_ref(&mut doc["team"], Kind::Team);
    default(&mut doc, "integrations", json!([]));
    default(&mut doc, "created_at", json!(ctx.now_str()));
    let created = doc["created_at"].clone();
    default(&mut doc, "updated_at", created);
    fix_user(ctx, &mut doc, "created_by");
    fix_user(ctx, &mut doc, "updated_by");
    default(
        &mut doc,
        "version",
        json!(derived_hex(&format!("{id}/version"), 8)),
    );
    doc["routes"] = json!(route_targets(&router_doc(ctx, &id)).len());
    doc
}

fn render_path(ctx: &Ctx, kind: &str, owner: &str, orchestration: &Value, mut doc: Value) -> Value {
    let (parent, path) = match kind {
        "service" => (
            json!({ "id": owner, "type": "service_reference", "self": ctx.api_url(Kind::Service, owner) }),
            format!("{}/event_orchestrations/services/{owner}", ctx.api),
        ),
        _ => (
            json!({ "id": owner, "type": "event_orchestration_reference", "self": orchestration_url(ctx, owner) }),
            format!("{}/{kind}", orchestration_url(ctx, owner)),
        ),
    };
    let catch_all = if kind == "router" {
        json!({ "actions": { "route_to": "unrouted" } })
    } else {
        json!({ "actions": {} })
    };
    doc["type"] = json!(kind);
    doc["parent"] = parent;
    doc["self"] = json!(path);
    default(&mut doc, "sets", json!([{ "id": "start", "rules": [] }]));
    default(&mut doc, "catch_all", catch_all);
    default(&mut doc, "created_at", orchestration["created_at"].clone());
    let created_at = doc["created_at"].clone();
    default(&mut doc, "updated_at", created_at);
    default(&mut doc, "created_by", orchestration["created_by"].clone());
    default(&mut doc, "updated_by", orchestration["updated_by"].clone());
    fix_user(ctx, &mut doc, "created_by");
    fix_user(ctx, &mut doc, "updated_by");
    default(
        &mut doc,
        "version",
        json!(derived_hex(&format!("{owner}/{kind}/version"), 8)),
    );
    doc
}

fn orchestration_or_404(ctx: &Ctx, id: &str) -> Result<Value, ApiErr> {
    ctx.get("event_orchestrations", id)
        .map(|o| render_orchestration(ctx, o))
        .ok_or(ApiErr::NotFound)
}

pub async fn list_orchestrations(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let mut items: Vec<Value> = ctx
            .docs("event_orchestrations")
            .into_iter()
            .map(|o| render_orchestration(ctx, o))
            .collect();
        let sort = req.query.get("sort_by").unwrap_or("name:asc");
        let (field, dir) = sort.split_once(':').unwrap_or((sort, "asc"));
        match field {
            "routes" => items.sort_by_key(|o| o["routes"].as_u64()),
            "created_at" => {
                items.sort_by(|a, b| a["created_at"].as_str().cmp(&b["created_at"].as_str()))
            }
            _ => sort_by_name(&mut items),
        }
        if dir == "desc" {
            items.reverse();
        }
        classic(&req.query, "orchestrations", items)
    })
}

pub async fn get_orchestration(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let o = orchestration_or_404(ctx, &id)?;
        ok(json!({ "orchestration": o }))
    })
}

pub async fn get_router(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let o = orchestration_or_404(ctx, &id)?;
        let path = render_path(ctx, "router", &id, &o, router_doc(ctx, &id));
        ok(json!({ "orchestration_path": path }))
    })
}

fn check_target(ctx: &Ctx, actions: &Value, errors: &mut Vec<String>, what: &str) {
    match actions["route_to"].as_str() {
        Some("unrouted") => {}
        Some(s) if ctx.exists("services", s) => {}
        Some(s) => errors.push(format!("{what}: route_to service {s} not found")),
        None if actions["dynamic_route_to"].is_object() => {}
        None => errors.push(format!("{what}: route_to or dynamic_route_to is required")),
    }
}

pub async fn update_router(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let o = orchestration_or_404(ctx, &id)?;
        let body = req.json()?;
        let input = body.get("orchestration_path").unwrap_or(&body);
        let mut errors = Vec::new();
        let sets = input["sets"].as_array().cloned().unwrap_or_default();
        if sets.len() != 1 {
            errors.push("A router must contain exactly one set of rules".to_owned());
        }
        let existing: Vec<String> = router_doc(ctx, &id)["sets"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|s| s["rules"].as_array().cloned().unwrap_or_default())
            .filter_map(|r| r["id"].as_str().map(str::to_owned))
            .collect();
        let mut out_sets = Vec::new();
        for set in &sets {
            let mut rules = Vec::new();
            for (i, rule) in set["rules"].as_array().into_iter().flatten().enumerate() {
                let what = format!("rule {}", i + 1);
                let conditions = rule["conditions"].as_array().cloned().unwrap_or_default();
                if conditions
                    .iter()
                    .any(|c| c["expression"].as_str().is_none_or(str::is_empty))
                {
                    errors.push(format!("{what}: every condition needs an expression"));
                }
                if rule["actions"]["route_to"] == "unrouted" {
                    errors.push(format!("{what}: route_to must be a service"));
                } else {
                    check_target(ctx, &rule["actions"], &mut errors, &what);
                }
                let mut rule = rule.clone();
                let keep = rule["id"]
                    .as_str()
                    .is_some_and(|rid| existing.iter().any(|e| e == rid));
                if !keep {
                    rule["id"] = json!(ctx.new_hex("orchestration_rule", 8));
                }
                default(&mut rule, "label", Value::Null);
                default(&mut rule, "disabled", json!(false));
                rules.push(rule);
            }
            out_sets.push(json!({ "id": set["id"].as_str().unwrap_or("start"), "rules": rules }));
        }
        let catch_all = input
            .get("catch_all")
            .cloned()
            .unwrap_or_else(|| json!({ "actions": { "route_to": "unrouted" } }));
        check_target(ctx, &catch_all["actions"], &mut errors, "catch_all");
        if !errors.is_empty() {
            return Err(ApiErr::Invalid(errors));
        }
        let actor = ctx.actor();
        let mut doc = router_doc(ctx, &id);
        doc["sets"] = json!(out_sets);
        doc["catch_all"] = catch_all;
        default(&mut doc, "created_at", o["created_at"].clone());
        doc["updated_at"] = json!(ctx.now_str());
        doc["updated_by"] = stored_ref(Kind::User, actor["id"].as_str().unwrap_or_default());
        doc["version"] = json!(ctx.new_hex("orchestration_version", 8));
        ctx.put("orchestration_routers", &id, doc.clone());
        let path = render_path(ctx, "router", &id, &o, doc);
        ok(json!({ "orchestration_path": path }))
    })
}

pub async fn get_global(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let o = orchestration_or_404(ctx, &id)?;
        let doc = ctx
            .get("orchestration_globals", &id)
            .unwrap_or_else(|| json!({}));
        ok(json!({ "orchestration_path": render_path(ctx, "global", &id, &o, doc) }))
    })
}

pub async fn get_service_orchestration(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let service = ctx.lookup(Kind::Service, &id).ok_or(ApiErr::NotFound)?;
        let doc = ctx
            .get("service_orchestrations", &id)
            .unwrap_or_else(|| json!({}));
        let owner = json!({ "created_at": service["created_at"] });
        ok(json!({ "orchestration_path": render_path(ctx, "service", &id, &owner, doc) }))
    })
}

pub async fn get_service_orchestration_active(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        ctx.lookup(Kind::Service, &id).ok_or(ApiErr::NotFound)?;
        let active = ctx
            .get("service_orchestrations", &id)
            .is_none_or(|d| d["active"].as_bool() != Some(false));
        ok(json!({ "active": active }))
    })
}

// ---------------------------------------------------------------------------------------
// Incident workflows
// ---------------------------------------------------------------------------------------

fn render_workflow(ctx: &Ctx, mut doc: Value, with_steps: bool) -> Value {
    let id = doc["id"].as_str().unwrap_or_default().to_owned();
    default(&mut doc, "type", json!("incident_workflow"));
    let name = doc.get("name").cloned().unwrap_or(Value::Null);
    default(&mut doc, "summary", name);
    default(
        &mut doc,
        "self",
        json!(format!("{}/incident_workflows/{id}", ctx.api)),
    );
    default(
        &mut doc,
        "html_url",
        json!(format!("{}/flex-workflows/workflows/{id}", ctx.web)),
    );
    default(&mut doc, "description", Value::Null);
    default(&mut doc, "is_enabled", json!(true));
    default(&mut doc, "team", Value::Null);
    ctx.fix_ref(&mut doc["team"], Kind::Team);
    if with_steps {
        default(&mut doc, "steps", json!([]));
        if let Some(steps) = doc["steps"].as_array_mut() {
            for s in steps {
                default(s, "type", json!("step"));
            }
        }
    } else if let Some(o) = doc.as_object_mut() {
        o.shift_remove("steps");
    }
    doc
}

pub async fn list_workflows(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let q = &req.query;
        let include = q.list("include");
        let with_steps = include.iter().any(|i| i == "steps");
        let mut items: Vec<Value> = ctx
            .docs("incident_workflows")
            .into_iter()
            .filter(|w| matches_query(q.get("query"), &[&w["name"]]))
            .collect();
        sort_by_name(&mut items);
        let items = items
            .into_iter()
            .map(|w| render_workflow(ctx, w, with_steps))
            .collect();
        classic(q, "incident_workflows", items)
    })
}

pub async fn get_workflow(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let w = ctx.get("incident_workflows", &id).ok_or(ApiErr::NotFound)?;
        ok(json!({ "incident_workflow": render_workflow(ctx, w, true) }))
    })
}

pub async fn start_workflow(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let workflow = ctx.get("incident_workflows", &id).ok_or(ApiErr::NotFound)?;
        let body = req.json()?;
        let input = body.get("incident_workflow_instance").unwrap_or(&body);
        let Some((_, incident)) = ref_id(&input["incident"]).and_then(|i| find_incident(ctx, i))
        else {
            return Err(ApiErr::invalid("Incident not found"));
        };
        if workflow["is_enabled"] == false {
            return Err(ApiErr::invalid("Incident Workflow is disabled"));
        }
        let iid = incident["id"].as_str().unwrap_or_default().to_owned();
        let instance_id = match input["id"].as_str() {
            Some(given) if !given.is_empty() => given.to_owned(),
            _ => ctx.new_id("incident_workflow_instances"),
        };
        let doc = json!({
            "id": instance_id,
            "type": "incident_workflow_instance",
            "incident": stored_ref(Kind::Incident, &iid),
            "incident_workflow": { "id": id, "type": "incident_workflow_reference" },
            "created_at": ctx.now_str(),
        });
        ctx.put("incident_workflow_instances", &instance_id, doc);
        created(json!({
            "incident_workflow_instance": {
                "id": instance_id,
                "type": "incident_workflow_instance",
                "incident": stored_ref(Kind::Incident, &iid),
            }
        }))
    })
}

// ---------------------------------------------------------------------------------------
// Alert grouping settings (cursor pagination)
// ---------------------------------------------------------------------------------------

fn render_grouping(ctx: &Ctx, mut doc: Value) -> Value {
    default(&mut doc, "name", Value::Null);
    default(&mut doc, "description", Value::Null);
    default(&mut doc, "services", json!([]));
    ctx.fix_refs(&mut doc["services"], Kind::Service);
    default(&mut doc, "created_at", Value::Null);
    let created_at = doc["created_at"].clone();
    default(&mut doc, "updated_at", created_at);
    doc
}

fn check_window(v: &Value, min: i64, max: i64, field: &str, errors: &mut Vec<String>) {
    match v[field].as_i64() {
        Some(0) => {}
        Some(n) if (min..=max).contains(&n) => {}
        _ => errors.push(format!(
            "config.{field} must be 0 or between {min} and {max}"
        )),
    }
}

fn grouping_input(ctx: &Ctx, input: &Value, except: Option<&str>) -> Result<Value, ApiErr> {
    let mut errors = Vec::new();
    let kind = input["type"].as_str().unwrap_or_default();
    let config = &input["config"];
    match kind {
        "content_based" | "content_based_intelligent" => {
            if !matches!(config["aggregate"].as_str(), Some("all" | "any")) {
                errors.push("config.aggregate must be all or any".to_owned());
            }
            if config["fields"].as_array().is_none_or(Vec::is_empty) {
                errors.push("config.fields can't be blank".to_owned());
            }
            let max = if kind == "content_based" {
                86_400
            } else {
                3_600
            };
            check_window(config, 300, max, "time_window", &mut errors);
        }
        "intelligent" => check_window(config, 300, 3_600, "time_window", &mut errors),
        "time" => check_window(config, 60, 86_400, "timeout", &mut errors),
        _ => errors.push(
            "type must be one of content_based, content_based_intelligent, intelligent, time"
                .to_owned(),
        ),
    }
    let services: Vec<String> = input["services"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| ref_id(s).map(str::to_owned))
        .collect();
    if services.is_empty() {
        errors.push("services can't be blank".to_owned());
    }
    if kind == "content_based_intelligent" && services.len() > 1 {
        errors.push("content_based_intelligent allows only one service".to_owned());
    }
    for s in &services {
        if !ctx.exists("services", s) {
            errors.push(format!("Service {s} not found"));
        } else if ctx.docs("alert_grouping_settings").iter().any(|g| {
            except.is_none_or(|e| g["id"] != e)
                && g["services"]
                    .as_array()
                    .is_some_and(|ss| ss.iter().any(|x| x["id"] == s.as_str()))
        }) {
            errors.push(format!("Service {s} already has an alert grouping setting"));
        }
    }
    if !errors.is_empty() {
        return Err(ApiErr::Invalid(errors));
    }
    Ok(json!({
        "name": input.get("name").cloned().unwrap_or(Value::Null),
        "description": input.get("description").cloned().unwrap_or(Value::Null),
        "type": kind,
        "config": config,
        "services": services.iter().map(|s| stored_ref(Kind::Service, s)).collect::<Vec<_>>(),
    }))
}

pub async fn list_groupings(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let q = &req.query;
        let services = q.list("service_ids");
        let limit = q.number("limit")?.unwrap_or(20).clamp(1, 100);
        let all: Vec<Value> = ctx
            .docs("alert_grouping_settings")
            .into_iter()
            .filter(|g| {
                services.is_empty()
                    || g["services"].as_array().is_some_and(|ss| {
                        ss.iter()
                            .any(|s| services.iter().any(|x| s["id"] == x.as_str()))
                    })
            })
            .collect();
        let position = |cursor: &str| all.iter().position(|g| g["id"] == cursor);
        let (start, end) = match (q.get("after"), q.get("before")) {
            (Some(a), _) => {
                let p = position(a).ok_or_else(|| ApiErr::invalid("Invalid cursor"))?;
                (p + 1, (p + 1 + limit).min(all.len()))
            }
            (None, Some(b)) => {
                let p = position(b).ok_or_else(|| ApiErr::invalid("Invalid cursor"))?;
                (p.saturating_sub(limit), p)
            }
            _ => (0, limit.min(all.len())),
        };
        let page = &all[start..end];
        let after = (end < all.len())
            .then(|| page.last().map(|g| g["id"].clone()))
            .flatten();
        let before = (start > 0)
            .then(|| page.first().map(|g| g["id"].clone()))
            .flatten();
        let items: Vec<Value> = page
            .iter()
            .map(|g| render_grouping(ctx, g.clone()))
            .collect();
        ok(json!({
            "alert_grouping_settings": items,
            "after": after,
            "before": before,
            "limit": limit,
            "total": if q.flag("total") { json!(all.len()) } else { Value::Null },
        }))
    })
}

pub async fn get_grouping(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let g = ctx
            .get("alert_grouping_settings", &id)
            .ok_or(ApiErr::NotFound)?;
        ok(json!({ "alert_grouping_setting": render_grouping(ctx, g) }))
    })
}

pub async fn create_grouping(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let body = req.json()?;
        let input = body.get("alert_grouping_setting").unwrap_or(&body);
        let mut doc = grouping_input(ctx, input, None)?;
        let id = ctx.new_id("alert_grouping_settings");
        let now = ctx.now_str();
        let mut out = json!({ "id": id });
        if let (Some(o), Some(d)) = (out.as_object_mut(), doc.as_object_mut()) {
            o.append(d);
            o.insert("created_at".into(), json!(now));
            o.insert("updated_at".into(), json!(now));
        }
        ctx.put("alert_grouping_settings", &id, out.clone());
        created(json!({ "alert_grouping_setting": render_grouping(ctx, out) }))
    })
}

pub async fn update_grouping(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let current = ctx
            .get("alert_grouping_settings", &id)
            .ok_or(ApiErr::NotFound)?;
        let body = req.json()?;
        let input = body.get("alert_grouping_setting").unwrap_or(&body);
        let mut doc = grouping_input(ctx, input, Some(&id))?;
        let mut out = json!({ "id": id, "created_at": current["created_at"] });
        if let (Some(o), Some(d)) = (out.as_object_mut(), doc.as_object_mut()) {
            o.append(d);
            o.insert("updated_at".into(), json!(ctx.now_str()));
        }
        ctx.put("alert_grouping_settings", &id, out.clone());
        ok(json!({ "alert_grouping_setting": render_grouping(ctx, out) }))
    })
}

pub async fn delete_grouping(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        ctx.remove("alert_grouping_settings", &id)
            .ok_or(ApiErr::NotFound)?;
        no_content()
    })
}

// ---------------------------------------------------------------------------------------
// Webhook subscriptions
// ---------------------------------------------------------------------------------------

const WEBHOOK_EVENTS: [&str; 22] = [
    "incident.acknowledged",
    "incident.annotated",
    "incident.conference_bridge.updated",
    "incident.custom_field_values.updated",
    "incident.delegated",
    "incident.escalated",
    "incident.incident_type.changed",
    "incident.priority_updated",
    "incident.reassigned",
    "incident.reopened",
    "incident.resolved",
    "incident.responder.added",
    "incident.responder.replied",
    "incident.status_update_published",
    "incident.triggered",
    "incident.unacknowledged",
    "incident.workflow.completed",
    "incident.workflow.started",
    "pagey.ping",
    "service.created",
    "service.deleted",
    "service.updated",
];

fn render_webhook(mut doc: Value) -> Value {
    default(&mut doc, "type", json!("webhook_subscription"));
    default(&mut doc, "active", json!(true));
    default(&mut doc, "description", Value::Null);
    default(&mut doc, "events", json!([]));
    default(&mut doc, "delivery_method", json!({}));
    let dm = &mut doc["delivery_method"];
    default(dm, "type", json!("http_delivery_method"));
    default(dm, "custom_headers", json!([]));
    default(dm, "temporarily_disabled", json!(false));
    dm["secret"] = Value::Null;
    doc
}

fn check_webhook(ctx: &Ctx, input: &Value, create: bool, errors: &mut Vec<String>) {
    let url = input["delivery_method"]["url"].as_str();
    if create || url.is_some() {
        match url {
            Some(u) if u.starts_with("https://") && u.len() > "https://".len() => {}
            _ => errors.push("delivery_method.url must be a valid HTTPS URL".to_owned()),
        }
    }
    if let Some(events) = input.get("events").filter(|e| !e.is_null()) {
        let events = events.as_array().cloned().unwrap_or_default();
        if events.is_empty() {
            errors.push("events can't be blank".to_owned());
        }
        for e in events {
            if !e.as_str().is_some_and(|e| WEBHOOK_EVENTS.contains(&e)) {
                errors.push(format!("{e} is not a supported event type"));
            }
        }
    } else if create {
        errors.push("events can't be blank".to_owned());
    }
    if let Some(filter) = input.get("filter").filter(|f| !f.is_null()) {
        let id = filter["id"].as_str();
        match filter["type"].as_str() {
            Some("account_reference") => {}
            Some("service_reference") if id.is_some_and(|i| ctx.exists("services", i)) => {}
            Some("team_reference") if id.is_some_and(|i| ctx.exists("teams", i)) => {}
            Some("service_reference" | "team_reference") => {
                errors.push("filter.id must reference an existing service or team".to_owned())
            }
            _ => errors.push(
                "filter.type must be account_reference, service_reference or team_reference"
                    .to_owned(),
            ),
        }
    } else if create {
        errors.push("filter can't be blank".to_owned());
    }
}

fn delivery_method(input: &Value, id: &str) -> Value {
    json!({
        "id": id,
        "type": "http_delivery_method",
        "url": input["url"],
        "custom_headers": input["custom_headers"].as_array().cloned().unwrap_or_default(),
        "temporarily_disabled": false,
        "secret": null,
    })
}

pub async fn list_webhooks(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let q = &req.query;
        let items: Vec<Value> = ctx
            .docs("webhook_subscriptions")
            .into_iter()
            .filter(|w| {
                q.get("filter_type")
                    .is_none_or(|t| w["filter"]["type"] == t)
            })
            .filter(|w| q.get("filter_id").is_none_or(|t| w["filter"]["id"] == t))
            .map(render_webhook)
            .collect();
        classic(q, "webhook_subscriptions", items)
    })
}

pub async fn get_webhook(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let w = ctx
            .get("webhook_subscriptions", &id)
            .ok_or(ApiErr::NotFound)?;
        ok(json!({ "webhook_subscription": render_webhook(w) }))
    })
}

pub async fn create_webhook(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let body = req.json()?;
        let input = body.get("webhook_subscription").unwrap_or(&body);
        let mut errors = Vec::new();
        check_webhook(ctx, input, true, &mut errors);
        if !errors.is_empty() {
            return Err(ApiErr::Invalid(errors));
        }
        let id = ctx.new_id("webhook_subscriptions");
        let dm_id = ctx.new_id("webhook_delivery_methods");
        let mut filter = json!({ "type": input["filter"]["type"] });
        if let Some(fid) = input["filter"]["id"].as_str()
            && input["filter"]["type"] != "account_reference"
        {
            filter["id"] = json!(fid);
        }
        let doc = json!({
            "id": id,
            "type": "webhook_subscription",
            "active": input["active"].as_bool().unwrap_or(true),
            "description": input.get("description").cloned().unwrap_or(Value::Null),
            "delivery_method": delivery_method(&input["delivery_method"], &dm_id),
            "events": input["events"],
            "filter": filter,
        });
        ctx.put("webhook_subscriptions", &id, doc.clone());
        let mut out = render_webhook(doc);
        out["delivery_method"]["secret"] = json!(ctx.new_hex("webhook_secret", 64));
        created(json!({ "webhook_subscription": out }))
    })
}

pub async fn update_webhook(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let mut doc = ctx
            .get("webhook_subscriptions", &id)
            .ok_or(ApiErr::NotFound)?;
        let body = req.json()?;
        let input = body.get("webhook_subscription").unwrap_or(&body);
        let mut errors = Vec::new();
        check_webhook(ctx, input, false, &mut errors);
        if !errors.is_empty() {
            return Err(ApiErr::Invalid(errors));
        }
        merge_fields(
            &mut doc,
            input,
            &["active", "description", "events", "filter"],
        );
        if let Some(dm) = input["delivery_method"].as_object() {
            default(&mut doc, "delivery_method", json!({}));
            for (k, v) in dm {
                if k != "id" && k != "secret" {
                    doc["delivery_method"][k] = v.clone();
                }
            }
        }
        ctx.put("webhook_subscriptions", &id, doc.clone());
        ok(json!({ "webhook_subscription": render_webhook(doc) }))
    })
}

pub async fn delete_webhook(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        ctx.remove("webhook_subscriptions", &id)
            .ok_or(ApiErr::NotFound)?;
        no_content()
    })
}

// ---------------------------------------------------------------------------------------
// Extension schemas
// ---------------------------------------------------------------------------------------

/// PagerDuty's built-in extension vendors, used when the dataset does not list any.
fn builtin_schemas() -> Vec<Value> {
    let schema = |id: &str, label: &str, key: &str, description: &str, guide: &str| {
        json!({
            "id": id,
            "label": label,
            "key": key,
            "description": description,
            "guide_url": guide,
            "send_types": ["trigger", "acknowledge", "resolve", "delegate", "escalate", "unacknowledge", "assign", "custom"],
            "url": null,
        })
    };
    vec![
        schema(
            "PJFWPEP",
            "Generic V2 Webhook",
            "generic_v2_webhook",
            "Send a webhook to a URL of your choice when an incident changes.",
            "https://support.pagerduty.com/docs/webhooks",
        ),
        schema(
            "PAKEDK0",
            "ServiceNow (v7)",
            "servicenow_v7",
            "Synchronize PagerDuty incidents with ServiceNow incidents.",
            "https://www.pagerduty.com/docs/guides/servicenow-integration-guide/",
        ),
        schema(
            "PD8SURB",
            "Slack V2",
            "slack_v2",
            "Send incident notifications to a Slack channel.",
            "https://support.pagerduty.com/docs/slack-integration-guide",
        ),
    ]
}

fn schemas(ctx: &Ctx) -> Vec<Value> {
    let mut items = ctx.docs("extension_schemas");
    if items.is_empty() {
        items = builtin_schemas();
    }
    items
        .into_iter()
        .map(|mut s| {
            let id = s["id"].as_str().unwrap_or_default().to_owned();
            default(&mut s, "type", json!("extension_schema"));
            let label = s.get("label").cloned().unwrap_or(Value::Null);
            default(&mut s, "summary", label);
            default(
                &mut s,
                "self",
                json!(format!("{}/extension_schemas/{id}", ctx.api)),
            );
            default(&mut s, "html_url", Value::Null);
            let key = s["key"].as_str().unwrap_or("extension").to_owned();
            default(
                &mut s,
                "icon_url",
                json!(format!(
                    "https://pagerduty.com/assets/extensions/{key}/icon.png"
                )),
            );
            default(
                &mut s,
                "logo_url",
                json!(format!(
                    "https://pagerduty.com/assets/extensions/{key}/logo.png"
                )),
            );
            s
        })
        .collect()
}

pub async fn list_extension_schemas(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let mut items = schemas(ctx);
        items.sort_by(|a, b| a["label"].as_str().cmp(&b["label"].as_str()));
        classic(&req.query, "extension_schemas", items)
    })
}

pub async fn get_extension_schema(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let s = schemas(ctx)
            .into_iter()
            .find(|s| s["id"] == id.as_str())
            .ok_or(ApiErr::NotFound)?;
        ok(json!({ "extension_schema": s }))
    })
}
