//! Grafana-managed alerting: the Prometheus-compatible rules view, the ruler API, the
//! provisioning API (alert rules, contact points, policies, mute timings, templates), and
//! the Grafana Alertmanager (silences, alerts, status, config).

use axum::{
    http::{Method, StatusCode},
    response::Response,
};
use mock_core::http::json;
use regex::Regex;
use serde_json::{Map, Value, json};

use crate::{Ctx, Req, error, not_found, ok, parse_rfc3339, rfc3339};

pub(crate) fn route(ctx: &mut Ctx, req: &Req, segs: &[&str]) -> Response {
    let m = req.method.clone();
    match (m, segs) {
        (Method::GET, ["api", "prometheus", "grafana", "api", "v1", "rules"]) => {
            prom_rules(ctx, req)
        }
        (Method::GET, ["api", "prometheus", "grafana", "api", "v1", "alerts"]) => prom_alerts(ctx),
        (Method::GET, ["api", "prometheus", ds, "api", "v1", "rules"]) => {
            match ctx.get("datasources", ds) {
                Some(_) => ok(json!({"status": "success", "data": {"groups": []}})),
                None => json(
                    StatusCode::NOT_FOUND,
                    json!({"status": "error", "errorType": "not_found", "error": format!("data source not found (uid: {ds})")}),
                ),
            }
        }
        (Method::GET, ["api", "ruler", "grafana", "api", "v1", "rules"]) => ruler_rules(ctx),
        (Method::GET, ["api", "ruler", "grafana", "api", "v1", "rule", uid]) => {
            match ctx.get("alert_rules", uid) {
                Some(r) => ok(ruler_node(ctx, &r)),
                None => not_found("rule not found"),
            }
        }
        (
            Method::GET,
            [
                "api",
                "ruler",
                "grafana",
                "api",
                "v1",
                "rule",
                uid,
                "versions",
            ],
        ) => match ctx.get("alert_rules", uid) {
            Some(r) => {
                let mut versions = vec![ruler_node(ctx, &r)];
                if let Some(prev) = r["previousVersions"].as_array() {
                    for p in prev {
                        let mut merged = r.clone();
                        if let Some(o) = p.as_object() {
                            for (k, v) in o {
                                merged[k] = v.clone();
                            }
                        }
                        versions.push(ruler_node(ctx, &merged));
                    }
                }
                ok(Value::Array(versions))
            }
            None => not_found("rule not found"),
        },
        (Method::GET, ["api", "v1", "provisioning", "alert-rules"]) => ok(Value::Array(
            ctx.coll("alert_rules")
                .into_iter()
                .map(|(_, r)| provisioned(&r))
                .collect(),
        )),
        (Method::GET, ["api", "v1", "provisioning", "alert-rules", uid]) => {
            match ctx.get("alert_rules", uid) {
                Some(r) => ok(provisioned(&r)),
                None => json(StatusCode::NOT_FOUND, json!({"message": "", "traceID": ""})),
            }
        }
        (Method::GET, ["api", "v1", "provisioning", "alert-rules", uid, "export"]) => {
            match ctx.get("alert_rules", uid) {
                Some(r) => ok(
                    json!({"apiVersion": 1, "groups": [{"orgId": 1, "name": r["ruleGroup"], "folder": folder_title(ctx, r["folderUID"].as_str().unwrap_or_default()), "interval": "1m", "rules": [provisioned(&r)]}]}),
                ),
                None => json(StatusCode::NOT_FOUND, json!({"message": "", "traceID": ""})),
            }
        }
        (Method::POST, ["api", "v1", "provisioning", "alert-rules"]) => save_rule(ctx, req, None),
        (Method::PUT, ["api", "v1", "provisioning", "alert-rules", uid]) => {
            save_rule(ctx, req, Some(uid))
        }
        (Method::DELETE, ["api", "v1", "provisioning", "alert-rules", uid]) => {
            ctx.remove("alert_rules", uid);
            json(StatusCode::NO_CONTENT, Value::Null)
        }
        (Method::GET, ["api", "v1", "provisioning", "contact-points"]) => {
            let name = req.q("name");
            ok(Value::Array(
                ctx.coll("contact_points")
                    .into_iter()
                    .map(|(_, c)| c)
                    .filter(|c| name.is_none_or(|n| c["name"] == n))
                    .collect(),
            ))
        }
        (Method::POST, ["api", "v1", "provisioning", "contact-points"]) => {
            save_contact_point(ctx, req, None)
        }
        (Method::PUT, ["api", "v1", "provisioning", "contact-points", uid]) => {
            save_contact_point(ctx, req, Some(uid))
        }
        (Method::DELETE, ["api", "v1", "provisioning", "contact-points", uid]) => {
            if ctx.coll("policies").iter().any(|(_, p)| {
                p.to_string().contains(&format!(
                    "\"receiver\":{}",
                    ctx.get("contact_points", uid)
                        .map(|c| c["name"].to_string())
                        .unwrap_or_default()
                ))
            }) {
                return json(
                    StatusCode::CONFLICT,
                    json!({"message": "contact point is referenced by a notification policy", "traceID": ""}),
                );
            }
            ctx.remove("contact_points", uid);
            json(
                StatusCode::ACCEPTED,
                json!({"message": "contactpoint deleted"}),
            )
        }
        (Method::GET, ["api", "v1", "provisioning", "policies"]) => {
            ok(ctx.get("policies", "root").unwrap_or_else(default_policy))
        }
        (Method::GET, ["api", "v1", "provisioning", "policies", "export"]) => ok(
            json!({"apiVersion": 1, "policies": [ctx.get("policies", "root").unwrap_or_else(default_policy)]}),
        ),
        (Method::PUT, ["api", "v1", "provisioning", "policies"]) => {
            let body = match req.json() {
                Ok(b) => b,
                Err(r) => return r,
            };
            if body["receiver"].as_str().is_none_or(str::is_empty) {
                return error(
                    StatusCode::BAD_REQUEST,
                    "invalid object specification: root route must specify a default receiver",
                );
            }
            ctx.put("policies", "root", body);
            json(StatusCode::ACCEPTED, json!({"message": "policies updated"}))
        }
        (Method::DELETE, ["api", "v1", "provisioning", "policies"]) => {
            ctx.put("policies", "root", default_policy());
            json(
                StatusCode::ACCEPTED,
                json!({"message": "policies resetted"}),
            )
        }
        (Method::GET, ["api", "v1", "provisioning", "mute-timings"]) => ok(Value::Array(
            ctx.coll("mute_timings")
                .into_iter()
                .map(|(_, m)| m)
                .collect(),
        )),
        (Method::GET, ["api", "v1", "provisioning", "mute-timings", name]) => {
            match ctx.get("mute_timings", &crate::url_decode(name)) {
                Some(m) => ok(m),
                None => json(
                    StatusCode::NOT_FOUND,
                    json!({"message": "mute timing not found", "traceID": ""}),
                ),
            }
        }
        (Method::POST, ["api", "v1", "provisioning", "mute-timings"]) => {
            let mut body = match req.json() {
                Ok(b) => b,
                Err(r) => return r,
            };
            let Some(name) = body["name"]
                .as_str()
                .map(str::to_owned)
                .filter(|n| !n.is_empty())
            else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "invalid object specification: missing name",
                );
            };
            if ctx.get("mute_timings", &name).is_some() {
                return json(
                    StatusCode::CONFLICT,
                    json!({"message": "a mute timing with this name already exists", "traceID": ""}),
                );
            }
            body["version"] = json!(format!(
                "{:x}",
                crate::irm::hash(&body.to_string()) & 0xffff_ffff
            ));
            body["provenance"] = json!("api");
            ctx.put("mute_timings", &name, body.clone());
            json(StatusCode::CREATED, body)
        }
        (Method::GET, ["api", "v1", "provisioning", "templates"]) => ok(Value::Array(
            ctx.coll("notification_templates")
                .into_iter()
                .map(|(_, t)| t)
                .collect(),
        )),
        (_, ["api", "alertmanager", "grafana", "api", "v2", rest @ ..]) => {
            alertmanager(ctx, req, rest)
        }
        (
            Method::GET,
            [
                "api",
                "alertmanager",
                "grafana",
                "config",
                "api",
                "v1",
                "alerts",
            ],
        ) => ok(am_config(ctx)),
        _ => not_found("Not found"),
    }
}

fn folder_title(ctx: &Ctx, uid: &str) -> String {
    ctx.get("folders", uid)
        .and_then(|f| f["title"].as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn provisioned(r: &Value) -> Value {
    json!({
        "id": r["id"], "uid": r["uid"], "orgID": 1, "folderUID": r["folderUID"], "ruleGroup": r["ruleGroup"], "title": r["title"],
        "condition": r["condition"].as_str().unwrap_or("C"), "data": r["data"].clone().as_array().cloned().unwrap_or_default(), "updated": r["updated"],
        "noDataState": r["noDataState"].as_str().unwrap_or("NoData"), "execErrState": r["execErrState"].as_str().unwrap_or("Error"),
        "for": r["for"].as_str().unwrap_or("5m"), "annotations": r["annotations"].clone().as_object().cloned().unwrap_or_default(),
        "labels": r["labels"].clone().as_object().cloned().unwrap_or_default(), "isPaused": r["isPaused"].as_bool().unwrap_or(false),
        "notification_settings": r["notification_settings"], "record": r["record"], "provenance": r["provenance"].as_str().unwrap_or(""),
    })
}

fn ruler_node(ctx: &Ctx, r: &Value) -> Value {
    json!({
        "expr": "", "for": r["for"].as_str().unwrap_or("5m"), "labels": r["labels"], "annotations": r["annotations"],
        "grafana_alert": {
            "id": r["id"], "orgId": 1, "title": r["title"], "condition": r["condition"].as_str().unwrap_or("C"), "data": r["data"],
            "updated": r["updated"], "intervalSeconds": 60, "version": r["version"].as_i64().unwrap_or(1), "uid": r["uid"],
            "namespace_uid": r["folderUID"], "rule_group": r["ruleGroup"], "no_data_state": r["noDataState"].as_str().unwrap_or("NoData"),
            "exec_err_state": r["execErrState"].as_str().unwrap_or("Error"), "is_paused": r["isPaused"].as_bool().unwrap_or(false),
            "provenance": r["provenance"].as_str().unwrap_or(""), "namespace_title": folder_title(ctx, r["folderUID"].as_str().unwrap_or_default()),
        }
    })
}

fn ruler_rules(ctx: &Ctx) -> Response {
    let mut out: Map<String, Value> = Map::new();
    for (_, r) in ctx.coll("alert_rules") {
        let folder = folder_title(ctx, r["folderUID"].as_str().unwrap_or_default());
        let groups = out.entry(folder).or_insert_with(|| json!([]));
        let arr = groups.as_array_mut().expect("array");
        let g = r["ruleGroup"].clone();
        match arr.iter_mut().find(|x| x["name"] == g) {
            Some(x) => x["rules"]
                .as_array_mut()
                .expect("rules")
                .push(ruler_node(ctx, &r)),
            None => arr.push(json!({"name": g, "interval": "1m", "rules": [ruler_node(ctx, &r)]})),
        }
    }
    ok(Value::Object(out))
}

/// The rule's state in the Prometheus-compatible view.
fn rule_state(r: &Value) -> &str {
    match r["state"].as_str().unwrap_or("inactive") {
        "normal" => "inactive",
        other => other,
    }
}

fn label_match(m: &Value, labels: &Value) -> bool {
    let name = m["name"].as_str().unwrap_or_default();
    let value = m["value"].as_str().unwrap_or_default();
    let actual = labels[name].as_str().unwrap_or_default();
    let is_regex = m["isRegex"].as_bool().unwrap_or(false);
    let is_equal = m["isEqual"].as_bool().unwrap_or(true);
    let hit = if is_regex {
        Regex::new(&format!("^(?:{value})$")).is_ok_and(|re| re.is_match(actual))
    } else {
        actual == value
    };
    hit == is_equal
}

fn prom_rules(ctx: &Ctx, req: &Req) -> Response {
    let folder = req.q("folder_uid");
    let search_folder = req.q("search.folder").map(str::to_lowercase);
    let group = req.q("rule_group");
    let name = req.q("search.rule_name").map(str::to_lowercase);
    let rule_type = req.q("rule_type");
    let states: Vec<String> = req
        .qs("state")
        .iter()
        .map(|s| {
            if *s == "normal" {
                "inactive".to_owned()
            } else {
                s.to_lowercase()
            }
        })
        .collect();
    let rule_limit = req
        .q("rule_limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(usize::MAX);
    let limit_alerts = req
        .q("limit_alerts")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(usize::MAX);
    let matchers: Vec<Value> = req
        .qs("matcher")
        .iter()
        .filter_map(|m| serde_json::from_str(m).ok())
        .collect();
    let mut groups: Vec<Value> = Vec::new();
    let mut totals: Map<String, Value> = Map::new();
    let mut count = 0;
    for (_, r) in ctx.coll("alert_rules") {
        let fuid = r["folderUID"].as_str().unwrap_or_default();
        let ftitle = folder_title(ctx, fuid);
        let typ = if r["record"].is_object() {
            "recording"
        } else {
            "alerting"
        };
        let state = rule_state(&r).to_owned();
        if folder.is_some_and(|f| f != fuid)
            || search_folder
                .as_ref()
                .is_some_and(|s| !ftitle.to_lowercase().contains(s))
            || group.is_some_and(|g| r["ruleGroup"] != g)
            || name.as_ref().is_some_and(|n| {
                !r["title"]
                    .as_str()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(n)
            })
            || rule_type.is_some_and(|t| t != typ)
            || (!states.is_empty() && !states.contains(&state))
            || !matchers.iter().all(|m| {
                label_match(m, &r["labels"])
                    || r["alerts"]
                        .as_array()
                        .is_some_and(|a| a.iter().any(|x| label_match(m, &x["labels"])))
            })
        {
            continue;
        }
        if count >= rule_limit {
            break;
        }
        count += 1;
        *totals.entry(state.clone()).or_insert(json!(0)) =
            json!(totals.get(&state).and_then(Value::as_i64).unwrap_or(0) + 1);
        let alerts: Vec<Value> = r["alerts"].as_array().cloned().unwrap_or_default();
        let mut atotals: Map<String, Value> = Map::new();
        for a in &alerts {
            let k = a["state"].as_str().unwrap_or("Normal").to_lowercase();
            let k = if k == "alerting" {
                "alerting".to_owned()
            } else {
                k
            };
            let n = atotals.get(&k).and_then(Value::as_i64).unwrap_or(0) + 1;
            atotals.insert(k, json!(n));
        }
        let query = r["data"]
            .as_array()
            .and_then(|d| {
                d.iter()
                    .find_map(|q| q["model"]["expr"].as_str().map(str::to_owned))
            })
            .unwrap_or_default();
        let rule = json!({
            "state": state, "name": r["title"], "query": query, "duration": duration_secs(r["for"].as_str().unwrap_or("5m")),
            "annotations": r["annotations"], "labels": r["labels"], "health": r["health"].as_str().unwrap_or("ok"), "lastError": r["lastError"].as_str().unwrap_or(""),
            "type": typ, "lastEvaluation": r["lastEvaluation"].as_str().map(str::to_owned).unwrap_or_else(|| rfc3339(ctx.now - 23.0)), "evaluationTime": 0.0123,
            "uid": r["uid"], "folderUid": fuid, "alerts": alerts.iter().take(limit_alerts).cloned().collect::<Vec<_>>(), "totals": atotals.clone(), "totalsFiltered": atotals,
            "activeAt": alerts.iter().filter_map(|a| a["activeAt"].as_str()).min(),
            "isPaused": r["isPaused"].as_bool().unwrap_or(false),
        });
        let gname = r["ruleGroup"].clone();
        match groups
            .iter_mut()
            .find(|g| g["name"] == gname && g["folderUid"] == fuid)
        {
            Some(g) => g["rules"].as_array_mut().expect("rules").push(rule),
            None => groups.push(json!({
                "name": gname, "file": ftitle, "folderUid": fuid, "rules": [rule], "interval": 60,
                "lastEvaluation": rfc3339(ctx.now - 23.0), "evaluationTime": 0.0123, "totals": {},
            })),
        }
    }
    ok(json!({"status": "success", "data": {"groups": groups, "totals": totals}}))
}

fn prom_alerts(ctx: &Ctx) -> Response {
    let alerts: Vec<Value> = ctx
        .coll("alert_rules")
        .into_iter()
        .flat_map(|(_, r)| r["alerts"].as_array().cloned().unwrap_or_default())
        .collect();
    ok(json!({"status": "success", "data": {"alerts": alerts}}))
}

fn duration_secs(s: &str) -> f64 {
    let mut total = 0.0;
    let mut num = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() || c == '.' {
            num.push(c);
        } else {
            let n: f64 = num.parse().unwrap_or(0.0);
            num.clear();
            total += n * match c {
                's' => 1.0,
                'm' => 60.0,
                'h' => 3600.0,
                'd' => 86400.0,
                _ => 0.0,
            };
        }
    }
    total
}

fn save_rule(ctx: &mut Ctx, req: &Req, uid: Option<&str>) -> Response {
    let body = match req.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    let existing = uid.and_then(|u| ctx.get("alert_rules", u));
    if uid.is_some() && existing.is_none() {
        return json(StatusCode::NOT_FOUND, json!({"message": "", "traceID": ""}));
    }
    let mut missing = Vec::new();
    for k in ["title", "folderUID", "ruleGroup", "condition"] {
        if body[k].as_str().is_none_or(str::is_empty) {
            missing.push(k);
        }
    }
    if body["data"].as_array().is_none_or(Vec::is_empty) {
        missing.push("data");
    }
    if !missing.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            &format!(
                "invalid rule specification at index [0]: {} is missing",
                missing.join(", ")
            ),
        );
    }
    let folder = body["folderUID"].as_str().unwrap_or_default();
    if ctx.get("folders", folder).is_none() {
        return error(
            StatusCode::BAD_REQUEST,
            &format!("invalid alert rule: folder does not exist: {folder}"),
        );
    }
    let new_uid = match (uid, body["uid"].as_str().filter(|u| !u.is_empty())) {
        (Some(u), _) => u.to_owned(),
        (None, Some(u)) => {
            if ctx.get("alert_rules", u).is_some() {
                return json(
                    StatusCode::CONFLICT,
                    json!({"message": "failed to create alert rule: a conflicting alert rule is found: rule title under the same organisation and folder should be unique", "traceID": ""}),
                );
            }
            u.to_owned()
        }
        (None, None) => ctx.new_uid("alert-rule"),
    };
    let mut rule = body.clone();
    rule["uid"] = json!(new_uid);
    rule["id"] = existing
        .as_ref()
        .map(|e| e["id"].clone())
        .unwrap_or_else(|| json!(ctx.next_id("alert_rules", "id", 0)));
    rule["updated"] = json!(ctx.now_str());
    rule["version"] = json!(
        existing
            .as_ref()
            .and_then(|e| e["version"].as_i64())
            .unwrap_or(0)
            + 1
    );
    rule["provenance"] = json!(if req.headers.get("x-disable-provenance").is_some() {
        ""
    } else {
        "api"
    });
    rule["state"] = existing
        .as_ref()
        .map(|e| e["state"].clone())
        .unwrap_or(json!("inactive"));
    rule["alerts"] = existing
        .as_ref()
        .map(|e| e["alerts"].clone())
        .unwrap_or(json!([]));
    ctx.put("alert_rules", &new_uid, rule.clone());
    json(
        if uid.is_some() {
            StatusCode::OK
        } else {
            StatusCode::CREATED
        },
        provisioned(&rule),
    )
}

fn save_contact_point(ctx: &mut Ctx, req: &Req, uid: Option<&str>) -> Response {
    let body = match req.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    if body["type"].as_str().is_none_or(str::is_empty)
        || body["name"].as_str().is_none_or(str::is_empty)
    {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid object specification: contact point name and type are required",
        );
    }
    if let Some(u) = uid
        && ctx.get("contact_points", u).is_none()
    {
        return json(
            StatusCode::NOT_FOUND,
            json!({"message": "contact point not found", "traceID": ""}),
        );
    }
    let new_uid = uid
        .map(str::to_owned)
        .or_else(|| {
            body["uid"]
                .as_str()
                .filter(|u| !u.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| ctx.new_uid("contact-point"));
    let cp = json!({
        "uid": new_uid, "name": body["name"], "type": body["type"], "settings": body["settings"].clone().as_object().cloned().unwrap_or_default(),
        "disableResolveMessage": body["disableResolveMessage"].as_bool().unwrap_or(false), "provenance": "api",
    });
    ctx.put("contact_points", &new_uid, cp.clone());
    json(StatusCode::ACCEPTED, cp)
}

fn default_policy() -> Value {
    json!({"receiver": "grafana-default-email", "group_by": ["grafana_folder", "alertname"], "provenance": ""})
}

fn am_config(ctx: &Ctx) -> Value {
    let mut receivers: Vec<Value> = Vec::new();
    for (_, c) in ctx.coll("contact_points") {
        let name = c["name"].clone();
        let cfg = json!({"uid": c["uid"], "name": c["name"], "type": c["type"], "disableResolveMessage": c["disableResolveMessage"], "settings": c["settings"], "secureFields": {}});
        match receivers.iter_mut().find(|r| r["name"] == name) {
            Some(r) => r["grafana_managed_receiver_configs"]
                .as_array_mut()
                .expect("array")
                .push(cfg),
            None => {
                receivers.push(json!({"name": name, "grafana_managed_receiver_configs": [cfg]}))
            }
        }
    }
    json!({
        "template_files": {},
        "alertmanager_config": {
            "route": ctx.get("policies", "root").unwrap_or_else(default_policy),
            "mute_time_intervals": ctx.coll("mute_timings").into_iter().map(|(_, m)| m).collect::<Vec<_>>(),
            "templates": null,
            "receivers": receivers,
        }
    })
}

// ───────────────────────────── alertmanager ─────────────────────────────

fn silence_state(ctx: &Ctx, s: &Value) -> &'static str {
    let start = s["startsAt"]
        .as_str()
        .and_then(parse_rfc3339)
        .unwrap_or(0.0);
    let end = s["endsAt"].as_str().and_then(parse_rfc3339).unwrap_or(0.0);
    if end <= ctx.now {
        "expired"
    } else if start > ctx.now {
        "pending"
    } else {
        "active"
    }
}

fn silence_json(ctx: &Ctx, s: &Value) -> Value {
    json!({
        "id": s["id"], "status": {"state": silence_state(ctx, s)}, "updatedAt": s["updatedAt"], "comment": s["comment"],
        "createdBy": s["createdBy"], "endsAt": s["endsAt"], "matchers": s["matchers"], "startsAt": s["startsAt"],
    })
}

/// `name="value"`, `name=~"re"`, `name!="v"` filter expressions (Alertmanager `filter=`).
fn parse_filter(f: &str) -> Option<Value> {
    let f = f.trim().trim_start_matches('{').trim_end_matches('}');
    for (op, is_equal, is_regex) in [
        ("!~", false, true),
        ("=~", true, true),
        ("!=", false, false),
        ("=", true, false),
    ] {
        if let Some((k, v)) = f.split_once(op) {
            return Some(
                json!({"name": k.trim(), "value": v.trim().trim_matches('"'), "isEqual": is_equal, "isRegex": is_regex}),
            );
        }
    }
    None
}

fn silenced_by(ctx: &Ctx, labels: &Value) -> Vec<Value> {
    ctx.coll("silences")
        .into_iter()
        .filter(|(_, s)| silence_state(ctx, s) == "active")
        .filter(|(_, s)| {
            s["matchers"]
                .as_array()
                .is_some_and(|ms| !ms.is_empty() && ms.iter().all(|m| label_match(m, labels)))
        })
        .map(|(id, _)| json!(id))
        .collect()
}

fn alertmanager(ctx: &mut Ctx, req: &Req, rest: &[&str]) -> Response {
    match (req.method.clone(), rest) {
        (Method::GET, ["silences"]) => {
            let filters: Vec<Value> = req
                .qs("filter")
                .iter()
                .filter_map(|f| parse_filter(f))
                .collect();
            let mut list: Vec<Value> = ctx
                .coll("silences")
                .into_iter()
                .map(|(_, s)| s)
                .filter(|s| {
                    filters.iter().all(|f| {
                        s["matchers"].as_array().is_some_and(|ms| {
                            ms.iter().any(|m| {
                                m["name"] == f["name"]
                                    && (f["isRegex"] == true || m["value"] == f["value"])
                            })
                        })
                    })
                })
                .map(|s| silence_json(ctx, &s))
                .collect();
            let order = |s: &Value| match s["status"]["state"].as_str() {
                Some("active") => 0,
                Some("pending") => 1,
                _ => 2,
            };
            list.sort_by(|a, b| {
                order(a)
                    .cmp(&order(b))
                    .then_with(|| a["endsAt"].as_str().cmp(&b["endsAt"].as_str()))
            });
            ok(Value::Array(list))
        }
        (Method::POST, ["silences"]) => {
            let body = match req.json() {
                Ok(b) => b,
                Err(r) => return r,
            };
            let matchers = body["matchers"].as_array().cloned().unwrap_or_default();
            if matchers.is_empty() {
                return json(
                    StatusCode::BAD_REQUEST,
                    json!({"message": "unable to save silence: invalid silence: at least one matcher required", "traceID": ""}),
                );
            }
            let (Some(starts), Some(ends)) = (
                body["startsAt"].as_str().and_then(parse_rfc3339),
                body["endsAt"].as_str().and_then(parse_rfc3339),
            ) else {
                return json(
                    StatusCode::BAD_REQUEST,
                    json!({"message": "unable to save silence: invalid silence: invalid zero start or end timestamp", "traceID": ""}),
                );
            };
            if ends <= starts {
                return json(
                    StatusCode::BAD_REQUEST,
                    json!({"message": "unable to save silence: invalid silence: end time must not be before start time", "traceID": ""}),
                );
            }
            if ends <= ctx.now {
                return json(
                    StatusCode::BAD_REQUEST,
                    json!({"message": "unable to save silence: invalid silence: end time can't be in the past", "traceID": ""}),
                );
            }
            let id = match body["id"].as_str().filter(|i| !i.is_empty()) {
                Some(id) => {
                    if ctx.get("silences", id).is_none() {
                        return json(
                            StatusCode::NOT_FOUND,
                            json!({"message": "silence not found", "traceID": ""}),
                        );
                    }
                    id.to_owned()
                }
                None => ctx.state.ids.next_uuid("silence").to_string(),
            };
            let matchers: Vec<Value> = matchers
                .into_iter()
                .map(|m| json!({"name": m["name"], "value": m["value"], "isRegex": m["isRegex"].as_bool().unwrap_or(false), "isEqual": m["isEqual"].as_bool().unwrap_or(true)}))
                .collect();
            let s = json!({
                "id": id, "matchers": matchers, "startsAt": rfc3339(starts.max(ctx.now)), "endsAt": rfc3339(ends),
                "createdBy": body["createdBy"].as_str().unwrap_or_default(), "comment": body["comment"].as_str().unwrap_or_default(), "updatedAt": ctx.now_str(),
            });
            ctx.put("silences", &id, s);
            json(StatusCode::ACCEPTED, json!({"silenceID": id}))
        }
        (Method::GET, ["silence", id]) => match ctx.get("silences", id) {
            Some(s) => ok(silence_json(ctx, &s)),
            None => json(
                StatusCode::NOT_FOUND,
                json!({"message": "silence not found", "traceID": ""}),
            ),
        },
        (Method::DELETE, ["silence", id]) => match ctx.get("silences", id) {
            Some(mut s) => {
                if silence_state(ctx, &s) != "expired" {
                    s["endsAt"] = json!(ctx.now_str());
                    s["updatedAt"] = json!(ctx.now_str());
                    ctx.put("silences", id, s);
                }
                ok(json!({"message": "silence deleted"}))
            }
            None => json(
                StatusCode::NOT_FOUND,
                json!({"message": "silence not found", "traceID": ""}),
            ),
        },
        (Method::GET, ["alerts"]) => ok(Value::Array(am_alerts(ctx, req))),
        (Method::GET, ["alerts", "groups"]) => {
            let alerts = am_alerts(ctx, req);
            let mut groups: Vec<Value> = Vec::new();
            for a in alerts {
                let key = json!({"alertname": a["labels"]["alertname"], "grafana_folder": a["labels"]["grafana_folder"]});
                match groups.iter_mut().find(|g| g["labels"] == key) {
                    Some(g) => g["alerts"].as_array_mut().expect("alerts").push(a),
                    None => groups.push(json!({"labels": key, "receiver": {"name": a["receivers"][0]["name"]}, "alerts": [a]})),
                }
            }
            ok(Value::Array(groups))
        }
        (Method::GET, ["status"]) => ok(json!({
            "cluster": {"peers": [], "status": "disabled"},
            "config": {"route": ctx.get("policies", "root").unwrap_or_else(default_policy), "receivers": am_config(ctx)["alertmanager_config"]["receivers"]},
            "uptime": rfc3339(ctx.now - 86_400.0 * 9.0),
            "versionInfo": {"branch": "HEAD", "buildDate": "", "buildUser": "", "goVersion": "go1.24.6", "revision": "", "version": "0.28.0"},
        })),
        _ => not_found("Not found"),
    }
}

/// Firing and pending alert instances from the rules, plus any stored `alerts`.
fn am_alerts(ctx: &Ctx, req: &Req) -> Vec<Value> {
    let filters: Vec<Value> = req
        .qs("filter")
        .iter()
        .filter_map(|f| parse_filter(f))
        .collect();
    let want = |k: &str| req.q(k).is_none_or(|v| v != "false");
    let receiver = req.q("receiver").map(str::to_owned);
    let default_receiver = ctx
        .get("policies", "root")
        .and_then(|p| p["receiver"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "grafana-default-email".into());
    let mut out = Vec::new();
    let mut sources: Vec<(Value, Value)> = Vec::new();
    for (_, r) in ctx.coll("alert_rules") {
        for a in r["alerts"].as_array().cloned().unwrap_or_default() {
            if matches!(
                a["state"].as_str(),
                Some("Alerting") | Some("Alerting (NoData)") | Some("Alerting (Error)")
            ) {
                sources.push((a, r.clone()));
            }
        }
    }
    for (_, a) in ctx.coll("alerts") {
        sources.push((a, Value::Null));
    }
    for (a, r) in sources {
        let mut labels = a["labels"].clone();
        if !r.is_null() {
            labels["__alert_rule_uid__"] = r["uid"].clone();
            if labels["grafana_folder"].is_null() {
                labels["grafana_folder"] = json!(folder_title(
                    ctx,
                    r["folderUID"].as_str().unwrap_or_default()
                ));
            }
        }
        if !filters.iter().all(|f| label_match(f, &labels)) {
            continue;
        }
        let silenced = silenced_by(ctx, &labels);
        let state = if silenced.is_empty() {
            "active"
        } else {
            "suppressed"
        };
        if (state == "active" && !want("active")) || (state == "suppressed" && !want("silenced")) {
            continue;
        }
        let recv = a["receiver"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| default_receiver.clone());
        if receiver
            .as_ref()
            .is_some_and(|r| !Regex::new(&format!("^(?:{r})$")).is_ok_and(|re| re.is_match(&recv)))
        {
            continue;
        }
        let fp = format!("{:016x}", crate::irm::hash(&labels.to_string()));
        out.push(json!({
            "annotations": a["annotations"], "endsAt": rfc3339(ctx.now + 240.0), "fingerprint": fp,
            "receivers": [{"name": recv}], "startsAt": a["activeAt"].as_str().unwrap_or(&ctx.now_str()),
            "status": {"inhibitedBy": [], "silencedBy": silenced, "state": state}, "updatedAt": rfc3339(ctx.now - 23.0),
            "generatorURL": format!("{}/alerting/grafana/{}/view?orgId=1", crate::PUBLIC_URL, r["uid"].as_str().unwrap_or_default()),
            "labels": labels,
        }));
    }
    out
}
