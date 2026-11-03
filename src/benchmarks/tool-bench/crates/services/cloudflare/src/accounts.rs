//! `/user`, `/user/tokens/verify`, `/memberships`, `/accounts`, `/accounts/{id}`,
//! `/accounts/{id}/members`, `/accounts/{id}/tokens/verify`.

use mock_core::world::WorldState;
use serde_json::{Value, json};

use crate::api::{
    ApiRequest, ApiResult, account, fail, get, hex_id, method_not_allowed, no_route, ok, paged,
    put, remove, session_accounts, sort_by_query, str_field, values,
};

/// Roles an account member can hold (ids as in Cloudflare's role list).
const ROLES: &[(&str, &str, &str)] = &[
    (
        "33666b9c79b9a5273fc7344ff42f953d",
        "Super Administrator - All Privileges",
        "Can edit any Cloudflare setting, make purchases, update billing, and manage memberships. Super Administrators can revoke the access of other Super Administrators.",
    ),
    (
        "05784afa30c1afe1440e79d9351c7430",
        "Administrator",
        "Can access the full account, except for membership management and billing.",
    ),
    (
        "b1f81de8f5e4410f8d6e3b7a3c2e9a14",
        "Administrator Read Only",
        "Can access the full account in read-only mode.",
    ),
    (
        "3536bcfad5faccb999b47003c79917fb",
        "DNS",
        "Grants edit access to DNS settings for all zones in the account.",
    ),
    (
        "e58cefd75d7adae0b761796c28815e5c",
        "Workers Admin",
        "Can edit Workers, Workers KV, R2 and D1 resources.",
    ),
    (
        "e086da7e2179491d91ee5f35b3ca210a",
        "Billing",
        "Can edit the account's billing profile and subscriptions.",
    ),
];

pub fn role(id_or_name: &str) -> Option<Value> {
    ROLES
        .iter()
        .find(|(id, name, _)| *id == id_or_name || name.eq_ignore_ascii_case(id_or_name))
        .map(|(id, name, description)| {
            json!({ "id": id, "name": name, "description": description, "permissions": {} })
        })
}

pub fn route(w: &mut WorldState, req: &ApiRequest, segs: &[&str]) -> ApiResult {
    let m = req.method.as_str();
    match segs {
        ["user"] => match m {
            "GET" => ok(user(w)),
            _ => Err(method_not_allowed()),
        },
        ["user", "tokens", "verify"] | ["accounts", _, "tokens", "verify"] => {
            if let ["accounts", id, ..] = segs {
                account(w, req, id)?;
            }
            match m {
                "GET" => Ok(crate::api::ApiResponse::json(
                    200,
                    &json!({
                        "result": { "id": hex_id(w, &["token"]), "status": "active" },
                        "success": true,
                        "errors": [],
                        "messages": [{ "code": 10000, "message": "This API Token is valid and active", "type": null }]
                    }),
                )),
                _ => Err(method_not_allowed()),
            }
        }
        ["memberships"] => match m {
            "GET" => {
                let mut items: Vec<Value> = memberships(w)
                    .into_iter()
                    .filter(|ms| {
                        req.q("account.name")
                            .is_none_or(|n| str_field(&ms["account"], "name") == n)
                            && req.q("status").is_none_or(|s| str_field(ms, "status") == s)
                    })
                    .collect();
                sort_by_query(req, &mut items, &["id", "account.name", "status"]);
                paged(req, items, 20, 50)
            }
            _ => Err(method_not_allowed()),
        },
        ["memberships", id] => match m {
            "GET" => memberships(w)
                .into_iter()
                .find(|ms| str_field(ms, "id") == *id)
                .map_or_else(|| Err(membership_not_found()), ok),
            _ => Err(method_not_allowed()),
        },
        ["accounts"] => match m {
            "GET" => {
                let mut items: Vec<Value> = session_accounts(w)
                    .iter()
                    .filter(|a| req.q("name").is_none_or(|n| str_field(a, "name") == n))
                    .map(account_view)
                    .collect();
                if req.q("direction") == Some("desc") {
                    items.reverse();
                }
                paged(req, items, 20, 50)
            }
            _ => Err(method_not_allowed()),
        },
        ["accounts", id] => {
            let acct = account(w, req, id)?;
            match m {
                "GET" => ok(account_view(&acct)),
                _ => Err(method_not_allowed()),
            }
        }
        ["accounts", id, "members"] => {
            account(w, req, id)?;
            match m {
                "GET" => {
                    let mut items: Vec<Value> = members_of(w, id)
                        .into_iter()
                        .filter(|mem| {
                            req.q("status")
                                .is_none_or(|s| str_field(mem, "status") == s)
                        })
                        .map(|mem| member_view(&mem))
                        .collect();
                    sort_by_query(
                        req,
                        &mut items,
                        &["user.first_name", "user.last_name", "user.email", "status"],
                    );
                    paged(req, items, 20, 50)
                }
                "POST" => create_member(w, req, id),
                _ => Err(method_not_allowed()),
            }
        }
        ["accounts", id, "members", member_id] => {
            account(w, req, id)?;
            let Some(member) =
                get(w, "members", member_id).filter(|mem| str_field(mem, "account_id") == *id)
            else {
                return Err(member_not_found());
            };
            match m {
                "GET" => ok(member_view(&member)),
                "PUT" => update_member(w, req, member_id, member),
                "DELETE" => {
                    remove(w, "members", member_id);
                    ok(json!({ "id": member_id }))
                }
                _ => Err(method_not_allowed()),
            }
        }
        _ => Err(no_route(&req.path)),
    }
}

pub fn user(w: &WorldState) -> Value {
    let stored = get(w, "user", "user").unwrap_or_else(|| {
        json!({ "id": hex_id(w, &["user"]), "email": "ops@octo-corp.example", "first_name": null, "last_name": null })
    });
    let mut user = json!({
        "id": stored["id"],
        "email": stored["email"],
        "first_name": stored.get("first_name").cloned().unwrap_or(Value::Null),
        "last_name": stored.get("last_name").cloned().unwrap_or(Value::Null),
        "username": stored.get("username").cloned().unwrap_or_else(|| json!(hex_id(w, &["username"]))),
        "telephone": stored.get("telephone").cloned().unwrap_or(Value::Null),
        "country": stored.get("country").cloned().unwrap_or(Value::Null),
        "zipcode": stored.get("zipcode").cloned().unwrap_or(Value::Null),
        "two_factor_authentication_enabled": stored.get("two_factor_authentication_enabled").cloned().unwrap_or(json!(true)),
        "two_factor_authentication_locked": false,
        "created_on": stored.get("created_on").cloned().unwrap_or(json!("2019-04-02T16:11:20.112233Z")),
        "modified_on": stored.get("modified_on").cloned().unwrap_or(json!("2025-11-20T10:02:41.554100Z")),
        "organizations": [],
        "has_pro_zones": false,
        "has_business_zones": false,
        "has_enterprise_zones": false,
        "suspended": false,
        "betas": [],
    });
    if let Some(extra) = stored.as_object() {
        for (k, v) in extra {
            if user.get(k).is_none() {
                user[k] = v.clone();
            }
        }
    }
    user
}

pub fn account_view(a: &Value) -> Value {
    json!({
        "id": a["id"],
        "name": a["name"],
        "type": a.get("type").cloned().unwrap_or(json!("standard")),
        "settings": a.get("settings").cloned().unwrap_or_else(|| json!({
            "enforce_twofactor": false,
            "api_access_enabled": null,
            "access_approval_expiry": null,
            "abuse_contact_email": null,
            "use_account_custom_ns_by_default": false
        })),
        "legacy_flags": {
            "enterprise_zone_quota": { "maximum": 0, "current": 0, "available": 0 }
        },
        "created_on": a.get("created_on").cloned().unwrap_or(json!("2019-04-02T16:11:20.112233Z")),
    })
}

fn memberships(w: &WorldState) -> Vec<Value> {
    session_accounts(w)
        .iter()
        .map(|a| {
            let id = str_field(a, "id");
            json!({
                "id": hex_id(w, &["membership", id]),
                "account": account_view(a),
                "api_access_enabled": null,
                "status": "accepted",
                "permissions": {
                    "analytics": { "read": true, "write": true },
                    "billing": { "read": true, "write": true },
                    "dns": { "read": true, "write": true },
                    "organization": { "read": true, "write": true },
                    "workers": { "read": true, "write": true },
                    "zones": { "read": true, "write": true }
                },
                "roles": ["Super Administrator - All Privileges"],
            })
        })
        .collect()
}

fn members_of(w: &WorldState, account_id: &str) -> Vec<Value> {
    values(w, "members")
        .into_iter()
        .filter(|m| str_field(m, "account_id") == account_id)
        .collect()
}

pub fn member_view(m: &Value) -> Value {
    let mut out = m.clone();
    if let Some(o) = out.as_object_mut() {
        o.shift_remove("account_id");
        o.entry("policies").or_insert(json!([]));
    }
    out
}

fn create_member(w: &mut WorldState, req: &ApiRequest, account_id: &str) -> ApiResult {
    let body = req.json_object()?;
    let Some(email) = body
        .get("email")
        .and_then(Value::as_str)
        .filter(|e| e.contains('@'))
    else {
        return Err(fail(400, &[(1001, "email: must be a valid email address")]));
    };
    let email = email.to_lowercase();
    if members_of(w, account_id)
        .iter()
        .any(|m| str_field(&m["user"], "email").eq_ignore_ascii_case(&email))
    {
        return Err(fail(
            400,
            &[(1005, "User is already a member of this account")],
        ));
    }
    let roles = resolve_roles(body.get("roles"))?;
    let id = hex_id(w, &["member", account_id, &email]);
    let status = body
        .get("status")
        .and_then(Value::as_str)
        .filter(|s| *s == "accepted")
        .unwrap_or("pending");
    let member = json!({
        "id": id,
        "account_id": account_id,
        "user": {
            "id": hex_id(w, &["member-user", &email]),
            "first_name": null,
            "last_name": null,
            "email": email,
            "two_factor_authentication_enabled": false
        },
        "status": status,
        "roles": roles,
        "policies": [],
    });
    put(w, "members", &id, member.clone());
    ok(member_view(&member))
}

fn update_member(w: &mut WorldState, req: &ApiRequest, id: &str, mut member: Value) -> ApiResult {
    let body = req.json_object()?;
    if let Some(r) = body.get("roles") {
        member["roles"] = resolve_roles(Some(r))?;
    }
    if let Some(s) = body.get("status").and_then(Value::as_str) {
        member["status"] = json!(s);
    }
    put(w, "members", id, member.clone());
    ok(member_view(&member))
}

fn resolve_roles(roles: Option<&Value>) -> Result<Value, crate::api::ApiResponse> {
    let Some(Value::Array(list)) = roles else {
        return Err(fail(
            400,
            &[(1001, "roles: must provide at least one role")],
        ));
    };
    let mut out = Vec::new();
    for r in list {
        let id = r
            .as_str()
            .or_else(|| r.get("id").and_then(Value::as_str))
            .unwrap_or("");
        match role(id) {
            Some(role) => out.push(role),
            None => return Err(fail(400, &[(1001, &format!("Invalid role: {id}"))])),
        }
    }
    if out.is_empty() {
        return Err(fail(
            400,
            &[(1001, "roles: must provide at least one role")],
        ));
    }
    Ok(Value::Array(out))
}

fn member_not_found() -> crate::api::ApiResponse {
    fail(404, &[(1003, "Member not found")])
}

fn membership_not_found() -> crate::api::ApiResponse {
    fail(404, &[(1000, "Membership not found")])
}
