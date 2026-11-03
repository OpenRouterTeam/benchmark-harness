//! Zendesk Support data model over the scenario store, shared by the REST API and the
//! replica tools.
//!
//! Collections (the grader addresses them as `/services/zendesk/<collection>/<key>`):
//!
//! | Collection | Key | Notes |
//! |---|---|---|
//! | `tickets` | ticket id | real ticket fields; missing ones are filled when rendered |
//! | `comments` | `"<ticket_id>/<n>"` | `n` is the comment's 1-based position on its ticket |
//! | `audits` | audit id | written for every create/update |
//! | `users`, `organizations` | id | the first `admin` (else `agent`) user is the API token owner |
//! | `attachment_data` | attachment id | optional `{"id", "data_base64"}` file contents |
//! | `articles`, `sections`, `categories` | id | Help Center |
//!
//! Attachments live inside their comment's `attachments` array, as in the real API.

use axum::http::StatusCode;
use mock_core::store::Store;
use serde_json::{Map, Value, json};

pub const BASE: &str = "https://octo-corp.zendesk.com";

pub const TICKETS: &str = "tickets";
pub const COMMENTS: &str = "comments";
pub const AUDITS: &str = "audits";
pub const USERS: &str = "users";
pub const ORGANIZATIONS: &str = "organizations";
pub const ATTACHMENT_DATA: &str = "attachment_data";
pub const ARTICLES: &str = "articles";
pub const SECTIONS: &str = "sections";
pub const CATEGORIES: &str = "categories";

pub const STATUSES: [&str; 6] = ["new", "open", "pending", "hold", "solved", "closed"];
pub const PRIORITIES: [&str; 4] = ["low", "normal", "high", "urgent"];
pub const TYPES: [&str; 4] = ["problem", "incident", "question", "task"];

/// A 1x1 transparent PNG, served for image attachments without stored contents.
const DEFAULT_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

/// An API error: status plus the exact Zendesk JSON body.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiError {
    pub status: StatusCode,
    pub body: Value,
}

impl ApiError {
    pub fn not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            body: json!({ "error": "RecordNotFound", "description": "Not found" }),
        }
    }

    pub fn invalid_endpoint() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            body: json!({ "error": "InvalidEndpoint", "description": "Not found" }),
        }
    }

    pub fn invalid(field: &str, description: impl Into<String>, error: &str) -> Self {
        let mut details = Map::new();
        details.insert(
            field.to_owned(),
            json!([{ "description": description.into(), "error": error }]),
        );
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            body: json!({
                "error": "RecordInvalid",
                "description": "Record validation errors",
                "details": details,
            }),
        }
    }

    pub fn bad_request(error: &str, description: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            body: json!({ "error": error, "description": description.into() }),
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

// ───────────────────────────── lookups ─────────────────────────────

/// An id given as a JSON number or numeric string (a trailing `.json` is allowed).
pub fn parse_id(value: &Value) -> Option<u64> {
    match value {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => parse_id_str(s),
        _ => None,
    }
}

pub fn parse_id_str(s: &str) -> Option<u64> {
    s.strip_suffix(".json").unwrap_or(s).parse().ok()
}

fn docs<'a>(store: &'a Store, name: &str) -> Vec<(&'a str, &'a Value)> {
    store
        .collection(name)
        .map(|c| c.iter().collect())
        .unwrap_or_default()
}

pub fn get<'a>(store: &'a Store, collection: &str, id: u64) -> Option<&'a Value> {
    store.collection(collection)?.get(&id.to_string())
}

fn id_of(doc: &Value) -> u64 {
    doc["id"].as_u64().unwrap_or(0)
}

/// The user the API token belongs to: the first admin, else the first agent.
pub fn current_user(store: &Store) -> Option<u64> {
    let users = docs(store, USERS);
    ["admin", "agent"].iter().find_map(|role| {
        users
            .iter()
            .find(|(_, u)| u["role"] == *role)
            .map(|(_, u)| id_of(u))
    })
}

fn is_agent(store: &Store, id: u64) -> bool {
    get(store, USERS, id).is_some_and(|u| u["role"] == "agent" || u["role"] == "admin")
}

/// `(key, doc)` of a ticket's comments in order.
pub fn ticket_comments(store: &Store, ticket_id: u64) -> Vec<&Value> {
    let prefix = format!("{ticket_id}/");
    docs(store, COMMENTS)
        .into_iter()
        .filter(|(k, _)| k.starts_with(&prefix))
        .map(|(_, v)| v)
        .collect()
}

// ───────────────────────────── rendering ─────────────────────────────

fn or(doc: &Value, key: &str, default: Value) -> Value {
    match doc.get(key) {
        Some(v) => v.clone(),
        None => default,
    }
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

fn default_via(channel: &str) -> Value {
    json!({ "channel": channel, "source": { "from": {}, "to": {}, "rel": null } })
}

/// The full ticket JSON as `GET /api/v2/tickets/{id}` returns it.
pub fn render_ticket(store: &Store, doc: &Value) -> Value {
    let id = id_of(doc);
    let comments = ticket_comments(store, id);
    let description = doc
        .get("description")
        .cloned()
        .or_else(|| comments.first().map(|c| c["body"].clone()))
        .unwrap_or(json!(""));
    let requester_id = or(doc, "requester_id", Value::Null);
    let organization_id = doc.get("organization_id").cloned().unwrap_or_else(|| {
        requester_id
            .as_u64()
            .and_then(|r| get(store, USERS, r))
            .map(|u| u["organization_id"].clone())
            .unwrap_or(Value::Null)
    });
    let is_public = doc.get("is_public").cloned().unwrap_or_else(|| {
        json!(comments.is_empty() || comments.iter().any(|c| c["public"] != false))
    });
    let created_at = or(doc, "created_at", Value::Null);
    let subject = or(doc, "subject", Value::Null);
    let custom_fields = or(doc, "custom_fields", json!([]));
    obj(vec![
        ("url", json!(format!("{BASE}/api/v2/tickets/{id}.json"))),
        ("id", json!(id)),
        ("external_id", or(doc, "external_id", Value::Null)),
        ("via", or(doc, "via", default_via("web"))),
        ("created_at", created_at.clone()),
        ("updated_at", or(doc, "updated_at", created_at)),
        ("type", or(doc, "type", Value::Null)),
        ("subject", subject.clone()),
        ("raw_subject", or(doc, "raw_subject", subject)),
        ("description", description),
        ("priority", or(doc, "priority", Value::Null)),
        ("status", or(doc, "status", json!("open"))),
        ("recipient", or(doc, "recipient", Value::Null)),
        ("requester_id", requester_id.clone()),
        ("submitter_id", or(doc, "submitter_id", requester_id)),
        ("assignee_id", or(doc, "assignee_id", Value::Null)),
        ("organization_id", organization_id),
        ("group_id", or(doc, "group_id", Value::Null)),
        ("collaborator_ids", or(doc, "collaborator_ids", json!([]))),
        ("follower_ids", or(doc, "follower_ids", json!([]))),
        ("email_cc_ids", or(doc, "email_cc_ids", json!([]))),
        ("forum_topic_id", Value::Null),
        ("problem_id", or(doc, "problem_id", Value::Null)),
        ("has_incidents", or(doc, "has_incidents", json!(false))),
        ("is_public", is_public),
        ("due_at", or(doc, "due_at", Value::Null)),
        ("tags", or(doc, "tags", json!([]))),
        ("custom_fields", custom_fields.clone()),
        (
            "satisfaction_rating",
            or(doc, "satisfaction_rating", Value::Null),
        ),
        ("sharing_agreement_ids", json!([])),
        ("fields", custom_fields),
        ("followup_ids", or(doc, "followup_ids", json!([]))),
        (
            "ticket_form_id",
            or(doc, "ticket_form_id", json!(360000000001_u64)),
        ),
        ("brand_id", or(doc, "brand_id", json!(360000000002_u64))),
        ("allow_channelback", json!(false)),
        ("allow_attachments", json!(true)),
        ("from_messaging_channel", json!(false)),
    ])
}

pub fn render_comment(doc: &Value) -> Value {
    let body = or(doc, "body", json!(""));
    let html = doc.get("html_body").cloned().unwrap_or_else(|| {
        json!(crate::text::plain_to_comment_html(
            body.as_str().unwrap_or_default()
        ))
    });
    let attachments: Vec<Value> = doc["attachments"]
        .as_array()
        .map(|a| a.iter().map(render_attachment).collect())
        .unwrap_or_default();
    obj(vec![
        ("id", or(doc, "id", Value::Null)),
        ("type", json!("Comment")),
        ("author_id", or(doc, "author_id", Value::Null)),
        ("body", body.clone()),
        ("html_body", html),
        ("plain_body", or(doc, "plain_body", body)),
        ("public", or(doc, "public", json!(true))),
        ("attachments", json!(attachments)),
        ("audit_id", or(doc, "audit_id", Value::Null)),
        ("via", or(doc, "via", default_via("web"))),
        ("created_at", or(doc, "created_at", Value::Null)),
        (
            "metadata",
            or(doc, "metadata", json!({ "system": {}, "custom": {} })),
        ),
    ])
}

/// `content_url` of an attachment (stored, or derived from its id and file name).
pub fn content_url(doc: &Value) -> String {
    if let Some(url) = doc["content_url"].as_str() {
        return url.to_owned();
    }
    let name = doc["file_name"].as_str().unwrap_or("file");
    format!(
        "{BASE}/attachments/token/{}/?name={}",
        attachment_token(doc),
        url_encode(name)
    )
}

fn attachment_token(doc: &Value) -> String {
    doc["token"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("zdAt{}", id_of(doc)))
}

pub fn render_attachment(doc: &Value) -> Value {
    let id = id_of(doc);
    let url = content_url(doc);
    obj(vec![
        ("url", json!(format!("{BASE}/api/v2/attachments/{id}.json"))),
        ("id", json!(id)),
        ("file_name", or(doc, "file_name", Value::Null)),
        ("content_url", json!(url.clone())),
        ("mapped_content_url", json!(url)),
        (
            "content_type",
            or(doc, "content_type", json!("application/octet-stream")),
        ),
        ("size", or(doc, "size", json!(0))),
        ("width", or(doc, "width", Value::Null)),
        ("height", or(doc, "height", Value::Null)),
        ("inline", or(doc, "inline", json!(false))),
        ("deleted", json!(false)),
        ("malware_access_override", json!(false)),
        ("malware_scan_result", json!("malware_not_found")),
        ("thumbnails", json!([])),
    ])
}

pub fn render_user(doc: &Value) -> Value {
    let id = id_of(doc);
    let created_at = or(doc, "created_at", Value::Null);
    let role = or(doc, "role", json!("end-user"));
    obj(vec![
        ("id", json!(id)),
        ("url", json!(format!("{BASE}/api/v2/users/{id}.json"))),
        ("name", or(doc, "name", Value::Null)),
        ("email", or(doc, "email", Value::Null)),
        ("created_at", created_at.clone()),
        ("updated_at", or(doc, "updated_at", created_at)),
        (
            "time_zone",
            or(doc, "time_zone", json!("Eastern Time (US & Canada)")),
        ),
        (
            "iana_time_zone",
            or(doc, "iana_time_zone", json!("America/New_York")),
        ),
        ("phone", or(doc, "phone", Value::Null)),
        ("shared_phone_number", Value::Null),
        ("photo", Value::Null),
        ("locale_id", json!(1)),
        ("locale", or(doc, "locale", json!("en-US"))),
        ("organization_id", or(doc, "organization_id", Value::Null)),
        ("role", role.clone()),
        ("verified", or(doc, "verified", json!(true))),
        ("external_id", or(doc, "external_id", Value::Null)),
        ("tags", or(doc, "tags", json!([]))),
        ("alias", json!("")),
        ("active", or(doc, "active", json!(true))),
        ("shared", json!(false)),
        ("shared_agent", json!(false)),
        ("last_login_at", or(doc, "last_login_at", Value::Null)),
        ("two_factor_auth_enabled", Value::Null),
        ("signature", or(doc, "signature", Value::Null)),
        ("details", or(doc, "details", json!(""))),
        ("notes", or(doc, "notes", json!(""))),
        ("role_type", Value::Null),
        ("custom_role_id", Value::Null),
        ("moderator", json!(role != "end-user")),
        (
            "ticket_restriction",
            json!(if role == "end-user" {
                "requested"
            } else {
                "assigned"
            }),
        ),
        ("only_private_comments", json!(false)),
        ("restricted_agent", json!(role == "end-user")),
        ("suspended", or(doc, "suspended", json!(false))),
        ("default_group_id", or(doc, "default_group_id", Value::Null)),
        ("report_csv", json!(false)),
        ("user_fields", or(doc, "user_fields", json!({}))),
    ])
}

pub fn render_organization(doc: &Value) -> Value {
    let id = id_of(doc);
    let created_at = or(doc, "created_at", Value::Null);
    obj(vec![
        (
            "url",
            json!(format!("{BASE}/api/v2/organizations/{id}.json")),
        ),
        ("id", json!(id)),
        ("name", or(doc, "name", Value::Null)),
        ("shared_tickets", json!(false)),
        ("shared_comments", json!(false)),
        ("external_id", or(doc, "external_id", Value::Null)),
        ("created_at", created_at.clone()),
        ("updated_at", or(doc, "updated_at", created_at)),
        ("domain_names", or(doc, "domain_names", json!([]))),
        ("details", or(doc, "details", json!(""))),
        ("notes", or(doc, "notes", json!(""))),
        ("group_id", Value::Null),
        ("tags", or(doc, "tags", json!([]))),
        (
            "organization_fields",
            or(doc, "organization_fields", json!({})),
        ),
    ])
}

fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_owned()
}

pub fn render_article(doc: &Value) -> Value {
    let id = id_of(doc);
    let title = or(doc, "title", json!(""));
    let created_at = or(doc, "created_at", Value::Null);
    let updated_at = or(doc, "updated_at", created_at.clone());
    obj(vec![
        ("id", json!(id)),
        (
            "url",
            json!(format!(
                "{BASE}/api/v2/help_center/en-us/articles/{id}.json"
            )),
        ),
        (
            "html_url",
            json!(format!(
                "{BASE}/hc/en-us/articles/{id}-{}",
                slug(title.as_str().unwrap_or_default())
            )),
        ),
        ("author_id", or(doc, "author_id", Value::Null)),
        ("comments_disabled", json!(false)),
        ("draft", or(doc, "draft", json!(false))),
        ("promoted", or(doc, "promoted", json!(false))),
        ("position", or(doc, "position", json!(0))),
        ("vote_sum", or(doc, "vote_sum", json!(0))),
        ("vote_count", or(doc, "vote_count", json!(0))),
        ("section_id", or(doc, "section_id", Value::Null)),
        ("created_at", created_at),
        ("updated_at", updated_at.clone()),
        ("name", title.clone()),
        ("title", title),
        ("source_locale", json!("en-us")),
        ("locale", json!("en-us")),
        ("outdated", json!(false)),
        ("outdated_locales", json!([])),
        ("edited_at", updated_at),
        ("user_segment_id", Value::Null),
        ("permission_group_id", json!(1_u64)),
        ("content_tag_ids", json!([])),
        ("label_names", or(doc, "label_names", json!([]))),
        ("body", or(doc, "body", json!(""))),
    ])
}

pub fn render_section(doc: &Value) -> Value {
    let id = id_of(doc);
    let name = or(doc, "name", json!(""));
    let created_at = or(doc, "created_at", Value::Null);
    obj(vec![
        ("id", json!(id)),
        (
            "url",
            json!(format!(
                "{BASE}/api/v2/help_center/en-us/sections/{id}.json"
            )),
        ),
        (
            "html_url",
            json!(format!(
                "{BASE}/hc/en-us/sections/{id}-{}",
                slug(name.as_str().unwrap_or_default())
            )),
        ),
        ("category_id", or(doc, "category_id", Value::Null)),
        ("position", or(doc, "position", json!(0))),
        ("sorting", json!("manual")),
        ("created_at", created_at.clone()),
        ("updated_at", or(doc, "updated_at", created_at)),
        ("name", name),
        ("description", or(doc, "description", json!(""))),
        ("locale", json!("en-us")),
        ("source_locale", json!("en-us")),
        ("outdated", json!(false)),
        (
            "parent_section_id",
            or(doc, "parent_section_id", Value::Null),
        ),
        ("theme_template", json!("section_page")),
    ])
}

pub fn render_category(doc: &Value) -> Value {
    let id = id_of(doc);
    let name = or(doc, "name", json!(""));
    let created_at = or(doc, "created_at", Value::Null);
    obj(vec![
        ("id", json!(id)),
        (
            "url",
            json!(format!(
                "{BASE}/api/v2/help_center/en-us/categories/{id}.json"
            )),
        ),
        (
            "html_url",
            json!(format!(
                "{BASE}/hc/en-us/categories/{id}-{}",
                slug(name.as_str().unwrap_or_default())
            )),
        ),
        ("position", or(doc, "position", json!(0))),
        ("created_at", created_at.clone()),
        ("updated_at", or(doc, "updated_at", created_at)),
        ("name", name),
        ("description", or(doc, "description", json!(""))),
        ("locale", json!("en-us")),
        ("source_locale", json!("en-us")),
        ("outdated", json!(false)),
    ])
}

pub fn all(store: &Store, collection: &str, render: fn(&Value) -> Value) -> Vec<Value> {
    docs(store, collection)
        .into_iter()
        .map(|(_, d)| render(d))
        .collect()
}

pub fn all_tickets(store: &Store) -> Vec<Value> {
    docs(store, TICKETS)
        .into_iter()
        .map(|(_, d)| render_ticket(store, d))
        .collect()
}

// ───────────────────────────── listing ─────────────────────────────

fn rank(list: &[&str], value: &Value) -> usize {
    value
        .as_str()
        .and_then(|v| list.iter().position(|x| *x == v))
        .map_or(0, |p| p + 1)
}

/// Sorts rendered tickets like `GET /api/v2/tickets?sort_by=&sort_order=` (default: id asc).
pub fn sort_tickets(tickets: &mut [Value], sort_by: Option<&str>, sort_order: Option<&str>) {
    let key = sort_by.unwrap_or("id");
    tickets.sort_by(|a, b| {
        let ord = match key {
            "created_at" | "updated_at" | "subject" | "due_at" => a[key]
                .as_str()
                .unwrap_or_default()
                .cmp(b[key].as_str().unwrap_or_default()),
            "priority" => rank(&PRIORITIES, &a[key]).cmp(&rank(&PRIORITIES, &b[key])),
            "status" => rank(&STATUSES, &a[key]).cmp(&rank(&STATUSES, &b[key])),
            "assignee" | "requester" | "group" => {
                let k = format!("{key}_id");
                a[&k].as_u64().cmp(&b[&k].as_u64())
            }
            _ => std::cmp::Ordering::Equal,
        };
        ord.then(a["id"].as_u64().cmp(&b["id"].as_u64()))
    });
    if sort_order == Some("desc") {
        tickets.reverse();
    }
}

/// One page of an offset-paginated list.
pub struct Page {
    pub items: Vec<Value>,
    pub count: usize,
    pub page: usize,
    pub per_page: usize,
}

impl Page {
    pub fn of(items: Vec<Value>, page: usize, per_page: usize) -> Self {
        let page = page.max(1);
        let per_page = per_page.clamp(1, 100);
        let count = items.len();
        let items = items
            .into_iter()
            .skip((page - 1) * per_page)
            .take(per_page)
            .collect();
        Self {
            items,
            count,
            page,
            per_page,
        }
    }

    pub fn has_next(&self) -> bool {
        self.page * self.per_page < self.count
    }

    pub fn has_previous(&self) -> bool {
        self.page > 1
    }

    pub fn page_count(&self) -> usize {
        self.count.div_ceil(self.per_page).max(1)
    }
}

// ───────────────────────────── mutations ─────────────────────────────

fn max_key_id(store: &Store, collection: &str) -> u64 {
    docs(store, collection)
        .iter()
        .filter_map(|(k, d)| d["id"].as_u64().or_else(|| k.parse().ok()))
        .max()
        .unwrap_or(0)
}

/// Next audit-event id: comment ids and audit event ids share one sequence.
fn next_event_id(store: &Store) -> u64 {
    let comments = max_key_id(store, COMMENTS);
    let events = docs(store, AUDITS)
        .iter()
        .flat_map(|(_, a)| a["events"].as_array().cloned().unwrap_or_default())
        .filter_map(|e| e["id"].as_u64())
        .max()
        .unwrap_or(0);
    comments.max(events) + 1
}

fn next_audit_id(store: &Store) -> u64 {
    let audits = max_key_id(store, AUDITS);
    let from_comments = docs(store, COMMENTS)
        .iter()
        .filter_map(|(_, c)| c["audit_id"].as_u64())
        .max()
        .unwrap_or(0);
    audits.max(from_comments) + 1
}

fn normalize_tags(value: &Value) -> Value {
    let mut out: Vec<String> = Vec::new();
    for tag in value.as_array().into_iter().flatten() {
        let Some(t) = tag.as_str() else { continue };
        for part in t.split_whitespace() {
            let part = part.to_lowercase();
            if !out.contains(&part) {
                out.push(part);
            }
        }
    }
    json!(out)
}

/// The comment part of a ticket create/update (`{"body"|"html_body", "public", "author_id"}`).
struct NewComment {
    body: String,
    html_body: String,
    public: bool,
    author_id: Option<u64>,
}

fn parse_comment(value: &Value) -> Option<NewComment> {
    let (body, html) = match value {
        Value::String(s) => (Some(s.clone()), None),
        Value::Object(o) => (
            o.get("body")
                .or_else(|| o.get("value"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            o.get("html_body")
                .or_else(|| o.get("html_value"))
                .and_then(Value::as_str)
                .map(str::to_owned),
        ),
        _ => (None, None),
    };
    let (body, html_body) = match (body, html) {
        (_, Some(html)) => (crate::text::html_to_text(&html), html),
        (Some(body), None) => {
            let html = crate::text::plain_to_comment_html(&body);
            (body, html)
        }
        (None, None) => return None,
    };
    if body.trim().is_empty() {
        return None;
    }
    let public = match &value["public"] {
        Value::Bool(b) => *b,
        Value::String(s) => !matches!(s.to_lowercase().as_str(), "false" | "0" | "no"),
        Value::Number(n) => n.as_i64() != Some(0),
        _ => true,
    };
    Some(NewComment {
        body,
        html_body,
        public,
        author_id: value["author_id"].as_u64(),
    })
}

fn insert_comment(
    store: &mut Store,
    now: &str,
    ticket_id: u64,
    comment: NewComment,
    author_id: Option<u64>,
    audit_id: u64,
    event_id: u64,
) -> Value {
    let position = ticket_comments(store, ticket_id).len() + 1;
    let doc = json!({
        "id": event_id,
        "type": "Comment",
        "author_id": comment.author_id.or(author_id),
        "body": comment.body,
        "html_body": comment.html_body,
        "plain_body": comment.body,
        "public": comment.public,
        "attachments": [],
        "audit_id": audit_id,
        "via": default_via("api"),
        "created_at": now,
        "metadata": { "system": { "client": "zenpy/2.0.56" }, "custom": {} },
    });
    store
        .collection_mut(COMMENTS)
        .insert(format!("{ticket_id}/{position}"), doc.clone());
    doc
}

fn check_enum(field: &str, label: &str, value: &Value, allowed: &[&str]) -> ApiResult<()> {
    match value {
        Value::Null => Ok(()),
        Value::String(s) if allowed.contains(&s.as_str()) => Ok(()),
        _ => Err(ApiError::invalid(
            field,
            format!("{label}: is invalid"),
            "InvalidValue",
        )),
    }
}

fn check_user(
    store: &Store,
    field: &str,
    label: &str,
    value: &Value,
    agent: bool,
) -> ApiResult<()> {
    if value.is_null() {
        return Ok(());
    }
    let ok = value.as_u64().is_some_and(|id| {
        if agent {
            is_agent(store, id)
        } else {
            get(store, USERS, id).is_some()
        }
    });
    if ok {
        Ok(())
    } else {
        Err(ApiError::invalid(
            field,
            format!("{label}: is invalid"),
            "InvalidValue",
        ))
    }
}

/// Field-level validation shared by create and update.
fn validate(store: &Store, fields: &Map<String, Value>) -> ApiResult<()> {
    if let Some(v) = fields.get("status") {
        check_enum("status", "Status", v, &STATUSES)?;
    }
    if let Some(v) = fields.get("priority") {
        check_enum("priority", "Priority", v, &PRIORITIES)?;
    }
    if let Some(v) = fields.get("type") {
        check_enum("type", "Type", v, &TYPES)?;
    }
    if let Some(v) = fields.get("assignee_id") {
        check_user(store, "assignee", "Assignee", v, true)?;
    }
    if let Some(v) = fields.get("requester_id") {
        check_user(store, "requester", "Requester", v, false)?;
    }
    if let Some(v) = fields.get("due_at")
        && !v.is_null()
        && v.as_str().is_none_or(|s| {
            time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).is_err()
        })
    {
        return Err(ApiError::invalid(
            "due_at",
            "Due at: is invalid",
            "InvalidValue",
        ));
    }
    Ok(())
}

const SETTABLE: [&str; 13] = [
    "subject",
    "status",
    "priority",
    "type",
    "assignee_id",
    "requester_id",
    "group_id",
    "organization_id",
    "tags",
    "custom_fields",
    "due_at",
    "external_id",
    "collaborator_ids",
];

fn audit(
    store: &mut Store,
    now: &str,
    audit_id: u64,
    ticket_id: u64,
    author_id: Option<u64>,
    events: Vec<Value>,
) -> Value {
    let doc = json!({
        "id": audit_id,
        "ticket_id": ticket_id,
        "created_at": now,
        "author_id": author_id,
        "metadata": { "system": { "client": "zenpy/2.0.56" }, "custom": {} },
        "events": events,
        "via": default_via("api"),
    });
    store
        .collection_mut(AUDITS)
        .insert(audit_id.to_string(), doc.clone());
    doc
}

/// `POST /api/v2/tickets` with the `ticket` object. Returns `(ticket, audit)`.
pub fn create_ticket(
    store: &mut Store,
    now: &str,
    input: &Map<String, Value>,
) -> ApiResult<(Value, Value)> {
    let me = current_user(store);
    let comment = input
        .get("comment")
        .and_then(parse_comment)
        .or_else(|| input.get("description").and_then(parse_comment));
    let Some(comment) = comment else {
        return Err(ApiError::invalid(
            "base",
            "Description: cannot be blank",
            "BlankValue",
        ));
    };
    validate(store, input)?;

    let mut requester_id = input.get("requester_id").and_then(Value::as_u64);
    if requester_id.is_none()
        && let Some(requester) = input.get("requester").and_then(Value::as_object)
    {
        requester_id = Some(find_or_create_user(store, now, requester)?);
    }
    let requester_id = requester_id.or(me);
    let assignee_id = input.get("assignee_id").and_then(Value::as_u64);
    let status = input
        .get("status")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| if assignee_id.is_some() { "open" } else { "new" }.to_owned());

    let id = max_key_id(store, TICKETS) + 1;
    let audit_id = next_audit_id(store);
    let event_id = next_event_id(store);
    let mut doc = json!({
        "id": id,
        "via": default_via("api"),
        "created_at": now,
        "updated_at": now,
        "type": input.get("type").cloned().unwrap_or(Value::Null),
        "subject": input.get("subject").cloned().unwrap_or(Value::Null),
        "description": comment.body,
        "priority": input.get("priority").cloned().unwrap_or(Value::Null),
        "status": status,
        "requester_id": requester_id,
        "submitter_id": me.or(requester_id),
        "assignee_id": assignee_id,
        "tags": input.get("tags").map(normalize_tags).unwrap_or(json!([])),
        "custom_fields": input.get("custom_fields").cloned().unwrap_or(json!([])),
        "due_at": input.get("due_at").cloned().unwrap_or(Value::Null),
        "external_id": input.get("external_id").cloned().unwrap_or(Value::Null),
        "group_id": input.get("group_id").cloned().unwrap_or(Value::Null),
    });
    if let Some(org) = input.get("organization_id") {
        doc["organization_id"] = org.clone();
    }
    let public = comment.public;
    store.collection_mut(TICKETS).insert(id.to_string(), doc);
    let comment_doc = insert_comment(store, now, id, comment, me, audit_id, event_id);
    let full = render_ticket(store, get(store, TICKETS, id).expect("just inserted"));
    store
        .collection_mut(TICKETS)
        .insert(id.to_string(), full.clone());

    let mut events = vec![json!({
        "id": event_id,
        "type": "Comment",
        "author_id": comment_doc["author_id"],
        "body": comment_doc["body"],
        "html_body": comment_doc["html_body"],
        "public": public,
        "attachments": [],
        "audit_id": audit_id,
    })];
    let mut next = event_id + 1;
    for field in [
        "subject",
        "status",
        "priority",
        "type",
        "requester_id",
        "assignee_id",
        "tags",
    ] {
        if !full[field].is_null() && full[field] != json!([]) {
            events.push(
                json!({ "id": next, "type": "Create", "value": full[field], "field_name": field }),
            );
            next += 1;
        }
    }
    let audit = audit(store, now, audit_id, id, me, events);
    Ok((full, audit))
}

fn find_or_create_user(
    store: &mut Store,
    now: &str,
    requester: &Map<String, Value>,
) -> ApiResult<u64> {
    let email = requester.get("email").and_then(Value::as_str);
    if let Some(email) = email
        && let Some((_, u)) = docs(store, USERS).into_iter().find(|(_, u)| {
            u["email"]
                .as_str()
                .is_some_and(|e| e.eq_ignore_ascii_case(email))
        })
    {
        return Ok(id_of(u));
    }
    let Some(email) = email else {
        return Err(ApiError::invalid(
            "requester",
            "Requester: Email is required",
            "InvalidValue",
        ));
    };
    let id = max_key_id(store, USERS) + 1;
    let name = requester
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| email.split('@').next().unwrap_or(email).to_owned());
    store.collection_mut(USERS).insert(
        id.to_string(),
        json!({ "id": id, "name": name, "email": email.to_lowercase(), "role": "end-user", "created_at": now, "updated_at": now, "verified": false }),
    );
    Ok(id)
}

/// `PUT /api/v2/tickets/{id}` with the `ticket` object. Returns `(ticket, audit)`.
pub fn update_ticket(
    store: &mut Store,
    now: &str,
    id: u64,
    input: &Map<String, Value>,
) -> ApiResult<(Value, Value)> {
    let me = current_user(store);
    let Some(current) = get(store, TICKETS, id) else {
        return Err(ApiError::not_found());
    };
    let before = render_ticket(store, current);
    if before["status"] == "closed" {
        return Err(ApiError::invalid(
            "status",
            "Status: closed prevents ticket update",
            "ClosedPreventsUpdate",
        ));
    }
    validate(store, input)?;
    if input.get("status").and_then(Value::as_str) == Some("new") && before["status"] != "new" {
        return Err(ApiError::invalid(
            "status",
            "Status: cannot be set to New",
            "InvalidValue",
        ));
    }
    let comment = input.get("comment").and_then(parse_comment);

    let mut after = before.clone();
    for field in SETTABLE {
        if let Some(value) = input.get(field) {
            after[field] = if field == "tags" {
                normalize_tags(value)
            } else {
                value.clone()
            };
        }
    }
    if let Some(extra) = input.get("additional_tags") {
        let mut tags: Vec<Value> = after["tags"].as_array().cloned().unwrap_or_default();
        for t in normalize_tags(extra).as_array().into_iter().flatten() {
            if !tags.contains(t) {
                tags.push(t.clone());
            }
        }
        after["tags"] = json!(tags);
    }
    if let Some(remove) = input.get("remove_tags") {
        let remove = normalize_tags(remove);
        let tags: Vec<Value> = after["tags"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|t| !remove.as_array().is_some_and(|r| r.contains(t)))
            .collect();
        after["tags"] = json!(tags);
    }
    if input.contains_key("subject") {
        after["raw_subject"] = after["subject"].clone();
    }
    if input.contains_key("custom_fields") {
        after["fields"] = after["custom_fields"].clone();
    }
    let touched = comment.is_some() || input.contains_key("assignee_id");
    if after["status"] == "new" && !input.contains_key("status") && touched {
        after["status"] = json!("open");
    }
    if after["status"] == "solved" && after["assignee_id"].is_null() {
        after["assignee_id"] = json!(me);
    }
    after["updated_at"] = json!(now);

    let audit_id = next_audit_id(store);
    let mut next = next_event_id(store);
    let mut events = Vec::new();
    if let Some(comment) = comment {
        let public = comment.public;
        let c = insert_comment(store, now, id, comment, me, audit_id, next);
        events.push(json!({
            "id": next,
            "type": "Comment",
            "author_id": c["author_id"],
            "body": c["body"],
            "html_body": c["html_body"],
            "public": public,
            "attachments": [],
            "audit_id": audit_id,
        }));
        next += 1;
    }
    for field in SETTABLE {
        if after[field] != before[field] {
            events.push(json!({
                "id": next,
                "type": "Change",
                "value": after[field],
                "field_name": field,
                "previous_value": before[field],
            }));
            next += 1;
        }
    }
    // Write back only the stored fields that changed, keeping the dataset document's shape.
    let stored = store
        .collection_mut(TICKETS)
        .get_mut(&id.to_string())
        .expect("ticket exists");
    for field in SETTABLE
        .iter()
        .copied()
        .chain(["raw_subject", "fields", "updated_at"])
    {
        if after[field] != before[field] {
            stored[field] = after[field].clone();
        }
    }
    let rendered = render_ticket(store, get(store, TICKETS, id).expect("ticket exists"));
    let audit = audit(store, now, audit_id, id, me, events);
    Ok((rendered, audit))
}

// ───────────────────────────── attachments ─────────────────────────────

/// Every attachment on every comment, rendered, with the ticket it belongs to.
pub fn attachments(store: &Store) -> Vec<(u64, Value, Value)> {
    docs(store, COMMENTS)
        .into_iter()
        .flat_map(|(key, c)| {
            let ticket_id: u64 = key
                .split('/')
                .next()
                .and_then(|t| t.parse().ok())
                .unwrap_or(0);
            c["attachments"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(move |a| (ticket_id, render_attachment(&a), a))
        })
        .collect()
}

pub fn attachment_by_id(store: &Store, id: u64) -> Option<Value> {
    attachments(store)
        .into_iter()
        .find(|(_, a, _)| a["id"] == id)
        .map(|(_, a, _)| a)
}

/// Finds the attachment a content URL (or its `/attachments/token/<token>/` path) points at.
pub fn attachment_by_url(store: &Store, url: &str) -> Option<Value> {
    let token = token_of(url)?;
    attachments(store)
        .into_iter()
        .find(|(_, a, raw)| {
            a["content_url"] == url || attachment_token_from(raw).as_deref() == Some(token)
        })
        .map(|(_, a, _)| a)
}

fn attachment_token_from(raw: &Value) -> Option<String> {
    let url = content_url(raw);
    token_of(&url).map(str::to_owned)
}

fn token_of(url: &str) -> Option<&str> {
    let rest = url.split("/attachments/token/").nth(1)?;
    rest.split(['/', '?']).next().filter(|t| !t.is_empty())
}

/// Raw bytes of an attachment: `attachment_data` if present, else a small stand-in file.
pub fn attachment_bytes(store: &Store, attachment: &Value) -> Vec<u8> {
    let id = id_of(attachment);
    if let Some(data) = get(store, ATTACHMENT_DATA, id).and_then(|d| d["data_base64"].as_str()) {
        return crate::text::b64_decode(data);
    }
    match attachment["content_type"].as_str().unwrap_or_default() {
        "image/png" => crate::text::b64_decode(DEFAULT_PNG),
        "image/gif" => b"GIF89a\x01\x00\x01\x00\x00\x00\x00;".to_vec(),
        "image/jpeg" => {
            b"\xff\xd8\xff\xe0\x00\x10JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00\xff\xd9".to_vec()
        }
        "application/pdf" => format!(
            "%PDF-1.4\n% {}\n%%EOF\n",
            attachment["file_name"].as_str().unwrap_or("document.pdf")
        )
        .into_bytes(),
        _ => format!("{}\n", attachment["file_name"].as_str().unwrap_or("file")).into_bytes(),
    }
}

pub fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ───────────────────────────── search ─────────────────────────────

/// Help Center article search: every query word must appear in the title, body or labels.
pub fn search_articles(store: &Store, query: &str) -> Vec<Value> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Vec::new();
    }
    docs(store, ARTICLES)
        .into_iter()
        .filter(|(_, a)| a["draft"] != true)
        .filter(|(_, a)| {
            let hay = format!(
                "{} {} {}",
                a["title"].as_str().unwrap_or_default(),
                a["body"].as_str().unwrap_or_default(),
                a["label_names"]
            )
            .to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .map(|(_, a)| {
            let mut r = render_article(a);
            let snippet: String = crate::text::html_to_text(a["body"].as_str().unwrap_or_default())
                .chars()
                .take(120)
                .collect();
            r["snippet"] = json!(snippet);
            r["result_type"] = json!("article");
            r
        })
        .collect()
}

pub fn search_users(store: &Store, query: &str) -> Vec<Value> {
    let q = query.trim().trim_start_matches("email:").to_lowercase();
    docs(store, USERS)
        .into_iter()
        .filter(|(_, u)| {
            !q.is_empty()
                && (u["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(&q)
                    || u["email"]
                        .as_str()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&q))
        })
        .map(|(_, u)| render_user(u))
        .collect()
}
