//! `/api/v1/groups`: CRUD, membership, assigned apps, and group rules.
//! Membership lives in `group_members` keyed `<groupId>/<userId>`.

use axum::http::{Method, StatusCode};
use mock_core::world::WorldState;
use serde_json::{Value, json};

use crate::{
    apps, filter,
    util::{
        Reply, Req, Res, bad_request, endpoint_not_found, error, get, group_target, href, id_of,
        list, log_event, new_id, not_found, page, put, remove, ts, user_target, validation,
    },
};

pub fn handle(w: &mut WorldState, req: &Req) -> Res {
    let s: Vec<&str> = req.segs.iter().map(String::as_str).collect();
    let m = &req.method;
    match s.as_slice() {
        ["groups", "rules", ..] => rules(w, req, &s[2..]),
        ["groups"] if m == Method::GET => list_groups(w, req),
        ["groups"] if m == Method::POST => create_group(w, req),
        ["groups", id] if m == Method::GET => Ok(Reply::ok(render_group(&find(w, id)?))),
        ["groups", id] if m == Method::PUT => update_group(w, req, id),
        ["groups", id] if m == Method::DELETE => delete_group(w, id),
        ["groups", id, "users"] if m == Method::GET => {
            find(w, id)?;
            let prefix = format!("{id}/");
            let members: Vec<String> = list(w, "group_members")
                .into_iter()
                .filter_map(|(k, _)| k.strip_prefix(&prefix).map(str::to_owned))
                .collect();
            let users: Vec<Value> = list(w, "users")
                .into_iter()
                .filter(|(uid, _)| members.contains(uid))
                .map(|(_, u)| crate::users::render(&u))
                .collect();
            Ok(page(req, users, 1000, 1000, id_of))
        }
        ["groups", gid, "users", uid] if m == Method::PUT => {
            let g = find(w, gid)?;
            let u = crate::users::find(w, uid)?;
            if g["type"] != "OKTA_GROUP" {
                return Err(error(
                    StatusCode::BAD_REQUEST,
                    "E0000060",
                    "Unsupported operation.",
                    &[],
                ));
            }
            let uid = id_of(&u);
            if get(w, "group_members", &format!("{gid}/{uid}")).is_none() {
                add_member(w, gid, &uid);
                log_event(
                    w,
                    "group.user_membership.add",
                    "Add user to group membership",
                    vec![user_target(&u), group_target(&g)],
                );
            }
            Ok(Reply::no_content())
        }
        ["groups", gid, "users", uid] if m == Method::DELETE => {
            let g = find(w, gid)?;
            let u = crate::users::find(w, uid)?;
            if g["type"] != "OKTA_GROUP" {
                return Err(error(
                    StatusCode::BAD_REQUEST,
                    "E0000060",
                    "Unsupported operation.",
                    &[],
                ));
            }
            let uid = id_of(&u);
            if remove(w, "group_members", &format!("{gid}/{uid}")).is_some() {
                touch_membership(w, gid);
                log_event(
                    w,
                    "group.user_membership.remove",
                    "Remove user from group membership",
                    vec![user_target(&u), group_target(&g)],
                );
            }
            Ok(Reply::no_content())
        }
        ["groups", gid, "apps"] if m == Method::GET => {
            find(w, gid)?;
            let suffix = format!("/{gid}");
            let assigned: Vec<String> = list(w, "app_groups")
                .into_iter()
                .filter_map(|(k, _)| k.strip_suffix(&suffix).map(str::to_owned))
                .collect();
            let apps: Vec<Value> = list(w, "apps")
                .into_iter()
                .filter(|(aid, _)| assigned.contains(aid))
                .map(|(_, a)| apps::render(&a))
                .collect();
            Ok(page(req, apps, 20, 200, id_of))
        }
        _ => Err(endpoint_not_found()),
    }
}

pub fn find(w: &mut WorldState, id: &str) -> Result<Value, Reply> {
    get(w, "groups", id).ok_or_else(|| not_found(id, "UserGroup"))
}

pub fn render_group(g: &Value) -> Value {
    let mut out = g.clone();
    let id = id_of(g);
    out["_links"] = json!({
        "logo": [
            { "name": "medium", "href": "https://ok12static.oktacdn.com/assets/img/logos/groups/odyssey/okta-medium.30ce6d4085dff29412984e4c191bc874.png", "type": "image/png" },
            { "name": "large", "href": "https://ok12static.oktacdn.com/assets/img/logos/groups/odyssey/okta-large.c3cb8cda8ae0add1b4fe928f5844dbe3.png", "type": "image/png" }
        ],
        "users": href(&format!("/api/v1/groups/{id}/users")),
        "apps": href(&format!("/api/v1/groups/{id}/apps"))
    });
    out
}

pub fn add_member(w: &mut WorldState, gid: &str, uid: &str) {
    put(
        w,
        "group_members",
        &format!("{gid}/{uid}"),
        json!({ "groupId": gid, "userId": uid }),
    );
    touch_membership(w, gid);
}

fn touch_membership(w: &mut WorldState, gid: &str) {
    let now = ts(w);
    if let Some(mut g) = get(w, "groups", gid) {
        g["lastMembershipUpdated"] = json!(now);
        put(w, "groups", gid, g);
    }
}

fn list_groups(w: &mut WorldState, req: &Req) -> Res {
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
    let expand_stats = req.q("expand").is_some_and(|e| e.contains("stats"));
    let members = list(w, "group_members");
    let mut groups: Vec<Value> = list(w, "groups")
        .into_iter()
        .map(|(_, g)| g)
        .filter(|g| exprs.iter().all(|e| e.matches(g)))
        .filter(|g| {
            q.as_ref().is_none_or(|q| {
                g["profile"]["name"]
                    .as_str()
                    .is_some_and(|n| n.to_lowercase().starts_with(q.as_str()))
            })
        })
        .map(|g| {
            let mut r = render_group(&g);
            if expand_stats {
                let gid = id_of(&g);
                let count = members
                    .iter()
                    .filter(|(k, _)| k.starts_with(&format!("{gid}/")))
                    .count();
                r["_embedded"] = json!({ "stats": { "usersCount": count, "appsCount": 0, "groupPushMappingsCount": 0, "hasAdminPrivilege": false } });
            }
            r
        })
        .collect();
    if req.q("sortBy").is_some() {
        groups.sort_by(|a, b| {
            a["profile"]["name"]
                .as_str()
                .cmp(&b["profile"]["name"].as_str())
        });
        if req
            .q("sortOrder")
            .is_some_and(|o| o.eq_ignore_ascii_case("desc"))
        {
            groups.reverse();
        }
    }
    Ok(page(req, groups, 10000, 10000, id_of))
}

fn check_name(w: &mut WorldState, name: Option<&str>, self_id: Option<&str>) -> Result<(), Reply> {
    let Some(name) = name.filter(|n| !n.trim().is_empty()) else {
        return Err(validation(&["name: The field cannot be left blank"]));
    };
    if list(w, "groups").iter().any(|(k, g)| {
        Some(k.as_str()) != self_id
            && g["profile"]["name"]
                .as_str()
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
    }) {
        return Err(validation(&[
            "name: An object with this field already exists in the current organization",
        ]));
    }
    Ok(())
}

fn create_group(w: &mut WorldState, req: &Req) -> Res {
    let body = req.object()?;
    let profile = body.get("profile").cloned().unwrap_or(json!({}));
    check_name(w, profile["name"].as_str(), None)?;
    let now = ts(w);
    let id = new_id(w, "group", "00g");
    let mut profile = profile;
    if profile.get("description").is_none() {
        profile["description"] = Value::Null;
    }
    let group = json!({
        "id": id,
        "created": now,
        "lastUpdated": now,
        "lastMembershipUpdated": now,
        "objectClass": ["okta:user_group"],
        "type": "OKTA_GROUP",
        "profile": profile
    });
    put(w, "groups", &id, group.clone());
    log_event(
        w,
        "group.lifecycle.create",
        "Create okta user group",
        vec![group_target(&group)],
    );
    Ok(Reply::ok(render_group(&group)))
}

fn update_group(w: &mut WorldState, req: &Req, id: &str) -> Res {
    let mut g = find(w, id)?;
    if g["type"] != "OKTA_GROUP" {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "E0000060",
            "Unsupported operation.",
            &[],
        ));
    }
    let body = req.object()?;
    let profile = body.get("profile").cloned().unwrap_or(json!({}));
    check_name(w, profile["name"].as_str(), Some(id))?;
    g["profile"] = profile;
    g["lastUpdated"] = json!(ts(w));
    put(w, "groups", id, g.clone());
    log_event(
        w,
        "group.profile.update",
        "Update group profile",
        vec![group_target(&g)],
    );
    Ok(Reply::ok(render_group(&g)))
}

fn delete_group(w: &mut WorldState, id: &str) -> Res {
    let g = find(w, id)?;
    if g["type"] == "BUILT_IN" {
        return Err(error(
            StatusCode::FORBIDDEN,
            "E0000006",
            "You do not have permission to perform the requested action",
            &[],
        ));
    }
    remove(w, "groups", id);
    for coll in ["group_members"] {
        for (k, _) in list(w, coll) {
            if k.starts_with(&format!("{id}/")) {
                remove(w, coll, &k);
            }
        }
    }
    for (k, _) in list(w, "app_groups") {
        if k.ends_with(&format!("/{id}")) {
            remove(w, "app_groups", &k);
        }
    }
    log_event(
        w,
        "group.lifecycle.delete",
        "Delete Okta group",
        vec![group_target(&g)],
    );
    Ok(Reply::no_content())
}

fn render_rule(r: &Value) -> Value {
    r.clone()
}

fn rules(w: &mut WorldState, req: &Req, rest: &[&str]) -> Res {
    let m = &req.method;
    match rest {
        [] if m == Method::GET => {
            let q = req.q("search").map(str::to_lowercase);
            let items: Vec<Value> = list(w, "group_rules")
                .into_iter()
                .map(|(_, r)| r)
                .filter(|r| {
                    q.as_ref().is_none_or(|q| {
                        r["name"]
                            .as_str()
                            .is_some_and(|n| n.to_lowercase().contains(q.as_str()))
                    })
                })
                .map(|r| render_rule(&r))
                .collect();
            Ok(page(req, items, 50, 200, id_of))
        }
        [] if m == Method::POST => {
            let mut body = Value::Object(req.object()?);
            if body["name"].as_str().is_none_or(str::is_empty) {
                return Err(validation(&["name: The field cannot be left blank"]));
            }
            let now = ts(w);
            let id = new_id(w, "group_rule", "0pr");
            body["id"] = json!(id);
            body["type"] = json!("group_rule");
            body["status"] = json!("INACTIVE");
            body["created"] = json!(now);
            body["lastUpdated"] = json!(now);
            put(w, "group_rules", &id, body.clone());
            Ok(Reply::ok(render_rule(&body)))
        }
        [id] if m == Method::GET => get(w, "group_rules", id)
            .map(|r| Reply::ok(render_rule(&r)))
            .ok_or_else(|| not_found(id, "GroupRule")),
        [id] if m == Method::PUT => {
            let old = get(w, "group_rules", id).ok_or_else(|| not_found(id, "GroupRule"))?;
            if old["status"] == "ACTIVE" {
                return Err(bad_request(
                    "E0000090",
                    "Cannot update a rule that is active",
                ));
            }
            let mut body = Value::Object(req.object()?);
            for k in ["id", "type", "status", "created"] {
                body[k] = old[k].clone();
            }
            body["lastUpdated"] = json!(ts(w));
            put(w, "group_rules", id, body.clone());
            Ok(Reply::ok(render_rule(&body)))
        }
        [id] if m == Method::DELETE => {
            let old = get(w, "group_rules", id).ok_or_else(|| not_found(id, "GroupRule"))?;
            if old["status"] == "ACTIVE" {
                return Err(bad_request(
                    "E0000090",
                    "Cannot delete a rule that is active",
                ));
            }
            remove(w, "group_rules", id);
            Ok(Reply::no_content())
        }
        [id, "lifecycle", op] if m == Method::POST => {
            let mut r = get(w, "group_rules", id).ok_or_else(|| not_found(id, "GroupRule"))?;
            r["status"] = json!(match *op {
                "activate" => "ACTIVE",
                "deactivate" => "INACTIVE",
                _ => return Err(endpoint_not_found()),
            });
            r["lastUpdated"] = json!(ts(w));
            put(w, "group_rules", id, r);
            Ok(Reply::no_content())
        }
        _ => Err(endpoint_not_found()),
    }
}
