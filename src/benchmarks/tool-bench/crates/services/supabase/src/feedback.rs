//! Supabase's own feedback project (`xguihxuzqibwxjnimxev.supabase.co`), which
//! `submit_feedback` / `delete_feedback` call through PostgREST. Served over the world's TLS.

use std::collections::HashMap;

use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use mock_core::{http::json, world::SharedWorld};
use serde_json::{Value, json as json_value};

use crate::NAME;

pub const HOST: &str = "xguihxuzqibwxjnimxev.supabase.co";
const COLLECTION: &str = "feedback";

/// `POST /rest/v1/rpc/submit_interfaces_feedback`: stores the feedback, returns its delete token.
pub async fn submit(State(world): State<SharedWorld>, Json(body): Json<Value>) -> Response {
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    let now = w.clock.now_rfc3339();
    let service = w.service_mut(NAME);
    let token = service.ids.next_prefixed("feedback_token", "", 32);
    let mut doc = body;
    if let Some(obj) = doc.as_object_mut() {
        obj.insert("delete_token".to_owned(), json_value!(token));
        obj.insert("created_at".to_owned(), json_value!(now));
    }
    service
        .store
        .collection_mut(COLLECTION)
        .insert(token.clone(), doc);
    json(StatusCode::OK, json_value!(token))
}

/// `DELETE /rest/v1/interfaces_feedback?delete_token=eq.<token>` with `Prefer: count=exact`.
pub async fn delete(
    State(world): State<SharedWorld>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let token = query
        .get("delete_token")
        .and_then(|v| v.strip_prefix("eq."))
        .unwrap_or_default();
    let removed = world
        .lock()
        .expect("world lock")
        .service_mut(NAME)
        .store
        .collection_mut(COLLECTION)
        .remove(token)
        .is_some();
    let mut response = StatusCode::NO_CONTENT.into_response();
    let range = if removed { "*/1" } else { "*/0" };
    response
        .headers_mut()
        .insert("content-range", HeaderValue::from_static(range));
    response
}
