use std::net::SocketAddr;

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{ConnectInfo, Request, State},
    http::{Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use tokio::net::TcpListener;

use crate::{
    journal::{JournalEntry, body_value},
    service::MockService,
    world::SharedWorld,
};

const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone)]
struct Recorder {
    world: SharedWorld,
    service: &'static str,
}

/// The service's router with clock ticking and journaling applied, bound to `world`.
pub fn service_app(world: SharedWorld, service: &dyn MockService) -> Router {
    let recorder = Recorder {
        world: world.clone(),
        service: service.name(),
    };
    service
        .router()
        .layer(middleware::from_fn_with_state(recorder, record))
        .with_state(world)
}

/// Serves `app`, attaching the caller's address to each request for the journal.
pub async fn serve(listener: TcpListener, app: Router) -> std::io::Result<()> {
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
}

async fn record(State(recorder): State<Recorder>, request: Request, next: Next) -> Response {
    let (parts, body) = request.into_parts();
    let Ok(request_bytes) = to_bytes(body, MAX_BODY_BYTES).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let method = parts.method.clone();
    let header = |name: &str| {
        parts
            .headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let host = header("host").or_else(|| parts.uri.authority().map(|a| a.to_string()));
    let scheme = header("x-forwarded-proto").unwrap_or_else(|| "http".to_owned());
    let content_type = header("content-type");
    let path = parts.uri.path().to_owned();
    let query = parts.uri.query().map(str::to_owned);
    let source = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip().to_string())
        .unwrap_or_else(|| "unknown".to_owned());

    if is_mutating(&method) {
        recorder.world.lock().expect("world lock").clock.tick();
    }

    let response = next
        .run(Request::from_parts(
            parts,
            Body::from(request_bytes.clone()),
        ))
        .await;
    let (response_parts, response_body) = response.into_parts();
    let response_bytes = to_bytes(response_body, MAX_BODY_BYTES)
        .await
        .unwrap_or_default();

    {
        let mut world = recorder.world.lock().expect("world lock");
        let entry = JournalEntry {
            seq: world.journal.next_seq(),
            time: world.clock.now_rfc3339(),
            service: recorder.service.to_owned(),
            source,
            method: method.to_string(),
            host,
            scheme,
            content_type,
            path,
            query,
            request_body: body_value(&request_bytes),
            status: response_parts.status.as_u16(),
            response_body: body_value(&response_bytes),
        };
        world.journal.push(entry);
    }

    Response::from_parts(response_parts, Body::from(response_bytes))
}

fn is_mutating(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}
