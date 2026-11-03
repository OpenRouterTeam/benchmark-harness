//! v1 dashboards: `GET /api/v1/dashboard` (summaries) and `GET /api/v1/dashboard/{id}`.

use axum::{
    extract::{Path, RawQuery, State},
    http::StatusCode,
    response::Response,
};
use mock_core::{http::json, world::SharedWorld};
use serde_json::{Map, Value, json as json_value};

use crate::support::{OK, bool_param, doc, docs, errors, int_param, parse_query};

const SUMMARY_FIELDS: [&str; 9] = [
    "author_handle",
    "created_at",
    "description",
    "id",
    "is_read_only",
    "layout_type",
    "modified_at",
    "title",
    "url",
];

fn summary(dashboard: &Value) -> Value {
    let mut out = Map::new();
    for field in SUMMARY_FIELDS {
        out.insert(
            field.to_owned(),
            dashboard.get(field).cloned().unwrap_or(Value::Null),
        );
    }
    Value::Object(out)
}

fn is_deleted(d: &Value) -> bool {
    !d["deleted_at"].is_null()
}

pub async fn list(State(world): State<SharedWorld>, RawQuery(raw): RawQuery) -> Response {
    let params = parse_query(raw.as_deref());
    let (count, start) = match (int_param(&params, "count"), int_param(&params, "start")) {
        (Ok(c), Ok(s)) => (c, s.unwrap_or(0)),
        (Err(r), _) | (_, Err(r)) => return r,
    };
    if count.is_some_and(|c| c < 0) || start < 0 {
        return errors::bad_request("count and start must be non-negative");
    }
    let shared_only = bool_param(&params, "filter[shared]").unwrap_or(false);
    let deleted_only = bool_param(&params, "filter[deleted]").unwrap_or(false);
    let world = world.lock().expect("world lock");
    let dashboards: Vec<Value> = docs(&world, "dashboards")
        .into_iter()
        .filter(|d| is_deleted(d) == deleted_only)
        .filter(|d| !shared_only || d["is_shared"].as_bool().unwrap_or(false))
        .skip(start as usize)
        .take(count.map_or(usize::MAX, |c| c as usize))
        .map(|d| summary(&d))
        .collect();
    json(OK, json_value!({ "dashboards": dashboards }))
}

pub async fn get(State(world): State<SharedWorld>, Path(dashboard_id): Path<String>) -> Response {
    let world = world.lock().expect("world lock");
    match doc(&world, "dashboards", &dashboard_id).filter(|d| !is_deleted(d)) {
        Some(mut d) => {
            if let Some(obj) = d.as_object_mut() {
                obj.shift_remove("is_shared");
                obj.shift_remove("deleted_at");
            }
            json(OK, d)
        }
        None => errors::error(
            StatusCode::NOT_FOUND,
            &format!("Dashboard with ID {dashboard_id} not found"),
        ),
    }
}
