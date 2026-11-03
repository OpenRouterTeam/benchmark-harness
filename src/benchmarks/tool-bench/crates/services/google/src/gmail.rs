//! Gmail tools over the `gmail_messages`, `gmail_labels`, `gmail_drafts`, `gmail_filters`
//! and `gmail_settings` collections.
//!
//! A message document: `{id, threadId, labelIds, internalDate (ms), headers: {From, To, Cc,
//! Subject, Date, Message-ID, ...}, body_text, body_html, attachments: [{attachmentId,
//! filename, mimeType, size, data}]}`.

use serde_json::{Map, Value, json};

use crate::{
    tz,
    util::{Ctx, R, USER, arg, arg_bool, arg_i64, arg_json, arg_list, not_found_error, rfc2822, s},
};

const SYSTEM_LABELS: [&str; 13] = [
    "CHAT",
    "SENT",
    "INBOX",
    "IMPORTANT",
    "TRASH",
    "DRAFT",
    "SPAM",
    "CATEGORY_FORUMS",
    "CATEGORY_UPDATES",
    "CATEGORY_PERSONAL",
    "CATEGORY_PROMOTIONS",
    "CATEGORY_SOCIAL",
    "STARRED",
];

fn web_url(id: &str) -> String {
    format!("https://mail.google.com/mail/u/0/#all/{id}")
}

fn user_zone(ctx: &Ctx) -> String {
    ctx.get("gmail_settings", "settings")
        .map(|v| s(&v, "timeZone").to_owned())
        .filter(|z| !z.is_empty())
        .unwrap_or_else(|| "America/New_York".to_owned())
}

fn internal_ms(m: &Value) -> i64 {
    match &m["internalDate"] {
        Value::Number(n) => n.as_i64().unwrap_or(0),
        Value::String(x) => x.parse().unwrap_or(0),
        _ => 0,
    }
}

fn labels(m: &Value) -> Vec<String> {
    m["labelIds"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn header<'a>(m: &'a Value, name: &str) -> &'a str {
    m["headers"].get(name).and_then(Value::as_str).unwrap_or("")
}

fn has_header(m: &Value, name: &str) -> bool {
    m["headers"].get(name).is_some()
}

// ───────────────────────────── search ─────────────────────────────

#[derive(Debug, Clone)]
enum Q {
    Term(String, String),
    Text(String),
    Not(Box<Q>),
    And(Vec<Q>),
    Or(Vec<Q>),
}

fn tokenize(q: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = q.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                cur.push('"');
                for n in chars.by_ref() {
                    cur.push(n);
                    if n == '"' {
                        break;
                    }
                }
            }
            '(' | ')' | '{' | '}' if !cur.ends_with(':') || c == ')' || c == '}' => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                out.push(c.to_string());
            }
            '(' => {
                // key:(a b) groups the value
                let mut depth = 1;
                cur.push('(');
                for n in chars.by_ref() {
                    cur.push(n);
                    if n == '(' {
                        depth += 1;
                    } else if n == ')' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                }
            }
            c if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn parse_seq(tokens: &[String], pos: &mut usize, close: Option<&str>) -> Q {
    let mut groups: Vec<Vec<Q>> = vec![Vec::new()];
    while *pos < tokens.len() {
        let t = tokens[*pos].clone();
        *pos += 1;
        if close.is_some_and(|c| c == t) {
            break;
        }
        if t == "OR" || t == "|" {
            groups.push(Vec::new());
            continue;
        }
        let item = if t == "(" {
            parse_seq(tokens, pos, Some(")"))
        } else if t == "{" {
            match parse_seq(tokens, pos, Some("}")) {
                Q::And(items) => Q::Or(items),
                other => other,
            }
        } else if let Some(rest) = t.strip_prefix('-').filter(|r| !r.is_empty()) {
            Q::Not(Box::new(atom(rest)))
        } else {
            atom(&t)
        };
        groups.last_mut().expect("group").push(item);
    }
    let ands: Vec<Q> = groups
        .into_iter()
        .filter(|g| !g.is_empty())
        .map(|g| {
            if g.len() == 1 {
                g.into_iter().next().expect("one")
            } else {
                Q::And(g)
            }
        })
        .collect();
    if ands.len() == 1 {
        ands.into_iter().next().expect("one")
    } else if ands.is_empty() {
        Q::And(Vec::new())
    } else {
        Q::Or(ands)
    }
}

fn atom(t: &str) -> Q {
    if let Some((k, v)) = t.split_once(':')
        && !k.is_empty()
        && k.chars().all(|c| c.is_ascii_alphabetic() || c == '_')
    {
        let v = v.trim_matches('"');
        if let Some(inner) = v.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
            let parts: Vec<Q> = inner
                .split_whitespace()
                .map(|p| Q::Term(k.to_lowercase(), p.to_owned()))
                .collect();
            return Q::And(parts);
        }
        return Q::Term(k.to_lowercase(), v.to_owned());
    }
    Q::Text(t.trim_matches('"').to_lowercase())
}

fn day_start(ctx: &Ctx, v: &str) -> Option<i64> {
    let v = v.replace('/', "-");
    let (y, m, d) = tz::parse_date(&v)?;
    let local = tz::days_from_civil(y, m, d) * 86_400;
    Some(local - tz::offset_at(&user_zone(ctx), local).0)
}

fn rel_secs(v: &str) -> Option<i64> {
    let (n, unit) = v.split_at(v.len().saturating_sub(1));
    let n: i64 = n.parse().ok()?;
    Some(
        n * match unit {
            "d" => 86_400,
            "m" => 30 * 86_400,
            "y" => 365 * 86_400,
            "h" => 3600,
            _ => return None,
        },
    )
}

fn contains_ci(hay: &str, needle: &str) -> bool {
    hay.to_lowercase().contains(&needle.to_lowercase())
}

fn eval(ctx: &Ctx, q: &Q, m: &Value) -> bool {
    match q {
        Q::And(items) => items.iter().all(|i| eval(ctx, i, m)),
        Q::Or(items) => items.iter().any(|i| eval(ctx, i, m)),
        Q::Not(i) => !eval(ctx, i, m),
        Q::Text(t) => {
            let names: Vec<String> = m["attachments"]
                .as_array()
                .map(|a| a.iter().map(|x| s(x, "filename").to_owned()).collect())
                .unwrap_or_default();
            let hay = format!(
                "{} {} {} {} {} {} {}",
                header(m, "Subject"),
                header(m, "From"),
                header(m, "To"),
                header(m, "Cc"),
                s(m, "body_text"),
                s(m, "body_html"),
                names.join(" ")
            );
            contains_ci(&hay, t)
        }
        Q::Term(k, v) => {
            let ls = labels(m);
            let label_names: Vec<String> = ls
                .iter()
                .map(|id| {
                    ctx.get("gmail_labels", id)
                        .map(|l| s(&l, "name").to_owned())
                        .unwrap_or_else(|| id.clone())
                })
                .collect();
            let secs = internal_ms(m) / 1000;
            match k.as_str() {
                "from" => contains_ci(header(m, "From"), v),
                "to" => {
                    contains_ci(header(m, "To"), v)
                        || contains_ci(header(m, "Cc"), v)
                        || contains_ci(header(m, "Bcc"), v)
                }
                "cc" => contains_ci(header(m, "Cc"), v),
                "bcc" => contains_ci(header(m, "Bcc"), v),
                "subject" => contains_ci(header(m, "Subject"), v),
                "deliveredto" => contains_ci(header(m, "To"), v),
                "list" => contains_ci(header(m, "List-Id"), v),
                "filename" => m["attachments"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|x| contains_ci(s(x, "filename"), v))),
                "label" => {
                    let v2 = v.replace('-', " ").to_lowercase();
                    ls.iter().any(|l| l.eq_ignore_ascii_case(v))
                        || label_names.iter().any(|n| {
                            n.to_lowercase() == v.to_lowercase()
                                || n.to_lowercase().replace(['-', '/'], " ") == v2
                        })
                }
                "category" => ls
                    .iter()
                    .any(|l| l.eq_ignore_ascii_case(&format!("CATEGORY_{v}"))),
                "in" => match v.to_lowercase().as_str() {
                    "anywhere" => true,
                    "inbox" => ls.iter().any(|l| l == "INBOX"),
                    "sent" => ls.iter().any(|l| l == "SENT"),
                    "draft" | "drafts" => ls.iter().any(|l| l == "DRAFT"),
                    "trash" => ls.iter().any(|l| l == "TRASH"),
                    "spam" => ls.iter().any(|l| l == "SPAM"),
                    "chats" => ls.iter().any(|l| l == "CHAT"),
                    other => label_names.iter().any(|n| n.eq_ignore_ascii_case(other)),
                },
                "is" => match v.to_lowercase().as_str() {
                    "unread" => ls.iter().any(|l| l == "UNREAD"),
                    "read" => !ls.iter().any(|l| l == "UNREAD"),
                    "starred" => ls.iter().any(|l| l == "STARRED"),
                    "important" => ls.iter().any(|l| l == "IMPORTANT"),
                    "draft" => ls.iter().any(|l| l == "DRAFT"),
                    _ => false,
                },
                "has" => match v.to_lowercase().as_str() {
                    "attachment" => m["attachments"].as_array().is_some_and(|a| !a.is_empty()),
                    "userlabels" => ls.iter().any(|l| l.starts_with("Label_")),
                    "nouserlabels" => !ls.iter().any(|l| l.starts_with("Label_")),
                    _ => false,
                },
                "after" => {
                    day_start(ctx, v).is_some_and(|d| secs >= d)
                        || v.parse::<i64>().is_ok_and(|e| secs >= e)
                }
                "before" => {
                    day_start(ctx, v).is_some_and(|d| secs < d)
                        || v.parse::<i64>().is_ok_and(|e| secs < e)
                }
                "newer_than" => rel_secs(v).is_some_and(|r| secs > ctx.now - r),
                "older_than" => rel_secs(v).is_some_and(|r| secs < ctx.now - r),
                "larger" | "smaller" => true,
                _ => {
                    contains_ci(&format!("{k}:{v}"), "")
                        && eval(ctx, &Q::Text(format!("{k}:{v}")), m)
                }
            }
        }
    }
}

fn mentions(q: &Q, key: &str, values: &[&str]) -> bool {
    match q {
        Q::Term(k, v) => k == key && values.iter().any(|x| v.eq_ignore_ascii_case(x)),
        Q::And(a) | Q::Or(a) => a.iter().any(|x| mentions(x, key, values)),
        Q::Not(x) => mentions(x, key, values),
        Q::Text(_) => false,
    }
}

pub fn search(ctx: &Ctx, query: &str) -> Vec<Value> {
    let tokens = tokenize(query);
    let mut pos = 0;
    let q = parse_seq(&tokens, &mut pos, None);
    let include_trash = mentions(&q, "in", &["trash", "anywhere"]);
    let include_spam = mentions(&q, "in", &["spam", "anywhere"]);
    let mut out: Vec<Value> = ctx
        .all("gmail_messages")
        .into_iter()
        .filter(|m| {
            let ls = labels(m);
            (include_trash || !ls.iter().any(|l| l == "TRASH"))
                && (include_spam || !ls.iter().any(|l| l == "SPAM"))
        })
        .filter(|m| eval(ctx, &q, m))
        .collect();
    out.sort_by_key(|m| std::cmp::Reverse(internal_ms(m)));
    out
}

pub fn search_messages(ctx: &mut Ctx, a: &Value) -> R {
    let query = s(a, "query").to_owned();
    let page_size = arg_i64(a, "page_size").unwrap_or(10).max(1) as usize;
    let offset: usize = arg(a, "page_token")
        .and_then(|t| t.trim_start_matches('0').parse().ok())
        .unwrap_or(0);
    let all = search(ctx, &query);
    let page: Vec<&Value> = all.iter().skip(offset).take(page_size).collect();
    let next = (all.len() > offset + page_size).then(|| format!("{:020}", offset + page_size));
    if page.is_empty() {
        return Ok(format!("No messages found for query: '{query}'"));
    }
    let include_headers = arg_bool(a, "include_headers").unwrap_or(false);
    let mut lines = vec![
        format!("Found {} messages matching '{query}':", page.len()),
        String::new(),
        "📧 MESSAGES:".into(),
    ];
    for (i, m) in page.iter().enumerate() {
        let id = s(m, "id");
        let tid = s(m, "threadId");
        lines.push(format!("  {}. Message ID: {id}", i + 1));
        if include_headers {
            lines.push(format!(
                "     Subject: {}",
                non_empty(header(m, "Subject"), "(no subject)")
            ));
            lines.push(format!(
                "     From: {}",
                non_empty(header(m, "From"), "(unknown sender)")
            ));
            lines.push(format!(
                "     Date: {}",
                non_empty(header(m, "Date"), "(unknown date)")
            ));
            let ls = labels(m);
            if !ls.is_empty() {
                lines.push(format!("     Labels: {}", ls.join(", ")));
            }
        }
        lines.push(format!("     Web Link: {}", web_url(id)));
        lines.push(format!("     Thread ID: {tid}"));
        lines.push(format!("     Thread Link: {}", web_url(tid)));
        lines.push(String::new());
    }
    lines.extend([
        "💡 USAGE:".to_owned(),
        "  • Pass the Message IDs **as a list** to get_gmail_messages_content_batch()".to_owned(),
        "    e.g. get_gmail_messages_content_batch(message_ids=[...])".to_owned(),
        "  • Pass the Thread IDs to get_gmail_thread_content() (single) or get_gmail_threads_content_batch() (batch)".to_owned(),
    ]);
    if let Some(n) = next {
        lines.push(String::new());
        lines.push(format!("📄 PAGINATION: To get the next page, call search_gmail_messages again with page_token='{n}'"));
    }
    Ok(lines.join("\n"))
}

fn non_empty<'a>(v: &'a str, default: &'a str) -> &'a str {
    if v.is_empty() { default } else { v }
}

// ───────────────────────────── reading ─────────────────────────────

fn header_lines(m: &Value, with_id: bool) -> Vec<String> {
    let mut out = Vec::new();
    if with_id {
        out.push(format!("Message ID: {}", s(m, "id")));
    }
    out.push(format!(
        "Subject: {}",
        non_empty(header(m, "Subject"), "(no subject)")
    ));
    out.push(format!(
        "From: {}",
        non_empty(header(m, "From"), "(unknown sender)")
    ));
    out.push(format!(
        "Date: {}",
        non_empty(header(m, "Date"), "(unknown date)")
    ));
    for h in ["Reply-To", "Message-ID", "In-Reply-To", "References"] {
        if !header(m, h).is_empty() {
            out.push(format!("{h}: {}", header(m, h)));
        }
    }
    out.push(if has_header(m, "To") {
        format!("To: {}", header(m, "To"))
    } else {
        "To: [not present in Gmail response]".into()
    });
    out.push(if has_header(m, "Cc") {
        format!("Cc: {}", header(m, "Cc"))
    } else {
        "Cc: [not present in Gmail response]".into()
    });
    for h in ["List-Unsubscribe", "Precedence", "List-Id"] {
        if !header(m, h).is_empty() {
            out.push(format!("{h}: {}", header(m, h)));
        }
    }
    out
}

fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut tag = String::new();
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' => {
                in_tag = false;
                let t = tag.to_lowercase();
                if t.starts_with("br")
                    || t.starts_with("/p")
                    || t.starts_with("/div")
                    || t.starts_with("/li")
                    || t.starts_with("/h")
                {
                    out.push('\n');
                }
            }
            c if in_tag => tag.push(c),
            c => out.push(c),
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn body(m: &Value, format: &str) -> String {
    let text = s(m, "body_text");
    let html = s(m, "body_html");
    if format == "html" {
        if !html.trim().is_empty() {
            return html.trim().to_owned();
        }
        return if text.trim().is_empty() {
            "[No readable content found]".into()
        } else {
            text.trim().to_owned()
        };
    }
    if !text.trim().is_empty() {
        text.to_owned()
    } else if !html.trim().is_empty() {
        html_to_text(html).trim().to_owned()
    } else {
        "[No readable content found]".into()
    }
}

fn attachment_lines(m: &Value, trailing_newline: bool) -> Vec<String> {
    let mut out = Vec::new();
    for (i, att) in m["attachments"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        let kb = att["size"].as_f64().unwrap_or(0.0) / 1024.0;
        out.push(format!(
            "{}. {} ({}, {kb:.1} KB)\n   Attachment ID: {}\n   Use get_gmail_attachment_content(message_id='{}', attachment_id='{}', attachment_index={i}) to download{}",
            i + 1,
            s(att, "filename"),
            s(att, "mimeType"),
            s(att, "attachmentId"),
            s(m, "id"),
            s(att, "attachmentId"),
            if trailing_newline { "\n" } else { "" }
        ));
    }
    out
}

fn raw_mime(m: &Value) -> String {
    let mut out = String::new();
    for h in ["From", "To", "Cc", "Subject", "Date", "Message-ID"] {
        if has_header(m, h) {
            out.push_str(&format!("{h}: {}\r\n", header(m, h)));
        }
    }
    out.push_str("MIME-Version: 1.0\r\nContent-Type: text/plain; charset=\"UTF-8\"\r\n\r\n");
    out.push_str(s(m, "body_text"));
    out
}

fn need_message(ctx: &Ctx, tool: &str, id: &str) -> Result<Value, String> {
    ctx.get("gmail_messages", id).ok_or_else(|| {
        not_found_error(tool, &format!("https://gmail.googleapis.com/gmail/v1/users/me/messages/{id}?format=metadata&alt=json"))
    })
}

pub fn get_message(ctx: &mut Ctx, a: &Value) -> R {
    let id = s(a, "message_id").to_owned();
    let format = arg(a, "format").unwrap_or("full");
    let body_format = arg(a, "body_format").unwrap_or("text");
    let include_headers = arg_bool(a, "include_headers").unwrap_or(true);
    if format == "metadata" && arg_bool(a, "full").unwrap_or(false) {
        return Err("full=True requires format='full'.".into());
    }
    if format == "metadata" && !include_headers {
        return Err("include_headers=False requires format='full'.".into());
    }
    let m = need_message(ctx, "get_gmail_message_content", &id)?;
    let mut lines = if include_headers {
        header_lines(&m, false)
    } else {
        Vec::new()
    };
    if format == "metadata" {
        return Ok(lines.join("\n"));
    }
    if body_format == "raw" {
        if !include_headers {
            return Err("include_headers=False cannot hide the headers in inline raw MIME; use body_format='text' or a full=True file export.".into());
        }
        lines.push(format!("\n--- RAW MIME ---\n{}", raw_mime(&m)));
        return Ok(lines.join("\n"));
    }
    lines.push(format!("\n--- BODY ---\n{}", body(&m, body_format)));
    let att = attachment_lines(&m, false);
    if !att.is_empty() {
        lines.push("\n--- ATTACHMENTS ---".into());
        lines.extend(att);
    }
    Ok(lines.join("\n"))
}

pub fn get_messages_batch(ctx: &mut Ctx, a: &Value) -> R {
    let ids = arg_list(a, "message_ids").unwrap_or_default();
    if ids.is_empty() {
        return Err("No message IDs provided".into());
    }
    let format = arg(a, "format").unwrap_or("full");
    let body_format = arg(a, "body_format").unwrap_or("text");
    let mut outputs = Vec::new();
    for id in &ids {
        let Some(m) = ctx.get("gmail_messages", id) else {
            outputs.push(format!(
                "⚠️ Message {id}: {}\n",
                crate::util::http_error(404, &format!("https://gmail.googleapis.com/gmail/v1/users/me/messages/{id}?format={}&alt=json", if format == "metadata" { "metadata" } else { "full" }), "Requested entity was not found.", "notFound")
            ));
            continue;
        };
        let mut out = header_lines(&m, true).join("\n");
        out.push_str(&format!("\nWeb Link: {}\n", web_url(id)));
        if format != "metadata" {
            let (label, b) = if body_format == "raw" {
                ("RAW MIME", raw_mime(&m))
            } else {
                ("BODY", body(&m, body_format))
            };
            out.push_str(&format!("\n--- {label} ---\n{b}\n"));
            let att = attachment_lines(&m, true);
            if !att.is_empty() {
                out.push_str("\n--- ATTACHMENTS ---\n");
                out.push_str(&att.join(""));
            }
        }
        outputs.push(out);
    }
    Ok(format!(
        "Retrieved {} messages:\n\n{}",
        ids.len(),
        outputs.join("\n---\n\n")
    ))
}

fn thread_messages(ctx: &Ctx, thread_id: &str) -> Vec<Value> {
    let mut v: Vec<Value> = ctx
        .all("gmail_messages")
        .into_iter()
        .filter(|m| s(m, "threadId") == thread_id)
        .collect();
    v.sort_by_key(internal_ms);
    v
}

fn format_thread(ctx: &Ctx, thread_id: &str, body_format: &str) -> String {
    let msgs = thread_messages(ctx, thread_id);
    if msgs.is_empty() {
        return format!("No messages found in thread '{thread_id}'.");
    }
    let subject = non_empty(header(&msgs[0], "Subject"), "(no subject)").to_owned();
    let mut lines = vec![
        format!("Thread ID: {thread_id}"),
        format!("Subject: {subject}"),
        format!("Messages: {}", msgs.len()),
        String::new(),
    ];
    for (i, m) in msgs.iter().enumerate() {
        lines.push(format!("=== Message {} ===", i + 1));
        lines.push(format!(
            "From: {}",
            non_empty(header(m, "From"), "(unknown sender)")
        ));
        lines.push(format!(
            "Date: {}",
            non_empty(header(m, "Date"), "(unknown date)")
        ));
        if !header(m, "Reply-To").is_empty() {
            lines.push(format!("Reply-To: {}", header(m, "Reply-To")));
        }
        lines.push(if has_header(m, "To") {
            format!("To: {}", header(m, "To"))
        } else {
            "To: [not present in Gmail response]".into()
        });
        lines.push(if has_header(m, "Cc") {
            format!("Cc: {}", header(m, "Cc"))
        } else {
            "Cc: [not present in Gmail response]".into()
        });
        for h in ["Message-ID", "In-Reply-To", "References"] {
            if !header(m, h).is_empty() {
                lines.push(format!("{h}: {}", header(m, h)));
            }
        }
        let subj = non_empty(header(m, "Subject"), "(no subject)");
        if subj != subject {
            lines.push(format!("Subject: {subj}"));
        }
        if body_format == "raw" {
            lines.extend([
                String::new(),
                "--- RAW MIME ---".into(),
                raw_mime(m),
                String::new(),
            ]);
        } else {
            lines.extend([String::new(), body(m, body_format), String::new()]);
        }
        let att = attachment_lines(m, false);
        if !att.is_empty() {
            lines.push("--- ATTACHMENTS ---".into());
            lines.extend(att);
            lines.push(String::new());
        }
    }
    lines.join("\n")
}

pub fn get_thread(ctx: &mut Ctx, a: &Value) -> R {
    let tid = s(a, "thread_id").to_owned();
    if thread_messages(ctx, &tid).is_empty() {
        return Err(not_found_error(
            "get_gmail_thread_content",
            &format!(
                "https://gmail.googleapis.com/gmail/v1/users/me/threads/{tid}?format=full&alt=json"
            ),
        ));
    }
    let content = format_thread(ctx, &tid, arg(a, "body_format").unwrap_or("text"));
    if arg_bool(a, "include_analysis").unwrap_or(false) {
        let msgs = thread_messages(ctx, &tid);
        let last = msgs
            .last()
            .map(|m| header(m, "From").to_owned())
            .unwrap_or_default();
        let mut counts = Map::new();
        for m in &msgs {
            let f = header(m, "From").to_owned();
            let n = counts.get(&f).and_then(Value::as_i64).unwrap_or(0) + 1;
            counts.insert(f, json!(n));
        }
        let last_is_user = last.contains(USER);
        let analysis = json!({
            "message_count": msgs.len(), "last_sender": last, "last_sender_is_user": last_is_user,
            "ball_in_court": if last_is_user { "them" } else { "user" },
            "messages_per_sender": counts, "participants": counts.keys().cloned().collect::<Vec<_>>(),
        });
        return Ok(
            serde_json::to_string(&json!({"content": content, "analysis": analysis}))
                .expect("JSON"),
        );
    }
    Ok(content)
}

pub fn get_threads_batch(ctx: &mut Ctx, a: &Value) -> R {
    let ids = arg_list(a, "thread_ids").unwrap_or_default();
    if ids.is_empty() {
        return Err("No thread IDs provided".into());
    }
    let body_format = arg(a, "body_format").unwrap_or("text");
    let outputs: Vec<String> = ids
        .iter()
        .map(|t| {
            if thread_messages(ctx, t).is_empty() {
                format!(
                    "⚠️ Thread {t}: {}\n",
                    crate::util::http_error(404, &format!("https://gmail.googleapis.com/gmail/v1/users/me/threads/{t}?format=full&alt=json"), "Requested entity was not found.", "notFound")
                )
            } else {
                format_thread(ctx, t, body_format)
            }
        })
        .collect();
    Ok(format!(
        "Retrieved {} threads:\n\n{}",
        ids.len(),
        outputs.join("\n---\n\n")
    ))
}

pub fn get_attachment(ctx: &mut Ctx, a: &Value) -> R {
    let mid = s(a, "message_id").to_owned();
    let aid = s(a, "attachment_id").to_owned();
    let m = need_message(ctx, "get_gmail_attachment_content", &mid)?;
    let atts = m["attachments"].as_array().cloned().unwrap_or_default();
    let found = atts
        .iter()
        .find(|x| s(x, "attachmentId") == aid)
        .cloned()
        .or_else(|| arg_i64(a, "attachment_index").and_then(|i| atts.get(i as usize).cloned()))
        .or_else(|| {
            if atts.len() == 1 {
                atts.first().cloned()
            } else {
                None
            }
        });
    let Some(att) = found else {
        return Ok(format!(
            "Error: Failed to download attachment. The attachment ID may have changed.\nPlease fetch the message content again to get an updated attachment ID.\n\nError details: {}",
            crate::util::http_error(
                400,
                &format!(
                    "https://gmail.googleapis.com/gmail/v1/users/me/messages/{mid}/attachments/{aid}?alt=json"
                ),
                "Invalid attachment token",
                "invalidArgument"
            )
        ));
    };
    let size = att["size"].as_i64().unwrap_or(0);
    let name = s(&att, "filename");
    let saved = format!("{}_{name}", ctx.hex_id("attachment_file", 12));
    let mut lines = vec![
        "Attachment downloaded successfully!".to_owned(),
        format!("Message ID: {mid}"),
        format!("Filename: {name}"),
        format!("Saved filename: {saved}"),
        format!("Size: {:.1} KB ({size} bytes)", size as f64 / 1024.0),
        format!("\n📎 Saved to: /home/app/.workspace-mcp/attachments/{saved}"),
        "\nThe file has been saved to disk and can be accessed directly via the file path.".into(),
        "\nNote: Attachment IDs are ephemeral. Always use IDs from the most recent message fetch."
            .into(),
    ];
    if arg_bool(a, "return_base64").unwrap_or(false) {
        let data = s(&att, "data").replace('-', "+").replace('_', "/");
        lines.push("\n--- BASE64 CONTENT (standard alphabet) ---".into());
        lines.push(data);
        lines.push("--- END BASE64 CONTENT ---".into());
    }
    Ok(lines.join("\n"))
}

// ───────────────────────────── sending & drafts ─────────────────────────────

fn sender(ctx: &Ctx, a: &Value) -> String {
    let email = arg(a, "from_email").unwrap_or(USER).to_owned();
    let settings = ctx.get("gmail_settings", "settings").unwrap_or(Value::Null);
    let name = arg(a, "from_name")
        .map(str::to_owned)
        .or_else(|| {
            (arg_bool(a, "include_signature").unwrap_or(true))
                .then(|| s(&settings, "displayName").to_owned())
        })
        .filter(|n| !n.is_empty());
    match name {
        Some(n) => format!("{n} <{email}>"),
        None => email,
    }
}

fn signature(ctx: &Ctx, a: &Value) -> String {
    if !arg_bool(a, "include_signature").unwrap_or(true) {
        return String::new();
    }
    ctx.get("gmail_settings", "settings")
        .map(|v| s(&v, "signature").to_owned())
        .unwrap_or_default()
}

struct Built {
    headers: Map<String, Value>,
    body: String,
    thread: Option<String>,
}

fn build_message(ctx: &mut Ctx, a: &Value, tool: &str, is_draft: bool) -> Result<Built, String> {
    let body_format = arg(a, "body_format").unwrap_or("plain");
    let mut body = a.get("body").and_then(Value::as_str).map(str::to_owned);
    if body_format == "html"
        && body
            .as_deref()
            .is_some_and(|b| b.contains("&lt;") && !b.contains('<'))
    {
        return Err("Input error: body looks entity-escaped ('&lt;div&gt;'); with body_format='html' write real markup.".into());
    }
    let mut subject = arg(a, "subject").map(str::to_owned);
    let mut to = arg(a, "to").map(str::to_owned);
    let mut cc = arg(a, "cc").map(str::to_owned);
    let thread_id = arg(a, "thread_id").map(str::to_owned);
    let mut headers = Map::new();
    if let Some(fwd) = arg(a, "forward_message_id") {
        if !is_draft && to.is_none() {
            return Err("'to' is required when forwarding via 'forward_message_id'.".into());
        }
        let orig = need_message(ctx, tool, fwd)?;
        subject = subject.or_else(|| Some(format!("Fwd: {}", header(&orig, "Subject"))));
        let note = body.unwrap_or_default();
        body = Some(format!(
            "{note}\n\n---------- Forwarded message ---------\nFrom: {}\nDate: {}\nSubject: {}\nTo: {}\n\n{}",
            header(&orig, "From"),
            header(&orig, "Date"),
            header(&orig, "Subject"),
            header(&orig, "To"),
            s(&orig, "body_text")
        ));
        let mut built = Built {
            headers,
            body: body.unwrap_or_default(),
            thread: None,
        };
        fill_headers(
            ctx,
            a,
            &mut built.headers,
            to.as_deref().unwrap_or(""),
            cc.as_deref(),
            subject.as_deref().unwrap_or(""),
        );
        return Ok(built);
    }
    if !is_draft {
        if body.is_none()
            || (thread_id.is_none() && subject.as_deref().is_none_or(|x| x.trim().is_empty()))
        {
            return Err("'body' is required, and 'subject' is required unless replying with thread_id (it is then inherited from the parent message) or forwarding via 'forward_message_id'.".into());
        }
        if arg_bool(a, "reply_all").unwrap_or(false) && thread_id.is_none() {
            return Err("'reply_all' requires a thread_id: the recipients are derived from the message being replied to.".into());
        }
        if to.is_none() && !(thread_id.is_some() && arg_bool(a, "reply_all").unwrap_or(false)) {
            return Err("'to' is required unless replying with reply_all=True and a thread_id, which derives the recipients from the thread.".into());
        }
    } else if body.is_none() {
        return Err("'body' is required unless forwarding via 'forward_message_id'.".into());
    }
    let mut body = body.unwrap_or_default();
    if let Some(tid) = &thread_id {
        let msgs: Vec<Value> = thread_messages(ctx, tid)
            .into_iter()
            .filter(|m| {
                !labels(m).iter().any(|l| l == "DRAFT" || l == "TRASH")
                    && !header(m, "Message-ID").is_empty()
            })
            .collect();
        let target = match arg(a, "in_reply_to") {
            Some(irt) => msgs
                .iter()
                .find(|m| header(m, "Message-ID") == irt)
                .cloned(),
            None => msgs.last().cloned(),
        };
        if msgs.is_empty() && thread_messages(ctx, tid).is_empty() {
            return Err(not_found_error(
                tool,
                &format!(
                    "https://gmail.googleapis.com/gmail/v1/users/me/threads/{tid}?format=metadata&alt=json"
                ),
            ));
        }
        if let Some(t) = &target {
            let irt = header(t, "Message-ID").to_owned();
            let refs = match header(t, "References") {
                "" => irt.clone(),
                r => format!("{r} {irt}"),
            };
            headers.insert(
                "In-Reply-To".into(),
                json!(arg(a, "in_reply_to").unwrap_or(&irt)),
            );
            headers.insert(
                "References".into(),
                json!(arg(a, "references").map(str::to_owned).unwrap_or(refs)),
            );
            if subject.as_deref().is_none_or(|x| x.trim().is_empty()) {
                let parent = header(t, "Subject");
                subject = Some(if parent.to_lowercase().starts_with("re:") {
                    parent.to_owned()
                } else {
                    format!("Re: {parent}")
                });
            }
            if arg_bool(a, "reply_all").unwrap_or(false) {
                let from_addr = header(t, "From").to_owned();
                if to.is_none() {
                    to = Some(from_addr.clone());
                }
                if cc.is_none() {
                    let others: Vec<String> = format!("{},{}", header(t, "To"), header(t, "Cc"))
                        .split(',')
                        .map(|x| x.trim().to_owned())
                        .filter(|x| !x.is_empty() && !x.contains(USER))
                        .collect();
                    if !others.is_empty() {
                        cc = Some(others.join(", "));
                    }
                }
            }
            if arg_bool(a, "quote_original").unwrap_or(false) {
                let quoted: Vec<String> = s(t, "body_text")
                    .lines()
                    .map(|l| format!("> {l}"))
                    .collect();
                body = format!(
                    "{body}\n\nOn {}, {} wrote:\n{}",
                    header(t, "Date"),
                    header(t, "From"),
                    quoted.join("\n")
                );
            }
        } else if arg_bool(a, "reply_all").unwrap_or(false) {
            return Err(format!(
                "Could not load the message to reply to in thread '{tid}', so reply-all recipients cannot be derived. Retry, or pass 'to' and 'cc' explicitly without 'reply_all'."
            ));
        }
    }
    let sig = signature(ctx, a);
    if !sig.is_empty() {
        body = format!("{body}\n\n-- \n{sig}");
    }
    fill_headers(
        ctx,
        a,
        &mut headers,
        to.as_deref().unwrap_or(""),
        cc.as_deref(),
        subject.as_deref().unwrap_or(""),
    );
    Ok(Built {
        headers,
        body,
        thread: thread_id,
    })
}

fn fill_headers(
    ctx: &mut Ctx,
    a: &Value,
    h: &mut Map<String, Value>,
    to: &str,
    cc: Option<&str>,
    subject: &str,
) {
    h.insert("From".into(), json!(sender(ctx, a)));
    if !to.is_empty() {
        h.insert("To".into(), json!(to));
    }
    if let Some(c) = cc {
        h.insert("Cc".into(), json!(c));
    }
    if let Some(b) = arg(a, "bcc") {
        h.insert("Bcc".into(), json!(b));
    }
    h.insert("Subject".into(), json!(subject));
    h.insert("Date".into(), json!(rfc2822(ctx.now, &user_zone(ctx))));
    let mid = ctx.hex_id("rfc_message_id", 24);
    h.insert(
        "Message-ID".into(),
        json!(format!("<CA+{mid}@mail.gmail.com>")),
    );
}

fn store_message(ctx: &mut Ctx, built: Built, label: &str, a: &Value) -> Value {
    let id = ctx.hex_id("message", 16);
    let thread = built.thread.clone().unwrap_or_else(|| id.clone());
    let attachments: Vec<Value> = a
        .get("attachments")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .enumerate()
                .map(|(i, x)| {
                    let name = s(x, "filename");
                    let name = if name.is_empty() { s(x, "path").rsplit('/').next().unwrap_or("attachment") } else { name };
                    let data = s(x, "content");
                    json!({"attachmentId": format!("ANGjdJ_{id}_{i}"), "filename": name, "mimeType": x.get("mime_type").and_then(Value::as_str).unwrap_or("application/octet-stream"), "size": data.len() * 3 / 4, "data": data})
                })
                .collect()
        })
        .unwrap_or_default();
    let html = arg(a, "body_format") == Some("html");
    let msg = json!({
        "id": id, "threadId": thread, "labelIds": [label], "internalDate": (ctx.now * 1000).to_string(),
        "headers": built.headers,
        "body_text": if html { html_to_text(&built.body) } else { built.body.clone() },
        "body_html": if html { json!(built.body) } else { Value::Null },
        "attachments": attachments, "sizeEstimate": built.body.len() + 600,
    });
    ctx.put("gmail_messages", msg.clone());
    msg
}

pub fn send(ctx: &mut Ctx, a: &Value) -> R {
    let built = build_message(ctx, a, "send_gmail_message", false)?;
    let requested = a
        .get("attachments")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let msg = store_message(ctx, built, "SENT", a);
    if requested > 0 {
        return Ok(format!(
            "Email sent with {requested} attachment(s)! Message ID: {}",
            s(&msg, "id")
        ));
    }
    Ok(format!("Email sent! Message ID: {}", s(&msg, "id")))
}

pub fn draft(ctx: &mut Ctx, a: &Value) -> R {
    let built = build_message(ctx, a, "draft_gmail_message", true)?;
    let requested = a
        .get("attachments")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let msg = store_message(ctx, built, "DRAFT", a);
    let n = ctx.st.ids.next_int("draft", 5_367_123_405_329_023_114);
    let did = format!("r{n}");
    ctx.put(
        "gmail_drafts",
        json!({"id": did, "message_id": s(&msg, "id")}),
    );
    if requested > 0 {
        return Ok(format!(
            "Draft created with {requested} attachment(s)! Draft ID: {did}"
        ));
    }
    Ok(format!("Draft created! Draft ID: {did}"))
}

// ───────────────────────────── labels & filters ─────────────────────────────

fn all_labels(ctx: &Ctx) -> Vec<Value> {
    let mut out: Vec<Value> = SYSTEM_LABELS
        .iter()
        .map(|l| json!({"id": l, "name": l, "type": "system"}))
        .collect();
    out.extend(
        ctx.all("gmail_labels")
            .into_iter()
            .filter(|l| s(l, "type") != "system"),
    );
    out
}

pub fn list_labels(ctx: &mut Ctx, a: &Value) -> R {
    let mut ls = all_labels(ctx);
    let include_system = arg_bool(a, "include_system").unwrap_or(true);
    if !include_system {
        ls.retain(|l| s(l, "type") != "system");
    }
    if let Some(p) = a.get("prefix").and_then(Value::as_str) {
        ls.retain(|l| s(l, "name").starts_with(p));
    }
    if arg_bool(a, "compact").unwrap_or(false) {
        let mut items: Vec<Value> = ls
            .iter()
            .map(|l| json!({"id": s(l, "id"), "name": s(l, "name")}))
            .collect();
        items.sort_by(|x, y| s(x, "name").cmp(s(y, "name")));
        return Ok(
            serde_json::to_string(&json!({"count": items.len(), "labels": items}))
                .expect("JSON")
                .replace("\":", "\": ")
                .replace(",\"", ", \""),
        );
    }
    if ls.is_empty() {
        let mut parts = vec!["No"];
        if !include_system {
            parts.push("user");
        }
        parts.push("labels found");
        let mut msg = parts.join(" ");
        if let Some(p) = a.get("prefix").and_then(Value::as_str) {
            msg.push_str(&format!(" with prefix {}", crate::util::py_str(p)));
        }
        return Ok(format!("{msg}."));
    }
    let mut lines = vec![format!("Found {} labels:", ls.len()), String::new()];
    let system: Vec<&Value> = ls.iter().filter(|l| s(l, "type") == "system").collect();
    let user: Vec<&Value> = ls.iter().filter(|l| s(l, "type") != "system").collect();
    if !system.is_empty() {
        lines.push("📂 SYSTEM LABELS:".into());
        for l in system {
            lines.push(format!("  • {} (ID: {})", s(l, "name"), s(l, "id")));
        }
        lines.push(String::new());
    }
    if !user.is_empty() {
        lines.push("🏷️  USER LABELS:".into());
        for l in user {
            lines.push(format!("  • {} (ID: {})", s(l, "name"), s(l, "id")));
        }
    }
    Ok(lines.join("\n"))
}

pub fn manage_label(ctx: &mut Ctx, a: &Value) -> R {
    let action = s(a, "action");
    let url = "https://gmail.googleapis.com/gmail/v1/users/me/labels";
    match action {
        "create" => {
            let name = arg(a, "name")
                .ok_or_else(|| "Label name is required for create action.".to_owned())?
                .to_owned();
            if all_labels(ctx)
                .iter()
                .any(|l| s(l, "name").eq_ignore_ascii_case(&name))
            {
                return Err(format!(
                    "API error in manage_gmail_label: {}",
                    crate::util::http_error(
                        409,
                        &format!("{url}?alt=json"),
                        "Label name exists or conflicts",
                        "failedPrecondition"
                    )
                ));
            }
            let n = ctx
                .all("gmail_labels")
                .iter()
                .filter_map(|l| {
                    s(l, "id")
                        .strip_prefix("Label_")
                        .and_then(|x| x.parse::<i64>().ok())
                })
                .max()
                .unwrap_or(0)
                + 1;
            let id = format!("Label_{n}");
            let mut label = json!({"id": id, "name": name, "type": "user", "labelListVisibility": arg(a, "label_list_visibility").unwrap_or("labelShow"), "messageListVisibility": arg(a, "message_list_visibility").unwrap_or("show")});
            if let (Some(bg), Some(fg)) = (arg(a, "background_color"), arg(a, "text_color")) {
                label["color"] = json!({"backgroundColor": bg, "textColor": fg});
            }
            ctx.put("gmail_labels", label);
            Ok(format!(
                "Label created successfully!\nName: {name}\nID: {id}"
            ))
        }
        "update" | "delete" => {
            let id = arg(a, "label_id")
                .ok_or_else(|| "Label ID is required for update and delete actions.".to_owned())?
                .to_owned();
            let mut label = ctx.get("gmail_labels", &id).ok_or_else(|| {
                not_found_error("manage_gmail_label", &format!("{url}/{id}?alt=json"))
            })?;
            if action == "delete" {
                let name = s(&label, "name").to_owned();
                ctx.remove("gmail_labels", &id);
                for mut m in ctx.all("gmail_messages") {
                    let before = labels(&m);
                    if before.contains(&id) {
                        m["labelIds"] =
                            json!(before.into_iter().filter(|l| *l != id).collect::<Vec<_>>());
                        ctx.put("gmail_messages", m);
                    }
                }
                return Ok(format!("Label '{name}' (ID: {id}) deleted successfully!"));
            }
            if let Some(n) = arg(a, "name") {
                label["name"] = json!(n);
            }
            for (k, f) in [
                ("label_list_visibility", "labelListVisibility"),
                ("message_list_visibility", "messageListVisibility"),
            ] {
                if let Some(v) = arg(a, k) {
                    label[f] = json!(v);
                }
            }
            if arg_bool(a, "clear_color").unwrap_or(false) {
                label.as_object_mut().expect("label").remove("color");
            } else if let (Some(bg), Some(fg)) = (arg(a, "background_color"), arg(a, "text_color"))
            {
                label["color"] = json!({"backgroundColor": bg, "textColor": fg});
            }
            ctx.put("gmail_labels", label.clone());
            Ok(format!(
                "Label updated successfully!\nName: {}\nID: {id}",
                s(&label, "name")
            ))
        }
        _ => Err(format!(
            "Input should be 'create', 'update' or 'delete' (got '{action}')"
        )),
    }
}

fn py_dict(v: &Value) -> String {
    if v.as_object().is_none_or(|o| o.is_empty()) {
        "(none)".into()
    } else {
        crate::util::py_repr(v)
    }
}

pub fn list_filters(ctx: &mut Ctx, _a: &Value) -> R {
    let filters = ctx.all("gmail_filters");
    if filters.is_empty() {
        return Ok("No filters found.".into());
    }
    let mut lines = vec![format!("Found {} filters:", filters.len()), String::new()];
    for f in &filters {
        lines.push(format!("🔹 Filter ID: {}", s(f, "id")));
        lines.push("  Criteria:".into());
        let c = &f["criteria"];
        let mut cl = Vec::new();
        for (k, label) in [
            ("from", "From"),
            ("to", "To"),
            ("subject", "Subject"),
            ("query", "Query"),
            ("negatedQuery", "Exclude Query"),
        ] {
            if !s(c, k).is_empty() {
                cl.push(format!("{label}: {}", s(c, k)));
            }
        }
        if c["hasAttachment"].as_bool().unwrap_or(false) {
            cl.push("Has attachment".into());
        }
        if cl.is_empty() {
            cl.push("(none)".into());
        }
        lines.extend(cl.iter().map(|l| format!("    • {l}")));
        lines.push("  Actions:".into());
        let ac = &f["action"];
        let mut al = Vec::new();
        if !s(ac, "forward").is_empty() {
            al.push(format!("Forward to: {}", s(ac, "forward")));
        }
        for (k, label) in [
            ("removeLabelIds", "Remove labels"),
            ("addLabelIds", "Add labels"),
        ] {
            if let Some(ids) = ac[k].as_array().filter(|x| !x.is_empty()) {
                al.push(format!(
                    "{label}: {}",
                    ids.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
        if al.is_empty() {
            al.push("(none)".into());
        }
        lines.extend(al.iter().map(|l| format!("    • {l}")));
        lines.push(String::new());
    }
    Ok(lines.join("\n").trim_end().to_owned())
}

pub fn manage_filter(ctx: &mut Ctx, a: &Value) -> R {
    let action = s(a, "action").trim().to_lowercase();
    let criteria = arg_json(a, "criteria");
    let filter_action = arg_json(a, "filter_action");
    let url = "https://gmail.googleapis.com/gmail/v1/users/me/settings/filters";
    match action.as_str() {
        "create" => {
            let (Some(c), Some(fa)) = (
                criteria.filter(|c| c.as_object().is_some_and(|o| !o.is_empty())),
                filter_action.filter(|f| f.as_object().is_some_and(|o| !o.is_empty())),
            ) else {
                return Err("criteria and filter_action are required for create action".into());
            };
            let id = format!("ANe1Bm{}", ctx.hex_id("filter", 20));
            ctx.put(
                "gmail_filters",
                json!({"id": id, "criteria": c, "action": fa}),
            );
            Ok(format!("Filter created successfully!\nFilter ID: {id}"))
        }
        "delete" => {
            let id = arg(a, "filter_id")
                .ok_or_else(|| "filter_id is required for delete action".to_owned())?
                .to_owned();
            let f = ctx.get("gmail_filters", &id).ok_or_else(|| {
                not_found_error("manage_gmail_filter", &format!("{url}/{id}?alt=json"))
            })?;
            ctx.remove("gmail_filters", &id);
            Ok(format!(
                "Filter deleted successfully!\nFilter ID: {id}\nCriteria: {}\nAction: {}",
                py_dict(&f["criteria"]),
                py_dict(&f["action"])
            ))
        }
        "update" => {
            let id = arg(a, "filter_id")
                .ok_or_else(|| "filter_id is required for update action".to_owned())?
                .to_owned();
            if criteria.is_none() && filter_action.is_none() {
                return Err("criteria and/or filter_action are required for update action".into());
            }
            let f = ctx.get("gmail_filters", &id).ok_or_else(|| {
                not_found_error("manage_gmail_filter", &format!("{url}/{id}?alt=json"))
            })?;
            let new_id = format!("ANe1Bm{}", ctx.hex_id("filter", 20));
            let nf = json!({"id": new_id, "criteria": criteria.unwrap_or(f["criteria"].clone()), "action": filter_action.unwrap_or(f["action"].clone())});
            ctx.remove("gmail_filters", &id);
            ctx.put("gmail_filters", nf.clone());
            Ok(format!(
                "Filter updated successfully (Gmail recreates filters on update).\nOld filter ID: {id} (deleted)\nNew filter ID: {new_id}\nCriteria: {}\nAction: {}",
                py_dict(&nf["criteria"]),
                py_dict(&nf["action"])
            ))
        }
        "apply" => {
            let (c, fa) = match arg(a, "filter_id") {
                Some(id) => {
                    let f = ctx.get("gmail_filters", id).ok_or_else(|| {
                        not_found_error("manage_gmail_filter", &format!("{url}/{id}?alt=json"))
                    })?;
                    (f["criteria"].clone(), f["action"].clone())
                }
                None => match (criteria, filter_action) {
                    (Some(c), Some(f)) => (c, f),
                    _ => {
                        return Err(
                            "apply needs filter_id, or both criteria and filter_action".into()
                        );
                    }
                },
            };
            let mut q = Vec::new();
            for (k, op) in [("from", "from:"), ("to", "to:"), ("subject", "subject:")] {
                if !s(&c, k).is_empty() {
                    q.push(format!("{op}({})", s(&c, k)));
                }
            }
            if !s(&c, "query").is_empty() {
                q.push(s(&c, "query").to_owned());
            }
            if c["hasAttachment"].as_bool().unwrap_or(false) {
                q.push("has:attachment".into());
            }
            let query = q.join(" ");
            let matches = search(ctx, &query);
            let dry = arg_bool(a, "dry_run").unwrap_or(false);
            if !dry {
                let add: Vec<String> = fa["addLabelIds"]
                    .as_array()
                    .map(|x| {
                        x.iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                let rem: Vec<String> = fa["removeLabelIds"]
                    .as_array()
                    .map(|x| {
                        x.iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                for mut m in matches.clone() {
                    let mut ls = labels(&m);
                    ls.retain(|l| !rem.contains(l));
                    for l in &add {
                        if !ls.contains(l) {
                            ls.push(l.clone());
                        }
                    }
                    m["labelIds"] = json!(ls);
                    ctx.put("gmail_messages", m);
                }
            }
            Ok(format!(
                "{}Search query: {query}\nMatching messages: {}{}",
                if dry {
                    "DRY RUN - no messages were changed.\n"
                } else {
                    "Filter applied to existing mail.\n"
                },
                matches.len(),
                if dry {
                    String::new()
                } else {
                    format!("\nMessages changed: {}", matches.len())
                }
            ))
        }
        other => Err(format!(
            "Invalid action '{other}'. Must be 'create', 'delete', 'update' or 'apply'."
        )),
    }
}

fn check_labels(ctx: &Ctx, tool: &str, ids: &[String], url: &str) -> Result<(), String> {
    let known: Vec<String> = all_labels(ctx)
        .iter()
        .map(|l| s(l, "id").to_owned())
        .chain(["UNREAD".to_owned()])
        .collect();
    for l in ids {
        if !known.contains(l) {
            return Err(crate::util::bad_request(
                tool,
                url,
                &format!("Invalid label: {l}"),
            ));
        }
    }
    Ok(())
}

fn apply_labels(m: &mut Value, add: &[String], rem: &[String]) {
    let mut ls = labels(m);
    ls.retain(|l| !rem.contains(l));
    for l in add {
        if !ls.contains(l) {
            ls.push(l.clone());
        }
    }
    m["labelIds"] = json!(ls);
}

pub fn modify_labels(ctx: &mut Ctx, a: &Value) -> R {
    let id = s(a, "message_id").to_owned();
    let add = arg_list(a, "add_label_ids").unwrap_or_default();
    let rem = arg_list(a, "remove_label_ids").unwrap_or_default();
    if add.is_empty() && rem.is_empty() {
        return Err("At least one of add_label_ids or remove_label_ids must be provided.".into());
    }
    let url =
        format!("https://gmail.googleapis.com/gmail/v1/users/me/messages/{id}/modify?alt=json");
    let mut m = ctx
        .get("gmail_messages", &id)
        .ok_or_else(|| not_found_error("modify_gmail_message_labels", &url))?;
    check_labels(ctx, "modify_gmail_message_labels", &add, &url)?;
    apply_labels(&mut m, &add, &rem);
    ctx.put("gmail_messages", m);
    let mut actions = Vec::new();
    if !add.is_empty() {
        actions.push(format!("Added labels: {}", add.join(", ")));
    }
    if !rem.is_empty() {
        actions.push(format!("Removed labels: {}", rem.join(", ")));
    }
    Ok(format!(
        "Message labels updated successfully!\nMessage ID: {id}\n{}",
        actions.join("; ")
    ))
}

pub fn batch_modify(ctx: &mut Ctx, a: &Value) -> R {
    let ids = arg_list(a, "message_ids").unwrap_or_default();
    let threads = arg_list(a, "thread_ids").unwrap_or_default();
    let add = arg_list(a, "add_label_ids").unwrap_or_default();
    let rem = arg_list(a, "remove_label_ids").unwrap_or_default();
    if add.is_empty() && rem.is_empty() {
        return Err("At least one of add_label_ids or remove_label_ids must be provided.".into());
    }
    if ids.is_empty() && threads.is_empty() {
        return Err("At least one of message_ids or thread_ids must be provided.".into());
    }
    check_labels(
        ctx,
        "batch_modify_gmail_message_labels",
        &add,
        "https://gmail.googleapis.com/gmail/v1/users/me/messages/batchModify?alt=json",
    )?;
    let mut actions = Vec::new();
    if !add.is_empty() {
        actions.push(format!("Added labels: {}", add.join(", ")));
    }
    if !rem.is_empty() {
        actions.push(format!("Removed labels: {}", rem.join(", ")));
    }
    let summary = actions.join("; ");
    let mut thread_report = String::new();
    if !threads.is_empty() {
        let mut lines = vec![format!(
            "Label changes for {} thread ID(s): {summary}",
            threads.len()
        )];
        for t in &threads {
            let msgs = thread_messages(ctx, t);
            if msgs.is_empty() {
                lines.push(format!("  {t}: not found"));
                continue;
            }
            for mut m in msgs.clone() {
                apply_labels(&mut m, &add, &rem);
                ctx.put("gmail_messages", m);
            }
            lines.push(format!("  {t}: applied ({} message(s))", msgs.len()));
        }
        thread_report = lines.join("\n");
        if ids.is_empty() {
            return Ok(thread_report);
        }
    }
    let mut applied = 0;
    let mut missing = Vec::new();
    for id in &ids {
        match ctx.get("gmail_messages", id) {
            Some(mut m) => {
                apply_labels(&mut m, &add, &rem);
                ctx.put("gmail_messages", m);
                applied += 1;
            }
            None => missing.push(id.clone()),
        }
    }
    let total = ids.len();
    if !arg_bool(a, "verify").unwrap_or(true) {
        let mut out = format!(
            "Requested label changes for {total} message ID(s): {summary}\nNOT VERIFIED: Gmail's batchModify returns no per-message result and silently ignores IDs it does not recognise, so this records what was asked for, not what changed. Re-run with verify=True to confirm."
        );
        if !thread_report.is_empty() {
            out.push_str(&format!("\n\n{thread_report}"));
        }
        return Ok(out);
    }
    let mut lines = vec![
        format!("Label changes for {total} message ID(s): {summary}"),
        format!("Applied: {applied}/{total}"),
    ];
    if !missing.is_empty() {
        lines.push(format!(
            "No such message ({}): {}\n  These IDs do not currently resolve to messages in this mailbox, so the requested change could not be verified. One possible cause is passing a THREAD id where a MESSAGE id is required.",
            missing.len(),
            missing.join(", ")
        ));
    }
    if !thread_report.is_empty() {
        lines.push(String::new());
        lines.push(thread_report);
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_queries() {
        assert_eq!(
            tokenize("from:alice subject:(budget review) \"q3 plan\" -is:read"),
            [
                "from:alice",
                "subject:(budget review)",
                "\"q3 plan\"",
                "-is:read"
            ]
        );
        assert_eq!(tokenize("(a OR b) c"), ["(", "a", "OR", "b", ")", "c"]);
    }
}
