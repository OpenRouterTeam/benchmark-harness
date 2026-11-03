//! v1 monitors: `GET /api/v1/monitor` and `GET /api/v1/monitor/{id}`.

use axum::{
    extract::{Path, RawQuery, State},
    http::StatusCode,
    response::Response,
};
use mock_core::{http::json, world::SharedWorld, world::WorldState};
use serde_json::{Value, json as json_value};

use crate::{
    downtimes::is_live,
    search::query::glob,
    support::{OK, Params, bool_param, doc, docs, errors, int_param, parse_query},
};

const MAX_PAGE_SIZE: i64 = 1000;
const DEFAULT_PAGE_SIZE: i64 = 100;

/// Tags in the monitor query's scope: `avg(last_5m):avg:x{env:prod,service:a} by {host} > 1`
/// gives `["env:prod", "service:a"]`.
pub fn query_scope(query: &str) -> Vec<String> {
    let Some(start) = query.find('{') else {
        return Vec::new();
    };
    let Some(len) = query[start + 1..].find('}') else {
        return Vec::new();
    };
    query[start + 1..start + 1 + len]
        .replace(" AND ", ",")
        .replace(" and ", ",")
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty() && s != "*")
        .collect()
}

fn split_csv(value: Option<&String>) -> Vec<String> {
    value
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn has_all(have: &[String], wanted: &[String]) -> bool {
    wanted
        .iter()
        .all(|w| have.iter().any(|h| glob(w, h) || h.eq_ignore_ascii_case(w)))
}

fn string_list(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t.as_str().map(str::to_owned))
        .collect()
}

/// Shapes a stored monitor for output: `state.groups` only for the requested group states,
/// `matching_downtimes` when asked for.
fn render(
    world: &WorldState,
    monitor: &Value,
    group_states: &[String],
    with_downtimes: bool,
) -> Value {
    let mut out = monitor.clone();
    let Some(obj) = out.as_object_mut() else {
        return out;
    };
    if group_states.is_empty() {
        obj.shift_remove("state");
    } else {
        let all = group_states.iter().any(|s| s == "all");
        let groups = monitor["state"]["groups"]
            .as_object()
            .map(|groups| {
                groups
                    .iter()
                    .filter(|(_, g)| {
                        all || g["status"]
                            .as_str()
                            .is_some_and(|s| group_states.iter().any(|w| w.eq_ignore_ascii_case(s)))
                    })
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<serde_json::Map<_, _>>()
            })
            .unwrap_or_default();
        obj.insert("state".to_owned(), json_value!({ "groups": groups }));
    }
    if with_downtimes {
        let now = world.clock.now_unix();
        let id = monitor["id"].as_i64();
        let matching: Vec<Value> = docs(world, "downtimes")
            .into_iter()
            .filter(|d| d["monitor_id"].as_i64() == id && is_live(d, now))
            .map(|d| {
                json_value!({
                    "end": d["end"], "id": d["id"], "scope": d["scope"], "start": d["start"],
                })
            })
            .collect();
        obj.insert("matching_downtimes".to_owned(), Value::Array(matching));
    }
    out
}

fn group_states(params: &Params) -> Result<Vec<String>, Response> {
    let states = split_csv(params.get("group_states"));
    for s in &states {
        if !["all", "alert", "warn", "no data", "ok"].contains(&s.to_lowercase().as_str()) {
            return Err(errors::bad_request(&format!(
                "Invalid group_states value '{s}'; expected all, alert, warn, no data"
            )));
        }
    }
    Ok(states)
}

pub async fn list(State(world): State<SharedWorld>, RawQuery(raw): RawQuery) -> Response {
    let params = parse_query(raw.as_deref());
    let states = match group_states(&params) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let (id_offset, page, page_size) = match (
        int_param(&params, "id_offset"),
        int_param(&params, "page"),
        int_param(&params, "page_size"),
    ) {
        (Ok(a), Ok(b), Ok(c)) => (a, b, c),
        (Err(r), _, _) | (_, Err(r), _) | (_, _, Err(r)) => return r,
    };
    if page.is_some_and(|p| p < 0) {
        return errors::bad_request("page must be greater than or equal to 0");
    }
    if page_size.is_some_and(|s| !(1..=MAX_PAGE_SIZE).contains(&s)) {
        return errors::bad_request("page_size must be between 1 and 1000");
    }
    let name = params.get("name").map(|n| n.to_lowercase());
    let scope_tags = split_csv(params.get("tags"));
    let monitor_tags = split_csv(params.get("monitor_tags"));
    let with_downtimes = bool_param(&params, "with_downtimes").unwrap_or(false);

    let world = world.lock().expect("world lock");
    let mut monitors: Vec<Value> = docs(&world, "monitors")
        .into_iter()
        .filter(|m| m["deleted"].is_null())
        .filter(|m| {
            name.as_ref().is_none_or(|n| {
                m["name"]
                    .as_str()
                    .is_some_and(|mn| mn.to_lowercase().contains(n))
            })
        })
        .filter(|m| {
            has_all(
                &query_scope(m["query"].as_str().unwrap_or_default()),
                &scope_tags,
            )
        })
        .filter(|m| has_all(&string_list(&m["tags"]), &monitor_tags))
        .filter(|m| id_offset.is_none_or(|o| m["id"].as_i64().is_some_and(|id| id > o)))
        .collect();
    monitors.sort_by_key(|m| m["id"].as_i64().unwrap_or_default());
    if page.is_some() || page_size.is_some() {
        let size = page_size.unwrap_or(DEFAULT_PAGE_SIZE) as usize;
        let page = page.unwrap_or(0) as usize;
        monitors = monitors.into_iter().skip(page * size).take(size).collect();
    }
    let rendered: Vec<Value> = monitors
        .iter()
        .map(|m| render(&world, m, &states, with_downtimes))
        .collect();
    json(OK, Value::Array(rendered))
}

pub async fn get(
    State(world): State<SharedWorld>,
    Path(monitor_id): Path<String>,
    RawQuery(raw): RawQuery,
) -> Response {
    let params = parse_query(raw.as_deref());
    let states = match group_states(&params) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if monitor_id.parse::<i64>().is_err() {
        return errors::error(StatusCode::BAD_REQUEST, "Invalid monitor_id");
    }
    let world = world.lock().expect("world lock");
    match doc(&world, "monitors", &monitor_id).filter(|m| m["deleted"].is_null()) {
        Some(m) => json(
            OK,
            render(
                &world,
                &m,
                &states,
                bool_param(&params, "with_downtimes").unwrap_or(false),
            ),
        ),
        None => errors::error(StatusCode::NOT_FOUND, "Monitor not found"),
    }
}
