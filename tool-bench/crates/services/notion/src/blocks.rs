//! Blocks: children listing/appending, retrieval, updates, deletion, and the page
//! Markdown endpoints (render blocks to enhanced Markdown and back).

use serde_json::{Value, json};

use crate::{
    model::{
        ApiError, Ctx, Res, list, need_id, norm_id, paginate, plain, public, rich_text, s, user_ref,
    },
    pages::title_of,
};

pub const TEXT_TYPES: &[&str] = &[
    "paragraph",
    "heading_1",
    "heading_2",
    "heading_3",
    "bulleted_list_item",
    "numbered_list_item",
    "to_do",
    "toggle",
    "quote",
    "callout",
    "code",
];

const OTHER_TYPES: &[&str] = &[
    "divider",
    "image",
    "bookmark",
    "embed",
    "video",
    "file",
    "pdf",
    "equation",
    "table_of_contents",
    "breadcrumb",
    "column_list",
    "column",
    "table",
    "table_row",
    "link_to_page",
    "synced_block",
];

fn order_of(v: &Value) -> f64 {
    v.get("_order").and_then(Value::as_f64).unwrap_or(0.0)
}

fn parent_id(v: &Value) -> String {
    let p = &v["parent"];
    match s(p, "type") {
        "page_id" => s(p, "page_id").to_owned(),
        "block_id" => s(p, "block_id").to_owned(),
        "data_source_id" => s(p, "data_source_id").to_owned(),
        "database_id" => s(p, "database_id").to_owned(),
        _ => String::new(),
    }
}

/// Every child (blocks, child pages, child databases) of a page or block, in order.
pub fn children_raw(ctx: &Ctx, id: &str) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for b in ctx.all("blocks") {
        if parent_id(&b) == id && !b["in_trash"].as_bool().unwrap_or(false) {
            out.push(b);
        }
    }
    for p in ctx.all("pages") {
        if s(&p["parent"], "type") == "page_id"
            && s(&p["parent"], "page_id") == id
            && !p["in_trash"].as_bool().unwrap_or(false)
        {
            out.push(page_as_block(ctx, &p));
        }
    }
    for d in ctx.all("databases") {
        if s(&d["parent"], "type") == "page_id"
            && s(&d["parent"], "page_id") == id
            && !d["in_trash"].as_bool().unwrap_or(false)
        {
            out.push(database_as_block(&d));
        }
    }
    out.sort_by(|a, b| {
        order_of(a)
            .total_cmp(&order_of(b))
            .then(s(a, "created_time").cmp(s(b, "created_time")))
    });
    out
}

pub fn page_as_block(ctx: &Ctx, p: &Value) -> Value {
    json!({
        "object": "block", "id": s(p, "id"), "parent": p["parent"], "created_time": p["created_time"],
        "last_edited_time": p["last_edited_time"], "created_by": p["created_by"], "last_edited_by": p["last_edited_by"],
        "has_children": !children_raw(ctx, s(p, "id")).is_empty(), "archived": p["archived"], "in_trash": p["in_trash"],
        "type": "child_page", "child_page": {"title": title_of(p)}, "_order": order_of(p),
    })
}

fn database_as_block(d: &Value) -> Value {
    json!({
        "object": "block", "id": s(d, "id"), "parent": d["parent"], "created_time": d["created_time"],
        "last_edited_time": d["last_edited_time"], "created_by": d["created_by"], "last_edited_by": d["last_edited_by"],
        "has_children": false, "archived": d["in_trash"], "in_trash": d["in_trash"],
        "type": "child_database", "child_database": {"title": plain(&d["title"])}, "_order": order_of(d),
    })
}

pub fn block_out(ctx: &Ctx, b: &Value) -> Value {
    let mut out = public(b.clone());
    if s(b, "type") != "child_page" && s(b, "type") != "child_database" {
        out["has_children"] = json!(!children_raw(ctx, s(b, "id")).is_empty());
    }
    out
}

/// A page, block, or database the caller can address as a block parent.
fn find_container(ctx: &Ctx, id: &str) -> Option<(&'static str, Value)> {
    if let Some(p) = ctx.get("pages", id) {
        return Some(("page", p));
    }
    if let Some(b) = ctx.get("blocks", id) {
        return Some(("block", b));
    }
    None
}

pub fn get_children(ctx: &mut Ctx, raw: &str, cursor: Option<&str>, size: Option<i64>) -> Res {
    let id = need_id(raw, "block_id")?;
    if find_container(ctx, &id).is_none() {
        return Err(ApiError::not_found("block", &id));
    }
    let items: Vec<Value> = children_raw(ctx, &id)
        .iter()
        .map(|b| block_out(ctx, b))
        .collect();
    let (page, next) = paginate(items, cursor, size)?;
    let rid = ctx.request_id();
    Ok(list(page, next, "block", &rid))
}

/// Builds a stored block from a request block object.
fn build_block(
    ctx: &mut Ctx,
    input: &Value,
    parent: Value,
    order: f64,
    path: &str,
) -> Result<Vec<Value>, ApiError> {
    let input = match input {
        Value::String(t) => serde_json::from_str::<Value>(t).unwrap_or(Value::Null),
        other => other.clone(),
    };
    let ty = match s(&input, "type") {
        "" => input
            .as_object()
            .and_then(|o| {
                o.keys()
                    .find(|k| TEXT_TYPES.contains(&k.as_str()) || OTHER_TYPES.contains(&k.as_str()))
                    .cloned()
            })
            .unwrap_or_default(),
        t => t.to_owned(),
    };
    if ty == "child_page" || ty == "child_database" {
        return Err(ApiError::validation(format!(
            "body failed validation: {path}.{ty} should be not present. Use the create page or create database endpoint instead."
        )));
    }
    if !TEXT_TYPES.contains(&ty.as_str()) && !OTHER_TYPES.contains(&ty.as_str()) {
        return Err(ApiError::validation(format!(
            "body failed validation: {path} should be a valid block object, instead the block type `{ty}` is not supported."
        )));
    }
    let content = input.get(&ty).cloned().unwrap_or(json!({}));
    let mut body = json!({});
    if TEXT_TYPES.contains(&ty.as_str()) {
        let rt = content
            .get("rich_text")
            .or_else(|| content.get("text"))
            .cloned()
            .unwrap_or(json!([]));
        body["rich_text"] = json!(rich_text(ctx, &rt)?);
        body["color"] = content.get("color").cloned().unwrap_or(json!("default"));
        match ty.as_str() {
            "to_do" => body["checked"] = json!(content["checked"].as_bool().unwrap_or(false)),
            "heading_1" | "heading_2" | "heading_3" => {
                body["is_toggleable"] = json!(content["is_toggleable"].as_bool().unwrap_or(false))
            }
            "code" => {
                body["caption"] = json!([]);
                body["language"] = content
                    .get("language")
                    .cloned()
                    .unwrap_or(json!("plain text"));
                body.as_object_mut().expect("object").remove("color");
            }
            "callout" => {
                body["icon"] = content
                    .get("icon")
                    .cloned()
                    .unwrap_or(json!({"type": "emoji", "emoji": "💡"}))
            }
            _ => {}
        }
    } else {
        body = content.clone();
        body.as_object_mut().map(|m| m.remove("children"));
    }
    let id = ctx.new_id("block");
    let now = ctx.iso_now();
    let bot = ctx.bot_id();
    let doc = json!({
        "object": "block", "id": id, "parent": parent, "created_time": now, "last_edited_time": now,
        "created_by": user_ref(&bot), "last_edited_by": user_ref(&bot), "has_children": false,
        "archived": false, "in_trash": false, "type": ty, ty.clone(): body, "_order": order,
    });
    let mut out = vec![doc];
    if let Some(kids) = content.get("children").and_then(Value::as_array) {
        for (i, k) in kids.iter().enumerate() {
            let mut nested = build_block(
                ctx,
                k,
                json!({"type": "block_id", "block_id": id}),
                i as f64,
                &format!("{path}.{ty}.children[{i}]"),
            )?;
            out.append(&mut nested);
        }
    }
    Ok(out)
}

/// Appends blocks under `parent_id` (optionally after a sibling). Returns the top-level new blocks.
pub fn append(
    ctx: &mut Ctx,
    parent_id: &str,
    children: &[Value],
    after: Option<&str>,
) -> Result<Vec<Value>, ApiError> {
    if children.len() > 100 {
        return Err(ApiError::validation(format!(
            "body failed validation: body.children.length should be ≤ `100`, instead was `{}`.",
            children.len()
        )));
    }
    let (kind, container) =
        find_container(ctx, parent_id).ok_or_else(|| ApiError::not_found("block", parent_id))?;
    if kind == "block"
        && matches!(
            s(&container, "type"),
            "divider" | "image" | "bookmark" | "embed" | "code" | "equation"
        )
    {
        return Err(ApiError::validation(format!(
            "Block type {} does not support children.",
            s(&container, "type")
        )));
    }
    let parent = if kind == "page" {
        json!({"type": "page_id", "page_id": parent_id})
    } else {
        json!({"type": "block_id", "block_id": parent_id})
    };
    let siblings = children_raw(ctx, parent_id);
    let (mut base, upper) = match after {
        Some(a) => {
            let a = norm_id(a).ok_or_else(|| ApiError::validation(format!("body failed validation: body.after should be a valid uuid, instead was `\"{a}\"`.")))?;
            let pos = siblings
                .iter()
                .position(|b| s(b, "id") == a)
                .ok_or_else(|| {
                    ApiError::validation(format!(
                        "Could not find block with ID: {a} among the children of {parent_id}."
                    ))
                })?;
            let lo = order_of(&siblings[pos]);
            let hi = siblings
                .get(pos + 1)
                .map(order_of)
                .unwrap_or(lo + children.len() as f64 + 1.0);
            (lo, Some(hi))
        }
        None => (siblings.last().map(order_of).unwrap_or(0.0), None),
    };
    let step = match upper {
        Some(hi) => (hi - base) / (children.len() as f64 + 1.0),
        None => 1.0,
    };
    let mut top = Vec::new();
    for (i, c) in children.iter().enumerate() {
        base += step;
        let docs = build_block(ctx, c, parent.clone(), base, &format!("body.children[{i}]"))?;
        top.push(docs[0].clone());
        for d in docs {
            ctx.put("blocks", d);
        }
    }
    touch(ctx, parent_id);
    Ok(top)
}

/// Updates `last_edited_time` of a page (walking up from blocks).
pub fn touch(ctx: &mut Ctx, id: &str) {
    let now = ctx.iso_now();
    let bot = ctx.bot_id();
    let mut cur = id.to_owned();
    for _ in 0..20 {
        if let Some(mut p) = ctx.get("pages", &cur) {
            p["last_edited_time"] = json!(now);
            p["last_edited_by"] = user_ref(&bot);
            ctx.put("pages", p);
            return;
        }
        match ctx.get("blocks", &cur) {
            Some(b) => cur = parent_id(&b),
            None => return,
        }
    }
}

pub fn patch_children(ctx: &mut Ctx, raw: &str, body: &Value) -> Res {
    let id = need_id(raw, "block_id")?;
    let children = match body.get("children") {
        Some(Value::Array(a)) => a.clone(),
        _ => {
            return Err(ApiError::validation(
                "body failed validation: body.children should be defined, instead was `undefined`.",
            ));
        }
    };
    let after = body.get("after").and_then(Value::as_str);
    let top = append(ctx, &id, &children, after)?;
    let results: Vec<Value> = top.iter().map(|b| block_out(ctx, b)).collect();
    let rid = ctx.request_id();
    Ok(list(results, None, "block", &rid))
}

pub fn retrieve(ctx: &mut Ctx, raw: &str) -> Res {
    let id = need_id(raw, "block_id")?;
    if let Some(b) = ctx.get("blocks", &id) {
        return Ok(block_out(ctx, &b));
    }
    if let Some(p) = ctx.get("pages", &id) {
        return Ok(public(page_as_block(ctx, &p)));
    }
    if let Some(d) = ctx.get("databases", &id) {
        return Ok(public(database_as_block(&d)));
    }
    Err(ApiError::not_found("block", &id))
}

pub fn update(ctx: &mut Ctx, raw: &str, body: &Value) -> Res {
    let id = need_id(raw, "block_id")?;
    let Some(mut b) = ctx.get("blocks", &id) else {
        if ctx.get("pages", &id).is_some() {
            if let Some(a) = body
                .get("archived")
                .or_else(|| body.get("in_trash"))
                .and_then(Value::as_bool)
            {
                let mut p = ctx.get("pages", &id).expect("page");
                p["archived"] = json!(a);
                p["in_trash"] = json!(a);
                ctx.put("pages", p.clone());
                return Ok(public(page_as_block(ctx, &p)));
            }
            return Err(ApiError::validation(
                "Block type child_page cannot be updated. Use the update page endpoint instead.",
            ));
        }
        return Err(ApiError::not_found("block", &id));
    };
    if b["in_trash"].as_bool().unwrap_or(false)
        && body.get("archived").and_then(Value::as_bool) != Some(false)
        && body.get("in_trash").and_then(Value::as_bool) != Some(false)
    {
        return Err(ApiError::validation(
            "Can't edit block that is archived. You must unarchive the block before editing.",
        ));
    }
    let ty = s(&b, "type").to_owned();
    // The tool exposes the block payload as `type: {<block type>: {...}}`; the API itself
    // takes `<block type>: {...}` at the top level. Accept both.
    let mut payload = body.get(&ty).cloned();
    if let Some(t) = body.get("type").and_then(Value::as_object) {
        if let Some(v) = t.get(&ty) {
            payload = Some(v.clone());
        } else if let Some((other, _)) = t
            .iter()
            .find(|(k, _)| TEXT_TYPES.contains(&k.as_str()) || OTHER_TYPES.contains(&k.as_str()))
        {
            return Err(ApiError::validation(format!(
                "Block type `{other}` does not match the existing block type `{ty}`."
            )));
        } else if t.contains_key("rich_text") || t.contains_key("checked") {
            payload = Some(Value::Object(t.clone()));
        }
    }
    for other in TEXT_TYPES.iter().chain(OTHER_TYPES.iter()) {
        if *other != ty && body.get(*other).is_some() {
            return Err(ApiError::validation(format!(
                "Block type `{other}` does not match the existing block type `{ty}`."
            )));
        }
    }
    if let Some(p) = payload {
        if let Some(rt) = p.get("rich_text").or_else(|| p.get("text")) {
            b[&ty]["rich_text"] = json!(rich_text(ctx, rt)?);
        }
        if let Some(c) = p.get("checked").and_then(Value::as_bool) {
            if ty != "to_do" {
                return Err(ApiError::validation(format!(
                    "body failed validation: body.{ty}.checked should be not present."
                )));
            }
            b[&ty]["checked"] = json!(c);
        }
        for k in [
            "color",
            "language",
            "is_toggleable",
            "icon",
            "url",
            "caption",
        ] {
            if let Some(v) = p.get(k) {
                b[&ty][k] = v.clone();
            }
        }
    }
    if let Some(a) = body
        .get("archived")
        .or_else(|| body.get("in_trash"))
        .and_then(Value::as_bool)
    {
        b["archived"] = json!(a);
        b["in_trash"] = json!(a);
    }
    b["last_edited_time"] = json!(ctx.iso_now());
    b["last_edited_by"] = user_ref(&ctx.bot_id());
    ctx.put("blocks", b.clone());
    touch(ctx, &parent_id(&b));
    Ok(block_out(ctx, &b))
}

pub fn delete(ctx: &mut Ctx, raw: &str) -> Res {
    let id = need_id(raw, "block_id")?;
    if let Some(mut p) = ctx.get("pages", &id) {
        p["archived"] = json!(true);
        p["in_trash"] = json!(true);
        ctx.put("pages", p.clone());
        return Ok(public(page_as_block(ctx, &p)));
    }
    let mut b = ctx
        .get("blocks", &id)
        .ok_or_else(|| ApiError::not_found("block", &id))?;
    b["archived"] = json!(true);
    b["in_trash"] = json!(true);
    b["last_edited_time"] = json!(ctx.iso_now());
    ctx.put("blocks", b.clone());
    touch(ctx, &parent_id(&b));
    Ok(block_out(ctx, &b))
}

// ───────────────────────────── markdown ─────────────────────────────

fn inline_md(rich: &Value) -> String {
    let mut out = String::new();
    for r in rich.as_array().into_iter().flatten() {
        let mut t = s(r, "plain_text").to_owned();
        let a = &r["annotations"];
        if a["code"].as_bool().unwrap_or(false) {
            t = format!("`{t}`");
        }
        if a["bold"].as_bool().unwrap_or(false) {
            t = format!("**{t}**");
        }
        if a["italic"].as_bool().unwrap_or(false) {
            t = format!("*{t}*");
        }
        if a["strikethrough"].as_bool().unwrap_or(false) {
            t = format!("~~{t}~~");
        }
        if let Some(h) = r["href"].as_str() {
            t = format!("[{t}]({h})");
        }
        out.push_str(&t);
    }
    out
}

fn render(ctx: &Ctx, id: &str, depth: usize, out: &mut Vec<String>) {
    let indent = "\t".repeat(depth);
    let mut number = 0;
    for b in children_raw(ctx, id) {
        let ty = s(&b, "type");
        let body = &b[ty];
        let text = inline_md(&body["rich_text"]);
        if ty == "numbered_list_item" {
            number += 1;
        } else {
            number = 0;
        }
        let line = match ty {
            "paragraph" => text,
            "heading_1" => format!("# {text}"),
            "heading_2" => format!("## {text}"),
            "heading_3" => format!("### {text}"),
            "bulleted_list_item" => format!("- {text}"),
            "numbered_list_item" => format!("{number}. {text}"),
            "to_do" => format!(
                "- [{}] {text}",
                if body["checked"].as_bool().unwrap_or(false) {
                    "x"
                } else {
                    " "
                }
            ),
            "quote" => format!("> {text}"),
            "toggle" => format!("<details>\n{indent}<summary>{text}</summary>"),
            "callout" => format!(
                "<callout icon=\"{}\">{text}</callout>",
                body["icon"]["emoji"].as_str().unwrap_or("💡")
            ),
            "code" => format!(
                "```{}\n{indent}{}\n{indent}```",
                s(body, "language"),
                plain(&body["rich_text"])
            ),
            "divider" => "---".into(),
            "child_page" => format!(
                "<page url=\"{}\">{}</page>",
                crate::model::notion_url(s(body, "title"), s(&b, "id")),
                s(body, "title")
            ),
            "child_database" => format!(
                "<database url=\"{}\">{}</database>",
                crate::model::notion_url(s(body, "title"), s(&b, "id")),
                s(body, "title")
            ),
            "bookmark" | "embed" | "image" => format!(
                "<{ty} url=\"{}\"/>",
                body["url"]
                    .as_str()
                    .or_else(|| body["external"]["url"].as_str())
                    .unwrap_or("")
            ),
            "equation" => format!("$$\n{}\n$$", s(body, "expression")),
            other => format!("<unsupported type=\"{other}\"/>"),
        };
        out.push(format!("{indent}{line}"));
        if ty != "child_page" && ty != "child_database" {
            render(ctx, s(&b, "id"), depth + 1, out);
        }
        if ty == "toggle" {
            out.push(format!("{indent}</details>"));
        }
    }
}

fn line_prefix(b: &Value) -> Option<String> {
    let ty = s(b, "type");
    Some(match ty {
        "paragraph" => String::new(),
        "heading_1" => "# ".into(),
        "heading_2" => "## ".into(),
        "heading_3" => "### ".into(),
        "bulleted_list_item" => "- ".into(),
        "to_do" => format!(
            "- [{}] ",
            if b[ty]["checked"].as_bool().unwrap_or(false) {
                "x"
            } else {
                " "
            }
        ),
        "quote" => "> ".into(),
        _ => return None,
    })
}

fn is_plain(rich: &Value) -> bool {
    rich.as_array().is_some_and(|a| {
        a.iter().all(|r| {
            s(r, "type") == "text"
                && r["href"].is_null()
                && ["bold", "italic", "strikethrough", "underline", "code"]
                    .iter()
                    .all(|k| !r["annotations"][*k].as_bool().unwrap_or(false))
        })
    })
}

/// `update_content` edits that stay within single plain-text blocks are applied in place,
/// keeping block IDs (as Notion does for unchanged structure). Returns false when the
/// edits need a full re-render instead.
fn update_in_place(ctx: &mut Ctx, page_id: &str, edits: &[Value]) -> Result<bool, ApiError> {
    let mut stack = vec![page_id.to_owned()];
    let mut text_blocks: Vec<Value> = Vec::new();
    while let Some(cur) = stack.pop() {
        for b in children_raw(ctx, &cur) {
            if matches!(s(&b, "type"), "child_page" | "child_database") {
                continue;
            }
            stack.push(s(&b, "id").to_owned());
            if line_prefix(&b).is_some() && is_plain(&b[s(&b, "type")]["rich_text"]) {
                text_blocks.push(b);
            }
        }
    }
    let mut changed: Vec<usize> = Vec::new();
    for e in edits {
        let old = s(e, "old_str");
        let new = s(e, "new_str");
        let all = e["replace_all_matches"].as_bool().unwrap_or(false);
        if old.is_empty() || old.contains('\n') || new.contains('\n') {
            return Ok(false);
        }
        let mut hit = false;
        for (i, b) in text_blocks.iter_mut().enumerate() {
            let ty = s(b, "type").to_owned();
            let prefix = line_prefix(b).unwrap_or_default();
            let line = format!("{prefix}{}", plain(&b[&ty]["rich_text"]));
            if !line.contains(old) {
                continue;
            }
            let replaced = if all {
                line.replace(old, new)
            } else {
                line.replacen(old, new, 1)
            };
            let Some(text) = replaced.strip_prefix(&prefix) else {
                return Ok(false);
            };
            b[&ty]["rich_text"] = json!([{"type": "text", "text": {"content": text, "link": null}, "annotations": {"bold": false, "italic": false, "strikethrough": false, "underline": false, "code": false, "color": "default"}, "plain_text": text, "href": null}]);
            if !changed.contains(&i) {
                changed.push(i);
            }
            hit = true;
            if !all {
                break;
            }
        }
        if !hit {
            return Ok(false);
        }
    }
    let now = ctx.iso_now();
    let bot = ctx.bot_id();
    for i in changed {
        let mut b = text_blocks[i].clone();
        b["last_edited_time"] = json!(now);
        b["last_edited_by"] = user_ref(&bot);
        ctx.put("blocks", b);
    }
    Ok(true)
}

pub fn page_markdown(ctx: &Ctx, page_id: &str) -> String {
    let mut lines = Vec::new();
    render(ctx, page_id, 0, &mut lines);
    lines.join("\n")
}

/// Parses enhanced Markdown into request block objects (child page/database tags are
/// returned separately as IDs so they are kept, not recreated).
fn parse_md(md: &str) -> (Vec<Value>, Vec<String>) {
    let mut blocks = Vec::new();
    let mut kept = Vec::new();
    let mut lines = md.lines().peekable();
    let text = |t: &str| json!([{"type": "text", "text": {"content": t}}]);
    while let Some(raw) = lines.next() {
        let line = raw.trim_start_matches('\t');
        if line.trim().is_empty() {
            continue;
        }
        if let Some(lang) = line.strip_prefix("```") {
            let mut code = Vec::new();
            for l in lines.by_ref() {
                if l.trim_start().starts_with("```") {
                    break;
                }
                code.push(l.trim_start_matches('\t'));
            }
            blocks.push(json!({"type": "code", "code": {"rich_text": text(&code.join("\n")), "language": if lang.is_empty() { "plain text" } else { lang }}}));
            continue;
        }
        if line.starts_with("<page ") || line.starts_with("<database ") {
            if let Some(id) = line
                .split("url=\"")
                .nth(1)
                .and_then(|r| r.split('"').next())
                .and_then(norm_id)
            {
                kept.push(id);
            }
            continue;
        }
        let (ty, rest, checked) = if let Some(r) = line.strip_prefix("### ") {
            ("heading_3", r, None)
        } else if let Some(r) = line.strip_prefix("## ") {
            ("heading_2", r, None)
        } else if let Some(r) = line.strip_prefix("# ") {
            ("heading_1", r, None)
        } else if let Some(r) = line.strip_prefix("- [ ] ") {
            ("to_do", r, Some(false))
        } else if let Some(r) = line
            .strip_prefix("- [x] ")
            .or_else(|| line.strip_prefix("- [X] "))
        {
            ("to_do", r, Some(true))
        } else if let Some(r) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            ("bulleted_list_item", r, None)
        } else if let Some(r) = line.strip_prefix("> ") {
            ("quote", r, None)
        } else if line.trim() == "---" {
            blocks.push(json!({"type": "divider", "divider": {}}));
            continue;
        } else if let Some((n, r)) = line
            .split_once(". ")
            .filter(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        {
            let _ = n;
            ("numbered_list_item", r, None)
        } else {
            ("paragraph", line, None)
        };
        let mut body = json!({"rich_text": text(rest)});
        if let Some(c) = checked {
            body["checked"] = json!(c);
        }
        blocks.push(json!({"type": ty, ty: body}));
    }
    (blocks, kept)
}

fn markdown_response(ctx: &Ctx, id: &str) -> Value {
    json!({"object": "page_markdown", "id": id, "markdown": page_markdown(ctx, id), "truncated": false, "unknown_block_ids": []})
}

pub fn get_markdown(ctx: &mut Ctx, raw: &str) -> Res {
    let id = need_id(raw, "page_id")?;
    let page = ctx
        .get("pages", &id)
        .ok_or_else(|| ApiError::not_found("page", &id))?;
    let _ = page;
    Ok(markdown_response(ctx, &id))
}

/// Replaces the page's block content with `md`.
fn replace_all(ctx: &mut Ctx, id: &str, md: &str, allow_delete: bool) -> Result<(), ApiError> {
    let (blocks, kept) = parse_md(md);
    let existing_pages: Vec<String> = children_raw(ctx, id)
        .iter()
        .filter(|b| matches!(s(b, "type"), "child_page" | "child_database"))
        .map(|b| s(b, "id").to_owned())
        .collect();
    let removed: Vec<&String> = existing_pages
        .iter()
        .filter(|p| !kept.contains(p))
        .collect();
    if !removed.is_empty() && !allow_delete {
        return Err(ApiError::validation(
            "The update would delete child pages or databases. Set allow_deleting_content to true to permit this.",
        ));
    }
    for p in removed {
        if let Some(mut pg) = ctx.get("pages", p) {
            pg["in_trash"] = json!(true);
            pg["archived"] = json!(true);
            ctx.put("pages", pg);
        }
    }
    // Same block structure: update changed blocks in place and keep their IDs.
    let existing: Vec<Value> = children_raw(ctx, id)
        .into_iter()
        .filter(|b| !matches!(s(b, "type"), "child_page" | "child_database"))
        .collect();
    let same_shape = existing.len() == blocks.len()
        && existing
            .iter()
            .zip(&blocks)
            .all(|(e, n)| s(e, "type") == s(n, "type") && children_raw(ctx, s(e, "id")).is_empty());
    if same_shape {
        let now = ctx.iso_now();
        let bot = ctx.bot_id();
        for (e, n) in existing.iter().zip(&blocks) {
            let ty = s(e, "type").to_owned();
            if ty == "divider" {
                continue;
            }
            let new_text = n[&ty]["rich_text"][0]["text"]["content"]
                .as_str()
                .unwrap_or("")
                .to_owned();
            let checked = n[&ty].get("checked").cloned();
            let unchanged = plain(&e[&ty]["rich_text"]) == new_text
                && checked.as_ref().is_none_or(|c| e[&ty]["checked"] == *c);
            if unchanged {
                continue;
            }
            let mut b = e.clone();
            b[&ty]["rich_text"] = json!(rich_text(ctx, &n[&ty]["rich_text"])?);
            if let Some(c) = checked {
                b[&ty]["checked"] = c;
            }
            b["last_edited_time"] = json!(now);
            b["last_edited_by"] = user_ref(&bot);
            ctx.put("blocks", b);
        }
        return Ok(());
    }
    // Drop the current non-page blocks (recursively).
    let mut stack = vec![id.to_owned()];
    while let Some(cur) = stack.pop() {
        for b in ctx.all("blocks") {
            if parent_id(&b) == cur {
                stack.push(s(&b, "id").to_owned());
                ctx.st.store.collection_mut("blocks").remove(s(&b, "id"));
            }
        }
    }
    append(ctx, id, &blocks, None)?;
    Ok(())
}

/// Resolves an ellipsis selection `"start...end"` to a byte range in `md`.
fn ellipsis_range(md: &str, sel: &str) -> Option<(usize, usize)> {
    match sel.split_once("...") {
        Some((a, b)) => {
            let start = md.find(a)?;
            let end_rel = md[start + a.len()..].find(b)?;
            Some((start, start + a.len() + end_rel + b.len()))
        }
        None => md.find(sel).map(|i| (i, i + sel.len())),
    }
}

pub fn update_markdown(ctx: &mut Ctx, raw: &str, body: &Value) -> Res {
    let id = need_id(raw, "page_id")?;
    let page = ctx
        .get("pages", &id)
        .ok_or_else(|| ApiError::not_found("page", &id))?;
    if page["in_trash"].as_bool().unwrap_or(false) {
        return Err(ApiError::validation(
            "Can't edit page that is archived. You must unarchive the page before editing.",
        ));
    }
    let ty = s(body, "type");
    let op = &body[ty];
    if !op.is_object() {
        return Err(ApiError::validation(format!(
            "body failed validation: body.{ty} should be defined, instead was `undefined`."
        )));
    }
    let current = page_markdown(ctx, &id);
    let allow = op["allow_deleting_content"].as_bool().unwrap_or(false);
    let new_md = match ty {
        "replace_content" => op["new_str"]
            .as_str()
            .ok_or_else(|| ApiError::validation("body failed validation: body.replace_content.new_str should be defined, instead was `undefined`."))?
            .to_owned(),
        "update_content" => {
            let edits = op["content_updates"]
                .as_array()
                .ok_or_else(|| ApiError::validation("body failed validation: body.update_content.content_updates should be defined, instead was `undefined`."))?;
            if edits.len() > 100 {
                return Err(ApiError::validation("body failed validation: body.update_content.content_updates.length should be ≤ `100`."));
            }
            if update_in_place(ctx, &id, edits)? {
                touch(ctx, &id);
                return Ok(markdown_response(ctx, &id));
            }
            let mut md = current.clone();
            for (i, e) in edits.iter().enumerate() {
                let old = s(e, "old_str");
                if old.is_empty() || !md.contains(old) {
                    return Err(ApiError::validation(format!(
                        "content_updates[{i}].old_str did not match any content on the page."
                    )));
                }
                let new = s(e, "new_str");
                md = if e["replace_all_matches"].as_bool().unwrap_or(false) {
                    md.replace(old, new)
                } else {
                    md.replacen(old, new, 1)
                };
            }
            md
        }
        "insert_content" => {
            let content = s(op, "content");
            if let Some(after) = op["after"].as_str() {
                let (_, end) = ellipsis_range(&current, after)
                    .ok_or_else(|| ApiError::validation("insert_content.after did not match any content on the page."))?;
                format!("{}\n{content}{}", &current[..end], &current[end..])
            } else if op["position"]["type"].as_str() == Some("start") {
                format!("{content}\n{current}")
            } else {
                format!("{current}\n{content}")
            }
        }
        "replace_content_range" => {
            let (a, b) = ellipsis_range(&current, s(op, "content_range"))
                .ok_or_else(|| ApiError::validation("replace_content_range.content_range did not match any content on the page."))?;
            format!("{}{}{}", &current[..a], s(op, "content"), &current[b..])
        }
        other => {
            return Err(ApiError::validation(format!(
                "body failed validation: body.type should be one of `\"replace_content\"`, `\"update_content\"`, `\"insert_content\"`, `\"replace_content_range\"`, instead was `\"{other}\"`."
            )));
        }
    };
    replace_all(ctx, &id, &new_md, allow || ty == "insert_content")?;
    touch(ctx, &id);
    Ok(markdown_response(ctx, &id))
}
