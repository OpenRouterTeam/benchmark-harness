//! Store access, IDs, rich text and error helpers for the Notion mock.
//!
//! Collections hold API-shaped objects keyed by dashed UUID: `users`, `pages`, `blocks`,
//! `databases`, `data_sources`, `comments`, `templates`. Internal bookkeeping fields start
//! with `_` (e.g. `_order`) and are stripped from responses.

use axum::{http::StatusCode, response::Response};
use mock_core::{clock::format_unix, http::json, world::ServiceState};
use serde_json::{Map, Value, json};

pub struct Ctx<'a> {
    pub st: &'a mut ServiceState,
    pub now: i64,
}

pub type Res = Result<Value, ApiError>;

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::new(400, "validation_error", message)
    }

    pub fn not_found(kind: &str, id: &str) -> Self {
        Self::new(
            404,
            "object_not_found",
            format!(
                "Could not find {kind} with ID: {id}. Make sure the relevant pages and databases are shared with your integration."
            ),
        )
    }

    pub fn response(&self, request_id: &str) -> Response {
        json(
            StatusCode::from_u16(self.status).unwrap_or(StatusCode::BAD_REQUEST),
            json!({
                "object": "error",
                "status": self.status,
                "code": self.code,
                "message": self.message,
                "request_id": request_id,
            }),
        )
    }
}

pub fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// Normalizes a Notion ID (dashed or not) to dashed lowercase form.
pub fn norm_id(raw: &str) -> Option<String> {
    let hex: String = raw
        .trim()
        .chars()
        .filter(|c| *c != '-')
        .collect::<String>()
        .to_lowercase();
    // Notion URLs end with the ID; accept a trailing 32-hex segment.
    let hex = if hex.len() > 32 && hex[hex.len() - 32..].chars().all(|c| c.is_ascii_hexdigit()) {
        hex[hex.len() - 32..].to_owned()
    } else {
        hex
    };
    if hex.len() != 32 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

pub fn need_id(raw: &str, location: &str) -> Result<String, ApiError> {
    norm_id(raw).ok_or_else(|| {
        ApiError::validation(format!(
            "path failed validation: path.{location} should be a valid uuid, instead was `\"{raw}\"`."
        ))
    })
}

pub fn iso(secs: i64) -> String {
    let t = format_unix(secs);
    format!("{}.000Z", t.trim_end_matches('Z'))
}

/// Strips internal `_` fields recursively at the top level.
pub fn public(mut v: Value) -> Value {
    if let Some(m) = v.as_object_mut() {
        m.retain(|k, _| !k.starts_with('_'));
    }
    v
}

pub fn user_ref(id: &str) -> Value {
    json!({"object": "user", "id": id})
}

impl Ctx<'_> {
    pub fn all(&self, coll: &str) -> Vec<Value> {
        self.st
            .store
            .collection(coll)
            .map(|c| c.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn get(&self, coll: &str, id: &str) -> Option<Value> {
        self.st.store.collection(coll)?.get(id).cloned()
    }

    pub fn put(&mut self, coll: &str, doc: Value) {
        let id = s(&doc, "id").to_owned();
        self.st.store.collection_mut(coll).insert(id, doc);
    }

    pub fn iso_now(&self) -> String {
        iso(self.now)
    }

    pub fn new_id(&mut self, kind: &str) -> String {
        let u = self.st.ids.next_uuid(kind);
        let mut b = *u.as_bytes();
        b[6] = (b[6] & 0x0f) | 0x40;
        b[8] = (b[8] & 0x3f) | 0x80;
        let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
        norm_id(&hex).expect("32 hex chars")
    }

    pub fn request_id(&mut self) -> String {
        self.new_id("request")
    }

    /// The integration's bot user (`users/me`).
    pub fn bot(&self) -> Value {
        self.all("users")
            .into_iter()
            .find(|u| s(u, "type") == "bot")
            .unwrap_or_else(|| {
                json!({
                    "object": "user", "id": "9f1c3a52-7b2e-4d6a-8c1f-2e3d4b5a6c7d", "name": "Ops Assistant",
                    "avatar_url": null, "type": "bot",
                    "bot": {"owner": {"type": "workspace", "workspace": true}, "workspace_name": "Octo Corp", "workspace_limits": {"max_file_upload_size_in_bytes": 5_368_709_120_u64}},
                })
            })
    }

    pub fn bot_id(&self) -> String {
        s(&self.bot(), "id").to_owned()
    }

    pub fn user_name(&self, id: &str) -> String {
        self.get("users", id)
            .map(|u| s(&u, "name").to_owned())
            .unwrap_or_else(|| "Unknown user".to_owned())
    }
}

// ───────────────────────────── rich text ─────────────────────────────

fn default_annotations() -> Value {
    json!({"bold": false, "italic": false, "strikethrough": false, "underline": false, "code": false, "color": "default"})
}

/// Normalizes request rich text (or a plain string) into response rich text objects.
pub fn rich_text(ctx: &Ctx, input: &Value) -> Result<Vec<Value>, ApiError> {
    let items: Vec<Value> = match input {
        Value::Null => return Ok(Vec::new()),
        Value::String(t) => vec![json!({"type": "text", "text": {"content": t}})],
        Value::Array(a) => a.clone(),
        Value::Object(_) => vec![input.clone()],
        _ => return Err(ApiError::validation("rich_text should be an array")),
    };
    let mut out = Vec::new();
    for item in items {
        let mut annotations = default_annotations();
        if let Some(a) = item.get("annotations").and_then(Value::as_object) {
            for (k, v) in a {
                annotations[k] = v.clone();
            }
        }
        let ty = match s(&item, "type") {
            "" if item.get("mention").is_some() => "mention",
            "" if item.get("equation").is_some() => "equation",
            "" => "text",
            t => t,
        };
        match ty {
            "text" => {
                let content = item["text"]["content"].as_str().ok_or_else(|| {
                    ApiError::validation("body failed validation: rich_text[].text.content should be defined, instead was `undefined`.")
                })?;
                if content.chars().count() > 2000 {
                    return Err(ApiError::validation(format!(
                        "body failed validation: rich_text[].text.content.length should be ≤ `2000`, instead was `{}`.",
                        content.chars().count()
                    )));
                }
                let link = item["text"]
                    .get("link")
                    .filter(|l| l.is_object())
                    .cloned()
                    .unwrap_or(Value::Null);
                let href = link.get("url").cloned().unwrap_or(Value::Null);
                out.push(json!({
                    "type": "text",
                    "text": {"content": content, "link": link},
                    "annotations": annotations,
                    "plain_text": content,
                    "href": href,
                }));
            }
            "mention" => {
                let m = item["mention"].clone();
                let plain = if let Some(u) = m
                    .get("user")
                    .and_then(|u| u.get("id"))
                    .and_then(Value::as_str)
                {
                    format!("@{}", ctx.user_name(u))
                } else if let Some(p) = m
                    .get("page")
                    .and_then(|p| p.get("id"))
                    .and_then(Value::as_str)
                {
                    ctx.get("pages", &norm_id(p).unwrap_or_default())
                        .map(|pg| crate::pages::title_of(&pg))
                        .unwrap_or_else(|| "Untitled".into())
                } else if let Some(d) = m
                    .get("date")
                    .and_then(|d| d.get("start"))
                    .and_then(Value::as_str)
                {
                    d.to_owned()
                } else {
                    String::new()
                };
                let mut mention = m.clone();
                if let Some(u) = m
                    .get("user")
                    .and_then(|u| u.get("id"))
                    .and_then(Value::as_str)
                {
                    mention = json!({"type": "user", "user": {"object": "user", "id": u}});
                } else if mention.get("type").is_none() {
                    let key = mention
                        .as_object()
                        .and_then(|o| o.keys().next().cloned())
                        .unwrap_or_default();
                    mention["type"] = json!(key);
                }
                out.push(json!({"type": "mention", "mention": mention, "annotations": annotations, "plain_text": plain, "href": null}));
            }
            "equation" => {
                let expr = item["equation"]["expression"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned();
                out.push(json!({"type": "equation", "equation": {"expression": expr}, "annotations": annotations, "plain_text": expr, "href": null}));
            }
            other => {
                return Err(ApiError::validation(format!(
                    "body failed validation: rich_text[].type should be `\"text\"`, `\"mention\"`, or `\"equation\"`, instead was `\"{other}\"`."
                )));
            }
        }
    }
    Ok(out)
}

pub fn plain(rich: &Value) -> String {
    rich.as_array()
        .map(|a| {
            a.iter()
                .map(|r| s(r, "plain_text"))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

/// Paginates `items` by an opaque cursor (the ID of the next item).
pub fn paginate(
    items: Vec<Value>,
    cursor: Option<&str>,
    page_size: Option<i64>,
) -> Result<(Vec<Value>, Option<String>), ApiError> {
    let size = page_size.unwrap_or(100);
    if !(1..=100).contains(&size) {
        return Err(ApiError::validation(format!(
            "body failed validation: body.page_size should be ≤ `100`, instead was `{size}`."
        )));
    }
    let start = match cursor.filter(|c| !c.is_empty()) {
        Some(c) => items
            .iter()
            .position(|i| s(i, "id") == c)
            .ok_or_else(|| ApiError::validation("start_cursor provided is invalid."))?,
        None => 0,
    };
    let page: Vec<Value> = items
        .iter()
        .skip(start)
        .take(size as usize)
        .cloned()
        .collect();
    let next = items
        .get(start + size as usize)
        .map(|i| s(i, "id").to_owned());
    Ok((page, next))
}

pub fn list(results: Vec<Value>, next: Option<String>, ty: &str, request_id: &str) -> Value {
    let mut m = Map::new();
    m.insert("object".into(), json!("list"));
    m.insert("results".into(), Value::Array(results));
    m.insert("next_cursor".into(), json!(next));
    m.insert("has_more".into(), json!(next.is_some()));
    m.insert("type".into(), json!(ty));
    m.insert(ty.into(), json!({}));
    m.insert("request_id".into(), json!(request_id));
    Value::Object(m)
}

pub fn slug_title(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_owned()
}

pub fn notion_url(title: &str, id: &str) -> String {
    let compact = id.replace('-', "");
    let slug = slug_title(title);
    if slug.is_empty() {
        format!("https://www.notion.so/{compact}")
    } else {
        format!("https://www.notion.so/{slug}-{compact}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_normalize() {
        assert_eq!(
            norm_id("1A2B3C4D5E6F47A8B9C0D1E2F3A4B5C6").unwrap(),
            "1a2b3c4d-5e6f-47a8-b9c0-d1e2f3a4b5c6"
        );
        assert_eq!(
            norm_id("https://www.notion.so/Launch-plan-1a2b3c4d5e6f47a8b9c0d1e2f3a4b5c6").unwrap(),
            "1a2b3c4d-5e6f-47a8-b9c0-d1e2f3a4b5c6"
        );
        assert!(norm_id("not-an-id").is_none());
        assert_eq!(
            notion_url("Launch plan (Q4)", "1a2b3c4d-5e6f-47a8-b9c0-d1e2f3a4b5c6"),
            "https://www.notion.so/Launch-plan-Q4-1a2b3c4d5e6f47a8b9c0d1e2f3a4b5c6"
        );
    }
}
