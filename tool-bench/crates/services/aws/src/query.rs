//! AWS query protocol envelopes (IAM, STS).

use axum::{http::StatusCode, response::Response};

use crate::util::{esc, xml_response};

pub fn ok(ns: &str, action: &str, result: Option<String>, request_id: &str) -> Response {
    let result = result
        .map(|r| format!("<{action}Result>{r}</{action}Result>"))
        .unwrap_or_default();
    xml_response(
        StatusCode::OK,
        format!(
            "<{action}Response xmlns=\"{ns}\">{result}<ResponseMetadata><RequestId>{request_id}</RequestId></ResponseMetadata></{action}Response>"
        ),
    )
}

pub fn error(
    ns: &str,
    status: StatusCode,
    code: &str,
    message: &str,
    request_id: &str,
) -> Response {
    let kind = if status.is_server_error() {
        "Receiver"
    } else {
        "Sender"
    };
    xml_response(
        status,
        format!(
            "<ErrorResponse xmlns=\"{ns}\"><Error><Type>{kind}</Type><Code>{code}</Code><Message>{}</Message></Error><RequestId>{request_id}</RequestId></ErrorResponse>",
            esc(message)
        ),
    )
}
