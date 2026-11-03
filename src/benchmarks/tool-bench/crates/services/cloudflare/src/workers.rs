//! Workers scripts (`/accounts/{id}/workers/scripts`): upload, download, delete,
//! settings, and secrets. Documents are keyed `<account_id>/<script_name>`.

use mock_core::world::WorldState;
use serde_json::{Map, Value, json};

use crate::api::{
    ApiRequest, ApiResponse, ApiResult, Body, account, fail, get, hex_id, method_not_allowed,
    no_route, now, ok, put, remove, str_field, values,
};

pub fn route(w: &mut WorldState, req: &ApiRequest, segs: &[&str]) -> ApiResult {
    let account_id = segs[1];
    account(w, req, account_id)?;
    let m = req.method.as_str();
    match &segs[2..] {
        ["workers", "scripts"] => match m {
            "GET" => {
                let scripts: Vec<Value> = values(w, "workers_scripts")
                    .iter()
                    .filter(|s| str_field(s, "account_id") == account_id)
                    .map(script_view)
                    .collect();
                ok(Value::Array(scripts))
            }
            _ => Err(method_not_allowed()),
        },
        ["workers", "scripts", name] => match m {
            "GET" => {
                let script = script(w, account_id, name)?;
                Ok(content_response(&script))
            }
            "PUT" => upload(w, req, account_id, name),
            "DELETE" => {
                script(w, account_id, name)?;
                remove(w, "workers_scripts", &key(account_id, name));
                ok(Value::Null)
            }
            _ => Err(method_not_allowed()),
        },
        ["workers", "scripts", name, "content", "v2"] => match m {
            "GET" => Ok(content_response(&script(w, account_id, name)?)),
            _ => Err(method_not_allowed()),
        },
        ["workers", "scripts", name, "settings"] => {
            let s = script(w, account_id, name)?;
            match m {
                "GET" => ok(settings_view(&s)),
                "PATCH" => patch_settings(w, req, s),
                _ => Err(method_not_allowed()),
            }
        }
        ["workers", "scripts", name, "secrets"] => {
            let s = script(w, account_id, name)?;
            match m {
                "GET" => ok(Value::Array(secrets(&s))),
                "PUT" => put_secret(w, req, s),
                _ => Err(method_not_allowed()),
            }
        }
        ["workers", "scripts", name, "secrets", secret] => {
            let mut s = script(w, account_id, name)?;
            let list = secrets(&s);
            let Some(found) = list
                .iter()
                .find(|x| str_field(x, "name") == *secret)
                .cloned()
            else {
                return Err(fail(404, &[(10056, "Secret not found")]));
            };
            match m {
                "GET" => ok(found),
                "DELETE" => {
                    let rest: Vec<Value> = list
                        .into_iter()
                        .filter(|x| str_field(x, "name") != *secret)
                        .collect();
                    s["secrets"] = Value::Array(rest);
                    s["modified_on"] = json!(now(w));
                    put(w, "workers_scripts", &key(account_id, name), s);
                    ok(Value::Null)
                }
                _ => Err(method_not_allowed()),
            }
        }
        _ => Err(no_route(&req.path)),
    }
}

fn key(account_id: &str, name: &str) -> String {
    format!("{account_id}/{name}")
}

fn script(w: &WorldState, account_id: &str, name: &str) -> Result<Value, ApiResponse> {
    get(w, "workers_scripts", &key(account_id, name)).ok_or_else(|| {
        fail(
            404,
            &[(10007, "This Worker does not exist on your account.")],
        )
    })
}

fn handlers(content: &str) -> Vec<&'static str> {
    let mut out = Vec::new();
    for (needle, handler) in [
        ("fetch", "fetch"),
        ("scheduled", "scheduled"),
        ("queue", "queue"),
        ("email", "email"),
        ("tail", "tail"),
    ] {
        if content.contains(needle) {
            out.push(handler);
        }
    }
    if out.is_empty() {
        out.push("fetch");
    }
    out
}

pub fn script_view(s: &Value) -> Value {
    let content = str_field(s, "content");
    json!({
        "id": s.get("id").cloned().unwrap_or_else(|| json!(str_field(s, "name"))),
        "tag": s.get("tag").cloned().unwrap_or(Value::Null),
        "etag": s.get("etag").cloned().unwrap_or(Value::Null),
        "handlers": s.get("handlers").cloned().unwrap_or_else(|| json!(handlers(content))),
        "modified_on": s["modified_on"],
        "created_on": s["created_on"],
        "usage_model": s.get("usage_model").cloned().unwrap_or(json!("standard")),
        "compatibility_date": s.get("compatibility_date").cloned().unwrap_or(json!("2025-09-01")),
        "compatibility_flags": s.get("compatibility_flags").cloned().unwrap_or(json!([])),
        "has_assets": false,
        "has_modules": s.get("has_modules").cloned().unwrap_or(json!(true)),
        "logpush": s.get("logpush").cloned().unwrap_or(json!(false)),
        "placement_mode": null,
        "tail_consumers": null,
        "last_deployed_from": s.get("last_deployed_from").cloned().unwrap_or(json!("wrangler")),
    })
}

fn content_response(s: &Value) -> ApiResponse {
    let module = s
        .get("has_modules")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    ApiResponse::raw(
        200,
        if module {
            "application/javascript+module"
        } else {
            "application/javascript"
        },
        str_field(s, "content").as_bytes().to_vec(),
    )
}

fn upload(w: &mut WorldState, req: &ApiRequest, account_id: &str, name: &str) -> ApiResult {
    if name.is_empty()
        || name.len() > 63
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    {
        return Err(fail(
            400,
            &[(
                10016,
                "Invalid Worker name. Worker names must be lowercase alphanumeric, dashes or underscores.",
            )],
        ));
    }
    let (metadata, content, has_modules) = match &req.body {
        Body::Form(parts) => {
            let metadata: Map<String, Value> = parts
                .iter()
                .find(|p| p.name == "metadata")
                .and_then(|p| serde_json::from_slice(&p.bytes).ok())
                .unwrap_or_default();
            let main = metadata
                .get("main_module")
                .or_else(|| metadata.get("body_part"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let has_modules = metadata.contains_key("main_module");
            let part = match &main {
                Some(main) => parts
                    .iter()
                    .find(|p| p.name == *main || p.filename.as_deref() == Some(main)),
                None => parts.iter().find(|p| p.name != "metadata"),
            };
            let Some(part) = part else {
                return Err(fail(
                    400,
                    &[(
                        10021,
                        &format!(
                            "Uncaught Error: No such module \"{}\".",
                            main.as_deref().unwrap_or("worker.js")
                        ),
                    )],
                ));
            };
            (metadata, part.text(), has_modules)
        }
        Body::Bytes(bytes) if req.media_type().contains("javascript") => (
            Map::new(),
            String::from_utf8_lossy(bytes).into_owned(),
            false,
        ),
        _ => {
            return Err(fail(
                400,
                &[(
                    10001,
                    "Uncaught TypeError: Content-Type must be multipart/form-data or application/javascript",
                )],
            ));
        }
    };
    let k = key(account_id, name);
    let existing = get(w, "workers_scripts", &k);
    let t = now(w);
    let etag = hex_id(w, &["script", account_id, name, &content]);
    let tag = hex_id(w, &["script-tag", account_id, name]);
    let prior = |field: &str| existing.as_ref().and_then(|e| e.get(field)).cloned();
    let mut bindings = metadata.get("bindings").cloned().unwrap_or(json!([]));
    if metadata.get("keep_bindings").is_some()
        && let Some(old) = prior("bindings")
    {
        bindings = old;
    }
    let doc = json!({
        "account_id": account_id,
        "id": name,
        "tag": tag,
        "etag": etag,
        "content": content,
        "handlers": handlers(&content),
        "has_modules": has_modules,
        "main_module": metadata.get("main_module").cloned().unwrap_or(Value::Null),
        "bindings": bindings,
        "compatibility_date": metadata.get("compatibility_date").cloned().or_else(|| prior("compatibility_date")).unwrap_or(json!("2025-09-01")),
        "compatibility_flags": metadata.get("compatibility_flags").cloned().or_else(|| prior("compatibility_flags")).unwrap_or(json!([])),
        "usage_model": metadata.get("usage_model").cloned().unwrap_or(json!("standard")),
        "logpush": metadata.get("logpush").cloned().unwrap_or(json!(false)),
        "tags": metadata.get("tags").cloned().unwrap_or(json!([])),
        "secrets": prior("secrets").unwrap_or(json!([])),
        "created_on": prior("created_on").unwrap_or_else(|| json!(t)),
        "modified_on": t,
        "last_deployed_from": "api",
    });
    put(w, "workers_scripts", &k, doc.clone());
    let mut view = script_view(&doc);
    view["startup_time_ms"] = json!(0);
    ok(view)
}

fn secrets(s: &Value) -> Vec<Value> {
    s.get("secrets")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|x| json!({ "name": x["name"], "type": x.get("type").cloned().unwrap_or(json!("secret_text")) }))
                .collect()
        })
        .unwrap_or_default()
}

fn settings_view(s: &Value) -> Value {
    let mut bindings: Vec<Value> = s
        .get("bindings")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for secret in secrets(s) {
        bindings.push(json!({ "name": secret["name"], "type": "secret_text" }));
    }
    json!({
        "bindings": bindings,
        "compatibility_date": s.get("compatibility_date").cloned().unwrap_or(json!("2025-09-01")),
        "compatibility_flags": s.get("compatibility_flags").cloned().unwrap_or(json!([])),
        "limits": s.get("limits").cloned().unwrap_or(json!({})),
        "logpush": s.get("logpush").cloned().unwrap_or(json!(false)),
        "observability": s.get("observability").cloned().unwrap_or(json!({ "enabled": false, "head_sampling_rate": null })),
        "placement": s.get("placement").cloned().unwrap_or(json!({})),
        "tags": s.get("tags").cloned().unwrap_or(json!([])),
        "tail_consumers": s.get("tail_consumers").cloned().unwrap_or(json!([])),
        "usage_model": s.get("usage_model").cloned().unwrap_or(json!("standard")),
    })
}

fn patch_settings(w: &mut WorldState, req: &ApiRequest, mut s: Value) -> ApiResult {
    let settings = match &req.body {
        Body::Form(parts) => parts
            .iter()
            .find(|p| p.name == "settings")
            .and_then(|p| serde_json::from_slice::<Map<String, Value>>(&p.bytes).ok())
            .unwrap_or_default(),
        _ => req.json_object()?,
    };
    for (k, v) in settings {
        match k.as_str() {
            "bindings" => {
                let list: Vec<Value> = v
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|b| str_field(b, "type") != "secret_text")
                    .collect();
                s["bindings"] = Value::Array(list);
            }
            "compatibility_date"
            | "compatibility_flags"
            | "logpush"
            | "observability"
            | "placement"
            | "tags"
            | "tail_consumers"
            | "usage_model"
            | "limits" => {
                s[k.as_str()] = v;
            }
            _ => {}
        }
    }
    s["modified_on"] = json!(now(w));
    let k = key(str_field(&s, "account_id"), str_field(&s, "id"));
    put(w, "workers_scripts", &k, s.clone());
    ok(settings_view(&s))
}

fn put_secret(w: &mut WorldState, req: &ApiRequest, mut s: Value) -> ApiResult {
    let body = req.json_object()?;
    let Some(name) = body.get("name").and_then(Value::as_str) else {
        return Err(fail(400, &[(10021, "name: required")]));
    };
    let kind = body
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("secret_text");
    let mut list: Vec<Value> = secrets(&s)
        .into_iter()
        .filter(|x| str_field(x, "name") != name)
        .collect();
    list.push(json!({ "name": name, "type": kind }));
    s["secrets"] = Value::Array(list);
    s["modified_on"] = json!(now(w));
    let k = key(str_field(&s, "account_id"), str_field(&s, "id"));
    put(w, "workers_scripts", &k, s);
    ok(json!({ "name": name, "type": kind }))
}
