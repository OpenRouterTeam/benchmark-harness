//! Log entries (the incident timeline) and change events.

use axum::extract::{Path, State};
use axum::response::Response;
use mock_core::world::SharedWorld;
use serde_json::{Value, json};

use crate::util::{
    ApiErr, Ctx, Kind, Query, Req, Res, classic, default, ok, ref_id, run, stored_ref, time_of,
};

/// Log entry types shown when `is_overview=true`.
const OVERVIEW_TYPES: [&str; 8] = [
    "trigger_log_entry",
    "acknowledge_log_entry",
    "resolve_log_entry",
    "assign_log_entry",
    "escalate_log_entry",
    "annotate_log_entry",
    "snooze_log_entry",
    "urgency_change_log_entry",
];

/// A log entry about `incident`, attributed to `agent` (a stored reference).
pub fn log_entry(
    ctx: &mut Ctx,
    incident: &Value,
    kind: &str,
    summary: String,
    agent: Value,
    extra: Value,
) {
    let id = ctx.new_id("log_entries");
    let inc_id = incident["id"].as_str().unwrap_or_default();
    let mut doc = json!({
        "id": id,
        "type": kind,
        "summary": summary,
        "created_at": ctx.now_str(),
        "agent": agent,
        "channel": { "type": "api" },
        "service": incident["service"].clone(),
        "incident": stored_ref(Kind::Incident, inc_id),
        "teams": incident["teams"].clone(),
        "contexts": [],
    });
    if let (Some(d), Value::Object(e)) = (doc.as_object_mut(), extra) {
        for (k, v) in e {
            d.insert(k, v);
        }
    }
    ctx.put("log_entries", &id, doc);
}

fn agent_kind(agent: &Value) -> Kind {
    match agent["type"].as_str() {
        Some(t) if t.starts_with("service") => Kind::Service,
        _ => Kind::User,
    }
}

pub fn render_log_entry(ctx: &Ctx, mut doc: Value) -> Value {
    let id = doc["id"].as_str().unwrap_or_default().to_owned();
    let inc = ref_id(&doc["incident"]).unwrap_or_default().to_owned();
    default(
        &mut doc,
        "self",
        json!(format!("{}/log_entries/{id}", ctx.api)),
    );
    default(
        &mut doc,
        "html_url",
        json!(format!("{}/incidents/{inc}/log_entries/{id}", ctx.web)),
    );
    default(&mut doc, "contexts", json!([]));
    default(&mut doc, "channel", json!({ "type": "api" }));
    if doc["agent"].is_object() {
        let kind = agent_kind(&doc["agent"]);
        ctx.fix_ref(&mut doc["agent"], kind);
    }
    ctx.fix_ref(&mut doc["service"], Kind::Service);
    ctx.fix_ref(&mut doc["incident"], Kind::Incident);
    default(&mut doc, "teams", json!([]));
    ctx.fix_refs(&mut doc["teams"], Kind::Team);
    if doc["assignees"].is_array() {
        ctx.fix_refs(&mut doc["assignees"], Kind::User);
    }
    doc
}

fn by_time_desc(items: &mut [Value], field: &str) {
    items.sort_by(|a, b| {
        time_of(b, field)
            .cmp(&time_of(a, field))
            .then_with(|| b["id"].as_str().cmp(&a["id"].as_str()))
    });
}

/// Log entries filtered by `since`/`until`/`team_ids[]`/`is_overview`, newest first.
pub fn filtered_log_entries(
    ctx: &Ctx,
    q: &Query,
    incident: Option<&str>,
) -> Result<Vec<Value>, ApiErr> {
    let since = q.time("since")?;
    let until = q.time("until")?;
    let teams = q.list("team_ids");
    let overview = q.flag("is_overview");
    let mut items: Vec<Value> = ctx
        .docs("log_entries")
        .into_iter()
        .filter(|e| incident.is_none_or(|i| e["incident"]["id"] == i))
        .filter(|e| {
            let t = time_of(e, "created_at");
            since.is_none_or(|s| t.is_some_and(|t| t >= s))
                && until.is_none_or(|u| t.is_some_and(|t| t < u))
        })
        .filter(|e| {
            teams.is_empty()
                || e["teams"].as_array().is_some_and(|ts| {
                    ts.iter()
                        .any(|t| teams.iter().any(|x| t["id"] == x.as_str()))
                })
        })
        .filter(|e| {
            !overview
                || e["type"]
                    .as_str()
                    .is_some_and(|t| OVERVIEW_TYPES.contains(&t))
        })
        .collect();
    by_time_desc(&mut items, "created_at");
    Ok(items
        .into_iter()
        .map(|e| render_log_entry(ctx, e))
        .collect())
}

pub async fn list_log_entries(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let items = filtered_log_entries(ctx, &req.query, None)?;
        classic(&req.query, "log_entries", items)
    })
}

pub async fn get_log_entry(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let doc = ctx.get("log_entries", &id).ok_or(ApiErr::NotFound)?;
        ok(json!({ "log_entry": render_log_entry(ctx, doc) }))
    })
}

// ---------------------------------------------------------------------------------------
// Change events
// ---------------------------------------------------------------------------------------

pub fn render_change_event(ctx: &Ctx, mut doc: Value) -> Value {
    default(&mut doc, "type", json!("change_event"));
    default(&mut doc, "summary", json!(""));
    default(&mut doc, "services", json!([]));
    ctx.fix_refs(&mut doc["services"], Kind::Service);
    default(&mut doc, "integration", Value::Null);
    if let Some(id) = ref_id(&doc["integration"]).map(str::to_owned) {
        doc["integration"] = json!({
            "id": id,
            "type": "inbound_integration_reference",
            "summary": doc["integration"]["summary"],
            "self": format!("{}/integrations/{id}", ctx.api),
            "html_url": Value::Null,
        });
    }
    default(&mut doc, "routing_key", Value::Null);
    default(&mut doc, "source", Value::Null);
    default(&mut doc, "links", json!([]));
    default(&mut doc, "images", json!([]));
    default(&mut doc, "custom_details", json!({}));
    doc
}

fn event_teams(ctx: &Ctx, event: &Value) -> Vec<String> {
    event["services"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(ref_id)
        .filter_map(|s| ctx.get("services", s))
        .flat_map(|s| {
            s["teams"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| t["id"].as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn filtered_change_events(
    ctx: &Ctx,
    q: &Query,
    service: Option<&str>,
) -> Result<Vec<Value>, ApiErr> {
    let since = q.time("since")?;
    let until = q.time("until")?;
    let teams = q.list("team_ids");
    let integrations = q.list("integration_ids");
    let mut items: Vec<Value> = ctx
        .docs("change_events")
        .into_iter()
        .filter(|e| {
            service.is_none_or(|s| {
                e["services"]
                    .as_array()
                    .is_some_and(|ss| ss.iter().any(|x| x["id"] == s))
            })
        })
        .filter(|e| {
            let t = time_of(e, "timestamp");
            since.is_none_or(|s| t.is_some_and(|t| t >= s))
                && until.is_none_or(|u| t.is_some_and(|t| t < u))
        })
        .filter(|e| teams.is_empty() || event_teams(ctx, e).iter().any(|t| teams.contains(t)))
        .filter(|e| {
            integrations.is_empty()
                || ref_id(&e["integration"]).is_some_and(|i| integrations.iter().any(|x| x == i))
        })
        .collect();
    by_time_desc(&mut items, "timestamp");
    Ok(items
        .into_iter()
        .map(|e| render_change_event(ctx, e))
        .collect())
}

pub async fn list_change_events(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let items = filtered_change_events(ctx, &req.query, None)?;
        classic(&req.query, "change_events", items)
    })
}

pub async fn get_change_event(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let doc = ctx.get("change_events", &id).ok_or(ApiErr::NotFound)?;
        ok(json!({ "change_event": render_change_event(ctx, doc) }))
    })
}

pub async fn list_service_change_events(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| -> Res {
        if ctx.lookup(Kind::Service, &id).is_none() {
            return Err(ApiErr::NotFound);
        }
        let items = filtered_change_events(ctx, &req.query, Some(&id))?;
        classic(&req.query, "change_events", items)
    })
}
