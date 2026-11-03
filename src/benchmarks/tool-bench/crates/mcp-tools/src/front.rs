//! Streamable-HTTP MCP front end (single endpoint, JSON responses, `Mcp-Session-Id`).
//! What happens behind a session is up to the [`Backend`].

use std::{
    collections::HashMap,
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use std::convert::Infallible;

use axum::response::sse::{Event, Sse};
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::{Value, json};
use tokio_stream::{StreamExt, wrappers::UnboundedReceiverStream};

pub const SESSION_HEADER: &str = "mcp-session-id";

/// A backend's answer to a request: one JSON-RPC response, or a stream of messages
/// (server→client requests, then the response) sent to the client as SSE.
pub enum Reply {
    Json(Value),
    Stream(tokio::sync::mpsc::UnboundedReceiver<Value>),
}

/// One MCP server implementation behind the HTTP front.
pub trait Backend: Send + Sync + 'static {
    type Session: Send + Sync + 'static;
    /// Called for an `initialize` request without a session header.
    fn open(&self) -> impl Future<Output = anyhow::Result<Self::Session>> + Send;
    /// A request (has `id` and `method`).
    fn request(
        &self,
        session: &Self::Session,
        msg: Value,
    ) -> impl Future<Output = anyhow::Result<Reply>> + Send;
    /// A notification or client response.
    fn notify(
        &self,
        session: &Self::Session,
        msg: Value,
    ) -> impl Future<Output = anyhow::Result<()>> + Send;
    fn close(&self, session: Arc<Self::Session>) -> impl Future<Output = ()> + Send;
}

struct Front<B: Backend> {
    backend: B,
    sessions: Mutex<HashMap<String, Arc<B::Session>>>,
    next: AtomicU64,
}

pub fn router<B: Backend>(backend: B) -> Router {
    let front = Arc::new(Front {
        backend,
        sessions: Mutex::new(HashMap::new()),
        next: AtomicU64::new(1),
    });
    Router::new()
        .route(
            "/mcp",
            post(handle_post::<B>)
                .delete(handle_delete::<B>)
                .get(|| async { StatusCode::METHOD_NOT_ALLOWED }),
        )
        .with_state(front)
}

async fn handle_post<B: Backend>(
    State(front): State<Arc<Front<B>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
        return rpc_error(StatusCode::BAD_REQUEST, Value::Null, -32700, "Parse error");
    };
    if !msg.is_object() {
        return rpc_error(
            StatusCode::BAD_REQUEST,
            Value::Null,
            -32600,
            "Batching is not supported",
        );
    }
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let header = headers.get(SESSION_HEADER).and_then(|v| v.to_str().ok());
    let is_initialize = msg["method"] == "initialize";

    let (session_id, session) = match header {
        None if is_initialize => {
            let session = match front.backend.open().await {
                Ok(s) => Arc::new(s),
                Err(e) => return rpc_error(StatusCode::BAD_GATEWAY, id, -32603, &e.to_string()),
            };
            let session_id = format!("tb-{:06}", front.next.fetch_add(1, Ordering::Relaxed));
            front
                .sessions
                .lock()
                .expect("sessions lock")
                .insert(session_id.clone(), session.clone());
            (session_id, session)
        }
        None => {
            return rpc_error(
                StatusCode::BAD_REQUEST,
                id,
                -32600,
                "Missing Mcp-Session-Id",
            );
        }
        Some(sid) => match front
            .sessions
            .lock()
            .expect("sessions lock")
            .get(sid)
            .cloned()
        {
            Some(session) => (sid.to_owned(), session),
            None => return rpc_error(StatusCode::NOT_FOUND, id, -32001, "Session not found"),
        },
    };

    let is_request = msg.get("id").is_some() && msg.get("method").is_some();
    let mut response = if is_request {
        match front.backend.request(&session, msg).await {
            Ok(Reply::Json(reply)) => (
                StatusCode::OK,
                [(CONTENT_TYPE, "application/json")],
                reply.to_string(),
            )
                .into_response(),
            Ok(Reply::Stream(rx)) => {
                let events = UnboundedReceiverStream::new(rx).map(|msg| {
                    Ok::<_, Infallible>(Event::default().event("message").data(msg.to_string()))
                });
                Sse::new(events).into_response()
            }
            Err(e) => rpc_error(StatusCode::BAD_GATEWAY, id, -32603, &e.to_string()),
        }
    } else {
        match front.backend.notify(&session, msg).await {
            Ok(()) => StatusCode::ACCEPTED.into_response(),
            Err(e) => rpc_error(StatusCode::BAD_GATEWAY, id, -32603, &e.to_string()),
        }
    };
    response.headers_mut().insert(
        SESSION_HEADER,
        HeaderValue::from_str(&session_id).expect("session ids are ASCII"),
    );
    response
}

async fn handle_delete<B: Backend>(
    State(front): State<Arc<Front<B>>>,
    headers: HeaderMap,
) -> StatusCode {
    let Some(sid) = headers.get(SESSION_HEADER).and_then(|v| v.to_str().ok()) else {
        return StatusCode::BAD_REQUEST;
    };
    let removed = front.sessions.lock().expect("sessions lock").remove(sid);
    match removed {
        Some(session) => {
            front.backend.close(session).await;
            StatusCode::NO_CONTENT
        }
        None => StatusCode::NOT_FOUND,
    }
}

fn rpc_error(status: StatusCode, id: Value, code: i64, message: &str) -> Response {
    let body = json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}});
    (
        status,
        [(CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}
