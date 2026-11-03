//! Users, search, pages (+ properties, move), data sources (+ query, templates),
//! databases and comments.

use serde_json::{Map, Value, json};

use crate::{
    blocks::{self, children_raw},
    model::{
        ApiError, Ctx, Res, list, need_id, norm_id, notion_url, paginate, plain, public, rich_text,
        s, user_ref,
    },
};

// ───────────────────────────── users ─────────────────────────────

pub fn get_user(ctx: &mut Ctx, raw: &str) -> Res {
    let id = need_id(raw, "user_id")?;
    ctx.get("users", &id)
        .map(public)
        .ok_or_else(|| ApiError::not_found("user", &id))
}

pub fn list_users(ctx: &mut Ctx, cursor: Option<&str>, size: Option<i64>) -> Res {
    let (page, next) = paginate(
        ctx.all("users").into_iter().map(public).collect(),
        cursor,
        size,
    )?;
    let rid = ctx.request_id();
    Ok(list(page, next, "user", &rid))
}

pub fn me(ctx: &mut Ctx) -> Res {
    Ok(public(ctx.bot()))
}

// ───────────────────────────── titles & outputs ─────────────────────────────

pub fn title_of(page: &Value) -> String {
    if let Some(props) = page["properties"].as_object() {
        for p in props.values() {
            if s(p, "type") == "title" {
                return plain(&p["title"]);
            }
        }
    }
    String::new()
}

fn ds_of_page(ctx: &Ctx, page: &Value) -> Option<Value> {
    let p = &page["parent"];
    match s(p, "type") {
        "data_source_id" => ctx.get("data_sources", s(p, "data_source_id")),
        "database_id" => ctx
            .all("data_sources")
            .into_iter()
            .find(|d| s(&d["parent"], "database_id") == s(p, "database_id")),
        _ => None,
    }
}

fn page_url(page: &Value) -> String {
    notion_url(&title_of(page), s(page, "id"))
}

/// The page as served: computed properties filled in, internal fields removed.
pub fn page_out(ctx: &Ctx, page: &Value, only: Option<&[String]>) -> Value {
    let mut out = public(page.clone());
    if let Some(ds) = ds_of_page(ctx, page) {
        let mut props = Map::new();
        for (name, schema) in ds["properties"].as_object().cloned().unwrap_or_default() {
            let ty = s(&schema, "type").to_owned();
            let id = s(&schema, "id").to_owned();
            let stored = page["properties"].get(&name).cloned();
            let value = match ty.as_str() {
                "created_time" => json!(page["created_time"]),
                "last_edited_time" => json!(page["last_edited_time"]),
                "created_by" => full_user(ctx, s(&page["created_by"], "id")),
                "last_edited_by" => full_user(ctx, s(&page["last_edited_by"], "id")),
                _ => stored
                    .as_ref()
                    .and_then(|v| v.get(&ty).cloned())
                    .unwrap_or_else(|| empty_value(&ty)),
            };
            let mut p = json!({"id": id, "type": ty, ty.clone(): value});
            if ty == "relation" {
                p["has_more"] = json!(false);
            }
            props.insert(name, p);
        }
        out["properties"] = Value::Object(props);
    }
    if let Some(ids) = only
        && let Some(m) = out["properties"].as_object_mut()
    {
        m.retain(|_, v| ids.iter().any(|i| i == s(v, "id")));
    }
    out["url"] = json!(page_url(page));
    out
}

fn empty_value(ty: &str) -> Value {
    match ty {
        "title" | "rich_text" | "multi_select" | "people" | "relation" | "files" => json!([]),
        "checkbox" => json!(false),
        "formula" => json!({"type": "string", "string": null}),
        "rollup" => json!({"type": "array", "array": [], "function": "show_original"}),
        "unique_id" => json!({"prefix": null, "number": null}),
        _ => Value::Null,
    }
}

fn full_user(ctx: &Ctx, id: &str) -> Value {
    ctx.get("users", id)
        .map(public)
        .unwrap_or_else(|| user_ref(id))
}

pub fn ds_out(ctx: &Ctx, ds: &Value) -> Value {
    let mut out = public(ds.clone());
    let db = ctx.get("databases", s(&ds["parent"], "database_id"));
    if let Some(db) = &db {
        out["database_parent"] = db["parent"].clone();
        out["is_inline"] = db.get("is_inline").cloned().unwrap_or(json!(false));
    }
    out["url"] = json!(notion_url(&plain(&ds["title"]), s(ds, "id")));
    out
}

pub fn db_out(ctx: &Ctx, db: &Value) -> Value {
    let mut out = public(db.clone());
    let sources: Vec<Value> = ctx
        .all("data_sources")
        .iter()
        .filter(|d| s(&d["parent"], "database_id") == s(db, "id"))
        .map(|d| json!({"id": s(d, "id"), "name": plain(&d["title"])}))
        .collect();
    out["data_sources"] = Value::Array(sources);
    out["url"] = json!(notion_url(&plain(&db["title"]), s(db, "id")));
    out
}

// ───────────────────────────── property values ─────────────────────────────

fn short_id(ctx: &mut Ctx) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let u = ctx.new_id("option");
    u.bytes()
        .take(4)
        .map(|b| A[b as usize % A.len()] as char)
        .collect()
}

/// Converts a request property value into the stored `{<type>: value}` form, creating
/// select options when needed (mutating `schema`).
fn convert_value(
    ctx: &mut Ctx,
    name: &str,
    schema: &mut Value,
    input: &Value,
) -> Result<Value, ApiError> {
    let ty = s(schema, "type").to_owned();
    let path = format!("body.properties.{name}");
    let Some(v) = input.get(&ty) else {
        let given = input
            .as_object()
            .and_then(|o| o.keys().find(|k| *k != "id" && *k != "type").cloned());
        return Err(match given {
            Some(other) => {
                ApiError::validation(format!("{name} is expected to be {ty}. Got `{other}`."))
            }
            None => ApiError::validation(format!(
                "body failed validation: {path}.{ty} should be defined, instead was `undefined`."
            )),
        });
    };
    let bad = |what: &str| {
        ApiError::validation(format!(
            "body failed validation: {path}.{ty} should be {what}, instead was `{v}`."
        ))
    };
    let out = match ty.as_str() {
        "title" | "rich_text" => json!(rich_text(ctx, v)?),
        "number" => {
            if v.is_null() || v.is_number() {
                v.clone()
            } else {
                return Err(bad("a number or `null`"));
            }
        }
        "checkbox" => json!(v.as_bool().ok_or_else(|| bad("a boolean"))?),
        "url" | "email" | "phone_number" => {
            if v.is_null() || v.is_string() {
                v.clone()
            } else {
                return Err(bad("a string or `null`"));
            }
        }
        "select" | "status" => {
            if v.is_null() {
                if ty == "status" {
                    return Err(bad("an object"));
                }
                Value::Null
            } else {
                resolve_option(ctx, name, &ty, schema, v)?
            }
        }
        "multi_select" => {
            let items = v.as_array().ok_or_else(|| bad("an array"))?;
            let mut out = Vec::new();
            for it in items {
                out.push(resolve_option(ctx, name, &ty, schema, it)?);
            }
            Value::Array(out)
        }
        "date" => {
            if v.is_null() {
                Value::Null
            } else {
                let start = v["start"].as_str().ok_or_else(|| ApiError::validation(format!("body failed validation: {path}.date.start should be defined, instead was `undefined`.")))?;
                if !valid_date(start) {
                    return Err(ApiError::validation(format!(
                        "{start} is not a valid ISO 8601 date."
                    )));
                }
                json!({"start": start, "end": v.get("end").cloned().unwrap_or(Value::Null), "time_zone": v.get("time_zone").cloned().unwrap_or(Value::Null)})
            }
        }
        "people" => {
            let items = v.as_array().ok_or_else(|| bad("an array"))?;
            let mut out = Vec::new();
            for it in items {
                let id = norm_id(s(it, "id")).ok_or_else(|| {
                    ApiError::validation(format!(
                        "body failed validation: {path}.people[].id should be a valid uuid."
                    ))
                })?;
                let u = ctx.get("users", &id).ok_or_else(|| {
                    ApiError::validation(format!("Could not find user with ID: {id}."))
                })?;
                out.push(public(u));
            }
            Value::Array(out)
        }
        "relation" => {
            let items = v.as_array().ok_or_else(|| bad("an array"))?;
            let mut out = Vec::new();
            for it in items {
                let id = norm_id(s(it, "id")).ok_or_else(|| {
                    ApiError::validation(format!(
                        "body failed validation: {path}.relation[].id should be a valid uuid."
                    ))
                })?;
                if ctx.get("pages", &id).is_none() {
                    return Err(ApiError::not_found("page", &id));
                }
                out.push(json!({"id": id}));
            }
            Value::Array(out)
        }
        "files" => v.clone(),
        other => {
            return Err(ApiError::validation(format!(
                "{name} is a {other} property and cannot be updated."
            )));
        }
    };
    Ok(json!({"id": s(schema, "id"), "type": ty, ty.clone(): out}))
}

fn valid_date(d: &str) -> bool {
    let date = d.get(0..10).unwrap_or("");
    let parts: Vec<&str> = date.split('-').collect();
    parts.len() == 3
        && parts[0].len() == 4
        && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit()))
        && parts[1].parse::<u32>().is_ok_and(|m| (1..=12).contains(&m))
        && parts[2].parse::<u32>().is_ok_and(|x| (1..=31).contains(&x))
}

fn resolve_option(
    ctx: &mut Ctx,
    name: &str,
    ty: &str,
    schema: &mut Value,
    v: &Value,
) -> Result<Value, ApiError> {
    let options = schema[ty]["options"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let by_id = v.get("id").and_then(Value::as_str);
    let by_name = v.get("name").and_then(Value::as_str);
    if let Some(o) = options.iter().find(|o| {
        by_id.is_some_and(|i| s(o, "id") == i) || by_name.is_some_and(|n| s(o, "name") == n)
    }) {
        return Ok(json!({"id": s(o, "id"), "name": s(o, "name"), "color": s(o, "color")}));
    }
    match (ty, by_name) {
        ("status", Some(n)) => Err(ApiError::validation(format!(
            "Invalid status option. Status option \"{n}\" does not exist for property \"{name}\"."
        ))),
        (_, Some(n)) => {
            if n.contains(',') {
                return Err(ApiError::validation(format!(
                    "Invalid select option, commas not allowed: {n}"
                )));
            }
            let id = short_id(ctx);
            let opt = json!({"id": id, "name": n, "color": "default", "description": null});
            schema[ty]["options"]
                .as_array_mut()
                .map(|a| a.push(opt.clone()))
                .unwrap_or_else(|| schema[ty]["options"] = json!([opt.clone()]));
            Ok(json!({"id": id, "name": n, "color": "default"}))
        }
        (_, None) => Err(ApiError::validation(format!(
            "Invalid {ty} option for property \"{name}\": option ID not found."
        ))),
    }
}

/// Applies `input` properties to `page`; returns the (possibly extended) data source schema.
fn apply_properties(ctx: &mut Ctx, page: &mut Value, input: &Value) -> Result<(), ApiError> {
    let Some(map) = input.as_object() else {
        return Err(ApiError::validation(
            "body failed validation: body.properties should be an object.",
        ));
    };
    match ds_of_page(ctx, page) {
        Some(mut ds) => {
            let mut changed_schema = false;
            for (key, v) in map {
                let name = ds["properties"]
                    .as_object()
                    .and_then(|props| {
                        props
                            .iter()
                            .find(|(n, p)| *n == key || s(p, "id") == key)
                            .map(|(n, _)| n.clone())
                    })
                    .ok_or_else(|| {
                        ApiError::validation(format!("{key} is not a property that exists."))
                    })?;
                let mut schema = ds["properties"][&name].clone();
                let before = schema.clone();
                let value = convert_value(ctx, &name, &mut schema, v)?;
                if schema != before {
                    ds["properties"][&name] = schema;
                    changed_schema = true;
                }
                page["properties"][&name] = value;
            }
            if changed_schema {
                ctx.put("data_sources", ds);
            }
        }
        None => {
            for (key, v) in map {
                if key != "title" && s(v, "type") != "title" && v.get("title").is_none() {
                    return Err(ApiError::validation(format!(
                        "Invalid property identifier `{key}`: pages whose parent is a page can only set `title`."
                    )));
                }
                let mut schema = json!({"id": "title", "type": "title"});
                let value = convert_value(ctx, "title", &mut schema, v)?;
                page["properties"]["title"] = value;
            }
        }
    }
    Ok(())
}

// ───────────────────────────── pages ─────────────────────────────

fn need_page(ctx: &Ctx, raw: &str) -> Result<Value, ApiError> {
    let id = need_id(raw, "page_id")?;
    ctx.get("pages", &id)
        .ok_or_else(|| ApiError::not_found("page", &id))
}

pub fn retrieve_page(ctx: &mut Ctx, raw: &str, filter: &[String]) -> Res {
    let page = need_page(ctx, raw)?;
    Ok(page_out(
        ctx,
        &page,
        if filter.is_empty() {
            None
        } else {
            Some(filter)
        },
    ))
}

fn parse_json_field(v: &Value) -> Value {
    match v {
        Value::String(t) => serde_json::from_str(t).unwrap_or_else(|_| v.clone()),
        other => other.clone(),
    }
}

fn icon_value(v: &Value) -> Result<Value, ApiError> {
    let v = parse_json_field(v);
    match &v {
        Value::Null => Ok(Value::Null),
        Value::String(e) => Ok(json!({"type": "emoji", "emoji": e})),
        Value::Object(o) if o.contains_key("emoji") => {
            Ok(json!({"type": "emoji", "emoji": o["emoji"]}))
        }
        Value::Object(o) if o.contains_key("external") => {
            Ok(json!({"type": "external", "external": o["external"]}))
        }
        _ => Err(ApiError::validation(
            "body failed validation: body.icon should be an emoji or external file object.",
        )),
    }
}

/// Resolves a request `parent` into the stored page parent.
fn resolve_parent(ctx: &Ctx, parent: &Value, for_move: bool) -> Result<Value, ApiError> {
    let parent = parse_json_field(parent);
    let ty = match s(&parent, "type") {
        "" if parent.get("page_id").is_some() => "page_id",
        "" if parent.get("data_source_id").is_some() => "data_source_id",
        "" if parent.get("database_id").is_some() => "database_id",
        t => t,
    };
    match ty {
        "page_id" => {
            let id = norm_id(s(&parent, "page_id")).ok_or_else(|| {
                ApiError::validation(
                    "body failed validation: body.parent.page_id should be a valid uuid.",
                )
            })?;
            if ctx.get("pages", &id).is_none() {
                return Err(ApiError::not_found("page", &id));
            }
            Ok(json!({"type": "page_id", "page_id": id}))
        }
        "data_source_id" => {
            let id = norm_id(s(&parent, "data_source_id")).ok_or_else(|| {
                ApiError::validation(
                    "body failed validation: body.parent.data_source_id should be a valid uuid.",
                )
            })?;
            let ds = ctx
                .get("data_sources", &id)
                .ok_or_else(|| ApiError::not_found("data_source", &id))?;
            Ok(
                json!({"type": "data_source_id", "data_source_id": id, "database_id": s(&ds["parent"], "database_id")}),
            )
        }
        "database_id" => {
            let id = norm_id(s(&parent, "database_id")).ok_or_else(|| {
                ApiError::validation(
                    "body failed validation: body.parent.database_id should be a valid uuid.",
                )
            })?;
            if ctx.get("databases", &id).is_none() {
                return Err(ApiError::not_found("database", &id));
            }
            let sources: Vec<Value> = ctx
                .all("data_sources")
                .into_iter()
                .filter(|d| s(&d["parent"], "database_id") == id)
                .collect();
            if sources.len() != 1 {
                return Err(ApiError::validation(format!(
                    "Database {id} has {} data sources; use a data_source_id parent instead.",
                    sources.len()
                )));
            }
            Ok(
                json!({"type": "data_source_id", "data_source_id": s(&sources[0], "id"), "database_id": id}),
            )
        }
        "workspace" => Err(ApiError::validation(if for_move {
            "Pages can't be moved to the workspace level by an integration."
        } else {
            "Can't create a page with a workspace parent. Use a page or data source as the parent."
        })),
        _ => Err(ApiError::validation(
            "body failed validation: body.parent should be an object with `page_id`, `data_source_id` or `database_id`.",
        )),
    }
}

fn next_order(ctx: &Ctx, parent_page: &str) -> f64 {
    children_raw(ctx, parent_page)
        .iter()
        .map(|b| b.get("_order").and_then(Value::as_f64).unwrap_or(0.0))
        .fold(0.0, f64::max)
        + 1.0
}

pub fn create_page(ctx: &mut Ctx, body: &Value) -> Res {
    let parent = resolve_parent(ctx, body.get("parent").unwrap_or(&Value::Null), false)?;
    let props = parse_json_field(body.get("properties").unwrap_or(&json!({})));
    let now = ctx.iso_now();
    let bot = ctx.bot_id();
    let mut page = json!({
        "object": "page", "id": "", "created_time": now, "last_edited_time": now,
        "created_by": user_ref(&bot), "last_edited_by": user_ref(&bot),
        "cover": null, "icon": null, "parent": parent, "archived": false, "in_trash": false,
        "is_locked": false, "properties": {}, "public_url": null,
    });
    if s(&parent, "type") == "page_id" {
        page["_order"] = json!(next_order(ctx, s(&parent, "page_id")));
        page["properties"]["title"] = json!({"id": "title", "type": "title", "title": []});
    }
    apply_properties(ctx, &mut page, &props)?;
    if let Some(icon) = body.get("icon") {
        page["icon"] = icon_value(icon)?;
    }
    if let Some(cover) = body.get("cover") {
        page["cover"] = parse_json_field(cover);
    }
    // IDs are only consumed once the request is valid.
    let id = ctx.new_id("page");
    page["id"] = json!(id);
    // Unique IDs are assigned on creation.
    if let Some(ds) = ds_of_page(ctx, &page) {
        for (name, schema) in ds["properties"].as_object().cloned().unwrap_or_default() {
            if s(&schema, "type") == "unique_id" {
                let max = ctx
                    .all("pages")
                    .iter()
                    .filter(|p| s(&p["parent"], "data_source_id") == s(&ds, "id"))
                    .filter_map(|p| p["properties"][&name]["unique_id"]["number"].as_i64())
                    .max()
                    .unwrap_or(0);
                page["properties"][&name] = json!({"id": s(&schema, "id"), "type": "unique_id", "unique_id": {"prefix": schema["unique_id"]["prefix"], "number": max + 1}});
            }
        }
    }
    ctx.put("pages", page.clone());
    if let Some(children) = body.get("children") {
        let children: Vec<Value> = match parse_json_field(children) {
            Value::Array(a) => a,
            _ => {
                return Err(ApiError::validation(
                    "body failed validation: body.children should be an array.",
                ));
            }
        };
        if let Err(e) = blocks::append(ctx, &id, &children, None) {
            ctx.st.store.collection_mut("pages").remove(&id);
            return Err(e);
        }
    }
    let page = ctx.get("pages", &id).expect("created page");
    Ok(page_out(ctx, &page, None))
}

pub fn patch_page(ctx: &mut Ctx, raw: &str, body: &Value) -> Res {
    let mut page = need_page(ctx, raw)?;
    let trash = body
        .get("in_trash")
        .or_else(|| body.get("archived"))
        .and_then(Value::as_bool);
    if page["in_trash"].as_bool().unwrap_or(false)
        && trash != Some(false)
        && body.get("properties").is_some()
    {
        return Err(ApiError::validation(
            "Can't edit block that is archived. You must unarchive the block before editing.",
        ));
    }
    if let Some(props) = body.get("properties") {
        let props = parse_json_field(props);
        apply_properties(ctx, &mut page, &props)?;
    }
    if let Some(t) = trash {
        page["in_trash"] = json!(t);
        page["archived"] = json!(t);
    }
    if let Some(icon) = body.get("icon") {
        page["icon"] = icon_value(icon)?;
    }
    if let Some(cover) = body.get("cover") {
        page["cover"] = parse_json_field(cover);
    }
    page["last_edited_time"] = json!(ctx.iso_now());
    page["last_edited_by"] = user_ref(&ctx.bot_id());
    ctx.put("pages", page.clone());
    Ok(page_out(ctx, &page, None))
}

pub fn move_page(ctx: &mut Ctx, raw: &str, body: &Value) -> Res {
    let mut page = need_page(ctx, raw)?;
    let parent = resolve_parent(ctx, body.get("parent").unwrap_or(&Value::Null), true)?;
    if s(&parent, "page_id") == s(&page, "id") {
        return Err(ApiError::validation("A page can't be moved into itself."));
    }
    let old_ds = ds_of_page(ctx, &page);
    page["parent"] = parent.clone();
    if s(&parent, "type") == "page_id" {
        page["_order"] = json!(next_order(ctx, s(&parent, "page_id")));
        let title = page["properties"]
            .as_object()
            .and_then(|m| m.values().find(|p| s(p, "type") == "title").cloned())
            .unwrap_or(json!({"id": "title", "type": "title", "title": []}));
        page["properties"] = json!({"title": title});
    } else if old_ds.as_ref().map(|d| s(d, "id")) != Some(s(&parent, "data_source_id")) {
        let title = page["properties"]
            .as_object()
            .and_then(|m| m.values().find(|p| s(p, "type") == "title").cloned());
        let new_ds = ds_of_page(ctx, &page).expect("resolved data source");
        let title_name = new_ds["properties"]
            .as_object()
            .and_then(|m| {
                m.iter()
                    .find(|(_, p)| s(p, "type") == "title")
                    .map(|(n, _)| n.clone())
            })
            .unwrap_or_else(|| "Name".into());
        page["properties"] = json!({});
        if let Some(t) = title {
            page["properties"][title_name] = t;
        }
    }
    page["last_edited_time"] = json!(ctx.iso_now());
    ctx.put("pages", page.clone());
    Ok(page_out(ctx, &page, None))
}

fn percent_decode(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&text[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn page_property(
    ctx: &mut Ctx,
    page_raw: &str,
    prop_raw: &str,
    cursor: Option<&str>,
    size: Option<i64>,
) -> Res {
    let page = need_page(ctx, page_raw)?;
    let out = page_out(ctx, &page, None);
    let prop_id = percent_decode(prop_raw);
    let prop = out["properties"]
        .as_object()
        .and_then(|m| {
            m.iter()
                .find(|(n, p)| {
                    s(p, "id") == prop_raw
                        || s(p, "id") == prop_id
                        || percent_decode(s(p, "id")) == prop_id
                        || **n == prop_id
                })
                .map(|(_, p)| p.clone())
        })
        .ok_or_else(|| ApiError::not_found("property", &prop_id))?;
    let ty = s(&prop, "type").to_owned();
    let id = s(&prop, "id").to_owned();
    match ty.as_str() {
        "title" | "rich_text" | "people" | "relation" => {
            let items: Vec<Value> = prop[&ty]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .enumerate()
                .map(|(i, v)| json!({"object": "property_item", "id": format!("{id}{i}"), "type": ty, ty.clone(): v}))
                .collect();
            let items: Vec<Value> = items
                .into_iter()
                .map(|mut it| {
                    let item_id = s(&it, "id").to_owned();
                    it["id"] = json!(id);
                    it["_cursor"] = json!(item_id);
                    it
                })
                .collect();
            let size = size.unwrap_or(100).clamp(1, 100) as usize;
            let start = cursor
                .and_then(|c| items.iter().position(|i| s(i, "_cursor") == c))
                .unwrap_or(0);
            let page_items: Vec<Value> = items
                .iter()
                .skip(start)
                .take(size)
                .cloned()
                .map(public)
                .collect();
            let next = items.get(start + size).map(|i| s(i, "_cursor").to_owned());
            let rid = ctx.request_id();
            let mut l = list(page_items, next.clone(), "property_item", &rid);
            l["property_item"] = json!({"id": id, "next_url": next.map(|n| format!("https://api.notion.com/v1/pages/{}/properties/{id}?start_cursor={n}", s(&page, "id"))), "type": ty, ty.clone(): {}});
            Ok(l)
        }
        _ => Ok(
            json!({"object": "property_item", "id": id, "type": ty, ty.clone(): prop[&ty], "request_id": ctx.request_id()}),
        ),
    }
}

// ───────────────────────────── search ─────────────────────────────

pub fn search(ctx: &mut Ctx, body: &Value) -> Res {
    let query = s(body, "query").to_lowercase();
    let filter = parse_json_field(body.get("filter").unwrap_or(&Value::Null));
    let want = if filter.is_object() {
        if s(&filter, "property") != "object" {
            return Err(ApiError::validation(
                "body failed validation: body.filter.property should be `\"object\"`.",
            ));
        }
        match s(&filter, "value") {
            "page" => Some("page"),
            "data_source" => Some("data_source"),
            other => {
                return Err(ApiError::validation(format!(
                    "body failed validation: body.filter.value should be `\"page\"` or `\"data_source\"`, instead was `\"{other}\"`."
                )));
            }
        }
    } else {
        None
    };
    let mut items: Vec<(String, Value)> = Vec::new();
    if want != Some("data_source") {
        for p in ctx.all("pages") {
            if p["in_trash"].as_bool().unwrap_or(false) {
                continue;
            }
            if query.is_empty() || title_of(&p).to_lowercase().contains(&query) {
                items.push((
                    s(&p, "last_edited_time").to_owned(),
                    page_out(ctx, &p, None),
                ));
            }
        }
    }
    if want != Some("page") {
        for d in ctx.all("data_sources") {
            if d["in_trash"].as_bool().unwrap_or(false) {
                continue;
            }
            if query.is_empty() || plain(&d["title"]).to_lowercase().contains(&query) {
                items.push((s(&d, "last_edited_time").to_owned(), ds_out(ctx, &d)));
            }
        }
    }
    let sort = parse_json_field(body.get("sort").unwrap_or(&Value::Null));
    let asc = s(&sort, "direction") == "ascending";
    items.sort_by(|a, b| if asc { a.0.cmp(&b.0) } else { b.0.cmp(&a.0) });
    let (page, next) = paginate(
        items.into_iter().map(|(_, v)| v).collect(),
        body.get("start_cursor").and_then(Value::as_str),
        body.get("page_size").and_then(Value::as_i64),
    )?;
    let rid = ctx.request_id();
    Ok(list(page, next, "page_or_data_source", &rid))
}

// ───────────────────────────── data sources & databases ─────────────────────────────

fn need_ds(ctx: &Ctx, raw: &str) -> Result<Value, ApiError> {
    let id = need_id(raw, "data_source_id")?;
    ctx.get("data_sources", &id)
        .ok_or_else(|| ApiError::not_found("data_source", &id))
}

pub fn get_ds(ctx: &mut Ctx, raw: &str) -> Res {
    let ds = need_ds(ctx, raw)?;
    Ok(ds_out(ctx, &ds))
}

pub fn get_db(ctx: &mut Ctx, raw: &str) -> Res {
    let id = need_id(raw, "database_id")?;
    let db = ctx
        .get("databases", &id)
        .ok_or_else(|| ApiError::not_found("database", &id))?;
    Ok(db_out(ctx, &db))
}

fn schema_from_request(ctx: &mut Ctx, name: &str, def: &Value) -> Result<Value, ApiError> {
    let def = parse_json_field(def);
    let ty = def
        .as_object()
        .and_then(|o| {
            o.keys()
                .find(|k| *k != "name" && *k != "id" && *k != "description" && *k != "type")
                .cloned()
        })
        .or_else(|| def.get("type").and_then(Value::as_str).map(str::to_owned))
        .ok_or_else(|| {
            ApiError::validation(format!(
                "body failed validation: body.properties.{name} should define a property type."
            ))
        })?;
    let known = [
        "title",
        "rich_text",
        "number",
        "select",
        "multi_select",
        "status",
        "date",
        "people",
        "files",
        "checkbox",
        "url",
        "email",
        "phone_number",
        "formula",
        "relation",
        "rollup",
        "created_time",
        "created_by",
        "last_edited_time",
        "last_edited_by",
        "unique_id",
    ];
    if !known.contains(&ty.as_str()) {
        return Err(ApiError::validation(format!(
            "body failed validation: body.properties.{name}.{ty} is not a supported property type."
        )));
    }
    let mut config = def.get(&ty).cloned().unwrap_or(json!({}));
    if matches!(ty.as_str(), "select" | "multi_select" | "status") {
        let mut opts = Vec::new();
        for o in config["options"].as_array().cloned().unwrap_or_default() {
            let id = short_id(ctx);
            opts.push(json!({"id": id, "name": s(&o, "name"), "color": o.get("color").cloned().unwrap_or(json!("default")), "description": null}));
        }
        config["options"] = Value::Array(opts);
    }
    let id = if ty == "title" {
        "title".to_owned()
    } else {
        short_id(ctx)
    };
    Ok(
        json!({"id": id, "name": def.get("name").and_then(Value::as_str).unwrap_or(name), "description": null, "type": ty, ty.clone(): config}),
    )
}

pub fn create_ds(ctx: &mut Ctx, body: &Value) -> Res {
    let parent = parse_json_field(body.get("parent").unwrap_or(&Value::Null));
    let props = parse_json_field(body.get("properties").unwrap_or(&Value::Null));
    let Some(prop_map) = props.as_object() else {
        return Err(ApiError::validation(
            "body failed validation: body.properties should be defined, instead was `undefined`.",
        ));
    };
    let now = ctx.iso_now();
    let bot = ctx.bot_id();
    let title = json!(rich_text(ctx, body.get("title").unwrap_or(&json!([])))?);
    let db_id = if let Some(db) = parent.get("database_id").and_then(Value::as_str) {
        let id = norm_id(db).ok_or_else(|| {
            ApiError::validation(
                "body failed validation: body.parent.database_id should be a valid uuid.",
            )
        })?;
        ctx.get("databases", &id)
            .ok_or_else(|| ApiError::not_found("database", &id))?;
        id
    } else if let Some(p) = parent.get("page_id").and_then(Value::as_str) {
        // A page parent creates a new database holding the data source.
        let pid = norm_id(p).ok_or_else(|| {
            ApiError::validation(
                "body failed validation: body.parent.page_id should be a valid uuid.",
            )
        })?;
        ctx.get("pages", &pid)
            .ok_or_else(|| ApiError::not_found("page", &pid))?;
        let id = ctx.new_id("database");
        let order = next_order(ctx, &pid);
        ctx.put(
            "databases",
            json!({"object": "database", "id": id, "title": title, "description": [], "parent": {"type": "page_id", "page_id": pid},
                   "is_inline": false, "in_trash": false, "is_locked": false, "created_time": now, "last_edited_time": now,
                   "created_by": user_ref(&bot), "last_edited_by": user_ref(&bot), "icon": null, "cover": null, "public_url": null, "_order": order}),
        );
        id
    } else {
        return Err(ApiError::validation(
            "body failed validation: body.parent should have `database_id` or `page_id`.",
        ));
    };
    let mut schema = Map::new();
    for (name, def) in prop_map {
        schema.insert(name.clone(), schema_from_request(ctx, name, def)?);
    }
    if !schema.values().any(|p| s(p, "type") == "title") {
        return Err(ApiError::validation(
            "Data source schema must include a title property.",
        ));
    }
    let id = ctx.new_id("data_source");
    let ds = json!({
        "object": "data_source", "id": id, "created_time": now, "last_edited_time": now,
        "created_by": user_ref(&bot), "last_edited_by": user_ref(&bot), "title": title, "description": [],
        "icon": null, "cover": null, "properties": schema, "parent": {"type": "database_id", "database_id": db_id},
        "archived": false, "in_trash": false, "public_url": null,
    });
    ctx.put("data_sources", ds.clone());
    Ok(ds_out(ctx, &ds))
}

pub fn update_ds(ctx: &mut Ctx, raw: &str, body: &Value) -> Res {
    let mut ds = need_ds(ctx, raw)?;
    if let Some(t) = body.get("title") {
        ds["title"] = json!(rich_text(ctx, &parse_json_field(t))?);
    }
    if let Some(d) = body.get("description") {
        ds["description"] = json!(rich_text(ctx, &parse_json_field(d))?);
    }
    if let Some(props) = body.get("properties") {
        let props = parse_json_field(props);
        for (key, def) in props.as_object().cloned().unwrap_or_default() {
            let existing = ds["properties"].as_object().and_then(|m| {
                m.iter()
                    .find(|(n, p)| **n == key || s(p, "id") == key)
                    .map(|(n, _)| n.clone())
            });
            match (existing, def.is_null()) {
                (Some(name), true) => {
                    if s(&ds["properties"][&name], "type") == "title" {
                        return Err(ApiError::validation("Cannot delete the title property."));
                    }
                    ds["properties"]
                        .as_object_mut()
                        .expect("object")
                        .remove(&name);
                }
                (Some(name), false) => {
                    let mut cur = ds["properties"][&name].clone();
                    let mut new_name = name.clone();
                    if let Some(n) = def.get("name").and_then(Value::as_str) {
                        new_name = n.to_owned();
                        cur["name"] = json!(n);
                    }
                    let has_type = def
                        .as_object()
                        .is_some_and(|o| o.keys().any(|k| k != "name"));
                    if has_type {
                        let mut rebuilt = schema_from_request(ctx, &new_name, &def)?;
                        if s(&rebuilt, "type") == s(&cur, "type") {
                            rebuilt["id"] = cur["id"].clone();
                        }
                        rebuilt["name"] = json!(new_name);
                        cur = rebuilt;
                    }
                    let m = ds["properties"].as_object_mut().expect("object");
                    m.remove(&name);
                    m.insert(new_name, cur);
                }
                (None, true) => {
                    return Err(ApiError::validation(format!(
                        "{key} is not a property that exists."
                    )));
                }
                (None, false) => {
                    let p = schema_from_request(ctx, &key, &def)?;
                    let n = s(&p, "name").to_owned();
                    ds["properties"][&n] = p;
                }
            }
        }
    }
    ds["last_edited_time"] = json!(ctx.iso_now());
    ctx.put("data_sources", ds.clone());
    Ok(ds_out(ctx, &ds))
}

pub fn templates(ctx: &mut Ctx, raw: &str, cursor: Option<&str>, size: Option<i64>) -> Res {
    let ds = need_ds(ctx, raw)?;
    let list: Vec<Value> = ctx
        .all("templates")
        .into_iter()
        .filter(|t| s(t, "data_source_id") == s(&ds, "id"))
        .map(|t| json!({"id": s(&t, "id"), "name": s(&t, "name"), "is_default": t["is_default"].as_bool().unwrap_or(false)}))
        .collect();
    let (page, next) = paginate(list, cursor, size)?;
    Ok(json!({"templates": page, "has_more": next.is_some(), "next_cursor": next}))
}

// ───────────────────────────── query ─────────────────────────────

fn prop_text(p: &Value) -> Option<String> {
    let ty = s(p, "type");
    match ty {
        "title" | "rich_text" => Some(plain(&p[ty])),
        "url" | "email" | "phone_number" => p[ty].as_str().map(str::to_owned),
        _ => None,
    }
}

fn date_key(v: &Value) -> Option<String> {
    v.as_str().map(|x| x.chars().take(19).collect())
}

fn cmp_date(value: Option<&str>, cond: &Value, now: &str) -> Result<bool, ApiError> {
    let v = value.map(|x| x.chars().take(19).collect::<String>());
    let c = cond
        .as_object()
        .ok_or_else(|| ApiError::validation("date filter should be an object"))?;
    let (op, arg) = c
        .iter()
        .next()
        .ok_or_else(|| ApiError::validation("date filter should have a condition"))?;
    let a = date_key(arg).unwrap_or_default();
    let a10: String = a.chars().take(10).collect();
    Ok(match op.as_str() {
        "is_empty" => v.is_none(),
        "is_not_empty" => v.is_some(),
        _ => {
            let Some(v) = v else { return Ok(false) };
            let v10: String = v.chars().take(10).collect();
            let shift = |days: i64| {
                crate::model::iso(crate::pages::parse_day(now) + days * 86_400)[..10].to_owned()
            };
            match op.as_str() {
                "equals" => {
                    if a.len() <= 10 {
                        v10 == a10
                    } else {
                        v == a
                    }
                }
                "before" => v < a,
                "after" => v > a && !(a.len() <= 10 && v10 == a10),
                "on_or_before" => v <= a || v10 == a10,
                "on_or_after" => v >= a || v10 == a10,
                "past_week" => v10 >= shift(-7) && v10 <= shift(0),
                "past_month" => v10 >= shift(-30) && v10 <= shift(0),
                "past_year" => v10 >= shift(-365) && v10 <= shift(0),
                "next_week" => v10 >= shift(0) && v10 <= shift(7),
                "next_month" => v10 >= shift(0) && v10 <= shift(30),
                "next_year" => v10 >= shift(0) && v10 <= shift(365),
                "this_week" => v10 >= shift(-3) && v10 <= shift(3),
                other => {
                    return Err(ApiError::validation(format!(
                        "body failed validation: date filter condition `{other}` is not supported."
                    )));
                }
            }
        }
    })
}

pub fn parse_day(iso_text: &str) -> i64 {
    let parts: Vec<i64> = iso_text
        .get(0..10)
        .unwrap_or("1970-01-01")
        .split('-')
        .filter_map(|x| x.parse().ok())
        .collect();
    if parts.len() != 3 {
        return 0;
    }
    let (y, m, d) = (parts[0], parts[1], parts[2]);
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) * 86_400
}

fn matches_cond(ctx: &Ctx, p: &Value, filter_type: &str, cond: &Value) -> Result<bool, ApiError> {
    let ty = s(p, "type");
    let compatible = filter_type == ty
        || (filter_type == "rich_text" && matches!(ty, "title" | "url" | "email" | "phone_number"))
        || (filter_type == "date" && matches!(ty, "created_time" | "last_edited_time"))
        || (filter_type == "people" && matches!(ty, "created_by" | "last_edited_by"));
    if !compatible {
        return Err(ApiError::validation(format!(
            "body failed validation: the property type `{ty}` does not match the filter type `{filter_type}`."
        )));
    }
    let c = cond
        .as_object()
        .ok_or_else(|| ApiError::validation("filter condition should be an object"))?;
    let Some((op, arg)) = c.iter().next() else {
        return Err(ApiError::validation("filter condition should not be empty"));
    };
    let now = ctx.iso_now();
    let value = &p[ty];
    Ok(match ty {
        "title" | "rich_text" | "url" | "email" | "phone_number" => {
            let t = prop_text(p).unwrap_or_default();
            let a = arg.as_str().unwrap_or("");
            let (tl, al) = (t.to_lowercase(), a.to_lowercase());
            match op.as_str() {
                "equals" => t == a,
                "does_not_equal" => t != a,
                "contains" => tl.contains(&al),
                "does_not_contain" => !tl.contains(&al),
                "starts_with" => tl.starts_with(&al),
                "ends_with" => tl.ends_with(&al),
                "is_empty" => t.is_empty(),
                "is_not_empty" => !t.is_empty(),
                other => {
                    return Err(ApiError::validation(format!(
                        "Unsupported text filter condition `{other}`."
                    )));
                }
            }
        }
        "number" => {
            let v = value.as_f64();
            let a = arg.as_f64();
            match op.as_str() {
                "is_empty" => v.is_none(),
                "is_not_empty" => v.is_some(),
                _ => match (v, a) {
                    (Some(v), Some(a)) => match op.as_str() {
                        "equals" => v == a,
                        "does_not_equal" => v != a,
                        "greater_than" => v > a,
                        "less_than" => v < a,
                        "greater_than_or_equal_to" => v >= a,
                        "less_than_or_equal_to" => v <= a,
                        other => {
                            return Err(ApiError::validation(format!(
                                "Unsupported number filter condition `{other}`."
                            )));
                        }
                    },
                    _ => op == "does_not_equal",
                },
            }
        }
        "checkbox" => {
            let v = value.as_bool().unwrap_or(false);
            match op.as_str() {
                "equals" => Some(v) == arg.as_bool(),
                "does_not_equal" => Some(v) != arg.as_bool(),
                other => {
                    return Err(ApiError::validation(format!(
                        "Unsupported checkbox filter condition `{other}`."
                    )));
                }
            }
        }
        "select" | "status" => {
            let name = value.get("name").and_then(Value::as_str);
            match op.as_str() {
                "equals" => name.is_some_and(|n| Some(n) == arg.as_str()),
                "does_not_equal" => name != arg.as_str(),
                "is_empty" => name.is_none(),
                "is_not_empty" => name.is_some(),
                other => {
                    return Err(ApiError::validation(format!(
                        "Unsupported {ty} filter condition `{other}`."
                    )));
                }
            }
        }
        "multi_select" => {
            let names: Vec<&str> = value
                .as_array()
                .map(|a| a.iter().map(|o| s(o, "name")).collect())
                .unwrap_or_default();
            match op.as_str() {
                "contains" => names.iter().any(|n| Some(*n) == arg.as_str()),
                "does_not_contain" => !names.iter().any(|n| Some(*n) == arg.as_str()),
                "is_empty" => names.is_empty(),
                "is_not_empty" => !names.is_empty(),
                other => {
                    return Err(ApiError::validation(format!(
                        "Unsupported multi_select filter condition `{other}`."
                    )));
                }
            }
        }
        "date" => cmp_date(value.get("start").and_then(Value::as_str), cond, &now)?,
        "created_time" | "last_edited_time" => cmp_date(value.as_str(), cond, &now)?,
        "people" | "relation" | "created_by" | "last_edited_by" => {
            let ids: Vec<String> = match value {
                Value::Array(a) => a.iter().map(|x| s(x, "id").to_owned()).collect(),
                Value::Object(_) => vec![s(value, "id").to_owned()],
                _ => Vec::new(),
            };
            let a = arg.as_str().and_then(norm_id).unwrap_or_default();
            match op.as_str() {
                "contains" => ids.contains(&a),
                "does_not_contain" => !ids.contains(&a),
                "is_empty" => ids.is_empty(),
                "is_not_empty" => !ids.is_empty(),
                other => {
                    return Err(ApiError::validation(format!(
                        "Unsupported {ty} filter condition `{other}`."
                    )));
                }
            }
        }
        "unique_id" => {
            let n = value["number"].as_f64();
            let a = arg.as_f64();
            match (op.as_str(), n, a) {
                ("equals", Some(n), Some(a)) => n == a,
                ("does_not_equal", Some(n), Some(a)) => n != a,
                ("greater_than", Some(n), Some(a)) => n > a,
                ("less_than", Some(n), Some(a)) => n < a,
                ("greater_than_or_equal_to", Some(n), Some(a)) => n >= a,
                ("less_than_or_equal_to", Some(n), Some(a)) => n <= a,
                _ => false,
            }
        }
        "formula" => {
            let inner_ty = s(value, "type");
            let inner = json!({"type": inner_ty, inner_ty: value.get(inner_ty).cloned().unwrap_or(Value::Null)});
            let (sub_ty, sub_cond) = c
                .iter()
                .next()
                .map(|(k, v)| (k.clone(), v.clone()))
                .unwrap_or_default();
            let mapped = match sub_ty.as_str() {
                "string" => "rich_text",
                other => other,
            };
            let as_prop = if inner_ty == "string" {
                json!({"type": "rich_text", "rich_text": [{"plain_text": value["string"].as_str().unwrap_or("")}]})
            } else {
                inner
            };
            matches_cond(ctx, &as_prop, mapped, &sub_cond)?
        }
        other => {
            return Err(ApiError::validation(format!(
                "Filtering on {other} properties is not supported."
            )));
        }
    })
}

fn eval_filter(ctx: &Ctx, page: &Value, f: &Value, depth: usize) -> Result<bool, ApiError> {
    if depth > 2 {
        return Err(ApiError::validation(
            "body failed validation: body.filter nesting is limited to two levels.",
        ));
    }
    if let Some(all) = f.get("and").and_then(Value::as_array) {
        for x in all {
            if !eval_filter(ctx, page, x, depth + 1)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    if let Some(any) = f.get("or").and_then(Value::as_array) {
        for x in any {
            if eval_filter(ctx, page, x, depth + 1)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    if let Some(ts) = f.get("timestamp").and_then(Value::as_str) {
        let cond = f.get(ts).ok_or_else(|| {
            ApiError::validation(format!(
                "body failed validation: body.filter.{ts} should be defined."
            ))
        })?;
        return cmp_date(page[ts].as_str(), cond, &ctx.iso_now());
    }
    let prop_key = f.get("property").and_then(Value::as_str).ok_or_else(|| {
        ApiError::validation("body failed validation: body.filter should have `property`, `timestamp`, `and` or `or`.")
    })?;
    let props = page["properties"].as_object().cloned().unwrap_or_default();
    let p = props
        .iter()
        .find(|(n, p)| *n == prop_key || s(p, "id") == prop_key)
        .map(|(_, p)| p.clone())
        .ok_or_else(|| {
            ApiError::validation(format!(
                "Could not find property with name or id: {prop_key}"
            ))
        })?;
    let (filter_type, cond) = f
        .as_object()
        .and_then(|o| {
            o.iter()
                .find(|(k, _)| *k != "property")
                .map(|(k, v)| (k.clone(), v.clone()))
        })
        .ok_or_else(|| {
            ApiError::validation(
                "body failed validation: body.filter should have a property type condition.",
            )
        })?;
    matches_cond(ctx, &p, &filter_type, &cond)
}

fn sort_value(p: &Value, schema: Option<&Value>) -> (i64, String, f64) {
    let ty = s(p, "type");
    let v = &p[ty];
    match ty {
        "number" => (v.is_null() as i64, String::new(), v.as_f64().unwrap_or(0.0)),
        "checkbox" => (
            0,
            String::new(),
            if v.as_bool().unwrap_or(false) {
                1.0
            } else {
                0.0
            },
        ),
        "select" | "status" => {
            let name = s(v, "name");
            let pos = schema
                .and_then(|sc| sc[ty]["options"].as_array())
                .and_then(|o| o.iter().position(|x| s(x, "name") == name))
                .unwrap_or(usize::MAX);
            (v.is_null() as i64, String::new(), pos as f64)
        }
        "date" => (v.is_null() as i64, s(v, "start").to_owned(), 0.0),
        "created_time" | "last_edited_time" => (0, v.as_str().unwrap_or("").to_owned(), 0.0),
        "unique_id" => (0, String::new(), v["number"].as_f64().unwrap_or(0.0)),
        _ => {
            let t = prop_text(p).unwrap_or_default();
            (t.is_empty() as i64, t.to_lowercase(), 0.0)
        }
    }
}

pub fn query(ctx: &mut Ctx, raw: &str, body: &Value, filter_props: &[String]) -> Res {
    let ds = need_ds(ctx, raw)?;
    let include_trash = body
        .get("in_trash")
        .or_else(|| body.get("archived"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let filter = parse_json_field(body.get("filter").unwrap_or(&Value::Null));
    let sorts = match parse_json_field(body.get("sorts").unwrap_or(&Value::Null)) {
        Value::Array(a) => a,
        Value::Null => Vec::new(),
        _ => {
            return Err(ApiError::validation(
                "body failed validation: body.sorts should be an array.",
            ));
        }
    };
    let mut rows: Vec<Value> = Vec::new();
    for p in ctx.all("pages") {
        if s(&p["parent"], "data_source_id") != s(&ds, "id") {
            continue;
        }
        if !include_trash && p["in_trash"].as_bool().unwrap_or(false) {
            continue;
        }
        let out = page_out(ctx, &p, None);
        if filter.is_object() && !eval_filter(ctx, &out, &filter, 0)? {
            continue;
        }
        rows.push(out);
    }
    if sorts.is_empty() {
        rows.sort_by(|a, b| s(b, "created_time").cmp(s(a, "created_time")));
    } else {
        for srt in sorts.iter().rev() {
            let desc = s(srt, "direction") == "descending";
            if let Some(ts) = srt.get("timestamp").and_then(Value::as_str) {
                rows.sort_by(|a, b| {
                    let o = s(a, ts).cmp(s(b, ts));
                    if desc { o.reverse() } else { o }
                });
                continue;
            }
            let key = s(srt, "property").to_owned();
            let name = ds["properties"]
                .as_object()
                .and_then(|m| {
                    m.iter()
                        .find(|(n, p)| **n == key || s(p, "id") == key)
                        .map(|(n, _)| n.clone())
                })
                .ok_or_else(|| {
                    ApiError::validation(format!(
                        "Could not find sort property with name or id: {key}"
                    ))
                })?;
            let schema = ds["properties"][&name].clone();
            rows.sort_by(|a, b| {
                let (ea, sa, na) = sort_value(&a["properties"][&name], Some(&schema));
                let (eb, sb, nb) = sort_value(&b["properties"][&name], Some(&schema));
                // Empty values sort last in either direction.
                ea.cmp(&eb).then_with(|| {
                    let o = sa.cmp(&sb).then(na.total_cmp(&nb));
                    if desc { o.reverse() } else { o }
                })
            });
        }
    }
    if !filter_props.is_empty() {
        for r in &mut rows {
            if let Some(m) = r["properties"].as_object_mut() {
                m.retain(|_, v| filter_props.iter().any(|f| f == s(v, "id")));
            }
        }
    }
    let (page, next) = paginate(
        rows,
        body.get("start_cursor").and_then(Value::as_str),
        body.get("page_size").and_then(Value::as_i64),
    )?;
    let rid = ctx.request_id();
    Ok(list(page, next, "page_or_data_source", &rid))
}

// ───────────────────────────── comments ─────────────────────────────

fn comment_out(c: &Value) -> Value {
    public(c.clone())
}

pub fn list_comments(
    ctx: &mut Ctx,
    block: Option<&str>,
    cursor: Option<&str>,
    size: Option<i64>,
) -> Res {
    let raw = block.ok_or_else(|| {
        ApiError::validation(
            "query failed validation: query.block_id should be defined, instead was `undefined`.",
        )
    })?;
    let id = need_id(raw, "block_id")?;
    if ctx.get("pages", &id).is_none() && ctx.get("blocks", &id).is_none() {
        return Err(ApiError::not_found("block", &id));
    }
    let items: Vec<Value> = ctx
        .all("comments")
        .into_iter()
        .filter(|c| s(&c["parent"], "page_id") == id || s(&c["parent"], "block_id") == id)
        .map(|c| comment_out(&c))
        .collect();
    let (page, next) = paginate(items, cursor, size)?;
    let rid = ctx.request_id();
    Ok(list(page, next, "comment", &rid))
}

pub fn create_comment(ctx: &mut Ctx, body: &Value) -> Res {
    let parent = parse_json_field(body.get("parent").unwrap_or(&Value::Null));
    let rt = json!(rich_text(
        ctx,
        &parse_json_field(body.get("rich_text").unwrap_or(&Value::Null))
    )?);
    if rt.as_array().is_none_or(Vec::is_empty) {
        return Err(ApiError::validation(
            "body failed validation: body.rich_text should be a non-empty array.",
        ));
    }
    let (parent_out, discussion) = if let Some(d) =
        body.get("discussion_id").and_then(Value::as_str)
    {
        let did = norm_id(d).ok_or_else(|| {
            ApiError::validation(
                "body failed validation: body.discussion_id should be a valid uuid.",
            )
        })?;
        let first = ctx
            .all("comments")
            .into_iter()
            .find(|c| s(c, "discussion_id") == did)
            .ok_or_else(|| ApiError::not_found("discussion", &did))?;
        (first["parent"].clone(), did)
    } else {
        let raw = parent.get("page_id").or_else(|| parent.get("block_id")).and_then(Value::as_str).ok_or_else(|| {
            ApiError::validation("body failed validation: body.parent.page_id should be defined, instead was `undefined`.")
        })?;
        let id = norm_id(raw).ok_or_else(|| {
            ApiError::validation(
                "body failed validation: body.parent.page_id should be a valid uuid.",
            )
        })?;
        let p = if ctx.get("pages", &id).is_some() {
            json!({"type": "page_id", "page_id": id})
        } else if ctx.get("blocks", &id).is_some() {
            json!({"type": "block_id", "block_id": id})
        } else {
            return Err(ApiError::not_found("page", &id));
        };
        (p, ctx.new_id("discussion"))
    };
    let id = ctx.new_id("comment");
    let now = ctx.iso_now();
    let bot = ctx.bot();
    let c = json!({
        "object": "comment", "id": id, "parent": parent_out, "discussion_id": discussion,
        "created_time": now, "last_edited_time": now, "created_by": user_ref(s(&bot, "id")),
        "rich_text": rt, "display_name": {"type": "integration", "resolved_name": s(&bot, "name")},
    });
    ctx.put("comments", c.clone());
    let mut out = comment_out(&c);
    out["request_id"] = json!(ctx.request_id());
    Ok(out)
}
