//! Canvases: markdown documents split into addressable sections.
//!
//! Stored as `canvases/<id>`: `{id, title, sections: [{id, markdown}], user, created,
//! updated, channels, next_section}`. A dataset may give `content` (markdown) instead of
//! `sections`; it is split on first use. Section IDs are derived from the canvas ID and a
//! per-canvas counter, so reads never consume IDs.

use serde_json::{Map, Value, json};

use crate::{
    model::{CALLER, Ctx},
    tools::{ToolResult, err},
    util::{arg_str, s},
};

fn section_id(canvas: &str, n: u64) -> String {
    let mut h: u64 = 0x9e37_79b9_7f4a_7c15;
    for byte in canvas.bytes().chain(n.to_be_bytes()) {
        h ^= u64::from(byte);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut out = String::from("temp:C:");
    let mut x = h;
    for _ in 0..11 {
        out.push(A[(x % 62) as usize] as char);
        x /= 62;
        if x == 0 {
            x = h.rotate_left(17) ^ n;
        }
    }
    out
}

/// Splits canvas markdown into top-level blocks (headings, paragraphs, lists, tables,
/// code fences, callouts).
pub fn split_blocks(md: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    let mut fence: Option<&str> = None;
    let flush = |cur: &mut Vec<&str>, blocks: &mut Vec<String>| {
        if !cur.is_empty() {
            blocks.push(cur.join("\n"));
            cur.clear();
        }
    };
    let class = |l: &str| -> u8 {
        let t = l.trim_start();
        if t.starts_with('#') {
            1
        } else if t.starts_with("- ")
            || t.starts_with("* ")
            || t.starts_with("+ ")
            || t.split_once(". ")
                .is_some_and(|(n, _)| n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty())
        {
            2
        } else if t.starts_with('|') {
            3
        } else if t.starts_with('>') {
            4
        } else {
            0
        }
    };
    for line in md.lines() {
        let t = line.trim_start();
        if let Some(f) = fence {
            cur.push(line);
            if t.starts_with(f) && cur.len() > 1 {
                fence = None;
                flush(&mut cur, &mut blocks);
            }
            continue;
        }
        if t.starts_with("```") || t.starts_with(":::") {
            flush(&mut cur, &mut blocks);
            let marker = if t.starts_with("```") { "```" } else { ":::" };
            cur.push(line);
            fence = Some(marker);
            continue;
        }
        if t.is_empty() {
            flush(&mut cur, &mut blocks);
            continue;
        }
        let c = class(line);
        if c == 1 {
            flush(&mut cur, &mut blocks);
            blocks.push(line.to_owned());
            continue;
        }
        if let Some(prev) = cur.last()
            && class(prev) != c
            && !(c == 0 && class(prev) == 2 && line.starts_with(' '))
        {
            flush(&mut cur, &mut blocks);
        }
        cur.push(line);
    }
    flush(&mut cur, &mut blocks);
    blocks
}

/// Ensures `sections` exists, splitting `content` if needed.
fn normalized(mut c: Value) -> Value {
    if c.get("sections").and_then(Value::as_array).is_some() {
        return c;
    }
    let id = s(&c, "id").to_owned();
    let content = s(&c, "content").to_owned();
    let blocks = split_blocks(&content);
    let mut n = c.get("next_section").and_then(Value::as_u64).unwrap_or(0);
    let mut sections =
        vec![json!({"id": section_id(&id, n), "markdown": s(&c, "title"), "title": true})];
    n += 1;
    for b in blocks {
        sections.push(json!({"id": section_id(&id, n), "markdown": b}));
        n += 1;
    }
    let obj = c.as_object_mut().expect("canvas object");
    obj.remove("content");
    obj.insert("sections".into(), Value::Array(sections));
    obj.insert("next_section".into(), json!(n));
    c
}

pub fn markdown(c: &Value) -> String {
    let c = normalized(c.clone());
    let body: Vec<&str> = c["sections"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|x| !x.get("title").and_then(Value::as_bool).unwrap_or(false))
                .map(|x| s(x, "markdown"))
                .collect()
        })
        .unwrap_or_default();
    format!("# {}\n\n{}", s(&c, "title"), body.join("\n\n"))
}

fn url(ctx: &Ctx, id: &str) -> String {
    format!("{}/docs/{}/{id}", ctx.base_url(), ctx.team_id())
}

pub fn create(ctx: &mut Ctx, args: &Value) -> ToolResult {
    let title =
        arg_str(args, "title").ok_or_else(|| err("invalid_arguments: `title` is required"))?;
    let content = arg_str(args, "content")
        .ok_or_else(|| err("invalid_arguments: `content` must be a non-empty string"))?;
    if title.len() > 255 {
        return Err(err("invalid_arguments: title too long"));
    }
    let id = ctx.new_id("canvas", "F0");
    let doc = normalized(json!({
        "id": id, "title": title, "content": content, "user": CALLER,
        "created": ctx.now, "updated": ctx.now, "channels": [],
    }));
    ctx.put("canvases", &id, doc);
    Ok(json!({
        "canvas_id": id,
        "title": title,
        "canvas_url": url(ctx, &id),
        "result": format!("Canvas \"{title}\" created successfully."),
    }))
}

pub fn read(ctx: &mut Ctx, args: &Value) -> ToolResult {
    let id = arg_str(args, "canvas_id")
        .ok_or_else(|| err("invalid_arguments: `canvas_id` is required"))?;
    let c = ctx
        .get("canvases", id)
        .ok_or_else(|| err("canvas_not_found"))?;
    if s(&c, "user") != CALLER
        && c.get("is_private")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return Err(err("canvas_not_found"));
    }
    let c = normalized(c);
    let mut mapping = Map::new();
    for sec in c["sections"].as_array().into_iter().flatten() {
        let md = s(sec, "markdown");
        let label = if sec.get("title").and_then(Value::as_bool).unwrap_or(false) {
            format!("[title] {md}")
        } else {
            md.lines().next().unwrap_or("").chars().take(120).collect()
        };
        mapping.insert(s(sec, "id").to_owned(), json!(label));
    }
    let threads: Vec<Value> = c["comment_threads"].as_array().cloned().unwrap_or_default();
    let mut out = json!({
        "canvas_id": id,
        "title": s(&c, "title"),
        "canvas_url": url(ctx, id),
        "markdown_content": markdown(&c),
        "section_id_mapping": mapping,
        "comment_threads": threads,
    });
    if threads.len() > 20 {
        out["comment_threads_total_count"] = json!(threads.len());
    }
    Ok(out)
}

enum Op {
    Append,
    Prepend,
    Replace,
    Delete,
}

pub fn update(ctx: &mut Ctx, args: &Value) -> ToolResult {
    let id = arg_str(args, "canvas_id")
        .ok_or_else(|| err("invalid_arguments: `canvas_id` is required"))?
        .to_owned();
    let c = ctx
        .get("canvases", &id)
        .ok_or_else(|| err("canvas_not_found"))?;
    let mut c = normalized(c);
    if s(&c, "user") != CALLER
        && !c
            .get("editable_by_all")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    {
        return Err(err(
            "canvas_editing_failed: no permission to edit this canvas",
        ));
    }
    let mut ops: Vec<(Op, Option<String>, Option<String>)> = Vec::new();
    let parse_op = |t: &str| match t {
        "append" => Some(Op::Append),
        "prepend" => Some(Op::Prepend),
        "replace" => Some(Op::Replace),
        "delete" => Some(Op::Delete),
        _ => None,
    };
    if let Some(list) = args
        .get("sections")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
    {
        if list.len() > 100 {
            return Err(err("invalid_arguments: too many operations (max 100)"));
        }
        for op in list {
            let kind = parse_op(s(op, "edit_type")).ok_or_else(|| {
                err("invalid_arguments: edit_type must be append, prepend, replace or delete")
            })?;
            ops.push((
                kind,
                arg_str(op, "section_id").map(str::to_owned),
                arg_str(op, "content").map(str::to_owned),
            ));
        }
    } else {
        let kind = parse_op(arg_str(args, "action").unwrap_or("append"))
            .ok_or_else(|| err("invalid_arguments: action must be append, prepend or replace"))?;
        ops.push((
            kind,
            arg_str(args, "section_id").map(str::to_owned),
            arg_str(args, "content").map(str::to_owned),
        ));
    }
    let mut sections: Vec<Value> = c["sections"].as_array().cloned().unwrap_or_default();
    let mut n = c
        .get("next_section")
        .and_then(Value::as_u64)
        .unwrap_or(sections.len() as u64);
    let new_sections = |content: &str, n: &mut u64| -> Vec<Value> {
        split_blocks(content)
            .into_iter()
            .map(|b| {
                let v = json!({"id": section_id(&id, *n), "markdown": b});
                *n += 1;
                v
            })
            .collect()
    };
    for (kind, section, content) in ops {
        let needs_content = !matches!(kind, Op::Delete);
        let content = content.unwrap_or_default();
        if needs_content && content.trim().is_empty() {
            return Err(err(
                "invalid_arguments: `content` must be a non-empty string",
            ));
        }
        let pos = match &section {
            Some(sid) => Some(
                sections
                    .iter()
                    .position(|x| s(x, "id") == sid)
                    .ok_or_else(|| err(&format!("invalid_section_id: {sid}")))?,
            ),
            None => None,
        };
        let is_title = |i: usize, sections: &[Value]| {
            sections[i]
                .get("title")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        };
        match (kind, pos) {
            (Op::Append, Some(i)) => {
                let new = new_sections(&content, &mut n);
                sections.splice(i + 1..i + 1, new);
            }
            (Op::Append, None) => {
                let new = new_sections(&content, &mut n);
                sections.extend(new);
            }
            (Op::Prepend, Some(i)) => {
                let at = if is_title(i, &sections) { i + 1 } else { i };
                let new = new_sections(&content, &mut n);
                sections.splice(at..at, new);
            }
            (Op::Prepend, None) => {
                let at = usize::from(sections.first().is_some_and(|x| x.get("title").is_some()));
                let new = new_sections(&content, &mut n);
                sections.splice(at..at, new);
            }
            (Op::Replace, Some(i)) => {
                if is_title(i, &sections) {
                    let line = content.trim().trim_start_matches('#').trim();
                    if line.contains('\n') || line.len() > 255 {
                        return Err(err(
                            "invalid_arguments: the title section takes a single line (max 255 bytes)",
                        ));
                    }
                    sections[i]["markdown"] = json!(line);
                    c["title"] = json!(line);
                } else {
                    let new = new_sections(&content, &mut n);
                    sections.splice(i..=i, new);
                }
            }
            (Op::Replace, None) => {
                let keep_title: Vec<Value> = sections
                    .iter()
                    .filter(|x| x.get("title").is_some())
                    .cloned()
                    .collect();
                sections = keep_title;
                let new = new_sections(&content, &mut n);
                sections.extend(new);
            }
            (Op::Delete, Some(i)) => {
                if is_title(i, &sections) {
                    return Err(err(
                        "invalid_arguments: the title section cannot be deleted",
                    ));
                }
                sections.remove(i);
            }
            (Op::Delete, None) => {
                return Err(err(
                    "invalid_arguments: `section_id` is required for delete",
                ));
            }
        }
    }
    c["sections"] = Value::Array(sections);
    c["next_section"] = json!(n);
    c["updated"] = json!(ctx.now);
    ctx.put("canvases", &id, c);
    Ok(json!({
        "canvas_id": id,
        "canvas_url": url(ctx, &id),
        "result": "Canvas updated successfully. Section IDs have changed; call slack_read_canvas for a fresh mapping before further edits.",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_markdown_into_sections() {
        let md = "# Goal\nShip it\n\n- a\n- b\n\n```\ncode\n\nmore\n```\n| a | b |\n|---|---|\n";
        assert_eq!(
            split_blocks(md),
            vec![
                "# Goal",
                "Ship it",
                "- a\n- b",
                "```\ncode\n\nmore\n```",
                "| a | b |\n|---|---|"
            ]
        );
    }
}
