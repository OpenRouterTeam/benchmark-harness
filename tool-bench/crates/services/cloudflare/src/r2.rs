//! R2 buckets (`/accounts/{id}/r2/buckets`), keyed `<account_id>/<bucket>`.

use mock_core::world::WorldState;
use serde_json::{Value, json};

use crate::api::{
    ApiRequest, ApiResult, account, fail, get, method_not_allowed, no_route, ok, ok_with_info, put,
    remove, str_field, values,
};

const LOCATIONS: &[&str] = &["apac", "eeur", "enam", "weur", "wnam", "oc"];

pub fn route(w: &mut WorldState, req: &ApiRequest, segs: &[&str]) -> ApiResult {
    let account_id = segs[1];
    account(w, req, account_id)?;
    let m = req.method.as_str();
    match &segs[2..] {
        ["r2", "buckets"] => match m {
            "GET" => list(w, req, account_id),
            "POST" => create(w, req, account_id),
            _ => Err(method_not_allowed()),
        },
        ["r2", "buckets", name] => {
            let key = format!("{account_id}/{name}");
            let Some(bucket) = get(w, "r2_buckets", &key) else {
                return Err(fail(
                    404,
                    &[(10006, "The specified bucket does not exist.")],
                ));
            };
            match m {
                "GET" => ok(view(&bucket)),
                "DELETE" => {
                    if bucket
                        .get("object_count")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        > 0
                    {
                        return Err(fail(
                            409,
                            &[(10008, "The bucket you tried to delete is not empty.")],
                        ));
                    }
                    remove(w, "r2_buckets", &key);
                    ok(json!({}))
                }
                _ => Err(method_not_allowed()),
            }
        }
        _ => Err(no_route(&req.path)),
    }
}

fn view(b: &Value) -> Value {
    json!({
        "name": b["name"],
        "creation_date": b["creation_date"],
        "location": b.get("location").cloned().unwrap_or(json!("enam")),
        "storage_class": b.get("storage_class").cloned().unwrap_or(json!("Standard")),
        "jurisdiction": b.get("jurisdiction").cloned().unwrap_or(json!("default")),
    })
}

fn list(w: &WorldState, req: &ApiRequest, account_id: &str) -> ApiResult {
    let mut buckets: Vec<Value> = values(w, "r2_buckets")
        .into_iter()
        .filter(|b| str_field(b, "account_id") == account_id)
        .filter(|b| {
            req.q("name_contains")
                .is_none_or(|n| str_field(b, "name").contains(n))
        })
        .collect();
    buckets.sort_by(|a, b| str_field(a, "name").cmp(str_field(b, "name")));
    if req.q("direction") == Some("desc") {
        buckets.reverse();
    }
    let after = req.q("cursor").or(req.q("start_after"));
    let buckets: Vec<Value> = match after {
        Some(a) => buckets
            .into_iter()
            .skip_while(|b| str_field(b, "name") != a)
            .skip(1)
            .collect(),
        None => buckets,
    };
    let per_page = req
        .q("per_page")
        .and_then(|p| p.parse::<usize>().ok())
        .unwrap_or(20)
        .clamp(1, 1000);
    let page: Vec<Value> = buckets.iter().take(per_page).map(view).collect();
    let mut info = json!({ "per_page": per_page });
    if buckets.len() > per_page {
        info["cursor"] = page[page.len() - 1]["name"].clone();
    }
    ok_with_info(json!({ "buckets": page }), info)
}

fn valid_name(name: &str) -> bool {
    (3..=63).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

fn create(w: &mut WorldState, req: &ApiRequest, account_id: &str) -> ApiResult {
    let body = req.json_object()?;
    let name = body.get("name").and_then(Value::as_str).unwrap_or("");
    if !valid_name(name) {
        return Err(fail(
            400,
            &[(10005, "The specified bucket name is not valid.")],
        ));
    }
    let key = format!("{account_id}/{name}");
    if get(w, "r2_buckets", &key).is_some() {
        return Err(fail(
            409,
            &[(
                10004,
                "The bucket you tried to create already exists, and you own it.",
            )],
        ));
    }
    let location = body
        .get("locationHint")
        .and_then(Value::as_str)
        .map(str::to_lowercase)
        .filter(|l| LOCATIONS.contains(&l.as_str()))
        .unwrap_or_else(|| "enam".to_owned());
    let storage_class = body
        .get("storageClass")
        .and_then(Value::as_str)
        .unwrap_or("Standard");
    if !matches!(storage_class, "Standard" | "InfrequentAccess") {
        return Err(fail(
            400,
            &[(10040, "The storage class specified is not valid.")],
        ));
    }
    let created = w.clock.now_rfc3339().replace('Z', ".000Z");
    let doc = json!({
        "account_id": account_id,
        "name": name,
        "creation_date": created,
        "location": location,
        "storage_class": storage_class,
        "jurisdiction": "default",
        "object_count": 0,
    });
    put(w, "r2_buckets", &key, doc.clone());
    ok(view(&doc))
}
