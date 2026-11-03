//! Zones (`/zones`), DNS records, cache purge, and Workers routes.

use std::net::{Ipv4Addr, Ipv6Addr};

use mock_core::world::WorldState;
use serde_json::{Map, Value, json};

use crate::api::{
    ApiRequest, ApiResponse, ApiResult, all, fail, fail_chain, get, hex_id, is_hex32,
    method_not_allowed, no_route, now, ok, paged, put, remove, session_accounts, sort_by_query,
    str_field, values,
};

const NAME_SERVERS: &[&str] = &[
    "ada.ns.cloudflare.com",
    "bob.ns.cloudflare.com",
    "carl.ns.cloudflare.com",
    "dana.ns.cloudflare.com",
    "elle.ns.cloudflare.com",
    "fred.ns.cloudflare.com",
];

const RECORD_TYPES: &[&str] = &[
    "A",
    "AAAA",
    "CAA",
    "CERT",
    "CNAME",
    "DNSKEY",
    "DS",
    "HTTPS",
    "LOC",
    "MX",
    "NAPTR",
    "NS",
    "OPENPGPKEY",
    "PTR",
    "SMIMEA",
    "SRV",
    "SSHFP",
    "SVCB",
    "TLSA",
    "TXT",
    "URI",
];

pub fn route(w: &mut WorldState, req: &ApiRequest, segs: &[&str]) -> ApiResult {
    let m = req.method.as_str();
    if let ["zones"] = segs {
        return match m {
            "GET" => list_zones(w, req),
            "POST" => create_zone(w, req),
            _ => Err(method_not_allowed()),
        };
    }
    let zone_id = segs[1];
    let zone = zone(w, req, zone_id)?;
    match &segs[2..] {
        [] => match m {
            "GET" => ok(zone_view(&zone)),
            "PATCH" => patch_zone(w, req, zone),
            "DELETE" => delete_zone(w, zone_id),
            _ => Err(method_not_allowed()),
        },
        ["dns_records"] => match m {
            "GET" => list_records(w, req, zone_id),
            "POST" => {
                let body = req.json_object()?;
                let record = create_record(w, &zone, &body)?;
                ok(record_view(&record))
            }
            _ => Err(method_not_allowed()),
        },
        ["dns_records", "batch"] => match m {
            "POST" => batch(w, req, &zone),
            _ => Err(method_not_allowed()),
        },
        ["dns_records", "export"] => match m {
            "GET" => Ok(ApiResponse::raw(200, "text/plain", export(w, &zone))),
            _ => Err(method_not_allowed()),
        },
        ["dns_records", "usage"] => match m {
            "GET" => ok(json!({
                "record_quota": 1000,
                "record_usage": records_of(w, zone_id).len(),
            })),
            _ => Err(method_not_allowed()),
        },
        ["dns_records", record_id] => {
            let Some(record) =
                get(w, "dns_records", record_id).filter(|r| str_field(r, "zone_id") == zone_id)
            else {
                return Err(record_not_found());
            };
            match m {
                "GET" => ok(record_view(&record)),
                "PATCH" => {
                    let body = req.json_object()?;
                    ok(record_view(&update_record(w, &zone, record, &body, false)?))
                }
                "PUT" => {
                    let body = req.json_object()?;
                    ok(record_view(&update_record(w, &zone, record, &body, true)?))
                }
                "DELETE" => {
                    remove(w, "dns_records", record_id);
                    ok(json!({ "id": record_id }))
                }
                _ => Err(method_not_allowed()),
            }
        }
        ["purge_cache"] => match m {
            "POST" => purge(w, req, zone_id),
            _ => Err(method_not_allowed()),
        },
        ["workers", "routes"] => match m {
            "GET" => {
                let routes: Vec<Value> = routes_of(w, zone_id).iter().map(route_view).collect();
                ok(Value::Array(routes))
            }
            "POST" => create_route(w, req, &zone),
            _ => Err(method_not_allowed()),
        },
        ["workers", "routes", route_id] => {
            let Some(found) =
                get(w, "workers_routes", route_id).filter(|r| str_field(r, "zone_id") == zone_id)
            else {
                return Err(fail(404, &[(10023, "Route not found")]));
            };
            match m {
                "GET" => ok(route_view(&found)),
                "PUT" => update_route(w, req, &zone, found),
                "DELETE" => {
                    remove(w, "workers_routes", route_id);
                    ok(json!({ "id": route_id }))
                }
                _ => Err(method_not_allowed()),
            }
        }
        _ => Err(no_route(&req.path)),
    }
}

fn zone(w: &WorldState, req: &ApiRequest, id: &str) -> Result<Value, ApiResponse> {
    if !is_hex32(id) {
        return Err(no_route(&req.path));
    }
    get(w, "zones", id).ok_or_else(|| no_route(&req.path))
}

// ── Zones ────────────────────────────────────────────────────────────────────

pub fn zone_view(z: &Value) -> Value {
    let account = z
        .get("account")
        .cloned()
        .unwrap_or(json!({ "id": null, "name": null }));
    let plan = z.get("plan").cloned().unwrap_or_else(|| {
        json!({
            "id": "0feeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
            "name": "Free Website",
            "price": 0,
            "currency": "USD",
            "frequency": "",
            "is_subscribed": false,
            "can_subscribe": false,
            "legacy_id": "free",
            "legacy_discount": false,
            "externally_managed": false
        })
    });
    let status = z.get("status").cloned().unwrap_or(json!("active"));
    json!({
        "id": z["id"],
        "name": z["name"],
        "status": status,
        "paused": z.get("paused").cloned().unwrap_or(json!(false)),
        "type": z.get("type").cloned().unwrap_or(json!("full")),
        "development_mode": 0,
        "name_servers": z.get("name_servers").cloned().unwrap_or(json!(["ada.ns.cloudflare.com", "bob.ns.cloudflare.com"])),
        "original_name_servers": z.get("original_name_servers").cloned().unwrap_or(Value::Null),
        "original_registrar": z.get("original_registrar").cloned().unwrap_or(Value::Null),
        "original_dnshost": null,
        "modified_on": z.get("modified_on").cloned().unwrap_or(json!("2025-01-01T00:00:00.000000Z")),
        "created_on": z.get("created_on").cloned().unwrap_or(json!("2025-01-01T00:00:00.000000Z")),
        "activated_on": z.get("activated_on").cloned().unwrap_or(Value::Null),
        "vanity_name_servers": z.get("vanity_name_servers").cloned().unwrap_or(json!([])),
        "meta": {
            "step": 2,
            "custom_certificate_quota": 0,
            "page_rule_quota": 3,
            "phishing_detected": false,
            "cdn_only": false,
            "dns_only": false,
            "foundation_dns": false
        },
        "owner": { "id": null, "type": "user", "email": null },
        "account": account,
        "tenant": { "id": null, "name": null },
        "tenant_unit": { "id": null },
        "permissions": ["#dns_records:edit", "#dns_records:read", "#worker:edit", "#worker:read", "#zone:edit", "#zone:read"],
        "plan": plan,
    })
}

fn name_matches(value: &str, filter: &str) -> bool {
    let value = value.to_lowercase();
    let lower = filter.to_lowercase();
    if let Some(x) = lower.strip_prefix("contains:") {
        value.contains(x)
    } else if let Some(x) = lower.strip_prefix("starts_with:") {
        value.starts_with(x)
    } else if let Some(x) = lower.strip_prefix("ends_with:") {
        value.ends_with(x)
    } else if let Some(x) = lower.strip_prefix("not_equal:") {
        value != x
    } else if let Some(x) = lower.strip_prefix("equal:") {
        value == x
    } else {
        value == lower
    }
}

fn list_zones(w: &WorldState, req: &ApiRequest) -> ApiResult {
    let any = req.q("match") == Some("any");
    let mut items: Vec<Value> = values(w, "zones")
        .into_iter()
        .filter(|z| {
            let checks: Vec<bool> = [
                req.q("name").map(|n| name_matches(str_field(z, "name"), n)),
                req.q("status")
                    .map(|s| z.get("status").and_then(Value::as_str).unwrap_or("active") == s),
                req.q("account.id")
                    .map(|a| str_field(&z["account"], "id") == a),
                req.q("account.name")
                    .map(|a| name_matches(str_field(&z["account"], "name"), a)),
            ]
            .into_iter()
            .flatten()
            .collect();
            checks.is_empty()
                || if any {
                    checks.iter().any(|c| *c)
                } else {
                    checks.iter().all(|c| *c)
                }
        })
        .map(|z| zone_view(&z))
        .collect();
    sort_by_query(
        req,
        &mut items,
        &["name", "status", "account.id", "account.name"],
    );
    paged(req, items, 20, 50)
}

fn create_zone(w: &mut WorldState, req: &ApiRequest) -> ApiResult {
    let body = req.json_object()?;
    let Some(name) = body.get("name").and_then(Value::as_str) else {
        return Err(fail(400, &[(1001, "name: required")]));
    };
    let name = name.trim_end_matches('.').to_lowercase();
    let Some(account_id) = body
        .get("account")
        .and_then(|a| a.get("id"))
        .and_then(Value::as_str)
    else {
        return Err(fail(400, &[(1001, "account: required")]));
    };
    let Some(acct) = session_accounts(w)
        .into_iter()
        .find(|a| str_field(a, "id") == account_id)
    else {
        return Err(fail(
            403,
            &[(9109, "Unauthorized to access requested resource")],
        ));
    };
    if !name.contains('.') || name.starts_with('.') || name.contains(' ') {
        return Err(fail(
            400,
            &[(1049, &format!("{name} is not a registered domain"))],
        ));
    }
    if values(w, "zones")
        .iter()
        .any(|z| str_field(z, "name") == name)
    {
        return Err(fail(400, &[(1061, &format!("{name} already exists"))]));
    }
    let id = hex_id(w, &["zone", &name]);
    let ns_index = usize::from_str_radix(&id[..2], 16).unwrap_or(0) % (NAME_SERVERS.len() - 1);
    let created = now(w);
    let zone = json!({
        "id": id,
        "name": name,
        "status": "pending",
        "paused": false,
        "type": body.get("type").cloned().unwrap_or(json!("full")),
        "name_servers": [NAME_SERVERS[ns_index], NAME_SERVERS[ns_index + 1]],
        "account": { "id": acct["id"], "name": acct["name"] },
        "created_on": created,
        "modified_on": created,
        "activated_on": null,
    });
    put(w, "zones", &id, zone.clone());
    ok(zone_view(&zone))
}

fn patch_zone(w: &mut WorldState, req: &ApiRequest, mut zone: Value) -> ApiResult {
    let body = req.json_object()?;
    for (key, value) in &body {
        match key.as_str() {
            "paused" => {
                let Some(b) = value.as_bool() else {
                    return Err(fail(400, &[(1001, "paused: must be a boolean")]));
                };
                zone["paused"] = json!(b);
            }
            "type" => zone["type"] = value.clone(),
            "vanity_name_servers" => zone["vanity_name_servers"] = value.clone(),
            "plan" => {}
            other => {
                return Err(fail(400, &[(1001, &format!("{other}: cannot be changed"))]));
            }
        }
    }
    zone["modified_on"] = json!(now(w));
    let id = str_field(&zone, "id").to_owned();
    put(w, "zones", &id, zone.clone());
    ok(zone_view(&zone))
}

fn delete_zone(w: &mut WorldState, zone_id: &str) -> ApiResult {
    remove(w, "zones", zone_id);
    for (key, r) in all(w, "dns_records") {
        if str_field(&r, "zone_id") == zone_id {
            remove(w, "dns_records", &key);
        }
    }
    for (key, r) in all(w, "workers_routes") {
        if str_field(&r, "zone_id") == zone_id {
            remove(w, "workers_routes", &key);
        }
    }
    ok(json!({ "id": zone_id }))
}

// ── DNS records ──────────────────────────────────────────────────────────────

fn records_of(w: &WorldState, zone_id: &str) -> Vec<Value> {
    values(w, "dns_records")
        .into_iter()
        .filter(|r| str_field(r, "zone_id") == zone_id)
        .collect()
}

fn proxiable(kind: &str) -> bool {
    matches!(kind, "A" | "AAAA" | "CNAME")
}

pub fn record_view(r: &Value) -> Value {
    let kind = str_field(r, "type");
    let mut out = Map::new();
    out.insert("id".into(), r["id"].clone());
    out.insert("name".into(), r["name"].clone());
    out.insert("type".into(), r["type"].clone());
    out.insert("content".into(), r["content"].clone());
    if let Some(p) = r.get("priority") {
        out.insert("priority".into(), p.clone());
    }
    if let Some(d) = r.get("data") {
        out.insert("data".into(), d.clone());
    }
    out.insert("proxiable".into(), json!(proxiable(kind)));
    out.insert(
        "proxied".into(),
        r.get("proxied").cloned().unwrap_or(json!(false)),
    );
    out.insert("ttl".into(), r.get("ttl").cloned().unwrap_or(json!(1)));
    out.insert(
        "settings".into(),
        r.get("settings").cloned().unwrap_or(json!({})),
    );
    out.insert("meta".into(), json!({}));
    out.insert(
        "comment".into(),
        r.get("comment").cloned().unwrap_or(Value::Null),
    );
    out.insert("tags".into(), r.get("tags").cloned().unwrap_or(json!([])));
    out.insert(
        "created_on".into(),
        r.get("created_on").cloned().unwrap_or(Value::Null),
    );
    out.insert(
        "modified_on".into(),
        r.get("modified_on").cloned().unwrap_or(Value::Null),
    );
    for key in ["comment_modified_on", "tags_modified_on"] {
        if let Some(v) = r.get(key) {
            out.insert(key.into(), v.clone());
        }
    }
    Value::Object(out)
}

fn text_filter(req: &ApiRequest, field: &str, value: &str) -> Vec<bool> {
    let value = value.to_lowercase();
    let mut out = Vec::new();
    for (suffix, test) in [
        ("", 0),
        (".exact", 0),
        (".contains", 1),
        (".startswith", 2),
        (".endswith", 3),
    ] {
        if let Some(q) = req.q(&format!("{field}{suffix}")) {
            let q = q.to_lowercase();
            out.push(match test {
                0 => value == q,
                1 => value.contains(&q),
                2 => value.starts_with(&q),
                _ => value.ends_with(&q),
            });
        }
    }
    out
}

fn list_records(w: &WorldState, req: &ApiRequest, zone_id: &str) -> ApiResult {
    let any = req.q("match") == Some("any");
    let mut items: Vec<Value> = records_of(w, zone_id)
        .into_iter()
        .filter(|r| {
            let mut checks = Vec::new();
            checks.extend(text_filter(req, "name", str_field(r, "name")));
            checks.extend(text_filter(req, "content", str_field(r, "content")));
            let comment = r.get("comment").and_then(Value::as_str);
            checks.extend(text_filter(req, "comment", comment.unwrap_or("")));
            if req.q("comment.present").is_some() {
                checks.push(comment.is_some_and(|c| !c.is_empty()));
            }
            if req.q("comment.absent").is_some() {
                checks.push(comment.is_none_or(str::is_empty));
            }
            if let Some(t) = req.q("type") {
                checks.push(str_field(r, "type").eq_ignore_ascii_case(t));
            }
            if let Some(p) = req.q("proxied") {
                checks.push(
                    r.get("proxied").and_then(Value::as_bool).unwrap_or(false) == (p == "true"),
                );
            }
            if let Some(tag) = req.q("tag") {
                let tags = r.get("tags").and_then(Value::as_array);
                checks.push(tags.is_some_and(|t| t.iter().any(|x| x.as_str() == Some(tag))));
            }
            if let Some(s) = req.q("search") {
                let s = s.to_lowercase();
                checks.push(
                    str_field(r, "name").to_lowercase().contains(&s)
                        || str_field(r, "content").to_lowercase().contains(&s)
                        || comment.unwrap_or("").to_lowercase().contains(&s),
                );
            }
            checks.is_empty()
                || if any {
                    checks.iter().any(|c| *c)
                } else {
                    checks.iter().all(|c| *c)
                }
        })
        .map(|r| record_view(&r))
        .collect();
    sort_by_query(
        req,
        &mut items,
        &["type", "name", "content", "ttl", "proxied"],
    );
    paged(req, items, 100, 5_000_000)
}

fn validation(code: u32, message: &str) -> ApiResponse {
    fail_chain(400, 1004, "DNS Validation Error", &[(code, message)])
}

/// Validates and normalizes a record from `fields`, returning the stored document (without
/// id or timestamps).
fn build_record(
    zone: &Value,
    fields: &Map<String, Value>,
) -> Result<Map<String, Value>, ApiResponse> {
    let zone_name = str_field(zone, "name");
    let kind = fields
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_ascii_uppercase)
        .unwrap_or_default();
    if !RECORD_TYPES.contains(&kind.as_str()) {
        return Err(validation(9020, "Invalid DNS record type"));
    }
    let raw_name = fields
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if raw_name.is_empty() || raw_name.contains(' ') {
        return Err(validation(9000, "DNS name is invalid."));
    }
    let raw_name = raw_name.trim_end_matches('.').to_lowercase();
    let name = if raw_name == "@" || raw_name == zone_name {
        zone_name.to_owned()
    } else if raw_name.ends_with(&format!(".{zone_name}")) {
        raw_name
    } else {
        format!("{raw_name}.{zone_name}")
    };
    let mut content = match fields.get("content") {
        Some(Value::String(s)) => s.trim().to_owned(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    };
    if content.is_empty()
        && let Some(Value::Object(data)) = fields.get("data")
    {
        content = data
            .values()
            .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned))
            .collect::<Vec<_>>()
            .join(" ");
    }
    if content.is_empty() {
        return Err(validation(
            9010,
            &format!("Content for {kind} record is invalid."),
        ));
    }
    match kind.as_str() {
        "A" if content.parse::<Ipv4Addr>().is_err() => {
            return Err(validation(
                9005,
                "Content for A record must be a valid IPv4 address.",
            ));
        }
        "AAAA" => match content.parse::<Ipv6Addr>() {
            Ok(ip) => content = ip.to_string(),
            Err(_) => {
                return Err(validation(
                    9006,
                    "Content for AAAA record must be a valid IPv6 address.",
                ));
            }
        },
        "CNAME" | "NS" | "PTR" => {
            if content.contains(' ') || !content.contains('.') {
                return Err(validation(
                    9007,
                    &format!("Content for {kind} record is invalid."),
                ));
            }
            content = content.trim_end_matches('.').to_lowercase();
        }
        "MX" => {
            content = content.trim_end_matches('.').to_lowercase();
        }
        _ => {}
    }
    let ttl = match fields.get("ttl") {
        None | Some(Value::Null) => 1,
        Some(v) => v.as_i64().unwrap_or(-1),
    };
    if ttl != 1 && !(30..=86400).contains(&ttl) {
        return Err(validation(
            9021,
            "Invalid TTL. Must be between 60 and 86400 seconds, or 1 for Automatic.",
        ));
    }
    let proxied = fields
        .get("proxied")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if proxied && !proxiable(&kind) {
        return Err(validation(9004, "This record type cannot be proxied."));
    }
    let mut out = Map::new();
    out.insert("zone_id".into(), zone["id"].clone());
    out.insert("zone_name".into(), json!(zone_name));
    out.insert("name".into(), json!(name));
    out.insert("type".into(), json!(kind));
    out.insert("content".into(), json!(content));
    if kind == "MX" || kind == "URI" {
        let Some(priority) = fields.get("priority").and_then(Value::as_u64) else {
            return Err(validation(
                9101,
                &format!("Priority must be set for {kind} records."),
            ));
        };
        out.insert("priority".into(), json!(priority));
    }
    if let Some(data) = fields.get("data") {
        out.insert("data".into(), data.clone());
    }
    out.insert("proxied".into(), json!(proxied));
    out.insert("ttl".into(), json!(ttl));
    out.insert(
        "settings".into(),
        fields.get("settings").cloned().unwrap_or(json!({})),
    );
    out.insert(
        "comment".into(),
        fields.get("comment").cloned().unwrap_or(Value::Null),
    );
    out.insert(
        "tags".into(),
        fields.get("tags").cloned().unwrap_or(json!([])),
    );
    Ok(out)
}

/// Rejects records that clash with others in the zone (`except` is the record being changed).
fn check_conflicts(
    w: &WorldState,
    record: &Map<String, Value>,
    except: Option<&str>,
) -> Result<(), ApiResponse> {
    let zone_id = record["zone_id"].as_str().unwrap_or("");
    let name = record["name"].as_str().unwrap_or("");
    let kind = record["type"].as_str().unwrap_or("");
    let content = record["content"].as_str().unwrap_or("");
    for other in records_of(w, zone_id) {
        if except == Some(str_field(&other, "id")) || str_field(&other, "name") != name {
            continue;
        }
        let other_kind = str_field(&other, "type");
        if other_kind == kind && str_field(&other, "content") == content {
            return Err(fail(400, &[(81058, "An identical record already exists.")]));
        }
        let address = |k: &str| matches!(k, "A" | "AAAA" | "CNAME");
        if (kind == "CNAME" && address(other_kind)) || (other_kind == "CNAME" && address(kind)) {
            return Err(fail(
                400,
                &[(
                    81053,
                    "An A, AAAA, or CNAME record with that host already exists.",
                )],
            ));
        }
    }
    Ok(())
}

fn create_record(
    w: &mut WorldState,
    zone: &Value,
    body: &Map<String, Value>,
) -> Result<Value, ApiResponse> {
    let mut record = build_record(zone, body)?;
    check_conflicts(w, &record, None)?;
    let id = hex_id(
        w,
        &[
            "dns",
            record["zone_id"].as_str().unwrap_or(""),
            record["type"].as_str().unwrap_or(""),
            record["name"].as_str().unwrap_or(""),
            record["content"].as_str().unwrap_or(""),
        ],
    );
    let t = now(w);
    let mut doc = Map::new();
    doc.insert("id".into(), json!(id));
    doc.append(&mut record);
    doc.insert("created_on".into(), json!(t));
    doc.insert("modified_on".into(), json!(t));
    let doc = Value::Object(doc);
    put(w, "dns_records", &id, doc.clone());
    Ok(doc)
}

fn update_record(
    w: &mut WorldState,
    zone: &Value,
    existing: Value,
    body: &Map<String, Value>,
    replace: bool,
) -> Result<Value, ApiResponse> {
    let id = str_field(&existing, "id").to_owned();
    let mut fields: Map<String, Value> = if replace {
        Map::new()
    } else {
        existing.as_object().cloned().unwrap_or_default()
    };
    for (k, v) in body {
        if k != "id" {
            fields.insert(k.clone(), v.clone());
        }
    }
    if replace {
        for key in ["name", "type", "content"] {
            if !body.contains_key(key) && !(key == "content" && body.contains_key("data")) {
                return Err(validation(9000, &format!("{key}: required")));
            }
        }
    }
    let mut record = build_record(zone, &fields)?;
    check_conflicts(w, &record, Some(&id))?;
    let mut doc = Map::new();
    doc.insert("id".into(), json!(id));
    doc.append(&mut record);
    doc.insert(
        "created_on".into(),
        existing.get("created_on").cloned().unwrap_or(Value::Null),
    );
    doc.insert("modified_on".into(), json!(now(w)));
    if body.contains_key("comment") {
        doc.insert("comment_modified_on".into(), json!(now(w)));
    }
    if body.contains_key("tags") {
        doc.insert("tags_modified_on".into(), json!(now(w)));
    }
    let doc = Value::Object(doc);
    put(w, "dns_records", &id, doc.clone());
    Ok(doc)
}

fn batch(w: &mut WorldState, req: &ApiRequest, zone: &Value) -> ApiResult {
    let body = req.json_object()?;
    let saved = all(w, "dns_records");
    let result = run_batch(w, zone, &body);
    if result.is_err() {
        for (key, _) in all(w, "dns_records") {
            remove(w, "dns_records", &key);
        }
        for (key, doc) in saved {
            put(w, "dns_records", &key, doc);
        }
    }
    result
}

fn run_batch(w: &mut WorldState, zone: &Value, body: &Map<String, Value>) -> ApiResult {
    let zone_id = str_field(zone, "id").to_owned();
    let list = |key: &str| {
        body.get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let mut deletes = Vec::new();
    for item in list("deletes") {
        let id = str_field(&item, "id").to_owned();
        match get(w, "dns_records", &id).filter(|r| str_field(r, "zone_id") == zone_id) {
            Some(r) => {
                remove(w, "dns_records", &id);
                deletes.push(record_view(&r));
            }
            None => return Err(record_not_found()),
        }
    }
    let mut patches = Vec::new();
    let mut puts = Vec::new();
    for (key, out, replace) in [("patches", &mut patches, false), ("puts", &mut puts, true)] {
        for item in list(key) {
            let id = str_field(&item, "id").to_owned();
            let Some(existing) =
                get(w, "dns_records", &id).filter(|r| str_field(r, "zone_id") == zone_id)
            else {
                return Err(record_not_found());
            };
            let fields = item.as_object().cloned().unwrap_or_default();
            out.push(record_view(&update_record(
                w, zone, existing, &fields, replace,
            )?));
        }
    }
    let mut posts = Vec::new();
    for item in list("posts") {
        let fields = item.as_object().cloned().unwrap_or_default();
        posts.push(record_view(&create_record(w, zone, &fields)?));
    }
    ok(json!({ "deletes": deletes, "patches": patches, "puts": puts, "posts": posts }))
}

fn export(w: &WorldState, zone: &Value) -> String {
    let zone_name = str_field(zone, "name");
    let mut out = format!(
        ";;\n;; Domain:     {zone_name}.\n;; Exported:   {}\n;;\n;; This file is intended for use for informational and archival\n;; purposes ONLY and MUST be edited before use on a production\n;; DNS server.\n;;\n\n;; SOA Record\n{zone_name}\t3600\tIN\tSOA\tada.ns.cloudflare.com. dns.cloudflare.com. 2051234567 10000 2400 604800 3600\n\n",
        w.clock
            .now_rfc3339()
            .replace('T', " ")
            .trim_end_matches('Z')
    );
    let records = records_of(w, str_field(zone, "id"));
    for kind in RECORD_TYPES {
        let of_kind: Vec<&Value> = records
            .iter()
            .filter(|r| str_field(r, "type") == *kind)
            .collect();
        if of_kind.is_empty() {
            continue;
        }
        out.push_str(&format!(";; {kind} Records\n"));
        for r in of_kind {
            let ttl = r.get("ttl").and_then(Value::as_i64).unwrap_or(1);
            let mut content = str_field(r, "content").to_owned();
            if matches!(*kind, "CNAME" | "NS" | "MX" | "PTR") {
                content.push('.');
            }
            if let Some(p) = r.get("priority").and_then(Value::as_u64) {
                content = format!("{p} {content}");
            }
            if *kind == "TXT" && !content.starts_with('"') {
                content = format!("\"{content}\"");
            }
            let proxied = r.get("proxied").and_then(Value::as_bool).unwrap_or(false);
            out.push_str(&format!(
                "{}.\t{}\tIN\t{kind}\t{content} ; cf_tags=cf-proxied:{proxied}\n",
                str_field(r, "name"),
                if ttl == 1 { 1 } else { ttl }
            ));
        }
        out.push('\n');
    }
    out
}

fn record_not_found() -> ApiResponse {
    fail(404, &[(81044, "Record does not exist.")])
}

// ── Cache purge ──────────────────────────────────────────────────────────────

fn purge(w: &mut WorldState, req: &ApiRequest, zone_id: &str) -> ApiResult {
    let body = req.json_object()?;
    let everything = body.get("purge_everything").and_then(Value::as_bool) == Some(true);
    let lists = ["files", "tags", "hosts", "prefixes"];
    let has_list = lists.iter().any(|k| {
        body.get(*k)
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty())
    });
    if !everything && !has_list {
        return Err(fail(
            400,
            &[(
                1012,
                "Request must contain one of \"purge_everything\", \"files\", \"tags\", \"hosts\" or \"prefixes\"",
            )],
        ));
    }
    let n = all(w, "cache_purges")
        .iter()
        .filter(|(_, p)| str_field(p, "zone_id") == zone_id)
        .count()
        + 1;
    let doc = json!({ "zone_id": zone_id, "request": body, "time": now(w) });
    put(w, "cache_purges", &format!("{zone_id}#{n}"), doc);
    ok(json!({ "id": zone_id }))
}

// ── Workers routes ───────────────────────────────────────────────────────────

fn routes_of(w: &WorldState, zone_id: &str) -> Vec<Value> {
    values(w, "workers_routes")
        .into_iter()
        .filter(|r| str_field(r, "zone_id") == zone_id)
        .collect()
}

fn route_view(r: &Value) -> Value {
    let mut out = json!({ "id": r["id"], "pattern": r["pattern"] });
    if let Some(s) = r.get("script").filter(|s| !s.is_null()) {
        out["script"] = s.clone();
    }
    out
}

fn route_fields(
    w: &WorldState,
    req: &ApiRequest,
    zone: &Value,
) -> Result<(String, Option<String>), ApiResponse> {
    let body = req.json_object()?;
    let Some(pattern) = body.get("pattern").and_then(Value::as_str).map(str::trim) else {
        return Err(fail(400, &[(10021, "pattern: required")]));
    };
    let zone_name = str_field(zone, "name");
    let host = pattern
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or("")
        .trim_start_matches("*.")
        .trim_start_matches('*')
        .to_lowercase();
    if host != zone_name && !host.ends_with(&format!(".{zone_name}")) {
        return Err(fail(
            400,
            &[(
                10022,
                &format!("Route pattern must include zone name: {zone_name}"),
            )],
        ));
    }
    let script = body
        .get("script")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    if let Some(name) = &script {
        let account_id = str_field(&zone["account"], "id");
        if get(w, "workers_scripts", &format!("{account_id}/{name}")).is_none() {
            return Err(fail(
                404,
                &[(10007, "This Worker does not exist on your account.")],
            ));
        }
    }
    Ok((pattern.to_owned(), script))
}

fn create_route(w: &mut WorldState, req: &ApiRequest, zone: &Value) -> ApiResult {
    let zone_id = str_field(zone, "id").to_owned();
    let (pattern, script) = route_fields(w, req, zone)?;
    if routes_of(w, &zone_id)
        .iter()
        .any(|r| str_field(r, "pattern") == pattern)
    {
        return Err(fail(
            409,
            &[(10020, "A route with the same pattern already exists")],
        ));
    }
    let id = hex_id(w, &["route", &zone_id, &pattern]);
    let doc = json!({ "id": id, "zone_id": zone_id, "pattern": pattern, "script": script });
    put(w, "workers_routes", &id, doc.clone());
    ok(route_view(&doc))
}

fn update_route(w: &mut WorldState, req: &ApiRequest, zone: &Value, existing: Value) -> ApiResult {
    let zone_id = str_field(zone, "id").to_owned();
    let id = str_field(&existing, "id").to_owned();
    let (pattern, script) = route_fields(w, req, zone)?;
    if routes_of(w, &zone_id)
        .iter()
        .any(|r| str_field(r, "pattern") == pattern && str_field(r, "id") != id)
    {
        return Err(fail(
            409,
            &[(10020, "A route with the same pattern already exists")],
        ));
    }
    let doc = json!({ "id": id, "zone_id": zone_id, "pattern": pattern, "script": script });
    put(w, "workers_routes", &id, doc.clone());
    ok(route_view(&doc))
}
