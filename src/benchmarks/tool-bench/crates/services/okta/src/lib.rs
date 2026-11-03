//! Mock Okta org (`octo-corp.okta.com`) as called by `okta-mcp-server` through
//! okta-sdk-python: the OAuth device flow it runs at startup, and the management API
//! (`/api/v1`) for users, groups, apps, policies, the System Log, brands/themes/pages/
//! email templates, custom and email domains, and device assurance policies.
//!
//! Collections (dataset `dataset/okta/<name>.json`): `users`, `groups`, `group_members`
//! (`<groupId>/<userId>`), `group_rules`, `apps`, `app_users` (`<appId>/<userId>`),
//! `app_groups` (`<appId>/<groupId>`), `catalog`, `policies`, `policy_rules`
//! (`<policyId>/<ruleId>`), `factors` (`<userId>/<factorId>`), `logs` (by uuid),
//! `brands`, `themes` (`<brandId>/<themeId>`), `pages`, `email_customizations`,
//! `email_settings`, `domains`, `email_domains`, `device_assurances`, `user_types`.

mod apps;
mod custom;
pub mod filter;
mod groups;
mod logs;
mod policies;
mod users;
pub mod util;

use axum::{
    Router,
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, Method, StatusCode, Uri},
    response::Response,
    routing::post,
};
use mock_core::{http::json, service::MockService, world::SharedWorld};
use serde_json::{Value, json as json_value};

use crate::util::{Reply, Req, error, into_response};

pub const NAME: &str = "okta";
pub const PORT: u16 = 8120;
pub const ORG_URL: &str = "https://octo-corp.okta.com";

/// `exp` of every issued token: 2100-01-01T00:00:00Z. The server compares `exp` to the
/// wall clock, so it must outlive any real run; nothing else reads it.
const TOKEN_EXP: u64 = 4_102_444_800;

pub struct Okta;

impl MockService for Okta {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port(&self) -> u16 {
        PORT
    }

    fn hosts(&self) -> &'static [&'static str] {
        &["octo-corp.okta.com"]
    }

    fn router(&self) -> Router<SharedWorld> {
        Router::new()
            .route("/oauth2/v1/device/authorize", post(device_authorize))
            .route("/oauth2/v1/token", post(token))
            .fallback(api)
    }
}

async fn device_authorize() -> Response {
    json(
        StatusCode::OK,
        json_value!({
            "device_code": "toolbench-device-code",
            "user_code": "TOOLBNCH",
            "verification_uri": format!("{ORG_URL}/activate"),
            "verification_uri_complete": format!("{ORG_URL}/activate?user_code=TOOLBNCH"),
            "expires_in": 600,
            "interval": 1
        }),
    )
}

async fn token() -> Response {
    json(
        StatusCode::OK,
        json_value!({
            "token_type": "Bearer",
            "expires_in": 3600,
            "access_token": access_token(),
            "scope": "okta.users.read okta.users.manage okta.groups.read okta.groups.manage okta.apps.read okta.apps.manage",
            "refresh_token": "toolbench-refresh-token"
        }),
    )
}

/// A fixed JWT (signature not meaningful; the client decodes without verifying).
pub fn access_token() -> String {
    let header = json_value!({ "kid": "toolbench", "alg": "RS256" });
    let claims = json_value!({
        "ver": 1,
        "jti": "AT.toolbench",
        "iss": ORG_URL,
        "aud": ORG_URL,
        "sub": "ops@octo-corp.example",
        "iat": 1_767_225_600_u64,
        "exp": TOKEN_EXP,
        "cid": "0oatoolbench",
        "uid": "00utoolbench",
        "scp": ["okta.users.read", "okta.users.manage", "okta.groups.read", "okta.groups.manage", "okta.apps.read", "okta.apps.manage"]
    });
    format!(
        "{}.{}.{}",
        b64url(header.to_string().as_bytes()),
        b64url(claims.to_string().as_bytes()),
        b64url(b"toolbench-signature")
    )
}

/// Decodes the claims segment of a token produced by [`access_token`] (for tests).
pub fn decode_claims(jwt: &str) -> Value {
    let segment = jwt.split('.').nth(1).expect("claims segment");
    serde_json::from_slice(&b64url_decode(segment)).expect("claims JSON")
}

/// Every `/api/v1` request: parse, authenticate, dispatch by resource.
async fn api(
    State(world): State<SharedWorld>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut guard = world.lock().expect("world lock");
    let w = &mut *guard;
    let request_id = util::svc(w).ids.next_prefixed("request", "", 20);
    let path = uri.path().trim_end_matches('/').to_owned();
    let Some(rest) = path.strip_prefix("/api/v1/") else {
        return into_response(util::endpoint_not_found(), &request_id);
    };
    if !headers.contains_key("authorization") {
        return into_response(
            error(
                StatusCode::UNAUTHORIZED,
                "E0000011",
                "Invalid token provided",
                &[],
            ),
            &request_id,
        );
    }
    let query: Vec<(String, String)> = Query::<Vec<(String, String)>>::try_from_uri(&uri)
        .map(|q| q.0)
        .unwrap_or_default();
    let req = Req {
        method,
        segs: rest.split('/').map(str::to_owned).collect(),
        path: path.clone(),
        query,
        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
        raw_body: body.to_vec(),
    };
    let result = match req.segs[0].as_str() {
        "users" => users::handle(w, &req),
        "groups" => groups::handle(w, &req),
        "apps" | "catalog" => apps::handle(w, &req),
        "policies" => policies::handle(w, &req),
        "logs" if req.segs.len() == 1 && req.method == Method::GET => logs::list_logs(w, &req),
        "brands" | "domains" | "email-domains" | "device-assurances" => custom::handle(w, &req),
        "meta" => user_types(w, &req),
        _ => Err(util::endpoint_not_found()),
    };
    let reply = match result {
        Ok(r) | Err(r) => r,
    };
    into_response(reply, &request_id)
}

/// `/api/v1/meta/types/user`: the dataset's user types, or the org's default type.
fn user_types(w: &mut mock_core::world::WorldState, req: &Req) -> util::Res {
    let types: Vec<Value> = {
        let from_dataset: Vec<Value> = util::list(w, "user_types")
            .into_iter()
            .map(|(_, t)| t)
            .collect();
        if from_dataset.is_empty() {
            vec![json_value!({
                "id": users::DEFAULT_USER_TYPE,
                "name": "user",
                "displayName": "User",
                "description": "Okta user profile template with default permission settings",
                "default": true,
                "createdBy": "00utoolbench0000000",
                "lastUpdatedBy": "00utoolbench0000000",
                "created": "2024-01-08T17:02:11.000Z",
                "lastUpdated": "2024-01-08T17:02:11.000Z",
                "_links": { "schema": util::href("/api/v1/meta/schemas/user/osc1toolbench0000001"), "self": util::href(&format!("/api/v1/meta/types/user/{}", users::DEFAULT_USER_TYPE)) }
            })]
        } else {
            from_dataset
        }
    };
    let s: Vec<&str> = req.segs.iter().map(String::as_str).collect();
    match s.as_slice() {
        ["meta", "types", "user"] if req.method == Method::GET => {
            Ok(Reply::ok(Value::Array(types)))
        }
        ["meta", "types", "user", id] if req.method == Method::GET => types
            .into_iter()
            .find(|t| t["id"] == *id || (*id == "default" && t["default"] == true))
            .map(Reply::ok)
            .ok_or_else(|| util::not_found(id, "UserType")),
        _ => Err(util::endpoint_not_found()),
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn b64url(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(B64[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

fn b64url_decode(text: &str) -> Vec<u8> {
    let values: Vec<u32> = text
        .bytes()
        .map(|c| {
            B64.iter()
                .position(|b| *b == c)
                .expect("base64url character") as u32
        })
        .collect();
    let mut out = Vec::new();
    for chunk in values.chunks(4) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, v)| acc | (v << (18 - 6 * i)));
        for i in 0..chunk.len().saturating_sub(1) {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    out
}
