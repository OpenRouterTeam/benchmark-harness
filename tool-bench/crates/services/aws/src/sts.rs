//! STS (query protocol): caller identity and deterministic temporary credentials.
//! The caller is `account.json`'s `UserName` (default `ops-automation`).

use axum::{http::StatusCode, response::Response};
use serde_json::Value;

use crate::{
    Ctx, Req, query,
    util::{Params, el, iso, s, tel},
};

const NS: &str = "https://sts.amazonaws.com/doc/2011-06-15/";

pub fn caller(ctx: &Ctx) -> (String, String) {
    let user = ctx
        .docs("account")
        .first()
        .map(|(_, a)| s(a, "UserName"))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "ops-automation".into());
    let user_id = ctx
        .get("users", &user)
        .map(|u| s(&u, "UserId"))
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| "AIDATOOLBENCH0000000".into());
    (user, user_id)
}

pub fn handle(ctx: &mut Ctx, req: &Req) -> Response {
    let p = Params::parse(&req.body_text());
    let action = p.str("Action");
    let rid = ctx.request_id();
    let creds = |ctx: &mut Ctx, secs: i64| {
        let key = format!(
            "ASIA{}",
            ctx.svc
                .ids
                .next_prefixed("sts-key", "", 16)
                .to_ascii_uppercase()
        );
        let secret = ctx.svc.ids.next_prefixed("sts-secret", "", 40);
        let token = ctx.svc.ids.next_prefixed("sts-token", "FwoGZXIvYXdzE", 200);
        el(
            "Credentials",
            &format!(
                "{}{}{}{}",
                tel("AccessKeyId", &key),
                tel("SecretAccessKey", &secret),
                tel("SessionToken", &token),
                tel("Expiration", &iso(ctx.now + secs))
            ),
        )
    };
    let result = match action.as_str() {
        "GetCallerIdentity" => {
            let (user, user_id) = caller(ctx);
            Ok(format!(
                "{}{}{}",
                tel("Arn", &format!("arn:aws:iam::{}:user/{user}", ctx.account)),
                tel("UserId", &user_id),
                tel("Account", &ctx.account)
            ))
        }
        "GetSessionToken" => {
            let secs = p.int("DurationSeconds").unwrap_or(43_200);
            Ok(creds(ctx, secs))
        }
        "AssumeRole" => {
            let role_arn = p.str("RoleArn");
            let session = p.str("RoleSessionName");
            let role_name = role_arn.rsplit('/').next().unwrap_or_default().to_owned();
            match ctx.get("roles", &role_name) {
                Some(role) if role_arn.contains(&format!(":{}:", ctx.account)) => {
                    let secs = p.int("DurationSeconds").unwrap_or(3600);
                    let c = creds(ctx, secs);
                    let role_id = role
                        .get("RoleId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    Ok(format!(
                        "{c}{}",
                        el(
                            "AssumedRoleUser",
                            &format!(
                                "{}{}",
                                tel("AssumedRoleId", &format!("{role_id}:{session}")),
                                tel(
                                    "Arn",
                                    &format!(
                                        "arn:aws:sts::{}:assumed-role/{role_name}/{session}",
                                        ctx.account
                                    )
                                )
                            )
                        )
                    ))
                }
                _ => {
                    let (user, _) = caller(ctx);
                    Err((
                        StatusCode::FORBIDDEN,
                        "AccessDenied",
                        format!(
                            "User: arn:aws:iam::{}:user/{user} is not authorized to perform: sts:AssumeRole on resource: {role_arn}",
                            ctx.account
                        ),
                    ))
                }
            }
        }
        "GetAccessKeyInfo" => Ok(tel("Account", &ctx.account)),
        other => Err((
            StatusCode::BAD_REQUEST,
            "InvalidAction",
            format!("Could not find operation {other} for version 2011-06-15"),
        )),
    };
    match result {
        Ok(body) => query::ok(NS, &action, Some(body), &rid),
        Err((status, code, msg)) => query::error(NS, status, code, &msg, &rid),
    }
}
