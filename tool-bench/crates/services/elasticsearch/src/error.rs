//! Elasticsearch error envelopes:
//! `{"error": {"root_cause": [...], "type": ..., "reason": ...}, "status": N}`.

use axum::{http::StatusCode, response::Response};
use serde_json::{Map, Value, json};

use crate::respond;

#[derive(Debug, Clone)]
pub struct EsError {
    pub status: StatusCode,
    pub kind: String,
    pub reason: String,
    pub extra: Map<String, Value>,
    /// Overrides `root_cause` (wrapped errors report the innermost cause).
    pub root_cause: Option<Value>,
}

impl EsError {
    pub fn new(status: StatusCode, kind: &str, reason: impl Into<String>) -> Self {
        Self {
            status,
            kind: kind.to_owned(),
            reason: reason.into(),
            extra: Map::new(),
            root_cause: None,
        }
    }

    pub fn with(mut self, key: &str, value: Value) -> Self {
        self.extra.insert(key.to_owned(), value);
        self
    }

    pub fn index_not_found(index: &str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "index_not_found_exception",
            format!("no such index [{index}]"),
        )
        .with("resource.type", json!("index_or_alias"))
        .with("resource.id", json!(index))
        .with("index_uuid", json!("_na_"))
        .with("index", json!(index))
    }

    pub fn parsing(reason: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "parsing_exception", reason)
            .with("line", json!(1))
            .with("col", json!(1))
    }

    pub fn illegal(reason: impl Into<String>) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "illegal_argument_exception",
            reason,
        )
    }

    pub fn verification(problems: &[String]) -> Self {
        let n = problems.len();
        let reason = format!(
            "Found {n} problem{}\n{}",
            if n == 1 { "" } else { "s" },
            problems.join("\n")
        );
        Self::new(StatusCode::BAD_REQUEST, "verification_exception", reason)
    }

    pub fn body(&self) -> Value {
        let mut cause = Map::new();
        cause.insert("type".into(), json!(self.kind));
        cause.insert("reason".into(), json!(self.reason));
        for (k, v) in &self.extra {
            cause.insert(k.clone(), v.clone());
        }
        // `root_cause` comes first, as in real responses.
        let mut ordered = Map::new();
        ordered.insert(
            "root_cause".into(),
            self.root_cause
                .clone()
                .unwrap_or_else(|| json!([Value::Object(cause.clone())])),
        );
        for (k, v) in cause {
            ordered.insert(k, v);
        }
        json!({ "error": Value::Object(ordered), "status": self.status.as_u16() })
    }

    pub fn response(&self) -> Response {
        respond(self.status, self.body())
    }
}
