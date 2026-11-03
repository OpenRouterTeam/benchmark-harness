//! Mutation resolvers. Each validates first and writes only when there are no user
//! errors, then returns its payload object (with `__ref`s resolved after the write).

use mock_core::store::Store;
use serde_json::{Map, Value, json};

use crate::model::{
    self, default_option, find, id_of, is_default_option, key_of, normalize_product, parse_gid,
    reference, variant_title,
};

/// Runs the named root mutation field; `None` if no such mutation exists.
pub fn execute(
    store: &mut Store,
    name: &str,
    args: &Map<String, Value>,
    now: &str,
) -> Option<Value> {
    Some(match name {
        "orderUpdate" => order_update(store, args, now),
        "customerUpdate" => customer_update(store, args, now),
        "productCreate" => product_create(store, args, now),
        "productUpdate" => product_update(store, args, now),
        "productDelete" => product_delete(store, args),
        "productVariantsBulkCreate" => variants_bulk_create(store, args, now),
        "productVariantsBulkUpdate" => variants_bulk_update(store, args, now),
        "productVariantsBulkDelete" => variants_bulk_delete(store, args, now),
        "productOptionsCreate" => options_create(store, args, now),
        "productOptionUpdate" => option_update(store, args, now),
        "productOptionsDelete" => options_delete(store, args, now),
        _ => return None,
    })
}

fn field(parts: &[&str]) -> Value {
    Value::Array(parts.iter().map(|p| json!(p)).collect())
}

fn user_error(parts: &[&str], message: impl Into<String>) -> Value {
    json!({"__typename": "UserError", "field": field(parts), "message": message.into()})
}

fn coded_error(type_name: &str, parts: &[&str], message: impl Into<String>, code: &str) -> Value {
    json!({"__typename": type_name, "field": field(parts), "message": message.into(), "code": code})
}

fn gid_of(v: &Value, kind: &str) -> Option<u64> {
    v.as_str()
        .and_then(parse_gid)
        .filter(|(k, _)| *k == kind)
        .map(|(_, id)| id)
}

// ---------------------------------------------------------------------------------------
// ID allocation (dataset max + 1)

struct Ids {
    product: u64,
    variant: u64,
    option: u64,
    value: u64,
    metafield: u64,
}

impl Ids {
    fn new(store: &Store) -> Self {
        let mut ids = Self {
            product: 0,
            variant: 0,
            option: 0,
            value: 0,
            metafield: 0,
        };
        let metafields = |doc: &Value, max: &mut u64| {
            for m in doc["metafields"].as_array().into_iter().flatten() {
                *max = (*max).max(id_of(m));
            }
        };
        // Raw documents: the ids `normalize_product` derives for a missing default option
        // must not push real allocations up.
        for p in model::docs(store, "products") {
            ids.product = ids.product.max(id_of(p));
            metafields(p, &mut ids.metafield);
            for v in p["variants"].as_array().into_iter().flatten() {
                ids.variant = ids.variant.max(id_of(v));
                metafields(v, &mut ids.metafield);
            }
            for o in p["options"].as_array().into_iter().flatten() {
                ids.option = ids.option.max(id_of(o));
                for v in o["values"].as_array().into_iter().flatten() {
                    ids.value = ids.value.max(id_of(v));
                }
            }
        }
        for c in ["orders", "customers"] {
            for d in model::docs(store, c) {
                metafields(d, &mut ids.metafield);
            }
        }
        ids
    }

    fn next(slot: &mut u64) -> u64 {
        *slot += 1;
        *slot
    }
}

// ---------------------------------------------------------------------------------------
// Shared input handling

/// Shopify splits tags on commas, trims them and drops duplicates.
fn tags(v: &Value) -> Value {
    let raw: Vec<String> = match v {
        Value::Array(a) => a
            .iter()
            .filter_map(|t| t.as_str().map(str::to_owned))
            .collect(),
        Value::String(s) => vec![s.clone()],
        _ => Vec::new(),
    };
    let mut out: Vec<String> = Vec::new();
    for tag in raw.iter().flat_map(|t| t.split(',')) {
        let tag = tag.trim();
        if !tag.is_empty() && !out.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
            out.push(tag.to_owned());
        }
    }
    json!(out)
}

fn text(v: &Value) -> Value {
    match v {
        Value::String(s) if s.is_empty() => Value::Null,
        Value::String(_) => v.clone(),
        _ => Value::Null,
    }
}

/// Upserts metafields on `doc` (by id, else by namespace + key; new ones need a type).
fn apply_metafields(
    doc: &mut Value,
    inputs: &Value,
    ids: &mut Ids,
    now: &str,
    errors: &mut Vec<Value>,
) {
    let Some(inputs) = inputs.as_array() else {
        return;
    };
    if !doc["metafields"].is_array() {
        doc["metafields"] = json!([]);
    }
    for (i, input) in inputs.iter().enumerate() {
        let idx = i.to_string();
        let value = input["value"].as_str().unwrap_or_default().to_owned();
        let list = doc["metafields"].as_array_mut().expect("metafields array");
        let existing = if let Some(gid) = input["id"].as_str() {
            match gid_of(&json!(gid), "Metafield")
                .and_then(|mid| list.iter().position(|m| id_of(m) == mid))
            {
                Some(pos) => Some(pos),
                None => {
                    errors.push(user_error(
                        &["metafields", &idx, "id"],
                        "Metafield does not exist",
                    ));
                    continue;
                }
            }
        } else {
            let ns = input["namespace"].as_str().unwrap_or("custom");
            let Some(key) = input["key"].as_str().filter(|k| !k.is_empty()) else {
                errors.push(user_error(
                    &["metafields", &idx, "key"],
                    "Key can't be blank",
                ));
                continue;
            };
            list.iter()
                .position(|m| m["namespace"] == ns && m["key"] == key)
        };
        let ty = input["type"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| existing.and_then(|p| list[p]["type"].as_str().map(str::to_owned)));
        let Some(ty) = ty else {
            errors.push(user_error(
                &["metafields", &idx, "type"],
                "Type can't be blank",
            ));
            continue;
        };
        if let Some(problem) = metafield_value_problem(&ty, &value) {
            errors.push(user_error(&["metafields", &idx, "value"], problem));
            continue;
        }
        match existing {
            Some(pos) => {
                let m = &mut list[pos];
                m["value"] = json!(value);
                m["type"] = json!(ty);
                m["updatedAt"] = json!(now);
            }
            None => list.push(json!({
                "id": Ids::next(&mut ids.metafield),
                "namespace": input["namespace"].as_str().unwrap_or("custom"),
                "key": input["key"],
                "value": value,
                "type": ty,
                "createdAt": now,
                "updatedAt": now,
            })),
        }
    }
}

fn metafield_value_problem(ty: &str, value: &str) -> Option<String> {
    match ty {
        "number_integer" if value.parse::<i64>().is_err() => {
            Some("Value must be an integer.".into())
        }
        "number_decimal" if value.parse::<f64>().is_err() => {
            Some("Value must be a decimal.".into())
        }
        "boolean" if value != "true" && value != "false" => {
            Some("Value must be true or false.".into())
        }
        "json" if serde_json::from_str::<Value>(value).is_err() => {
            Some("Value must be valid JSON.".into())
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------
// Orders and customers

fn order_update(store: &mut Store, args: &Map<String, Value>, now: &str) -> Value {
    let payload = |order: Value, errors: Vec<Value>| json!({"__typename": "OrderUpdatePayload", "order": order, "userErrors": errors});
    let input = args.get("input").cloned().unwrap_or(Value::Null);
    let Some((id, key)) =
        gid_of(&input["id"], "Order").and_then(|id| key_of(store, "orders", id).map(|k| (id, k)))
    else {
        return payload(
            Value::Null,
            vec![user_error(&["id"], "Order does not exist")],
        );
    };
    let mut ids = Ids::new(store);
    let mut doc = store
        .collection("orders")
        .and_then(|c| c.get(&key))
        .cloned()
        .unwrap_or_default();
    let mut errors = Vec::new();
    if let Some(t) = input.get("tags") {
        doc["tags"] = tags(t);
    }
    if let Some(email) = input.get("email") {
        match email.as_str() {
            Some(e) if !e.contains('@') => errors.push(user_error(&["email"], "Email is invalid")),
            _ => doc["email"] = text(email),
        }
    }
    if let Some(note) = input.get("note") {
        doc["note"] = text(note);
    }
    if let Some(attrs) = input.get("customAttributes") {
        doc["customAttributes"] = json!(
            attrs
                .as_array()
                .into_iter()
                .flatten()
                .map(|a| json!({"key": a["key"], "value": a["value"]}))
                .collect::<Vec<_>>()
        );
    }
    if let Some(addr) = input.get("shippingAddress").and_then(Value::as_object) {
        if !doc["shippingAddress"].is_object() {
            doc["shippingAddress"] = json!({});
        }
        for (k, v) in addr {
            doc["shippingAddress"][k] = v.clone();
            if k == "province"
                && let Some(code) = v
                    .as_str()
                    .filter(|s| s.len() == 2 && s.chars().all(|c| c.is_ascii_uppercase()))
            {
                doc["shippingAddress"]["provinceCode"] = json!(code);
            }
        }
    }
    if let Some(mf) = input.get("metafields") {
        apply_metafields(&mut doc, mf, &mut ids, now, &mut errors);
    }
    if !errors.is_empty() {
        return payload(Value::Null, errors);
    }
    doc["updatedAt"] = json!(now);
    store.collection_mut("orders").insert(key, doc);
    payload(reference("Order", id), Vec::new())
}

/// E.164 form of a phone number, or `None` if it isn't one.
fn normalize_phone(phone: &str) -> Option<String> {
    let plus = phone.trim_start().starts_with('+');
    let digits: String = phone.chars().filter(char::is_ascii_digit).collect();
    let allowed = phone
        .chars()
        .all(|c| c.is_ascii_digit() || " +-().".contains(c));
    if !allowed {
        return None;
    }
    match (plus, digits.len()) {
        (true, 8..=15) => Some(format!("+{digits}")),
        (false, 10) => Some(format!("+1{digits}")),
        (false, 11) if digits.starts_with('1') => Some(format!("+{digits}")),
        _ => None,
    }
}

fn customer_update(store: &mut Store, args: &Map<String, Value>, now: &str) -> Value {
    let payload = |customer: Value, errors: Vec<Value>| json!({"__typename": "CustomerUpdatePayload", "customer": customer, "userErrors": errors});
    let input = args.get("input").cloned().unwrap_or(Value::Null);
    let Some((id, key)) = gid_of(&input["id"], "Customer")
        .and_then(|id| key_of(store, "customers", id).map(|k| (id, k)))
    else {
        return payload(
            Value::Null,
            vec![user_error(&["id"], "Customer does not exist")],
        );
    };
    let mut ids = Ids::new(store);
    let mut doc = store
        .collection("customers")
        .and_then(|c| c.get(&key))
        .cloned()
        .unwrap_or_default();
    let mut errors = Vec::new();
    for name in ["firstName", "lastName", "note", "locale"] {
        if let Some(v) = input.get(name) {
            doc[name] = text(v);
        }
    }
    if let Some(email) = input.get("email") {
        let e = email.as_str().unwrap_or_default().trim().to_lowercase();
        let taken = model::docs(store, "customers").into_iter().any(|c| {
            id_of(c) != id
                && c["email"]
                    .as_str()
                    .is_some_and(|ce| ce.eq_ignore_ascii_case(&e))
        });
        if !e.is_empty() && !e.contains('@') {
            errors.push(user_error(&["email"], "Email is invalid"));
        } else if taken {
            errors.push(user_error(&["email"], "Email has already been taken"));
        } else {
            doc["email"] = if e.is_empty() { Value::Null } else { json!(e) };
        }
    }
    if let Some(phone) = input.get("phone") {
        match phone.as_str().filter(|p| !p.trim().is_empty()) {
            None => doc["phone"] = Value::Null,
            Some(p) => match normalize_phone(p) {
                None => errors.push(user_error(&["phone"], "Phone is invalid")),
                Some(e164) => {
                    let taken = model::docs(store, "customers")
                        .into_iter()
                        .any(|c| id_of(c) != id && c["phone"].as_str() == Some(e164.as_str()));
                    if taken {
                        errors.push(user_error(&["phone"], "Phone has already been taken"));
                    } else {
                        doc["phone"] = json!(e164);
                    }
                }
            },
        }
    }
    if let Some(t) = input.get("tags") {
        doc["tags"] = tags(t);
    }
    if let Some(v) = input.get("taxExempt").and_then(Value::as_bool) {
        doc["taxExempt"] = json!(v);
    }
    if let Some(addresses) = input.get("addresses").and_then(Value::as_array) {
        doc["addresses"] = json!(addresses);
    }
    if let Some(mf) = input.get("metafields") {
        apply_metafields(&mut doc, mf, &mut ids, now, &mut errors);
    }
    if !errors.is_empty() {
        return payload(Value::Null, errors);
    }
    doc["updatedAt"] = json!(now);
    store.collection_mut("customers").insert(key, doc);
    payload(reference("Customer", id), Vec::new())
}

// ---------------------------------------------------------------------------------------
// Products

fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

fn handle_taken(store: &Store, handle: &str, except: Option<u64>) -> bool {
    model::docs(store, "products")
        .into_iter()
        .any(|p| Some(id_of(p)) != except && p["handle"] == handle)
}

fn unique_handle(store: &Store, base: &str) -> String {
    let base = if base.is_empty() {
        "product".to_owned()
    } else {
        base.to_owned()
    };
    if !handle_taken(store, &base, None) {
        return base;
    }
    (1..)
        .map(|n| format!("{base}-{n}"))
        .find(|h| !handle_taken(store, h, None))
        .expect("an unused handle")
}

fn collection_ids(store: &Store, v: &Value, field_name: &str, errors: &mut Vec<Value>) -> Vec<u64> {
    let mut out = Vec::new();
    for (i, c) in v.as_array().into_iter().flatten().enumerate() {
        match gid_of(c, "Collection").filter(|id| find(store, "collections", *id).is_some()) {
            Some(id) => out.push(id),
            None => errors.push(user_error(
                &[field_name, &i.to_string()],
                "Collection does not exist",
            )),
        }
    }
    out
}

fn product_create(store: &mut Store, args: &Map<String, Value>, now: &str) -> Value {
    let payload = |product: Value, errors: Vec<Value>| json!({"__typename": "ProductCreatePayload", "product": product, "userErrors": errors});
    let input = args
        .get("product")
        .or_else(|| args.get("input"))
        .cloned()
        .unwrap_or(Value::Null);
    let mut errors = Vec::new();
    let title = input["title"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_owned();
    if title.is_empty() {
        errors.push(user_error(&["title"], "Title can't be blank"));
    }
    let handle = match input["handle"].as_str().filter(|h| !h.is_empty()) {
        Some(h) => {
            let h = slugify(h);
            if handle_taken(store, &h, None) {
                errors.push(user_error(
                    &["handle"],
                    format!("Handle '{h}' already in use. Please provide a new handle."),
                ));
            }
            h
        }
        None => unique_handle(store, &slugify(&title)),
    };
    let mut ids = Ids::new(store);
    let id = Ids::next(&mut ids.product);
    let option_inputs = input["productOptions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if option_inputs.len() > 3 {
        errors.push(user_error(
            &["productOptions"],
            "Can only specify a maximum of 3 options",
        ));
    }
    let mut options = Vec::new();
    for (i, o) in option_inputs.iter().enumerate() {
        let name = o["name"].as_str().unwrap_or_default().trim().to_owned();
        let values: Vec<String> = o["values"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v["name"].as_str().map(str::to_owned))
            .collect();
        if name.is_empty() {
            errors.push(user_error(
                &["productOptions", &i.to_string(), "name"],
                "Name can't be blank",
            ));
        } else if values.is_empty() {
            errors.push(user_error(
                &["productOptions", &i.to_string(), "values"],
                format!("Option '{name}' must specify at least one option value."),
            ));
        }
        options.push(json!({
            "id": Ids::next(&mut ids.option),
            "name": name,
            "position": i + 1,
            "values": values.iter().map(|v| json!({"id": Ids::next(&mut ids.value), "name": v})).collect::<Vec<_>>(),
        }));
    }
    if options.is_empty() {
        options.push(default_option(
            Ids::next(&mut ids.option),
            Ids::next(&mut ids.value),
        ));
    }
    let selected: Vec<Value> = options
        .iter()
        .map(|o| json!({"name": o["name"], "value": o["values"][0]["name"]}))
        .collect();
    let collections = collection_ids(
        store,
        &input["collectionsToJoin"],
        "collectionsToJoin",
        &mut errors,
    );
    let status = input["status"].as_str().unwrap_or("ACTIVE").to_owned();
    let shop_name = model::shop(store)["name"]
        .as_str()
        .unwrap_or("Octo Corp")
        .to_owned();
    let mut doc = json!({
        "id": id,
        "title": title,
        "handle": handle,
        "descriptionHtml": input["descriptionHtml"].as_str().unwrap_or_default(),
        "vendor": input["vendor"].as_str().filter(|v| !v.is_empty()).unwrap_or(&shop_name),
        "productType": input["productType"].as_str().unwrap_or_default(),
        "status": status,
        "tags": tags(&input["tags"]),
        "createdAt": now,
        "updatedAt": now,
        "publishedAt": if status == "ACTIVE" { json!(now) } else { Value::Null },
        "seo": {"title": text(&input["seo"]["title"]), "description": text(&input["seo"]["description"])},
        "options": options,
        "variants": [{
            "id": Ids::next(&mut ids.variant),
            "title": variant_title(&selected),
            "price": "0.00",
            "compareAtPrice": null,
            "sku": null,
            "barcode": null,
            "taxable": true,
            "tracked": false,
            "inventoryQuantity": 0,
            "selectedOptions": selected,
            "createdAt": now,
            "updatedAt": now,
        }],
        "images": [],
        "collections": collections,
        "metafields": [],
    });
    apply_metafields(&mut doc, &input["metafields"], &mut ids, now, &mut errors);
    if !errors.is_empty() {
        return payload(Value::Null, errors);
    }
    store.collection_mut("products").insert(id.to_string(), doc);
    payload(reference("Product", id), Vec::new())
}

fn product_update(store: &mut Store, args: &Map<String, Value>, now: &str) -> Value {
    let payload = |product: Value, errors: Vec<Value>| json!({"__typename": "ProductUpdatePayload", "product": product, "userErrors": errors});
    let input = args
        .get("input")
        .or_else(|| args.get("product"))
        .cloned()
        .unwrap_or(Value::Null);
    let Some((id, key)) = gid_of(&input["id"], "Product")
        .and_then(|id| key_of(store, "products", id).map(|k| (id, k)))
    else {
        return payload(
            Value::Null,
            vec![user_error(&["id"], "Product does not exist")],
        );
    };
    let mut ids = Ids::new(store);
    let mut doc = store
        .collection("products")
        .and_then(|c| c.get(&key))
        .cloned()
        .unwrap_or_default();
    normalize_product(&mut doc);
    let mut errors = Vec::new();
    if let Some(t) = input.get("title") {
        let t = t.as_str().unwrap_or_default().trim();
        if t.is_empty() {
            errors.push(user_error(&["title"], "Title can't be blank"));
        } else {
            doc["title"] = json!(t);
        }
    }
    if let Some(h) = input.get("handle").and_then(Value::as_str) {
        let h = slugify(h);
        if handle_taken(store, &h, Some(id)) {
            errors.push(user_error(
                &["handle"],
                format!("Handle '{h}' already in use. Please provide a new handle."),
            ));
        } else {
            doc["handle"] = json!(h);
        }
    }
    for name in ["descriptionHtml", "vendor", "productType"] {
        if let Some(v) = input.get(name) {
            doc[name] = json!(v.as_str().unwrap_or_default());
        }
    }
    if let Some(t) = input.get("tags") {
        doc["tags"] = tags(t);
    }
    if let Some(status) = input.get("status").and_then(Value::as_str) {
        doc["status"] = json!(status);
        if status == "ACTIVE" && doc["publishedAt"].is_null() {
            doc["publishedAt"] = json!(now);
        }
    }
    if let Some(seo) = input.get("seo").and_then(Value::as_object) {
        if !doc["seo"].is_object() {
            doc["seo"] = json!({"title": null, "description": null});
        }
        for k in ["title", "description"] {
            if let Some(v) = seo.get(k) {
                doc["seo"][k] = text(v);
            }
        }
    }
    let join = collection_ids(
        store,
        input.get("collectionsToJoin").unwrap_or(&Value::Null),
        "collectionsToJoin",
        &mut errors,
    );
    let leave = collection_ids(
        store,
        input.get("collectionsToLeave").unwrap_or(&Value::Null),
        "collectionsToLeave",
        &mut errors,
    );
    if !join.is_empty() || !leave.is_empty() {
        let mut current: Vec<u64> = doc["collections"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .collect();
        for c in join {
            if !current.contains(&c) {
                current.push(c);
            }
        }
        current.retain(|c| !leave.contains(c));
        doc["collections"] = json!(current);
    }
    if let Some(mf) = input.get("metafields") {
        apply_metafields(&mut doc, mf, &mut ids, now, &mut errors);
    }
    if !errors.is_empty() {
        return payload(Value::Null, errors);
    }
    doc["updatedAt"] = json!(now);
    store.collection_mut("products").insert(key, doc);
    payload(reference("Product", id), Vec::new())
}

fn product_delete(store: &mut Store, args: &Map<String, Value>) -> Value {
    let input = args.get("input").cloned().unwrap_or(Value::Null);
    let found = gid_of(&input["id"], "Product")
        .and_then(|id| key_of(store, "products", id).map(|k| (id, k)));
    match found {
        None => json!({
            "__typename": "ProductDeletePayload",
            "deletedProductId": null,
            "shop": model::shop(store),
            "userErrors": [user_error(&["id"], "Product does not exist")],
        }),
        Some((id, key)) => {
            store.collection_mut("products").remove(&key);
            json!({
                "__typename": "ProductDeletePayload",
                "deletedProductId": model::gid("Product", id),
                "shop": model::shop(store),
                "userErrors": [],
            })
        }
    }
}

// ---------------------------------------------------------------------------------------
// Variants and options

/// The product named by `args.productId`, normalized, with its store key.
fn product_arg(store: &Store, args: &Map<String, Value>) -> Option<(u64, String, Value)> {
    let id = gid_of(args.get("productId")?, "Product")?;
    let key = key_of(store, "products", id)?;
    let mut doc = store.collection("products")?.get(&key)?.clone();
    normalize_product(&mut doc);
    Some((id, key, doc))
}

/// Reorders each variant's selected options to the option order and refreshes titles.
fn sync_variants(doc: &mut Value) {
    let names: Vec<String> = doc["options"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| o["name"].as_str().map(str::to_owned))
        .collect();
    for (n, o) in doc["options"]
        .as_array_mut()
        .into_iter()
        .flatten()
        .enumerate()
    {
        o["position"] = json!(n + 1);
    }
    for v in doc["variants"].as_array_mut().into_iter().flatten() {
        let selected = v["selectedOptions"].as_array().cloned().unwrap_or_default();
        let ordered: Vec<Value> = names
            .iter()
            .filter_map(|name| {
                selected
                    .iter()
                    .find(|s| s["name"] == name.as_str())
                    .cloned()
            })
            .collect();
        v["title"] = json!(variant_title(&ordered));
        v["selectedOptions"] = json!(ordered);
    }
}

fn combo(selected: &Value) -> Vec<(String, String)> {
    let mut c: Vec<(String, String)> = selected
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| {
            (
                s["name"].as_str().unwrap_or_default().to_lowercase(),
                s["value"].as_str().unwrap_or_default().to_lowercase(),
            )
        })
        .collect();
    c.sort();
    c
}

const VARIANT_ERR: &str = "ProductVariantsBulkCreateUserError";

/// Resolves `optionValues` input into selected options, adding new values to existing
/// options (or new options when `allow_new_options`).
fn resolve_option_values(
    doc: &mut Value,
    inputs: &[Value],
    allow_new_options: bool,
    ids: &mut Ids,
    error_type: &str,
    prefix: &[&str],
    errors: &mut Vec<Value>,
) -> Vec<Value> {
    let mut selected = Vec::new();
    for (j, ov) in inputs.iter().enumerate() {
        let j = j.to_string();
        let option_name = ov["optionName"].as_str().map(str::to_owned).or_else(|| {
            let oid = gid_of(&ov["optionId"], "ProductOption")?;
            doc["options"]
                .as_array()?
                .iter()
                .find(|o| id_of(o) == oid)?["name"]
                .as_str()
                .map(str::to_owned)
        });
        let Some(option_name) = option_name else {
            let mut path = prefix.to_vec();
            path.extend(["optionValues", &j, "optionName"]);
            errors.push(coded_error(
                error_type,
                &path,
                "Option does not exist",
                "OPTION_DOES_NOT_EXIST",
            ));
            continue;
        };
        let value_name = ov["name"].as_str().unwrap_or_default().to_owned();
        let options = doc["options"].as_array_mut().expect("options array");
        let pos = options.iter().position(|o| {
            o["name"]
                .as_str()
                .is_some_and(|n| n.eq_ignore_ascii_case(&option_name))
        });
        let pos = match pos {
            Some(p) => p,
            None if allow_new_options && options.len() < 3 => {
                options.push(json!({"id": Ids::next(&mut ids.option), "name": option_name, "position": options.len() + 1, "values": []}));
                options.len() - 1
            }
            None => {
                let mut path = prefix.to_vec();
                path.extend(["optionValues", &j, "optionName"]);
                errors.push(coded_error(
                    error_type,
                    &path,
                    "Option does not exist",
                    "OPTION_DOES_NOT_EXIST",
                ));
                continue;
            }
        };
        let option = &mut options[pos];
        let canonical_name = option["name"].as_str().unwrap_or_default().to_owned();
        let values = option["values"].as_array_mut().expect("values array");
        let existing = values
            .iter()
            .find(|v| {
                v["name"]
                    .as_str()
                    .is_some_and(|n| n.eq_ignore_ascii_case(&value_name))
            })
            .and_then(|v| v["name"].as_str().map(str::to_owned));
        let value = match existing {
            Some(v) => v,
            None => {
                values.push(json!({"id": Ids::next(&mut ids.value), "name": value_name}));
                value_name.clone()
            }
        };
        selected.push(json!({"name": canonical_name, "value": value}));
    }
    selected
}

fn missing_options(doc: &Value, selected: &[Value]) -> Vec<String> {
    doc["options"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| o["name"].as_str())
        .filter(|name| !selected.iter().any(|s| s["name"] == *name))
        .map(str::to_owned)
        .collect()
}

fn variant_fields(v: &mut Value, input: &Value) {
    if let Some(p) = input.get("price").and_then(model::cents) {
        v["price"] = json!(model::money_str(p));
    }
    if let Some(c) = input.get("compareAtPrice") {
        v["compareAtPrice"] = model::cents(c).map_or(Value::Null, |c| json!(model::money_str(c)));
    }
    if let Some(b) = input.get("barcode") {
        v["barcode"] = text(b);
    }
    if let Some(t) = input.get("taxable").and_then(Value::as_bool) {
        v["taxable"] = json!(t);
    }
    if let Some(p) = input.get("inventoryPolicy").and_then(Value::as_str) {
        v["inventoryPolicy"] = json!(p);
    }
    if let Some(item) = input.get("inventoryItem") {
        if let Some(sku) = item.get("sku") {
            v["sku"] = text(sku);
        }
        if let Some(t) = item.get("tracked").and_then(Value::as_bool) {
            v["tracked"] = json!(t);
        }
    }
}

fn variants_payload(type_name: &str, product: Value, variants: Value, errors: Vec<Value>) -> Value {
    json!({"__typename": type_name, "product": product, "productVariants": variants, "userErrors": errors})
}

fn variants_bulk_create(store: &mut Store, args: &Map<String, Value>, now: &str) -> Value {
    const T: &str = "ProductVariantsBulkCreatePayload";
    let Some((pid, key, mut doc)) = product_arg(store, args) else {
        return variants_payload(
            T,
            Value::Null,
            Value::Null,
            vec![coded_error(
                VARIANT_ERR,
                &["productId"],
                "Product does not exist",
                "PRODUCT_DOES_NOT_EXIST",
            )],
        );
    };
    let mut ids = Ids::new(store);
    let mut errors = Vec::new();
    let inputs = args
        .get("variants")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let strategy = args
        .get("strategy")
        .and_then(Value::as_str)
        .unwrap_or("DEFAULT");
    let was_default = is_default_option(
        doc["options"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default(),
    );
    let single = doc["variants"].as_array().is_some_and(|v| v.len() == 1);
    let remove_standalone = match strategy {
        "PRESERVE_STANDALONE_VARIANT" => false,
        "REMOVE_STANDALONE_VARIANT" => single,
        _ => was_default && single,
    };
    let any_options = inputs
        .iter()
        .any(|i| i["optionValues"].as_array().is_some_and(|a| !a.is_empty()));
    if was_default && any_options {
        doc["options"] = json!([]);
    }
    if remove_standalone {
        doc["variants"] = json!([]);
    }
    let mut created = Vec::new();
    for (i, input) in inputs.iter().enumerate() {
        let idx = i.to_string();
        let ov = input["optionValues"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let selected = if ov.is_empty()
            && is_default_option(
                doc["options"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            ) {
            vec![json!({"name": "Title", "value": "Default Title"})]
        } else {
            resolve_option_values(
                &mut doc,
                &ov,
                was_default,
                &mut ids,
                VARIANT_ERR,
                &["variants", &idx],
                &mut errors,
            )
        };
        let missing = missing_options(&doc, &selected);
        if !missing.is_empty() {
            errors.push(coded_error(
                VARIANT_ERR,
                &["variants", &idx],
                format!("You need to add option values for {}", missing.join(", ")),
                "NEED_TO_ADD_OPTION_VALUES",
            ));
            continue;
        }
        let sel = json!(selected);
        let duplicate = doc["variants"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|v| combo(&v["selectedOptions"]) == combo(&sel));
        if duplicate {
            errors.push(coded_error(
                VARIANT_ERR,
                &["variants", &idx],
                format!("The variant '{}' already exists.", variant_title(&selected)),
                "VARIANT_ALREADY_EXISTS",
            ));
            continue;
        }
        let vid = Ids::next(&mut ids.variant);
        let mut v = json!({
            "id": vid,
            "title": variant_title(&selected),
            "price": "0.00",
            "compareAtPrice": null,
            "sku": null,
            "barcode": null,
            "taxable": true,
            "tracked": true,
            "inventoryQuantity": 0,
            "selectedOptions": sel,
            "createdAt": now,
            "updatedAt": now,
        });
        variant_fields(&mut v, input);
        doc["variants"]
            .as_array_mut()
            .expect("variants array")
            .push(v);
        created.push(vid);
    }
    if !errors.is_empty() {
        return variants_payload(T, Value::Null, Value::Null, errors);
    }
    sync_variants(&mut doc);
    doc["updatedAt"] = json!(now);
    store.collection_mut("products").insert(key, doc);
    let refs = created
        .into_iter()
        .map(|v| reference("ProductVariant", v))
        .collect::<Vec<_>>();
    variants_payload(T, reference("Product", pid), json!(refs), Vec::new())
}

fn variants_bulk_update(store: &mut Store, args: &Map<String, Value>, now: &str) -> Value {
    const T: &str = "ProductVariantsBulkUpdatePayload";
    const E: &str = "ProductVariantsBulkUpdateUserError";
    let Some((pid, key, mut doc)) = product_arg(store, args) else {
        return variants_payload(
            T,
            Value::Null,
            Value::Null,
            vec![coded_error(
                E,
                &["productId"],
                "Product does not exist",
                "PRODUCT_DOES_NOT_EXIST",
            )],
        );
    };
    let mut ids = Ids::new(store);
    let mut errors = Vec::new();
    let inputs = args
        .get("variants")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut updated = Vec::new();
    for (i, input) in inputs.iter().enumerate() {
        let idx = i.to_string();
        let Some(pos) = gid_of(&input["id"], "ProductVariant").and_then(|vid| {
            doc["variants"]
                .as_array()?
                .iter()
                .position(|v| id_of(v) == vid)
        }) else {
            errors.push(coded_error(
                E,
                &["variants", &idx, "id"],
                "Product variant does not exist",
                "PRODUCT_VARIANT_DOES_NOT_EXIST",
            ));
            continue;
        };
        if let Some(ov) = input["optionValues"].as_array().filter(|a| !a.is_empty()) {
            let changes = resolve_option_values(
                &mut doc,
                ov,
                false,
                &mut ids,
                E,
                &["variants", &idx],
                &mut errors,
            );
            let mut selected = doc["variants"][pos]["selectedOptions"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            for c in changes {
                match selected.iter_mut().find(|s| s["name"] == c["name"]) {
                    Some(s) => s["value"] = c["value"].clone(),
                    None => selected.push(c),
                }
            }
            let sel = json!(selected);
            let duplicate = doc["variants"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
                .any(|(n, v)| n != pos && combo(&v["selectedOptions"]) == combo(&sel));
            if duplicate {
                errors.push(coded_error(
                    E,
                    &["variants", &idx],
                    format!("The variant '{}' already exists.", variant_title(&selected)),
                    "VARIANT_ALREADY_EXISTS",
                ));
                continue;
            }
            doc["variants"][pos]["selectedOptions"] = sel;
        }
        let v = &mut doc["variants"][pos];
        variant_fields(v, input);
        v["updatedAt"] = json!(now);
        updated.push(id_of(v));
    }
    if !errors.is_empty() {
        return variants_payload(T, Value::Null, Value::Null, errors);
    }
    sync_variants(&mut doc);
    doc["updatedAt"] = json!(now);
    store.collection_mut("products").insert(key, doc);
    let refs = updated
        .into_iter()
        .map(|v| reference("ProductVariant", v))
        .collect::<Vec<_>>();
    variants_payload(T, reference("Product", pid), json!(refs), Vec::new())
}

fn variants_bulk_delete(store: &mut Store, args: &Map<String, Value>, now: &str) -> Value {
    const E: &str = "ProductVariantsBulkDeleteUserError";
    let payload = |product: Value, errors: Vec<Value>| json!({"__typename": "ProductVariantsBulkDeletePayload", "product": product, "userErrors": errors});
    let Some((pid, key, mut doc)) = product_arg(store, args) else {
        return payload(
            Value::Null,
            vec![coded_error(
                E,
                &["productId"],
                "Product does not exist",
                "PRODUCT_DOES_NOT_EXIST",
            )],
        );
    };
    let wanted: Vec<Option<u64>> = args
        .get("variantsIds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|v| gid_of(v, "ProductVariant"))
        .collect();
    let variants = doc["variants"].as_array().cloned().unwrap_or_default();
    let belongs = |id: &Option<u64>| id.is_some_and(|id| variants.iter().any(|v| id_of(v) == id));
    if wanted.is_empty() || !wanted.iter().all(belongs) {
        return payload(
            Value::Null,
            vec![coded_error(
                E,
                &["variantsIds"],
                "At least one variant does not belong to the product",
                "AT_LEAST_ONE_VARIANT_DOES_NOT_BELONG_TO_THE_PRODUCT",
            )],
        );
    }
    let remaining: Vec<Value> = variants
        .into_iter()
        .filter(|v| !wanted.contains(&Some(id_of(v))))
        .collect();
    if remaining.is_empty() {
        // A product always keeps one variant: Shopify falls back to the default one.
        let mut ids = Ids::new(store);
        doc["options"] = json!([default_option(
            Ids::next(&mut ids.option),
            Ids::next(&mut ids.value)
        )]);
        doc["variants"] = json!([{
            "id": Ids::next(&mut ids.variant),
            "title": "Default Title",
            "price": "0.00",
            "compareAtPrice": null,
            "sku": null,
            "barcode": null,
            "taxable": true,
            "tracked": false,
            "inventoryQuantity": 0,
            "selectedOptions": [{"name": "Title", "value": "Default Title"}],
            "createdAt": now,
            "updatedAt": now,
        }]);
    } else {
        doc["variants"] = json!(remaining);
    }
    doc["updatedAt"] = json!(now);
    store.collection_mut("products").insert(key, doc);
    payload(reference("Product", pid), Vec::new())
}

fn options_payload(type_name: &str, product: Value, errors: Vec<Value>) -> Value {
    json!({"__typename": type_name, "product": product, "userErrors": errors})
}

fn options_create(store: &mut Store, args: &Map<String, Value>, now: &str) -> Value {
    const T: &str = "ProductOptionsCreatePayload";
    const E: &str = "ProductOptionsCreateUserError";
    let Some((pid, key, mut doc)) = product_arg(store, args) else {
        return options_payload(
            T,
            Value::Null,
            vec![coded_error(
                E,
                &["productId"],
                "Product does not exist",
                "PRODUCT_DOES_NOT_EXIST",
            )],
        );
    };
    let mut ids = Ids::new(store);
    let mut errors = Vec::new();
    let inputs = args
        .get("options")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let was_default = is_default_option(
        doc["options"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default(),
    );
    let mut options: Vec<Value> = if was_default {
        Vec::new()
    } else {
        doc["options"].as_array().cloned().unwrap_or_default()
    };
    if options.len() + inputs.len() > 3 {
        errors.push(coded_error(
            E,
            &["options"],
            "Can only specify a maximum of 3 options",
            "OPTIONS_OVER_LIMIT",
        ));
    }
    let mut added = Vec::new();
    for (i, input) in inputs.iter().enumerate() {
        let idx = i.to_string();
        let name = input["name"].as_str().unwrap_or_default().trim().to_owned();
        let values: Vec<String> = input["values"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v["name"].as_str().map(str::to_owned))
            .collect();
        if name.is_empty() {
            errors.push(coded_error(
                E,
                &["options", &idx, "name"],
                "Name can't be blank",
                "BLANK",
            ));
            continue;
        }
        if options.iter().any(|o| {
            o["name"]
                .as_str()
                .is_some_and(|n| n.eq_ignore_ascii_case(&name))
        }) {
            errors.push(coded_error(
                E,
                &["options", &idx, "name"],
                format!("Option '{name}' already exists."),
                "OPTION_ALREADY_EXISTS",
            ));
            continue;
        }
        if values.is_empty() {
            errors.push(coded_error(
                E,
                &["options", &idx, "values"],
                format!("Option '{name}' must specify at least one option value."),
                "OPTION_VALUES_MISSING",
            ));
            continue;
        }
        let option = json!({
            "id": Ids::next(&mut ids.option),
            "name": name,
            "position": 0,
            "values": values.iter().map(|v| json!({"id": Ids::next(&mut ids.value), "name": v})).collect::<Vec<_>>(),
        });
        let at = input["position"].as_u64().map_or(options.len(), |p| {
            (p as usize).saturating_sub(1).min(options.len())
        });
        options.insert(at, option.clone());
        added.push(option);
    }
    if !errors.is_empty() {
        return options_payload(T, Value::Null, errors);
    }
    doc["options"] = json!(options);
    for v in doc["variants"].as_array_mut().into_iter().flatten() {
        let mut selected = if was_default {
            Vec::new()
        } else {
            v["selectedOptions"].as_array().cloned().unwrap_or_default()
        };
        for o in &added {
            selected.push(json!({"name": o["name"], "value": o["values"][0]["name"]}));
        }
        v["selectedOptions"] = json!(selected);
    }
    sync_variants(&mut doc);
    doc["updatedAt"] = json!(now);
    store.collection_mut("products").insert(key, doc);
    options_payload(T, reference("Product", pid), Vec::new())
}

fn option_update(store: &mut Store, args: &Map<String, Value>, now: &str) -> Value {
    const T: &str = "ProductOptionUpdatePayload";
    const E: &str = "ProductOptionUpdateUserError";
    let Some((pid, key, mut doc)) = product_arg(store, args) else {
        return options_payload(
            T,
            Value::Null,
            vec![coded_error(
                E,
                &["productId"],
                "Product does not exist",
                "PRODUCT_DOES_NOT_EXIST",
            )],
        );
    };
    let mut ids = Ids::new(store);
    let input = args.get("option").cloned().unwrap_or(Value::Null);
    let Some(pos) = gid_of(&input["id"], "ProductOption").and_then(|oid| {
        doc["options"]
            .as_array()?
            .iter()
            .position(|o| id_of(o) == oid)
    }) else {
        return options_payload(
            T,
            Value::Null,
            vec![coded_error(
                E,
                &["option", "id"],
                "Option does not exist",
                "OPTION_DOES_NOT_EXIST",
            )],
        );
    };
    let mut errors = Vec::new();
    let old_name = doc["options"][pos]["name"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if let Some(name) = input["name"]
        .as_str()
        .map(str::trim)
        .filter(|n| !n.is_empty() && *n != old_name)
    {
        let clash = doc["options"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .any(|(n, o)| {
                n != pos
                    && o["name"]
                        .as_str()
                        .is_some_and(|on| on.eq_ignore_ascii_case(name))
            });
        if clash {
            errors.push(coded_error(
                E,
                &["option", "name"],
                format!("Option '{name}' already exists."),
                "OPTION_ALREADY_EXISTS",
            ));
        } else {
            doc["options"][pos]["name"] = json!(name);
            for v in doc["variants"].as_array_mut().into_iter().flatten() {
                for s in v["selectedOptions"].as_array_mut().into_iter().flatten() {
                    if s["name"] == old_name.as_str() {
                        s["name"] = json!(name);
                    }
                }
            }
        }
    }
    let variants = doc["variants"].as_array().cloned().unwrap_or_default();
    let option_name = doc["options"][pos]["name"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let to_delete: Vec<Value> = args
        .get("optionValuesToDelete")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for (i, vid) in to_delete.iter().enumerate() {
        let idx = i.to_string();
        let values = doc["options"][pos]["values"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let Some(value) =
            gid_of(vid, "ProductOptionValue").and_then(|id| values.iter().find(|v| id_of(v) == id))
        else {
            errors.push(coded_error(
                E,
                &["optionValuesToDelete", &idx],
                "Option value does not exist",
                "OPTION_VALUE_DOES_NOT_EXIST",
            ));
            continue;
        };
        let used = variants.iter().any(|v| {
            v["selectedOptions"].as_array().is_some_and(|s| {
                s.iter()
                    .any(|so| so["name"] == option_name.as_str() && so["value"] == value["name"])
            })
        });
        if used {
            errors.push(coded_error(
                E,
                &["optionValuesToDelete", &idx],
                format!(
                    "Option value '{}' has variants and cannot be deleted.",
                    value["name"].as_str().unwrap_or_default()
                ),
                "OPTION_VALUE_HAS_VARIANTS",
            ));
            continue;
        }
        let id = id_of(value);
        if let Some(list) = doc["options"][pos]["values"].as_array_mut() {
            list.retain(|v| id_of(v) != id);
        }
    }
    for (i, v) in args
        .get("optionValuesToAdd")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let name = v["name"].as_str().unwrap_or_default().trim().to_owned();
        let list = doc["options"][pos]["values"]
            .as_array_mut()
            .expect("values array");
        if name.is_empty() {
            errors.push(coded_error(
                E,
                &["optionValuesToAdd", &i.to_string(), "name"],
                "Name can't be blank",
                "BLANK",
            ));
        } else if list.iter().any(|x| {
            x["name"]
                .as_str()
                .is_some_and(|n| n.eq_ignore_ascii_case(&name))
        }) {
            errors.push(coded_error(
                E,
                &["optionValuesToAdd", &i.to_string(), "name"],
                format!("Option value '{name}' already exists."),
                "OPTION_VALUE_ALREADY_EXISTS",
            ));
        } else {
            list.push(json!({"id": Ids::next(&mut ids.value), "name": name}));
        }
    }
    if doc["options"][pos]["values"]
        .as_array()
        .is_some_and(Vec::is_empty)
    {
        errors.push(coded_error(
            E,
            &["optionValuesToDelete"],
            "Cannot delete all option values of an option",
            "CANNOT_DELETE_ALL_OPTION_VALUES_IN_OPTION",
        ));
    }
    if let Some(p) = input["position"].as_u64() {
        let options = doc["options"].as_array_mut().expect("options array");
        let option = options.remove(pos);
        let at = (p as usize).saturating_sub(1).min(options.len());
        options.insert(at, option);
    }
    if !errors.is_empty() {
        return options_payload(T, Value::Null, errors);
    }
    sync_variants(&mut doc);
    doc["updatedAt"] = json!(now);
    store.collection_mut("products").insert(key, doc);
    options_payload(T, reference("Product", pid), Vec::new())
}

fn options_delete(store: &mut Store, args: &Map<String, Value>, now: &str) -> Value {
    const E: &str = "ProductOptionsDeleteUserError";
    let payload = |product: Value, deleted: Value, errors: Vec<Value>| json!({"__typename": "ProductOptionsDeletePayload", "deletedOptionsIds": deleted, "product": product, "userErrors": errors});
    let Some((pid, key, mut doc)) = product_arg(store, args) else {
        return payload(
            Value::Null,
            Value::Null,
            vec![coded_error(
                E,
                &["productId"],
                "Product does not exist",
                "PRODUCT_DOES_NOT_EXIST",
            )],
        );
    };
    let mut errors = Vec::new();
    let options = doc["options"].as_array().cloned().unwrap_or_default();
    let mut doomed = Vec::new();
    for (i, oid) in args
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        match gid_of(oid, "ProductOption").and_then(|id| options.iter().find(|o| id_of(o) == id)) {
            Some(o) => doomed.push(o.clone()),
            None => errors.push(coded_error(
                E,
                &["options", &i.to_string()],
                "Option does not exist",
                "OPTION_DOES_NOT_EXIST",
            )),
        }
    }
    if !errors.is_empty() {
        return payload(Value::Null, Value::Null, errors);
    }
    let doomed_names: Vec<&str> = doomed.iter().filter_map(|o| o["name"].as_str()).collect();
    let remaining: Vec<Value> = options
        .iter()
        .filter(|o| !doomed_names.contains(&o["name"].as_str().unwrap_or_default()))
        .cloned()
        .collect();
    let mut variants = doc["variants"].as_array().cloned().unwrap_or_default();
    for v in &mut variants {
        let kept: Vec<Value> = v["selectedOptions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| !doomed_names.contains(&s["name"].as_str().unwrap_or_default()))
            .cloned()
            .collect();
        v["selectedOptions"] = json!(kept);
    }
    let mut seen = Vec::new();
    for v in &variants {
        let c = combo(&v["selectedOptions"]);
        if seen.contains(&c) {
            return payload(
                Value::Null,
                Value::Null,
                vec![coded_error(
                    E,
                    &["options"],
                    "Cannot delete option with multiple values",
                    "CANNOT_DELETE_OPTION_WITH_MULTIPLE_VALUES",
                )],
            );
        }
        seen.push(c);
    }
    if remaining.is_empty() {
        let mut ids = Ids::new(store);
        doc["options"] = json!([default_option(
            Ids::next(&mut ids.option),
            Ids::next(&mut ids.value)
        )]);
        for v in &mut variants {
            v["selectedOptions"] = json!([{"name": "Title", "value": "Default Title"}]);
        }
    } else {
        doc["options"] = json!(remaining);
    }
    doc["variants"] = json!(variants);
    sync_variants(&mut doc);
    doc["updatedAt"] = json!(now);
    store.collection_mut("products").insert(key, doc);
    let deleted: Vec<Value> = doomed
        .iter()
        .map(|o| json!(model::gid("ProductOption", id_of(o))))
        .collect();
    payload(reference("Product", pid), json!(deleted), Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_split_trim_and_dedupe() {
        assert_eq!(
            tags(&json!(["VIP, wholesale", " vip", "", "Gift"])),
            json!(["VIP", "wholesale", "Gift"])
        );
    }

    #[test]
    fn phones_normalize_to_e164() {
        assert_eq!(
            normalize_phone("(415) 555-0134").as_deref(),
            Some("+14155550134")
        );
        assert_eq!(
            normalize_phone("+44 20 7946 0958").as_deref(),
            Some("+442079460958")
        );
        assert_eq!(normalize_phone("call me"), None);
        assert_eq!(normalize_phone("555"), None);
    }

    #[test]
    fn slugs() {
        assert_eq!(
            slugify("Harbor Canvas Tote — Navy!"),
            "harbor-canvas-tote-navy"
        );
    }
}
