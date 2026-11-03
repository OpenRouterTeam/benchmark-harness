//! v2 incidents: `GET /api/v2/incidents` (offset pagination) and `GET /api/v2/incidents/{id}`.

use axum::{
    extract::{Path, RawQuery, State},
    response::Response,
};
use mock_core::{http::json, world::SharedWorld};
use serde_json::{Value, json as json_value};

use crate::support::{OK, doc, docs, errors, int_param, parse_query};

const DEFAULT_PAGE_SIZE: i64 = 10;
const MAX_PAGE_SIZE: i64 = 100;

/// `included` user objects for `include=users`, from the relationships of `incidents`.
fn included_users(world: &mock_core::world::WorldState, incidents: &[Value]) -> Vec<Value> {
    let mut ids: Vec<String> = Vec::new();
    for incident in incidents {
        for rel in [
            "commander_user",
            "created_by_user",
            "last_modified_by_user",
            "declared_by_user",
        ] {
            if let Some(id) = incident["relationships"][rel]["data"]["id"].as_str()
                && !ids.iter().any(|i| i == id)
            {
                ids.push(id.to_owned());
            }
        }
    }
    ids.iter()
        .filter_map(|id| doc(world, "users", id))
        .collect()
}

fn wants_users(include: Option<&String>) -> Result<bool, Response> {
    let Some(include) = include else {
        return Ok(false);
    };
    let mut users = false;
    for item in include.split(',').filter(|s| !s.is_empty()) {
        match item {
            "users" => users = true,
            "attachments" => {}
            other => {
                return Err(errors::bad_request(&format!(
                    "Invalid include value '{other}', expected one of users, attachments"
                )));
            }
        }
    }
    Ok(users)
}

pub async fn list(State(world): State<SharedWorld>, RawQuery(raw): RawQuery) -> Response {
    let params = parse_query(raw.as_deref());
    let (size, offset) = match (
        int_param(&params, "page[size]"),
        int_param(&params, "page[offset]"),
    ) {
        (Ok(s), Ok(o)) => (s.unwrap_or(DEFAULT_PAGE_SIZE), o.unwrap_or(0)),
        (Err(r), _) | (_, Err(r)) => return r,
    };
    if !(1..=MAX_PAGE_SIZE).contains(&size) {
        return errors::bad_request("page[size] must be between 1 and 100");
    }
    if offset < 0 {
        return errors::bad_request("page[offset] must be greater than or equal to 0");
    }
    let users = match wants_users(params.get("include")) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let world = world.lock().expect("world lock");
    let page: Vec<Value> = docs(&world, "incidents")
        .into_iter()
        .skip(offset as usize)
        .take(size as usize)
        .collect();
    let mut body = json_value!({ "data": page });
    if users {
        body["included"] = Value::Array(included_users(&world, &page));
    }
    body["meta"] = json_value!({
        "pagination": { "next_offset": offset + size, "offset": offset, "size": size }
    });
    json(OK, body)
}

pub async fn get(
    State(world): State<SharedWorld>,
    Path(incident_id): Path<String>,
    RawQuery(raw): RawQuery,
) -> Response {
    let params = parse_query(raw.as_deref());
    let users = match wants_users(params.get("include")) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let world = world.lock().expect("world lock");
    let Some(incident) = doc(&world, "incidents", &incident_id) else {
        return errors::not_found();
    };
    let mut body = json_value!({ "data": incident });
    if users {
        body["included"] = Value::Array(included_users(&world, std::slice::from_ref(&incident)));
    }
    json(OK, body)
}
