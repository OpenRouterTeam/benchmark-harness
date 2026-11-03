//! Replica of `reminia/zendesk-mcp-server` @806d1ca: every tool builds the same dict the
//! Python server builds from zenpy objects and prints it with `json.dumps`. Failures are
//! returned as plain text `"Error: ..."` (not `isError`), because the server catches every
//! exception in `handle_call_tool`.

use mock_core::{mcp, world::WorldState};
use serde_json::{Map, Value, json};

use crate::{
    NAME,
    model::{self, ApiError, Page},
    pyjson::dumps,
    text::{b64_encode, markdown_to_html},
};

const ALLOWED_IMAGE_TYPES: [&str; 4] = ["image/gif", "image/jpeg", "image/png", "image/webp"];

pub fn call(world: &mut WorldState, name: &str, arguments: &Value) -> Value {
    let now = world.clock.now_rfc3339();
    let store = &mut world.service_mut(NAME).store;
    let args = arguments.as_object().filter(|a| !a.is_empty());
    let result = match name {
        "get_ticket" => get_ticket(store, args),
        "create_ticket" => create_ticket(store, &now, args),
        "get_tickets" => Ok(get_tickets(store, args)),
        "get_ticket_comments" => get_ticket_comments(store, args),
        "create_ticket_comment" => create_ticket_comment(store, &now, args),
        "get_ticket_attachment" => return get_ticket_attachment(store, args),
        "update_ticket" => update_ticket(store, &now, args),
        other => Err(format!("Unknown tool: {other}")),
    };
    match result {
        Ok(text) => mcp::text(text),
        Err(message) => mcp::text(format!("Error: {message}")),
    }
}

type Args<'a> = Option<&'a Map<String, Value>>;

fn required<'a>(args: Args<'a>, key: &str) -> Result<&'a Value, String> {
    let args = args.ok_or("Missing arguments")?;
    args.get(key).ok_or_else(|| format!("'{key}'"))
}

/// How Python's `str()` prints an argument inside an f-string.
fn py_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        other => other.to_string(),
    }
}

/// zenpy raises the Zendesk error body re-encoded with `json.dumps`.
fn api_message(error: &ApiError) -> String {
    dumps(&error.body, None)
}

fn get_ticket(store: &mut mock_core::store::Store, args: Args) -> Result<String, String> {
    let raw = required(args, "ticket_id")?;
    let fail = |e: &ApiError| format!("Failed to get ticket {}: {}", py_str(raw), api_message(e));
    let doc = model::parse_id(raw)
        .and_then(|id| model::get(store, model::TICKETS, id))
        .ok_or_else(|| fail(&ApiError::not_found()))?;
    let t = model::render_ticket(store, doc);
    Ok(dumps(
        &json!({
            "id": t["id"],
            "subject": t["subject"],
            "description": t["description"],
            "status": t["status"],
            "priority": t["priority"],
            "created_at": py_str(&t["created_at"]),
            "updated_at": py_str(&t["updated_at"]),
            "requester_id": t["requester_id"],
            "assignee_id": t["assignee_id"],
            "organization_id": t["organization_id"],
        }),
        None,
    ))
}

fn summary(t: &Value) -> Value {
    json!({
        "id": t["id"],
        "subject": t["subject"],
        "description": t["description"],
        "status": t["status"],
        "priority": t["priority"],
        "type": t["type"],
        "created_at": py_str(&t["created_at"]),
        "updated_at": py_str(&t["updated_at"]),
        "requester_id": t["requester_id"],
        "assignee_id": t["assignee_id"],
        "organization_id": t["organization_id"],
        "tags": t["tags"],
    })
}

fn create_ticket(
    store: &mut mock_core::store::Store,
    now: &str,
    args: Args,
) -> Result<String, String> {
    let args = args.ok_or("Missing arguments")?;
    // ZenpyTicket(...) only serializes attributes that are set.
    let mut input = Map::new();
    for key in [
        "subject",
        "description",
        "requester_id",
        "assignee_id",
        "priority",
        "type",
        "tags",
        "custom_fields",
    ] {
        if let Some(v) = args.get(key).filter(|v| !v.is_null()) {
            input.insert(key.to_owned(), v.clone());
        }
    }
    let (ticket, _) = model::create_ticket(store, now, &input)
        .map_err(|e| format!("Failed to create ticket: {}", api_message(&e)))?;
    Ok(dumps(
        &json!({ "message": "Ticket created successfully", "ticket": summary(&ticket) }),
        Some(2),
    ))
}

fn int_arg(args: Args, key: &str, default: i64) -> i64 {
    match args.and_then(|a| a.get(key)) {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(default),
        Some(Value::String(s)) => s.parse().unwrap_or(default),
        _ => default,
    }
}

fn str_arg(args: Args, key: &str, default: &str) -> String {
    args.and_then(|a| a.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_owned()
}

fn get_tickets(store: &mut mock_core::store::Store, args: Args) -> String {
    let page = int_arg(args, "page", 1);
    let per_page = int_arg(args, "per_page", 25).min(100);
    let sort_by = str_arg(args, "sort_by", "created_at");
    let sort_order = str_arg(args, "sort_order", "desc");

    let mut tickets = model::all_tickets(store);
    model::sort_tickets(&mut tickets, Some(&sort_by), Some(&sort_order));
    let listing = Page::of(tickets, page.max(1) as usize, per_page.max(1) as usize);
    let list: Vec<Value> = listing
        .items
        .iter()
        .map(|t| {
            json!({
                "id": t["id"],
                "subject": t["subject"],
                "status": t["status"],
                "priority": t["priority"],
                "description": t["description"],
                "created_at": t["created_at"],
                "updated_at": t["updated_at"],
                "requester_id": t["requester_id"],
                "assignee_id": t["assignee_id"],
            })
        })
        .collect();
    let count = list.len();
    dumps(
        &json!({
            "tickets": list,
            "page": page,
            "per_page": per_page,
            "count": count,
            "sort_by": sort_by,
            "sort_order": sort_order,
            "has_more": listing.has_next(),
            "next_page": if listing.has_next() { json!(page + 1) } else { Value::Null },
            "previous_page": if listing.has_previous() && page > 1 { json!(page - 1) } else { Value::Null },
        }),
        Some(2),
    )
}

fn get_ticket_comments(store: &mut mock_core::store::Store, args: Args) -> Result<String, String> {
    let raw = required(args, "ticket_id")?;
    let id = model::parse_id(raw)
        .filter(|id| model::get(store, model::TICKETS, *id).is_some())
        .ok_or_else(|| {
            format!(
                "Failed to get comments for ticket {}: {}",
                py_str(raw),
                api_message(&ApiError::not_found())
            )
        })?;
    let comments: Vec<Value> = model::ticket_comments(store, id)
        .into_iter()
        .map(|c| {
            let c = model::render_comment(c);
            let attachments: Vec<Value> = c["attachments"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|a| {
                    json!({
                        "id": a["id"],
                        "file_name": a["file_name"],
                        "content_url": a["content_url"],
                        "content_type": a["content_type"],
                        "size": a["size"],
                    })
                })
                .collect();
            json!({
                "id": c["id"],
                "author_id": c["author_id"],
                "body": c["body"],
                "html_body": c["html_body"],
                "public": c["public"],
                "created_at": py_str(&c["created_at"]),
                "attachments": attachments,
            })
        })
        .collect();
    Ok(dumps(&json!(comments), None))
}

fn create_ticket_comment(
    store: &mut mock_core::store::Store,
    now: &str,
    args: Args,
) -> Result<String, String> {
    let raw = required(args, "ticket_id")?;
    let comment = required(args, "comment")?;
    let public = args
        .and_then(|a| a.get("public"))
        .cloned()
        .unwrap_or(json!(true));
    let fail = |e: &ApiError| {
        format!(
            "Failed to post comment on ticket {}: {}",
            py_str(raw),
            api_message(e)
        )
    };
    let id = model::parse_id(raw).ok_or_else(|| fail(&ApiError::not_found()))?;
    let text = py_str(comment);
    let mut input = Map::new();
    input.insert(
        "comment".to_owned(),
        json!({ "html_body": markdown_to_html(&text), "public": public }),
    );
    model::update_ticket(store, now, id, &input).map_err(|e| fail(&e))?;
    Ok(format!("Comment created successfully: {text}"))
}

fn get_ticket_attachment(store: &mut mock_core::store::Store, args: Args) -> Value {
    let url = match required(args, "content_url") {
        Ok(v) => py_str(v),
        Err(message) => return mcp::text(format!("Error: {message}")),
    };
    let Some(attachment) = model::attachment_by_url(store, &url) else {
        return mcp::text(format!("Error: 404 Client Error: Not Found for url: {url}"));
    };
    let content_type = attachment["content_type"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase();
    if !ALLOWED_IMAGE_TYPES.contains(&content_type.as_str()) {
        let supported: Vec<String> = ALLOWED_IMAGE_TYPES
            .iter()
            .map(|t| format!("'{t}'"))
            .collect();
        return mcp::text(format!(
            "Error: Attachment type '{content_type}' is not allowed. Supported types: [{}]",
            supported.join(", ")
        ));
    }
    let bytes = model::attachment_bytes(store, &attachment);
    let magic: &[&[u8]] = match content_type.as_str() {
        "image/jpeg" => &[b"\xff\xd8\xff"],
        "image/png" => &[b"\x89PNG\r\n\x1a\n"],
        "image/gif" => &[b"GIF87a", b"GIF89a"],
        _ => &[b"RIFF"],
    };
    if !magic.iter().any(|m| bytes.starts_with(m)) {
        return mcp::text(format!(
            "Error: File header does not match declared content type '{content_type}'. The attachment may be spoofed."
        ));
    }
    if content_type == "image/webp" && bytes.get(8..12) != Some(b"WEBP") {
        return mcp::text("Error: File header does not match declared content type 'image/webp'.");
    }
    json!({ "content": [{ "type": "image", "data": b64_encode(&bytes), "mimeType": content_type }] })
}

fn update_ticket(
    store: &mut mock_core::store::Store,
    now: &str,
    args: Args,
) -> Result<String, String> {
    let args = args.ok_or("Missing arguments")?;
    let raw = args
        .get("ticket_id")
        .filter(|v| !v.is_null())
        .ok_or("ticket_id is required")?;
    let id = match raw {
        Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_f64().map(|f| f as u64))
            .ok_or_else(|| format!("invalid literal for int() with base 10: '{n}'"))?,
        Value::String(s) => s
            .trim()
            .parse()
            .map_err(|_| format!("invalid literal for int() with base 10: '{s}'"))?,
        other => {
            return Err(format!(
                "int() argument must be a string, a bytes-like object or a real number, not '{}'",
                match other {
                    Value::Array(_) => "list",
                    Value::Object(_) => "dict",
                    _ => "bool",
                }
            ));
        }
    };
    // update_fields: every argument but ticket_id; None values are skipped.
    let input: Map<String, Value> = args
        .iter()
        .filter(|(k, v)| k.as_str() != "ticket_id" && !v.is_null())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let (ticket, _) = model::update_ticket(store, now, id, &input)
        .map_err(|e| format!("Failed to update ticket {id}: {}", api_message(&e)))?;
    Ok(dumps(
        &json!({ "message": "Ticket updated successfully", "ticket": summary(&ticket) }),
        Some(2),
    ))
}
