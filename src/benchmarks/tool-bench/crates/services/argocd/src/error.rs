//! Argo CD API errors: grpc-gateway bodies `{"error", "code", "message"}` with the gRPC code
//! mapped to its HTTP status.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use mock_core::http::json;
use serde_json::json as json_value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgoError {
    pub status: StatusCode,
    pub code: u8,
    pub message: String,
}

impl ArgoError {
    fn new(status: StatusCode, code: u8, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    /// gRPC `Unknown` (2).
    pub fn unknown(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, 2, message)
    }

    /// gRPC `InvalidArgument` (3).
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, 3, message)
    }

    /// gRPC `NotFound` (5).
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, 5, message)
    }

    /// gRPC `PermissionDenied` (7).
    pub fn permission_denied(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, 7, message)
    }

    /// gRPC `FailedPrecondition` (9).
    pub fn failed_precondition(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, 9, message)
    }

    /// gRPC `Unimplemented` (12), which the gateway also uses for a wrong HTTP method.
    pub fn method_not_allowed() -> Self {
        Self::new(StatusCode::METHOD_NOT_ALLOWED, 12, "Method Not Allowed")
    }

    /// gRPC `Unauthenticated` (16).
    pub fn unauthenticated(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, 16, message)
    }

    pub fn app_not_found(name: &str) -> Self {
        Self::not_found(format!("applications.argoproj.io \"{name}\" not found"))
    }

    pub fn project_not_found(name: &str) -> Self {
        Self::not_found(format!("appprojects.argoproj.io \"{name}\" not found"))
    }

    pub fn bad_json() -> Self {
        Self::invalid("invalid character looking for beginning of value")
    }

    /// The gateway's error chunk inside a server-streaming response (sent with HTTP 200).
    pub fn stream_chunk(&self) -> String {
        let http_status = self.status.canonical_reason().unwrap_or("Unknown");
        format!(
            "{}\n",
            json_value!({
                "error": {
                    "grpc_code": self.code,
                    "http_code": self.status.as_u16(),
                    "message": self.message,
                    "http_status": http_status,
                }
            })
        )
    }
}

impl IntoResponse for ArgoError {
    fn into_response(self) -> Response {
        json(
            self.status,
            json_value!({ "error": self.message, "code": self.code, "message": self.message }),
        )
    }
}

pub type ApiResult = Result<Response, ArgoError>;
