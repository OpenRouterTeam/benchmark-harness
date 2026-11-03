//! `servicereference.us-east-1.amazonaws.com`: the index (`/`) and per-service documents
//! (`/v1/<svc>/<svc>.json`). `aws-api-mcp-server` reads the index at startup and a
//! service's document the first time it classifies an operation of that service.
//!
//! Pinned copies (fetched 2026-10-30) are kept for the services the scenarios use,
//! trimmed to the fields the server reads (`Name`, `Actions[].Name`,
//! `Actions[].Annotations`). Other services get a document with no actions, which the
//! server treats as "not read-only" (allowed, since read-only mode is off).

use axum::{http::StatusCode, response::Response};
use serde_json::json;

use crate::util::{json_response, respond};

const INDEX: &str = include_str!("../assets/service-reference-index.json");

const DOCS: [(&str, &str); 8] = [
    (
        "account",
        include_str!("../assets/service-reference/account.json"),
    ),
    (
        "cloudwatch",
        include_str!("../assets/service-reference/cloudwatch.json"),
    ),
    ("ec2", include_str!("../assets/service-reference/ec2.json")),
    ("iam", include_str!("../assets/service-reference/iam.json")),
    (
        "lambda",
        include_str!("../assets/service-reference/lambda.json"),
    ),
    (
        "logs",
        include_str!("../assets/service-reference/logs.json"),
    ),
    ("s3", include_str!("../assets/service-reference/s3.json")),
    ("sts", include_str!("../assets/service-reference/sts.json")),
];

pub fn serve(path: &str) -> Response {
    if path == "/" || path.is_empty() {
        return respond(StatusCode::OK, "application/json", INDEX);
    }
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    if let ["v1", svc, file] = parts.as_slice()
        && *file == format!("{svc}.json")
    {
        if let Some((_, doc)) = DOCS.iter().find(|(name, _)| name == svc) {
            return respond(StatusCode::OK, "application/json", *doc);
        }
        if INDEX.contains(&format!("\"service\" : \"{svc}\"")) {
            return json_response(
                StatusCode::OK,
                "application/json",
                &json!({ "Name": svc, "Actions": [], "Version": "v1.4" }),
            );
        }
    }
    respond(
        StatusCode::NOT_FOUND,
        "application/xml",
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Error><Code>NoSuchKey</Code><Message>The specified key does not exist.</Message></Error>",
    )
}
