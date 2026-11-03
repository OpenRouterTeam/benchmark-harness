//! Analytics: aggregated incident and responder metrics and raw incident data, all derived
//! from the incidents, their timelines and the on-call schedules. Hours of day are bucketed
//! in UTC: business hours 08-18 Mon-Fri, sleep hours 22-08, off hours the rest.

use axum::extract::{Path, State};
use axum::response::Response;
use mock_core::world::SharedWorld;
use serde_json::{Map, Value, json};

use crate::directory::target_kind;
use crate::schedules::{Seg, merge_per_user, schedule_segments};
use crate::util::{ApiErr, Ctx, Kind, Req, fmt, ok, parse_time, ref_id, run, time_of};

const DAY: i64 = 86_400;

/// Per-incident facts the metrics are computed from.
struct Facts {
    id: String,
    number: Value,
    title: Value,
    service: String,
    teams: Vec<String>,
    urgency: String,
    priority: Option<String>,
    escalation_policy: Option<String>,
    created: i64,
    resolved: Option<i64>,
    first_ack: Option<i64>,
    escalations: usize,
    manual_escalations: usize,
    assignments: usize,
    responders: Vec<String>,
    acknowledgers: Vec<String>,
}

impl Facts {
    fn to_ack(&self) -> Option<i64> {
        self.first_ack.map(|a| a - self.created)
    }

    fn to_resolve(&self) -> Option<i64> {
        self.resolved.map(|r| r - self.created)
    }

    fn engaged(&self) -> Option<i64> {
        match (self.first_ack, self.resolved) {
            (Some(a), Some(r)) => Some(r - a),
            _ => None,
        }
    }

    /// 0 = business, 1 = off hours, 2 = sleep hours.
    fn hour_bucket(&self) -> usize {
        let hour = self.created.rem_euclid(DAY) / 3600;
        let weekday = (self.created.div_euclid(DAY) + 4).rem_euclid(7);
        if !(8..22).contains(&hour) {
            2
        } else if (1..=5).contains(&weekday) && hour < 18 {
            0
        } else {
            1
        }
    }
}

fn push_unique(list: &mut Vec<String>, v: &str) {
    if !list.iter().any(|x| x == v) {
        list.push(v.to_owned());
    }
}

fn facts(ctx: &Ctx) -> Vec<Facts> {
    let logs = ctx.docs("log_entries");
    ctx.docs("incidents")
        .into_iter()
        .filter_map(|inc| {
            let id = inc["id"].as_str()?.to_owned();
            let created = time_of(&inc, "created_at")?;
            let service = ref_id(&inc["service"]).unwrap_or_default().to_owned();
            let service_doc = ctx.get("services", &service).unwrap_or_default();
            let teams_src = if inc["teams"].is_array() {
                &inc["teams"]
            } else {
                &service_doc["teams"]
            };
            let teams = teams_src
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| ref_id(t).map(str::to_owned))
                .collect();
            let mine: Vec<&Value> = logs
                .iter()
                .filter(|l| l["incident"]["id"] == id.as_str())
                .collect();
            let mut acks: Vec<(i64, String)> = inc["acknowledgements"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| Some((time_of(a, "at")?, ref_id(&a["acknowledger"])?.to_owned())))
                .collect();
            acks.extend(
                mine.iter()
                    .filter(|l| l["type"] == "acknowledge_log_entry")
                    .filter_map(|l| {
                        Some((time_of(l, "created_at")?, ref_id(&l["agent"])?.to_owned()))
                    }),
            );
            acks.sort();
            let mut responders = Vec::new();
            for a in inc["assignments"].as_array().into_iter().flatten() {
                if let Some(u) = ref_id(&a["assignee"]) {
                    push_unique(&mut responders, u);
                }
            }
            for l in &mine {
                for a in l["assignees"].as_array().into_iter().flatten() {
                    if let Some(u) = ref_id(a) {
                        push_unique(&mut responders, u);
                    }
                }
            }
            let mut acknowledgers = Vec::new();
            for (_, u) in &acks {
                push_unique(&mut acknowledgers, u);
                push_unique(&mut responders, u);
            }
            let escalations: Vec<&&Value> = mine
                .iter()
                .filter(|l| l["type"] == "escalate_log_entry")
                .collect();
            Some(Facts {
                number: inc["incident_number"].clone(),
                title: inc["title"].clone(),
                urgency: inc["urgency"].as_str().unwrap_or("high").to_owned(),
                priority: ref_id(&inc["priority"]).map(str::to_owned),
                escalation_policy: ref_id(&inc["escalation_policy"])
                    .or_else(|| ref_id(&service_doc["escalation_policy"]))
                    .map(str::to_owned),
                resolved: time_of(&inc, "resolved_at"),
                first_ack: acks.first().map(|a| a.0),
                manual_escalations: escalations
                    .iter()
                    .filter(|l| {
                        l["agent"]["type"]
                            .as_str()
                            .is_some_and(|t| t.starts_with("user"))
                    })
                    .count(),
                escalations: escalations.len(),
                assignments: mine
                    .iter()
                    .filter(|l| l["type"] == "assign_log_entry")
                    .count()
                    + usize::from(!responders.is_empty()),
                id,
                service,
                teams,
                created,
                responders,
                acknowledgers,
            })
        })
        .collect()
}

struct Filters {
    start: i64,
    end: i64,
    service_ids: Vec<String>,
    team_ids: Vec<String>,
    responder_ids: Vec<String>,
    urgency: Option<String>,
    raw: Value,
}

fn filters(body: &Value, start_key: &str, end_key: &str) -> Result<Filters, ApiErr> {
    let f = &body["filters"];
    let get = |k: &str| f[k].as_str().and_then(parse_time);
    let (Some(start), Some(end)) = (get(start_key), get(end_key)) else {
        return Err(ApiErr::invalid(format!(
            "filters.{start_key} and filters.{end_key} are required ISO8601 date/times"
        )));
    };
    if end <= start {
        return Err(ApiErr::invalid(format!(
            "filters.{end_key} must be after filters.{start_key}"
        )));
    }
    if end - start > 366 * DAY {
        return Err(ApiErr::invalid("The date range may not exceed one year"));
    }
    let list = |k: &str| -> Vec<String> {
        f[k].as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect()
    };
    let urgency = f["urgency"].as_str().map(str::to_owned);
    if urgency
        .as_deref()
        .is_some_and(|u| u != "high" && u != "low")
    {
        return Err(ApiErr::invalid("filters.urgency must be high or low"));
    }
    Ok(Filters {
        start,
        end,
        service_ids: list("service_ids"),
        team_ids: list("team_ids"),
        responder_ids: list("responder_ids"),
        urgency,
        raw: f.clone(),
    })
}

fn selected<'a>(all: &'a [Facts], f: &Filters) -> Vec<&'a Facts> {
    all.iter()
        .filter(|i| i.created >= f.start && i.created < f.end)
        .filter(|i| f.service_ids.is_empty() || f.service_ids.contains(&i.service))
        .filter(|i| f.team_ids.is_empty() || i.teams.iter().any(|t| f.team_ids.contains(t)))
        .filter(|i| f.urgency.as_ref().is_none_or(|u| *u == i.urgency))
        .filter(|i| {
            f.responder_ids.is_empty() || i.responders.iter().any(|r| f.responder_ids.contains(r))
        })
        .collect()
}

fn mean(values: &[i64]) -> Value {
    if values.is_empty() {
        return Value::Null;
    }
    json!((values.iter().sum::<i64>() as f64 / values.len() as f64).round() as i64)
}

fn percentile(values: &[i64], p: f64) -> Value {
    if values.is_empty() {
        return Value::Null;
    }
    let mut v = values.to_vec();
    v.sort_unstable();
    let rank = ((p / 100.0) * v.len() as f64).ceil().max(1.0) as usize;
    json!(v[rank.min(v.len()) - 1])
}

fn uptime(incidents: &[&Facts], start: i64, end: i64, now: i64) -> f64 {
    let mut down: Vec<(i64, i64)> = incidents
        .iter()
        .filter(|i| i.urgency == "high")
        .map(|i| (i.created.max(start), i.resolved.unwrap_or(now).min(end)))
        .filter(|(a, b)| b > a)
        .collect();
    down.sort_unstable();
    let mut total = 0;
    let mut cursor = start;
    for (a, b) in down {
        let a = a.max(cursor);
        if b > a {
            total += b - a;
            cursor = b;
        }
    }
    let pct = 100.0 - total as f64 * 100.0 / (end - start) as f64;
    (pct * 1000.0).round() / 1000.0
}

fn incident_metrics(incidents: &[&Facts], start: i64, end: i64, now: i64) -> Map<String, Value> {
    let acks: Vec<i64> = incidents.iter().filter_map(|i| i.to_ack()).collect();
    let resolves: Vec<i64> = incidents.iter().filter_map(|i| i.to_resolve()).collect();
    let engaged: Vec<i64> = incidents.iter().filter_map(|i| i.engaged()).collect();
    let buckets = |b: usize| incidents.iter().filter(|i| i.hour_bucket() == b).count();
    let mut m = Map::new();
    m.insert("total_incident_count".into(), json!(incidents.len()));
    m.insert("mean_seconds_to_first_ack".into(), mean(&acks));
    m.insert("mean_seconds_to_resolve".into(), mean(&resolves));
    m.insert("mean_engaged_seconds".into(), mean(&engaged));
    m.insert(
        "total_engaged_seconds".into(),
        json!(engaged.iter().sum::<i64>()),
    );
    m.insert(
        "total_escalation_count".into(),
        json!(incidents.iter().map(|i| i.escalations).sum::<usize>()),
    );
    m.insert(
        "total_incidents_manual_escalated".into(),
        json!(
            incidents
                .iter()
                .filter(|i| i.manual_escalations > 0)
                .count()
        ),
    );
    m.insert(
        "total_incidents_acknowledged".into(),
        json!(incidents.iter().filter(|i| i.first_ack.is_some()).count()),
    );
    m.insert(
        "total_interruptions".into(),
        json!(incidents.iter().map(|i| i.responders.len()).sum::<usize>()),
    );
    m.insert(
        "total_business_hour_interruptions".into(),
        json!(buckets(0)),
    );
    m.insert("total_off_hour_interruptions".into(), json!(buckets(1)));
    m.insert("total_sleep_hour_interruptions".into(), json!(buckets(2)));
    m.insert(
        "total_major_incidents".into(),
        json!(
            incidents
                .iter()
                .filter(|i| i.priority.is_some() && i.urgency == "high")
                .count()
        ),
    );
    m.insert(
        "up_time_pct".into(),
        json!(uptime(incidents, start, end, now)),
    );
    m
}

fn name_of(ctx: &Ctx, kind: Kind, id: &str) -> Value {
    ctx.lookup(kind, id)
        .map(|d| d["name"].clone())
        .unwrap_or(Value::Null)
}

fn envelope(body: &Value, f: &Filters, data: Vec<Value>) -> Value {
    let mut data = data;
    if let Some(field) = body["order_by"].as_str() {
        let desc = body["order"] == "desc";
        data.sort_by(|a, b| {
            let (x, y) = (&a[field], &b[field]);
            let o = match (x.as_f64(), y.as_f64()) {
                (Some(x), Some(y)) => x.total_cmp(&y),
                _ => x.as_str().cmp(&y.as_str()),
            };
            if desc { o.reverse() } else { o }
        });
    }
    json!({
        "filters": f.raw,
        "aggregate_unit": body.get("aggregate_unit").cloned().unwrap_or(Value::Null),
        "time_zone": body["time_zone"].as_str().unwrap_or("Etc/UTC"),
        "order": body["order"].as_str().unwrap_or("desc"),
        "order_by": body.get("order_by").cloned().unwrap_or(Value::Null),
        "data": data,
    })
}

fn bucket_start(ts: i64, unit: &str) -> i64 {
    let day = ts.div_euclid(DAY) * DAY;
    match unit {
        "week" => day - ((ts.div_euclid(DAY) + 3).rem_euclid(7)) * DAY,
        "month" => {
            let (y, m, _) = civil(ts.div_euclid(DAY));
            days_from_civil(y, m, 1) * DAY
        }
        _ => day,
    }
}

/// Days since epoch to (year, month, day), proleptic Gregorian.
fn civil(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn run_metrics(
    world: &SharedWorld,
    req: &Req,
    start_key: &'static str,
    end_key: &'static str,
    build: impl FnOnce(&Ctx, &Value, &Filters, Vec<&Facts>) -> Vec<Value>,
) -> Response {
    run(world, req, |ctx| {
        let body = req.json()?;
        let f = filters(&body, start_key, end_key)?;
        let all = facts(ctx);
        let incidents = selected(&all, &f);
        let data = build(ctx, &body, &f, incidents);
        ok(envelope(&body, &f, data))
    })
}

pub async fn incidents_all(State(world): State<SharedWorld>, req: Req) -> Response {
    run_metrics(
        &world,
        &req,
        "created_at_start",
        "created_at_end",
        |ctx, _, f, incidents| {
            let mut m = incident_metrics(&incidents, f.start, f.end, ctx.now);
            let acks: Vec<i64> = incidents.iter().filter_map(|i| i.to_ack()).collect();
            let resolves: Vec<i64> = incidents.iter().filter_map(|i| i.to_resolve()).collect();
            for p in [50.0, 75.0, 90.0, 95.0] {
                m.insert(format!("p{p}_seconds_to_first_ack"), percentile(&acks, p));
                m.insert(format!("p{p}_seconds_to_resolve"), percentile(&resolves, p));
            }
            vec![Value::Object(m)]
        },
    )
}

pub async fn incidents_by_service(State(world): State<SharedWorld>, req: Req) -> Response {
    run_metrics(
        &world,
        &req,
        "created_at_start",
        "created_at_end",
        |ctx, _, f, incidents| {
            let mut services: Vec<String> = Vec::new();
            for i in &incidents {
                push_unique(&mut services, &i.service);
            }
            services
                .iter()
                .map(|s| {
                    let group: Vec<&Facts> = incidents
                        .iter()
                        .copied()
                        .filter(|i| &i.service == s)
                        .collect();
                    let team = ctx
                        .get("services", s)
                        .and_then(|d| ref_id(&d["teams"][0]).map(str::to_owned));
                    let mut m = Map::new();
                    m.insert("service_id".into(), json!(s));
                    m.insert("service_name".into(), name_of(ctx, Kind::Service, s));
                    m.insert("team_id".into(), json!(team));
                    m.insert(
                        "team_name".into(),
                        team.as_deref()
                            .map_or(Value::Null, |t| name_of(ctx, Kind::Team, t)),
                    );
                    m.append(&mut incident_metrics(&group, f.start, f.end, ctx.now));
                    Value::Object(m)
                })
                .collect()
        },
    )
}

pub async fn incidents_by_team(State(world): State<SharedWorld>, req: Req) -> Response {
    run_metrics(
        &world,
        &req,
        "created_at_start",
        "created_at_end",
        |ctx, body, f, incidents| {
            let unit = body["aggregate_unit"].as_str();
            let mut keys: Vec<(i64, String)> = Vec::new();
            for i in &incidents {
                let b = unit.map_or(0, |u| bucket_start(i.created, u));
                for t in &i.teams {
                    if !keys.contains(&(b, t.clone())) {
                        keys.push((b, t.clone()));
                    }
                }
            }
            keys.sort();
            keys.iter()
                .map(|(b, t)| {
                    let group: Vec<&Facts> = incidents
                        .iter()
                        .copied()
                        .filter(|i| {
                            i.teams.contains(t)
                                && unit.is_none_or(|u| bucket_start(i.created, u) == *b)
                        })
                        .collect();
                    let mut m = Map::new();
                    m.insert(
                        "range_start".into(),
                        if unit.is_some() {
                            json!(fmt(*b))
                        } else {
                            Value::Null
                        },
                    );
                    m.insert("team_id".into(), json!(t));
                    m.insert("team_name".into(), name_of(ctx, Kind::Team, t));
                    m.append(&mut incident_metrics(&group, f.start, f.end, ctx.now));
                    Value::Object(m)
                })
                .collect()
        },
    )
}

/// Seconds a user was on call within `[start, end)`: (any level, level 1, level 2+).
fn oncall_seconds(ctx: &Ctx, user: &str, start: i64, end: i64) -> (i64, i64, i64) {
    let mut first: Vec<Seg> = Vec::new();
    let mut later: Vec<Seg> = Vec::new();
    for ep in ctx.docs("escalation_policies") {
        for (i, rule) in ep["escalation_rules"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            for t in rule["targets"].as_array().into_iter().flatten() {
                let Some(tid) = ref_id(t) else { continue };
                let segs = match target_kind(t) {
                    Kind::User if tid == user => vec![Seg {
                        user: user.to_owned(),
                        start,
                        end,
                    }],
                    Kind::User => Vec::new(),
                    _ => schedule_segments(ctx, tid, start, end)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|s| s.user == user)
                        .collect(),
                };
                if i == 0 { &mut first } else { &mut later }.extend(segs);
            }
        }
    }
    let total = |segs: Vec<Seg>| -> i64 {
        merge_per_user(segs)
            .iter()
            .map(|s| s.end.min(end) - s.start.max(start))
            .sum()
    };
    let all: Vec<Seg> = first.iter().chain(later.iter()).cloned().collect();
    (total(all), total(first), total(later))
}

fn user_teams(ctx: &Ctx, user: &str) -> Vec<String> {
    ctx.get("users", user)
        .map(|u| {
            u["teams"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| ref_id(t).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn responder_metrics(
    ctx: &Ctx,
    f: &Filters,
    incidents: &[&Facts],
    user: &str,
    team: Option<&str>,
) -> Map<String, Value> {
    let mine: Vec<&Facts> = incidents
        .iter()
        .copied()
        .filter(|i| i.responders.iter().any(|r| r == user))
        .collect();
    let acked: Vec<&Facts> = mine
        .iter()
        .copied()
        .filter(|i| i.acknowledgers.iter().any(|a| a == user))
        .collect();
    let to_ack: Vec<i64> = acked.iter().filter_map(|i| i.to_ack()).collect();
    let engaged: Vec<i64> = acked.iter().filter_map(|i| i.engaged()).collect();
    let buckets = |b: usize| mine.iter().filter(|i| i.hour_bucket() == b).count();
    let (any, l1, l2) = oncall_seconds(ctx, user, f.start, f.end);
    let mut m = Map::new();
    m.insert("responder_id".into(), json!(user));
    m.insert("responder_name".into(), name_of(ctx, Kind::User, user));
    m.insert("team_id".into(), json!(team));
    m.insert(
        "team_name".into(),
        team.map_or(Value::Null, |t| name_of(ctx, Kind::Team, t)),
    );
    m.insert("total_seconds_on_call".into(), json!(any));
    m.insert("total_seconds_on_call_level_1".into(), json!(l1));
    m.insert("total_seconds_on_call_level_2_plus".into(), json!(l2));
    m.insert("total_incident_count".into(), json!(mine.len()));
    m.insert("total_interruptions".into(), json!(mine.len()));
    m.insert(
        "total_business_hour_interruptions".into(),
        json!(buckets(0)),
    );
    m.insert("total_off_hour_interruptions".into(), json!(buckets(1)));
    m.insert("total_sleep_hour_interruptions".into(), json!(buckets(2)));
    m.insert(
        "total_engaged_seconds".into(),
        json!(engaged.iter().sum::<i64>()),
    );
    m.insert("mean_engaged_seconds".into(), mean(&engaged));
    m.insert("mean_time_to_acknowledge_seconds".into(), mean(&to_ack));
    m.insert("total_notifications".into(), json!(mine.len()));
    m.insert("total_incidents_acknowledged".into(), json!(acked.len()));
    for k in [
        "total_incidents_manual_escalated_from",
        "total_incidents_manual_escalated_to",
        "total_incidents_reassigned_from",
        "total_incidents_reassigned_to",
        "total_incidents_timeout_escalated_from",
        "total_incidents_timeout_escalated_to",
    ] {
        m.insert(k.into(), json!(0));
    }
    m
}

fn responders(ctx: &Ctx, f: &Filters, incidents: &[&Facts]) -> Vec<String> {
    let mut users: Vec<String> = Vec::new();
    for i in incidents {
        for r in &i.responders {
            push_unique(&mut users, r);
        }
    }
    for (id, _) in ctx.entries("users") {
        if oncall_seconds(ctx, &id, f.start, f.end).0 != 0 {
            push_unique(&mut users, &id);
        }
    }
    users
        .into_iter()
        .filter(|u| f.responder_ids.is_empty() || f.responder_ids.contains(u))
        .filter(|u| {
            f.team_ids.is_empty() || user_teams(ctx, u).iter().any(|t| f.team_ids.contains(t))
        })
        .collect()
}

/// Responder filters select responders, not incidents: drop them before incident selection.
fn responder_view(f: &Filters) -> Filters {
    Filters {
        start: f.start,
        end: f.end,
        service_ids: f.service_ids.clone(),
        team_ids: Vec::new(),
        responder_ids: Vec::new(),
        urgency: f.urgency.clone(),
        raw: f.raw.clone(),
    }
}

pub async fn responders_all(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let body = req.json()?;
        let f = filters(&body, "date_range_start", "date_range_end")?;
        let all = facts(ctx);
        let incidents = selected(&all, &responder_view(&f));
        let data = responders(ctx, &f, &incidents)
            .iter()
            .map(|u| {
                let team = user_teams(ctx, u).into_iter().next();
                Value::Object(responder_metrics(ctx, &f, &incidents, u, team.as_deref()))
            })
            .collect();
        ok(envelope(&body, &f, data))
    })
}

pub async fn responders_by_team(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let body = req.json()?;
        let f = filters(&body, "date_range_start", "date_range_end")?;
        let all = facts(ctx);
        let incidents = selected(&all, &responder_view(&f));
        let mut data = Vec::new();
        for u in responders(ctx, &f, &incidents) {
            for t in user_teams(ctx, &u) {
                if f.team_ids.is_empty() || f.team_ids.contains(&t) {
                    data.push(Value::Object(responder_metrics(
                        ctx,
                        &f,
                        &incidents,
                        &u,
                        Some(&t),
                    )));
                }
            }
        }
        ok(envelope(&body, &f, data))
    })
}

fn raw_incident(ctx: &Ctx, i: &Facts) -> Value {
    let team = i.teams.first();
    let buckets = i.hour_bucket();
    json!({
        "id": i.id,
        "incident_number": i.number,
        "description": i.title,
        "service_id": i.service,
        "service_name": name_of(ctx, Kind::Service, &i.service),
        "team_id": team,
        "team_name": team.map_or(Value::Null, |t| name_of(ctx, Kind::Team, t)),
        "escalation_policy_id": i.escalation_policy,
        "escalation_policy_name": i.escalation_policy.as_deref().map_or(Value::Null, |e| name_of(ctx, Kind::EscalationPolicy, e)),
        "urgency": i.urgency,
        "priority_id": i.priority,
        "priority_name": i.priority.as_deref().map_or(Value::Null, |p| name_of(ctx, Kind::Priority, p)),
        "major": i.priority.is_some() && i.urgency == "high",
        "created_at": fmt(i.created),
        "resolved_at": i.resolved.map(fmt),
        "seconds_to_first_ack": i.to_ack(),
        "seconds_to_resolve": i.to_resolve(),
        "engaged_seconds": i.engaged(),
        "escalation_count": i.escalations,
        "assignment_count": i.assignments,
        "engaged_user_count": i.acknowledgers.len(),
        "business_hour_interruptions": usize::from(buckets == 0),
        "off_hour_interruptions": usize::from(buckets == 1),
        "sleep_hour_interruptions": usize::from(buckets == 2),
        "user_defined_effort_seconds": null,
    })
}

pub async fn raw_incidents(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let body = req.json()?;
        let f = filters(&body, "created_at_start", "created_at_end")?;
        let all = facts(ctx);
        let mut incidents = selected(&all, &f);
        incidents.sort_by_key(|i| (i.created, i.id.clone()));
        if body["order"].as_str().unwrap_or("desc") == "desc" {
            incidents.reverse();
        }
        let start = match body["starting_after"].as_str() {
            Some(cursor) => incidents
                .iter()
                .position(|i| i.id == cursor)
                .map(|p| p + 1)
                .ok_or_else(|| ApiErr::invalid("starting_after is not a valid cursor"))?,
            None => 0,
        };
        let limit = body["limit"].as_u64().unwrap_or(20).clamp(1, 1000) as usize;
        let page: Vec<&Facts> = incidents.iter().skip(start).take(limit).copied().collect();
        let more = start + page.len() < incidents.len();
        let data: Vec<Value> = page.iter().map(|i| raw_incident(ctx, i)).collect();
        ok(json!({
            "first": page.first().map(|i| i.id.clone()),
            "last": page.last().map(|i| i.id.clone()),
            "limit": limit,
            "more": more,
            "order": body["order"].as_str().unwrap_or("desc"),
            "order_by": body["order_by"].as_str().unwrap_or("created_at"),
            "starting_after": body.get("starting_after").cloned().unwrap_or(Value::Null),
            "ending_before": null,
            "time_zone": body["time_zone"].as_str().unwrap_or("Etc/UTC"),
            "filters": f.raw,
            "data": data,
        }))
    })
}

pub async fn raw_incident_by_id(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let all = facts(ctx);
        let i = all.iter().find(|i| i.id == id).ok_or(ApiErr::NotFound)?;
        ok(raw_incident(ctx, i))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_round_trip_and_buckets() {
        let ts = parse_time("2026-10-29T13:00:00Z").unwrap();
        assert_eq!(fmt(bucket_start(ts, "day")), "2026-10-29T00:00:00Z");
        assert_eq!(fmt(bucket_start(ts, "week")), "2026-10-26T00:00:00Z");
        assert_eq!(fmt(bucket_start(ts, "month")), "2026-10-01T00:00:00Z");
        assert_eq!(percentile(&[10, 20, 30, 40], 50.0), json!(20));
        assert_eq!(mean(&[1, 2]), json!(2));
    }
}
