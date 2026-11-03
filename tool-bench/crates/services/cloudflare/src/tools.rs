//! The four Code Mode tools of cloudflare/mcp@68c7ec0: `docs`, `search`, `execute`, `whoami`.
//! Result and error texts follow `src/tools/*.ts`.

use mock_core::{
    journal::{JournalEntry, body_value},
    mcp::ToolCall,
    world::SharedWorld,
};
use serde_json::{Value, json};

use crate::{
    NAME, accounts,
    api::{self, ApiRequest, Body, Part, session_accounts, str_field},
    js::{self, Mode, Outcome},
};

const SPEC: &str = include_str!("../assets/spec.json");
const DOCS: &str = include_str!("../assets/docs.json");

pub fn call(world: &SharedWorld, call: &ToolCall, source: &str) -> Value {
    match call.name.as_str() {
        "whoami" => whoami(world),
        "docs" => match call.arguments.get("query").and_then(Value::as_str) {
            Some(q) => docs(q),
            None => invalid("docs", "query"),
        },
        "search" => match call.arguments.get("code").and_then(Value::as_str) {
            Some(code) => search(code, truncate_arg(&call.arguments)),
            None => invalid("search", "code"),
        },
        "execute" => match call.arguments.get("code").and_then(Value::as_str) {
            Some(code) => execute(
                world,
                code,
                call.arguments.get("account_id").and_then(Value::as_str),
                truncate_arg(&call.arguments),
                source,
            ),
            None => invalid("execute", "code"),
        },
        other => error_text(format!("Tool {other} not found")),
    }
}

fn truncate_arg(args: &Value) -> Option<bool> {
    args.get("truncate").and_then(Value::as_bool)
}

fn text(text: impl Into<String>) -> Value {
    json!({ "content": [{ "type": "text", "text": text.into() }] })
}

fn error_text(text: impl Into<String>) -> Value {
    json!({ "content": [{ "type": "text", "text": text.into() }], "isError": true })
}

/// What the MCP SDK answers when the arguments don't match the input schema.
fn invalid(tool: &str, field: &str) -> Value {
    error_text(format!(
        "MCP error -32602: Input validation error: Invalid arguments for tool {tool}: [\n  {{\n    \"expected\": \"string\",\n    \"code\": \"invalid_type\",\n    \"path\": [\n      \"{field}\"\n    ],\n    \"message\": \"Invalid input: expected string, received undefined\"\n  }}\n]"
    ))
}

/// `formatError`: `Error: <message>`.
fn format_error(message: &str) -> Value {
    error_text(format!("Error: {message}"))
}

// ── whoami ───────────────────────────────────────────────────────────────────

fn whoami(world: &SharedWorld) -> Value {
    let w = world.lock().expect("world lock");
    let user = accounts::user(&w);
    let id = str_field(&user, "id");
    if id.trim().is_empty() {
        return error_text("This credential has no Cloudflare user or account ID.");
    }
    let mut profile = json!({ "id": format!("user:{id}") });
    if let Some(email) = user
        .get("email")
        .and_then(Value::as_str)
        .filter(|e| !e.is_empty())
    {
        profile["email"] = json!(email);
    }
    json!({
        "content": [{ "type": "text", "text": profile.to_string() }],
        "structuredContent": profile,
    })
}

// ── docs ─────────────────────────────────────────────────────────────────────

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "can", "do", "does", "for", "from", "how",
    "i", "in", "is", "it", "my", "of", "on", "or", "the", "to", "what", "when", "which", "with",
    "you", "your",
];

fn tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() > 1 && !STOPWORDS.contains(t))
        .map(str::to_owned)
        .collect()
}

fn term_matches(term: &str, token: &str) -> bool {
    term == token
        || (term.len() >= 4
            && token.len() >= 4
            && (token.starts_with(term) || term.starts_with(token)))
}

/// `sourceToUrl` from docs-search.ts.
pub fn source_to_url(filename: &str) -> String {
    if filename.starts_with("https://") || filename.starts_with("http://") {
        return filename.to_owned();
    }
    let trimmed = filename
        .strip_suffix("index.mdx")
        .or_else(|| filename.strip_suffix("index.md"))
        .or_else(|| filename.strip_suffix(".mdx"))
        .or_else(|| filename.strip_suffix(".md"))
        .unwrap_or(filename);
    format!("https://developers.cloudflare.com/{trimmed}")
}

/// `extractTitle` from docs-search.ts.
pub fn extract_title(filename: &str) -> String {
    let path = filename.trim_end_matches('/');
    let path = path
        .strip_suffix(".mdx")
        .or_else(|| path.strip_suffix(".md"))
        .unwrap_or(path);
    let parts: Vec<&str> = path.split('/').collect();
    let last = parts.last().copied().unwrap_or("");
    let word = if last == "index" {
        parts
            .len()
            .checked_sub(2)
            .and_then(|i| parts.get(i))
            .copied()
            .unwrap_or("Documentation")
    } else if last.is_empty() {
        "Documentation"
    } else {
        last
    };
    title_case(word)
}

fn title_case(value: &str) -> String {
    let spaced = value.replace(['-', '_'], " ");
    let mut out = String::new();
    let mut boundary = true;
    for c in spaced.chars() {
        if boundary && c.is_ascii_alphanumeric() {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
        boundary = !(c.is_ascii_alphanumeric() || c == '_');
    }
    out
}

pub fn docs(query: &str) -> Value {
    let corpus: Vec<Value> = serde_json::from_str(DOCS).expect("docs corpus is JSON");
    let terms: Vec<String> = {
        let mut t = tokens(query);
        t.dedup();
        let mut seen = Vec::new();
        t.retain(|x| {
            let new = !seen.contains(x);
            seen.push(x.clone());
            new
        });
        t
    };
    let mut scored: Vec<(f64, usize)> = corpus
        .iter()
        .enumerate()
        .map(|(i, doc)| {
            let filename = str_field(doc, "filename");
            let body = tokens(str_field(doc, "text"));
            let path = tokens(filename);
            let weight: f64 = terms
                .iter()
                .map(|t| {
                    let in_body = body.iter().any(|b| term_matches(t, b));
                    let in_path = path.iter().any(|b| term_matches(t, b));
                    f64::from(u8::from(in_body)) + 0.5 * f64::from(u8::from(in_path))
                })
                .sum();
            let max = (terms.len() as f64 * 1.5).max(1.0);
            ((weight / max * 10_000.0).round() / 10_000.0, i)
        })
        .filter(|(s, _)| *s > 0.0)
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let results: Vec<Value> = scored
        .into_iter()
        .take(5)
        .map(|(score, i)| {
            let doc = &corpus[i];
            let filename = str_field(doc, "filename");
            json!({
                "similarity": score,
                "id": doc["id"],
                "url": source_to_url(filename),
                "title": extract_title(filename),
                "text": doc["text"],
            })
        })
        .collect();
    let rendered = results
        .iter()
        .map(|r| {
            format!(
                "<result>\n<url>{}</url>\n<title>{}</title>\n<text>\n{}\n</text>\n</result>",
                str_field(r, "url"),
                str_field(r, "title"),
                str_field(r, "text")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    json!({
        "content": [{ "type": "text", "text": rendered }],
        "structuredContent": { "results": results },
    })
}

// ── search ───────────────────────────────────────────────────────────────────

pub fn search(code: &str, truncate: Option<bool>) -> Value {
    match js::run(Mode::Search { spec: SPEC }, code, truncate) {
        Outcome::Ok(t) => text(t),
        Outcome::Err(e) => format_error(&e),
    }
}

// ── execute ──────────────────────────────────────────────────────────────────

fn format_accounts(accounts: &[Value]) -> String {
    accounts
        .iter()
        .map(|a| format!("- {} ({})", str_field(a, "id"), str_field(a, "name")))
        .collect::<Vec<_>>()
        .join("\n")
}

fn missing_account_message(accounts: &[Value]) -> String {
    if accounts.is_empty() {
        "No account selected: no Cloudflare accounts are authorized for this session.".to_owned()
    } else {
        format!(
            "No account selected. Pass account_id with one of this session's accounts:\n{}",
            format_accounts(accounts)
        )
    }
}

fn known(accounts: &[Value], id: &str) -> bool {
    accounts.iter().any(|a| str_field(a, "id") == id)
}

/// The account in a refused request's path (`refusedAccountId`).
fn refused_account(message: &str) -> Option<String> {
    let at = message.find("/accounts/")?;
    let id = message.get(at + 10..at + 42)?;
    let rest = &message[at + 42..];
    let line = rest.split('\n').next().unwrap_or("");
    (api::is_hex32(id)
        && !rest.starts_with(|c: char| c.is_ascii_alphanumeric())
        && (line.contains("returned HTTP 401") || line.contains("returned HTTP 403")))
    .then(|| id.to_owned())
}

pub fn execute(
    world: &SharedWorld,
    code: &str,
    account_id: Option<&str>,
    truncate: Option<bool>,
    source: &str,
) -> Value {
    let accounts = session_accounts(&world.lock().expect("world lock"));
    let effective = account_id
        .filter(|a| !a.is_empty())
        .map(str::to_owned)
        .or_else(|| (accounts.len() == 1).then(|| str_field(&accounts[0], "id").to_owned()));
    let fetch_world = world.clone();
    let source = source.to_owned();
    let mode = Mode::Execute {
        account_id: effective,
        unresolved_message: missing_account_message(&accounts),
        fetch: Box::new(move |req: String| host_fetch(&fetch_world, &req, &source)),
    };
    match js::run(mode, code, truncate) {
        Outcome::Ok(t) => text(t),
        Outcome::Err(message) => {
            let mut failure = format!("Error: {message}");
            let hint = match account_id {
                Some(id) if !known(&accounts, id) => format!(
                    "account_id {id} is not one of this session's accounts:\n{}",
                    format_accounts(&accounts)
                ),
                Some(_) => String::new(),
                None => match refused_account(&message) {
                    Some(id) if !known(&accounts, &id) => format!(
                        "The request's account {id} is not one of this session's accounts. Use one of these, or pass it as account_id and use `accountId` in the path:\n{}",
                        format_accounts(&accounts)
                    ),
                    _ => String::new(),
                },
            };
            if !hint.is_empty() {
                failure.push_str("\n\n");
                failure.push_str(&hint);
            }
            error_text(failure)
        }
    }
}

fn bytes_of(v: &Value) -> Vec<u8> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|b| b.as_u64().map(|b| b as u8))
                .collect()
        })
        .unwrap_or_default()
}

/// The `__cf_fetch` binding: one API request, answered in-process and journaled as if it
/// had reached the world over HTTP.
fn host_fetch(world: &SharedWorld, request: &str, source: &str) -> String {
    let parsed: Value = serde_json::from_str(request).unwrap_or(Value::Null);
    let extra: Vec<(String, String)> = parsed["query"]
        .as_array()
        .map(|pairs| {
            pairs
                .iter()
                .filter_map(|p| {
                    Some((
                        p.get(0)?.as_str()?.to_owned(),
                        p.get(1)?.as_str()?.to_owned(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut req = ApiRequest::new(
        parsed["method"].as_str().unwrap_or("GET"),
        parsed["path"].as_str().unwrap_or(""),
        extra,
    );
    req.content_type = parsed["contentType"].as_str().map(str::to_owned);
    req.body = match parsed["body"]["kind"].as_str() {
        Some("text") => Body::Bytes(
            parsed["body"]["text"]
                .as_str()
                .unwrap_or("")
                .as_bytes()
                .to_vec(),
        ),
        Some("bytes") => Body::Bytes(bytes_of(&parsed["body"]["bytes"])),
        Some("form") => Body::Form(
            parsed["body"]["parts"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|p| Part {
                    name: str_field(p, "name").to_owned(),
                    filename: p.get("filename").and_then(Value::as_str).map(str::to_owned),
                    content_type: p.get("type").and_then(Value::as_str).map(str::to_owned),
                    bytes: match p.get("text").and_then(Value::as_str) {
                        Some(t) => t.as_bytes().to_vec(),
                        None => bytes_of(&p["bytes"]),
                    },
                })
                .collect(),
        ),
        _ => Body::None,
    };

    let mut w = world.lock().expect("world lock");
    if api::is_mutating(&req.method) {
        w.clock.tick();
    }
    let response = api::handle(&mut w, &req);
    let entry = JournalEntry {
        seq: w.journal.next_seq(),
        time: w.clock.now_rfc3339(),
        service: NAME.to_owned(),
        source: source.to_owned(),
        method: req.method.clone(),
        // Recorded from inside a tool call; world-replay skips "internal" entries because
        // replaying the enclosing /_mcp/tools/call reproduces them.
        host: None,
        scheme: "internal".to_owned(),
        content_type: None,
        path: format!("/client/v4{}", req.path),
        query: req.query_string(),
        request_body: req.journal_body(),
        status: response.status,
        response_body: body_value(&response.body),
    };
    w.journal.push(entry);
    json!({
        "status": response.status,
        "contentType": response.content_type,
        "bytes": response.body,
    })
    .to_string()
}
