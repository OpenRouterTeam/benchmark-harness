//! Layer-based (v2) schedules, their overrides, on-call rendering and `/oncalls`.
//!
//! Rendering: a layer hands out turns of `rotation_turn_length_seconds` starting at
//! `rotation_virtual_start`, cycling through its users, clipped to the layer's `start`/`end`.
//! Later layers in `schedule_layers` win over earlier ones (Layer 2 sits above Layer 1),
//! overrides win over every layer, and the most recently created override wins among
//! overlapping overrides. Layer restrictions are not applied.

use axum::extract::{Path, State};
use axum::response::Response;
use mock_core::world::SharedWorld;
use serde_json::{Value, json};

use crate::directory::{render_user, sort_by_name, target_kind};
use crate::schedules_v3::{v3_segments, v3_user_ids};
use crate::util::{
    ApiErr, Ctx, Kind, Req, classic, created, default, derived_id, fmt, no_content, ok, parse_time,
    ref_id, run, stored_ref, time_of,
};

const DAY: i64 = 86_400;
/// How far around the requested window on-call shifts are rendered, so that the shift
/// covering "now" reports its real start and end.
const CONTEXT: i64 = 35 * DAY;
const MAX_TURNS: usize = 20_000;

/// One user on call from `start` to `end`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seg {
    pub user: String,
    pub start: i64,
    pub end: i64,
}

fn layer_users(layer: &Value) -> Vec<String> {
    layer["users"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|u| ref_id(&u["user"]).or_else(|| ref_id(u)).map(str::to_owned))
        .collect()
}

fn layer_segments(layer: &Value, from: i64, to: i64) -> Vec<Seg> {
    let users = layer_users(layer);
    if users.is_empty() {
        return Vec::new();
    }
    let start = time_of(layer, "start").unwrap_or(from);
    let end = time_of(layer, "end");
    let rvs = time_of(layer, "rotation_virtual_start").unwrap_or(start);
    let turn = layer["rotation_turn_length_seconds"]
        .as_i64()
        .filter(|t| *t > 0)
        .unwrap_or(7 * DAY);
    let lo = from.max(start);
    let hi = end.map_or(to, |e| to.min(e));
    let mut out = Vec::new();
    if lo >= hi {
        return out;
    }
    let mut k = (lo - rvs).div_euclid(turn);
    while out.len() < MAX_TURNS {
        let s = rvs + k * turn;
        if s >= hi {
            break;
        }
        let user = users[k.rem_euclid(users.len() as i64) as usize].clone();
        out.push(Seg {
            user,
            start: s.max(lo),
            end: (s + turn).min(hi),
        });
        k += 1;
    }
    out
}

fn override_segments(ctx: &Ctx, schedule: &str) -> Vec<Seg> {
    ctx.children("overrides", schedule)
        .into_iter()
        .filter_map(|(_, o)| {
            Some(Seg {
                user: ref_id(&o["user"])?.to_owned(),
                start: time_of(&o, "start")?,
                end: time_of(&o, "end")?,
            })
        })
        .collect()
}

/// The final (layers + overrides) schedule of a v2 schedule within `[from, to)`.
pub fn v2_segments(ctx: &Ctx, schedule: &Value, from: i64, to: i64) -> Vec<Seg> {
    let id = schedule["id"].as_str().unwrap_or_default();
    let mut candidates: Vec<(usize, Seg)> = Vec::new();
    for (i, layer) in schedule["schedule_layers"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        candidates.extend(layer_segments(layer, from, to).into_iter().map(|s| (i, s)));
    }
    for (i, s) in override_segments(ctx, id).into_iter().enumerate() {
        candidates.push((1_000_000 + i, s));
    }
    let mut bounds = vec![from, to];
    for (_, s) in &candidates {
        bounds.extend(
            [s.start, s.end]
                .into_iter()
                .filter(|t| *t > from && *t < to),
        );
    }
    bounds.sort_unstable();
    bounds.dedup();
    let mut out: Vec<Seg> = Vec::new();
    for w in bounds.windows(2) {
        let (a, b) = (w[0], w[1]);
        let Some((_, winner)) = candidates
            .iter()
            .filter(|(_, s)| s.start <= a && a < s.end)
            .max_by_key(|(p, _)| *p)
        else {
            continue;
        };
        match out.last_mut() {
            Some(last) if last.user == winner.user && last.end == a => last.end = b,
            _ => out.push(Seg {
                user: winner.user.clone(),
                start: a,
                end: b,
            }),
        }
    }
    out
}

/// Merges each user's overlapping or touching segments.
pub fn merge_per_user(mut segs: Vec<Seg>) -> Vec<Seg> {
    segs.sort_by(|a, b| (a.start, &a.user, a.end).cmp(&(b.start, &b.user, b.end)));
    let mut out: Vec<Seg> = Vec::new();
    for s in segs {
        if let Some(prev) = out
            .iter_mut()
            .rev()
            .find(|p| p.user == s.user && p.end >= s.start)
        {
            prev.end = prev.end.max(s.end);
        } else {
            out.push(s);
        }
    }
    out.sort_by(|a, b| (a.start, &a.user).cmp(&(b.start, &b.user)));
    out
}

/// Rendered on-call segments of any schedule (v2 or v3) within `[from, to)`.
pub fn schedule_segments(ctx: &Ctx, id: &str, from: i64, to: i64) -> Option<Vec<Seg>> {
    if let Some(s) = ctx.get("schedules", id) {
        return Some(v2_segments(ctx, &s, from, to));
    }
    ctx.get("schedules_v3", id)
        .map(|s| merge_per_user(v3_segments(ctx, &s, from, to)))
}

/// Segments overlapping `[since, until]` (a point when equal), with their full extent
/// computed from a wider window around it.
pub fn oncall_segments(ctx: &Ctx, id: &str, since: i64, until: i64) -> Vec<Seg> {
    schedule_segments(ctx, id, since - CONTEXT, until + CONTEXT)
        .unwrap_or_default()
        .into_iter()
        .filter(|s| {
            if since == until {
                s.start <= since && since < s.end
            } else {
                s.start < until && s.end > since
            }
        })
        .collect()
}

/// Everyone who appears on a schedule (layers, members, overrides).
pub fn schedule_user_ids(ctx: &Ctx, id: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(s) = ctx.get("schedules", id) {
        for layer in s["schedule_layers"].as_array().into_iter().flatten() {
            out.extend(layer_users(layer));
        }
        out.extend(override_segments(ctx, id).into_iter().map(|s| s.user));
    } else if let Some(s) = ctx.get("schedules_v3", id) {
        out.extend(v3_user_ids(ctx, &s));
    }
    let mut seen = Vec::new();
    out.retain(|u| {
        if seen.contains(u) {
            false
        } else {
            seen.push(u.clone());
            true
        }
    });
    out
}

/// Users on call at escalation `level` (1-based) of a policy at `at`.
pub fn oncall_users_at_level(ctx: &Ctx, ep: &Value, level: usize, at: i64) -> Vec<String> {
    let Some(rule) = ep["escalation_rules"]
        .as_array()
        .and_then(|r| r.get(level - 1))
    else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for t in rule["targets"].as_array().into_iter().flatten() {
        let Some(tid) = ref_id(t) else { continue };
        let users = match target_kind(t) {
            Kind::User => vec![tid.to_owned()],
            _ => oncall_segments(ctx, tid, at, at)
                .into_iter()
                .map(|s| s.user)
                .collect(),
        };
        for u in users {
            if !out.contains(&u) {
                out.push(u);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// Schedules (v2)
// ---------------------------------------------------------------------------------------

fn redirect_to_v3() -> ApiErr {
    ApiErr::bad_request(
        "This is a shift-based schedule. Use the v3 Schedules API (/v3/schedules) to access it.",
    )
}

fn v2_schedule(ctx: &Ctx, id: &str) -> Result<Value, ApiErr> {
    if let Some(s) = ctx.get("schedules", id) {
        return Ok(s);
    }
    if ctx.exists("schedules_v3", id) {
        return Err(redirect_to_v3());
    }
    Err(ApiErr::NotFound)
}

fn escalation_policies_for(ctx: &Ctx, schedule: &str) -> Vec<Value> {
    ctx.docs("escalation_policies")
        .into_iter()
        .filter(|ep| {
            ep["escalation_rules"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|r| r["targets"].as_array().into_iter().flatten())
                .any(|t| t["id"] == schedule)
        })
        .filter_map(|ep| {
            ep["id"]
                .as_str()
                .map(|id| ctx.reference(Kind::EscalationPolicy, id))
        })
        .collect()
}

fn summary_fields(ctx: &Ctx, mut doc: Value) -> Value {
    let id = doc["id"].as_str().unwrap_or_default().to_owned();
    ctx.base_fields(&mut doc, Kind::Schedule);
    default(&mut doc, "time_zone", json!("Etc/UTC"));
    default(&mut doc, "description", Value::Null);
    let users: Vec<Value> = schedule_user_ids(ctx, &id)
        .iter()
        .map(|u| ctx.reference(Kind::User, u))
        .collect();
    doc["users"] = json!(users);
    doc["escalation_policies"] = json!(escalation_policies_for(ctx, &id));
    default(&mut doc, "teams", json!([]));
    ctx.fix_refs(&mut doc["teams"], Kind::Team);
    doc
}

fn entries(ctx: &Ctx, segs: &[Seg]) -> Vec<Value> {
    segs.iter()
        .map(|s| {
            json!({
                "start": fmt(s.start),
                "end": fmt(s.end),
                "user": ctx.reference(Kind::User, &s.user),
            })
        })
        .collect()
}

fn coverage(segs: &[Seg], from: i64, to: i64) -> f64 {
    if to <= from {
        return 0.0;
    }
    let covered: i64 = segs.iter().map(|s| s.end.min(to) - s.start.max(from)).sum();
    (covered as f64 * 10_000.0 / (to - from) as f64).round() / 100.0
}

fn render_schedule(ctx: &Ctx, doc: Value, from: i64, to: i64) -> Value {
    let id = doc["id"].as_str().unwrap_or_default().to_owned();
    let final_segs = v2_segments(ctx, &doc, from, to);
    let mut doc = summary_fields(ctx, doc);
    default(&mut doc, "schedule_layers", json!([]));
    if let Some(layers) = doc["schedule_layers"].as_array_mut() {
        for (i, layer) in layers.iter_mut().enumerate() {
            let rendered = layer_segments(layer, from, to);
            default(layer, "id", json!(derived_id(&format!("{id}/layer/{i}"))));
            default(layer, "name", json!(format!("Layer {}", i + 1)));
            default(layer, "end", Value::Null);
            let start = layer["start"].clone();
            default(layer, "rotation_virtual_start", start);
            default(layer, "rotation_turn_length_seconds", json!(7 * DAY));
            default(layer, "restrictions", json!([]));
            let users: Vec<Value> = layer_users(layer)
                .iter()
                .map(|u| json!({ "user": ctx.reference(Kind::User, u) }))
                .collect();
            layer["users"] = json!(users);
            layer["rendered_schedule_entries"] = json!(entries(ctx, &rendered));
            layer["rendered_coverage_percentage"] = json!(coverage(&rendered, from, to));
        }
    }
    let overrides: Vec<Seg> = override_segments(ctx, &id)
        .into_iter()
        .filter(|s| s.start < to && s.end > from)
        .collect();
    doc["overrides_subschedule"] = json!({
        "name": "Overrides",
        "rendered_schedule_entries": entries(ctx, &overrides),
        "rendered_coverage_percentage": coverage(&overrides, from, to),
    });
    doc["final_schedule"] = json!({
        "name": "Final Schedule",
        "rendered_schedule_entries": entries(ctx, &final_segs),
        "rendered_coverage_percentage": coverage(&final_segs, from, to),
    });
    doc
}

fn window(ctx: &Ctx, q: &crate::util::Query, default_days: i64) -> Result<(i64, i64), ApiErr> {
    let since = q.time("since")?.unwrap_or(ctx.now);
    let until = q.time("until")?.unwrap_or(since + default_days * DAY);
    if until < since {
        return Err(ApiErr::invalid("Since must be before until"));
    }
    Ok((since, until))
}

pub async fn list_schedules(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let q = &req.query;
        let users = q.list("user_ids");
        let teams = q.list("team_ids");
        let mut schedules: Vec<Value> = ctx
            .docs("schedules")
            .into_iter()
            .filter(|s| {
                crate::util::matches_query(q.get("query"), &[&s["name"], &s["description"]])
            })
            .filter(|s| {
                teams.is_empty()
                    || s["teams"].as_array().is_some_and(|ts| {
                        ts.iter()
                            .any(|t| teams.iter().any(|x| t["id"] == x.as_str()))
                    })
            })
            .filter(|s| {
                users.is_empty() || {
                    let on = schedule_user_ids(ctx, s["id"].as_str().unwrap_or_default());
                    on.iter().any(|u| users.contains(u))
                }
            })
            .collect();
        sort_by_name(&mut schedules);
        let with_layers = q.list("include").iter().any(|i| i == "schedule_layers");
        let (from, to) = (ctx.now, ctx.now + 14 * DAY);
        let schedules = schedules
            .into_iter()
            .map(|s| {
                if with_layers {
                    render_schedule(ctx, s, from, to)
                } else {
                    let mut s = summary_fields(ctx, s);
                    if let Some(o) = s.as_object_mut() {
                        o.shift_remove("schedule_layers");
                    }
                    s
                }
            })
            .collect();
        classic(q, "schedules", schedules)
    })
}

pub async fn get_schedule(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let schedule = v2_schedule(ctx, &id)?;
        let (from, to) = window(ctx, &req.query, 14)?;
        ok(json!({ "schedule": render_schedule(ctx, schedule, from, to) }))
    })
}

pub async fn list_schedule_users(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let schedule = v2_schedule(ctx, &id)?;
        let q = &req.query;
        let ids: Vec<String> = if q.get("since").is_some() || q.get("until").is_some() {
            let (from, to) = window(ctx, q, 14)?;
            let mut ids = Vec::new();
            for s in v2_segments(ctx, &schedule, from, to) {
                if !ids.contains(&s.user) {
                    ids.push(s.user);
                }
            }
            ids
        } else {
            schedule_user_ids(ctx, &id)
        };
        let users: Vec<Value> = ids
            .iter()
            .filter_map(|u| ctx.lookup(Kind::User, u))
            .map(|u| render_user(ctx, u))
            .collect();
        ok(json!({ "users": users }))
    })
}

fn render_override(ctx: &Ctx, mut o: Value) -> Value {
    ctx.fix_ref(&mut o["user"], Kind::User);
    default(&mut o, "time_zone", Value::Null);
    o
}

fn create_one_override(ctx: &mut Ctx, schedule: &str, input: &Value) -> Result<Value, Vec<String>> {
    let mut errors = Vec::new();
    let start = input["start"].as_str().and_then(parse_time);
    let end = input["end"].as_str().and_then(parse_time);
    if start.is_none() {
        errors.push("Start is not a valid date/time".to_owned());
    }
    if end.is_none() {
        errors.push("End is not a valid date/time".to_owned());
    }
    if let (Some(s), Some(e)) = (start, end)
        && e <= s
    {
        errors.push("End must be after start".to_owned());
    }
    let user = ref_id(&input["user"]).or_else(|| input["user_id"].as_str());
    match user {
        Some(u) if ctx.exists("users", u) => {}
        Some(_) => errors.push("User not found".to_owned()),
        None => errors.push("User can't be blank".to_owned()),
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let id = ctx.new_id("overrides");
    let doc = json!({
        "id": id,
        "start": fmt(start.unwrap_or_default()),
        "end": fmt(end.unwrap_or_default()),
        "user": stored_ref(Kind::User, user.unwrap_or_default()),
        "time_zone": input.get("time_zone").cloned().unwrap_or(Value::Null),
    });
    let key = ctx.next_child_key("overrides", schedule);
    ctx.put("overrides", &key, doc.clone());
    Ok(render_override(ctx, doc))
}

pub async fn create_overrides(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        v2_schedule(ctx, &id)?;
        let body = req.json()?;
        if let Some(single) = body.get("override").filter(|v| v.is_object()) {
            return match create_one_override(ctx, &id, single) {
                Ok(o) => created(json!({ "override": o })),
                Err(errors) => Err(ApiErr::Invalid(errors)),
            };
        }
        let Some(items) = body["overrides"].as_array().filter(|a| !a.is_empty()) else {
            return Err(ApiErr::invalid("Overrides can't be blank"));
        };
        let results: Vec<Value> = items
            .iter()
            .map(|item| match create_one_override(ctx, &id, item) {
                Ok(o) => json!({ "status": 201, "errors": [], "override": o }),
                Err(errors) => json!({ "status": 400, "errors": errors }),
            })
            .collect();
        created(Value::Array(results))
    })
}

pub async fn list_overrides(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        v2_schedule(ctx, &id)?;
        let q = &req.query;
        let (Some(since), Some(until)) = (q.time("since")?, q.time("until")?) else {
            return Err(ApiErr::invalid("Since and until are required"));
        };
        let overrides: Vec<Value> = ctx
            .children("overrides", &id)
            .into_iter()
            .map(|(_, o)| o)
            .filter(|o| {
                time_of(o, "start").is_some_and(|s| s < until)
                    && time_of(o, "end").is_some_and(|e| e > since)
            })
            .map(|o| render_override(ctx, o))
            .collect();
        classic(q, "overrides", overrides)
    })
}

pub async fn delete_override(
    State(world): State<SharedWorld>,
    Path((id, override_id)): Path<(String, String)>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        v2_schedule(ctx, &id)?;
        let (key, _) = ctx
            .find_child("overrides", &id, &override_id)
            .ok_or(ApiErr::NotFound)?;
        ctx.remove("overrides", &key);
        no_content()
    })
}

// ---------------------------------------------------------------------------------------
// On-calls
// ---------------------------------------------------------------------------------------

fn oncall_entry(
    ctx: &Ctx,
    ep: &str,
    level: usize,
    schedule: Option<(Kind, &str)>,
    seg: Option<&Seg>,
    user: &str,
) -> Value {
    json!({
        "escalation_policy": ctx.reference(Kind::EscalationPolicy, ep),
        "escalation_level": level,
        "schedule": schedule.map(|(k, id)| ctx.reference(k, id)),
        "user": ctx.reference(Kind::User, user),
        "start": seg.map(|s| fmt(s.start)),
        "end": seg.map(|s| fmt(s.end)),
    })
}

pub fn oncalls(ctx: &Ctx, since: i64, until: i64) -> Vec<Value> {
    let mut out = Vec::new();
    for ep in ctx.docs("escalation_policies") {
        let ep_id = ep["id"].as_str().unwrap_or_default();
        for (i, rule) in ep["escalation_rules"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            for t in rule["targets"].as_array().into_iter().flatten() {
                let Some(tid) = ref_id(t) else { continue };
                match target_kind(t) {
                    Kind::User => out.push(oncall_entry(ctx, ep_id, i + 1, None, None, tid)),
                    kind => {
                        let kind = if ctx.exists("schedules_v3", tid) {
                            Kind::ScheduleV3
                        } else {
                            kind
                        };
                        for seg in oncall_segments(ctx, tid, since, until) {
                            out.push(oncall_entry(
                                ctx,
                                ep_id,
                                i + 1,
                                Some((kind, tid)),
                                Some(&seg),
                                &seg.user,
                            ));
                        }
                    }
                }
            }
        }
    }
    out
}

pub async fn list_oncalls(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let q = &req.query;
        let since = q.time("since")?.unwrap_or(ctx.now);
        let until = q.time("until")?.unwrap_or(since.max(ctx.now));
        if until < since {
            return Err(ApiErr::invalid("Since must be before until"));
        }
        if until - since > 90 * DAY {
            return Err(ApiErr::invalid(
                "The maximum range for since and until is 90 days",
            ));
        }
        let users = q.list("user_ids");
        let eps = q.list("escalation_policy_ids");
        let schedules = q.list("schedule_ids");
        let mut entries: Vec<Value> = oncalls(ctx, since, until)
            .into_iter()
            .filter(|e| users.is_empty() || users.iter().any(|u| e["user"]["id"] == u.as_str()))
            .filter(|e| {
                eps.is_empty()
                    || eps
                        .iter()
                        .any(|x| e["escalation_policy"]["id"] == x.as_str())
            })
            .filter(|e| {
                schedules.is_empty() || schedules.iter().any(|x| e["schedule"]["id"] == x.as_str())
            })
            .collect();
        if q.flag("earliest") {
            let mut seen: Vec<(Value, Value, Value)> = Vec::new();
            entries.retain(|e| {
                let key = (
                    e["user"]["id"].clone(),
                    e["escalation_policy"]["id"].clone(),
                    e["escalation_level"].clone(),
                );
                if seen.contains(&key) {
                    false
                } else {
                    seen.push(key);
                    true
                }
            });
        }
        classic(q, "oncalls", entries)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_rotates_users_by_turn() {
        let layer = json!({
            "start": "2026-01-05T09:00:00Z",
            "rotation_virtual_start": "2026-01-05T09:00:00Z",
            "rotation_turn_length_seconds": 604800,
            "users": [{"user": {"id": "A"}}, {"user": {"id": "B"}}]
        });
        let t0 = parse_time("2026-01-05T09:00:00Z").unwrap();
        let segs = layer_segments(&layer, t0, t0 + 3 * 604_800);
        let users: Vec<&str> = segs.iter().map(|s| s.user.as_str()).collect();
        assert_eq!(users, ["A", "B", "A"]);
        assert_eq!(segs[1].start, t0 + 604_800);
        // Before the layer starts there is nobody.
        assert!(layer_segments(&layer, t0 - 100, t0).is_empty());
    }

    #[test]
    fn merge_joins_touching_segments() {
        let s = |u: &str, a, b| Seg {
            user: u.into(),
            start: a,
            end: b,
        };
        assert_eq!(
            merge_per_user(vec![s("A", 0, 5), s("B", 0, 3), s("A", 5, 9)]),
            vec![s("A", 0, 9), s("B", 0, 3)]
        );
    }
}
