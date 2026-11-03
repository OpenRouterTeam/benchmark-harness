//! `slack_search_*`: keyword + modifier search over messages, files, channels and users.

use serde_json::{Value, json};

use crate::{
    model::{CALLER, Ctx, display_name, kind, real_name, ts_key, ts_secs},
    tools::{ToolResult, err},
    tz,
    util::{
        arg_bool, arg_i64, arg_str, arg_strings, b, cursor, human_size, parse_cursor, s, truncate,
    },
};

const HEADER: &str = "# Search Results for: \n\n";
const END: &str = "End of results - No more pages available.\\n";

fn no_results() -> Value {
    json!({"results": format!("{HEADER}No results found.\n"), "pagination_info": END})
}

fn page_info(page: usize, more: bool) -> String {
    if more {
        format!(
            "For the next page of results use cursor `{}`\n",
            cursor("CURRENT_PAGE", page + 1)
        )
    } else {
        END.to_owned()
    }
}

/// A keyword term: lowercase text, exclusion flag.
struct Term {
    text: String,
    negate: bool,
}

fn terms(keywords: &[String]) -> Vec<Term> {
    let mut out = Vec::new();
    for k in keywords {
        let k = k.trim();
        let (negate, rest) = match k.strip_prefix('-') {
            Some(r) if !r.is_empty() => (true, r),
            _ => (false, k),
        };
        let text = rest.trim_matches('"').trim().to_lowercase();
        if !text.is_empty() {
            out.push(Term { text, negate });
        }
    }
    out
}

fn matches_terms(haystack: &str, terms: &[Term]) -> bool {
    let h = haystack.to_lowercase();
    terms.iter().all(|t| h.contains(&t.text) != t.negate)
}

fn score(haystack: &str, terms: &[Term], nlq: &str) -> usize {
    let h = haystack.to_lowercase();
    let mut n: usize = terms
        .iter()
        .filter(|t| !t.negate)
        .map(|t| h.matches(&t.text).count() * 3)
        .sum();
    for w in nlq.split(|c: char| !c.is_alphanumeric()) {
        if w.len() >= 4 && h.contains(&w.to_lowercase()) {
            n += 1;
        }
    }
    n
}

#[derive(Default)]
struct Filters {
    in_: Vec<String>,
    from: Vec<String>,
    with: Vec<String>,
    has: Vec<String>,
    is: Vec<String>,
    before: Option<i64>,
    after: Option<i64>,
    on: Vec<(i64, i64)>,
    creator: Vec<String>,
}

fn strip_ref(v: &str) -> String {
    // <#C123|name>, <@U123|name>, <#C123>, #name, @name
    let v = v.trim();
    if let Some(inner) = v.strip_prefix('<').and_then(|x| x.strip_suffix('>')) {
        let inner = inner.trim_start_matches(['#', '@']);
        return inner.split('|').next().unwrap_or("").to_owned();
    }
    v.trim_start_matches(['#', '@']).to_owned()
}

fn day_bounds(ctx: &Ctx, date: &str) -> Option<(i64, i64)> {
    let (y, m, d) = tz::parse_date(date)?;
    let zone = ctx.caller_tz();
    let start_utc = tz::days_from_civil(y, m, d) * 86_400;
    let (off, _) = tz::offset_at(&zone, start_utc);
    let start = start_utc - off;
    Some((start, start + 86_400))
}

fn month_bounds(ctx: &Ctx, v: &str) -> Option<(i64, i64)> {
    let parts: Vec<&str> = v.split(['-', '/']).collect();
    if parts.len() == 2 {
        let y: i64 = parts[0].parse().ok()?;
        let m: i64 = parts[1].parse().ok()?;
        let (s1, _) = day_bounds(ctx, &format!("{y}-{m:02}-01"))?;
        let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
        let (s2, _) = day_bounds(ctx, &format!("{ny}-{nm:02}-01"))?;
        return Some((s1, s2));
    }
    day_bounds(ctx, v)
}

fn parse_filters(ctx: &Ctx, text: &str) -> Result<Filters, String> {
    let mut f = Filters::default();
    for token in text.split_whitespace() {
        let Some((key, value)) = token.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.to_ascii_lowercase().as_str() {
            "in" => f.in_.push(value.to_owned()),
            "from" => f.from.push(strip_ref(value)),
            "with" => f.with.push(strip_ref(value)),
            "creator" => f.creator.push(strip_ref(value)),
            "has" => f.has.push(value.to_owned()),
            "hasmy" => f.has.push(format!("my{value}")),
            "is" => f.is.push(value.to_ascii_lowercase()),
            "before" => {
                let (start, _) =
                    day_bounds(ctx, value).ok_or_else(|| format!("invalid date: {value}"))?;
                f.before = Some(f.before.map_or(start, |b: i64| b.min(start)));
            }
            "after" => {
                let (_, end) =
                    day_bounds(ctx, value).ok_or_else(|| format!("invalid date: {value}"))?;
                f.after = Some(f.after.map_or(end, |a: i64| a.max(end)));
            }
            "on" => {
                f.on.push(day_bounds(ctx, value).ok_or_else(|| format!("invalid date: {value}"))?)
            }
            "during" => {
                f.on.push(month_bounds(ctx, value).ok_or_else(|| format!("invalid date: {value}"))?)
            }
            _ => {}
        }
    }
    Ok(f)
}

/// Users matching a `from:`/`with:` reference (ID, username, display or real name, email).
fn user_matches(ctx: &Ctx, user_id: &str, reference: &str) -> bool {
    if user_id == reference {
        return true;
    }
    let Some(u) = ctx.user(user_id) else {
        return false;
    };
    let r = reference.to_lowercase();
    [
        s(&u, "name"),
        s(&u, "display_name"),
        s(&u, "real_name"),
        s(&u, "email"),
    ]
    .iter()
    .any(|x| !x.is_empty() && x.to_lowercase() == r)
        || (r == "me" && user_id == CALLER)
}

fn channel_matches(ctx: &Ctx, channel: &Value, reference: &str) -> bool {
    let r = reference.trim();
    // in:@user -> the DM with that user
    if r.starts_with('@') || r.starts_with("<@") {
        let who = strip_ref(r);
        return b(channel, "is_im") && user_matches(ctx, s(channel, "user"), &who);
    }
    let id_or_name = strip_ref(r);
    s(channel, "id") == id_or_name || s(channel, "name").eq_ignore_ascii_case(&id_or_name)
}

fn visible_types(args: &Value, private: bool) -> Vec<String> {
    if !private {
        return vec!["public_channel".into()];
    }
    match arg_str(args, "channel_types") {
        Some(t) => t.split(',').map(|x| x.trim().to_owned()).collect(),
        None => vec![
            "public_channel".into(),
            "private_channel".into(),
            "mpim".into(),
            "im".into(),
        ],
    }
}

/// `slack_search_public` and `slack_search_public_and_private`.
pub fn search_messages(ctx: &Ctx, args: &Value, private: bool) -> ToolResult {
    search(ctx, args, private, false)
}

/// Matching message documents in result order (Web API `search.messages`).
pub fn matching_messages(ctx: &Ctx, args: &Value) -> Result<Vec<Value>, String> {
    match search(ctx, args, true, true)? {
        Value::Array(a) => Ok(a),
        _ => Ok(Vec::new()),
    }
}

fn search(ctx: &Ctx, args: &Value, private: bool, raw: bool) -> ToolResult {
    let keywords = arg_strings(args, "keywords");
    let filters_text = arg_str(args, "filters").unwrap_or("");
    let nlq = arg_str(args, "natural_language_query").unwrap_or("");
    if keywords.is_empty() && filters_text.trim().is_empty() && nlq.trim().is_empty() {
        return Err(err(
            "invalid_arguments: at least one of `keywords` or `filters` is required",
        ));
    }
    let terms = terms(&keywords);
    let filters =
        parse_filters(ctx, filters_text).map_err(|e| err(&format!("invalid_arguments: {e}")))?;
    let types = visible_types(args, private);
    let only_mine = arg_bool(args, "only_my_channels").unwrap_or(false);
    let include_bots = arg_bool(args, "include_bots").unwrap_or(false);
    let after = arg_i64(args, "after");
    let before = arg_i64(args, "before");
    let concise = arg_str(args, "response_format") == Some("concise");
    let include_context = arg_bool(args, "include_context").unwrap_or(true) && !concise;
    let max_ctx = arg_i64(args, "max_context_length").unwrap_or(250).max(1) as usize;
    let limit = arg_i64(args, "limit").unwrap_or(20).clamp(1, 20) as usize;
    let page: usize = arg_str(args, "cursor")
        .and_then(|c| parse_cursor(c, "CURRENT_PAGE"))
        .and_then(|p| p.parse().ok())
        .unwrap_or(1)
        .max(1);
    let content_types = arg_str(args, "content_types").unwrap_or("messages,files");
    let want_messages = content_types.contains("message");
    let want_files = content_types.contains("file");

    let channel_ok = |c: &Value| -> bool {
        if !types.iter().any(|t| t == kind(c)) || !ctx.can_see(c) {
            return false;
        }
        if only_mine && !ctx.is_member(c, CALLER) {
            return false;
        }
        filters.in_.is_empty() || filters.in_.iter().any(|r| channel_matches(ctx, c, r))
    };
    let time_ok = |t: i64| -> bool {
        filters.before.is_none_or(|x| t < x)
            && filters.after.is_none_or(|x| t >= x)
            && (filters.on.is_empty() || filters.on.iter().any(|(a, z)| t >= *a && t < *z))
            && after.is_none_or(|x| t >= x)
            && before.is_none_or(|x| t <= x)
    };
    let nlq_terms: Vec<Term> = if keywords.is_empty() && filters_text.trim().is_empty() {
        nlq.split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() >= 4)
            .map(|w| Term {
                text: w.to_lowercase(),
                negate: false,
            })
            .collect()
    } else {
        Vec::new()
    };

    // (score, ts key, rendered)
    let mut hits: Vec<(usize, (i64, i64), Value)> = Vec::new();
    if want_messages {
        for m in ctx.values("messages") {
            let Some(channel) = ctx.channel(s(&m, "channel")) else {
                continue;
            };
            if !channel_ok(&channel) {
                continue;
            }
            let is_bot = !s(&m, "bot_id").is_empty() || s(&m, "subtype") == "bot_message";
            if is_bot && !include_bots {
                continue;
            }
            let text = s(&m, "text");
            if !matches_terms(text, &terms) {
                continue;
            }
            if !nlq_terms.is_empty()
                && !nlq_terms
                    .iter()
                    .any(|t| text.to_lowercase().contains(&t.text))
            {
                continue;
            }
            if !time_ok(ts_secs(s(&m, "ts"))) {
                continue;
            }
            if !filters.from.is_empty()
                && !filters
                    .from
                    .iter()
                    .any(|r| user_matches(ctx, s(&m, "user"), r))
            {
                continue;
            }
            if !message_has(ctx, &m, &filters) {
                continue;
            }
            hits.push((
                score(text, &terms, nlq),
                ts_key(s(&m, "ts")),
                json!({"kind": "message", "doc": m}),
            ));
        }
    }
    let mut file_hits: Vec<(usize, (i64, i64), Value)> = Vec::new();
    if want_files {
        for f in ctx
            .values("files")
            .into_iter()
            .chain(ctx.values("canvases"))
        {
            if s(&f, "upload_state") == "pending" {
                continue;
            }
            let channels: Vec<String> = f["channels"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let visible = s(&f, "user") == CALLER
                || channels
                    .iter()
                    .any(|c| ctx.channel(c).is_some_and(|c| channel_ok(&c)))
                || (channels.is_empty() && !b(&f, "is_private") && filters.in_.is_empty());
            if !visible {
                continue;
            }
            let hay = format!("{} {} {}", s(&f, "title"), s(&f, "name"), file_text(&f));
            if !matches_terms(&hay, &terms) {
                continue;
            }
            if !nlq_terms.is_empty()
                && !nlq_terms
                    .iter()
                    .any(|t| hay.to_lowercase().contains(&t.text))
            {
                continue;
            }
            let created = f.get("created").and_then(Value::as_i64).unwrap_or(0);
            if !time_ok(created) {
                continue;
            }
            let owner_refs: Vec<&String> =
                filters.from.iter().chain(filters.creator.iter()).collect();
            if !owner_refs.is_empty()
                && !owner_refs
                    .iter()
                    .any(|r| user_matches(ctx, s(&f, "user"), r))
            {
                continue;
            }
            file_hits.push((
                score(&hay, &terms, nlq),
                (created, 0),
                json!({"kind": "file", "doc": f}),
            ));
        }
    }

    let sort_ts = arg_str(args, "sort") == Some("timestamp");
    let asc = arg_str(args, "sort_dir") == Some("asc");
    let order = |v: &mut Vec<(usize, (i64, i64), Value)>| {
        if sort_ts {
            v.sort_by(|a, b| if asc { a.1.cmp(&b.1) } else { b.1.cmp(&a.1) });
        } else {
            v.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        }
    };
    order(&mut hits);
    order(&mut file_hits);
    if raw {
        return Ok(Value::Array(
            hits.into_iter().map(|h| h.2["doc"].clone()).collect(),
        ));
    }

    let start = (page - 1) * limit;
    let msg_page: Vec<_> = hits.iter().skip(start).take(limit).collect();
    let file_page: Vec<_> = file_hits.iter().skip(start).take(limit).collect();
    let more = hits.len() > start + limit || file_hits.len() > start + limit;
    if msg_page.is_empty() && file_page.is_empty() {
        return Ok(no_results());
    }
    let mut out = String::from(HEADER);
    if !msg_page.is_empty() {
        out.push_str(&format!("## Messages ({} results)\n", msg_page.len()));
        for (i, (_, _, hit)) in msg_page.iter().enumerate() {
            out.push_str(&format!("### Result {} of {}\n", i + 1, msg_page.len()));
            out.push_str(&render_message_hit(
                ctx,
                &hit["doc"],
                concise,
                include_context,
                max_ctx,
            ));
            out.push_str("\n---\n\n");
        }
    }
    if !file_page.is_empty() {
        if !msg_page.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("## Files ({} results)\n", file_page.len()));
        for (i, (_, _, hit)) in file_page.iter().enumerate() {
            out.push_str(&format!("### Result {} of {}\n", i + 1, file_page.len()));
            out.push_str(&render_file_hit(ctx, &hit["doc"], concise));
            out.push_str("\n---\n\n");
        }
    }
    Ok(json!({"results": out, "pagination_info": page_info(page, more)}))
}

fn file_text(f: &Value) -> String {
    if let Some(sections) = f["sections"].as_array() {
        return sections
            .iter()
            .map(|x| s(x, "markdown"))
            .collect::<Vec<_>>()
            .join("\n");
    }
    s(f, "content").to_owned()
}

fn message_has(ctx: &Ctx, m: &Value, f: &Filters) -> bool {
    for h in &f.has {
        let ok = match h.as_str() {
            "pin" => b(m, "pinned"),
            "link" => s(m, "text").contains("http://") || s(m, "text").contains("https://"),
            "file" => m["files"].as_array().is_some_and(|a| !a.is_empty()),
            "reaction" => m["reactions"].as_array().is_some_and(|a| !a.is_empty()),
            other => {
                let (mine, name) = match other.strip_prefix("my") {
                    Some(n) => (true, n),
                    None => (false, other),
                };
                let name = name.trim_matches(':');
                m["reactions"].as_array().is_some_and(|a| {
                    a.iter().any(|r| {
                        s(r, "name") == name
                            && (!mine
                                || r["users"]
                                    .as_array()
                                    .is_some_and(|u| u.iter().any(|x| x == CALLER)))
                    })
                })
            }
        };
        if !ok {
            return false;
        }
    }
    for i in &f.is {
        let ok = match i.as_str() {
            "thread" => {
                !s(m, "thread_ts").is_empty()
                    || !ctx.replies(s(m, "channel"), s(m, "ts")).is_empty()
            }
            "dm" => ctx
                .channel(s(m, "channel"))
                .is_some_and(|c| b(&c, "is_im") || b(&c, "is_mpim")),
            "saved" => b(m, "saved"),
            _ => true,
        };
        if !ok {
            return false;
        }
    }
    for w in &f.with {
        let channel = s(m, "channel");
        let thread = match s(m, "thread_ts") {
            "" => s(m, "ts"),
            t => t,
        };
        let in_thread = ctx.messages_in(channel).iter().any(|x| {
            (s(x, "ts") == thread || s(x, "thread_ts") == thread)
                && user_matches(ctx, s(x, "user"), w)
        });
        let in_dm = ctx.channel(channel).is_some_and(|c| {
            (b(&c, "is_im") || b(&c, "is_mpim"))
                && c["members"].as_array().is_some_and(|ms| {
                    ms.iter()
                        .any(|u| user_matches(ctx, u.as_str().unwrap_or(""), w))
                })
        });
        if !(in_thread && !ctx.replies(channel, thread).is_empty() || in_dm) {
            return false;
        }
    }
    true
}

fn from_line(ctx: &Ctx, user_id: &str) -> String {
    match ctx.user(user_id) {
        Some(u) => format!(
            "From: {} <{}> (ID: {user_id}) \n",
            real_name(&u),
            s(&u, "email")
        ),
        None => format!("From: {user_id} (ID: {user_id}) \n"),
    }
}

pub fn render_message_hit(
    ctx: &Ctx,
    m: &Value,
    concise: bool,
    context: bool,
    max_ctx: usize,
) -> String {
    let channel = ctx.channel(s(m, "channel")).unwrap_or(Value::Null);
    let ts = s(m, "ts");
    let mut out = format!(
        "Channel: {} (ID: {})\n",
        ctx.channel_label(&channel),
        s(m, "channel")
    );
    out.push_str(&from_line(ctx, s(m, "user")));
    out.push_str(&format!("Time: {}\n", ctx.time(ts)));
    out.push_str(&format!("Message_ts: {ts}\n"));
    let thread_ts = s(m, "thread_ts");
    if !thread_ts.is_empty() && thread_ts != ts {
        out.push_str(&format!("Thread_ts: {thread_ts}\n"));
    }
    let replies = ctx.replies(s(m, "channel"), ts);
    if !concise && !replies.is_empty() {
        out.push_str(&format!("Reply count: {}\n", replies.len()));
    }
    if !concise {
        out.push_str(&format!(
            "Permalink: [link]({})\n",
            ctx.message_permalink(m)
        ));
    }
    out.push_str(&format!("Text: \n{}\n", s(m, "text")));
    if context {
        let after: Vec<Value> = if !replies.is_empty() {
            replies.into_iter().take(10).collect()
        } else if thread_ts.is_empty() || thread_ts == ts {
            let top: Vec<Value> = ctx
                .messages_in(s(m, "channel"))
                .into_iter()
                .filter(|x| s(x, "thread_ts").is_empty() || s(x, "thread_ts") == s(x, "ts"))
                .collect();
            let pos = top.iter().position(|x| s(x, "ts") == ts).unwrap_or(0);
            top.into_iter().skip(pos + 1).take(2).collect()
        } else {
            ctx.replies(s(m, "channel"), thread_ts)
                .into_iter()
                .filter(|x| ts_key(s(x, "ts")) > ts_key(ts))
                .take(2)
                .collect()
        };
        if !after.is_empty() {
            out.push_str("Context after: \n");
            for x in after {
                out.push_str(&format!("- {}", from_line(ctx, s(&x, "user"))));
                out.push_str(&format!("  Message_ts: {}\n", s(&x, "ts")));
                out.push_str(&format!("  {}\n", truncate(s(&x, "text"), max_ctx)));
            }
        }
    }
    out
}

fn render_file_hit(ctx: &Ctx, f: &Value, concise: bool) -> String {
    let is_canvas = f.get("sections").is_some();
    let id = s(f, "id");
    let title = match s(f, "title") {
        "" => s(f, "name"),
        t => t,
    };
    let mut out = format!("File: {title} (ID: {id})\n");
    if is_canvas {
        out.push_str("Type: canvas\n");
    } else {
        out.push_str(&format!("Name: {}\n", s(f, "name")));
        out.push_str(&format!("Type: {}\n", s(f, "mimetype")));
    }
    if concise {
        return out;
    }
    out.push_str(&from_line(ctx, s(f, "user")).replacen("From:", "Shared by:", 1));
    if let Some(created) = f.get("created").and_then(Value::as_i64) {
        out.push_str(&format!(
            "Created: {}\n",
            tz::format_local(&ctx.caller_tz(), created)
        ));
    }
    if let Some(size) = f.get("size").and_then(Value::as_u64) {
        out.push_str(&format!("Size: {}\n", human_size(size)));
    }
    let channels: Vec<String> = f["channels"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| ctx.channel(c.as_str().unwrap_or("")))
                .map(|c| ctx.channel_label(&c))
                .collect()
        })
        .unwrap_or_default();
    if !channels.is_empty() {
        out.push_str(&format!("Channels: {}\n", channels.join(", ")));
    }
    let link = if is_canvas {
        format!("{}/docs/{}/{id}", ctx.base_url(), ctx.team_id())
    } else {
        format!(
            "{}/files/{}/{id}/{}",
            ctx.base_url(),
            s(f, "user"),
            s(f, "name")
        )
    };
    out.push_str(&format!("Permalink: [link]({link})\n"));
    let preview = file_text(f);
    if !preview.is_empty() {
        out.push_str(&format!("Preview: {}\n", truncate(&preview, 200)));
    }
    out
}

/// `slack_search_channels`.
pub fn search_channels(ctx: &Ctx, args: &Value) -> ToolResult {
    let terms = terms(&arg_strings(args, "keywords"));
    let nlq = arg_str(args, "natural_language_query").unwrap_or("");
    let types: Vec<String> = arg_str(args, "channel_types")
        .unwrap_or("public_channel")
        .split(',')
        .map(|x| x.trim().to_owned())
        .collect();
    let include_archived = arg_bool(args, "include_archived").unwrap_or(false);
    let concise = arg_str(args, "response_format") == Some("concise");
    let limit = arg_i64(args, "limit").unwrap_or(20).clamp(1, 20) as usize;
    let page: usize = arg_str(args, "cursor")
        .and_then(|c| parse_cursor(c, "CURRENT_PAGE"))
        .and_then(|p| p.parse().ok())
        .unwrap_or(1)
        .max(1);
    let mut hits: Vec<(usize, usize, Value)> = Vec::new();
    for (i, c) in ctx.values("channels").into_iter().enumerate() {
        let k = kind(&c);
        if k == "im" || k == "mpim" || !types.iter().any(|t| t == k) || !ctx.can_see(&c) {
            continue;
        }
        if b(&c, "is_archived") && !include_archived {
            continue;
        }
        let hay = format!(
            "{} {} {}",
            s(&c, "name").replace(['-', '_'], " ") + " " + s(&c, "name"),
            s(&c, "topic"),
            s(&c, "purpose")
        );
        if !matches_terms(&hay, &terms) {
            continue;
        }
        let name_hit = terms
            .iter()
            .filter(|t| !t.negate && s(&c, "name").to_lowercase().contains(&t.text))
            .count();
        hits.push((name_hit * 10 + score(&hay, &terms, nlq), i, c));
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let start = (page - 1) * limit;
    let page_hits: Vec<_> = hits.iter().skip(start).take(limit).collect();
    if page_hits.is_empty() {
        return Ok(no_results());
    }
    let mut out = format!("{HEADER}## Channels ({} results)\n", page_hits.len());
    for (i, (_, _, c)) in page_hits.iter().enumerate() {
        out.push_str(&format!("### Result {} of {}\n", i + 1, page_hits.len()));
        out.push_str(&format!("Name: #{}\n", s(c, "name")));
        if !concise {
            let creator = ctx
                .user(s(c, "creator"))
                .map(|u| real_name(&u))
                .unwrap_or_default();
            out.push_str(&format!("Creator: {creator} (<@{})\n", s(c, "creator")));
            if let Some(created) = c.get("created").and_then(Value::as_i64) {
                out.push_str(&format!(
                    "Created: {}\n",
                    tz::format_local(&ctx.caller_tz(), created)
                ));
            }
        }
        if !s(c, "topic").is_empty() && !concise {
            out.push_str(&format!("Topic: {}\n", s(c, "topic")));
        }
        out.push_str(&format!("Purpose: {}\n", s(c, "purpose")));
        out.push_str(&format!(
            "Permalink: [link]({}/archives/{})\n",
            ctx.base_url(),
            s(c, "id")
        ));
        if !concise {
            out.push_str(&format!("Is Archived: {}\n", b(c, "is_archived")));
            out.push_str(&format!("Channel Type: {}\n", kind(c)));
        }
        out.push_str("\n---\n\n");
    }
    Ok(json!({"results": out, "pagination_info": page_info(page, hits.len() > start + limit)}))
}

/// `slack_search_users`.
pub fn search_users(ctx: &Ctx, args: &Value) -> ToolResult {
    let terms = terms(&arg_strings(args, "keywords"));
    let nlq = arg_str(args, "natural_language_query").unwrap_or("");
    let concise = arg_str(args, "response_format") == Some("concise");
    let limit = arg_i64(args, "limit").unwrap_or(20).clamp(1, 20) as usize;
    let page: usize = arg_str(args, "cursor")
        .and_then(|c| parse_cursor(c, "CURRENT_PAGE"))
        .and_then(|p| p.parse().ok())
        .unwrap_or(1)
        .max(1);
    if terms.is_empty() && nlq.trim().is_empty() {
        return Err(err("invalid_arguments: `keywords` is required"));
    }
    let nlq_terms: Vec<Term> = if terms.is_empty() {
        nlq.split(|c: char| !c.is_alphanumeric() && c != '@' && c != '.')
            .filter(|w| w.len() >= 3)
            .map(|w| Term {
                text: w.to_lowercase(),
                negate: false,
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut hits: Vec<(usize, usize, Value)> = Vec::new();
    for (i, u) in ctx.values("users").into_iter().enumerate() {
        if b(&u, "deleted") {
            continue;
        }
        let hay = [
            "real_name",
            "display_name",
            "name",
            "email",
            "title",
            "department",
            "pronouns",
        ]
        .iter()
        .map(|k| s(&u, k))
        .collect::<Vec<_>>()
        .join(" ");
        if !matches_terms(&hay, &terms) {
            continue;
        }
        if !nlq_terms.is_empty()
            && !nlq_terms
                .iter()
                .any(|t| hay.to_lowercase().contains(&t.text))
        {
            continue;
        }
        hits.push((score(&hay, &terms, nlq), i, u));
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let start = (page - 1) * limit;
    let page_hits: Vec<_> = hits.iter().skip(start).take(limit).collect();
    if page_hits.is_empty() {
        return Ok(no_results());
    }
    let mut out = format!("{HEADER}## Users ({} results)\n", page_hits.len());
    for (i, (_, _, u)) in page_hits.iter().enumerate() {
        out.push_str(&format!("### Result {} of {}\n", i + 1, page_hits.len()));
        out.push_str(&format!("Name: {}\n", real_name(u)));
        out.push_str(&format!("User ID: {}\n", s(u, "id")));
        if !concise {
            out.push_str(&format!("Username: {}\n", s(u, "name")));
            out.push_str(&format!("Display Name: {}\n", display_name(u)));
        }
        out.push_str(&format!("Email: {}\n", s(u, "email")));
        out.push_str(&format!("Title: {}\n", s(u, "title")));
        if !concise {
            if !s(u, "department").is_empty() {
                out.push_str(&format!("Department: {}\n", s(u, "department")));
            }
            out.push_str(&format!("Timezone: {}\n", s(u, "tz")));
            if b(u, "is_bot") {
                out.push_str("Bot: Yes\n");
            }
        }
        out.push_str("\n---\n\n");
    }
    Ok(json!({"results": out, "pagination_info": page_info(page, hits.len() > start + limit)}))
}

/// `slack_search_emojis`.
pub fn search_emojis(ctx: &Ctx, args: &Value) -> ToolResult {
    let Some(query) = arg_str(args, "query") else {
        return Err(err("invalid_arguments: `query` is required"));
    };
    let terms: Vec<String> = query
        .split(',')
        .map(|t| t.trim().trim_matches(':').to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    let mut found = Vec::new();
    for e in ctx.values("emojis") {
        let name = s(&e, "name").to_lowercase();
        if terms.iter().any(|t| name.contains(t.as_str())) {
            found.push(e);
        }
        if found.len() == 200 {
            break;
        }
    }
    if found.is_empty() {
        return Ok(json!({"result": format!("No custom emojis found matching \"{query}\".")}));
    }
    let mut out = format!(
        "Found {} custom emojis matching \"{query}\":\n",
        found.len()
    );
    for e in &found {
        match s(e, "alias_for") {
            "" => out.push_str(&format!(":{}: - {}\n", s(e, "name"), s(e, "url"))),
            a => out.push_str(&format!(":{}: (alias for :{a}:)\n", s(e, "name"))),
        }
    }
    Ok(json!({"result": out}))
}
