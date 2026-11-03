//! Shift-based (v3) schedules: `/v3/schedules` with rotations, rotation events, custom
//! shifts and overrides.
//!
//! Rendering: each rotation event repeats its `start_time`..`end_time` window by its RRULE
//! (`FREQ=DAILY|WEEKLY`, `INTERVAL`, `BYDAY`), between `effective_since` and
//! `effective_until`. A rotating strategy hands occurrence `k` to member
//! `(k / shifts_per_member) % members`; an every-member strategy puts all members on.
//! Overrides swap the overridden member for the overriding one inside their window;
//! custom shifts add one-off coverage. Times are absolute (offsets honoured); time zones
//! only label the values.

use axum::extract::{Path, State};
use axum::response::Response;
use mock_core::world::SharedWorld;
use serde_json::{Map, Value, json};

use crate::schedules::Seg;
use crate::util::{
    ApiErr, Ctx, Kind, Req, classic, created, default, matches_query, no_content, norm_time, ok,
    parse_time, run, time_of,
};

const DAY: i64 = 86_400;
const MAX_STEPS: usize = 20_000;

// ---------------------------------------------------------------------------------------
// Rendering engine
// ---------------------------------------------------------------------------------------

struct Rule {
    freq: String,
    interval: i64,
    by_day: Vec<i64>,
}

fn parse_rrule(recurrence: &Value) -> Option<Rule> {
    let line = recurrence
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .find(|l| l.starts_with("RRULE:"))?;
    let mut rule = Rule {
        freq: String::new(),
        interval: 1,
        by_day: Vec::new(),
    };
    for part in line["RRULE:".len()..].split(';') {
        let (k, v) = part.split_once('=')?;
        match k {
            "FREQ" => rule.freq = v.to_owned(),
            "INTERVAL" => rule.interval = v.parse().ok().filter(|i| *i > 0)?,
            "BYDAY" => {
                for d in v.split(',') {
                    let day = ["SU", "MO", "TU", "WE", "TH", "FR", "SA"]
                        .iter()
                        .position(|x| d.ends_with(x))?;
                    rule.by_day.push(day as i64);
                }
            }
            _ => {}
        }
    }
    Some(rule)
}

fn weekday(ts: i64) -> i64 {
    (ts.div_euclid(DAY) + 4).rem_euclid(7)
}

fn member_user(member: &Value) -> Option<String> {
    (member["type"] == "user_member")
        .then(|| member["user_id"].as_str().map(str::to_owned))
        .flatten()
}

fn event_segments(event: &Value, from: i64, to: i64) -> Vec<Seg> {
    let mut out = Vec::new();
    let (Some(start), Some(end)) = (
        event["start_time"]["date_time"]
            .as_str()
            .and_then(parse_time),
        event["end_time"]["date_time"].as_str().and_then(parse_time),
    ) else {
        return out;
    };
    let duration = end - start;
    if duration <= 0 {
        return out;
    }
    let since = time_of(event, "effective_since").unwrap_or(start);
    let until = time_of(event, "effective_until");
    let lo = from.max(since);
    let hi = until.map_or(to, |u| u.min(to));
    if lo >= hi {
        return out;
    }
    let strategy = &event["assignment_strategy"];
    let members: Vec<Option<String>> = strategy["members"]
        .as_array()
        .into_iter()
        .flatten()
        .map(member_user)
        .collect();
    if members.is_empty() {
        return out;
    }
    let every = strategy["type"] == "every_member_assignment_strategy";
    let per = strategy["shifts_per_member"]
        .as_i64()
        .filter(|n| *n > 0)
        .unwrap_or(1);
    let rule = parse_rrule(&event["recurrence"]);
    let mut push = |k: i64, s: i64| {
        let (a, b) = (s.max(lo), (s + duration).min(hi));
        if a >= b {
            return;
        }
        let users: Vec<&Option<String>> = if every {
            members.iter().collect()
        } else {
            vec![&members[((k / per) as usize) % members.len()]]
        };
        for u in users.into_iter().flatten() {
            out.push(Seg {
                user: u.clone(),
                start: a,
                end: b,
            });
        }
    };
    let Some(rule) = rule else {
        push(0, start);
        return out;
    };
    let step_days = match (rule.freq.as_str(), rule.by_day.is_empty()) {
        ("DAILY", _) => rule.interval,
        ("WEEKLY", true) => 7 * rule.interval,
        ("WEEKLY", false) => 1,
        _ => {
            push(0, start);
            return out;
        }
    };
    let mut k = 0;
    for i in 0..MAX_STEPS as i64 {
        let s = start + i * step_days * DAY;
        if s >= hi {
            break;
        }
        if rule.freq == "WEEKLY" && !rule.by_day.is_empty() {
            let weeks = (s - start).div_euclid(DAY).div_euclid(7);
            if !rule.by_day.contains(&weekday(s)) || weeks % rule.interval != 0 {
                continue;
            }
        }
        push(k, s);
        k += 1;
    }
    out
}

/// Replaces `from_user` with `to_user` inside `[os, oe)` (an empty side means nobody).
fn apply_override(
    segs: Vec<Seg>,
    os: i64,
    oe: i64,
    from_user: Option<&str>,
    to_user: Option<&str>,
) -> Vec<Seg> {
    let mut out = Vec::new();
    for s in segs {
        if Some(s.user.as_str()) != from_user || s.end <= os || s.start >= oe {
            out.push(s);
            continue;
        }
        if s.start < os {
            out.push(Seg {
                user: s.user.clone(),
                start: s.start,
                end: os,
            });
        }
        if let Some(u) = to_user {
            out.push(Seg {
                user: u.to_owned(),
                start: s.start.max(os),
                end: s.end.min(oe),
            });
        }
        if s.end > oe {
            out.push(Seg {
                user: s.user.clone(),
                start: oe,
                end: s.end,
            });
        }
    }
    if from_user.is_none()
        && let Some(u) = to_user
    {
        out.push(Seg {
            user: u.to_owned(),
            start: os,
            end: oe,
        });
    }
    out
}

fn overrides_for<'a>(
    overrides: &'a [Value],
    field: &str,
    id: &str,
) -> impl Iterator<Item = &'a Value> {
    overrides.iter().filter(move |o| o[field] == id)
}

fn with_overrides<'a>(
    mut segs: Vec<Seg>,
    overrides: impl Iterator<Item = &'a Value>,
    from: i64,
    to: i64,
) -> Vec<Seg> {
    for o in overrides {
        let (Some(os), Some(oe)) = (time_of(o, "start_time"), time_of(o, "end_time")) else {
            continue;
        };
        let (os, oe) = (os.max(from), oe.min(to));
        if os >= oe {
            continue;
        }
        segs = apply_override(
            segs,
            os,
            oe,
            member_user(&o["overridden_member"]).as_deref(),
            member_user(&o["overriding_member"]).as_deref(),
        );
    }
    segs
}

/// Everyone on call on a v3 schedule within `[from, to)`; segments may overlap.
pub fn v3_segments(ctx: &Ctx, schedule: &Value, from: i64, to: i64) -> Vec<Seg> {
    let id = schedule["id"].as_str().unwrap_or_default();
    let overrides: Vec<Value> = ctx
        .children("v3_overrides", id)
        .into_iter()
        .map(|(_, o)| o)
        .collect();
    let mut out = Vec::new();
    for rotation in schedule["rotations"].as_array().into_iter().flatten() {
        let rid = rotation["id"].as_str().unwrap_or_default();
        let segs: Vec<Seg> = rotation["events"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|e| event_segments(e, from, to))
            .collect();
        out.extend(with_overrides(
            segs,
            overrides_for(&overrides, "rotation_id", rid),
            from,
            to,
        ));
    }
    for (_, cs) in ctx.children("v3_custom_shifts", id) {
        let (Some(s), Some(e)) = (time_of(&cs, "start_time"), time_of(&cs, "end_time")) else {
            continue;
        };
        let (s, e) = (s.max(from), e.min(to));
        if s >= e {
            continue;
        }
        let segs: Vec<Seg> = cs["assignments"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| member_user(&a["member"]))
            .map(|user| Seg {
                user,
                start: s,
                end: e,
            })
            .collect();
        let csid = cs["id"].as_str().unwrap_or_default();
        out.extend(with_overrides(
            segs,
            overrides_for(&overrides, "custom_shift_id", csid),
            from,
            to,
        ));
    }
    out
}

pub fn v3_user_ids(ctx: &Ctx, schedule: &Value) -> Vec<String> {
    let id = schedule["id"].as_str().unwrap_or_default();
    let mut out = Vec::new();
    for rotation in schedule["rotations"].as_array().into_iter().flatten() {
        for e in rotation["events"].as_array().into_iter().flatten() {
            out.extend(
                e["assignment_strategy"]["members"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(member_user),
            );
        }
    }
    for (_, cs) in ctx.children("v3_custom_shifts", id) {
        out.extend(
            cs["assignments"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| member_user(&a["member"])),
        );
    }
    for (_, o) in ctx.children("v3_overrides", id) {
        out.extend(member_user(&o["overriding_member"]));
    }
    out
}

// ---------------------------------------------------------------------------------------
// Rendering of documents
// ---------------------------------------------------------------------------------------

fn sched_url(ctx: &Ctx, id: &str) -> String {
    format!("{}/v3/schedules/{id}", ctx.api)
}

fn render_event(ctx: &Ctx, sid: &str, rid: &str, mut e: Value) -> Value {
    let eid = e["id"].as_str().unwrap_or_default().to_owned();
    default(&mut e, "type", json!("rotation_event"));
    default(&mut e, "effective_until", Value::Null);
    e["self"] = json!(format!(
        "{}/rotations/{rid}/events/{eid}",
        sched_url(ctx, sid)
    ));
    e["html_url"] = json!(format!("{}/schedules/{sid}", ctx.web));
    e
}

fn render_rotation(ctx: &Ctx, sid: &str, mut r: Value) -> Value {
    let rid = r["id"].as_str().unwrap_or_default().to_owned();
    default(&mut r, "type", json!("rotation"));
    default(&mut r, "events", json!([]));
    let events: Vec<Value> = r["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|e| render_event(ctx, sid, &rid, e))
        .collect();
    r["events"] = json!(events);
    r["self"] = json!(format!("{}/rotations/{rid}", sched_url(ctx, sid)));
    r["html_url"] = json!(format!("{}/schedules/{sid}", ctx.web));
    r
}

fn render_schedule(ctx: &Ctx, mut s: Value) -> Value {
    let sid = s["id"].as_str().unwrap_or_default().to_owned();
    ctx.base_fields(&mut s, Kind::ScheduleV3);
    default(&mut s, "description", Value::Null);
    default(&mut s, "teams", json!([]));
    ctx.fix_refs(&mut s["teams"], Kind::Team);
    default(&mut s, "rotations", json!([]));
    let rotations: Vec<Value> = s["rotations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|r| render_rotation(ctx, &sid, r))
        .collect();
    s["rotations"] = json!(rotations);
    s
}

fn render_child(ctx: &Ctx, sid: &str, kind: &str, mut doc: Value) -> Value {
    let id = doc["id"].as_str().unwrap_or_default().to_owned();
    doc["self"] = json!(format!("{}/{kind}/{id}", sched_url(ctx, sid)));
    doc["html_url"] = json!(format!("{}/schedules/{sid}", ctx.web));
    doc
}

// ---------------------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------------------

fn check_member(ctx: &Ctx, member: &Value, what: &str, errors: &mut Vec<String>) {
    match member["type"].as_str() {
        Some("user_member") => match member["user_id"].as_str() {
            Some(u) if ctx.exists("users", u) => {}
            Some(u) => errors.push(format!("{what}: user {u} not found.")),
            None => errors.push(format!("{what}: user_id is required for user_member.")),
        },
        Some("empty_member") => {}
        _ => errors.push(format!("{what}: type must be user_member or empty_member.")),
    }
}

fn zoned(value: &Value, what: &str, errors: &mut Vec<String>) -> Option<Value> {
    match (
        value["date_time"].as_str().and_then(parse_time),
        value["time_zone"].as_str(),
    ) {
        (Some(t), Some(tz)) if !tz.is_empty() => {
            Some(json!({ "date_time": crate::util::fmt(t), "time_zone": tz }))
        }
        _ => {
            errors.push(format!("{what} must have a valid date_time and time_zone."));
            None
        }
    }
}

/// Validates and normalizes an event payload (id assigned by the caller).
fn event_input(ctx: &Ctx, input: &Value) -> Result<Value, ApiErr> {
    let mut errors = Vec::new();
    let name = input["name"].as_str().unwrap_or_default();
    if name.trim().is_empty() || name.len() > 255 {
        errors.push("Name must be between 1 and 255 characters.".to_owned());
    }
    let start = zoned(&input["start_time"], "start_time", &mut errors);
    let end = zoned(&input["end_time"], "end_time", &mut errors);
    if let (Some(s), Some(e)) = (&start, &end)
        && parse_time(e["date_time"].as_str().unwrap_or_default())
            <= parse_time(s["date_time"].as_str().unwrap_or_default())
    {
        errors.push("end_time must be after start_time.".to_owned());
    }
    let since = norm_time(&input["effective_since"]);
    if since.is_none() {
        errors.push("effective_since must be a valid date/time.".to_owned());
    }
    let until = match &input["effective_until"] {
        Value::Null => None,
        v => match norm_time(v) {
            Some(t) => Some(t),
            None => {
                errors.push("effective_until must be a valid date/time.".to_owned());
                None
            }
        },
    };
    let rrules = input["recurrence"]
        .as_array()
        .map(|r| {
            r.iter()
                .filter(|l| l.as_str().is_some_and(|l| l.starts_with("RRULE:")))
                .count()
        })
        .unwrap_or(0);
    if rrules != 1 || parse_rrule(&input["recurrence"]).is_none() {
        errors.push("recurrence must contain exactly one valid RRULE.".to_owned());
    }
    let strategy = &input["assignment_strategy"];
    let members = strategy["members"].as_array().cloned().unwrap_or_default();
    match strategy["type"].as_str() {
        Some("rotating_member_assignment_strategy") => {
            if strategy["shifts_per_member"].as_i64().is_none_or(|n| n < 1) {
                errors.push(
                    "shifts_per_member is required for rotating_member_assignment_strategy."
                        .to_owned(),
                );
            }
        }
        Some("every_member_assignment_strategy") => {}
        _ => errors.push("assignment_strategy type is invalid.".to_owned()),
    }
    if members.is_empty() || members.len() > 20 {
        errors.push("assignment_strategy must have between 1 and 20 members.".to_owned());
    }
    for m in &members {
        check_member(ctx, m, "assignment_strategy member", &mut errors);
    }
    if !errors.is_empty() {
        return Err(ApiErr::Invalid(errors));
    }
    let mut strategy_out = json!({ "type": strategy["type"], "members": members });
    if let Some(n) = strategy["shifts_per_member"].as_i64() {
        strategy_out["shifts_per_member"] = json!(n);
    }
    Ok(json!({
        "name": name.trim(),
        "start_time": start,
        "end_time": end,
        "effective_since": since,
        "effective_until": until,
        "recurrence": input["recurrence"],
        "assignment_strategy": strategy_out,
    }))
}

fn new_event(ctx: &mut Ctx, input: &Value) -> Result<Value, ApiErr> {
    let mut e = event_input(ctx, input)?;
    let id = ctx.new_id("v3_events");
    let mut out = json!({ "id": id, "type": "rotation_event" });
    if let (Some(o), Some(fields)) = (out.as_object_mut(), e.as_object_mut()) {
        o.append(fields);
    }
    Ok(out)
}

fn new_rotation(ctx: &mut Ctx, input: &Value) -> Result<Value, ApiErr> {
    let mut events = Vec::new();
    for e in input["events"].as_array().into_iter().flatten() {
        events.push(new_event(ctx, e)?);
    }
    let id = ctx.new_id("v3_rotations");
    Ok(json!({ "id": id, "type": "rotation", "events": events }))
}

fn schedule_fields(input: &Value, partial: bool) -> Result<Map<String, Value>, ApiErr> {
    let mut errors = Vec::new();
    let mut out = Map::new();
    match input["name"].as_str().map(str::trim) {
        Some(n) if !n.is_empty() => {
            out.insert("name".into(), json!(n));
        }
        None if partial => {}
        _ => errors.push("Name cannot be empty.".to_owned()),
    }
    match input["time_zone"].as_str().map(str::trim) {
        Some(tz) if !tz.is_empty() => {
            out.insert("time_zone".into(), json!(tz));
        }
        None if partial => {}
        _ => errors.push("Time zone cannot be empty.".to_owned()),
    }
    if !errors.is_empty() {
        return Err(ApiErr::Invalid(errors));
    }
    if let Some(d) = input.get("description") {
        out.insert("description".into(), d.clone());
    }
    if let Some(teams) = input["teams"].as_array() {
        let refs: Vec<Value> = teams
            .iter()
            .filter_map(|t| t["id"].as_str())
            .map(|id| json!({ "id": id, "type": "team_reference" }))
            .collect();
        out.insert("teams".into(), json!(refs));
    }
    Ok(out)
}

fn v3_schedule(ctx: &Ctx, id: &str) -> Result<Value, ApiErr> {
    if let Some(s) = ctx.get("schedules_v3", id) {
        return Ok(s);
    }
    if ctx.exists("schedules", id) {
        return Err(ApiErr::bad_request(
            "This is a layer-based schedule. Use the v2 Schedules API to access it.",
        ));
    }
    Err(ApiErr::NotFound)
}

fn rotation_index(schedule: &Value, rid: &str) -> Result<usize, ApiErr> {
    schedule["rotations"]
        .as_array()
        .and_then(|rs| rs.iter().position(|r| r["id"] == rid))
        .ok_or(ApiErr::NotFound)
}

fn event_index(schedule: &Value, ri: usize, eid: &str) -> Result<usize, ApiErr> {
    schedule["rotations"][ri]["events"]
        .as_array()
        .and_then(|es| es.iter().position(|e| e["id"] == eid))
        .ok_or(ApiErr::NotFound)
}

// ---------------------------------------------------------------------------------------
// Schedules
// ---------------------------------------------------------------------------------------

pub async fn list(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let q = &req.query;
        let items: Vec<Value> = ctx
            .docs("schedules_v3")
            .into_iter()
            .filter(|s| matches_query(q.get("query"), &[&s["name"]]))
            .map(|s| {
                let id = s["id"].as_str().unwrap_or_default();
                json!({
                    "id": id,
                    "type": "schedule",
                    "self": sched_url(ctx, id),
                    "html_url": format!("{}/schedules/{id}", ctx.web),
                })
            })
            .collect();
        classic(q, "schedules", items)
    })
}

pub async fn create(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let body = req.json()?;
        let input = body.get("schedule").unwrap_or(&body);
        let mut fields = schedule_fields(input, false)?;
        let mut rotations = Vec::new();
        for r in input["rotations"].as_array().into_iter().flatten() {
            rotations.push(new_rotation(ctx, r)?);
        }
        let id = ctx.new_id("schedules_v3");
        let mut doc = json!({ "id": id, "type": "schedule" });
        if let Some(o) = doc.as_object_mut() {
            o.append(&mut fields);
            o.entry("description").or_insert(Value::Null);
            o.entry("teams").or_insert(json!([]));
            o.insert("rotations".into(), json!(rotations));
        }
        ctx.put("schedules_v3", &id, doc.clone());
        created(json!({ "schedule": render_schedule(ctx, doc) }))
    })
}

pub async fn get(State(world): State<SharedWorld>, Path(id): Path<String>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let s = v3_schedule(ctx, &id)?;
        ok(json!({ "schedule": render_schedule(ctx, s) }))
    })
}

pub async fn update(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let mut s = v3_schedule(ctx, &id)?;
        let body = req.json()?;
        let input = body.get("schedule").unwrap_or(&body);
        let mut fields = schedule_fields(input, false)?;
        if let Some(rs) = input["rotations"].as_array() {
            let mut rotations = Vec::new();
            for r in rs {
                rotations.push(new_rotation(ctx, r)?);
            }
            s["rotations"] = json!(rotations);
        }
        if let Some(o) = s.as_object_mut() {
            o.append(&mut fields);
        }
        ctx.put("schedules_v3", &id, s.clone());
        ok(json!({ "schedule": render_schedule(ctx, s) }))
    })
}

pub async fn delete(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        v3_schedule(ctx, &id)?;
        ctx.remove("schedules_v3", &id);
        for coll in ["v3_custom_shifts", "v3_overrides"] {
            for (k, _) in ctx.children(coll, &id) {
                ctx.remove(coll, &k);
            }
        }
        no_content()
    })
}

// ---------------------------------------------------------------------------------------
// Rotations and events
// ---------------------------------------------------------------------------------------

pub async fn list_rotations(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let s = v3_schedule(ctx, &id)?;
        let rotations: Vec<Value> = s["rotations"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|r| render_rotation(ctx, &id, r))
            .collect();
        ok(json!({ "rotations": rotations }))
    })
}

pub async fn create_rotation(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let mut s = v3_schedule(ctx, &id)?;
        let body = req.json()?;
        let rotation = new_rotation(ctx, body.get("rotation").unwrap_or(&body))?;
        default(&mut s, "rotations", json!([]));
        if let Some(rs) = s["rotations"].as_array_mut() {
            rs.push(rotation.clone());
        }
        ctx.put("schedules_v3", &id, s);
        created(json!({ "rotation": render_rotation(ctx, &id, rotation) }))
    })
}

pub async fn get_rotation(
    State(world): State<SharedWorld>,
    Path((id, rid)): Path<(String, String)>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let s = v3_schedule(ctx, &id)?;
        let ri = rotation_index(&s, &rid)?;
        ok(json!({ "rotation": render_rotation(ctx, &id, s["rotations"][ri].clone()) }))
    })
}

pub async fn delete_rotation(
    State(world): State<SharedWorld>,
    Path((id, rid)): Path<(String, String)>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let mut s = v3_schedule(ctx, &id)?;
        let ri = rotation_index(&s, &rid)?;
        if let Some(rs) = s["rotations"].as_array_mut() {
            rs.remove(ri);
        }
        ctx.put("schedules_v3", &id, s);
        for (k, o) in ctx.children("v3_overrides", &id) {
            if o["rotation_id"] == rid.as_str() {
                ctx.remove("v3_overrides", &k);
            }
        }
        no_content()
    })
}

pub async fn list_events(
    State(world): State<SharedWorld>,
    Path((id, rid)): Path<(String, String)>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let s = v3_schedule(ctx, &id)?;
        let ri = rotation_index(&s, &rid)?;
        let events: Vec<Value> = s["rotations"][ri]["events"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|e| render_event(ctx, &id, &rid, e))
            .collect();
        ok(json!({ "events": events }))
    })
}

pub async fn create_event(
    State(world): State<SharedWorld>,
    Path((id, rid)): Path<(String, String)>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let mut s = v3_schedule(ctx, &id)?;
        let ri = rotation_index(&s, &rid)?;
        let body = req.json()?;
        let event = new_event(ctx, body.get("event").unwrap_or(&body))?;
        default(&mut s["rotations"][ri], "events", json!([]));
        if let Some(es) = s["rotations"][ri]["events"].as_array_mut() {
            es.push(event.clone());
        }
        ctx.put("schedules_v3", &id, s);
        created(json!({ "event": render_event(ctx, &id, &rid, event) }))
    })
}

pub async fn get_event(
    State(world): State<SharedWorld>,
    Path((id, rid, eid)): Path<(String, String, String)>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let s = v3_schedule(ctx, &id)?;
        let ri = rotation_index(&s, &rid)?;
        let ei = event_index(&s, ri, &eid)?;
        let e = s["rotations"][ri]["events"][ei].clone();
        ok(json!({ "event": render_event(ctx, &id, &rid, e) }))
    })
}

pub async fn update_event(
    State(world): State<SharedWorld>,
    Path((id, rid, eid)): Path<(String, String, String)>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let mut s = v3_schedule(ctx, &id)?;
        let ri = rotation_index(&s, &rid)?;
        let ei = event_index(&s, ri, &eid)?;
        let body = req.json()?;
        let input = body.get("event").unwrap_or(&body);
        let current = s["rotations"][ri]["events"][ei].clone();
        if time_of(&current, "effective_until").is_some_and(|u| u <= ctx.now) {
            return Err(ApiErr::invalid("Past events cannot be updated."));
        }
        let mut fields = event_input(ctx, input)?;
        let updated = if time_of(&current, "effective_since").is_some_and(|t| t <= ctx.now) {
            // Active event: only effective_until is honoured.
            let mut e = current.clone();
            e["effective_until"] = fields["effective_until"].take();
            e
        } else {
            let mut e = json!({ "id": eid, "type": "rotation_event" });
            if let (Some(o), Some(f)) = (e.as_object_mut(), fields.as_object_mut()) {
                o.append(f);
            }
            e
        };
        s["rotations"][ri]["events"][ei] = updated.clone();
        ctx.put("schedules_v3", &id, s);
        ok(json!({ "event": render_event(ctx, &id, &rid, updated) }))
    })
}

pub async fn delete_event(
    State(world): State<SharedWorld>,
    Path((id, rid, eid)): Path<(String, String, String)>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let mut s = v3_schedule(ctx, &id)?;
        let ri = rotation_index(&s, &rid)?;
        let ei = event_index(&s, ri, &eid)?;
        if let Some(es) = s["rotations"][ri]["events"].as_array_mut() {
            es.remove(ei);
        }
        ctx.put("schedules_v3", &id, s);
        no_content()
    })
}

// ---------------------------------------------------------------------------------------
// Custom shifts and overrides
// ---------------------------------------------------------------------------------------

fn range(q: &crate::util::Query) -> Result<(i64, i64), ApiErr> {
    match (q.time("since")?, q.time("until")?) {
        (Some(s), Some(u)) if u > s => Ok((s, u)),
        (Some(_), Some(_)) => Err(ApiErr::invalid("until must be after since.")),
        _ => Err(ApiErr::invalid("since and until are required.")),
    }
}

fn overlapping(ctx: &Ctx, coll: &str, id: &str, since: i64, until: i64) -> Vec<Value> {
    ctx.children(coll, id)
        .into_iter()
        .map(|(_, d)| d)
        .filter(|d| {
            time_of(d, "start_time").is_some_and(|s| s < until)
                && time_of(d, "end_time").is_some_and(|e| e > since)
        })
        .collect()
}

fn shift_window(input: &Value, errors: &mut Vec<String>) -> (Option<String>, Option<String>) {
    let start = norm_time(&input["start_time"]);
    let end = norm_time(&input["end_time"]);
    match (&start, &end) {
        (Some(s), Some(e)) if parse_time(e) <= parse_time(s) => {
            errors.push("end_time must be after start_time.".to_owned())
        }
        (Some(_), Some(_)) => {}
        _ => errors.push("start_time and end_time must be valid date/times.".to_owned()),
    }
    (start, end)
}

fn custom_shift_input(ctx: &Ctx, input: &Value) -> Result<Value, ApiErr> {
    let mut errors = Vec::new();
    let (start, end) = shift_window(input, &mut errors);
    let assignments = input["assignments"].as_array().cloned().unwrap_or_default();
    if assignments.len() != 1 {
        errors.push("A custom shift must have exactly one assignment.".to_owned());
    }
    for a in &assignments {
        check_member(ctx, &a["member"], "assignment member", &mut errors);
    }
    if !errors.is_empty() {
        return Err(ApiErr::Invalid(errors));
    }
    let assignments: Vec<Value> = assignments
        .iter()
        .map(|a| json!({ "type": "shift_assignment", "member": a["member"] }))
        .collect();
    Ok(json!({
        "type": "custom_shift",
        "start_time": start,
        "end_time": end,
        "assignments": assignments,
    }))
}

pub async fn list_custom_shifts(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        v3_schedule(ctx, &id)?;
        let (since, until) = range(&req.query)?;
        let items: Vec<Value> = overlapping(ctx, "v3_custom_shifts", &id, since, until)
            .into_iter()
            .map(|d| render_child(ctx, &id, "custom_shifts", d))
            .collect();
        ok(json!({ "custom_shifts": items }))
    })
}

pub async fn create_custom_shifts(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        v3_schedule(ctx, &id)?;
        let body = req.json()?;
        let Some(items) = body["custom_shifts"].as_array().filter(|a| !a.is_empty()) else {
            return Err(ApiErr::invalid("custom_shifts must not be empty."));
        };
        let mut docs = Vec::new();
        for item in items {
            docs.push(custom_shift_input(ctx, item)?);
        }
        let mut out = Vec::new();
        for mut doc in docs {
            let cid = ctx.new_id("v3_custom_shifts");
            doc["id"] = json!(cid);
            let key = ctx.next_child_key("v3_custom_shifts", &id);
            ctx.put("v3_custom_shifts", &key, doc.clone());
            out.push(render_child(ctx, &id, "custom_shifts", doc));
        }
        created(json!({ "custom_shifts": out }))
    })
}

pub async fn get_custom_shift(
    State(world): State<SharedWorld>,
    Path((id, cid)): Path<(String, String)>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        v3_schedule(ctx, &id)?;
        let (_, doc) = ctx
            .find_child("v3_custom_shifts", &id, &cid)
            .ok_or(ApiErr::NotFound)?;
        ok(json!({ "custom_shift": render_child(ctx, &id, "custom_shifts", doc) }))
    })
}

pub async fn update_custom_shift(
    State(world): State<SharedWorld>,
    Path((id, cid)): Path<(String, String)>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        v3_schedule(ctx, &id)?;
        let (key, _) = ctx
            .find_child("v3_custom_shifts", &id, &cid)
            .ok_or(ApiErr::NotFound)?;
        let body = req.json()?;
        let mut doc = custom_shift_input(ctx, body.get("custom_shift").unwrap_or(&body))?;
        doc["id"] = json!(cid);
        ctx.put("v3_custom_shifts", &key, doc.clone());
        ok(json!({ "custom_shift": render_child(ctx, &id, "custom_shifts", doc) }))
    })
}

pub async fn delete_custom_shift(
    State(world): State<SharedWorld>,
    Path((id, cid)): Path<(String, String)>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        v3_schedule(ctx, &id)?;
        let (key, _) = ctx
            .find_child("v3_custom_shifts", &id, &cid)
            .ok_or(ApiErr::NotFound)?;
        ctx.remove("v3_custom_shifts", &key);
        no_content()
    })
}

fn override_input(
    ctx: &Ctx,
    schedule: &Value,
    input: &Value,
    update: bool,
) -> Result<Value, ApiErr> {
    let sid = schedule["id"].as_str().unwrap_or_default();
    let mut errors = Vec::new();
    let (start, end) = shift_window(input, &mut errors);
    check_member(
        ctx,
        &input["overridden_member"],
        "overridden_member",
        &mut errors,
    );
    check_member(
        ctx,
        &input["overriding_member"],
        "overriding_member",
        &mut errors,
    );
    let rotation = input["rotation_id"].as_str();
    let custom = input["custom_shift_id"].as_str();
    if !update {
        match (rotation, custom) {
            (Some(r), None) => {
                if rotation_index(schedule, r).is_err() {
                    errors.push(format!("Rotation {r} not found."));
                }
            }
            (None, Some(c)) => {
                if ctx.find_child("v3_custom_shifts", sid, c).is_none() {
                    errors.push(format!("Custom shift {c} not found."));
                }
            }
            _ => errors
                .push("Exactly one of rotation_id or custom_shift_id must be provided.".to_owned()),
        }
    }
    if !errors.is_empty() {
        return Err(ApiErr::Invalid(errors));
    }
    let member = |m: &Value| {
        let mut out = json!({ "type": m["type"] });
        if let Some(u) = m["user_id"].as_str() {
            out["user_id"] = json!(u);
        }
        out
    };
    Ok(json!({
        "type": "override_shift",
        "rotation_id": rotation,
        "custom_shift_id": custom,
        "start_time": start,
        "end_time": end,
        "overridden_member": member(&input["overridden_member"]),
        "overriding_member": member(&input["overriding_member"]),
    }))
}

pub async fn list_overrides(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        v3_schedule(ctx, &id)?;
        let (since, until) = range(&req.query)?;
        let items: Vec<Value> = overlapping(ctx, "v3_overrides", &id, since, until)
            .into_iter()
            .map(|d| render_child(ctx, &id, "overrides", d))
            .collect();
        ok(json!({ "overrides": items }))
    })
}

pub async fn create_overrides(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let s = v3_schedule(ctx, &id)?;
        let body = req.json()?;
        let Some(items) = body["overrides"].as_array().filter(|a| !a.is_empty()) else {
            return Err(ApiErr::invalid("overrides must not be empty."));
        };
        let mut docs = Vec::new();
        for item in items {
            docs.push(override_input(ctx, &s, item, false)?);
        }
        let mut out = Vec::new();
        for mut doc in docs {
            let oid = ctx.new_id("v3_overrides");
            doc["id"] = json!(oid);
            let key = ctx.next_child_key("v3_overrides", &id);
            ctx.put("v3_overrides", &key, doc.clone());
            out.push(render_child(ctx, &id, "overrides", doc));
        }
        created(json!({ "overrides": out }))
    })
}

pub async fn get_override(
    State(world): State<SharedWorld>,
    Path((id, oid)): Path<(String, String)>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        v3_schedule(ctx, &id)?;
        let (_, doc) = ctx
            .find_child("v3_overrides", &id, &oid)
            .ok_or(ApiErr::NotFound)?;
        ok(json!({ "override": render_child(ctx, &id, "overrides", doc) }))
    })
}

pub async fn update_override(
    State(world): State<SharedWorld>,
    Path((id, oid)): Path<(String, String)>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let s = v3_schedule(ctx, &id)?;
        let (key, current) = ctx
            .find_child("v3_overrides", &id, &oid)
            .ok_or(ApiErr::NotFound)?;
        let body = req.json()?;
        let mut doc = override_input(ctx, &s, body.get("override").unwrap_or(&body), true)?;
        doc["id"] = json!(oid);
        doc["rotation_id"] = current["rotation_id"].clone();
        doc["custom_shift_id"] = current["custom_shift_id"].clone();
        ctx.put("v3_overrides", &key, doc.clone());
        ok(json!({ "override": render_child(ctx, &id, "overrides", doc) }))
    })
}

pub async fn delete_override(
    State(world): State<SharedWorld>,
    Path((id, oid)): Path<(String, String)>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        v3_schedule(ctx, &id)?;
        let (key, _) = ctx
            .find_child("v3_overrides", &id, &oid)
            .ok_or(ApiErr::NotFound)?;
        ctx.remove("v3_overrides", &key);
        no_content()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekly_rotating_event_hands_out_turns() {
        let event = json!({
            "start_time": {"date_time": "2026-10-26T09:00:00Z", "time_zone": "UTC"},
            "end_time": {"date_time": "2026-11-02T09:00:00Z", "time_zone": "UTC"},
            "effective_since": "2026-10-26T09:00:00Z",
            "recurrence": ["RRULE:FREQ=WEEKLY"],
            "assignment_strategy": {
                "type": "rotating_member_assignment_strategy",
                "shifts_per_member": 1,
                "members": [{"type": "user_member", "user_id": "A"}, {"type": "user_member", "user_id": "B"}]
            }
        });
        let t0 = parse_time("2026-10-26T09:00:00Z").unwrap();
        let segs = event_segments(&event, t0, t0 + 21 * DAY);
        let users: Vec<&str> = segs.iter().map(|s| s.user.as_str()).collect();
        assert_eq!(users, ["A", "B", "A"]);
    }

    #[test]
    fn byday_weekly_only_listed_days() {
        let event = json!({
            "start_time": {"date_time": "2026-10-26T09:00:00Z", "time_zone": "UTC"},
            "end_time": {"date_time": "2026-10-26T17:00:00Z", "time_zone": "UTC"},
            "effective_since": "2026-10-26T00:00:00Z",
            "recurrence": ["RRULE:FREQ=WEEKLY;BYDAY=MO,WE"],
            "assignment_strategy": {"type": "every_member_assignment_strategy", "members": [{"type": "user_member", "user_id": "A"}]}
        });
        let t0 = parse_time("2026-10-26T00:00:00Z").unwrap();
        let segs = event_segments(&event, t0, t0 + 7 * DAY);
        assert_eq!(segs.len(), 2);
        assert_eq!(weekday(segs[1].start), 3);
    }

    #[test]
    fn override_splits_segment() {
        let segs = vec![Seg {
            user: "A".into(),
            start: 0,
            end: 10,
        }];
        let out = apply_override(segs, 3, 6, Some("A"), Some("B"));
        let users: Vec<(&str, i64, i64)> = out
            .iter()
            .map(|s| (s.user.as_str(), s.start, s.end))
            .collect();
        assert_eq!(users, [("A", 0, 3), ("B", 3, 6), ("A", 6, 10)]);
    }
}
