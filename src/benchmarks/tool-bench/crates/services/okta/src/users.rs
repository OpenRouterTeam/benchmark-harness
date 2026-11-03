//! `/api/v1/users`: CRUD, search, lifecycle, groups, app links, factors and sessions.

use axum::http::{Method, StatusCode};
use mock_core::world::WorldState;
use serde_json::{Map, Value, json};

use crate::{
    filter,
    groups::render_group,
    util::{
        Reply, Req, Res, bad_request, error, get, href, id_of, list, log_event, new_id, not_found,
        page, post_href, put, remove, ts, user_target, validation,
    },
};

pub const DEFAULT_USER_TYPE: &str = "oty1toolbench0000001";

pub fn handle(w: &mut WorldState, req: &Req) -> Res {
    let s: Vec<&str> = req.segs.iter().map(String::as_str).collect();
    let m = &req.method;
    match s.as_slice() {
        ["users"] if m == Method::GET => Ok(list_users(w, req)?),
        ["users"] if m == Method::POST => create_user(w, req),
        ["users", id] if m == Method::GET => {
            let u = find(w, id)?;
            Ok(Reply::ok(render(&u)))
        }
        ["users", id] if m == Method::POST => update_user(w, req, id, false),
        ["users", id] if m == Method::PUT => update_user(w, req, id, true),
        ["users", id] if m == Method::DELETE => delete_user(w, id),
        ["users", id, "lifecycle", op] if m == Method::POST => lifecycle(w, req, id, op),
        ["users", id, "groups"] if m == Method::GET => {
            let u = find(w, id)?;
            let uid = id_of(&u);
            let groups = user_groups(w, &uid);
            Ok(Reply::ok(Value::Array(
                groups.iter().map(render_group).collect(),
            )))
        }
        ["users", id, "appLinks"] if m == Method::GET => {
            let u = find(w, id)?;
            Ok(Reply::ok(Value::Array(app_links(w, &id_of(&u)))))
        }
        ["users", id, "blocks"] if m == Method::GET => {
            find(w, id)?;
            Ok(Reply::ok(json!([])))
        }
        ["users", id, "factors"] if m == Method::GET => {
            let uid = id_of(&find(w, id)?);
            let prefix = format!("{uid}/");
            let factors: Vec<Value> = list(w, "factors")
                .into_iter()
                .filter(|(k, _)| k.starts_with(&prefix))
                .map(|(_, v)| v)
                .collect();
            Ok(Reply::ok(Value::Array(factors)))
        }
        ["users", id, "factors", fid] if m == Method::GET => {
            let uid = id_of(&find(w, id)?);
            get(w, "factors", &format!("{uid}/{fid}"))
                .map(Reply::ok)
                .ok_or_else(|| not_found(fid, "UserFactor"))
        }
        ["users", id, "factors", fid] if m == Method::DELETE => {
            let u = find(w, id)?;
            let uid = id_of(&u);
            remove(w, "factors", &format!("{uid}/{fid}"))
                .ok_or_else(|| not_found(fid, "UserFactor"))?;
            log_event(
                w,
                "user.mfa.factor.deactivate",
                "Reset factor for user",
                vec![user_target(&u)],
            );
            Ok(Reply::no_content())
        }
        ["users", id, "sessions"] if m == Method::DELETE => {
            let u = find(w, id)?;
            log_event(
                w,
                "user.session.clear",
                "Clear user session",
                vec![user_target(&u)],
            );
            Ok(Reply::no_content())
        }
        _ => Err(crate::util::endpoint_not_found()),
    }
}

/// A user by ID, login, or unique login short name.
pub fn find(w: &mut WorldState, id_or_login: &str) -> Result<Value, Reply> {
    let users = list(w, "users");
    if let Some((_, u)) = users.iter().find(|(k, _)| k == id_or_login) {
        return Ok(u.clone());
    }
    let lower = id_or_login.to_lowercase();
    if let Some((_, u)) = users
        .iter()
        .find(|(_, u)| u["profile"]["login"].as_str().map(str::to_lowercase) == Some(lower.clone()))
    {
        return Ok(u.clone());
    }
    let short: Vec<&Value> = users
        .iter()
        .map(|(_, u)| u)
        .filter(|u| {
            u["profile"]["login"]
                .as_str()
                .and_then(|l| l.split('@').next())
                .map(str::to_lowercase)
                == Some(lower.clone())
        })
        .collect();
    if short.len() == 1 {
        return Ok(short[0].clone());
    }
    Err(not_found(id_or_login, "User"))
}

pub fn render(u: &Value) -> Value {
    let mut out = u.clone();
    let id = id_of(u);
    let base = format!("/api/v1/users/{id}");
    let mut links = Map::new();
    let status = u["status"].as_str().unwrap_or("");
    let lifecycle = |op: &str| post_href(&format!("{base}/lifecycle/{op}"));
    match status {
        "STAGED" => {
            links.insert("activate".into(), lifecycle("activate"));
        }
        "PROVISIONED" => {
            links.insert("reactivate".into(), lifecycle("reactivate"));
        }
        "DEPROVISIONED" => {
            links.insert("activate".into(), lifecycle("activate"));
        }
        "SUSPENDED" => {
            links.insert("unsuspend".into(), lifecycle("unsuspend"));
        }
        "LOCKED_OUT" => {
            links.insert("unlock".into(), lifecycle("unlock"));
        }
        _ => {
            links.insert("suspend".into(), lifecycle("suspend"));
            links.insert("resetPassword".into(), lifecycle("reset_password"));
            links.insert("expirePassword".into(), lifecycle("expire_password"));
            links.insert("resetFactors".into(), lifecycle("reset_factors"));
        }
    }
    if status != "DEPROVISIONED" {
        links.insert("deactivate".into(), lifecycle("deactivate"));
    }
    links.insert(
        "schema".into(),
        href("/api/v1/meta/schemas/user/osc1toolbench0000001"),
    );
    let type_id = u["type"]["id"]
        .as_str()
        .unwrap_or(DEFAULT_USER_TYPE)
        .to_owned();
    links.insert(
        "type".into(),
        href(&format!("/api/v1/meta/types/user/{type_id}")),
    );
    links.insert("self".into(), href(&base));
    out["_links"] = Value::Object(links);
    out
}

fn list_users(w: &mut WorldState, req: &Req) -> Res {
    let mut exprs = Vec::new();
    for key in ["search", "filter"] {
        if let Some(text) = req.q(key) {
            exprs.push(
                filter::parse(text)
                    .map_err(|_| bad_request("E0000031", "Invalid search criteria."))?,
            );
        }
    }
    let q = req.q("q").map(str::to_lowercase);
    let mentions_status = exprs.iter().any(|e| e.mentions("status"));
    let mut users: Vec<Value> = list(w, "users")
        .into_iter()
        .map(|(_, u)| u)
        .filter(|u| mentions_status || u["status"] != "DEPROVISIONED")
        .filter(|u| exprs.iter().all(|e| e.matches(u)))
        .filter(|u| {
            q.as_ref().is_none_or(|q| {
                ["firstName", "lastName", "email", "login"].iter().any(|f| {
                    u["profile"][f]
                        .as_str()
                        .is_some_and(|v| v.to_lowercase().starts_with(q.as_str()))
                })
            })
        })
        .collect();
    if let Some(sort_by) = req.q("sortBy") {
        users.sort_by(|a, b| {
            let av = filter::resolve(a, sort_by).first().map(|v| v.to_string());
            let bv = filter::resolve(b, sort_by).first().map(|v| v.to_string());
            av.cmp(&bv)
        });
        if req
            .q("sortOrder")
            .is_some_and(|o| o.eq_ignore_ascii_case("desc"))
        {
            users.reverse();
        }
    }
    let users: Vec<Value> = users.iter().map(render).collect();
    Ok(page(req, users, 200, 200, id_of))
}

fn validate_profile(
    w: &mut WorldState,
    profile: &Map<String, Value>,
    self_id: Option<&str>,
) -> Result<(), Reply> {
    let mut causes = Vec::new();
    for field in ["login", "email", "firstName", "lastName"] {
        if profile
            .get(field)
            .and_then(Value::as_str)
            .is_none_or(|v| v.trim().is_empty())
        {
            causes.push(format!("{field}: The field cannot be left blank"));
        }
    }
    if let Some(login) = profile.get("login").and_then(Value::as_str) {
        if !login.is_empty() && !login.contains('@') {
            causes.push("login: Username must be in the form of an email address".to_owned());
        }
        let taken = list(w, "users").iter().any(|(k, u)| {
            Some(k.as_str()) != self_id
                && u["profile"]["login"]
                    .as_str()
                    .is_some_and(|l| l.eq_ignore_ascii_case(login))
        });
        if taken {
            causes.push(
                "login: An object with this field already exists in the current organization"
                    .to_owned(),
            );
        }
    }
    if let Some(email) = profile.get("email").and_then(Value::as_str)
        && !email.is_empty()
        && !email.contains('@')
    {
        causes.push("email: Does not match required pattern".to_owned());
    }
    if causes.is_empty() {
        Ok(())
    } else {
        let refs: Vec<&str> = causes.iter().map(String::as_str).collect();
        Err(validation(&refs))
    }
}

fn create_user(w: &mut WorldState, req: &Req) -> Res {
    let body = req.object()?;
    let mut profile = body
        .get("profile")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    validate_profile(w, &profile, None)?;
    for f in ["secondEmail", "mobilePhone"] {
        profile.entry(f.to_owned()).or_insert(Value::Null);
    }
    let activate = req.q_bool("activate", true);
    let provider = req.q_bool("provider", false);
    let password = body
        .get("credentials")
        .and_then(|c| c.get("password"))
        .and_then(|p| p.get("value"))
        .and_then(Value::as_str)
        .is_some();
    let status = if !activate {
        "STAGED"
    } else if password || provider {
        "ACTIVE"
    } else {
        "PROVISIONED"
    };
    let now = ts(w);
    let id = new_id(w, "user", "00u");
    let email = profile.get("email").cloned().unwrap_or(Value::Null);
    let mut credentials = json!({
        "emails": [{ "value": email, "status": "VERIFIED", "type": "PRIMARY" }],
        "provider": { "type": "OKTA", "name": "OKTA" }
    });
    if password {
        credentials["password"] = json!({});
    }
    if let Some(rq) = body
        .get("credentials")
        .and_then(|c| c.get("recovery_question"))
        .and_then(|r| r.get("question"))
    {
        credentials["recovery_question"] = json!({ "question": rq });
    }
    let type_id = body
        .get("type")
        .and_then(|t| t.get("id"))
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_USER_TYPE)
        .to_owned();
    let user = json!({
        "id": id,
        "status": status,
        "created": now,
        "activated": if status == "ACTIVE" { json!(now) } else { Value::Null },
        "statusChanged": if status == "STAGED" { Value::Null } else { json!(now) },
        "lastLogin": null,
        "lastUpdated": now,
        "passwordChanged": if password { json!(now) } else { Value::Null },
        "type": { "id": type_id },
        "profile": Value::Object(profile),
        "credentials": credentials
    });
    put(w, "users", &id, user.clone());
    let mut group_ids: Vec<String> = body
        .get("groupIds")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|g| g.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    for (gid, g) in list(w, "groups") {
        if g["type"] == "BUILT_IN"
            && g["profile"]["name"] == "Everyone"
            && !group_ids.contains(&gid)
        {
            group_ids.push(gid);
        }
    }
    for gid in group_ids {
        if get(w, "groups", &gid).is_some() {
            crate::groups::add_member(w, &gid, &id);
        }
    }
    log_event(
        w,
        "user.lifecycle.create",
        "Create Okta user",
        vec![user_target(&user)],
    );
    if status != "STAGED" {
        log_event(
            w,
            "user.lifecycle.activate",
            "Activate Okta user",
            vec![user_target(&user)],
        );
    }
    Ok(Reply::ok(render(&user)))
}

fn update_user(w: &mut WorldState, req: &Req, id: &str, replace: bool) -> Res {
    let mut user = find(w, id)?;
    let uid = id_of(&user);
    let body = req.object()?;
    if let Some(profile) = body.get("profile").and_then(Value::as_object) {
        let mut merged = if replace {
            Map::new()
        } else {
            user["profile"].as_object().cloned().unwrap_or_default()
        };
        for (k, v) in profile {
            merged.insert(k.clone(), v.clone());
        }
        validate_profile(w, &merged, Some(&uid))?;
        user["profile"] = Value::Object(merged);
    } else if replace {
        return Err(validation(&["profile: The field cannot be left blank"]));
    }
    let now = ts(w);
    if let Some(pw) = body
        .get("credentials")
        .and_then(|c| c.get("password"))
        .and_then(|p| p.get("value"))
        && pw.is_string()
    {
        user["credentials"]["password"] = json!({});
        user["passwordChanged"] = json!(now);
    }
    user["lastUpdated"] = json!(now);
    put(w, "users", &uid, user.clone());
    log_event(
        w,
        "user.account.update_profile",
        "Update user profile for Okta",
        vec![user_target(&user)],
    );
    Ok(Reply::ok(render(&user)))
}

fn delete_user(w: &mut WorldState, id: &str) -> Res {
    let mut user = find(w, id)?;
    let uid = id_of(&user);
    if user["status"] != "DEPROVISIONED" {
        set_status(w, &mut user, "DEPROVISIONED");
        log_event(
            w,
            "user.lifecycle.deactivate",
            "Deactivate Okta user",
            vec![user_target(&user)],
        );
        return Ok(Reply::no_content());
    }
    remove(w, "users", &uid);
    for (k, _) in list(w, "factors") {
        if k.starts_with(&format!("{uid}/")) {
            remove(w, "factors", &k);
        }
    }
    for coll in ["group_members", "app_users"] {
        for (k, _) in list(w, coll) {
            if k.ends_with(&format!("/{uid}")) {
                remove(w, coll, &k);
            }
        }
    }
    log_event(
        w,
        "user.lifecycle.delete.initiated",
        "Delete Okta user",
        vec![user_target(&user)],
    );
    Ok(Reply::no_content())
}

fn set_status(w: &mut WorldState, user: &mut Value, status: &str) {
    let now = ts(w);
    user["status"] = json!(status);
    user["statusChanged"] = json!(now);
    user["lastUpdated"] = json!(now);
    if status == "ACTIVE" && user["activated"].is_null() {
        user["activated"] = json!(now);
    }
    if status == "DEPROVISIONED" {
        user["activated"] = Value::Null;
    }
    let uid = id_of(user);
    put(w, "users", &uid, user.clone());
}

fn lifecycle(w: &mut WorldState, req: &Req, id: &str, op: &str) -> Res {
    let mut user = find(w, id)?;
    let status = user["status"].as_str().unwrap_or("").to_owned();
    let send_email = req.q_bool("sendEmail", true);
    let token = |w: &mut WorldState| crate::util::svc(w).ids.next_prefixed("token", "", 20);
    let (event, display, reply) = match op {
        "activate" => {
            if !matches!(status.as_str(), "STAGED" | "DEPROVISIONED" | "PROVISIONED") {
                return Err(error(
                    StatusCode::FORBIDDEN,
                    "E0000016",
                    "Activation failed because the user is already active",
                    &[],
                ));
            }
            let next = if user["credentials"]["password"].is_object() {
                "ACTIVE"
            } else {
                "PROVISIONED"
            };
            set_status(w, &mut user, next);
            let body = if send_email {
                json!({})
            } else {
                let t = token(w);
                json!({ "activationUrl": format!("{}/welcome/{t}", crate::ORG_URL), "activationToken": t })
            };
            ("user.lifecycle.activate", "Activate Okta user", body)
        }
        "reactivate" => {
            if status != "PROVISIONED" {
                return Err(bad_request(
                    "E0000038",
                    "This operation is not allowed in the user's current status.",
                ));
            }
            set_status(w, &mut user, "PROVISIONED");
            let t = token(w);
            (
                "user.lifecycle.reactivate",
                "Reactivate Okta user",
                json!({ "activationUrl": format!("{}/welcome/{t}", crate::ORG_URL), "activationToken": t }),
            )
        }
        "deactivate" => {
            if status != "DEPROVISIONED" {
                set_status(w, &mut user, "DEPROVISIONED");
            }
            (
                "user.lifecycle.deactivate",
                "Deactivate Okta user",
                json!({}),
            )
        }
        "suspend" => {
            if status != "ACTIVE" {
                return Err(validation(&[
                    "status: Cannot suspend a user that is not active",
                ]));
            }
            set_status(w, &mut user, "SUSPENDED");
            ("user.lifecycle.suspend", "Suspend Okta user", json!({}))
        }
        "unsuspend" => {
            if status != "SUSPENDED" {
                return Err(validation(&[
                    "status: Cannot unsuspend a user that is not suspended",
                ]));
            }
            set_status(w, &mut user, "ACTIVE");
            ("user.lifecycle.unsuspend", "Unsuspend Okta user", json!({}))
        }
        "unlock" => {
            if status != "LOCKED_OUT" {
                return Err(error(
                    StatusCode::FORBIDDEN,
                    "E0000032",
                    "Unlock is not allowed for this user.",
                    &[],
                ));
            }
            set_status(w, &mut user, "ACTIVE");
            ("user.account.unlock", "Unlock user account", json!({}))
        }
        "reset_password" => {
            if !matches!(
                status.as_str(),
                "ACTIVE" | "RECOVERY" | "PASSWORD_EXPIRED" | "LOCKED_OUT"
            ) {
                return Err(error(
                    StatusCode::FORBIDDEN,
                    "E0000017",
                    "Reset password is not allowed in the user's current status.",
                    &[],
                ));
            }
            set_status(w, &mut user, "RECOVERY");
            let body = if send_email {
                json!({})
            } else {
                let t = token(w);
                json!({ "resetPasswordUrl": format!("{}/reset_password/{t}", crate::ORG_URL) })
            };
            (
                "user.account.reset_password",
                "Fired when the user's Okta password is reset",
                body,
            )
        }
        "expire_password" | "expire_password_with_temp_password" => {
            if !matches!(status.as_str(), "ACTIVE" | "RECOVERY" | "PASSWORD_EXPIRED") {
                return Err(error(
                    StatusCode::FORBIDDEN,
                    "E0000017",
                    "Expire password is not allowed in the user's current status.",
                    &[],
                ));
            }
            set_status(w, &mut user, "PASSWORD_EXPIRED");
            (
                "user.account.expire_password",
                "Expire user password",
                render(&user),
            )
        }
        "reset_factors" => {
            let uid = id_of(&user);
            for (k, _) in list(w, "factors") {
                if k.starts_with(&format!("{uid}/")) {
                    remove(w, "factors", &k);
                }
            }
            (
                "user.mfa.factor.reset_all",
                "Reset all factors for user",
                json!({}),
            )
        }
        _ => return Err(crate::util::endpoint_not_found()),
    };
    log_event(w, event, display, vec![user_target(&user)]);
    Ok(Reply::ok(reply))
}

/// Groups the user belongs to, in group dataset order.
pub fn user_groups(w: &mut WorldState, uid: &str) -> Vec<Value> {
    let members = list(w, "group_members");
    list(w, "groups")
        .into_iter()
        .filter(|(gid, _)| members.iter().any(|(k, _)| *k == format!("{gid}/{uid}")))
        .map(|(_, g)| g)
        .collect()
}

/// Apps assigned to the user directly or through a group.
pub fn assigned_app_ids(w: &mut WorldState, uid: &str) -> Vec<String> {
    let groups: Vec<String> = user_groups(w, uid).iter().map(id_of).collect();
    let direct = list(w, "app_users");
    let via_group = list(w, "app_groups");
    list(w, "apps")
        .into_iter()
        .filter(|(aid, _)| {
            direct.iter().any(|(k, _)| *k == format!("{aid}/{uid}"))
                || via_group
                    .iter()
                    .any(|(k, _)| groups.iter().any(|g| *k == format!("{aid}/{g}")))
        })
        .map(|(aid, _)| aid)
        .collect()
}

fn app_links(w: &mut WorldState, uid: &str) -> Vec<Value> {
    let ids = assigned_app_ids(w, uid);
    ids.iter()
        .enumerate()
        .filter_map(|(i, aid)| {
            let app = get(w, "apps", aid)?;
            (app["status"] == "ACTIVE").then(|| {
                json!({
                    "id": format!("00ub0oNGTSWTBKOLGLNR{i}"),
                    "label": app["label"],
                    "linkUrl": format!("{}/home/{}/{aid}/{}", crate::ORG_URL, app["name"].as_str().unwrap_or(""), "alnk"),
                    "logoUrl": "https://ok12static.oktacdn.com/assets/img/logos/default.6770228fb0dab49a1695ef440a5279bb.png",
                    "appName": app["name"],
                    "appInstanceId": aid,
                    "appAssignmentId": format!("0ua{}", &aid[3..]),
                    "credentialsSetup": false,
                    "hidden": false,
                    "sortOrder": i
                })
            })
        })
        .collect()
}
