//! v1 hosts: search (`GET /api/v1/hosts`), totals, mute and unmute.

use axum::{
    body::Bytes,
    extract::{Path, RawQuery, State},
    response::Response,
};
use mock_core::{http::json, world::SharedWorld};
use serde_json::{Value, json as json_value};

use crate::{
    NAME,
    search::query::glob,
    support::{OK, bool_param, doc, docs, errors, int_param, parse_query},
};

const DEFAULT_COUNT: i64 = 100;
const MAX_COUNT: i64 = 1000;

/// Every `source:tag` value of the host plus its tags without source.
fn host_tags(host: &Value) -> Vec<String> {
    host["tags_by_source"]
        .as_object()
        .map(|by_source| {
            by_source
                .values()
                .flat_map(|tags| tags.as_array().cloned().unwrap_or_default())
                .filter_map(|t| t.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// `filter` terms: `key:value` terms match host tags (and `host:` the name); bare terms are
/// case-insensitive substrings of the host name or an alias. All terms must match.
fn matches_filter(host: &Value, filter: &str) -> bool {
    let name = host["name"].as_str().unwrap_or_default().to_lowercase();
    let aliases: Vec<String> = host["aliases"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| a.as_str().map(str::to_lowercase))
        .collect();
    let tags = host_tags(host);
    filter.split_whitespace().all(|term| {
        let term_lower = term.to_lowercase();
        if let Some(h) = term_lower.strip_prefix("host:") {
            return glob(h, &name) || aliases.iter().any(|a| glob(h, a));
        }
        if term.contains(':') {
            return tags.iter().any(|t| glob(term, t));
        }
        if term.contains('*') {
            return glob(&term_lower, &name) || aliases.iter().any(|a| glob(&term_lower, a));
        }
        name.contains(&term_lower) || aliases.iter().any(|a| a.contains(&term_lower))
    })
}

fn sort_key(host: &Value, field: &str) -> Value {
    match field {
        "cpu" | "iowait" | "load" => host["metrics"][field].clone(),
        "apps" => json_value!(host["apps"].as_array().map_or(0, Vec::len)),
        "status" | "up" => host["up"].clone(),
        other => host[other].clone(),
    }
}

fn compare(a: &Value, b: &Value) -> std::cmp::Ordering {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x
            .as_f64()
            .unwrap_or_default()
            .total_cmp(&y.as_f64().unwrap_or_default()),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::String(x), Value::String(y)) => x.cmp(y),
        (Value::Null, Value::Null) => std::cmp::Ordering::Equal,
        (Value::Null, _) => std::cmp::Ordering::Less,
        (_, Value::Null) => std::cmp::Ordering::Greater,
        _ => a.to_string().cmp(&b.to_string()),
    }
}

/// A host as returned by the API: an expired mute reads as unmuted.
fn render(host: &Value, now: i64, include_meta: bool) -> Value {
    let mut out = host.clone();
    let expired = host["mute_timeout"].as_i64().is_some_and(|t| t <= now);
    if expired {
        out["is_muted"] = json_value!(false);
        out["mute_timeout"] = Value::Null;
    }
    if !include_meta && let Some(obj) = out.as_object_mut() {
        obj.shift_remove("meta");
    }
    out
}

pub async fn list(State(world): State<SharedWorld>, RawQuery(raw): RawQuery) -> Response {
    let params = parse_query(raw.as_deref());
    let (start, count, from) = match (
        int_param(&params, "start"),
        int_param(&params, "count"),
        int_param(&params, "from"),
    ) {
        (Ok(s), Ok(c), Ok(f)) => (s.unwrap_or(0), c.unwrap_or(DEFAULT_COUNT), f),
        (Err(r), _, _) | (_, Err(r), _) | (_, _, Err(r)) => return r,
    };
    if start < 0 {
        return errors::bad_request("start must be greater than or equal to 0");
    }
    if !(0..=MAX_COUNT).contains(&count) {
        return errors::bad_request("count must be between 0 and 1000");
    }
    let sort_field = params
        .get("sort_field")
        .map_or("name", String::as_str)
        .to_owned();
    let descending = match params.get("sort_dir").map(String::as_str) {
        None | Some("") | Some("asc") => false,
        Some("desc") => true,
        Some(other) => {
            return errors::bad_request(&format!(
                "Invalid sort_dir '{other}', expected asc or desc"
            ));
        }
    };
    let include_meta = bool_param(&params, "include_hosts_metadata").unwrap_or(true);
    let filter = params.get("filter").cloned().unwrap_or_default();

    let world = world.lock().expect("world lock");
    let now = world.clock.now_unix();
    let mut hosts: Vec<Value> = docs(&world, "hosts")
        .into_iter()
        .filter(|h| matches_filter(h, &filter))
        .filter(|h| from.is_none_or(|f| h["last_reported_time"].as_i64().is_some_and(|t| t >= f)))
        .collect();
    hosts.sort_by(|a, b| {
        let ord = compare(&sort_key(a, &sort_field), &sort_key(b, &sort_field));
        if descending { ord.reverse() } else { ord }
    });
    let total_matching = hosts.len();
    let page: Vec<Value> = hosts
        .iter()
        .skip(start as usize)
        .take(count as usize)
        .map(|h| render(h, now, include_meta))
        .collect();
    json(
        OK,
        json_value!({
            "host_list": page,
            "total_matching": total_matching,
            "total_returned": page.len(),
        }),
    )
}

pub async fn totals(State(world): State<SharedWorld>, RawQuery(raw): RawQuery) -> Response {
    let params = parse_query(raw.as_deref());
    let from = match int_param(&params, "from") {
        Ok(f) => f,
        Err(r) => return r,
    };
    let world = world.lock().expect("world lock");
    let since = from.unwrap_or(world.clock.now_unix() - 2 * 3600);
    let active: Vec<Value> = docs(&world, "hosts")
        .into_iter()
        .filter(|h| h["last_reported_time"].as_i64().is_some_and(|t| t >= since))
        .collect();
    let up = active
        .iter()
        .filter(|h| h["up"] == json_value!(true))
        .count();
    json(
        OK,
        json_value!({ "total_active": active.len(), "total_up": up }),
    )
}

pub async fn mute(
    State(world): State<SharedWorld>,
    Path(host_name): Path<String>,
    body: Bytes,
) -> Response {
    let input: Value = if body.is_empty() {
        json_value!({})
    } else {
        match serde_json::from_slice::<Value>(&body) {
            Ok(v) if v.is_object() => v,
            _ => return errors::bad_request("Invalid JSON body"),
        }
    };
    let end = match input.get("end") {
        None | Some(Value::Null) => None,
        Some(v) => match v.as_i64() {
            Some(e) => Some(e),
            None => return errors::bad_request("Invalid value for 'end': expected an integer"),
        },
    };
    let message = input
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let override_existing = input["override"].as_bool().unwrap_or(false);

    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    let now = world.clock.now_unix();
    let muted_at = world.clock.now_rfc3339();
    let Some(host) = doc(world, "hosts", &host_name) else {
        return errors::error(
            axum::http::StatusCode::NOT_FOUND,
            &format!("Host {host_name} not found"),
        );
    };
    let currently_muted = render(&host, now, true)["is_muted"] == json_value!(true);
    if currently_muted && !override_existing {
        return errors::bad_request(&format!(
            "host:{host_name} is already muted. To mute it again, set override to true"
        ));
    }
    if end.is_some_and(|e| e <= now) {
        return errors::bad_request("Mute end time must be in the future");
    }
    let service = world.service_mut(NAME);
    if let Some(h) = service.store.collection_mut("hosts").get_mut(&host_name) {
        h["is_muted"] = json_value!(true);
        h["mute_timeout"] = json_value!(end);
    }
    service.store.collection_mut("host_mutes").insert(
        host_name.clone(),
        json_value!({
            "hostname": host_name,
            "message": message,
            "end": end,
            "muted_at": muted_at,
        }),
    );
    json(
        OK,
        json_value!({
            "action": "Muted",
            "end": end,
            "hostname": host_name,
            "message": message,
        }),
    )
}

pub async fn unmute(State(world): State<SharedWorld>, Path(host_name): Path<String>) -> Response {
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    let now = world.clock.now_unix();
    let Some(host) = doc(world, "hosts", &host_name) else {
        return errors::error(
            axum::http::StatusCode::NOT_FOUND,
            &format!("Host {host_name} not found"),
        );
    };
    if render(&host, now, true)["is_muted"] != json_value!(true) {
        return errors::bad_request(&format!("host:{host_name} is not muted"));
    }
    let service = world.service_mut(NAME);
    if let Some(h) = service.store.collection_mut("hosts").get_mut(&host_name) {
        h["is_muted"] = json_value!(false);
        h["mute_timeout"] = Value::Null;
    }
    service
        .store
        .collection_mut("host_mutes")
        .remove(&host_name);
    json(
        OK,
        json_value!({ "action": "Unmuted", "hostname": host_name }),
    )
}
