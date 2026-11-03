//! `/api/v1/policies`: policies and their rules, with lifecycle. Rules live in
//! `policy_rules` keyed `<policyId>/<ruleId>`.

use axum::http::{Method, StatusCode};
use mock_core::world::WorldState;
use serde_json::{Value, json};

use crate::util::{
    Reply, Req, Res, endpoint_not_found, error, get, href, id_of, list, log_event, new_id,
    not_found, page, post_href, put, remove, ts, validation,
};

const POLICY_TYPES: [&str; 10] = [
    "OKTA_SIGN_ON",
    "PASSWORD",
    "MFA_ENROLL",
    "IDP_DISCOVERY",
    "ACCESS_POLICY",
    "PROFILE_ENROLLMENT",
    "POST_AUTH_SESSION",
    "ENTITY_RISK",
    "DEVICE_SIGNAL_COLLECTION",
    "CONTINUOUS_ACCESS",
];

pub fn handle(w: &mut WorldState, req: &Req) -> Res {
    let s: Vec<&str> = req.segs.iter().map(String::as_str).collect();
    let m = &req.method;
    match s.as_slice() {
        ["policies"] if m == Method::GET => list_policies(w, req),
        ["policies"] if m == Method::POST => create_policy(w, req),
        ["policies", id] if m == Method::GET => Ok(Reply::ok(render(&find(w, id)?))),
        ["policies", id] if m == Method::PUT => replace_policy(w, req, id),
        ["policies", id] if m == Method::DELETE => {
            let p = find(w, id)?;
            if p["system"] == true {
                return Err(system_error());
            }
            remove(w, "policies", id);
            for (k, _) in list(w, "policy_rules") {
                if k.starts_with(&format!("{id}/")) {
                    remove(w, "policy_rules", &k);
                }
            }
            log_event(
                w,
                "policy.lifecycle.delete",
                "Delete policy",
                vec![target(&p)],
            );
            Ok(Reply::no_content())
        }
        ["policies", id, "lifecycle", op] if m == Method::POST => {
            let mut p = find(w, id)?;
            let status = match *op {
                "activate" => "ACTIVE",
                "deactivate" => {
                    if p["system"] == true {
                        return Err(system_error());
                    }
                    "INACTIVE"
                }
                _ => return Err(endpoint_not_found()),
            };
            p["status"] = json!(status);
            p["lastUpdated"] = json!(ts(w));
            put(w, "policies", id, p.clone());
            log_event(
                w,
                &format!("policy.lifecycle.{op}"),
                &format!("{} policy", cap(op)),
                vec![target(&p)],
            );
            Ok(Reply::no_content())
        }
        ["policies", id, "rules"] if m == Method::GET => {
            find(w, id)?;
            let rules = rules_of(w, id);
            let rules: Vec<Value> = rules.iter().map(|r| render_rule(id, r)).collect();
            Ok(page(req, rules, 1000, 1000, id_of))
        }
        ["policies", id, "rules"] if m == Method::POST => create_rule(w, req, id),
        ["policies", id, "rules", rid] if m == Method::GET => {
            find(w, id)?;
            Ok(Reply::ok(render_rule(id, &find_rule(w, id, rid)?)))
        }
        ["policies", id, "rules", rid] if m == Method::PUT => replace_rule(w, req, id, rid),
        ["policies", id, "rules", rid] if m == Method::DELETE => {
            let p = find(w, id)?;
            let r = find_rule(w, id, rid)?;
            if r["system"] == true {
                return Err(system_error());
            }
            remove(w, "policy_rules", &format!("{id}/{rid}"));
            log_event(
                w,
                "policy.rule.delete",
                "Delete policy rule",
                vec![rule_target(&r), target(&p)],
            );
            Ok(Reply::no_content())
        }
        ["policies", id, "rules", rid, "lifecycle", op] if m == Method::POST => {
            let p = find(w, id)?;
            let mut r = find_rule(w, id, rid)?;
            let status = match *op {
                "activate" => "ACTIVE",
                "deactivate" => {
                    if r["system"] == true {
                        return Err(system_error());
                    }
                    "INACTIVE"
                }
                _ => return Err(endpoint_not_found()),
            };
            r["status"] = json!(status);
            r["lastUpdated"] = json!(ts(w));
            put(w, "policy_rules", &format!("{id}/{rid}"), r.clone());
            log_event(
                w,
                &format!("policy.rule.{op}"),
                &format!("{} policy rule", cap(op)),
                vec![rule_target(&r), target(&p)],
            );
            Ok(Reply::no_content())
        }
        _ => Err(endpoint_not_found()),
    }
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

fn system_error() -> Reply {
    error(
        StatusCode::BAD_REQUEST,
        "E0000001",
        "Api validation failed: system",
        &["system: The default policy or rule cannot be modified in this way.".to_owned()],
    )
}

fn target(p: &Value) -> Value {
    json!({ "id": p["id"], "type": "PolicyEntity", "alternateId": "unknown", "displayName": p["name"], "detailEntry": { "policyType": p["type"] } })
}

fn rule_target(r: &Value) -> Value {
    json!({ "id": r["id"], "type": "PolicyRule", "alternateId": "unknown", "displayName": r["name"], "detailEntry": { "policyRuleFactorMode": null } })
}

pub fn find(w: &mut WorldState, id: &str) -> Result<Value, Reply> {
    get(w, "policies", id).ok_or_else(|| not_found(id, "Policy"))
}

fn find_rule(w: &mut WorldState, pid: &str, rid: &str) -> Result<Value, Reply> {
    get(w, "policy_rules", &format!("{pid}/{rid}")).ok_or_else(|| not_found(rid, "PolicyRule"))
}

fn rules_of(w: &mut WorldState, pid: &str) -> Vec<Value> {
    let prefix = format!("{pid}/");
    let mut rules: Vec<Value> = list(w, "policy_rules")
        .into_iter()
        .filter(|(k, _)| k.starts_with(&prefix))
        .map(|(_, r)| r)
        .collect();
    rules.sort_by_key(|r| r["priority"].as_i64().unwrap_or(i64::MAX));
    rules
}

pub fn render(p: &Value) -> Value {
    let mut out = p.clone();
    let id = id_of(p);
    let op = if p["status"] == "ACTIVE" {
        "deactivate"
    } else {
        "activate"
    };
    let mut links = json!({
        "self": href(&format!("/api/v1/policies/{id}")),
        "rules": href(&format!("/api/v1/policies/{id}/rules")),
    });
    links[op] = post_href(&format!("/api/v1/policies/{id}/lifecycle/{op}"));
    out["_links"] = links;
    out
}

fn render_rule(pid: &str, r: &Value) -> Value {
    let mut out = r.clone();
    let id = id_of(r);
    let op = if r["status"] == "ACTIVE" {
        "deactivate"
    } else {
        "activate"
    };
    let mut links = json!({ "self": href(&format!("/api/v1/policies/{pid}/rules/{id}")) });
    links[op] = post_href(&format!("/api/v1/policies/{pid}/rules/{id}/lifecycle/{op}"));
    out["_links"] = links;
    out
}

fn list_policies(w: &mut WorldState, req: &Req) -> Res {
    let Some(kind) = req.q("type") else {
        return Err(validation(&["type: The field cannot be left blank"]));
    };
    if !POLICY_TYPES.contains(&kind) {
        return Err(validation(&["type: Invalid policy type"]));
    }
    let q = req.q("q").map(str::to_lowercase);
    let status = req.q("status");
    let mut items: Vec<Value> = list(w, "policies")
        .into_iter()
        .map(|(_, p)| p)
        .filter(|p| p["type"] == kind)
        .filter(|p| status.is_none_or(|s| p["status"] == s))
        .filter(|p| {
            q.as_ref().is_none_or(|q| {
                p["name"]
                    .as_str()
                    .is_some_and(|n| n.to_lowercase().contains(q.as_str()))
            })
        })
        .collect();
    items.sort_by_key(|p| p["priority"].as_i64().unwrap_or(i64::MAX));
    let items: Vec<Value> = items.iter().map(render).collect();
    Ok(page(req, items, 20, 200, id_of))
}

fn next_priority(existing: &[Value]) -> i64 {
    existing
        .iter()
        .filter(|p| p["system"] != true)
        .filter_map(|p| p["priority"].as_i64())
        .max()
        .unwrap_or(0)
        + 1
}

/// Inserts at `priority`, pushing the system default (always last) down.
fn shift_system(w: &mut WorldState, coll: &str, keys: &[(String, Value)], priority: i64) {
    for (k, v) in keys {
        if v["system"] == true && v["priority"].as_i64().unwrap_or(0) <= priority {
            let mut v = v.clone();
            v["priority"] = json!(priority + 1);
            put(w, coll, k, v);
        }
    }
}

fn create_policy(w: &mut WorldState, req: &Req) -> Res {
    let mut body = Value::Object(req.object()?);
    let mut causes = Vec::new();
    let kind = body["type"].as_str().unwrap_or_default().to_owned();
    if !POLICY_TYPES.contains(&kind.as_str()) {
        causes.push("type: The field cannot be left blank");
    }
    if body["name"].as_str().is_none_or(str::is_empty) {
        causes.push("name: The field cannot be left blank");
    }
    if !causes.is_empty() {
        return Err(validation(&causes));
    }
    let same_type: Vec<(String, Value)> = list(w, "policies")
        .into_iter()
        .filter(|(_, p)| p["type"] == kind.as_str())
        .collect();
    let priority = body["priority"].as_i64().unwrap_or_else(|| {
        next_priority(&same_type.iter().map(|(_, v)| v.clone()).collect::<Vec<_>>())
    });
    shift_system(w, "policies", &same_type, priority);
    let now = ts(w);
    let id = new_id(w, "policy", "00p");
    let activate = req.q_bool("activate", true);
    body["id"] = json!(id);
    body["status"] = json!(if activate { "ACTIVE" } else { "INACTIVE" });
    body["priority"] = json!(priority);
    body["system"] = json!(false);
    body["created"] = json!(now);
    body["lastUpdated"] = json!(now);
    if body.get("description").is_none() {
        body["description"] = Value::Null;
    }
    if body.get("conditions").is_none() {
        body["conditions"] = Value::Null;
    }
    put(w, "policies", &id, body.clone());
    log_event(
        w,
        "policy.lifecycle.create",
        "Create policy",
        vec![target(&body)],
    );
    Ok(Reply::ok(render(&body)))
}

fn replace_policy(w: &mut WorldState, req: &Req, id: &str) -> Res {
    let old = find(w, id)?;
    let mut body = Value::Object(req.object()?);
    if body["name"].as_str().is_none_or(str::is_empty) {
        return Err(validation(&["name: The field cannot be left blank"]));
    }
    if body["type"]
        .as_str()
        .is_some_and(|t| t != old["type"].as_str().unwrap_or_default())
    {
        return Err(validation(&["type: The policy type cannot be changed"]));
    }
    for k in ["id", "type", "system", "created", "status"] {
        body[k] = old[k].clone();
    }
    // Okta keeps the policy's conditions and settings when a replace omits them.
    for k in ["priority", "conditions", "settings", "description"] {
        if body.get(k).is_none() {
            body[k] = old[k].clone();
        }
    }
    body["lastUpdated"] = json!(ts(w));
    put(w, "policies", id, body.clone());
    log_event(
        w,
        "policy.lifecycle.update",
        "Update policy",
        vec![target(&body)],
    );
    Ok(Reply::ok(render(&body)))
}

fn rule_type_for(policy_type: &str) -> &'static str {
    match policy_type {
        "OKTA_SIGN_ON" => "SIGN_ON",
        "PASSWORD" => "PASSWORD",
        "MFA_ENROLL" => "MFA_ENROLL",
        "IDP_DISCOVERY" => "IDP_DISCOVERY",
        "PROFILE_ENROLLMENT" => "PROFILE_ENROLLMENT",
        "POST_AUTH_SESSION" => "POST_AUTH_SESSION",
        "ENTITY_RISK" => "ENTITY_RISK",
        "DEVICE_SIGNAL_COLLECTION" => "DEVICE_SIGNAL_COLLECTION",
        _ => "ACCESS_POLICY",
    }
}

fn create_rule(w: &mut WorldState, req: &Req, pid: &str) -> Res {
    let p = find(w, pid)?;
    let mut body = Value::Object(req.object()?);
    if body["name"].as_str().is_none_or(str::is_empty) {
        return Err(validation(&["name: The field cannot be left blank"]));
    }
    let expected = rule_type_for(p["type"].as_str().unwrap_or_default());
    match body["type"].as_str() {
        None => body["type"] = json!(expected),
        Some(t) if t != expected => {
            return Err(validation(&[
                "type: The rule type does not match the policy type",
            ]));
        }
        _ => {}
    }
    let existing: Vec<(String, Value)> = list(w, "policy_rules")
        .into_iter()
        .filter(|(k, _)| k.starts_with(&format!("{pid}/")))
        .collect();
    let priority = body["priority"].as_i64().unwrap_or_else(|| {
        next_priority(&existing.iter().map(|(_, v)| v.clone()).collect::<Vec<_>>())
    });
    shift_system(w, "policy_rules", &existing, priority);
    let now = ts(w);
    let rid = new_id(w, "policy_rule", "0pr");
    let activate = req.q_bool("activate", true);
    body["id"] = json!(rid);
    body["status"] = json!(if activate { "ACTIVE" } else { "INACTIVE" });
    body["priority"] = json!(priority);
    body["system"] = json!(false);
    body["created"] = json!(now);
    body["lastUpdated"] = json!(now);
    put(w, "policy_rules", &format!("{pid}/{rid}"), body.clone());
    log_event(
        w,
        "policy.rule.add",
        "Add policy rule",
        vec![rule_target(&body), target(&p)],
    );
    Ok(Reply::ok(render_rule(pid, &body)))
}

fn replace_rule(w: &mut WorldState, req: &Req, pid: &str, rid: &str) -> Res {
    let p = find(w, pid)?;
    let old = find_rule(w, pid, rid)?;
    let mut body = Value::Object(req.object()?);
    if body["name"].as_str().is_none_or(str::is_empty) {
        return Err(validation(&["name: The field cannot be left blank"]));
    }
    for k in ["id", "type", "system", "created", "status"] {
        body[k] = old[k].clone();
    }
    if body.get("priority").is_none() {
        body["priority"] = old["priority"].clone();
    }
    body["lastUpdated"] = json!(ts(w));
    put(w, "policy_rules", &format!("{pid}/{rid}"), body.clone());
    log_event(
        w,
        "policy.rule.update",
        "Update policy rule",
        vec![rule_target(&body), target(&p)],
    );
    Ok(Reply::ok(render_rule(pid, &body)))
}
