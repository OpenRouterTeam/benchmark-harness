//! Mock AWS account behind a single endpoint (`AWS_ENDPOINT_URL=http://world:8116`), as
//! called by `aws-api-mcp-server` (AWS CLI/botocore in-process), `cloudwatch-mcp-server`
//! and `iam-mcp-server` (boto3).
//!
//! Every SDK request is dispatched on the SigV4 credential scope's service name
//! (`Authorization: AWS4-HMAC-SHA256 Credential=AKID/date/region/<service>/aws4_request`),
//! then on the protocol's operation selector:
//!
//! | scope service | protocol (botocore 1.43 pick) | operation selector |
//! |---|---|---|
//! | `iam`, `sts` | query (XML) | `Action=` form field |
//! | `ec2` | ec2 (XML) | `Action=` form field |
//! | `monitoring` | JSON 1.0 (awsQueryCompatible) | `X-Amz-Target: GraniteServiceVersion20100801.*` |
//! | `logs` | JSON 1.1 | `X-Amz-Target: Logs_20140328.*` |
//! | `s3` | REST-XML, path-style | method + path + subresource |
//! | `lambda` | REST-JSON | method + path |
//! | `account` | REST-JSON | path (`/listRegions`) |
//!
//! Signatures are not checked. Other services get an empty, protocol-correct success.
//! `servicereference.us-east-1.amazonaws.com` (the service reference the aws-api server
//! reads to classify read-only operations) is served over TLS.
//!
//! Not reachable: the cloudwatch server's PromQL tools sign requests for
//! `https://monitoring.<region>.amazonaws.com` and send them with a bare
//! `requests.Session.send`, which ignores `REQUESTS_CA_BUNDLE`, so they cannot trust the
//! world's CA; those tools return their connection error.

mod account;
mod cloudwatch;
mod ec2;
mod iam;
mod insights;
mod lambda;
mod logs;
mod query;
mod reference;
mod s3;
mod sts;
mod suggest;
pub mod util;

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::Response,
};
use mock_core::{service::MockService, world::ServiceState, world::SharedWorld};
use serde_json::{Value, json};

use crate::util::{json_response, parse_pairs, percent_decode};

pub const NAME: &str = "aws";
pub const PORT: u16 = 8116;
pub const DEFAULT_ACCOUNT: &str = "123456789012";

pub struct Aws;

impl MockService for Aws {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port(&self) -> u16 {
        PORT
    }

    fn hosts(&self) -> &'static [&'static str] {
        &["servicereference.us-east-1.amazonaws.com"]
    }

    fn router(&self) -> Router<SharedWorld> {
        Router::new().fallback(dispatch)
    }
}

/// One decoded SDK request.
#[derive(Debug, Clone)]
pub struct Req {
    pub method: Method,
    /// Percent-decoded path.
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: HeaderMap,
    pub body: Bytes,
    pub region: String,
    pub service: String,
}

impl Req {
    pub fn q(&self, key: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn has_q(&self, key: &str) -> bool {
        self.query.iter().any(|(k, _)| k == key)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn body_json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|_| json!({}))
    }

    /// `X-Amz-Target` operation name after the dot.
    pub fn target_op(&self) -> Option<String> {
        self.header("x-amz-target")
            .and_then(|t| t.rsplit('.').next())
            .map(str::to_owned)
    }
}

/// What a handler sees: the service's documents and the logical time.
pub struct Ctx<'a> {
    pub svc: &'a mut ServiceState,
    pub now: i64,
    pub account: String,
    pub region: String,
}

impl Ctx<'_> {
    pub fn request_id(&mut self) -> String {
        self.svc.ids.next_uuid("request-id").to_string()
    }

    /// Clones of the documents in a collection, in dataset order.
    pub fn docs(&self, collection: &str) -> Vec<(String, Value)> {
        self.svc
            .store
            .collection(collection)
            .map(|c| c.iter().map(|(k, v)| (k.to_owned(), v.clone())).collect())
            .unwrap_or_default()
    }

    pub fn get(&self, collection: &str, key: &str) -> Option<Value> {
        self.svc
            .store
            .collection(collection)
            .and_then(|c| c.get(key))
            .cloned()
    }

    pub fn put(&mut self, collection: &str, key: &str, doc: Value) {
        self.svc.store.collection_mut(collection).insert(key, doc);
    }

    pub fn remove(&mut self, collection: &str, key: &str) -> Option<Value> {
        self.svc.store.collection_mut(collection).remove(key)
    }

    pub fn get_mut(&mut self, collection: &str, key: &str) -> Option<&mut Value> {
        self.svc.store.collection_mut(collection).get_mut(key)
    }

    /// Resources live in `us-east-1` unless their document says otherwise.
    pub fn in_region(&self, doc: &Value) -> bool {
        doc.get("_region")
            .and_then(Value::as_str)
            .unwrap_or("us-east-1")
            == self.region
    }
}

async fn dispatch(
    State(world): State<SharedWorld>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut resp = route(&world, method, &uri, headers, body);
    // AWS responses carry a Date header; use the logical clock so replies stay
    // byte-identical across runs (hyper only adds its own when none is set).
    let now = world.lock().expect("world lock").clock.now_unix();
    if let Ok(v) = axum::http::HeaderValue::from_str(&s3::http_date(now)) {
        resp.headers_mut().insert(axum::http::header::DATE, v);
    }
    resp
}

fn route(
    world: &SharedWorld,
    method: Method,
    uri: &Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let path = percent_decode(uri.path(), false);
    let query = uri.query().map(parse_pairs).unwrap_or_default();

    if host.starts_with("servicereference.") {
        return reference::serve(&path);
    }
    if path == "/suggest-aws-commands" {
        return suggest::serve(&body);
    }

    let (region, service) = credential_scope(&headers).unwrap_or_default();
    if service.is_empty() {
        // Unsigned requests: the service-reference paths answer without a Host match too.
        if method == Method::GET && (path == "/" || path.starts_with("/v1/")) {
            return reference::serve(&path);
        }
        return json_response(
            StatusCode::FORBIDDEN,
            "application/json",
            &json!({"message": "Missing Authentication Token"}),
        );
    }

    let req = Req {
        method,
        path,
        query,
        headers,
        body,
        region,
        service,
    };
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    let now = world.clock.now_unix();
    let svc = world.service_mut(NAME);
    let account = svc
        .store
        .collection("account")
        .and_then(|c| c.values().next())
        .and_then(|a| a.get("AccountId").and_then(Value::as_str))
        .unwrap_or(DEFAULT_ACCOUNT)
        .to_owned();
    let mut ctx = Ctx {
        svc,
        now,
        account,
        region: req.region.clone(),
    };
    match req.service.as_str() {
        "iam" => iam::handle(&mut ctx, &req),
        "sts" => sts::handle(&mut ctx, &req),
        "ec2" => ec2::handle(&mut ctx, &req),
        "s3" => s3::handle(&mut ctx, &req),
        "monitoring" => cloudwatch::handle(&mut ctx, &req),
        "logs" => logs::handle(&mut ctx, &req),
        "lambda" => lambda::handle(&mut ctx, &req),
        "account" => account::handle(&mut ctx, &req),
        _ => generic_empty(&mut ctx, &req),
    }
}

/// `(region, service)` from `Credential=AKID/20261030/us-east-1/iam/aws4_request`.
fn credential_scope(headers: &HeaderMap) -> Option<(String, String)> {
    let auth = headers.get("authorization")?.to_str().ok()?;
    let cred = auth.split("Credential=").nth(1)?;
    let cred = cred.split([',', ' ']).next()?;
    let parts: Vec<&str> = cred.split('/').collect();
    (parts.len() >= 5).then(|| (parts[2].to_owned(), parts[3].to_owned()))
}

/// Services this mock does not model answer every operation with an empty success in
/// their protocol, which the SDK parses as "nothing here".
fn generic_empty(ctx: &mut Ctx, req: &Req) -> Response {
    if let Some(target) = req.header("x-amz-target") {
        let ct = if target.contains("_20") || target.contains("Logs") {
            "application/x-amz-json-1.1"
        } else {
            "application/x-amz-json-1.0"
        };
        return json_response(StatusCode::OK, ct, &json!({}));
    }
    let ct = req.header("content-type").unwrap_or_default();
    if ct.starts_with("application/x-www-form-urlencoded") {
        let params = util::Params::parse(&req.body_text());
        let action = params.str("Action");
        let rid = ctx.request_id();
        return util::xml_response(
            StatusCode::OK,
            format!(
                "<{action}Response><{action}Result/><ResponseMetadata><RequestId>{rid}</RequestId></ResponseMetadata></{action}Response>"
            ),
        );
    }
    json_response(StatusCode::OK, "application/json", &json!({}))
}
