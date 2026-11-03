//! `/api/v1/clusters` and `/api/v1/projects`.

use axum::{
    extract::{Path, State},
    http::StatusCode,
};
use mock_core::{http::json, world::SharedWorld};
use serde_json::{Value, json};

use crate::{
    Params,
    error::{ApiResult, ArgoError},
    model::{self, str_field},
};

pub async fn list_clusters(State(world): State<SharedWorld>, params: Params) -> ApiResult {
    let world = world.lock().expect("world lock");
    let store = model::store(&world);
    let apps: Vec<&Value> = store
        .collection("applications")
        .map(|c| c.values().collect())
        .unwrap_or_default();
    let items: Vec<Value> = store
        .collection("clusters")
        .map(|c| {
            c.values()
                .filter(|cl| {
                    params
                        .non_empty("server")
                        .is_none_or(|s| str_field(cl, "/server") == s)
                        && params
                            .non_empty("name")
                            .is_none_or(|n| str_field(cl, "/name") == n)
                })
                .map(|cl| {
                    let server = str_field(cl, "/server");
                    let name = str_field(cl, "/name");
                    let count = apps
                        .iter()
                        .filter(|a| {
                            str_field(a, "/spec/destination/server") == server
                                || (!name.is_empty()
                                    && str_field(a, "/spec/destination/name") == name)
                        })
                        .count();
                    let mut cl = cl.clone();
                    cl["info"]["applicationsCount"] = json!(count);
                    cl
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(json(
        StatusCode::OK,
        json!({ "metadata": {}, "items": items }),
    ))
}

pub async fn list_projects(State(world): State<SharedWorld>) -> ApiResult {
    let world = world.lock().expect("world lock");
    let items: Vec<Value> = model::store(&world)
        .collection("projects")
        .map(|c| c.values().cloned().collect())
        .unwrap_or_default();
    Ok(json(
        StatusCode::OK,
        json!({ "metadata": {}, "items": items }),
    ))
}

pub async fn get_project(State(world): State<SharedWorld>, Path(name): Path<String>) -> ApiResult {
    let world = world.lock().expect("world lock");
    model::store(&world)
        .collection("projects")
        .and_then(|c| c.get(&name))
        .map(|p| json(StatusCode::OK, p.clone()))
        .ok_or_else(|| ArgoError::project_not_found(&name))
}
