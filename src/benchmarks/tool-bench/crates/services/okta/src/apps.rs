//! `/api/v1/apps` (CRUD, lifecycle, user and group assignments) and the OIN catalog
//! (`/api/v1/catalog/apps`). Assignments: `app_users` keyed `<appId>/<userId>`,
//! `app_groups` keyed `<appId>/<groupId>`.

use axum::http::{Method, StatusCode};
use mock_core::world::WorldState;
use serde_json::{Value, json};

use crate::{
    filter,
    util::{
        Reply, Req, Res, app_target, bad_request, endpoint_not_found, error, get, group_target,
        href, id_of, list, log_event, new_id, not_found, page, post_href, put, remove, ts,
        user_target, validation,
    },
};

const SIGN_ON_MODES: [&str; 9] = [
    "BOOKMARK",
    "AUTO_LOGIN",
    "BASIC_AUTH",
    "BROWSER_PLUGIN",
    "OPENID_CONNECT",
    "SAML_1_1",
    "SAML_2_0",
    "SECURE_PASSWORD_STORE",
    "WS_FEDERATION",
];

pub fn handle(w: &mut WorldState, req: &Req) -> Res {
    let s: Vec<&str> = req.segs.iter().map(String::as_str).collect();
    let m = &req.method;
    match s.as_slice() {
        ["catalog", "apps"] if m == Method::GET => {
            let q = req.q("q").map(str::to_lowercase);
            let limit = req
                .q("limit")
                .and_then(|l| l.parse::<usize>().ok())
                .unwrap_or(20)
                .clamp(1, 200);
            let items: Vec<Value> = catalog(w)
                .into_iter()
                .filter(|c| {
                    q.as_ref().is_none_or(|q| {
                        c["name"]
                            .as_str()
                            .is_some_and(|n| n.to_lowercase().contains(q.as_str()))
                            || c["displayName"]
                                .as_str()
                                .is_some_and(|n| n.to_lowercase().contains(q.as_str()))
                    })
                })
                .collect();
            let start = req
                .q("after")
                .and_then(|a| items.iter().position(|c| c["name"] == a).map(|i| i + 1))
                .unwrap_or(0);
            let slice: Vec<Value> = items.into_iter().skip(start).take(limit).collect();
            Ok(Reply::ok(Value::Array(slice)))
        }
        ["catalog", "apps", name] if m == Method::GET => catalog(w)
            .into_iter()
            .find(|c| c["name"] == *name)
            .map(Reply::ok)
            .ok_or_else(|| not_found(name, "CatalogApplication")),
        ["apps"] if m == Method::GET => list_apps(w, req),
        ["apps"] if m == Method::POST => create_app(w, req),
        ["apps", id] if m == Method::GET => Ok(Reply::ok(render(&find(w, id)?))),
        ["apps", id] if m == Method::PUT => replace_app(w, req, id),
        ["apps", id] if m == Method::DELETE => {
            let app = find(w, id)?;
            if app["status"] == "ACTIVE" {
                return Err(error(
                    StatusCode::FORBIDDEN,
                    "E0000056",
                    "Delete application forbidden.",
                    &[],
                ));
            }
            remove(w, "apps", id);
            for coll in ["app_users", "app_groups"] {
                for (k, _) in list(w, coll) {
                    if k.starts_with(&format!("{id}/")) {
                        remove(w, coll, &k);
                    }
                }
            }
            log_event(
                w,
                "application.lifecycle.delete",
                "Delete application",
                vec![app_target(&app)],
            );
            Ok(Reply::no_content())
        }
        ["apps", id, "lifecycle", op] if m == Method::POST => {
            let mut app = find(w, id)?;
            let (status, event, display) = match *op {
                "activate" => (
                    "ACTIVE",
                    "application.lifecycle.activate",
                    "Activate application",
                ),
                "deactivate" => (
                    "INACTIVE",
                    "application.lifecycle.deactivate",
                    "Deactivate application",
                ),
                _ => return Err(endpoint_not_found()),
            };
            if app["status"] != status {
                app["status"] = json!(status);
                app["lastUpdated"] = json!(ts(w));
                put(w, "apps", id, app.clone());
                log_event(w, event, display, vec![app_target(&app)]);
            }
            Ok(Reply::ok(json!({})))
        }
        ["apps", id, "users"] if m == Method::GET => {
            find(w, id)?;
            let prefix = format!("{id}/");
            let items: Vec<Value> = list(w, "app_users")
                .into_iter()
                .filter(|(k, _)| k.starts_with(&prefix))
                .map(|(_, v)| render_app_user(id, &v))
                .collect();
            Ok(page(req, items, 50, 500, id_of))
        }
        ["apps", id, "users"] if m == Method::POST => {
            let app = find(w, id)?;
            let body = req.object()?;
            let uid = body
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            assign_user(w, &app, &uid, &Value::Object(body))
        }
        ["apps", id, "users", uid] if m == Method::GET => {
            find(w, id)?;
            get(w, "app_users", &format!("{id}/{uid}"))
                .map(|v| Reply::ok(render_app_user(id, &v)))
                .ok_or_else(|| not_found(uid, "AppUser"))
        }
        ["apps", id, "users", uid] if m == Method::POST => {
            let app = find(w, id)?;
            let key = format!("{id}/{uid}");
            let mut au = get(w, "app_users", &key).ok_or_else(|| not_found(uid, "AppUser"))?;
            crate::util::merge(&mut au, &Value::Object(req.object()?));
            au["id"] = json!(uid);
            au["lastUpdated"] = json!(ts(w));
            put(w, "app_users", &key, au.clone());
            let _ = app;
            Ok(Reply::ok(render_app_user(id, &au)))
        }
        ["apps", id, "users", uid] if m == Method::DELETE => {
            let app = find(w, id)?;
            let key = format!("{id}/{uid}");
            remove(w, "app_users", &key).ok_or_else(|| not_found(uid, "AppUser"))?;
            if let Some(u) = get(w, "users", uid) {
                log_event(
                    w,
                    "application.user_membership.remove",
                    "Remove user from application membership",
                    vec![user_target(&u), app_target(&app)],
                );
            }
            Ok(Reply::no_content())
        }
        ["apps", id, "groups"] if m == Method::GET => {
            find(w, id)?;
            let prefix = format!("{id}/");
            let items: Vec<Value> = list(w, "app_groups")
                .into_iter()
                .filter(|(k, _)| k.starts_with(&prefix))
                .map(|(_, v)| render_app_group(id, &v))
                .collect();
            Ok(page(req, items, 20, 200, id_of))
        }
        ["apps", id, "groups", gid] if m == Method::GET => {
            find(w, id)?;
            get(w, "app_groups", &format!("{id}/{gid}"))
                .map(|v| Reply::ok(render_app_group(id, &v)))
                .ok_or_else(|| not_found(gid, "ApplicationGroupAssignment"))
        }
        ["apps", id, "groups", gid] if m == Method::PUT => {
            let app = find(w, id)?;
            let g = crate::groups::find(w, gid)?;
            let body = req.object()?;
            let now = ts(w);
            let a = json!({
                "id": gid,
                "lastUpdated": now,
                "priority": body.get("priority").cloned().unwrap_or(json!(0)),
                "profile": body.get("profile").cloned().unwrap_or(json!({}))
            });
            put(w, "app_groups", &format!("{id}/{gid}"), a.clone());
            log_event(
                w,
                "application.user_membership.add",
                "Add assigned application to group",
                vec![group_target(&g), app_target(&app)],
            );
            Ok(Reply::ok(render_app_group(id, &a)))
        }
        ["apps", id, "groups", gid] if m == Method::DELETE => {
            let app = find(w, id)?;
            remove(w, "app_groups", &format!("{id}/{gid}"))
                .ok_or_else(|| not_found(gid, "ApplicationGroupAssignment"))?;
            if let Some(g) = get(w, "groups", gid) {
                log_event(
                    w,
                    "group.application_assignment.remove",
                    "Remove assigned application from group",
                    vec![group_target(&g), app_target(&app)],
                );
            }
            Ok(Reply::no_content())
        }
        _ => Err(endpoint_not_found()),
    }
}

pub fn find(w: &mut WorldState, id: &str) -> Result<Value, Reply> {
    get(w, "apps", id).ok_or_else(|| not_found(id, "AppInstance"))
}

pub fn render(app: &Value) -> Value {
    let mut out = app.clone();
    let id = id_of(app);
    let name = app["name"].as_str().unwrap_or("app");
    let lifecycle = if app["status"] == "ACTIVE" {
        "deactivate"
    } else {
        "activate"
    };
    let mut links = json!({
        "uploadLogo": post_href(&format!("/api/v1/apps/{id}/logo")),
        "appLinks": [{ "name": name, "href": format!("{}/home/{name}/{id}/alnk", crate::ORG_URL), "type": "text/html" }],
        "groups": href(&format!("/api/v1/apps/{id}/groups")),
        "logo": [{ "name": "medium", "href": "https://ok12static.oktacdn.com/assets/img/logos/default.6770228fb0dab49a1695ef440a5279bb.png", "type": "image/png" }],
        "users": href(&format!("/api/v1/apps/{id}/users")),
        "policies": href(&format!("/api/v1/apps/{id}/policies")),
    });
    links[lifecycle] = post_href(&format!("/api/v1/apps/{id}/lifecycle/{lifecycle}"));
    out["_links"] = links;
    out
}

fn render_app_user(app_id: &str, au: &Value) -> Value {
    let mut out = au.clone();
    let uid = id_of(au);
    out["_links"] = json!({
        "app": href(&format!("/api/v1/apps/{app_id}")),
        "user": href(&format!("/api/v1/users/{uid}"))
    });
    out
}

fn render_app_group(app_id: &str, ag: &Value) -> Value {
    let mut out = ag.clone();
    let gid = id_of(ag);
    out["_links"] = json!({
        "app": href(&format!("/api/v1/apps/{app_id}")),
        "group": href(&format!("/api/v1/groups/{gid}"))
    });
    out
}

fn assign_user(w: &mut WorldState, app: &Value, uid: &str, body: &Value) -> Res {
    let u = crate::users::find(w, uid)?;
    let uid = id_of(&u);
    let aid = id_of(app);
    let now = ts(w);
    let login = u["profile"]["login"].clone();
    let mut au = json!({
        "id": uid,
        "externalId": null,
        "created": now,
        "lastUpdated": now,
        "scope": "USER",
        "status": "PROVISIONED",
        "statusChanged": now,
        "passwordChanged": null,
        "syncState": "DISABLED",
        "credentials": { "userName": login },
        "profile": {}
    });
    crate::util::merge(&mut au, body);
    au["id"] = json!(uid);
    put(w, "app_users", &format!("{aid}/{uid}"), au.clone());
    log_event(
        w,
        "application.user_membership.add",
        "Add user to application membership",
        vec![user_target(&u), app_target(app)],
    );
    Ok(Reply::ok(render_app_user(&aid, &au)))
}

fn list_apps(w: &mut WorldState, req: &Req) -> Res {
    let q = req.q("q").map(str::to_lowercase);
    let expr = match req.q("filter") {
        Some(f) => Some(
            filter::parse(f).map_err(|_| bad_request("E0000031", "Invalid search criteria."))?,
        ),
        None => None,
    };
    // `user.id` / `group.id` filters match assignments rather than app fields.
    let users = list(w, "app_users");
    let groups = list(w, "app_groups");
    let mut via_user_groups = std::collections::BTreeMap::new();
    let members = list(w, "group_members");
    let mut items = Vec::new();
    for (aid, app) in list(w, "apps") {
        let mut doc = app.clone();
        let mut user_ids: Vec<Value> = users
            .iter()
            .filter_map(|(k, _)| k.strip_prefix(&format!("{aid}/")).map(|u| json!(u)))
            .collect();
        let group_ids: Vec<Value> = groups
            .iter()
            .filter_map(|(k, _)| k.strip_prefix(&format!("{aid}/")).map(|g| json!(g)))
            .collect();
        for g in &group_ids {
            let g = g.as_str().unwrap_or_default();
            for (k, _) in &members {
                if let Some(u) = k.strip_prefix(&format!("{g}/")) {
                    user_ids.push(json!(u));
                    via_user_groups.insert(u.to_owned(), true);
                }
            }
        }
        doc["user"] = json!({ "id": user_ids });
        doc["group"] = json!({ "id": group_ids });
        if expr.as_ref().is_some_and(|e| !e.matches(&doc)) {
            continue;
        }
        if let Some(q) = &q {
            let hit = ["name", "label"].iter().any(|f| {
                app[f]
                    .as_str()
                    .is_some_and(|v| v.to_lowercase().starts_with(q.as_str()))
            });
            if !hit {
                continue;
            }
        }
        items.push(render(&app));
    }
    Ok(page(req, items, 20, 200, id_of))
}

fn custom_name(w: &mut WorldState, label: &str) -> String {
    let slug: String = label
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let n = crate::util::svc(w).ids.next_int("app_name", 1);
    format!("octocorp_{}_{n}", slug.trim_matches('_'))
}

fn create_app(w: &mut WorldState, req: &Req) -> Res {
    let mut body = Value::Object(req.object()?);
    let mode = body["signOnMode"].as_str().unwrap_or_default().to_owned();
    let mut causes = Vec::new();
    if body["label"].as_str().is_none_or(str::is_empty) {
        causes.push("label: The field cannot be left blank");
    }
    if !SIGN_ON_MODES.contains(&mode.as_str()) {
        causes.push("signOnMode: The field cannot be left blank");
    }
    if !causes.is_empty() {
        return Err(validation(&causes));
    }
    let label = body["label"].as_str().unwrap_or_default().to_owned();
    if let Some(name) = body["name"].as_str().map(str::to_owned) {
        let known = catalog(w).iter().any(|c| c["name"] == name.as_str())
            || matches!(
                name.as_str(),
                "bookmark"
                    | "oidc_client"
                    | "template_basic_auth"
                    | "template_swa"
                    | "template_sps"
            );
        if !known {
            return Err(validation(&["name: The application name is not valid"]));
        }
    } else {
        let name = match mode.as_str() {
            "BOOKMARK" => "bookmark".to_owned(),
            "OPENID_CONNECT" => "oidc_client".to_owned(),
            "BASIC_AUTH" => "template_basic_auth".to_owned(),
            "BROWSER_PLUGIN" => "template_swa".to_owned(),
            "SECURE_PASSWORD_STORE" => "template_sps".to_owned(),
            _ => custom_name(w, &label),
        };
        body["name"] = json!(name);
    }
    let activate = req.q_bool("activate", true);
    let now = ts(w);
    let id = new_id(w, "app", "0oa");
    body["id"] = json!(id);
    body["status"] = json!(if activate { "ACTIVE" } else { "INACTIVE" });
    body["created"] = json!(now);
    body["lastUpdated"] = json!(now);
    if body.get("features").is_none() {
        body["features"] = json!([]);
    }
    if body.get("accessibility").is_none() {
        body["accessibility"] =
            json!({ "selfService": false, "errorRedirectUrl": null, "loginRedirectUrl": null });
    }
    if body.get("visibility").is_none() {
        body["visibility"] = json!({ "autoSubmitToolbar": false, "hide": { "iOS": false, "web": false }, "appLinks": { body["name"].as_str().unwrap_or("app"): true } });
    }
    if body.get("credentials").is_none() {
        body["credentials"] = json!({ "userNameTemplate": { "template": "${source.login}", "type": "BUILT_IN" }, "signing": {} });
    }
    if mode == "OPENID_CONNECT" && body["credentials"].get("oauthClient").is_none() {
        body["credentials"]["oauthClient"] = json!({ "autoKeyRotation": true, "client_id": id, "token_endpoint_auth_method": "client_secret_basic" });
    }
    if body.get("settings").is_none() {
        body["settings"] = json!({ "app": {} });
    }
    put(w, "apps", &id, body.clone());
    log_event(
        w,
        "application.lifecycle.create",
        "Add application",
        vec![app_target(&body)],
    );
    if activate {
        log_event(
            w,
            "application.lifecycle.activate",
            "Activate application",
            vec![app_target(&body)],
        );
    }
    Ok(Reply::ok(render(&body)))
}

fn replace_app(w: &mut WorldState, req: &Req, id: &str) -> Res {
    let old = find(w, id)?;
    let mut body = Value::Object(req.object()?);
    if body["label"].as_str().is_none_or(str::is_empty) {
        return Err(validation(&["label: The field cannot be left blank"]));
    }
    if let Some(mode) = body["signOnMode"].as_str()
        && mode != old["signOnMode"].as_str().unwrap_or_default()
    {
        return Err(validation(&[
            "signOnMode: The sign on mode cannot be changed",
        ]));
    }
    for k in ["id", "name", "status", "created", "signOnMode"] {
        body[k] = old[k].clone();
    }
    for k in [
        "features",
        "accessibility",
        "visibility",
        "credentials",
        "settings",
    ] {
        if body.get(k).is_none_or(Value::is_null) {
            body[k] = old[k].clone();
        }
    }
    body["lastUpdated"] = json!(ts(w));
    put(w, "apps", id, body.clone());
    log_event(
        w,
        "application.lifecycle.update",
        "Update application",
        vec![app_target(&body)],
    );
    Ok(Reply::ok(render(&body)))
}

/// The OIN catalog: the dataset's `catalog` collection, or a small built-in one.
fn catalog(w: &mut WorldState) -> Vec<Value> {
    let from_dataset: Vec<Value> = list(w, "catalog").into_iter().map(|(_, v)| v).collect();
    if !from_dataset.is_empty() {
        return from_dataset;
    }
    let entry = |name: &str, display: &str, modes: &[&str], features: &[&str]| {
        json!({
            "name": name,
            "displayName": display,
            "description": format!("{display} integration from the Okta Integration Network."),
            "signOnModes": modes,
            "features": features,
            "verificationStatus": "OKTA_VERIFIED",
            "lastUpdated": "2025-11-04T18:22:10.000Z",
            "website": null,
            "_links": { "logo": [{ "name": "medium", "href": "https://ok12static.oktacdn.com/assets/img/logos/default.6770228fb0dab49a1695ef440a5279bb.png", "type": "image/png" }] }
        })
    };
    vec![
        entry(
            "atlassian",
            "Atlassian Cloud",
            &["SAML_2_0"],
            &[
                "PUSH_NEW_USERS",
                "PUSH_USER_DEACTIVATION",
                "PUSH_PROFILE_UPDATES",
                "GROUP_PUSH",
            ],
        ),
        entry(
            "aws_account_federation",
            "AWS Account Federation",
            &["SAML_2_0"],
            &[],
        ),
        entry(
            "box",
            "Box",
            &["SAML_2_0"],
            &[
                "PUSH_NEW_USERS",
                "PUSH_USER_DEACTIVATION",
                "PUSH_PROFILE_UPDATES",
            ],
        ),
        entry(
            "github_enterprise_cloud",
            "GitHub Enterprise Cloud - Enterprise Accounts",
            &["SAML_2_0"],
            &["PUSH_NEW_USERS", "PUSH_USER_DEACTIVATION"],
        ),
        entry(
            "google",
            "Google Workspace",
            &["SAML_2_0", "BROWSER_PLUGIN"],
            &[
                "PUSH_NEW_USERS",
                "PUSH_USER_DEACTIVATION",
                "PUSH_PROFILE_UPDATES",
                "GROUP_PUSH",
                "IMPORT_NEW_USERS",
            ],
        ),
        entry(
            "salesforce",
            "Salesforce.com",
            &["SAML_2_0", "BROWSER_PLUGIN"],
            &[
                "PUSH_NEW_USERS",
                "PUSH_USER_DEACTIVATION",
                "PUSH_PROFILE_UPDATES",
                "IMPORT_NEW_USERS",
            ],
        ),
        entry(
            "scim2testapp",
            "SCIM 2.0 Test App (OAuth Bearer Token)",
            &["SAML_2_0", "BROWSER_PLUGIN"],
            &[
                "PUSH_NEW_USERS",
                "PUSH_USER_DEACTIVATION",
                "PUSH_PROFILE_UPDATES",
                "GROUP_PUSH",
                "IMPORT_NEW_USERS",
            ],
        ),
        entry(
            "slack",
            "Slack",
            &["SAML_2_0"],
            &[
                "PUSH_NEW_USERS",
                "PUSH_USER_DEACTIVATION",
                "PUSH_PROFILE_UPDATES",
                "GROUP_PUSH",
            ],
        ),
        entry(
            "zoomus",
            "Zoom",
            &["SAML_2_0"],
            &[
                "PUSH_NEW_USERS",
                "PUSH_USER_DEACTIVATION",
                "PUSH_PROFILE_UPDATES",
            ],
        ),
    ]
}
