//! v1 downtimes: list, get, create (`POST /api/v1/downtime`) and cancel
//! (`DELETE /api/v1/downtime/{id}`, which keeps the record with `canceled` set).

use axum::{
    body::Bytes,
    extract::{Path, RawQuery, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use mock_core::{http::json, world::SharedWorld};
use serde_json::{Value, json as json_value};

use crate::{
    NAME,
    support::{OK, bool_param, doc, docs, errors, parse_query},
};

/// User id behind the benchmark's application key; recorded as creator/updater.
pub const CURRENT_USER_ID: i64 = 1_445_416;

/// Not canceled or disabled, and not yet over.
pub fn is_live(d: &Value, now: i64) -> bool {
    d["canceled"].is_null()
        && !d["disabled"].as_bool().unwrap_or(false)
        && d["end"].as_i64().is_none_or(|end| end > now)
}

fn is_active(d: &Value, now: i64) -> bool {
    is_live(d, now) && d["start"].as_i64().is_none_or(|s| s <= now)
}

fn render(d: &Value, now: i64) -> Value {
    let mut out = d.clone();
    out["active"] = json_value!(is_active(d, now));
    out
}

pub async fn list(State(world): State<SharedWorld>, RawQuery(raw): RawQuery) -> Response {
    let params = parse_query(raw.as_deref());
    let current_only = bool_param(&params, "current_only").unwrap_or(false);
    let world = world.lock().expect("world lock");
    let now = world.clock.now_unix();
    let list: Vec<Value> = docs(&world, "downtimes")
        .iter()
        .filter(|d| !current_only || is_active(d, now))
        .map(|d| render(d, now))
        .collect();
    json(OK, Value::Array(list))
}

fn downtime_not_found() -> Response {
    errors::error(StatusCode::NOT_FOUND, "Downtime not found")
}

pub async fn get(State(world): State<SharedWorld>, Path(downtime_id): Path<String>) -> Response {
    let world = world.lock().expect("world lock");
    match doc(&world, "downtimes", &downtime_id) {
        Some(d) => json(OK, render(&d, world.clock.now_unix())),
        None => downtime_not_found(),
    }
}

fn epoch(v: Option<&Value>, field: &str) -> Result<Option<i64>, Response> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_f64().map(|f| Some(f as i64)).ok_or_else(|| {
            errors::bad_request(&format!(
                "Invalid value for '{field}': expected a POSIX timestamp"
            ))
        }),
    }
}

fn strings(v: Option<&Value>, field: &str) -> Result<Option<Vec<String>>, Response> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => items
            .iter()
            .map(|i| i.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .map(Some)
            .ok_or_else(|| errors::bad_request(&format!("Invalid value for '{field}'"))),
        Some(Value::String(s)) => Ok(Some(s.split(',').map(|p| p.trim().to_owned()).collect())),
        Some(_) => Err(errors::bad_request(&format!("Invalid value for '{field}'"))),
    }
}

fn recurrence(v: Option<&Value>, start: i64) -> Result<Value, Response> {
    let Some(r) = v.filter(|r| !r.is_null()) else {
        return Ok(Value::Null);
    };
    let kind = r["type"].as_str().unwrap_or_default();
    if !["days", "weeks", "months", "years", "rrule"].contains(&kind) {
        return Err(errors::bad_request(
            "Invalid recurrence type: expected one of days, weeks, months, years, rrule",
        ));
    }
    let period = r["period"].as_i64().unwrap_or(1);
    if period < 1 {
        return Err(errors::bad_request("Recurrence period must be at least 1"));
    }
    let until_date = epoch(r.get("until_date"), "until_date")?;
    if until_date.is_some_and(|u| u <= start) {
        return Err(errors::bad_request(
            "Recurrence until_date must be after start",
        ));
    }
    Ok(json_value!({
        "period": period,
        "rrule": r.get("rrule").cloned().unwrap_or(Value::Null),
        "type": kind,
        "until_date": until_date,
        "until_occurrences": r.get("until_occurrences").cloned().unwrap_or(Value::Null),
        "week_days": r.get("week_days").cloned().unwrap_or(Value::Null),
    }))
}

pub async fn create(State(world): State<SharedWorld>, body: Bytes) -> Response {
    let Ok(input) = serde_json::from_slice::<Value>(&body) else {
        return errors::bad_request("Invalid JSON body");
    };
    if !input.is_object() {
        return errors::bad_request("Invalid JSON body");
    }
    let scope = match strings(input.get("scope"), "scope") {
        // `"host:a,env:prod"` in one entry means two scope tags.
        Ok(Some(s)) if !s.is_empty() && s.iter().all(|p| !p.trim().is_empty()) => s
            .iter()
            .flat_map(|p| p.split(','))
            .map(|p| p.trim().to_owned())
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>(),
        Ok(_) => return errors::bad_request("Invalid scope: a downtime requires a scope"),
        Err(r) => return r,
    };
    let (start, end) = match (
        epoch(input.get("start"), "start"),
        epoch(input.get("end"), "end"),
    ) {
        (Ok(s), Ok(e)) => (s, e),
        (Err(r), _) | (_, Err(r)) => return r,
    };
    let monitor_tags = match strings(input.get("monitor_tags"), "monitor_tags") {
        Ok(t) => t,
        Err(r) => return r,
    };
    let monitor_id = match input.get("monitor_id") {
        None | Some(Value::Null) => None,
        Some(v) => match v.as_f64() {
            Some(f) if f.fract() == 0.0 => Some(f as i64),
            _ => return errors::bad_request("Invalid value for 'monitor_id'"),
        },
    };

    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    let now = world.clock.now_unix();
    let start = start.unwrap_or(now);
    if end.is_some_and(|e| e <= start) {
        return errors::bad_request("Downtime end time must be after the start time");
    }
    if end.is_some_and(|e| e <= now) {
        return errors::bad_request("Downtime end time must be in the future");
    }
    let rec = match recurrence(input.get("recurrence"), start) {
        Ok(r) => r,
        Err(r) => return r,
    };
    if let Some(id) = monitor_id
        && doc(world, "monitors", &id.to_string()).is_none()
    {
        return errors::bad_request(&format!("Monitor {id} does not exist"));
    }
    let id = docs(world, "downtimes")
        .iter()
        .filter_map(|d| d["id"].as_i64())
        .max()
        .unwrap_or(0)
        + 1;
    let downtime = json_value!({
        "active": false,
        "active_child": null,
        "canceled": null,
        "creator_id": CURRENT_USER_ID,
        "disabled": false,
        "downtime_type": if monitor_id.is_some() { 2 } else { 0 },
        "end": end,
        "id": id,
        "message": input.get("message").cloned().unwrap_or(Value::Null),
        "monitor_id": monitor_id,
        "monitor_tags": monitor_tags.unwrap_or_else(|| vec!["*".to_owned()]),
        "mute_first_recovery_notification": input["mute_first_recovery_notification"].as_bool().unwrap_or(false),
        "notify_end_states": input.get("notify_end_states").cloned().unwrap_or_else(|| json_value!(["alert", "no data", "warn"])),
        "notify_end_types": input.get("notify_end_types").cloned().unwrap_or_else(|| json_value!(["expired"])),
        "parent_id": null,
        "recurrence": rec,
        "scope": scope,
        "start": start,
        "timezone": input.get("timezone").and_then(Value::as_str).unwrap_or("UTC"),
        "updater_id": null,
    });
    world
        .service_mut(NAME)
        .store
        .collection_mut("downtimes")
        .insert(id.to_string(), downtime.clone());
    json(OK, render(&downtime, now))
}

pub async fn cancel(State(world): State<SharedWorld>, Path(downtime_id): Path<String>) -> Response {
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    let now = world.clock.now_unix();
    let Some(d) = world
        .service_mut(NAME)
        .store
        .collection_mut("downtimes")
        .get_mut(&downtime_id)
    else {
        return downtime_not_found();
    };
    if d["canceled"].is_null() {
        d["canceled"] = json_value!(now);
        d["disabled"] = json_value!(true);
        d["active"] = json_value!(false);
        d["updater_id"] = json_value!(CURRENT_USER_ID);
    }
    StatusCode::NO_CONTENT.into_response()
}
