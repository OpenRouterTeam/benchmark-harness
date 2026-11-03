//! Mock Grafana 11.6 for `mcp-grafana` v2.1.1 (`GRAFANA_URL` pointed at the world).
//!
//! Serves the HTTP API the server's default tools call: frontend settings, health, user and
//! orgs, search, dashboards (+versions), folders, datasources (+health, resources/proxy for
//! Prometheus, Loki, Tempo and Pyroscope), `/api/ds/query`, annotations, snapshots,
//! short URLs, plugins, teams, Grafana-managed alerting (ruler view, provisioning, silences,
//! alerts), Grafana IRM (OnCall public API and the Incident API), Asserts, and `/render`.
//!
//! Dataset collections (all keyed as noted):
//! `user` (`current`), `orgs` (id), `teams` (id), `folders` (uid), `dashboards` (uid:
//! `{dashboard, folderUid, created, updated, createdBy, updatedBy}`), `dashboard_versions`
//! (`<uid>/<version>`), `datasources` (uid), `annotations` (id), `snapshots` (key),
//! `alert_rules` (uid), `contact_points` (uid), `policies` (`root`), `mute_timings` (name),
//! `silences` (id), `alerts` (fingerprint), `prom_series` / `prom_metadata`,
//! `loki_streams`, `tempo_traces` (each with a `datasource` uid), `plugins` (id),
//! `oncall_*` (id), `incidents` (incidentID), `incident_activity`, `incident_fields`.

#![allow(clippy::result_large_err)]

mod alerting;
mod api;
mod ds;
mod irm;
pub mod logql;
pub mod promql;

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use mock_core::{http::json, service::MockService, world::SharedWorld};
use serde_json::{Map, Value, json as json_value};

pub const NAME: &str = "grafana";
pub const PORT: u16 = 8114;
pub const VERSION: &str = "11.6.4";
pub const TOKEN: &str = "glsa_toolbench";
pub const PUBLIC_URL: &str = "https://grafana.octo-corp.com";

pub struct Grafana;

impl MockService for Grafana {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port(&self) -> u16 {
        PORT
    }

    fn hosts(&self) -> &'static [&'static str] {
        &[]
    }

    fn router(&self) -> Router<SharedWorld> {
        Router::new().fallback(handle)
    }
}

pub(crate) struct Req {
    pub method: Method,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl Req {
    pub fn q(&self, key: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn qs(&self, key: &str) -> Vec<&str> {
        self.query
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    /// Query parameters plus form-encoded body parameters (Prometheus/Loki POSTs).
    pub fn params(&self) -> Vec<(String, String)> {
        let mut out = self.query.clone();
        let ct = self
            .headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        if ct.starts_with("application/x-www-form-urlencoded") {
            out.extend(parse_query(std::str::from_utf8(&self.body).ok()));
        }
        out
    }

    pub fn json(&self) -> Result<Value, Response> {
        if self.body.is_empty() {
            return Ok(Value::Object(Map::new()));
        }
        serde_json::from_slice(&self.body)
            .map_err(|e| error(StatusCode::BAD_REQUEST, &format!("bad request data: {e}")))
    }

    pub fn host(&self) -> String {
        self.headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("world:8114")
            .to_owned()
    }
}

pub(crate) fn error(code: StatusCode, message: &str) -> Response {
    json(code, json_value!({ "message": message, "traceID": "" }))
}

pub(crate) fn not_found(message: &str) -> Response {
    error(StatusCode::NOT_FOUND, message)
}

pub(crate) fn ok(v: Value) -> Response {
    json(StatusCode::OK, v)
}

pub(crate) fn text(
    code: StatusCode,
    content_type: &'static str,
    body: impl Into<Vec<u8>>,
) -> Response {
    (code, [(header::CONTENT_TYPE, content_type)], body.into()).into_response()
}

pub(crate) fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                match std::str::from_utf8(&bytes[i + 1..i + 3])
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                {
                    Some(v) => {
                        out.push(v);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(crate) fn parse_query(q: Option<&str>) -> Vec<(String, String)> {
    q.unwrap_or("")
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (url_decode(k), url_decode(v)),
            None => (url_decode(p), String::new()),
        })
        .collect()
}

pub(crate) fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// RFC 3339 to unix seconds (fractional).
pub(crate) fn parse_rfc3339(s: &str) -> Option<f64> {
    if s.len() < 19 {
        return None;
    }
    let num = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, se) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let mut rest = &s[19..];
    let mut frac = 0.0;
    if let Some(r) = rest.strip_prefix('.') {
        let end = r.find(|c: char| !c.is_ascii_digit()).unwrap_or(r.len());
        frac = format!("0.{}", &r[..end]).parse().unwrap_or(0.0);
        rest = &r[end..];
    }
    let offset = match rest {
        "" | "Z" | "z" => 0,
        r if r.len() == 6 => {
            let sign = if r.starts_with('-') { -1 } else { 1 };
            sign * (r.get(1..3)?.parse::<i64>().ok()? * 3600
                + r.get(4..6)?.parse::<i64>().ok()? * 60)
        }
        _ => return None,
    };
    Some((days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se - offset) as f64 + frac)
}

/// Unix seconds to RFC 3339 with millisecond precision when fractional.
pub(crate) fn rfc3339(secs: f64) -> String {
    let whole = secs.floor() as i64;
    let base = mock_core::clock::format_unix(whole);
    let ms = ((secs - whole as f64) * 1000.0).round() as i64;
    if ms == 0 {
        base
    } else {
        format!("{}.{ms:03}Z", base.trim_end_matches('Z'))
    }
}

/// Prometheus/Loki/Tempo time parameters: unix seconds (float), unix ms/ns, or RFC 3339.
pub(crate) fn parse_time(s: &str) -> Option<f64> {
    if let Ok(n) = s.parse::<f64>() {
        return Some(if n > 1e17 {
            n / 1e9
        } else if n > 1e14 {
            n / 1e6
        } else if n > 1e11 {
            n / 1e3
        } else {
            n
        });
    }
    parse_rfc3339(s)
}

pub(crate) struct Ctx<'a> {
    pub state: &'a mut mock_core::world::ServiceState,
    pub now: f64,
}

impl Ctx<'_> {
    pub fn coll(&self, name: &str) -> Vec<(String, Value)> {
        self.state
            .store
            .collection(name)
            .map(|c| c.iter().map(|(k, v)| (k.to_owned(), v.clone())).collect())
            .unwrap_or_default()
    }

    pub fn get(&self, coll: &str, key: &str) -> Option<Value> {
        self.state
            .store
            .collection(coll)
            .and_then(|c| c.get(key))
            .cloned()
    }

    pub fn put(&mut self, coll: &str, key: &str, doc: Value) {
        self.state.store.collection_mut(coll).insert(key, doc);
    }

    pub fn remove(&mut self, coll: &str, key: &str) -> Option<Value> {
        self.state.store.collection_mut(coll).remove(key)
    }

    pub fn now_str(&self) -> String {
        rfc3339(self.now)
    }

    /// Next integer id for `coll`: one more than the largest `field` in the dataset.
    pub fn next_id(&self, coll: &str, field: &str, floor: i64) -> i64 {
        self.coll(coll)
            .iter()
            .filter_map(|(_, v)| v[field].as_i64())
            .max()
            .unwrap_or(floor)
            .max(floor)
            + 1
    }

    /// A Grafana-style 14 character lowercase uid.
    pub fn new_uid(&mut self, kind: &str) -> String {
        let u = self.state.ids.next_uuid(kind).simple().to_string();
        let mut out = String::from("e");
        out.push_str(&u[..13]);
        out
    }

    pub fn user(&self) -> Value {
        self.get("user", "current").unwrap_or_else(|| {
            json_value!({
                "id": 2, "uid": "ee5t0lbench0sa", "email": "sa-1-mcp-toolbench", "name": "mcp-toolbench",
                "login": "sa-1-mcp-toolbench", "orgId": 1, "isGrafanaAdmin": false, "isDisabled": false,
                "isExternal": false, "isExternallySynced": false, "isGrafanaAdminExternallySynced": false,
                "authLabels": [], "updatedAt": "2026-01-12T09:00:00Z", "createdAt": "2026-01-12T09:00:00Z",
                "avatarUrl": "/avatar/46d229b033af06a191ff2267bca9ae56", "theme": "", "isServiceAccount": true
            })
        })
    }

    pub fn login(&self) -> String {
        self.user()["login"].as_str().unwrap_or("admin").to_owned()
    }
}

async fn handle(
    State(world): State<SharedWorld>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let req = Req {
        method,
        path: uri.path().to_owned(),
        query: parse_query(uri.query()),
        headers,
        body,
    };
    let auth = req
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let is_oncall = req.path.starts_with("/oncall/");
    if !is_oncall && auth != format!("Bearer {TOKEN}") {
        return json(
            StatusCode::UNAUTHORIZED,
            json_value!({"message": "Invalid API key", "messageId": "api-key.invalid", "statusCode": 401, "traceID": ""}),
        );
    }
    if is_oncall && auth.trim_start_matches("Bearer ") != TOKEN {
        return json(
            StatusCode::UNAUTHORIZED,
            json_value!({"detail": "Invalid token."}),
        );
    }
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    let now = world.clock.now_unix() as f64;
    let mut ctx = Ctx {
        state: world.service_mut(NAME),
        now,
    };
    route(&mut ctx, &req)
}

fn route(ctx: &mut Ctx, req: &Req) -> Response {
    let path = req.path.trim_end_matches('/').to_owned();
    let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match segs.as_slice() {
        ["oncall", "api", "v1", rest @ ..] => irm::oncall(ctx, req, rest),
        [
            "api",
            "plugins",
            "grafana-irm-app",
            "resources",
            "api",
            "v1",
            method,
        ] => irm::incident(ctx, req, method),
        ["api", "datasources", "uid", uid, "resources", rest @ ..]
        | ["api", "datasources", "proxy", "uid", uid, rest @ ..] => {
            let uid = (*uid).to_owned();
            ds::proxy(ctx, req, &uid, rest)
        }
        ["api", "datasources", "proxy", id, rest @ ..] if id.parse::<i64>().is_ok() => {
            let id: i64 = id.parse().unwrap_or(0);
            match ctx
                .coll("datasources")
                .into_iter()
                .find(|(_, d)| d["id"].as_i64() == Some(id))
            {
                Some((uid, _)) => ds::proxy(ctx, req, &uid, rest),
                None => not_found("Data source not found"),
            }
        }
        ["api", "ds", "query"] => ds::ds_query(ctx, req),
        ["api", "prometheus", ..]
        | ["api", "ruler", ..]
        | ["api", "alertmanager", ..]
        | ["api", "v1", "provisioning", ..] => alerting::route(ctx, req, &segs),
        ["render", ..] => api::render(req),
        ["apis", ..] => json(
            StatusCode::NOT_FOUND,
            json_value!({"kind": "Status", "apiVersion": "v1", "metadata": {}, "status": "Failure", "message": "the server could not find the requested resource", "reason": "NotFound", "details": {}, "code": 404}),
        ),
        ["api", ..] => api::route(ctx, req, &segs),
        _ => text(
            StatusCode::NOT_FOUND,
            "text/html; charset=UTF-8",
            "<!doctype html><html><head><title>Grafana</title></head><body>Page not found</body></html>",
        ),
    }
}
