//! Workers KV (`/accounts/{id}/storage/kv/namespaces`). Namespaces are keyed by id; values
//! by `<namespace_id>/<key>`, with `value` (text) and, when the text is JSON, `value_json`.

use mock_core::world::WorldState;
use serde_json::{Map, Value, json};

use crate::api::{
    ApiRequest, ApiResponse, ApiResult, Body, account, fail, get, hex_id, method_not_allowed,
    no_route, ok, ok_with_info, paged, put, remove, sort_by_query, str_field, values,
};

pub fn route(w: &mut WorldState, req: &ApiRequest, segs: &[&str]) -> ApiResult {
    let account_id = segs[1];
    account(w, req, account_id)?;
    let m = req.method.as_str();
    let rest = &segs[2..];
    let ["storage", "kv", "namespaces", rest @ ..] = rest else {
        return Err(no_route(&req.path));
    };
    if rest.is_empty() {
        return match m {
            "GET" => {
                let mut items: Vec<Value> = namespaces(w, account_id).iter().map(ns_view).collect();
                sort_by_query(req, &mut items, &["id", "title"]);
                paged(req, items, 20, 1000)
            }
            "POST" => create_namespace(w, req, account_id),
            _ => Err(method_not_allowed()),
        };
    }
    let ns_id = rest[0];
    let Some(ns) =
        get(w, "kv_namespaces", ns_id).filter(|n| str_field(n, "account_id") == account_id)
    else {
        return Err(fail(
            404,
            &[(10013, "get namespace: 'namespace not found'")],
        ));
    };
    match &rest[1..] {
        [] => match m {
            "GET" => ok(ns_view(&ns)),
            "PUT" => {
                let body = req.json_object()?;
                let Some(title) = body.get("title").and_then(Value::as_str) else {
                    return Err(fail(
                        400,
                        &[(10019, "rename namespace: 'title' is required")],
                    ));
                };
                let mut ns = ns;
                ns["title"] = json!(title);
                put(w, "kv_namespaces", ns_id, ns.clone());
                ok(ns_view(&ns))
            }
            "DELETE" => {
                remove(w, "kv_namespaces", ns_id);
                for (k, _) in entries(w, ns_id) {
                    remove(w, "kv_values", &k);
                }
                ok(json!({}))
            }
            _ => Err(method_not_allowed()),
        },
        ["keys"] => match m {
            "GET" => list_keys(w, req, ns_id),
            _ => Err(method_not_allowed()),
        },
        ["values", key] => match m {
            "GET" => match get(w, "kv_values", &value_key(ns_id, key)) {
                Some(v) if live(w, &v) => Ok(ApiResponse::raw(
                    200,
                    "application/octet-stream",
                    str_field(&v, "value").as_bytes().to_vec(),
                )),
                _ => Err(fail(404, &[(10009, "get: 'key not found'")])),
            },
            "PUT" => write_value(w, req, ns_id, key),
            "DELETE" => {
                remove(w, "kv_values", &value_key(ns_id, key));
                ok(json!({}))
            }
            _ => Err(method_not_allowed()),
        },
        ["metadata", key] => match m {
            "GET" => match get(w, "kv_values", &value_key(ns_id, key)) {
                Some(v) if live(w, &v) => ok(v.get("metadata").cloned().unwrap_or(Value::Null)),
                _ => Err(fail(404, &[(10009, "metadata: 'key not found'")])),
            },
            _ => Err(method_not_allowed()),
        },
        ["bulk"] => match m {
            "PUT" => bulk_put(w, req, ns_id),
            _ => Err(method_not_allowed()),
        },
        ["bulk", "delete"] => match m {
            "POST" => bulk_delete(w, req, ns_id),
            _ => Err(method_not_allowed()),
        },
        ["bulk", "get"] => match m {
            "POST" => bulk_get(w, req, ns_id),
            _ => Err(method_not_allowed()),
        },
        _ => Err(no_route(&req.path)),
    }
}

fn namespaces(w: &WorldState, account_id: &str) -> Vec<Value> {
    values(w, "kv_namespaces")
        .into_iter()
        .filter(|n| str_field(n, "account_id") == account_id)
        .collect()
}

fn ns_view(n: &Value) -> Value {
    json!({
        "id": n["id"],
        "title": n["title"],
        "supports_url_encoding": true,
        "beta": false,
    })
}

fn create_namespace(w: &mut WorldState, req: &ApiRequest, account_id: &str) -> ApiResult {
    let body = req.json_object()?;
    let Some(title) = body
        .get("title")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
    else {
        return Err(fail(
            400,
            &[(10019, "create namespace: 'title' is required")],
        ));
    };
    if namespaces(w, account_id)
        .iter()
        .any(|n| str_field(n, "title") == title)
    {
        return Err(fail(
            400,
            &[(
                10014,
                "create namespace: 'A namespace with this account ID and title already exists.'",
            )],
        ));
    }
    let id = hex_id(w, &["kv-namespace", account_id, title]);
    let doc = json!({ "id": id, "account_id": account_id, "title": title });
    put(w, "kv_namespaces", &id, doc.clone());
    ok(ns_view(&doc))
}

pub fn value_key(ns_id: &str, key: &str) -> String {
    format!("{ns_id}/{key}")
}

/// Values of a namespace sorted by key name, as KV lists them.
fn entries(w: &WorldState, ns_id: &str) -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> = crate::api::all(w, "kv_values")
        .into_iter()
        .filter(|(_, v)| str_field(v, "namespace_id") == ns_id)
        .collect();
    out.sort_by(|a, b| str_field(&a.1, "key").cmp(str_field(&b.1, "key")));
    out
}

fn live(w: &WorldState, v: &Value) -> bool {
    v.get("expiration")
        .and_then(Value::as_i64)
        .is_none_or(|e| e > w.clock.now_unix())
}

fn list_keys(w: &WorldState, req: &ApiRequest, ns_id: &str) -> ApiResult {
    let prefix = req.q("prefix").unwrap_or("");
    let limit = req
        .q("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(1000)
        .clamp(10, 1000);
    let after = req.q("cursor").filter(|c| !c.is_empty()).map(decode_cursor);
    let all: Vec<Value> = entries(w, ns_id)
        .into_iter()
        .map(|(_, v)| v)
        .filter(|v| live(w, v) && str_field(v, "key").starts_with(prefix))
        .filter(|v| after.as_deref().is_none_or(|a| str_field(v, "key") > a))
        .collect();
    let page: Vec<Value> = all
        .iter()
        .take(limit)
        .map(|v| {
            let mut item = json!({ "name": v["key"] });
            if let Some(e) = v.get("expiration").filter(|e| !e.is_null()) {
                item["expiration"] = e.clone();
            }
            if let Some(m) = v.get("metadata").filter(|m| !m.is_null()) {
                item["metadata"] = m.clone();
            }
            item
        })
        .collect();
    let cursor = if all.len() > limit {
        encode_cursor(str_field(&page[page.len() - 1], "name"))
    } else {
        String::new()
    };
    ok_with_info(
        Value::Array(page.clone()),
        json!({ "count": page.len(), "cursor": cursor }),
    )
}

fn encode_cursor(key: &str) -> String {
    key.bytes().map(|b| format!("{b:02x}")).collect()
}

fn decode_cursor(cursor: &str) -> String {
    let bytes: Vec<u8> = (0..cursor.len() / 2)
        .filter_map(|i| u8::from_str_radix(&cursor[i * 2..i * 2 + 2], 16).ok())
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn expiration(
    w: &WorldState,
    expiration: Option<&Value>,
    ttl: Option<&Value>,
) -> Result<Option<i64>, ApiResponse> {
    let num = |v: &Value| {
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    };
    if let Some(ttl) = ttl.and_then(num) {
        if ttl < 60 {
            return Err(fail(
                400,
                &[(
                    10033,
                    &format!(
                        "Invalid expiration_ttl of {ttl}. Expiration TTL must be at least 60."
                    ),
                )],
            ));
        }
        return Ok(Some(w.clock.now_unix() + ttl));
    }
    if let Some(e) = expiration.and_then(num) {
        if e < w.clock.now_unix() + 60 {
            return Err(fail(
                400,
                &[(
                    10033,
                    "Invalid expiration. Expiration times must be at least 60 seconds in the future.",
                )],
            ));
        }
        return Ok(Some(e));
    }
    Ok(None)
}

fn store_value(
    w: &mut WorldState,
    ns_id: &str,
    key: &str,
    value: String,
    metadata: Option<Value>,
    expiration: Option<i64>,
) {
    let mut doc = Map::new();
    doc.insert("namespace_id".into(), json!(ns_id));
    doc.insert("key".into(), json!(key));
    if let Ok(parsed) = serde_json::from_str::<Value>(&value)
        && (parsed.is_object() || parsed.is_array() || parsed.is_boolean() || parsed.is_number())
    {
        doc.insert("value_json".into(), parsed);
    }
    doc.insert("value".into(), json!(value));
    if let Some(m) = metadata.filter(|m| !m.is_null()) {
        doc.insert("metadata".into(), m);
    }
    if let Some(e) = expiration {
        doc.insert("expiration".into(), json!(e));
    }
    put(w, "kv_values", &value_key(ns_id, key), Value::Object(doc));
}

fn check_key(key: &str) -> Result<(), ApiResponse> {
    if key.is_empty() || key.len() > 512 || key == "." || key == ".." {
        return Err(fail(400, &[(10024, "Invalid key name.")]));
    }
    Ok(())
}

fn write_value(w: &mut WorldState, req: &ApiRequest, ns_id: &str, key: &str) -> ApiResult {
    check_key(key)?;
    let (value, metadata) = match &req.body {
        Body::Form(parts) => {
            let value = parts
                .iter()
                .find(|p| p.name == "value")
                .map(|p| p.text())
                .unwrap_or_default();
            let metadata = parts
                .iter()
                .find(|p| p.name == "metadata")
                .map(|p| serde_json::from_slice(&p.bytes).unwrap_or(Value::String(p.text())));
            (value, metadata)
        }
        Body::Bytes(b) => (String::from_utf8_lossy(b).into_owned(), None),
        Body::None => (String::new(), None),
    };
    let exp = expiration(
        w,
        req.q("expiration").map(|s| json!(s)).as_ref(),
        req.q("expiration_ttl").map(|s| json!(s)).as_ref(),
    )?;
    store_value(w, ns_id, key, value, metadata, exp);
    ok(json!({}))
}

fn bulk_put(w: &mut WorldState, req: &ApiRequest, ns_id: &str) -> ApiResult {
    let Value::Array(items) = req.json()? else {
        return Err(fail(
            400,
            &[(10026, "bulk write: request body must be an array")],
        ));
    };
    if items.len() > 10_000 {
        return Err(fail(
            400,
            &[(10026, "bulk write: too many keys (max 10000)")],
        ));
    }
    let mut parsed = Vec::new();
    for item in &items {
        let key = str_field(item, "key").to_owned();
        check_key(&key)?;
        let mut value = item
            .get("value")
            .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned))
            .unwrap_or_default();
        if item.get("base64").and_then(Value::as_bool) == Some(true) {
            value = String::from_utf8_lossy(&base64_decode(&value)).into_owned();
        }
        let exp = expiration(w, item.get("expiration"), item.get("expiration_ttl"))?;
        parsed.push((key, value, item.get("metadata").cloned(), exp));
    }
    let n = parsed.len();
    for (key, value, metadata, exp) in parsed {
        store_value(w, ns_id, &key, value, metadata, exp);
    }
    ok(json!({ "successful_key_count": n, "unsuccessful_keys": [] }))
}

fn bulk_delete(w: &mut WorldState, req: &ApiRequest, ns_id: &str) -> ApiResult {
    let Value::Array(keys) = req.json()? else {
        return Err(fail(
            400,
            &[(10026, "bulk delete: request body must be an array of keys")],
        ));
    };
    for k in &keys {
        if let Some(k) = k.as_str() {
            remove(w, "kv_values", &value_key(ns_id, k));
        }
    }
    ok(json!({ "successful_key_count": keys.len(), "unsuccessful_keys": [] }))
}

fn bulk_get(w: &WorldState, req: &ApiRequest, ns_id: &str) -> ApiResult {
    let body = req.json_object()?;
    let keys: Vec<String> = body
        .get("keys")
        .and_then(Value::as_array)
        .map(|k| {
            k.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if keys.is_empty() || keys.len() > 100 {
        return Err(fail(
            400,
            &[(10026, "bulk get: between 1 and 100 keys are required")],
        ));
    }
    let as_json = body.get("type").and_then(Value::as_str) == Some("json");
    let with_metadata = body.get("withMetadata").and_then(Value::as_bool) == Some(true);
    let mut out = Map::new();
    for key in keys {
        let found = get(w, "kv_values", &value_key(ns_id, &key)).filter(|v| live(w, v));
        let value = match &found {
            None => Value::Null,
            Some(v) if as_json => match serde_json::from_str::<Value>(str_field(v, "value")) {
                Ok(parsed) => parsed,
                Err(_) => {
                    return Err(fail(
                        400,
                        &[(
                            10026,
                            &format!("bulk get: value of key '{key}' is not valid JSON"),
                        )],
                    ));
                }
            },
            Some(v) => v["value"].clone(),
        };
        if with_metadata {
            let mut entry = json!({
                "value": value,
                "metadata": found.as_ref().and_then(|v| v.get("metadata").cloned()).unwrap_or(Value::Null),
            });
            if let Some(e) = found.as_ref().and_then(|v| v.get("expiration")) {
                entry["expiration"] = e.clone();
            }
            out.insert(key, if found.is_some() { entry } else { Value::Null });
        } else {
            out.insert(key, value);
        }
    }
    ok(json!({ "values": out }))
}

fn base64_decode(text: &str) -> Vec<u8> {
    const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let vals: Vec<u32> = text
        .bytes()
        .filter_map(|c| B64.iter().position(|b| *b == c).map(|p| p as u32))
        .collect();
    let mut out = Vec::new();
    for chunk in vals.chunks(4) {
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
