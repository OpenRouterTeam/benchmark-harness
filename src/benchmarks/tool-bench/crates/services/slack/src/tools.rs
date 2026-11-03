//! Replica of the official Slack MCP server (`mcp.slack.com`) tools.

use serde_json::{Value, json};

use crate::{
    canvas, lists,
    model::{CALLER, Ctx, display_name, kind, reaction_count, real_name, ts_key},
    search, tz,
    util::{
        arg_bool, arg_i64, arg_str, arg_strings, b, cursor, human_size, parse_cursor, s, truncate,
    },
};

/// Ok: the JSON object the tool returns as text. Err: the tool error text.
pub type ToolResult = Result<Value, String>;

pub fn err(code: &str) -> String {
    format!("Error: {code}")
}

pub fn dispatch(ctx: &mut Ctx, name: &str, args: &Value) -> Option<ToolResult> {
    Some(match name {
        "slack_search_public" => search::search_messages(ctx, args, false),
        "slack_search_public_and_private" => search::search_messages(ctx, args, true),
        "slack_search_channels" => search::search_channels(ctx, args),
        "slack_search_users" => search::search_users(ctx, args),
        "slack_search_emojis" => search::search_emojis(ctx, args),
        "slack_read_user_profile" => read_user_profile(ctx, args),
        "slack_send_message" => send_message(ctx, args),
        "slack_send_message_draft" => send_message_draft(ctx, args),
        "slack_schedule_message" => schedule_message(ctx, args),
        "slack_read_channel" => read_channel(ctx, args),
        "slack_read_thread" => read_thread(ctx, args),
        "slack_create_conversation" => create_conversation(ctx, args),
        "slack_list_channel_members" => list_channel_members(ctx, args),
        "slack_list_user_channels" => list_user_channels(ctx, args),
        "slack_add_reaction" => add_reaction(ctx, args),
        "slack_get_reactions" => get_reactions(ctx, args),
        "slack_create_canvas" => canvas::create(ctx, args),
        "slack_read_canvas" => canvas::read(ctx, args),
        "slack_update_canvas" => canvas::update(ctx, args),
        "slack_read_file" => read_file(ctx, args),
        "slack_get_file_upload_url" => get_file_upload_url(ctx, args),
        "slack_complete_file_upload" => complete_file_upload(ctx, args),
        "slack_create_list" => lists::create(ctx, args),
        "slack_read_list" => lists::read(ctx, args),
        "slack_update_list" => lists::update(ctx, args),
        "slack_add_list_record" => lists::add_record(ctx, args),
        "slack_update_list_record" => lists::update_record(ctx, args),
        _ => return None,
    })
}

fn required<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    arg_str(args, key).ok_or_else(|| err(&format!("invalid_arguments: `{key}` is required")))
}

fn yes_no(v: bool) -> &'static str {
    if v { "Yes" } else { "No" }
}

fn read_user_profile(ctx: &mut Ctx, args: &Value) -> ToolResult {
    let id = arg_str(args, "user_id").unwrap_or(CALLER);
    let u = ctx.user(id).ok_or_else(|| err("user_not_found"))?;
    let team = ctx.team();
    let status = format!("{} {}", s(&u, "status_emoji"), s(&u, "status_text"));
    let status = status.trim_start();
    let mut out = String::new();
    out.push_str(&format!("User ID: {id}\n"));
    out.push_str(&format!("Username: {}\n", s(&u, "name")));
    out.push_str(&format!("Display Name: {}\n", s(&u, "display_name")));
    out.push_str(&format!("Real Name: {}\n", s(&u, "real_name")));
    if arg_str(args, "response_format") == Some("concise") {
        out.push_str(&format!("Title: {}\n", s(&u, "title")));
        out.push_str(&format!("Email: {}\n", s(&u, "email")));
        out.push_str(&format!("Timezone: {}\n", s(&u, "tz")));
        return Ok(json!({"result": out}));
    }
    out.push_str(&format!("Pronouns: {}\n", s(&u, "pronouns")));
    out.push_str(&format!("Title: {}\n", s(&u, "title")));
    out.push_str(&format!("Email: {}\n", s(&u, "email")));
    out.push_str(&format!("Organization Name: {}\n", s(&team, "name")));
    out.push_str(&format!("Phone: {}\n", s(&u, "phone")));
    out.push_str(&format!("Status: {status} \n"));
    out.push_str(&format!("Timezone: {}\n", s(&u, "tz")));
    if arg_bool(args, "include_locale").unwrap_or(false) {
        let locale = match s(&u, "locale") {
            "" => "en-US",
            l => l,
        };
        out.push_str(&format!("Locale: {locale}\n"));
    }
    out.push_str(&format!("Admin: {}\n", yes_no(b(&u, "is_admin"))));
    out.push_str(&format!("Owner: {}\n", yes_no(b(&u, "is_owner"))));
    out.push_str(&format!("Bot: {}\n", yes_no(b(&u, "is_bot"))));
    out.push_str(&format!("Restricted: {}\n", yes_no(b(&u, "is_restricted"))));
    if b(&u, "deleted") {
        out.push_str("Deactivated: Yes\n");
    }
    Ok(json!({"result": out}))
}

fn send_message(ctx: &mut Ctx, args: &Value) -> ToolResult {
    let channel = required(args, "channel_id")?.to_owned();
    let text = required(args, "message")?.to_owned();
    if text.chars().count() > 5000 * 8 {
        return Err(err("msg_too_long"));
    }
    let thread = arg_str(args, "thread_ts").map(str::to_owned);
    let broadcast = arg_bool(args, "reply_broadcast").unwrap_or(false);
    let msg = ctx
        .post_message(&channel, &text, thread.as_deref(), broadcast, Vec::new())
        .map_err(err)?;
    if let Some(draft) = arg_str(args, "draft_id") {
        ctx.remove("drafts", draft);
    }
    let link = ctx.message_permalink(&msg);
    let ch = ctx.channel(s(&msg, "channel")).unwrap_or(Value::Null);
    Ok(json!({
        "result": format!(
            "Message sent successfully to {} (ID: {}).\nMessage_ts: {}\n{}Permalink: [link]({link})\n",
            ctx.channel_label(&ch),
            s(&msg, "channel"),
            s(&msg, "ts"),
            match s(&msg, "thread_ts") { "" => String::new(), t => format!("Thread_ts: {t}\n") },
        ),
        "message_link": link,
    }))
}

fn send_message_draft(ctx: &mut Ctx, args: &Value) -> ToolResult {
    let channel_id = required(args, "channel_id")?.to_owned();
    let text = required(args, "message")?.to_owned();
    let channel = ctx.resolve_conversation(&channel_id).map_err(err)?;
    let cid = s(&channel, "id").to_owned();
    if !ctx.is_member(&channel, CALLER) {
        return Err(err("not_in_channel"));
    }
    let thread = arg_str(args, "thread_ts").map(str::to_owned);
    if let Some(t) = &thread
        && ctx.message(&cid, t).is_none()
    {
        return Err(err("thread_not_found"));
    }
    let exists = ctx
        .values("drafts")
        .iter()
        .any(|d| s(d, "channel") == cid && s(d, "thread_ts") == thread.as_deref().unwrap_or(""));
    if exists {
        return Err(err("attached_draft_exists"));
    }
    let id = ctx.new_id("draft", "Dr0");
    let mut draft =
        json!({"id": id, "channel": cid, "text": text, "user": CALLER, "created": ctx.now});
    if let Some(t) = &thread {
        draft["thread_ts"] = json!(t);
    }
    ctx.put("drafts", &id, draft);
    let link = format!("https://app.slack.com/client/{}/{cid}", ctx.team_id());
    Ok(json!({"channel_link": link, "draft_id": id}))
}

fn schedule_message(ctx: &mut Ctx, args: &Value) -> ToolResult {
    let channel_id = required(args, "channel_id")?.to_owned();
    let text = required(args, "message")?.to_owned();
    let post_at =
        arg_i64(args, "post_at").ok_or_else(|| err("invalid_arguments: `post_at` is required"))?;
    if post_at < ctx.now + 120 {
        return Err(err("time_in_past"));
    }
    if post_at > ctx.now + 120 * 86_400 {
        return Err(err("time_too_far"));
    }
    let channel = ctx.resolve_conversation(&channel_id).map_err(err)?;
    let cid = s(&channel, "id").to_owned();
    if !ctx.is_member(&channel, CALLER) {
        return Err(err("not_in_channel"));
    }
    if b(&channel, "is_ext_shared") {
        return Err(err("restricted_action"));
    }
    let thread = arg_str(args, "thread_ts").map(str::to_owned);
    if let Some(t) = &thread
        && ctx.message(&cid, t).is_none()
    {
        return Err(err("invalid_thread_ts"));
    }
    let id = ctx.new_id("scheduled", "Q0");
    let mut doc = json!({
        "id": id, "channel": cid, "text": text, "post_at": post_at,
        "date_created": ctx.now, "user": CALLER,
    });
    if let Some(t) = &thread {
        doc["thread_ts"] = json!(t);
        doc["reply_broadcast"] = json!(arg_bool(args, "reply_broadcast").unwrap_or(false));
    }
    ctx.put("scheduled_messages", &id, doc);
    Ok(json!({
        "result": format!(
            "Message scheduled successfully in {} (ID: {cid}).\nScheduled_message_id: {id}\nPost_at: {post_at} ({})\n",
            ctx.channel_label(&channel),
            tz::format_local(&ctx.caller_tz(), post_at)
        )
    }))
}

fn is_top_level(m: &Value) -> bool {
    let t = s(m, "thread_ts");
    t.is_empty() || t == s(m, "ts") || s(m, "subtype") == "thread_broadcast"
}

fn author_line(ctx: &Ctx, user_id: &str) -> String {
    match ctx.user(user_id) {
        Some(u) => format!("{} <{}> ({user_id})", display_name(&u), s(&u, "email")),
        None => user_id.to_owned(),
    }
}

fn reactions_line(m: &Value) -> Option<String> {
    let list = m["reactions"].as_array()?;
    if list.is_empty() {
        return None;
    }
    Some(
        list.iter()
            .map(|r| format!("{} ({})", s(r, "name"), reaction_count(r)))
            .collect::<Vec<_>>()
            .join(", "),
    )
}

fn files_line(ctx: &Ctx, m: &Value) -> Option<String> {
    let ids = m["files"].as_array()?;
    if ids.is_empty() {
        return None;
    }
    Some(
        ids.iter()
            .filter_map(Value::as_str)
            .map(
                |id| match ctx.get("files", id).or_else(|| ctx.get("canvases", id)) {
                    Some(f) => format!(
                        "{} (ID: {id}, {}, {})",
                        match s(&f, "name") {
                            "" => s(&f, "title"),
                            n => n,
                        },
                        match s(&f, "mimetype") {
                            "" => "application/vnd.slack-docs",
                            m => m,
                        },
                        human_size(f.get("size").and_then(Value::as_u64).unwrap_or(0))
                    ),
                    None => format!("(ID: {id})"),
                },
            )
            .collect::<Vec<_>>()
            .join(", "),
    )
}

fn read_channel(ctx: &mut Ctx, args: &Value) -> ToolResult {
    let channel_id = required(args, "channel_id")?;
    let channel = ctx.resolve_conversation(channel_id).map_err(err)?;
    let cid = s(&channel, "id").to_owned();
    let limit = arg_i64(args, "limit").unwrap_or(100).clamp(1, 100) as usize;
    let concise = arg_str(args, "response_format") == Some("concise");
    let oldest = arg_str(args, "oldest").map(ts_key);
    let latest = arg_str(args, "latest").map(ts_key);
    let from = match arg_str(args, "cursor") {
        Some(c) => Some(
            parse_cursor(c, "next_ts")
                .and_then(|raw| {
                    (raw.len() > 6)
                        .then(|| format!("{}.{}", &raw[..raw.len() - 6], &raw[raw.len() - 6..]))
                })
                .ok_or_else(|| err("invalid_cursor"))?,
        ),
        None => None,
    };
    let mut msgs: Vec<Value> = ctx
        .messages_in(&cid)
        .into_iter()
        .filter(is_top_level)
        .filter(|m| {
            let k = ts_key(s(m, "ts"));
            oldest.is_none_or(|o| k > o) && latest.is_none_or(|l| k < l)
        })
        .collect();
    msgs.reverse();
    if let Some(f) = &from {
        let fk = ts_key(f);
        msgs.retain(|m| ts_key(s(m, "ts")) <= fk);
    }
    let more = msgs.len() > limit;
    let next = msgs.get(limit).map(|m| s(m, "ts").replace('.', ""));
    msgs.truncate(limit);
    let mut out = format!("Channel: {} ({cid})\n", ctx.channel_label(&channel));
    if msgs.is_empty() {
        out.push_str("\nNo messages found.");
    }
    let blocks: Vec<String> = msgs
        .iter()
        .map(|m| {
            let mut block = format!(
                "=== Message from {} at {} === \nMessage TS: {}\n{}",
                author_line(ctx, s(m, "user")),
                ctx.time(s(m, "ts")),
                s(m, "ts"),
                s(m, "text")
            );
            if !concise {
                let replies = ctx.replies(&cid, s(m, "ts"));
                if let Some(last) = replies.last() {
                    block.push_str(&format!(
                        "\nThread: {} replies (latest: {})",
                        replies.len(),
                        ctx.time(s(last, "ts"))
                    ));
                }
                if let Some(r) = reactions_line(m) {
                    block.push_str(&format!("\nReactions: {r}"));
                }
            }
            if let Some(f) = files_line(ctx, m) {
                block.push_str(&format!("\nFiles: {f}"));
            }
            block
        })
        .collect();
    if !blocks.is_empty() {
        out.push('\n');
        out.push_str(&blocks.join("\n\n"));
    }
    let info = match (more, next) {
        (true, Some(n)) => format!(
            "There are more messages available. To view the next page, use cursor: `{}`\n",
            cursor("next_ts", n).trim_end_matches('=')
        ),
        _ => "There are no more messages available.\n".to_owned(),
    };
    Ok(json!({"messages": out, "pagination_info": info}))
}

fn read_thread(ctx: &mut Ctx, args: &Value) -> ToolResult {
    let channel_id = required(args, "channel_id")?;
    let ts = required(args, "message_ts")?.to_owned();
    let channel = ctx.resolve_conversation(channel_id).map_err(err)?;
    let cid = s(&channel, "id").to_owned();
    let mut parent = ctx
        .message(&cid, &ts)
        .ok_or_else(|| err("thread_not_found"))?;
    if !s(&parent, "thread_ts").is_empty() && s(&parent, "thread_ts") != ts {
        // A reply's ts: Slack returns the whole thread.
        parent = ctx
            .message(&cid, s(&parent, "thread_ts"))
            .ok_or_else(|| err("thread_not_found"))?;
    }
    let concise = arg_str(args, "response_format") == Some("concise");
    let limit = arg_i64(args, "limit").unwrap_or(100).clamp(1, 1000) as usize;
    let oldest = arg_str(args, "oldest").map(ts_key);
    let latest = arg_str(args, "latest").map(ts_key);
    let offset: usize = arg_str(args, "cursor")
        .and_then(|c| parse_cursor(c, "offset"))
        .and_then(|o| o.parse().ok())
        .unwrap_or(0);
    let all: Vec<Value> = ctx
        .replies(&cid, s(&parent, "ts"))
        .into_iter()
        .filter(|m| {
            let k = ts_key(s(m, "ts"));
            oldest.is_none_or(|o| k >= o) && latest.is_none_or(|l| k <= l)
        })
        .collect();
    let page: Vec<&Value> = all.iter().skip(offset).take(limit).collect();
    let render = |m: &Value| -> String {
        let mut x = format!(
            "From: {}\nTime: {}\nMessage TS: {}\n{}\n",
            author_line(ctx, s(m, "user")),
            ctx.time(s(m, "ts")),
            s(m, "ts"),
            s(m, "text")
        );
        if !concise && let Some(r) = reactions_line(m) {
            x.push_str(&format!("Reactions: {r}\n"));
        }
        if let Some(f) = files_line(ctx, m) {
            x.push_str(&format!("Files: {f}\n"));
        }
        x
    };
    let mut out = String::from("=== THREAD PARENT MESSAGE ===\n");
    out.push_str(&render(&parent));
    out.push_str(&format!(
        "\n=== THREAD REPLIES ({} total) ===\n",
        page.len()
    ));
    for (i, m) in page.iter().enumerate() {
        out.push_str(&format!("\n--- Reply {} of {} ---\n", i + 1, page.len()));
        out.push_str(&render(m));
    }
    let info = if all.len() > offset + limit {
        format!(
            "There are more replies available. To view the next page, use cursor: `{}`\n",
            cursor("offset", offset + limit)
        )
    } else {
        "There are no more messages in this thread.\n".to_owned()
    };
    Ok(json!({"messages": out, "pagination_info": info}))
}

fn sanitize_channel_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().trim_start_matches('#').chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
        } else if c.is_whitespace() || c == '.' {
            out.push('-');
        }
    }
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    out.trim_matches('-').chars().take(80).collect()
}

fn create_conversation(ctx: &mut Ctx, args: &Value) -> ToolResult {
    let user_ids = arg_strings(args, "user_ids");
    for u in &user_ids {
        let user = ctx
            .user(u)
            .ok_or_else(|| err(&format!("user_not_found: {u}")))?;
        if b(&user, "deleted") {
            return Err(err(&format!("user_not_found: {u}")));
        }
    }
    if let Some(name) = arg_str(args, "channel_name") {
        let name = sanitize_channel_name(name);
        if name.is_empty() {
            return Err(err("invalid_arguments: invalid channel name"));
        }
        if ctx
            .values("channels")
            .iter()
            .any(|c| !b(c, "is_im") && !b(c, "is_mpim") && s(c, "name") == name)
        {
            return Err(err("name_taken"));
        }
        if user_ids.len() > 1000 {
            return Err(err("too_many_users"));
        }
        let private = arg_bool(args, "is_private").unwrap_or(false);
        let id = ctx.new_id("channel", "C0");
        let mut members = vec![json!(CALLER)];
        for u in &user_ids {
            if u != CALLER {
                members.push(json!(u));
            }
        }
        let doc = json!({
            "id": id, "name": name, "is_private": private, "is_archived": false,
            "created": ctx.now, "creator": CALLER, "topic": "", "purpose": "",
            "members": members,
        });
        ctx.put("channels", &id, doc);
        return Ok(json!({
            "channel_id": id,
            "result": format!(
                "Created {} channel #{name} (ID: {id}){}.\nPermalink: [link]({}/archives/{id})\n",
                if private { "private" } else { "public" },
                if user_ids.is_empty() { String::new() } else { format!(" and invited {} user(s)", user_ids.iter().filter(|u| *u != CALLER).count()) },
                ctx.base_url()
            ),
        }));
    }
    let others: Vec<String> = user_ids.into_iter().filter(|u| u != CALLER).collect();
    if others.is_empty() {
        return Err(err("invalid_arguments: provide channel_name or user_ids"));
    }
    if others.len() > 8 {
        return Err(err("too_many_users"));
    }
    for u in &others {
        if ctx.user(u).is_some_and(|x| b(&x, "is_bot")) {
            return Err(err("cannot_dm_bot"));
        }
    }
    let mut members: Vec<String> = vec![CALLER.to_owned()];
    members.extend(others.iter().cloned());
    if others.len() == 1 {
        if let Some(existing) = ctx
            .values("channels")
            .into_iter()
            .find(|c| b(c, "is_im") && s(c, "user") == others[0])
        {
            return Ok(json!({
                "channel_id": s(&existing, "id"),
                "result": format!("DM with {} already exists (ID: {}).\n", others[0], s(&existing, "id")),
            }));
        }
        let id = ctx.new_id("im", "D0");
        ctx.put(
            "channels",
            &id,
            json!({"id": id, "is_im": true, "user": others[0], "created": ctx.now, "members": members}),
        );
        return Ok(
            json!({"channel_id": id, "result": format!("Opened DM with {} (ID: {id}).\n", others[0])}),
        );
    }
    let mut sorted = members.clone();
    sorted.sort();
    if let Some(existing) = ctx.values("channels").into_iter().find(|c| {
        b(c, "is_mpim") && {
            let mut m: Vec<String> = c["members"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            m.sort();
            m == sorted
        }
    }) {
        return Ok(json!({
            "channel_id": s(&existing, "id"),
            "result": format!("Group DM already exists (ID: {}).\n", s(&existing, "id")),
        }));
    }
    let id = ctx.new_id("mpim", "G0");
    let names: Vec<String> = members
        .iter()
        .map(|u| {
            ctx.user(u)
                .map(|x| s(&x, "name").to_owned())
                .unwrap_or_default()
        })
        .collect();
    let name = format!("mpdm-{}-1", names.join("--"));
    ctx.put(
        "channels",
        &id,
        json!({"id": id, "is_mpim": true, "is_private": true, "name": name, "created": ctx.now, "creator": CALLER, "members": members}),
    );
    Ok(
        json!({"channel_id": id, "result": format!("Opened group DM with {} users (ID: {id}).\n", others.len())}),
    )
}

fn list_channel_members(ctx: &mut Ctx, args: &Value) -> ToolResult {
    let channel_id = required(args, "channel_id")?;
    let channel = ctx.resolve_conversation(channel_id).map_err(err)?;
    if b(&channel, "is_im") {
        return Err(err("method_not_supported_for_channel_type"));
    }
    let format = arg_str(args, "response_format").unwrap_or("detailed");
    let limit = arg_i64(args, "limit").unwrap_or(30).clamp(1, 30) as usize;
    let offset: usize = arg_str(args, "cursor")
        .and_then(|c| parse_cursor(c, "offset"))
        .and_then(|o| o.parse().ok())
        .unwrap_or(0);
    let include_deleted = arg_bool(args, "include_deleted").unwrap_or(false);
    let include_bots = arg_bool(args, "include_bots").unwrap_or(false);
    let ids: Vec<String> = channel["members"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let ids: Vec<String> = if format == "ids_only" {
        ids
    } else {
        ids.into_iter()
            .filter(|id| {
                ctx.user(id).is_some_and(|u| {
                    (include_deleted || !b(&u, "deleted")) && (include_bots || !b(&u, "is_bot"))
                })
            })
            .collect()
    };
    let page: Vec<&String> = ids.iter().skip(offset).take(limit).collect();
    let mut out = format!(
        "Members of {} ({}): {} total\n\n",
        ctx.channel_label(&channel),
        s(&channel, "id"),
        ids.len()
    );
    for id in &page {
        match format {
            "ids_only" => out.push_str(&format!("{id}\n")),
            "concise" => {
                let u = ctx.user(id).unwrap_or(Value::Null);
                out.push_str(&format!(
                    "@{} ({}) - {id}\n",
                    s(&u, "name"),
                    display_name(&u)
                ));
            }
            _ => {
                let u = ctx.user(id).unwrap_or(Value::Null);
                out.push_str(&format!(
                    "User ID: {id}\nUsername: {}\nDisplay Name: {}\nReal Name: {}\nTitle: {}\nEmail: {}\nTimezone: {}\n\n",
                    s(&u, "name"),
                    s(&u, "display_name"),
                    real_name(&u),
                    s(&u, "title"),
                    s(&u, "email"),
                    s(&u, "tz")
                ));
            }
        }
    }
    let info = if ids.len() > offset + limit {
        format!(
            "There are more members available. To view the next page, use cursor: `{}`\n",
            cursor("offset", offset + limit)
        )
    } else {
        "End of results - No more pages available.\n".to_owned()
    };
    Ok(json!({"members": out, "pagination_info": info}))
}

fn list_user_channels(ctx: &mut Ctx, args: &Value) -> ToolResult {
    let types: Vec<String> = arg_str(args, "types")
        .unwrap_or("public_channel,private_channel")
        .split(',')
        .map(|t| t.trim().to_owned())
        .collect();
    for t in &types {
        if !["public_channel", "private_channel", "im", "mpim"].contains(&t.as_str()) {
            return Err(err(&format!("invalid_types: {t}")));
        }
    }
    if let Some(team) = arg_str(args, "team_id")
        && team != ctx.team_id()
    {
        return Err(err("team_not_found"));
    }
    let prefix = arg_str(args, "name_prefix").map(str::to_lowercase);
    let exclude_archived = arg_bool(args, "exclude_archived").unwrap_or(false);
    let limit = arg_i64(args, "limit").unwrap_or(50).clamp(1, 200) as usize;
    let offset: usize = if prefix.is_some() {
        0
    } else {
        arg_str(args, "cursor")
            .and_then(|c| parse_cursor(c, "offset"))
            .and_then(|o| o.parse().ok())
            .unwrap_or(0)
    };
    let format = arg_str(args, "format").unwrap_or("full");
    let label = |c: &Value| -> String {
        if b(c, "is_im") {
            ctx.user(s(c, "user"))
                .map(|u| display_name(&u))
                .unwrap_or_default()
        } else {
            s(c, "name").to_owned()
        }
    };
    let chans: Vec<Value> = ctx
        .values("channels")
        .into_iter()
        .filter(|c| ctx.is_member(c, CALLER))
        .filter(|c| types.iter().any(|t| t == kind(c)))
        .filter(|c| !(exclude_archived && b(c, "is_archived")))
        .filter(|c| {
            prefix
                .as_ref()
                .is_none_or(|p| label(c).to_lowercase().starts_with(p.as_str()))
        })
        .collect();
    let page: Vec<&Value> = chans.iter().skip(offset).take(limit).collect();
    let mut out = format!("Found {} channels\n\n", chans.len());
    for c in &page {
        match format {
            "ids_only" => out.push_str(&format!("{}\n", s(c, "id"))),
            "names_only" => out.push_str(&format!("{}\n", label(c))),
            _ => {
                let k = kind(c);
                let mut line = match k {
                    "im" => format!("- DM with {} (ID: {})", label(c), s(c, "id")),
                    "mpim" => {
                        let names: Vec<String> = c["members"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|u| ctx.user(u.as_str().unwrap_or("")))
                                    .map(|u| display_name(&u))
                                    .collect()
                            })
                            .unwrap_or_default();
                        format!("- Group DM with {} (ID: {})", names.join(", "), s(c, "id"))
                    }
                    _ => format!("- #{} (ID: {})", s(c, "name"), s(c, "id")),
                };
                line.push_str(&format!(" | Type: {k}"));
                if k != "im" && k != "mpim" {
                    line.push_str(&format!(
                        " | Members: {}",
                        c["members"].as_array().map_or(0, Vec::len)
                    ));
                    if !s(c, "topic").is_empty() {
                        line.push_str(&format!(" | Topic: {}", s(c, "topic")));
                    }
                    if !s(c, "purpose").is_empty() {
                        line.push_str(&format!(" | Purpose: {}", s(c, "purpose")));
                    }
                }
                if b(c, "is_archived") {
                    line.push_str(" | Archived");
                }
                out.push_str(&line);
                out.push('\n');
            }
        }
    }
    let info = if chans.len() > offset + limit {
        format!(
            "There are more channels available. To view the next page, use cursor: `{}`\n",
            cursor("offset", offset + limit)
        )
    } else {
        "End of results - No more pages available.\n".to_owned()
    };
    Ok(json!({"channels": out, "pagination_info": info}))
}

fn add_reaction(ctx: &mut Ctx, args: &Value) -> ToolResult {
    let channel = required(args, "channel_id")?.to_owned();
    let ts = required(args, "message_ts")?.to_owned();
    let emoji = required(args, "emoji")?.to_owned();
    let msg = ctx.add_reaction(&channel, &ts, &emoji).map_err(err)?;
    let name = emoji.trim().trim_matches(':');
    Ok(json!({
        "result": format!(
            "Added reaction :{name}: to message {} in channel {}.\n",
            s(&msg, "ts"),
            s(&msg, "channel")
        )
    }))
}

fn get_reactions(ctx: &mut Ctx, args: &Value) -> ToolResult {
    let channel_id = required(args, "channel_id")?;
    let ts = required(args, "message_ts")?;
    let channel = ctx.resolve_conversation(channel_id).map_err(err)?;
    let msg = ctx
        .message(s(&channel, "id"), ts)
        .ok_or_else(|| err("message_not_found"))?;
    let list = msg["reactions"].as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        return Ok(json!({"result": format!("No reactions on message {ts}.\n")}));
    }
    let mut out = format!(
        "Reactions on message {ts} in {}:\n",
        ctx.channel_label(&channel)
    );
    for r in &list {
        let users: Vec<String> = r["users"]
            .as_array()
            .map(|a| {
                a.iter()
                    .take(50)
                    .filter_map(Value::as_str)
                    .map(|u| match ctx.user(u) {
                        Some(x) => format!("{} ({u})", display_name(&x)),
                        None => u.to_owned(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.push_str(&format!(
            "- :{}: ({}) - {}\n",
            s(r, "name"),
            reaction_count(r),
            users.join(", ")
        ));
    }
    Ok(json!({"result": out}))
}

fn file_suffix(id: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in id.bytes() {
        h ^= u64::from(byte);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn read_file(ctx: &mut Ctx, args: &Value) -> ToolResult {
    let id = required(args, "file_id")?.to_owned();
    if let Some(c) = ctx.get("canvases", &id) {
        let md = canvas::markdown(&c);
        let suffix = file_suffix(&id);
        return Ok(json!({
            "file_id": id,
            "title": s(&c, "title"),
            "mimeType": "text/markdown",
            "content": format!("<file_content_{suffix}>\n{md}\n</file_content_{suffix}>"),
        }));
    }
    if let Some(l) = ctx.get("lists", &id) {
        let suffix = file_suffix(&id);
        let table = lists::render_table(ctx, &l, 0, 100).0;
        return Ok(json!({
            "file_id": id,
            "title": s(&l, "name"),
            "mimeType": "application/vnd.slack-list",
            "content": format!("<file_content_{suffix}>\n{table}\n</file_content_{suffix}>"),
        }));
    }
    let f = ctx.get("files", &id).ok_or_else(|| err("file_not_found"))?;
    if s(&f, "upload_state") == "pending" {
        return Err(err("file_not_found"));
    }
    let visible = s(&f, "user") == CALLER
        || f["channels"].as_array().is_some_and(|a| {
            a.iter().any(|c| {
                ctx.channel(c.as_str().unwrap_or(""))
                    .is_some_and(|c| ctx.can_see(&c))
            })
        });
    if !visible {
        return Err(err("file_not_found"));
    }
    if f.get("size").and_then(Value::as_u64).unwrap_or(0) > 10 * 1024 * 1024 {
        return Err(err("file_too_large"));
    }
    let mime = s(&f, "mimetype").to_owned();
    if let Some(data) = f.get("content_base64").and_then(Value::as_str) {
        return Ok(json!({
            "file_id": id, "name": s(&f, "name"), "mimeType": mime, "encoding": "base64", "data": data,
        }));
    }
    let suffix = file_suffix(&id);
    Ok(json!({
        "file_id": id,
        "name": s(&f, "name"),
        "title": s(&f, "title"),
        "mimeType": mime,
        "content": format!("<file_content_{suffix}>\n{}\n</file_content_{suffix}>", s(&f, "content")),
    }))
}

fn get_file_upload_url(ctx: &mut Ctx, args: &Value) -> ToolResult {
    let filename = required(args, "filename")?.to_owned();
    let length = arg_i64(args, "content_length").unwrap_or(0);
    if length < 1 {
        return Err(err("invalid_arguments: content_length must be at least 1"));
    }
    if length > 1024 * 1024 * 1024 {
        return Err(err("file_too_large"));
    }
    let id = ctx.new_id("file", "F0");
    let token = ctx.st.ids.next_prefixed("upload_token", "", 24);
    let mime = guess_mime(&filename);
    let mut doc = json!({
        "id": id, "name": filename, "title": filename, "mimetype": mime,
        "size": length, "user": CALLER, "created": ctx.now, "channels": [],
        "upload_state": "pending", "upload_token": token,
    });
    if let Some(t) = arg_str(args, "snippet_type") {
        doc["filetype"] = json!(t);
    }
    if let Some(a) = arg_str(args, "alt_txt") {
        doc["alt_txt"] = json!(a);
    }
    ctx.put("files", &id, doc);
    Ok(json!({
        "upload_url": format!("https://files.slack.com/upload/v1/{token}"),
        "file_id": id,
    }))
}

pub fn guess_mime(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "txt" | "log" => "text/plain",
        "md" => "text/markdown",
        "csv" => "text/csv",
        "json" => "application/json",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "py" => "text/x-python",
        "js" => "text/javascript",
        "html" => "text/html",
        _ => "application/octet-stream",
    }
}

fn complete_file_upload(ctx: &mut Ctx, args: &Value) -> ToolResult {
    let id = required(args, "file_id")?.to_owned();
    let mut f = ctx.get("files", &id).ok_or_else(|| err("file_not_found"))?;
    if s(&f, "upload_state") != "pending" && s(&f, "upload_state") != "uploaded" {
        return Err(err("file_not_found"));
    }
    if s(&f, "upload_state") == "pending" {
        return Err(err(
            "file_upload_not_found: the file bytes were never uploaded to the upload URL",
        ));
    }
    if let Some(t) = arg_str(args, "title") {
        f["title"] = json!(t);
    }
    let mut shared_in = None;
    if let Some(channel_id) = arg_str(args, "channel_id") {
        if !(channel_id.starts_with('C')
            || channel_id.starts_with('D')
            || channel_id.starts_with('G'))
        {
            return Err(err(
                "invalid_channel: channel_id must be an encoded channel ID",
            ));
        }
        let comment = arg_str(args, "initial_comment").unwrap_or("").to_owned();
        let thread = arg_str(args, "thread_ts").map(str::to_owned);
        let msg = ctx
            .post_message(
                channel_id,
                &comment,
                thread.as_deref(),
                false,
                vec![id.clone()],
            )
            .map_err(err)?;
        f["channels"] = json!([s(&msg, "channel")]);
        shared_in = Some(s(&msg, "channel").to_owned());
    }
    f["upload_state"] = json!("complete");
    f.as_object_mut().expect("file").remove("upload_token");
    ctx.put("files", &id, f.clone());
    let link = format!("{}/files/{CALLER}/{id}/{}", ctx.base_url(), s(&f, "name"));
    Ok(json!({
        "file_id": id,
        "title": s(&f, "title"),
        "permalink": link,
        "result": match shared_in {
            Some(c) => format!("File uploaded and shared in {c}.\n"),
            None => "File uploaded (not shared to any channel).\n".to_owned(),
        },
    }))
}

/// Text preview used by the Web API `files.info`.
pub fn file_preview(f: &Value) -> String {
    truncate(s(f, "content"), 200)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_names_are_sanitized() {
        assert_eq!(
            sanitize_channel_name("#Project X  Launch!"),
            "project-x-launch"
        );
    }

    #[test]
    fn ts_secs_parses() {
        assert_eq!(crate::model::ts_secs("1.5"), 1);
    }
}
